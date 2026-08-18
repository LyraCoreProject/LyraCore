//! Pure creature-AI geometry — flee / fear-flee / chase / return / wander step math — plus the AI
//! tuning constants they read. No `ReducerContext`, no tables: every fn here is a pure function of its
//! arguments, which is what makes them unit-testable (the tests live at the bottom of this file). The
//! `tick` pass pipeline calls these to compute each move leg. [pure]

// ===========================================================================================
//  Flee-at-low-health (vanilla creature AI) [pure helpers]
// ===========================================================================================

/// HP percentage at/below which a meleeing creature breaks off and runs (vanilla mobs flee around
/// ~15% HP). Integer percentage avoids float threshold drift.
const FLEE_HEALTH_PCT: u32 = 15;

/// Social aggro (vanilla pack behavior): same-faction creatures within this radius (yards) of a
/// creature that just aggroed will assist — join the fight against that creature's target, even ones
/// the player never got close enough to aggro directly. Measured from the AGGROER, not the player.
const ASSIST_RADIUS: f32 = 10.0;

/// True iff `(ax,ay)` and `(bx,by)` are within `ASSIST_RADIUS` of each other (squared-distance
/// compare, `<=` so the boundary assists). The assist scan's geometry check, extracted pure so it's
/// unit-testable without a `ReducerContext`. Pure — unit-tested.
pub fn within_assist_radius(ax: f32, ay: f32, bx: f32, by: f32) -> bool {
    let (dx, dy) = (ax - bx, ay - by);
    dx * dx + dy * dy <= ASSIST_RADIUS * ASSIST_RADIUS
}

/// Is a unit alive (`health > 0`) AND below `pct`% of its max health? Integer cross-multiply
/// (`health * 100 < max_health * pct`) avoids float rounding and a divide. The products are widened
/// before multiplying so custom/load-test creatures near the `u32` health ceiling remain exact.
/// Shared by `should_flee` and the creature spell rotation's SELF_HP_BELOW_PCT condition (rank 20 —
/// "heal when low"). Pure — unit-tested.
pub fn hp_pct_below(health: u32, max_health: u32, pct: u32) -> bool {
    health > 0 && (health as u64) * 100 < (max_health as u64) * (pct as u64)
}

/// Should this creature flee? True only when it's alive AND below the flee threshold. Vanilla creature
/// AI: a mob in melee that drops to ~15% HP breaks off and runs. Pure — unit-tested.
pub fn should_flee(health: u32, max_health: u32) -> bool {
    hp_pct_below(health, max_health, FLEE_HEALTH_PCT)
}

/// Is this creature TYPE eligible to flee at low HP? Fleeing is SELECTIVE in vanilla, not universal:
/// HUMANOIDS rout to call for help, while BEASTS (wolves/boars), undead, elementals, etc. fight to the
/// death. The rout once fled EVERY near-dead creature, so a Northshire wolf (BEAST) ran away and
/// dropped combat ("not all enemies should flee"). This is the minimal heuristic gate (a per-template
/// `flags_extra` NO_FLEE flag from the DBC is the fuller fix). `creature_type` is the template column the
/// importer loads verbatim. Pure — unit-tested. [creatures]
pub fn flee_eligible(creature_type: u8) -> bool {
    creature_type == crate::spell::HUMANOID_TYPE
}

/// How long a rout runs (ms) before the creature turns and fights again. Seeded at cmangos'
/// `CreatureFamilyFleeDelay` default; a calibration knob for live observation.
pub(crate) const ROUT_DURATION_MS: u32 = 10_000;

/// Is a rout still running, given the wall-clock ms its window closes at? `0` is the never-started
/// sentinel, so it is never open; a close time at or before `now_ms` means the rout is over AND spent
/// (the same field is what forbids a second rout). Pure — unit-tested.
pub fn rout_window_open(now_ms: u32, rout_ends_ms: u32) -> bool {
    rout_ends_ms != 0 && now_ms < rout_ends_ms
}

/// May a rout START now? Only for a flee-eligible creature (`eligible` = the HP threshold plus the
/// per-type gate) whose engagement has never stamped a window. A spent clock — open or long closed —
/// blocks every later rout in that engagement, which is what makes the rout once-per-engagement.
/// Pure — unit-tested.
pub fn may_start_rout(eligible: bool, rout_ends_ms: u32) -> bool {
    eligible && rout_ends_ms == 0
}

/// The wall-clock ms at which a rout stamped at `now_ms` closes. Never 0: that value is the
/// never-started sentinel, so a window that wraps the u32 clock onto it would hand out a second rout.
/// Pure — unit-tested.
pub fn rout_close_ms(now_ms: u32) -> u32 {
    now_ms.wrapping_add(ROUT_DURATION_MS).max(1)
}

/// The wall-clock ms at which a leash refreshed at `now_ms` expires. Never 0: that is the
/// never-refreshed sentinel, so a deadline wrapping the u32 clock onto it would re-seed the row.
/// Pure — unit-tested.
pub fn pursuit_deadline_ms(now_ms: u32) -> u32 {
    now_ms.wrapping_add(crate::combat::PURSUIT_WINDOW_MS).max(1)
}

/// Is the target past the ABSOLUTE distance backstop from the creature? The one raw distance that still
/// ends a fight, so no engagement can persist across a zone. Pure — unit-tested.
pub fn beyond_combat_backstop(dist_sq: f32) -> bool {
    dist_sq > crate::combat::COMBAT_BACKSTOP_SQ
}

/// Should this engagement EVADE? Only on an expired pursuit deadline AND a target outside
/// `combat::LEASH_RADIUS_SQ` of the position remembered at the last damage exchange — distance alone no
/// longer ends a fight (short of [`beyond_combat_backstop`]). `0` is the never-refreshed sentinel and
/// never evades: an armed row must survive until the leash pass stamps its clock. Pure — unit-tested.
pub fn should_evade(now_ms: u32, pursuit_ends_ms: u32, target_dist_sq: f32) -> bool {
    pursuit_ends_ms != 0
        && now_ms >= pursuit_ends_ms
        && target_dist_sq > crate::combat::LEASH_RADIUS_SQ
}

/// cmangos "wounded slowdown" — the mechanic that makes a fleeing (definitionally low-HP) mob
/// CATCHABLE. A creature under 30% HP moves slower: factor `1 − ((30 − min(hp%,30)) × 1.67)/100`,
/// floored ~0.5 (×1.0 at ≥30% HP, ×0.75 at 15%, ×0.67 at 10%, ~×0.5 near death). Applied to the flee
/// leg by the cycle's rout phase. Without it a fleer ran at the flat 7.0 yd/s player-run constant — exact parity,
/// so the chase gap never closed and the mob was uncatchable.
/// Pure — unit-tested. [creatures]
pub fn wounded_slow_factor(health: u32, max_health: u32) -> f32 {
    if max_health == 0 {
        return 1.0;
    }
    let hp_pct = (health as f32 / max_health as f32) * 100.0;
    let capped = hp_pct.min(30.0);
    (1.0 - ((30.0 - capped) * 1.67) / 100.0).max(0.499)
}

// ===========================================================================================
//  Proximity aggro radius (vanilla level-scaled) [pure helper]
// ===========================================================================================

/// Aggro radius (yards) at EQUAL level — vanilla's ~20-yard "notice you" distance. The level term
/// adjusts ±1 yd per level difference around this base.
const BASE_AGGRO_RADIUS: f32 = 20.0;
/// A hostile creature always aggroes within this floor (yards) — until it's GREY, which zeroes it.
const MIN_AGGRO_RADIUS: f32 = 5.0;
/// Cap so a much-higher-level creature has a large but bounded aggro radius.
const MAX_AGGRO_RADIUS: f32 = 45.0;
/// "Grey" gap: when the player is this many levels (or more) ABOVE the creature, the mob is trivial and
/// does NOT initiate proximity aggro (a high-level player isn't swarmed walking through a low zone).
const GREY_LEVEL_DIFF: i32 = 8;

/// Is this a position the world can actually hold? Guards the creature-movement writes the way
/// `movement_update` already guards the player one.
///
/// A non-finite coordinate is not merely wrong, it is INVISIBLE: `grid_cell` casts `(MAP_COORD_MAX -
/// inf) / 50` to `i32::MIN`, so the entity lands in no active cell and the creature tick stops
/// visiting it entirely — it never leashes, never disengages, never returns home. But `tick_melee`
/// iterates MELEE ROWS, not cells, so it keeps swinging. The result is an attacker that cannot be
/// shaken off by any means the game offers: 109 such creatures were found live on 2026-07-29, most
/// of them locked onto one player, which is what made a GM undyingly mobbed.
///
/// It also breaks diagnosis: infinity serialises as JSON `null`, so `spacetime sql` fails to decode
/// ANY query over the table with "invalid type: null, expected f32" — the entity table becomes
/// unreadable, exactly when you most need to read it. Pure — unit-tested. [pure]
pub fn finite_point(x: f32, y: f32, z: f32) -> bool {
    x.is_finite() && y.is_finite() && z.is_finite()
}

/// Is this player a proximity-aggro CANDIDATE at all, before any range maths?
///
/// A dead player obviously is not. Neither is a GM in **godmode**: mangos makes mobs ignore a GM
/// outright, and the reason is exactly the failure this fixes. A godmoded GM never dies, so the
/// normal resolution of a pull — you die, they disengage and leash home — never happens. Creatures
/// therefore only ever ACCUMULATE on them: every mob that wanders inside the ~20 yd radius engages
/// and stays engaged forever. A GM standing in Northshire during a 300-bot load test collected
/// **103 simultaneous attackers**, which is every creature in the valley.
///
/// Note what this is NOT: the radius is fine. It is `BASE_AGGRO_RADIUS + (creature - player) level`,
/// so a level-3 player draws level 1-6 fauna from 18-23 yd — vanilla-normal. Shrinking that would
/// break ordinary aggro everywhere and would not have stopped the pile-on, because the pile-on came
/// from accumulation over time, not from reach. Pure — unit-tested. [pure]
pub fn is_aggro_candidate(dead: bool, godmode: bool) -> bool {
    !dead && !godmode
}

/// Vanilla-style proximity-aggro radius (yards) for a creature noticing a player. An explicit
/// per-template override (`template_aggro_range > 0`, e.g. the tuned demo chicken's 8) WINS and is used
/// verbatim. Otherwise the radius scales with the level difference around `BASE_AGGRO_RADIUS`: a
/// higher-level creature notices you from farther, a lower-level one only up close, clamped to
/// `[MIN, MAX]`. A creature that is GREY to the player (player ≥ `GREY_LEVEL_DIFF` levels above it) does
/// NOT initiate aggro → returns 0. The aggro pass squares this per (creature, player) pair. Pure —
/// unit-tested. [pure]
pub fn aggro_radius(creature_level: u32, player_level: u32, template_aggro_range: u32) -> f32 {
    if template_aggro_range > 0 {
        return template_aggro_range as f32; // explicit hand-tuned override (e.g. seed chicken)
    }
    let leveldiff = creature_level as i32 - player_level as i32;
    if -leveldiff >= GREY_LEVEL_DIFF {
        return 0.0; // grey: the player out-levels the creature too far — no proximity aggro
    }
    (BASE_AGGRO_RADIUS + leveldiff as f32).clamp(MIN_AGGRO_RADIUS, MAX_AGGRO_RADIUS)
}

// ===========================================================================================
//  Active-cell activation radius (work-item 230: grid-activation — only cells near players tick)
//  [pure]
// ===========================================================================================

/// The combat activation radius (yards) around a player within which a creature must be treated as
/// ACTIVE for the aggro/assist passes this tick, rather than dormant. Two ceilings combine (the larger
/// wins), then the assist margin is added on top:
///   - `MAX_AGGRO_RADIUS` (45yd) — the vanilla level-scaled proximity-aggro ceiling that covers every
///     creature relying on the DEFAULT scaling (every imported cmangos creature: the ETL never sets
///     `aggro_range`, see importer/src/main.rs — `aggro_radius` above falls through to the scaled
///     formula, capped at this constant).
///   - `template_aggro_max` — the largest EXPLICIT per-template `aggro_range` override currently
///     loaded. `aggro_radius()` returns an override VERBATIM, UNCLAMPED, when > 0, so a future seed or
///     import setting one past 45 must still be covered. The caller (`tick::active_cell_radius`) feeds
///     this from a live scan of `game_creature_template` — a small reference table (tens of rows), not
///     the ~2500-creature live population this item exists to stop scanning — so the ceiling can never
///     go silently stale the way a hardcoded constant could.
///
/// `ASSIST_RADIUS` (10yd) is added on top of whichever ceiling wins: the pack-assist scan measures
/// distance from the AGGROER, not the player, and the aggroer can itself sit up to the combined aggro
/// ceiling away from the player — without this margin a same-faction neighbor at the far edge of
/// `ASSIST_RADIUS` from an edge-case aggroer would be wrongly excluded from the active set. Pure —
/// unit-tested. [pure]
/// `const fn` so `CHASE_LEASH_SQ` can be defined from it — a hand-copied 55 would be exactly the kind of
/// silent staleness the live template scan above exists to prevent. Spelled as a comparison rather than
/// `f32::max` (not const-callable); the NaN branch agrees with `max`'s.
pub const fn combat_active_radius(template_aggro_max: f32) -> f32 {
    let ceiling = if template_aggro_max > MAX_AGGRO_RADIUS {
        template_aggro_max
    } else {
        MAX_AGGRO_RADIUS
    };
    ceiling + ASSIST_RADIUS
}

/// The SMALLEST integer `game_creature_template.aggro_range` override that could make
/// `combat_active_radius` exceed `visibility_floor` — i.e. the only overrides that can change
/// `tick::active_cell_radius`'s answer at all. `combat_active_radius(t) > floor` ⟺
/// `max(MAX_AGGRO_RADIUS, t) + ASSIST_RADIUS > floor` ⟺ `t > floor - ASSIST_RADIUS` (the
/// `MAX_AGGRO_RADIUS` branch is already covered whenever the floor dominates it, which it does at
/// today's 100yd vs 55yd). Perf catalog 1.19 uses this as the lower bound of a `by_aggro_range` index
/// probe, so the common "no override anywhere near the floor" case costs one probe instead of a fold
/// over every template. Pure — unit-tested. [pure]
pub(crate) fn aggro_override_cutoff(visibility_floor: f32) -> u32 {
    let cutoff = visibility_floor - ASSIST_RADIUS;
    if cutoff < 0.0 {
        return 0; // every override matters — probe from the bottom (degenerate, floor below the margin)
    }
    // `t > cutoff` over the integers is `t >= floor(cutoff) + 1`, for an integral cutoff too.
    cutoff.floor() as u32 + 1
}

// Pure predicate over the tick's whole geometry input, kept argument-shaped (no `ReducerContext`) so it stays unit-testable.
#[allow(clippy::too_many_arguments)]
/// Is the grid cell `(map_id, instance_id, gx, gy)` inside `radius` yards of the player positioned at
/// `(p_map_id, p_instance_id, px, py)`? Same map AND instance is required first — a coincidental cell
/// match on a DIFFERENT map or instance is never active from this player (mirrors
/// `helpers::in_same_partition`'s isolation rule, work-item 190 slice 1: two entities can share a grid
/// cell across instances without being spatially near each other at all). The radius check reuses
/// `spatial::covering_cell_box`, whose own tests prove it brackets every point within `radius` — so
/// this fn is a thin, unit-tested restatement of "is this cell in that box" for the active-cell
/// pipeline. Pure — unit-tested. [pure]
pub(crate) fn cell_is_active(
    map_id: u32,
    instance_id: u64,
    gx: i32,
    gy: i32,
    p_map_id: u32,
    p_instance_id: u64,
    px: f32,
    py: f32,
    radius: f32,
) -> bool {
    if map_id != p_map_id || instance_id != p_instance_id {
        return false;
    }
    let (gx0, gx1, gy0, gy1) = lyracore_shared::spatial::covering_cell_box(px, py, radius);
    gx0 <= gx && gx <= gx1 && gy0 <= gy && gy <= gy1
}

// ===========================================================================================
//  Stealth detection radius (graded, level-scaled detect range vs. a stealthed target) [pure]
// ===========================================================================================

/// Base distance (yards) at which a creature sees through stealth at an equal-or-lower target level.
const STEALTH_DETECT_BASE: f32 = 5.0;
/// Extra detect range (yards) per creature level ABOVE the stealthed target's level.
const STEALTH_DETECT_PER_LEVEL: f32 = 1.0;
/// Cap on stealth-detection range (yards) — a much-higher-level mob still can't see a sneak from afar.
const STEALTH_DETECT_MAX: f32 = 30.0;

/// Range (yards) within which a creature of `creature_level` detects a STEALTHED target of
/// `target_level`. A same/higher-level creature detects from a small base radius that GROWS with its
/// level advantage; a LOWER-level creature gets only the base (the per-level term floors at 0 — never
/// less than base, so even a low mob stepped on by a stealthed rogue notices it). Clamped to a sane max.
/// The aggro pass squares this and skips a stealthed candidate whose squared distance exceeds it — so a
/// stealthed target outside the detect range stays invisible to that creature.
/// Because the base (5) equals `MIN_AGGRO_RADIUS`, this only ever NARROWS the aggro radius for a
/// stealthed candidate, never widens it. Pure — unit-tested. [pure]
pub fn stealth_detect_range(creature_level: u32, target_level: u32) -> f32 {
    let advantage = (creature_level as i32 - target_level as i32).max(0) as f32;
    (STEALTH_DETECT_BASE + advantage * STEALTH_DETECT_PER_LEVEL).min(STEALTH_DETECT_MAX)
}

/// A point `dist` yards from the creature `(cx,cy)` directly AWAY from the target `(tx,ty)` — the
/// destination of a flee leg. Steps along the unit vector pointing from the target to the creature.
/// Degenerate case (creature and target share a position, `dx==dy==0`): there's no defined "away"
/// direction, so pick a fixed one (+x) rather than dividing by a zero length and producing NaN. Pure
/// — unit-tested.
pub fn flee_step(cx: f32, cy: f32, tx: f32, ty: f32, dist: f32) -> (f32, f32) {
    let (dx, dy) = (cx - tx, cy - ty);
    let len = (dx * dx + dy * dy).sqrt();
    if len == 0.0 {
        // Coincident: arbitrary fixed bearing (+x) so the result is always finite.
        return (cx + dist, cy);
    }
    (cx + dx / len * dist, cy + dy / len * dist)
}

// ===========================================================================================
//  Fear flee (crowd control: a FEARED creature is force-walked away from the fear source) [pure]
// ===========================================================================================

/// Per-tick FEAR flee distance (yards) — how far a feared creature dashes away from its fear source each
/// `tick_creatures`. A panicked run leg (RUN speed sets the leg's animation duration); paired with
/// `feared_flee_step`'s angular jitter it zig-zags rather than tracing a beeline.
pub(crate) const FEAR_STEP: f32 = 15.0;

/// Max angular JITTER (radians) added to a fear flee leg's away-bearing — ±60° (`FRAC_PI_3`). Kept under
/// 90° so the away-component (`cos(jitter)`) is always POSITIVE: a feared creature always nets AWAY from
/// the source, never doubling back into it, while still zig-zagging enough to read as terror.
const FEAR_JITTER: f32 = std::f32::consts::FRAC_PI_3;

/// A FEAR flee leg — a point `dist` yards from the feared creature `(cx,cy)`, generally AWAY from the fear
/// source `(sx,sy)` but rotated by a random angular jitter in `[-FEAR_JITTER, +FEAR_JITTER]` so
/// successive legs zig-zag ("flees in terror"). `rand` (a full-span `u32` roll, e.g. `ctx.random()`) maps
/// linearly onto the jitter offset. Degenerate coincident case (source on top of the creature,
/// `dx==dy==0`): there is no defined "away" bearing, so fall back to +x (0 rad) before applying jitter,
/// so the result is always finite (never NaN) and still moves. Pure — unit-tested.
pub fn feared_flee_step(cx: f32, cy: f32, sx: f32, sy: f32, dist: f32, rand: u32) -> (f32, f32) {
    let (dx, dy) = (cx - sx, cy - sy);
    let base = if dx == 0.0 && dy == 0.0 {
        0.0
    } else {
        dy.atan2(dx)
    };
    let jitter = ((rand as f32 / u32::MAX as f32) * 2.0 - 1.0) * FEAR_JITTER;
    let a = base + jitter;
    (cx + a.cos() * dist, cy + a.sin() * dist)
}

// ===========================================================================================
//  Chase (vanilla creature AI: an engaged mob closes on an out-of-range target) [pure helper]
// ===========================================================================================

/// Squared melee reach — a creature this close (or closer) is in range to swing and must NOT chase.
/// Aliases combat's `MELEE_RANGE_SQ` ((5 yd)²) directly — a single source of truth, so this pass and
/// the swing pass agree on the boundary by construction (no lockstep-by-comment).
pub(crate) const CHASE_MELEE_SQ: f32 = crate::combat::MELEE_RANGE_SQ;

/// Squared chase cutoff — a target beyond this is too far to pursue. An engaged creature chases while its
/// engagement is live, bounded by the active-cell radius (past it its own cell may be dormant); the
/// pursuit timer, not this, ends the fight. Derived from `combat_active_radius(0.0)` so the two cannot
/// drift, replacing the old lockstep with the evade constant that kept a mob from standing neither
/// chasing nor evading.
pub(crate) const CHASE_LEASH_SQ: f32 = combat_active_radius(0.0) * combat_active_radius(0.0);

/// SPLINE chase LEAD (yd): a chaser aims this far PAST a TRANSLATING target along the chase line, so the
/// leg is long and the mob RIDES it for several ticks instead of re-throwing a tiny leg every 500ms tick
/// (which made the client visibly re-compute its heading each tick — the jitter). A straight-running
/// kiter keeps the mob on one committed heading. The lead is only ever thrown at a victim whose live
/// translation flags say it is running, and melee reach is read first, so a target standing inside reach
/// is turned to rather than led past.
pub(crate) const CHASE_LEAD_YD: f32 = 8.0;

/// SPLINE chase RE-AIM threshold (cosine): while chasing, re-emit a new leg only when the target has
/// VEERED off the current leg's heading by more than this angle (~26° at 0.9) — a target running roughly
/// straight holds one heading, so the mob rides one leg without a per-tick re-aim. Lower = re-aim sooner
/// on turns; higher = hold the heading longer.
pub(crate) const CHASE_REPATH_COS: f32 = 0.9;

/// SPLINE flee LEG length (yd): a fleeing mob COMMITS to one leg this far directly away from the threat
/// and re-rolls only once it finishes — instead of re-picking an away-direction every 500ms tick (which
/// snap-rotated the client's facing = the "flee spin"). Vanilla flee legs sit in a 28-38yd
/// quiet band; a single committed leg gives one stable travel tangent (one gentle turn per leg).
pub(crate) const FLEE_LEG_YD: f32 = 28.0;

/// SPLINE interpolation parameter `t` (0..=1): the clamped fraction of a leg elapsed at `now_us`, given
/// the leg's `start_micros` and `dur_ms`. A 0-duration leg is complete (t=1, a snap). Computed ABSOLUTELY
/// from `now_us − start_micros` each tick — never accumulated — so any number of skipped/dormant ticks
/// never drifts the position. The caller lerps `start + (dest − start) × t`. Pure — unit-tested.
pub fn spline_t(now_us: u64, start_micros: u64, dur_ms: u32) -> f32 {
    let dur_us = (dur_ms as u64) * 1000;
    if dur_us == 0 {
        return 1.0;
    }
    (now_us.saturating_sub(start_micros) as f32 / dur_us as f32).clamp(0.0, 1.0)
}

/// A point stepping from the creature `(cx,cy)` toward the target `(tx,ty)`, moving at most `max_step`
/// yards but stopping `stop_dist` yards SHORT of the target — so a chase leg ends just inside melee
/// range, never on top of or past the target (which would overshoot, look wrong, and leave the
/// creature unable to swing). If already within `stop_dist` there's nothing to close, so return
/// `(cx,cy)` (no move). Degenerate coincident case (`dx==dy==0`, distance 0 ≤ any `stop_dist`) is
/// caught by the within-`stop_dist` early return, so we never divide by a zero length. Pure —
/// unit-tested.
pub fn chase_step(cx: f32, cy: f32, tx: f32, ty: f32, max_step: f32, stop_dist: f32) -> (f32, f32) {
    let (dx, dy) = (tx - cx, ty - cy);
    let len = (dx * dx + dy * dy).sqrt();
    // Already inside the stop band (covers the coincident len==0 case) → hold position.
    if len <= stop_dist {
        return (cx, cy);
    }
    // Distance to actually travel: close the gap to `stop_dist` short, but never more than one
    // tick's worth of run (`max_step`).
    let travel = (len - stop_dist).min(max_step);
    (cx + dx / len * travel, cy + dy / len * travel)
}

// ===========================================================================================
//  Return-to-spawn (vanilla creature AI: a displaced, non-engaged creature walks home) [const]
// ===========================================================================================

/// Squared return LEASH — a creature within this of its spawn `(x,y)` is considered home-ENOUGH and
/// makes no return move; only one displaced FARTHER than this walks back. (15 yd)²; deliberately
/// roomy so the idle WANDER pass (which hops within `WANDER_RADIUS` = 6 yd of home, always inside
/// this leash) never trips return — the two are mutually exclusive (a creature inside the leash
/// wanders; one outside it returns). Pairs with the chase pass: pull → chase → evade → (here) walk
/// home.
pub(crate) const RETURN_LEASH_SQ: f32 = 225.0;

// ===========================================================================================
//  Idle wander (vanilla creature AI: an idle, home creature hops to nearby points) [const + pure]
// ===========================================================================================

/// Radius (yards) of the wander circle around a creature's spawn HOME — an idle creature hops to a
/// random point within this of home. 6 yd, comfortably inside `RETURN_LEASH_SQ` ((15 yd)²) so a
/// wandering creature is ALWAYS home-enough and the return pass never fights it.
pub(crate) const WANDER_RADIUS: f32 = 6.0;

/// Per-tick chance (percent) that an eligible idle creature picks a new wander hop. ~1/3 of ticks it
/// moves; the other ~2/3 it pauses where it is, so wander reads as idle loitering, not a constant
/// jog. Integer percent compared against `ctx.random::<u32>() % 100`.
pub(crate) const WANDER_CHANCE_PCT: u32 = 35;

/// A random destination within `radius` yards of home `(home_x, home_y)` from two random `u32` rolls:
/// `rand_a` maps to an angle in `[0, 2π)`, `rand_b` to a radius in `[0, radius]`. Mapping both rolls
/// through the full `u32` span keeps the math float-only (no modulo bias to reason about) and the
/// result is provably within `radius` of home (the radius scalar is clamped to `[0, radius]`). Pure —
/// unit-tested.
pub fn wander_point(home_x: f32, home_y: f32, rand_a: u32, rand_b: u32, radius: f32) -> (f32, f32) {
    let angle = (rand_a as f32 / u32::MAX as f32) * std::f32::consts::TAU;
    let r = (rand_b as f32 / u32::MAX as f32) * radius;
    (home_x + r * angle.cos(), home_y + r * angle.sin())
}

// ===========================================================================================
//  Movement tick cadence + the pure tick/segment helpers (mangos-parity smooth movement) [const + pure]
// ===========================================================================================

/// The creature movement tick interval in SECONDS. Movement legs (chase/return step, patrol/wander
/// segment durations) are sized to one tick so successive legs chain seamlessly — the client finishes
/// interpolating one spline just as the next arrives. 0.5s ≈ mangos's chase recheck cadence.
pub(crate) const MOVE_TICK_SECS: f32 = 0.5;

/// The same interval in MICROSECONDS — the `game_creature_move_schedule` Interval AND the sense-gate
/// divisor must use this so they stay in lockstep with `MOVE_TICK_SECS` (0.5s = 500_000us).
pub(crate) const MOVE_TICK_MICROS: i64 = 500_000;

/// The expensive O(N) "sensing" passes (aggro/cast/threat/regen/decay/respawn/wander) run once every
/// Nth movement tick, so their per-tick scans + tick-quantized effects (HP-regen amount, wander chance,
/// respawn timers) keep their original ~4s cadence while movement runs every 0.5s. 8 × 0.5s = 4s.
pub(crate) const SENSE_EVERY_N_TICKS: i64 = 8;

/// The target SENSE period in micros (~4s) — `MOVE_TICK_MICROS × SENSE_EVERY_N_TICKS`. A tick row of
/// ANY cadence quantizes its sensing passes to roughly this period (see
/// `is_sense_tick_for_interval`), so a tight per-instance tick (work-item 229) smooths MOVEMENT
/// latency without also multiplying the expensive sensing scans or the tick-quantized effect rates
/// (wander hop chance, aggro re-checks) that assume the ~4s cadence.
pub(crate) const SENSE_PERIOD_MICROS: i64 = MOVE_TICK_MICROS * SENSE_EVERY_N_TICKS;

/// Is this tick a "sensing" tick (run the ~4s passes)? Generalized to a tick row of ANY interval
/// (work-item 229 — per-instance tick rows carry their own `ScheduleAt::Interval`): one firing in
/// every `SENSE_PERIOD_MICROS / interval` firings is a sense tick, so the sensing cadence stays ~4s no
/// matter how fast the row fires. At the seeded default (`interval == MOVE_TICK_MICROS`) the divisor
/// is exactly `SENSE_EVERY_N_TICKS` (one sense tick in 8). An interval LONGER than the sense period
/// makes every firing a sense tick (divisor floors at 1); a non-positive interval (a `ScheduleAt::Time`
/// row, or garbage) is clamped to the default so the math never divides by zero. Pure fn of the
/// scheduler timestamp in micros, so it needs no stored counter and survives across reducer
/// invocations. Pure — unit-tested.
pub fn is_sense_tick_for_interval(now_micros: i64, interval_micros: i64) -> bool {
    let interval = if interval_micros > 0 {
        interval_micros
    } else {
        MOVE_TICK_MICROS
    };
    let every_n = (SENSE_PERIOD_MICROS / interval).max(1);
    (now_micros / interval) % every_n == 0
}

/// One tick's worth of SECONDS for a row firing every `interval_micros` — what the per-firing STEP
/// passes (chase/return/flee/fear-flee) multiply speed by, so a creature's NET speed is
/// cadence-invariant: a 100ms per-instance tick emits 5× as many legs each 1/5 the length, not 5×
/// the speed (the multiplied-movement bug the work-item 229 review hunts for). At the default
/// interval this is EXACTLY `MOVE_TICK_SECS` (500_000/1_000_000 = 0.5, exact in f32) — byte-identical
/// legs. Same non-positive clamp as `is_sense_tick_for_interval`. Pure — unit-tested.
pub fn tick_secs_for_interval(interval_micros: i64) -> f32 {
    let interval = if interval_micros > 0 {
        interval_micros
    } else {
        MOVE_TICK_MICROS
    };
    interval as f32 / 1_000_000.0
}

/// The fear-flee dash distance for one firing of a row ticking every `tick_secs` — `FEAR_STEP` was
/// authored as "per 0.5s firing", so a faster row must scale it down proportionally or a feared
/// creature would dash `FEAR_STEP` yards PER FIRING (5× terror-speed at 100ms). At the default tick
/// this is exactly `FEAR_STEP` (ratio 1.0). Pure — unit-tested.
pub fn fear_step_for_tick(tick_secs: f32) -> f32 {
    FEAR_STEP * (tick_secs / MOVE_TICK_SECS)
}

/// The ACTUAL seconds between two sense ticks of a row firing every `interval_micros` — i.e.
/// `every_n × interval` with the same `every_n = max(SENSE_PERIOD_MICROS / interval, 1)` floor as
/// `is_sense_tick_for_interval` (the two MUST stay in lockstep: this is the leg span a sense-tick
/// pass must cover to bridge to the NEXT sense tick). NOT always ~4s: the floor makes every firing
/// of a slower-than-half-the-period row a sense tick, so a 2.5s row's sense period is 2.5s, not 4s
/// — the pet follow pass hardcoding 4s emitted OVERLAPPING legs there (~1.6× closing speed, 229
/// review). At the default 500ms interval this is exactly `MOVE_TICK_SECS × SENSE_EVERY_N_TICKS`
/// (4.0). Same non-positive clamp. Pure — unit-tested.
pub fn sense_period_secs_for_interval(interval_micros: i64) -> f32 {
    let interval = if interval_micros > 0 {
        interval_micros
    } else {
        MOVE_TICK_MICROS
    };
    let every_n = (SENSE_PERIOD_MICROS / interval).max(1);
    (every_n * interval) as f32 / 1_000_000.0
}

// ===========================================================================================
//  Per-instance tick scoping (work-item 229) [pure]
// ===========================================================================================
//
// HONEST BOUND (from the work item — repeat it wherever this machinery is documented): SpacetimeDB
// serializes ALL reducers on ONE commit stream, so per-instance tick rows are LATENCY SMOOTHING +
// WORK AVOIDANCE, **NOT parallel throughput**. A dedicated row lets one instance tick tighter (or,
// later, slower/paused) and lets every row scan ONLY its own instances — it never adds CPU in
// parallel, and every extra firing TAXES the shared serialized stream (10 instances at 100ms = 100
// extra transactions/sec preempting player actions). Tight cadences are a knob to use sparingly.

/// Sentinel `instance_id` for a `game_creature_move_schedule` row meaning GLOBAL / catch-all: the row
/// covers every instance WITHOUT a dedicated row of its own — normally including instance 0 (the
/// open world), but the partition rule has no special case: arming a dedicated row for instance 0
/// (permitted, if odd) moves the open world's SCOPED passes onto that row, while the global
/// due-time passes stay on the catch-all firing — and the 230/233 catch-all evidence log lines
/// would then read ~0 creatures (they describe the catch-all's scoped work; 229 review). u64::MAX —
/// no real instance ever gets this id (instance ids are allocated small; and even an entity
/// teleported to a pathological instance u64::MAX would still be covered by the catch-all, never
/// stranded). The schema default on `CreatureMoveSchedule.instance_id` is this value WRITTEN AS A
/// LITERAL (`18_446_744_073_709_551_615u64` — the `#[default]` macro wants a literal expression);
/// the `global_tick_instance_sentinel_is_u64_max` test pins THIS CONST to u64::MAX, but nothing can
/// programmatically tie it to the `#[default]` attribute literal in tick.rs (migration metadata,
/// not a Rust `Default`) — editing THAT literal alone reds no test; the schema doc points back here.
pub(crate) const GLOBAL_TICK_INSTANCE: u64 = u64::MAX;

/// Which instances one FIRING of `tick_creatures` covers (work-item 229). Built per firing from the
/// firing row's `instance_id` + the full (tiny) schedule-row set:
///   - a DEDICATED row (`instance_id != GLOBAL_TICK_INSTANCE`) covers EXACTLY its instance;
///   - the CATCH-ALL row (`instance_id == GLOBAL_TICK_INSTANCE`) covers every instance EXCEPT those
///     with a dedicated row (so two rows never both tick one instance — coverage is a PARTITION,
///     proven by `tick_scope_partitions_every_instance_exactly_once`).
///
/// ANTI-STRANDING GUARANTEE: an entity can sit in an instance with NO `game_instance` row and NO
/// dedicated tick row (224's `teleport_player` accepts an arbitrary `instance_id` today) — the
/// catch-all's "everything not dedicated" rule means such an entity is ALWAYS covered; a
/// strictly-enumerated instance list would freeze it forever.
pub(crate) enum TickScope {
    /// The catch-all row: covers every instance except the `dedicated` set (instances with rows of
    /// their own).
    CatchAll {
        dedicated: std::collections::HashSet<u64>,
    },
    /// A dedicated row: covers exactly this instance.
    Only(u64),
}

impl TickScope {
    /// Resolve the scope for the firing row with `row_instance_id`, given every schedule row's
    /// `instance_id` (the schedule table is a handful of rows — one catch-all + one per dedicated
    /// instance — so this is O(rows), NOT a creature/entity scan; the "covered-instance set" is a
    /// PREDICATE over this tiny set, never a materialized enumeration of live instances).
    pub(crate) fn from_rows(
        row_instance_id: u64,
        all_row_instance_ids: impl IntoIterator<Item = u64>,
    ) -> Self {
        if row_instance_id == GLOBAL_TICK_INSTANCE {
            TickScope::CatchAll {
                dedicated: all_row_instance_ids
                    .into_iter()
                    .filter(|&i| i != GLOBAL_TICK_INSTANCE)
                    .collect(),
            }
        } else {
            TickScope::Only(row_instance_id)
        }
    }

    /// Does THIS firing cover `instance_id`? The one predicate every scoped pass gates on. With only
    /// the seeded catch-all row (no dedicated rows) this is `true` for EVERY instance — the
    /// equivalence case: the pass visits the identical candidate set it did before work-item 229.
    pub(crate) fn covers(&self, instance_id: u64) -> bool {
        match self {
            TickScope::CatchAll { dedicated } => !dedicated.contains(&instance_id),
            TickScope::Only(n) => instance_id == *n,
        }
    }

    /// Do the GLOBAL, due-time passes (decay/respawn/GO-respawn/regen/combat-drop/package ticks) run
    /// on this firing? Catch-all only: those passes are due-TIME bookkeeping over tables that aren't
    /// instance-partitioned (spawn timers, regen, players' combat flags) and they quantize their
    /// effects per RUN (regen amount per sense tick) — running them from a second, faster row would
    /// MULTIPLY their effects (double regen, double decay scans), the exact work-multiplication this
    /// item forbids. They cover ALL instances (including dedicated-row ones) from the catch-all row,
    /// at the unchanged global cadence — so a dedicated row is a movement/AI smoothing knob only, and
    /// deleting it can never strand an instance's timers. COROLLARY: the catch-all row is
    /// load-bearing — never delete it (`debug_repair_after_publish`, #378, restores it).
    pub(crate) fn runs_global_passes(&self) -> bool {
        matches!(self, TickScope::CatchAll { .. })
    }
}

/// Is a creature's in-flight move leg still playing? Its authoritative position is set to the leg's
/// destination at leg-start, while the client animates to `leg_ends_ms`; a SEGMENT pass (patrol/wander)
/// must not emit a new leg until the current one lands. `leg_ends_ms == 0` (fresh/respawned) is never in
/// flight. Pure — unit-tested.
pub fn leg_in_flight(now_ms: u32, leg_ends_ms: u32) -> bool {
    now_ms < leg_ends_ms
}

/// `to` is `speed` yards away from `from` in this many milliseconds. `speed` is yd/s, already
/// snare/wound-adjusted by the caller if its mover applies one. Pure.
fn leg_duration_ms(from: (f32, f32), to: (f32, f32), speed: f32) -> u32 {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    ((dx * dx + dy * dy).sqrt() / speed * 1000.0) as u32
}

/// The tail every stepped mover shares once navigation has resolved the real landing point: drop a
/// degenerate zero-length step (the client rejects it), else give the landing point and the leg's
/// duration at `speed` yd/s. Pure.
pub(crate) fn leg_toward(from: (f32, f32), to: (f32, f32), speed: f32) -> Option<(f32, f32, u32)> {
    if to == from {
        return None;
    }
    Some((to.0, to.1, leg_duration_ms(from, to, speed)))
}

/// The index AFTER `cur_idx` in a route of `len` waypoints, wrapping at the end (ordered patrol
/// traversal: 0→1→…→len-1→0). `len == 0` → 0 (caller guards `len >= 2`). Pure — unit-tested.
pub fn next_waypoint_idx(cur_idx: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        (cur_idx + 1) % len
    }
}

/// Index of the waypoint nearest `(cx, cy)` — the first patrol target when there's no live cursor
/// (fresh spawn / stale `wp_target`). Squared-distance compare, no sqrt. Empty → 0. Pure — unit-tested.
pub fn nearest_waypoint_idx(cx: f32, cy: f32, pts: &[(f32, f32)]) -> usize {
    pts.iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            let da = (a.0 - cx).powi(2) + (a.1 - cy).powi(2);
            let db = (b.0 - cx).powi(2) + (b.1 - cy).powi(2);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::constants;
    // The stat-roll tests live here too (lifted with the geometry tests); their subjects are in the
    // sibling `spawn` module, reached via the parent `creatures` re-export.
    use crate::creatures::{
        rank_hp_multiplier_permille, rolled_creature_stats, scale_creature_damage_for_level,
        scale_health_for_rank, CreatureFamily,
    };

    #[test]
    fn creature_damage_scales_from_template_to_live_level_without_overflow() {
        assert_eq!(scale_creature_damage_for_level(5, 10, 10, 10), (5, 10));
        assert_eq!(scale_creature_damage_for_level(5, 10, 10, 12), (6, 12));
        assert_eq!(
            scale_creature_damage_for_level(u32::MAX, u32::MAX, 1, 60),
            (u32::MAX, u32::MAX)
        );
    }

    #[test]
    fn hp_pct_below_threshold_and_flee_delegation() {
        // Below / at / above the threshold (cross-multiply, no float).
        assert!(hp_pct_below(49, 100, 50)); // 49% < 50%
        assert!(!hp_pct_below(50, 100, 50)); // exactly 50% is NOT below
        assert!(!hp_pct_below(80, 100, 50));
        // A dead unit (0 HP) is never "below" (the alive guard) — a corpse doesn't heal-cast.
        assert!(!hp_pct_below(0, 100, 50));
        // should_flee is hp_pct_below at the 15% flee threshold.
        assert!(should_flee(14, 100)); // 14% < 15%
        assert!(!should_flee(15, 100));
        assert!(!should_flee(0, 100)); // dead → no flee
    }

    #[test]
    fn hp_pct_below_widens_products_past_u32_overflow_boundary() {
        // `health * 100` still fits in u32 at this value, then overflows one HP later. Full-health
        // creatures on either side of that boundary are at 100%, never below the flee threshold.
        const LAST_SAFE_TIMES_100: u32 = u32::MAX / 100;
        assert!(!hp_pct_below(
            LAST_SAFE_TIMES_100,
            LAST_SAFE_TIMES_100,
            FLEE_HEALTH_PCT,
        ));
        assert!(!hp_pct_below(
            LAST_SAFE_TIMES_100 + 1,
            LAST_SAFE_TIMES_100 + 1,
            FLEE_HEALTH_PCT,
        ));

        // Both products exceed u32 here; widening must preserve full-health and genuinely-low-health
        // answers all the way through the public input range.
        assert!(!hp_pct_below(u32::MAX, u32::MAX, FLEE_HEALTH_PCT));
        assert!(hp_pct_below(u32::MAX / 10, u32::MAX, FLEE_HEALTH_PCT));
    }

    #[test]
    fn aggro_radius_scales_with_level_and_greys_out() {
        // Equal level → the ~20-yard base.
        assert_eq!(aggro_radius(10, 10, 0), BASE_AGGRO_RADIUS);
        // A higher-level creature notices you from farther (±1 yd/level), capped at MAX.
        assert_eq!(aggro_radius(13, 10, 0), 23.0); // +3 levels → 23 yd
        assert_eq!(aggro_radius(99, 10, 0), MAX_AGGRO_RADIUS); // huge gap → clamped to MAX
                                                               // A lower-level creature only aggroes up close (−1 yd/level) until it greys out.
        assert_eq!(aggro_radius(8, 14, 0), 14.0); // −6 levels → 14 yd
        assert_eq!(aggro_radius(8, 15, 0), 13.0); // −7 levels (just under grey) → 13 yd
                                                  // GREY: player GREY_LEVEL_DIFF+ levels above the creature → no proximity aggro at all.
        assert_eq!(aggro_radius(1, 1 + GREY_LEVEL_DIFF as u32, 0), 0.0); // exactly at the grey gap
        assert_eq!(aggro_radius(1, 60, 0), 0.0); // a L60 isn't swarmed by L1 mobs
                                                 // An explicit template override WINS verbatim (the demo chicken's tuned 8), ignoring level scaling.
        assert_eq!(aggro_radius(1, 60, 8), 8.0); // override beats the grey rule
        assert_eq!(aggro_radius(10, 10, 12), 12.0);
    }

    // =========================================================================================
    //  Active-cell activation math (work-item 230)
    // =========================================================================================

    #[test]
    fn combat_active_radius_covers_the_vanilla_ceiling_and_scales_with_template_overrides() {
        // No template override in play (every imported creature: aggro_range == 0) → the vanilla
        // MAX_AGGRO_RADIUS ceiling (45yd) plus the assist margin (10yd) = 55yd.
        assert_eq!(combat_active_radius(0.0), MAX_AGGRO_RADIUS + ASSIST_RADIUS);
        // A template override BELOW the vanilla ceiling never NARROWS the radius (the ceiling still wins).
        assert_eq!(combat_active_radius(8.0), MAX_AGGRO_RADIUS + ASSIST_RADIUS);
        // A template override ABOVE the vanilla ceiling (a future seed/import) WIDENS the radius —
        // the live scan, not a hardcoded constant, is what keeps this honest.
        assert_eq!(combat_active_radius(100.0), 100.0 + ASSIST_RADIUS);
    }

    /// Perf catalog 1.19 — the index-probe shortcut in `tick::active_cell_radius` is only safe if the
    /// cutoff is EXACTLY the set of overrides that could beat the visibility floor. Sweep every integer
    /// override around the boundary and assert the cutoff agrees with the real `combat_active_radius`
    /// comparison: below the cutoff the floor provably wins (so skipping the fold is byte-identical),
    /// at/above it the fold must run.
    #[test]
    fn aggro_override_cutoff_is_exactly_the_overrides_that_can_beat_the_visibility_floor() {
        let floor = lyracore_shared::spatial::BOX_HALF_SPAN as f32
            * lyracore_shared::spatial::GRID_CELL_SIZE;
        let cutoff = aggro_override_cutoff(floor);
        assert_eq!(
            cutoff, 91,
            "100yd floor − 10yd assist margin → overrides of 91+ matter"
        );
        for t in 0..200u32 {
            let beats_floor = combat_active_radius(t as f32) > floor;
            assert_eq!(
                beats_floor,
                t >= cutoff,
                "override {t}: cutoff must agree with the real radius comparison"
            );
        }
        // A degenerate floor below the assist margin means every override is in play.
        assert_eq!(aggro_override_cutoff(ASSIST_RADIUS - 1.0), 0);
    }

    #[test]
    fn cell_is_active_brackets_the_radius_and_excludes_other_maps_and_instances() {
        let radius = combat_active_radius(0.0); // 55yd
        let (gx, gy) = lyracore_shared::spatial::grid_cell(0.0, 0.0);
        // The player's own cell is always active around itself.
        assert!(cell_is_active(1, 0, gx, gy, 1, 0, 0.0, 0.0, radius));
        // A DIFFERENT map or instance is never active, even at the identical cell coordinates — the
        // exact 190-slice-1 isolation regression `helpers::in_same_partition` guards against.
        assert!(!cell_is_active(2, 0, gx, gy, 1, 0, 0.0, 0.0, radius));
        assert!(!cell_is_active(1, 7, gx, gy, 1, 0, 0.0, 0.0, radius));
        // A cell WELL beyond the radius (many cells over) is excluded.
        let (fgx, fgy) = lyracore_shared::spatial::grid_cell(0.0 - radius - 500.0, 0.0);
        assert!(!cell_is_active(1, 0, fgx, fgy, 1, 0, 0.0, 0.0, radius));
    }

    /// Work-item 230's engaged-creature-never-dormant rule ("a player could drag one far away") only
    /// holds if a creature glued to a player by combat can never wander past the active-cell radius
    /// before the active set would have covered it anyway. The chase and rout phases don't consult
    /// the active set at all (see their doc comments) — that's the primary guarantee — but this
    /// pins the geometric invariant too: the chase cutoff (the farthest a target can be and still be
    /// pursued) must stay inside `combat_active_radius`, so even a hypothetical future re-gate could
    /// never sleep a still-engaged creature out from under its target.
    ///
    /// Distance no longer ends a fight, so the cutoff is no longer in lockstep with an evade constant: it
    /// IS the active-cell radius, which is what keeps an engaged creature pursuing while its engagement
    /// lives instead of freezing between a chase cutoff and an evade cutoff. Equality is inside, not on
    /// the edge: `cell_is_active` brackets every point WITHIN the radius (`covering_cell_box`), and the
    /// live radius takes the larger of this and the 100yd visibility floor.
    #[test]
    fn chase_leash_radius_stays_within_the_combat_active_radius() {
        assert_eq!(
            CHASE_LEASH_SQ.sqrt(),
            combat_active_radius(0.0),
            "the chase cutoff must not exceed the active-cell radius"
        );
    }

    /// Work-item 230's "mid-leg flee/return either finishes or freezes coherently" requirement: this
    /// repo picks FREEZE (dormancy simply skips the pass, touching nothing). That choice is only
    /// coherent because `chase_step` (the shared return/chase step primitive) is a pure function of the
    /// CURRENT position and target — no elapsed-time or step-count argument — so pausing for any number
    /// of ticks and resuming produces exactly the same trajectory as if it had been ticked continuously
    /// to the same intermediate position. If `chase_step` ever gained hidden state this test would catch
    /// the divergence.
    #[test]
    fn chase_step_is_position_only_so_freezing_between_calls_is_lossless() {
        let (home_x, home_y) = (100.0, 100.0);
        let start = (130.0, 100.0); // 30yd out
                                    // Two ticks, uninterrupted.
        let mut cont = start;
        for _ in 0..2 {
            cont = chase_step(cont.0, cont.1, home_x, home_y, 5.0, 0.0);
        }
        // Two ticks computed identically, standing in for "then dormant for N ticks" — since dormancy
        // means the pass isn't called at all, the position after re-entry is BY CONSTRUCTION identical
        // to `cont` (nothing mutated it while dormant); the real claim under test is the next line.
        let frozen = cont;
        // Resuming from the frozen position yields the SAME next step as continuing uninterrupted would
        // have — chase_step reads only the position it's given, never a hidden tick counter.
        let next_after_resume = chase_step(frozen.0, frozen.1, home_x, home_y, 5.0, 0.0);
        let next_if_uninterrupted = chase_step(cont.0, cont.1, home_x, home_y, 5.0, 0.0);
        assert_eq!(next_after_resume, next_if_uninterrupted);
    }

    /// The walk home has no upper bound on displacement: the return pass re-steps ONE tick of run per
    /// firing (`nav_step` → this primitive), so a creature left far out chains legs to `RETURN_LEASH_SQ`
    /// instead of single-legging and stalling.
    #[test]
    fn a_creature_displaced_far_from_home_steps_all_the_way_back() {
        let (home_x, home_y) = (-100.0_f32, 250.0_f32);
        let step = constants::speeds::RUN * MOVE_TICK_SECS; // one movement tick of run
        let start = 300.0_f32; // far past anything the old spawn tether could leave behind
        let dist_sq = |p: (f32, f32)| (p.0 - home_x).powi(2) + (p.1 - home_y).powi(2);
        let mut pos = (home_x + start, home_y);
        let mut prev_sq = dist_sq(pos);
        let mut ticks = 0u32;
        // The loop condition IS the return pass's gate: while it holds, wander skips the guid.
        while dist_sq(pos) > RETURN_LEASH_SQ {
            let next = chase_step(pos.0, pos.1, home_x, home_y, step, 0.0);
            assert_ne!(next, pos, "return stalled after {ticks} ticks");
            pos = next;
            let now_sq = dist_sq(pos);
            assert!(now_sq < prev_sq, "step {ticks} did not close the gap");
            prev_sq = now_sq;
            ticks += 1;
            assert!(ticks < 10_000, "no arrival from {start} yd out");
        }
        // Derived, not hand-counted, so tuning any of the three constants can't fail this on arithmetic.
        assert_eq!(
            ticks,
            ((start - RETURN_LEASH_SQ.sqrt()) / step).ceil() as u32
        );
    }

    #[test]
    fn stealth_detect_range_grows_with_level_advantage_and_clamps() {
        // Equal level → the small base detect radius.
        assert_eq!(stealth_detect_range(10, 10), 5.0);
        // A higher-level creature sees through stealth from farther (+1 yd/level of advantage).
        assert_eq!(stealth_detect_range(20, 10), 15.0); // +10 levels → 5 + 10
        assert_eq!(stealth_detect_range(11, 10), 6.0); // +1 level → 6
                                                       // A LOWER-level creature is floored at base (never below 5, the per-level term floors at 0).
        assert_eq!(stealth_detect_range(5, 10), 5.0);
        assert_eq!(stealth_detect_range(1, 60), 5.0);
        // Clamped to the max even with a huge level gap.
        assert_eq!(stealth_detect_range(99, 1), STEALTH_DETECT_MAX);
        // Base equals the aggro floor → detection only ever NARROWS the aggro radius, never widens it.
        assert_eq!(stealth_detect_range(10, 10), MIN_AGGRO_RADIUS);
    }

    #[test]
    fn level_roll_respects_range_and_interpolates_health() {
        // No range (max ≤ min, or the 0 default) → the min level/health verbatim, any rand.
        assert_eq!(rolled_creature_stats(3, 0, 42, 0, 12345), (3, 42));
        assert_eq!(rolled_creature_stats(3, 3, 42, 60, 999), (3, 42));
        // A real range: level stays within [min, max] for any rand; the endpoints are reachable.
        for rand in 0..50u32 {
            let (lvl, _) = rolled_creature_stats(1, 3, 40, 80, rand);
            assert!(
                (1..=3).contains(&lvl),
                "level {lvl} out of [1,3] for rand {rand}"
            );
        }
        assert_eq!(rolled_creature_stats(1, 3, 40, 80, 0).0, 1); // rand 0 → min
        assert_eq!(rolled_creature_stats(1, 3, 40, 80, 2).0, 3); // rand 2 → max (span 2, 2%3==2)
                                                                 // Health interpolates by level position: L1→40 (min), L3→80 (max), L2→midpoint 60.
        assert_eq!(rolled_creature_stats(1, 3, 40, 80, 0), (1, 40));
        assert_eq!(rolled_creature_stats(1, 3, 40, 80, 1), (2, 60));
        assert_eq!(rolled_creature_stats(1, 3, 40, 80, 2), (3, 80));
        // Health never below min even if max_health is missing (0) — falls back to min health.
        assert_eq!(rolled_creature_stats(1, 3, 50, 0, 2), (3, 50));
    }

    #[test]
    fn rank_scales_health_and_rank0_is_identity() {
        // Rank 0 (normal) is the IDENTITY — every current spawn is rank 0, so scaling must not change
        // a single byte of its health for any base value.
        for base in [0u32, 1, 42, 1000, 250_000, u32::MAX] {
            assert_eq!(
                scale_health_for_rank(base, 0),
                base,
                "rank-0 must be identity for base {base}"
            );
        }
        // Elite/rare/boss multipliers (permille math): a 100-HP base scales by the documented factors.
        assert_eq!(scale_health_for_rank(100, 1), 150); // elite ×1.5
        assert_eq!(scale_health_for_rank(100, 2), 280); // rare-elite ×2.8
        assert_eq!(scale_health_for_rank(100, 3), 400); // boss ×4.0
        assert_eq!(scale_health_for_rank(100, 4), 180); // rare ×1.8
                                                        // A realistic L1 mob (42 HP) as an elite → 42*1500/1000 = 63 (truncated integer math).
        assert_eq!(scale_health_for_rank(42, 1), 63);
        // Unknown rank → normal (never amplify an unrecognized value).
        assert_eq!(scale_health_for_rank(100, 7), 100);
        assert_eq!(scale_health_for_rank(100, 255), 100);
        // The permille table itself: rank 0 is exactly 1000 (the identity divisor) and elite > normal.
        assert_eq!(rank_hp_multiplier_permille(0), 1000);
        assert!(rank_hp_multiplier_permille(1) > rank_hp_multiplier_permille(0));
        assert!(rank_hp_multiplier_permille(3) > rank_hp_multiplier_permille(1));
        // SATURATION: every >1× rank at the max base clamps to EXACTLY u32::MAX — the u64 product
        // exceeds the u32 ceiling for all of them, and the clamp (not a wrapping cast) must win.
        assert_eq!(scale_health_for_rank(u32::MAX, 1), u32::MAX); // elite
        assert_eq!(scale_health_for_rank(u32::MAX, 2), u32::MAX); // rare-elite
        assert_eq!(scale_health_for_rank(u32::MAX, 3), u32::MAX); // boss
        assert_eq!(scale_health_for_rank(u32::MAX, 4), u32::MAX); // rare
                                                                  // Large-but-fitting products stay EXACT (the clamp never fires early): one per >1× rank.
        assert_eq!(scale_health_for_rank(2_000_000_000, 1), 3_000_000_000); // ×1.5 < u32::MAX
        assert_eq!(scale_health_for_rank(1_000_000_000, 2), 2_800_000_000); // ×2.8 < u32::MAX
        assert_eq!(scale_health_for_rank(1_000_000_000, 3), 4_000_000_000); // ×4.0 < u32::MAX
        assert_eq!(scale_health_for_rank(2_000_000_000, 4), 3_600_000_000); // ×1.8 < u32::MAX
    }

    /// Schema shape guard (work-item 214, follows `loot.rs`'s `new_loot_family_tables_construct_with_
    /// the_documented_shape` convention): `game_creature_family` constructs with named fields exactly
    /// like the importer's positional SQL INSERT column list (`family_id,name,pet_food_mask,
    /// pet_talent_type,category`) — a compile-time guard against a silently reordered/renamed column.
    /// Purely compile-time: the runtime gates (188's feeding + tameable) don't exist yet, so there is
    /// nothing behavioral to assert.
    #[test]
    fn new_creature_family_table_constructs_with_the_documented_shape() {
        let _wolf = CreatureFamily {
            family_id: 1,
            name: "Wolf".to_string(),
            pet_food_mask: 0x41, // MEAT | RAW_MEAT
            pet_talent_type: 0,  // >= 0 = tameable; -1 marks non-pet families
            category: 8,
        };
    }

    #[test]
    fn flee_threshold_is_alive_and_low_health() {
        // 5/42 → 5*100=500 < 42*15=630 → flee.
        assert!(should_flee(5, 42));
        // 10/42 → 1000 < 630 is false → hold the line (above the threshold).
        assert!(!should_flee(10, 42));
        // 0 HP is dead, not fleeing (the death/loot path handles it, not the flee pass).
        assert!(!should_flee(0, 42));
        // Full HP never flees.
        assert!(!should_flee(42, 42));
    }

    #[test]
    fn flee_eligible_is_humanoid_only() {
        // Only HUMANOIDS (creature_type 7) rout at low HP — BEASTS (wolves/boars, type 1) fight to the
        // death instead.
        assert!(
            flee_eligible(7),
            "humanoid (Defias militiaman) routs to call for help"
        );
        assert!(
            !flee_eligible(1),
            "BEAST (wolf/boar) fights to the death, never routs"
        );
        assert!(
            !flee_eligible(8),
            "critter (chicken) is not flee-eligible either"
        );
        assert!(
            !flee_eligible(0),
            "no/unknown type → not eligible (safe default)"
        );
    }

    #[test]
    fn flee_step_runs_away_from_target() {
        // Creature east of the target at (10,0), target at the origin → step further east to x≈22.
        let (x, y) = flee_step(10.0, 0.0, 0.0, 0.0, 12.0);
        assert!((x - 22.0).abs() < 0.01, "x {x} expected ~22.0");
        assert!((y - 0.0).abs() < 0.01, "y {y} expected ~0.0");

        // Coincident creature/target: no defined "away" → fixed fallback bearing, always finite and
        // exactly `dist` from the start point (never NaN, never a divide-by-zero).
        let (x, y) = flee_step(5.0, 5.0, 5.0, 5.0, 12.0);
        assert!(
            x.is_finite() && y.is_finite(),
            "fallback must be finite, got ({x},{y})"
        );
        let d = ((x - 5.0).powi(2) + (y - 5.0).powi(2)).sqrt();
        assert!(
            (d - 12.0).abs() < 0.01,
            "fallback distance {d} expected ~12.0"
        );
    }

    #[test]
    fn feared_flee_step_moves_away_from_source() {
        // Source at the origin, creature east at (10,0): away bearing is +x. The midpoint roll
        // (u32::MAX/2) maps to zero jitter, so the leg lands straight out along +x at x≈22 (10+12).
        let (x, y) = feared_flee_step(10.0, 0.0, 0.0, 0.0, 12.0, u32::MAX / 2);
        assert!(
            (x - 22.0).abs() < 0.05,
            "zero-jitter leg goes straight away on +x, got x={x}"
        );
        assert!(
            y.abs() < 0.05,
            "zero-jitter leg stays on the away axis, got y={y}"
        );
    }

    #[test]
    fn feared_flee_step_always_nets_away() {
        // Across the FULL jitter range the away-component stays POSITIVE (jitter capped under 90° →
        // cos ≥ 0.5): a feared creature east of a source at the origin never doubles back toward it.
        let (cx, sx) = (10.0_f32, 0.0_f32);
        for r in [
            0u32,
            u32::MAX / 4,
            u32::MAX / 2,
            (u32::MAX / 4) * 3,
            u32::MAX,
        ] {
            let (x, _y) = feared_flee_step(cx, 0.0, sx, 0.0, 12.0, r);
            assert!(
                x > cx,
                "jitter roll {r}: must still net away from the source, got x={x}"
            );
        }
    }

    #[test]
    fn feared_flee_step_coincident_is_finite() {
        // Source on top of the creature: no defined away bearing → +x fallback, finite, and it moves.
        let (x, y) = feared_flee_step(5.0, 5.0, 5.0, 5.0, 12.0, 0);
        assert!(
            x.is_finite() && y.is_finite(),
            "coincident fallback must be finite, got ({x},{y})"
        );
        let d = ((x - 5.0).powi(2) + (y - 5.0).powi(2)).sqrt();
        assert!(
            (d - 12.0).abs() < 0.05,
            "coincident leg distance {d} expected ~12.0 (still moves)"
        );
    }

    #[test]
    fn chase_step_closes_to_just_inside_melee() {
        // Target 20 yds east; one full run tick (RUN*4 = 28 yd cap) easily reaches but must STOP 4 yd
        // short → lands at x≈16 (20 - 4), never on the target.
        let (x, y) = chase_step(0.0, 0.0, 20.0, 0.0, constants::speeds::RUN * 4.0, 4.0);
        assert!(
            (x - 16.0).abs() < 0.01,
            "x {x} expected ~16.0 (4 yd short of the target)"
        );
        assert!(
            (y - 0.0).abs() < 0.01,
            "y {y} expected ~0.0 (straight-line approach)"
        );

        // Far target (100 yds east) with the same cap → moves at most one tick of run (28 yd), so it
        // advances to x≈28, NOT all the way (the per-tick cap, not stop_dist, bounds this leg).
        let (x, y) = chase_step(0.0, 0.0, 100.0, 0.0, constants::speeds::RUN * 4.0, 4.0);
        assert!(
            (x - 28.0).abs() < 0.01,
            "x {x} expected ~28.0 (one tick of run, capped)"
        );
        assert!((y - 0.0).abs() < 0.01, "y {y} expected ~0.0");

        // Already within stop_dist (3 yds away, stop_dist 4) → no move (returns the start point), so a
        // creature on top of its target never steps onto / past it.
        let (x, y) = chase_step(0.0, 0.0, 3.0, 0.0, 28.0, 4.0);
        assert_eq!((x, y), (0.0, 0.0), "within stop_dist must hold position");

        // Exactly at stop_dist (4 yds, stop_dist 4) → `len <= stop_dist` → no move (boundary holds).
        let (x, y) = chase_step(0.0, 0.0, 4.0, 0.0, 28.0, 4.0);
        assert_eq!(
            (x, y),
            (0.0, 0.0),
            "at the stop_dist boundary must hold position"
        );

        // Coincident creature/target (len 0 ≤ any stop_dist) → caught by the early return, no NaN /
        // no divide-by-zero — holds position.
        let (x, y) = chase_step(5.0, 5.0, 5.0, 5.0, 28.0, 4.0);
        assert_eq!(
            (x, y),
            (5.0, 5.0),
            "coincident must hold position, never NaN"
        );
        assert!(x.is_finite() && y.is_finite());

        // Diagonal approach: target at (30,40) is 50 yds away (3-4-5); stop 5 yd short → travel 45 yd
        // along the unit vector (0.6, 0.8) → (27, 36). Confirms the direction normalization holds off-axis.
        let (x, y) = chase_step(0.0, 0.0, 30.0, 40.0, 100.0, 5.0);
        assert!((x - 27.0).abs() < 0.01, "x {x} expected ~27.0");
        assert!((y - 36.0).abs() < 0.01, "y {y} expected ~36.0");
    }

    #[test]
    fn wander_point_stays_within_radius_of_home() {
        let (hx, hy) = (-100.0_f32, 250.0_f32);
        let radius = WANDER_RADIUS; // 6.0
        let eps = 1e-3; // float slop on the angle/radius mapping
                        // A spread of `(rand_a, rand_b)` pairs across the u32 span: the destination is always within
                        // `radius` of home — the radius scalar is bounded to [0, radius], so distance never exceeds it.
        let rolls = [
            (0u32, 0u32),
            (u32::MAX, u32::MAX),
            (u32::MAX / 2, u32::MAX / 4),
            (1, u32::MAX),
            (u32::MAX, 1),
            (123_456_789, 987_654_321),
            (0xDEAD_BEEF, 0x1234_5678),
            (u32::MAX / 3, u32::MAX / 7),
        ];
        for (ra, rb) in rolls {
            let (px, py) = wander_point(hx, hy, ra, rb, radius);
            let d = ((px - hx).powi(2) + (py - hy).powi(2)).sqrt();
            assert!(
                d <= radius + eps,
                "wander dist {d} exceeds radius {radius} for rolls ({ra},{rb}) → ({px},{py})"
            );
            assert!(
                px.is_finite() && py.is_finite(),
                "non-finite point for rolls ({ra},{rb})"
            );
        }
        // `rand_b == 0` → radius scalar 0 → the point IS home (any angle), the tightest hop.
        let (px, py) = wander_point(hx, hy, 9999, 0, radius);
        assert!(
            (px - hx).abs() < eps && (py - hy).abs() < eps,
            "rand_b 0 must land on home"
        );
        // Max radius roll → sits ~on the rim (≤ radius), confirming the upper bound is reached, not
        // overshot.
        let (px, py) = wander_point(hx, hy, 0, u32::MAX, radius);
        let d = ((px - hx).powi(2) + (py - hy).powi(2)).sqrt();
        assert!(
            (d - radius).abs() < eps,
            "max rand_b should sit on the rim, dist {d} vs {radius}"
        );
    }

    #[test]
    fn assist_radius_is_inclusive_boundary() {
        // 7 yds apart (along x) → inside the 10-yard radius → assists.
        assert!(within_assist_radius(0.0, 0.0, 7.0, 0.0));
        // Exactly 10 yds apart → boundary, `<=` → still assists.
        assert!(within_assist_radius(0.0, 0.0, 10.0, 0.0));
        // 12 yds apart → outside the radius → no assist.
        assert!(!within_assist_radius(0.0, 0.0, 12.0, 0.0));
        // Same point (0 distance) → assists.
        assert!(within_assist_radius(3.0, 4.0, 3.0, 4.0));
    }

    #[test]
    fn wounded_slow_factor_curve() {
        let approx = |a: f32, b: f32| (a - b).abs() < 1e-3;
        // ≥30% HP → no slow.
        assert!(approx(wounded_slow_factor(30, 100), 1.0));
        assert!(approx(wounded_slow_factor(100, 100), 1.0));
        // cmangos curve below 30%: 15% → ×0.7495 (5.25 yd/s at 7.0 run < the 7.0 player → catchable),
        // 10% → ×0.666, near-death floored ~0.5.
        assert!(approx(wounded_slow_factor(15, 100), 0.7495));
        assert!(approx(wounded_slow_factor(10, 100), 0.666));
        assert!(approx(wounded_slow_factor(1, 100), 0.5157));
        // A fleeing mob is always sub-15% HP → ALWAYS strictly slower than the 7.0 player.
        assert!(wounded_slow_factor(14, 100) < 1.0);
        assert!(wounded_slow_factor(1, 100) >= 0.499); // never inverts
                                                       // Degenerate max_health 0 → safe ×1.0 (no divide-by-zero).
        assert!(approx(wounded_slow_factor(0, 0), 1.0));
    }

    /// Mutation target: an `||` instead of `&&`, or dropping the z arm, and a corrupted coordinate
    /// reaches the entity row again — where it is invisible to the tick and unshakeable in combat.
    #[test]
    fn finite_point_rejects_every_non_finite_coordinate() {
        assert!(finite_point(-8949.9, -132.4, 83.5));
        assert!(!finite_point(f32::INFINITY, 0.0, 0.0));
        assert!(!finite_point(0.0, f32::NEG_INFINITY, 0.0));
        assert!(!finite_point(0.0, 0.0, f32::NAN));
        assert!(!finite_point(f32::NAN, f32::NAN, f32::NAN));
    }

    /// A godmoded GM is invisible to creature AI. Mutation target: drop the `!godmode` arm and a GM
    /// standing in a starting zone re-collects the entire local creature population, because nothing
    /// ever kills them and nothing ever leashes home.
    #[test]
    fn godmode_and_dead_players_are_not_aggro_candidates() {
        assert!(
            is_aggro_candidate(false, false),
            "an ordinary live player IS a candidate"
        );
        assert!(!is_aggro_candidate(true, false), "a corpse is not");
        assert!(!is_aggro_candidate(false, true), "a godmoded GM is not");
        assert!(!is_aggro_candidate(true, true));
    }

    #[test]
    fn spline_t_interpolates_and_clamps() {
        let approx = |a: f32, b: f32| (a - b).abs() < 1e-4;
        // 1000ms leg starting at micros 0: t tracks elapsed fraction.
        assert!(approx(spline_t(0, 0, 1000), 0.0));
        assert!(approx(spline_t(250_000, 0, 1000), 0.25));
        assert!(approx(spline_t(500_000, 0, 1000), 0.5));
        assert!(approx(spline_t(1_000_000, 0, 1000), 1.0));
        // Past the end clamps to 1.0 (lands exactly on dest, no overshoot).
        assert!(approx(spline_t(2_000_000, 0, 1000), 1.0));
        // now < start (clock skew / not-yet-begun) clamps to 0.0 via saturating_sub.
        assert!(approx(spline_t(0, 500_000, 1000), 0.0));
        // 0-duration leg = snap (t=1), no divide-by-zero.
        assert!(approx(spline_t(0, 0, 0), 1.0));
        // Lerp sanity: pos at t=0.25 over (0)->(10) = 2.5.
        let t = spline_t(250_000, 0, 1000);
        assert!(approx(0.0 + (10.0 - 0.0) * t, 2.5));
    }

    #[test]
    fn chase_step_at_tick_scale_glues_to_a_fleeing_target() {
        let run = constants::speeds::RUN * MOVE_TICK_SECS; // 3.5yd per 0.5s tick
                                                           // One step toward a 20yd target advances ~3.5yd — a short, per-tick leg.
        let (x, _) = chase_step(0.0, 0.0, 20.0, 0.0, run, 4.0);
        assert!((x - 3.5).abs() < 0.01, "x {x} expected ~3.5 (RUN*0.5 step)");
        // Chasing a target fleeing +0.5yd/tick: the creature closes and stays within ~one leg (~4-5yd) —
        // the glued-pursuit property of re-pathing toward the live target every tick.
        let (mut cx, mut tx) = (0.0f32, 20.0f32);
        for _ in 0..20 {
            cx = chase_step(cx, 0.0, tx, 0.0, run, 4.0).0;
            tx += 0.5;
        }
        assert!(
            tx - cx <= 5.0,
            "should stay glued within ~one leg, gap {}",
            tx - cx
        );
    }

    // =========================================================================================
    //  Work-item 229 — per-instance tick scope + cadence-generalized sense/step math
    // =========================================================================================

    /// Pins the sentinel to u64::MAX — the schema `#[default(18_446_744_073_709_551_615u64)]` on
    /// `CreatureMoveSchedule.instance_id` (tick.rs) is this value spelled as a literal (the macro
    /// wants a literal); if either side ever drifts, the auto-migrated seeded row would silently stop
    /// being the catch-all and every creature would freeze (no covering row).
    #[test]
    fn global_tick_instance_sentinel_is_u64_max() {
        assert_eq!(GLOBAL_TICK_INSTANCE, u64::MAX);
        assert_eq!(GLOBAL_TICK_INSTANCE, 18_446_744_073_709_551_615u64);
    }

    #[test]
    fn tick_scope_dedicated_row_covers_exactly_its_instance() {
        let rows = [GLOBAL_TICK_INSTANCE, 7, 9];
        let scope = TickScope::from_rows(7, rows);
        assert!(scope.covers(7), "a dedicated row covers its own instance");
        assert!(!scope.covers(0), "…and never the open world");
        assert!(!scope.covers(9), "…and never a SIBLING dedicated instance");
        assert!(
            !scope.covers(3),
            "…and never an unrelated instance (that's the catch-all's)"
        );
        assert!(
            !scope.runs_global_passes(),
            "due-time passes (regen/decay/respawn) run only on the catch-all firing"
        );
    }

    #[test]
    fn tick_scope_catch_all_skips_dedicated_rows_and_covers_everything_else() {
        let rows = [GLOBAL_TICK_INSTANCE, 7, 9];
        let scope = TickScope::from_rows(GLOBAL_TICK_INSTANCE, rows);
        assert!(
            scope.covers(0),
            "the catch-all covers instance 0 (the open world)"
        );
        // THE ANTI-STRANDING CASE the review hunts for: an entity teleported (224) into instance 3,
        // which has NO game_instance row and NO dedicated tick row — still covered by the catch-all.
        assert!(
            scope.covers(3),
            "an instance with no row of its own is covered by the catch-all"
        );
        assert!(
            scope.covers(u64::MAX - 1),
            "…even a pathological huge instance id"
        );
        assert!(
            !scope.covers(7),
            "the catch-all SKIPS an instance with a dedicated row"
        );
        assert!(!scope.covers(9));
        assert!(scope.runs_global_passes());
    }

    /// The no-multiplication invariant: across ALL schedule rows, every instance id is covered by
    /// EXACTLY ONE firing scope — a dedicated row DIVIDES the work, it never adds a second ticker for
    /// the same creature (which would, e.g., double a chaser's speed).
    #[test]
    fn tick_scope_partitions_every_instance_exactly_once() {
        let rows = [GLOBAL_TICK_INSTANCE, 7, 9];
        let scopes: Vec<TickScope> = rows
            .iter()
            .map(|&r| TickScope::from_rows(r, rows))
            .collect();
        for instance in [0u64, 1, 3, 7, 9, 42, u64::MAX - 1] {
            let covering = scopes.iter().filter(|s| s.covers(instance)).count();
            assert_eq!(
                covering, 1,
                "instance {instance} must be covered by exactly one scope"
            );
        }
    }

    /// The work-item 233 equivalence-spec pattern, applied to 229: with ONLY the seeded catch-all row
    /// and every entity at instance 0 (today's world), the scope filter admits the IDENTICAL visit
    /// set — the passes behave byte-identically to the pre-229 code (the gate is `covers() == true`
    /// for every candidate, and the global passes still run).
    #[test]
    fn tick_scope_default_config_visits_the_identical_candidate_set() {
        let scope = TickScope::from_rows(GLOBAL_TICK_INSTANCE, [GLOBAL_TICK_INSTANCE]);
        // (guid, instance_id) candidates — all at instance 0, like every live entity today.
        let candidates: Vec<(u64, u64)> = (0..50).map(|g| (g, 0u64)).collect();
        let visited: Vec<(u64, u64)> = candidates
            .iter()
            .copied()
            .filter(|&(_, inst)| scope.covers(inst))
            .collect();
        assert_eq!(
            visited, candidates,
            "default config must not drop a single candidate"
        );
        assert!(
            scope.runs_global_passes(),
            "…and the global passes still run every firing"
        );
    }

    /// At the DEFAULT interval, the generalized sense gate must fire on exactly the documented boundary
    /// — `(now / MOVE_TICK_MICROS) % SENSE_EVERY_N_TICKS == 0` — the "one movement tick in every N is a
    /// sense tick" contract this fn exists to provide, asserted directly (not via a second, parallel
    /// implementation) across aligned AND unaligned timestamps.
    #[test]
    fn sense_tick_for_interval_fires_on_the_documented_boundary_at_default_cadence() {
        for step in 0..200i64 {
            let now = step * 137_777; // deliberately NOT a multiple of the tick interval
            let expected = (now / MOVE_TICK_MICROS) % SENSE_EVERY_N_TICKS == 0;
            assert_eq!(
                is_sense_tick_for_interval(now, MOVE_TICK_MICROS),
                expected,
                "divergence at now={now}"
            );
        }
    }

    #[test]
    fn sense_tick_for_interval_keeps_the_4s_cadence_for_fast_and_slow_rows() {
        // A 100ms row: exactly ONE sense firing per 4s window (40 firings), not five per window.
        let hits = (0..40i64)
            .filter(|t| is_sense_tick_for_interval(t * 100_000, 100_000))
            .count();
        assert_eq!(hits, 1, "a 100ms row senses once per ~4s, not per firing");
        // A row SLOWER than the sense period (10s): every firing senses (divisor floors at 1).
        assert!(is_sense_tick_for_interval(10_000_000, 10_000_000));
        assert!(is_sense_tick_for_interval(20_000_000, 10_000_000));
        // A ScheduleAt::Time row / garbage interval clamps to the default cadence (never div-by-zero) —
        // asserted by comparing against the SAME fn given the default interval explicitly.
        assert_eq!(
            is_sense_tick_for_interval(0, 0),
            is_sense_tick_for_interval(0, MOVE_TICK_MICROS)
        );
        assert_eq!(
            is_sense_tick_for_interval(4_000_000, -5),
            is_sense_tick_for_interval(4_000_000, MOVE_TICK_MICROS)
        );
    }

    /// The cadence-invariant-speed spec: per-firing step seconds scale with the interval, so
    /// (firings/sec × yards/firing) is constant — a tight tick smooths, it never speeds up.
    #[test]
    fn tick_secs_for_interval_is_exact_at_default_and_scales_linearly() {
        // EXACT equality at the default (0.5 is representable; 500000/1000000 rounds to nothing) —
        // this is what makes the default-row legs byte-identical.
        assert_eq!(tick_secs_for_interval(MOVE_TICK_MICROS), MOVE_TICK_SECS);
        assert_eq!(tick_secs_for_interval(100_000), 0.1);
        assert_eq!(tick_secs_for_interval(1_000_000), 1.0);
        // Net speed check: legs/sec × yd/leg is the same at both cadences.
        let run = constants::speeds::RUN;
        let default_net = (1.0 / MOVE_TICK_SECS) * (run * tick_secs_for_interval(MOVE_TICK_MICROS));
        let fast_net = (1.0 / 0.1) * (run * tick_secs_for_interval(100_000));
        assert!(
            (default_net - fast_net).abs() < 1e-4,
            "net yd/s must be cadence-invariant"
        );
        // Non-positive clamps to the default (mirrors the sense-gate clamp).
        assert_eq!(tick_secs_for_interval(0), MOVE_TICK_SECS);
        assert_eq!(tick_secs_for_interval(-1), MOVE_TICK_SECS);
    }

    #[test]
    fn fear_step_scales_with_tick_and_is_identity_at_default() {
        assert_eq!(
            fear_step_for_tick(MOVE_TICK_SECS),
            FEAR_STEP,
            "default tick → verbatim FEAR_STEP"
        );
        assert!(
            (fear_step_for_tick(0.1) - FEAR_STEP * 0.2).abs() < 1e-5,
            "100ms tick → 1/5 the dash"
        );
    }

    /// The sense-period span (pet follow legs) must equal the ACTUAL spacing between sense ticks of
    /// the row — `every_n × interval` with the same floor as `is_sense_tick_for_interval` — or a
    /// sense-tick pass emits overlapping legs (the 229-review pet bug: a 2.5s row senses EVERY
    /// firing, so a hardcoded-4s leg re-emitted every 2.5s closed at ~1.6× run).
    #[test]
    fn sense_period_matches_actual_sense_tick_spacing() {
        // Default 500ms row: exactly the old 4s (byte-identical pet legs).
        assert_eq!(
            sense_period_secs_for_interval(MOVE_TICK_MICROS),
            MOVE_TICK_SECS * SENSE_EVERY_N_TICKS as f32
        );
        // Fast rows quantize to exactly 4s (100ms × 40, 200ms × 20).
        assert_eq!(sense_period_secs_for_interval(100_000), 4.0);
        assert_eq!(sense_period_secs_for_interval(200_000), 4.0);
        // A 2.5s row senses every firing (floor at 1) → its sense period IS 2.5s, not 4s.
        assert_eq!(sense_period_secs_for_interval(2_500_000), 2.5);
        // A 10s row: every firing senses → 10s span (a slower row never emits overlapping legs).
        assert_eq!(sense_period_secs_for_interval(10_000_000), 10.0);
        // A 3s row: floor(4s/3s)=1 → 3s. A 707ms row: floor(4s/707ms)=5 → 3.535s.
        assert_eq!(sense_period_secs_for_interval(3_000_000), 3.0);
        assert!((sense_period_secs_for_interval(707_000) - 3.535).abs() < 1e-4);
        // Non-positive clamps to the default (mirrors the other cadence helpers).
        assert_eq!(
            sense_period_secs_for_interval(0),
            MOVE_TICK_SECS * SENSE_EVERY_N_TICKS as f32
        );
    }

    #[test]
    fn sense_tick_is_one_in_eight() {
        let hits: Vec<bool> = (0..16)
            .map(|t| is_sense_tick_for_interval(t * MOVE_TICK_MICROS, MOVE_TICK_MICROS))
            .collect();
        assert_eq!(
            hits.iter().filter(|&&h| h).count(),
            2,
            "2 sense ticks in 16 (1-in-8)"
        );
        assert!(hits[0] && hits[8], "ticks 0 and 8 are sense ticks");
        assert!(!hits[1] && !hits[7], "the 7 between are movement-only");
    }

    #[test]
    fn leg_in_flight_gate() {
        assert!(leg_in_flight(100, 600)); // mid-leg
        assert!(!leg_in_flight(600, 600)); // exactly at ETA → arrived
        assert!(!leg_in_flight(700, 600)); // past ETA
        assert!(!leg_in_flight(100, 0)); // fresh / no leg
    }

    #[test]
    fn rout_window_open_covers_the_whole_clock_lifecycle() {
        // Never started (the zero sentinel) → not routing, at any `now`.
        assert!(!rout_window_open(0, 0));
        assert!(!rout_window_open(50_000, 0));
        // Open: the close time is still ahead.
        assert!(rout_window_open(50_000, 60_000));
        // Just closed (exactly at the close time) and closed long ago → over.
        assert!(!rout_window_open(60_000, 60_000));
        assert!(!rout_window_open(600_000, 60_000));
    }

    #[test]
    fn a_rout_starts_once_per_engagement_and_only_when_eligible() {
        // Eligible with an unstarted clock → the rout starts.
        assert!(may_start_rout(true, 0));
        // Eligible but the clock is already spent (a window that closed) → never a second rout, at
        // any health. An open window is not a start either — the rout is already running.
        assert!(!may_start_rout(true, 60_000));
        // Ineligible (a BEAST, or above the HP threshold) never starts one.
        assert!(!may_start_rout(false, 0));
        assert!(!may_start_rout(false, 60_000));
    }

    #[test]
    fn a_rout_window_never_stamps_the_never_started_sentinel() {
        // The close time is the stamp instant plus the duration.
        assert_eq!(rout_close_ms(50_000), 50_000 + ROUT_DURATION_MS);
        // A stamp that wraps the u32 clock exactly onto 0 would read as "never started" and hand out
        // a second rout, so it is nudged off the sentinel.
        assert_eq!(rout_close_ms(u32::MAX - ROUT_DURATION_MS + 1), 1);
    }

    #[test]
    fn evade_needs_both_an_expired_deadline_and_a_target_that_left_the_leash() {
        let near = crate::combat::LEASH_RADIUS_SQ - 1.0;
        let far = crate::combat::LEASH_RADIUS_SQ + 1.0;
        // Deadline still live: the fight continues at ANY distance inside the backstop — the whole
        // point of the timer model (a kited mob keeps chasing while damage flows).
        assert!(!should_evade(50_000, 60_000, near));
        assert!(!should_evade(50_000, 60_000, far));
        // Deadline expired but the target is still standing by the remembered spot → keep fighting.
        assert!(!should_evade(60_000, 60_000, near));
        // Expired AND the target left the leash radius → the only evading corner.
        assert!(should_evade(60_000, 60_000, far));
    }

    #[test]
    fn the_backstop_is_the_only_raw_distance_that_still_ends_a_fight() {
        let backstop = crate::combat::COMBAT_BACKSTOP_SQ;
        assert!(!beyond_combat_backstop(backstop)); // exactly at the drop distance is still combat
        assert!(beyond_combat_backstop(backstop + 1.0));
        // A target FAR outside the leash radius but inside the backstop never evades on distance while
        // its deadline is live — the whole point of dropping the hard cap.
        assert!(!should_evade(50_000, 60_000, backstop - 1.0));
    }

    #[test]
    fn a_pursuit_deadline_never_stamps_the_never_refreshed_sentinel() {
        assert_eq!(
            pursuit_deadline_ms(50_000),
            50_000 + crate::combat::PURSUIT_WINDOW_MS
        );
        // A deadline that wraps the u32 clock exactly onto 0 would read as "never refreshed" — a row
        // the leash pass would re-seed for another full window — so it is nudged off the sentinel.
        assert_eq!(
            pursuit_deadline_ms(u32::MAX - crate::combat::PURSUIT_WINDOW_MS + 1),
            1
        );
        // A stamped deadline is always in the future of the stamp, so a refresh always defers the evade.
        assert!(!should_evade(
            50_000,
            pursuit_deadline_ms(50_000),
            f32::INFINITY
        ));
    }

    /// The tuning constants are only coherent together, so pin the relations rather than the values.
    /// `const` blocks: every relation is between constants, so a bad calibration fails the BUILD rather
    /// than waiting for the run (and clippy rejects a runtime assert over constants).
    #[test]
    fn the_leash_constants_hold_their_invariants() {
        const {
            // A rout must not outlive the engagement that carries it: a router whose pursuit window
            // expired mid-rout would evade and heal to full — the reported bug, in timer form.
            assert!(ROUT_DURATION_MS < crate::combat::PURSUIT_WINDOW_MS);
            // The leash radius is the small radius around the remembered spot; the backstop is the far
            // "across a zone" cutoff. Inverted, the backstop would be unreachable.
            assert!(crate::combat::LEASH_RADIUS_SQ < crate::combat::COMBAT_BACKSTOP_SQ);
            // The Auto Shot invariant, restated against the backstop: a pull from maximum ranged range
            // must start inside the only distance that evades. The leash radius may sit below the pull
            // range — the pursuit deadline, stamped on the pull, is what holds that engagement open.
            assert!(crate::combat::RANGED_RANGE_SQ < crate::combat::COMBAT_BACKSTOP_SQ);
        }
    }

    #[test]
    fn waypoint_cursor_traverses_in_order() {
        // next wraps 0→1→2→3→0 — ordered patrol traversal.
        assert_eq!(next_waypoint_idx(0, 4), 1);
        assert_eq!(next_waypoint_idx(2, 4), 3);
        assert_eq!(next_waypoint_idx(3, 4), 0);
        // nearest picks the closest waypoint as the first target when there's no live cursor.
        let pts = [(0.0, 0.0), (10.0, 0.0), (20.0, 0.0), (30.0, 0.0)];
        assert_eq!(nearest_waypoint_idx(19.0, 0.0, &pts), 2);
        assert_eq!(nearest_waypoint_idx(-5.0, 0.0, &pts), 0);
        // A full lap visits EVERY waypoint.
        let mut idx = nearest_waypoint_idx(0.0, 0.0, &pts);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..4 {
            seen.insert(idx);
            idx = next_waypoint_idx(idx, pts.len());
        }
        assert_eq!(seen.len(), 4, "ordered traversal visits all waypoints");
    }
}
