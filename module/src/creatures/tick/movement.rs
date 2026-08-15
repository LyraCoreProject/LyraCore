//! The RUN/step movement legs (issue #383 split of tick.rs): patrol, chase, return-to-spawn,
//! wander, flee, fear-flee. Each pass is called from `tick_creatures`'s pipeline in `tick/mod.rs`
//! — see that file's module doc for the load-bearing pass ORDER, which this split does not change
//! (every pass here keeps its original name and signature, called by bare name via `tick/mod.rs`'s
//! `use movement::*;`).
//!
//! Shares the movement-leg grammar (`PendingLeg` / `leg_toward` / `drain_legs` / `movable_creature`),
//! the one spline writer (`emit_creature_leg`), and `creature_is_routing` with `tick/mod.rs` and
//! `sense.rs` — all defined in `tick/mod.rs` and reachable here as private ancestor items (Rust
//! visibility: private == visible in the defining module + every descendant), no re-export needed.

use lyracore_shared::constants;
use spacetimedb::{ReducerContext, Table};

use crate::{game_melee_attack, game_spell, game_world_entity};

use super::*;

/// Pass 1 — patrol: an idle creature with >=2 waypoints walks its route IN ORDER, one segment per leg.
/// ETA-gated (`leg_in_flight`): while the current leg is still animating it emits nothing, so a leg
/// plays to completion instead of being re-thrown every tick. `wp_target` is the route cursor (the
/// `game_creature_waypoint.id` it's walking TO); a fresh/stale cursor re-acquires the NEAREST waypoint,
/// then advances 0→1→…→0. Skips dead, engaged, and self-movement-suppressed creatures (combat AI /
/// fear-flee own those). Work-item 230: SCOPED to `active` — a patrolling creature outside every
/// player's active-cell neighborhood is DORMANT (no leg emitted). This is always safe to freeze: the
/// authoritative row is already snapshotted at the END of its current leg the instant that leg was
/// emitted (see the position writes below — the client is still mid-ANIMATION, but the server's stored
/// x/y/z/`wp_target` are already the completed state), so skipping a dormant tick never leaves the row
/// in a half-applied state. On re-entry the route resumes from the stored `wp_target` cursor exactly as
/// if it had never paused.
/// Work-item 229: instance scope is inherited from `active` (built from covered players only) — no
/// separate gate needed. Returns candidates visited (guids past the active gate).
pub(crate) fn pass_patrol(ctx: &ReducerContext, active: &std::collections::HashSet<u64>) -> usize {
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    let entities = ctx.db.game_world_entity();
    let waypoints = ctx.db.game_creature_waypoint();

    // Perf catalog 1.4: INVERTED. This used to scan + sort + dedup the ENTIRE waypoint table (a full
    // cmangos import carries 100k+ `creature_movement` rows) on every 500ms firing, only to throw away
    // everything outside the active set two lines later. Now the small, player-density-scaled `active`
    // set is the outer loop and `by_creature` probes patrol membership — the same candidate set
    // (`active` ∩ has-waypoints), discovered from the cheap side, exactly like `pass_return` does.
    // Sorted so the visit order stays PK-ascending like the old scan's (per-creature work is
    // independent, but a stable order keeps the emitted move-event ids deterministic).
    let mut guids: Vec<u64> = active
        .iter()
        .copied()
        .filter(|g| waypoints.by_creature().filter(g).next().is_some())
        .collect();
    guids.sort_unstable();

    let mut visited = 0usize;
    for guid in guids {
        visited += 1;
        let Some(mut e) = entities.guid().find(guid) else {
            continue;
        };
        // A dead creature (corpse) holds position until it decays — it must not patrol.
        if e.dead {
            continue;
        }
        // A creature engaged in melee yields its patrol to the combat AI (chase/flee pass).
        if crate::combat::is_engaged(ctx, guid) {
            continue;
        }
        // Crowd control: a FROZEN/FEARED creature doesn't patrol (fear-flee walks a feared one instead).
        if crate::spell::is_self_movement_suppressed(ctx, guid) {
            continue;
        }
        // ETA gate: the current segment leg is still animating — don't re-emit or jump. This is what
        // makes a patrol leg play to completion instead of being re-thrown every 0.5s tick (the dither).
        if leg_in_flight(now_ms, e.leg_ends_ms) {
            continue;
        }
        // The route IN ORDER (waypoint PK `id` ASC = insertion order). Carry the id so the cursor tracks
        // WHICH waypoint we're walking to, not just its coords — that's what keeps the traversal in
        // route order instead of jumping to whichever waypoint is farthest.
        let mut wps: Vec<(u64, f32, f32, f32)> = waypoints
            .by_creature() // perf catalog 1.4 — was another full waypoint scan, PER candidate
            .filter(&guid)
            .map(|w| (w.id, w.x, w.y, w.z))
            .collect();
        if wps.len() < 2 {
            continue;
        }
        wps.sort_unstable_by_key(|w| w.0);
        // Next destination: the waypoint AFTER the cursor (`wp_target`), wrapping; if the cursor is
        // unset/stale (0, or not in this route — fresh spawn), acquire the NEAREST waypoint first.
        let pts: Vec<(f32, f32)> = wps.iter().map(|w| (w.1, w.2)).collect();
        let dest_idx = match wps.iter().position(|w| w.0 == e.wp_target) {
            Some(i) => next_waypoint_idx(i, wps.len()),
            None => nearest_waypoint_idx(e.x, e.y, &pts),
        };
        let (wid, dx, dy, dz) = wps[dest_idx];
        let (ddx, ddy, ddz) = (dx - e.x, dy - e.y, dz - e.z);
        let dist = (ddx * ddx + ddy * ddy + ddz * ddz).sqrt();
        if dist < 0.5 {
            // Already on the next waypoint (e.g. just acquired the nearest) — advance the cursor and
            // skip the zero-length leg the client rejects; next tick walks to the following waypoint.
            e.wp_target = wid;
            entities.guid().update(e);
            continue;
        }
        let duration_ms = ((dist / constants::speeds::WALK) * 1000.0) as u32;
        // Advance the route cursor on the row, then funnel through the ONE shared writer (walk, arms
        // the ETA gate). 181: patrol now GROUND-SNAPS its leg like every other pass (work-item 174) —
        // the raw waypoint dz is only the OFF-SLICE fallback; on terrain-enabled slices the dest z is
        // the ground under the waypoint (== dz wherever the waypoint already sits on the ground).
        e.wp_target = wid;
        emit_creature_leg(ctx, e, (dx, dy), dz, duration_ms, false, now_ms, true);
    }
    visited
}


/// Pass 7 — chase: an engaged creature whose target sits OUT of melee but inside the chase cutoff steps
/// toward it (a run leg). Runs AFTER aggro (a creature aggroed this tick can start closing) and BEFORE regen
/// (regen's in-combat gate then skips the still-engaged chaser, so the move isn't reverted).
///
/// Work-item 230 classification: ALWAYS ACTIVE, no active-cell gate — the item calls this pass out by
/// name ("make sure the movement/chase pass doesn't sleep an engaged creature"). It already iterates
/// `game_melee_attack`'s engaged rows (never the full entity table), so it was O(active combats) before
/// this item and stays that way — a player who drags a creature outside every active cell must still
/// see it keep chasing/swinging.
/// Work-item 229: gated per candidate on `scope.covers(c.instance_id)` — WITHOUT this gate a
/// dedicated instance row + the catch-all would BOTH step the same chaser each of their firings
/// (double/6× chase speed), the multiplied-work bug this item exists to prevent.
///
/// Issue #383: dropped the `_tick_secs` parameter this pass used to accept but never read. Re-checked
/// against the CURRENT geometry below, not the stale doc it replaced: this pass commits to one long
/// "aim past the target" leg (`leg_len` — `CHASE_LEAD_YD` past a moving target, or the exact gap to
/// melee reach for a stationary one) and only re-emits when that leg finishes or the target veers off
/// its heading (`CHASE_REPATH_COS`) — never "one tick of run" sized off `MOVE_TICK_SECS`. A dedicated
/// row firing faster just re-checks `repath` more often; between re-emits it's a no-op, so the actual
/// travel distance/speed is already cadence-invariant. Returns covered candidates visited.
/// 049: the longest ENEMY-cast range (yd) creature `entry` can bring to bear — rotation
/// nukes/debuffs (conditions ALWAYS / TARGET_MISSING_AURA target the melee victim) plus the
/// legacy `game_creature_cast` spell, mapped through `game_spell.range_yd`. 0 = not an
/// offensive caster (melee-only mobs, pure healers/buff-bots) — the chase pass is then
/// byte-identical to pre-049.
fn caster_hold_range_yd(ctx: &ReducerContext, entry: u32) -> f32 {
    let spells = ctx.db.game_spell();
    let mut max_r = 0u32;
    for r in ctx.db.game_creature_spell().by_entry().filter(&entry) {
        if matches!(
            r.condition,
            cast_condition::ALWAYS | cast_condition::TARGET_MISSING_AURA
        ) {
            if let Some(h) = spells.spell_id().find(r.spell_id) {
                max_r = max_r.max(h.range_yd);
            }
        }
    }
    if let Some(c) = ctx.db.game_creature_cast().creature_entry().find(entry) {
        if let Some(h) = spells.spell_id().find(c.spell_id) {
            max_r = max_r.max(h.range_yd);
        }
    }
    max_r as f32
}

/// #518: minimum bearing drift (radians, ~17°) before a stationary stand-and-swing creature turns
/// to face its target. Below this the mob is already close enough that a correction would be
/// imperceptible AND would re-throw a facing packet every tick (a stationary fight has to settle
/// into silence, same discipline as the committed-leg flee-spin fix above) — the epsilon exists
/// purely to make "have I already turned to face it" idempotent tick over tick.
const FACING_EPSILON_RAD: f32 = 0.3;

/// Shortest signed angular distance from `from` to `to`, wrapped into `(-PI, PI]` — so a bearing
/// that crosses the ±PI seam (e.g. orientation 3.0, target bearing -3.0) reads as a small turn, not
/// a near-full-circle one. Pure — no I/O, unit-tested directly.
fn angle_diff(from: f32, to: f32) -> f32 {
    let two_pi = std::f32::consts::TAU;
    let mut d = (to - from) % two_pi;
    if d > std::f32::consts::PI {
        d -= two_pi;
    } else if d < -std::f32::consts::PI {
        d += two_pi;
    }
    d
}

pub(crate) fn pass_chase(ctx: &ReducerContext, scope: &TickScope) -> usize {
    let mut visited = 0usize;
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    let entities = ctx.db.game_world_entity();
    // Chase pass (vanilla creature AI: an engaged mob closes the gap on a target that ran out of
    // melee range). For each ALIVE creature that is the ATTACKER in a `game_melee_attack` row, look up
    // its target and the squared distance. Step it toward the target only when it's OUT of melee but
    // still inside the chase cutoff (`CHASE_MELEE_SQ < dist² <= CHASE_LEASH_SQ`):
    //   - dist² <= melee  → already in range; `tick_melee` swings — chasing would walk onto the target.
    //   - dist²  > cutoff → past the active-cell radius, so leave it: the engagement's own pursuit
    //     timer ends the fight (distance does not), and the return pass walks it home afterwards.
    // The committed leg (below) stops ~4 yd short of a stationary target, landing just inside the 5-yd
    // melee band so the next swing connects; re-aimed toward the LIVE target on every veer (not a
    // 4s-stale snapshot) so it stays glued to a fleeing player.
    //
    // The chaser stays ENGAGED — we do NOT touch its melee row — so the regen passes below skip it
    // (their `in_combat` gate excludes engaged guids), and they never re-write its row from a stale
    // snapshot and REVERT the move. That's why this runs AFTER aggro (so a creature aggroed THIS tick
    // can immediately start closing) and BEFORE regen (so regen sees the post-move row, not the other
    // way round). Snapshot the legs first (collect-then-mutate), so we never write the entity table
    // while iterating it — mirrors the patrol/aggro/flee passes. Emit a `CreatureMoveEvent` leg AND
    // advance the authoritative position, exactly like the patrol pass, so the client animates the
    // close-in via SMSG_MONSTER_MOVE.
    let melee_chase = ctx.db.game_melee_attack();
    let mut to_chase: Vec<PendingLeg> = Vec::new();
    // Iterate the ENGAGED rows (one per attacker; few) rather than scanning every entity — at the 0.5s
    // tick this keeps chase O(active combats), not O(world). The attacker must be a CREATURE (a player's
    // own attack row is skipped via `is_player`).
    for row in melee_chase.iter() {
        // Issue #383: the shared gate ladder (creature, alive, this firing's covered instance).
        let Some(c) = movable_creature(ctx, row.attacker_guid, scope) else {
            continue;
        };
        visited += 1;
        // CC: a FROZEN/FEARED creature doesn't chase (fear-flee walks a feared one AWAY instead). It
        // stays engaged, so a frozen one still swings if the target is already in range.
        if crate::spell::is_self_movement_suppressed(ctx, c.guid) {
            continue;
        }
        // A creature ACTIVELY routing is diverted out of chasing — the flee pass (last) is its sole mover,
        // so it never emits both a chase leg and a flee leg (same spline_id → client rejects the 2nd).
        // Anything else keeps chasing: a non-eligible BEAST, and equally a humanoid whose rout window has
        // closed, which is how a spent router walks back into melee instead of standing frozen at low HP.
        if creature_is_routing(ctx, &c) {
            continue;
        }
        let Some(t) = entities.guid().find(row.target_guid) else {
            continue;
        };
        let (dx, dy, dz) = (t.x - c.x, t.y - c.y, t.z - c.z);
        let dist_sq = dx * dx + dy * dy + dz * dz;
        // Past the active-cell radius → stop pursuing; the pursuit timer, not distance, ends the fight.
        if dist_sq > CHASE_LEASH_SQ {
            continue;
        }
        // STAND + swing ONLY when in melee reach AND the target is STATIONARY. A MOVING (kiting) target
        // keeps the mob CHASING — vanilla mobs run a kiter down continuously and swing on the move; they
        // plant into attack stance only when the target stops. Gating the melee-skip on the target being
        // still is what removes the run→attack-stance→run flicker while chasing a moving player. The
        // melee scheduler still swings whenever in range, so a chasing mob attacks on the move.
        let target_moving = now_ms.wrapping_sub(t.last_move_ms) < CHASE_TARGET_MOVING_MS;
        if dist_sq <= CHASE_MELEE_SQ && !target_moving {
            // Cancel any in-flight leg so a mob that was lead-chasing a now-stopped target doesn't ride
            // PAST it (pass_advance would otherwise finish the leg). Stop the client at the current point.
            if ctx.db.game_creature_spline().guid().find(c.guid).is_some() {
                // 0-dur STOP where it stands — replaces the in-flight leg rather than deleting it,
                // so the client is TOLD to halt instead of being left interpolating to the old dest.
                emit_move_spline(
                    ctx,
                    c.guid,
                    (c.x, c.y, c.z),
                    (c.x, c.y, c.z),
                    0,
                    false,
                    now_ms,
                    c.map_id,
                    c.instance_id,
                    (c.grid_x, c.grid_y),
                );
            }
            // #518: a stand-and-swing creature never throws another movement leg — a spline is the
            // ONLY thing the client ever derives creature facing from, so without this it keeps
            // whatever heading it had when it planted (its pre-combat orientation) for the entire
            // fight, correcting only when the target's next kite step re-triggers a chase spline.
            // Turn it toward the target here instead: epsilon-gated (`FACING_EPSILON_RAD`) so a
            // truly stationary fight settles into silence rather than re-emitting every 500ms tick.
            let bearing = dy.atan2(dx);
            if angle_diff(c.orientation, bearing).abs() > FACING_EPSILON_RAD {
                // Re-find (not a stale clone of `c`) — the shared entities-table-write convention this
                // file already uses (e.g. `sense.rs`'s target-point write), so a concurrent write to
                // this row within the same tick isn't clobbered by an out-of-date snapshot.
                if let Some(mut e) = entities.guid().find(c.guid) {
                    e.orientation = bearing;
                    entities.guid().update(e);
                }
                emit_facing_spline(
                    ctx,
                    c.guid,
                    (c.x, c.y, c.z),
                    bearing,
                    now_ms,
                    c.map_id,
                    c.instance_id,
                    (c.grid_x, c.grid_y),
                );
            }
            continue;
        }
        // 049: an OFFENSIVE CASTER holds at spell range instead of face-tanking — vanilla casters
        // stand and cast (pass_cast already ran this tick; its range/GCD gates fire the spell).
        // Hold iff the target sits inside ~90% of its longest enemy-cast range AND is LoS-visible;
        // a wall-blocked caster keeps chasing (pass_cast drops its enemy candidates on the same
        // LoS test — the two agree, so it closes instead of standing spell-less). The 90% buffer
        // keeps small target strafes inside range so the caster doesn't yo-yo on the boundary.
        // Melee-only creatures (hold range 0) take the exact pre-049 path. CEILING: a school-locked
        // (Kicked) caster still holds through the lockout window instead of closing — rare at 1-10.
        let hold = caster_hold_range_yd(ctx, c.entry);
        if hold > 0.0 {
            let hold_r = hold * 0.9;
            if dist_sq <= hold_r * hold_r
                && crate::nav::has_los(ctx, c.map_id, (c.x, c.y, c.z), (t.x, t.y, t.z))
            {
                continue;
            }
        }
        // SPLINE chase, mangos-style CONTINUOUS follow: commit to one leg and RIDE it for several ticks,
        // re-aiming only when the target VEERS off the current heading — NOT a fresh tiny leg every 500ms
        // tick (which made the client visibly re-compute its path each tick). For a MOVING target we aim a
        // LEAD past it along the chase line, so the leg is long and a straight-running kiter keeps the mob
        // on one held heading; for a STATIONARY target we aim at melee reach and stop. `pass_advance_splines`
        // advances the authoritative position along the leg. Snare-aware.
        let run = crate::combat::effective_move_speed(ctx, c.guid, constants::speeds::RUN);
        let dist_to_t = dist_sq.sqrt();
        let (dir_x, dir_y) = ((t.x - c.x) / dist_to_t, (t.y - c.y) / dist_to_t);
        // Aim point: moving → dist + CHASE_LEAD_YD along the heading (a long, rideable leg); stationary →
        // melee reach (dist - 4), so the mob closes and the stand above stops it.
        let leg_len = if target_moving {
            dist_to_t + CHASE_LEAD_YD
        } else {
            (dist_to_t - 4.0).max(0.0)
        };
        let (ax, ay) = (c.x + dir_x * leg_len, c.y + dir_y * leg_len);
        // 243: nav-aware — nav_step returns the aim point (LoS clear) or the first detour corner (blocked).
        let (nx, ny) = crate::nav::nav_step(ctx, c.map_id, (c.x, c.y), (ax, ay), leg_len, 0.0, c.z);
        if nx == c.x && ny == c.y {
            continue; // already there → nothing to move.
        }
        let active_leg = ctx.db.game_creature_spline().guid().find(c.guid);
        // Re-aim ONLY when: no active leg (it completed), OR the target VEERED off the current leg's
        // heading by more than ~CHASE_REPATH_COS (a turn). A target running roughly straight keeps the
        // same heading → the mob rides ONE leg without re-emitting = smooth, no per-tick recompute.
        let repath = match active_leg {
            None => true,
            Some(sp) => {
                let (lx, ly) = (sp.dx - sp.sx, sp.dy - sp.sy);
                let ll = (lx * lx + ly * ly).sqrt();
                ll <= 0.001 || (lx / ll) * dir_x + (ly / ll) * dir_y < CHASE_REPATH_COS
            }
        };
        if !repath {
            continue;
        }
        // 086: the writer snaps the leg's dest to GROUND at (nx, ny) — t.z is the target's height at
        // the TARGET's position, wrong on slopes and plain nonsense when (nx, ny) is a nav detour
        // corner nowhere near the target — so t.z is only the off-slice FALLBACK (pre-terrain shape).
        let Some((dx2, dy2, duration_ms)) = leg_toward((c.x, c.y), (nx, ny), run) else {
            continue; // degenerate (repath already excludes this in practice, kept for safety).
        };
        to_chase.push(PendingLeg {
            guid: c.guid,
            dest: (dx2, dy2),
            z_fallback: t.z,
            duration_ms,
        });
    }
    // A chase is a RUN and never arms the ETA gate (re-stepped every tick toward the live target).
    drain_legs(ctx, to_chase, true, false, now_ms);
    visited
}

/// Pass 8 — return-to-spawn: a displaced, NON-engaged, waypoint-less creature beyond `RETURN_LEASH_SQ`
/// of its home walks back. WRITES `moved_this_tick` (the guids it moved) so the wander pass can skip
/// them. Runs after chase (engaged creatures already excluded) and before regen (only moves position).
///
/// Work-item 230: SCOPED to `active` — a displaced creature outside every player's active-cell
/// neighborhood is DORMANT (freezes mid-walk-home rather than continuing). Coherent by construction:
/// each step is computed fresh from the CURRENT stored `(x, y)` toward `home` (no elapsed-time or
/// step-count state), so any number of dormant ticks in between changes nothing about how the walk
/// resumes — it's exactly as if the creature paused. Never dormant-izes an engaged creature: this pass
/// already excludes them via `is_engaged` regardless of `active`.
/// Work-item 229: instance scope inherited from `active`; `tick_secs` = the firing row's interval
/// (cadence-invariant walk-home speed, exactly `MOVE_TICK_SECS` on the default row). Returns
/// candidates visited (guids past the active gate).
pub(crate) fn pass_return(
    ctx: &ReducerContext,
    moved_this_tick: &mut std::collections::HashSet<u64>,
    active: &std::collections::HashSet<u64>,
    tick_secs: f32,
) -> usize {
    let mut visited = 0usize;
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    let entities = ctx.db.game_world_entity();
    let waypoints = ctx.db.game_creature_waypoint();

    // Return-to-spawn pass (vanilla creature AI: a creature pushed off its post walks back). This is
    // how a creature that ended up displaced from its home — a future knockback, or a leash that
    // dropped it somewhere far from where it spawned — finds its way back to `game_creature_spawn`.
    // Combined with the chase pass it gives the basic loop: pull → chase → evade (leash disengages) →
    // walk home.
    //
    // SCOPE — this pass moves ONLY creatures that no other mover governs, so it can never fight the
    // patrol/chase/flee legs:
    //   - NOT engaged (`is_engaged` false): the chase/flee passes move only ENGAGED attackers, so a
    //     non-engaged creature is invisible to them — no overlap. A creature mid-pull walks toward its
    //     target (chase), not home.
    //   - NO patrol waypoints: a patrolling creature is governed by the patrol pass (it owns the
    //     creature's idle movement, walking it between waypoints — "home" for it is a waypoint, not the
    //     spawn). We reuse the SAME `by_creature` waypoint lookup the patrol pass uses to detect one.
    //   - ALIVE creature (no PLAYER bit, not dead): a corpse holds position; players move themselves.
    // Snapshot the legs first (collect-then-mutate) so we never write the entity table while iterating
    // it — mirrors the patrol/chase/flee passes.
    //
    // It runs AFTER chase (so an engaged creature is already excluded by `is_engaged`) and BEFORE regen
    // (a returning creature is out of combat, so the regen passes' `in_combat` gate does NOT skip it —
    // but this pass ONLY moves x,y,z/grid/last_move and emits a leg, never touches HP/power, so regen
    // re-writing the row from its own snapshot heals HP without reverting the move: regen reads the
    // post-move row, and even a stale read would only restore HP, not position). It also can't duplicate
    // a chase/flee leg this tick: those move only ENGAGED creatures and this one only NON-engaged ones —
    // the two sets are disjoint, so no creature gets two legs (or two `spline_id`s) in a single tick.
    let spawns_home = ctx.db.game_creature_spawn();
    let mut to_return: Vec<PendingLeg> = Vec::new();
    // ACTIVE CELLS (work-item 230): iterate the pre-computed active-cell guid set instead of the full
    // entity table.
    for guid in active.iter().copied() {
        visited += 1;
        let Some(c) = entities.guid().find(guid) else {
            continue;
        };
        // Creatures only (no PLAYER bit), alive — same predicate as the chase/flee passes.
        if c.is_player() || c.dead {
            continue;
        }
        // Crowd control: a creature whose self-movement is suppressed — FROZEN (stun/root/poly) or FEARED
        // (the fear-flee pass walks it instead) — doesn't walk home (a movement pass). Baseline-safe:
        // `false` without a CC aura → an un-CC'd displaced creature returns exactly as before.
        if crate::spell::is_self_movement_suppressed(ctx, c.guid) {
            continue;
        }
        // Engaged creatures belong to chase/flee — never return-walk them.
        if crate::combat::is_engaged(ctx, c.guid) {
            continue;
        }
        // A creature with patrol waypoints is governed by the patrol pass (same `by_creature` lookup);
        // skip it so we don't drag it off its route toward its spawn.
        if waypoints.by_creature().filter(&c.guid).next().is_some() {
            continue;
        }
        // Its persistent home; no spawn row → nothing to return to.
        let Some(home) = spawns_home.guid().find(c.guid) else {
            continue;
        };
        // Within the return LEASH (squared compare, like the aggro/leash ranges) → home-enough, no
        // move. The roomy (15 yd)² leash leaves the WANDER pass space to hop within `WANDER_RADIUS`
        // (6 yd) of home without return immediately pulling it back — only a creature displaced
        // FARTHER than the leash walks home.
        let (dx, dy) = (home.x - c.x, home.y - c.y);
        if dx * dx + dy * dy <= RETURN_LEASH_SQ {
            continue;
        }
        // Step toward home up to one full tick of run, landing AT spawn if within reach (`stop_dist`
        // 0.0 — unlike chase, we want to arrive ON the home point, not short of it). Snare-aware (a snared
        // creature crawls home slower); no snare → RUN exactly.
        let run = crate::combat::effective_move_speed(ctx, c.guid, constants::speeds::RUN);
        // Step ONE TICK of run toward home (a STEP pass like chase — re-stepped every firing so the
        // walk-home chains smoothly; no ETA gate, since each short leg's duration ≈ the tick interval).
        let (nx, ny) = crate::nav::nav_step(
            ctx,
            c.map_id,
            (c.x, c.y),
            (home.x, home.y),
            run * tick_secs,
            0.0,
            c.z,
        );
        // Ground-snap (in the writer) each intermediate step of the walk home (work-item 174 follow-up,
        // PR-9 review: un-snapped home.z here made a leashed creature float/clip down the slope and
        // Z-pop when the snapped wander pass took over at the post). Off-slice falls back to the spawn's
        // z — the old behavior, landing the leg at the post like patrol legs.
        let Some((dx2, dy2, duration_ms)) = leg_toward((c.x, c.y), (nx, ny), run) else {
            continue; // no-op step (can't close the gap) — skip the zero-length leg the client rejects.
        };
        moved_this_tick.insert(c.guid); // per-pass tail — return owns the wander-skip set
        to_return.push(PendingLeg {
            guid: c.guid,
            dest: (dx2, dy2),
            z_fallback: home.z,
            duration_ms,
        });
    }
    // Return-to-spawn RUNS home; no ETA gate (re-stepped every tick until it arrives). `moved_this_tick`
    // is populated above (per-candidate, not per-drained-leg) so the wander pass below sees the guid
    // even on the — impossible today, but not by construction — chance `drain_legs` no-ops a stale row.
    drain_legs(ctx, to_return, true, false, now_ms);
    visited
}

/// Pass 9 — wander: an idle, NON-engaged, waypoint-less RANDOM-movement creature WITHIN the leash of
/// its home hops to a random point near home on ~`WANDER_CHANCE_PCT` of ticks. READS `moved_this_tick`
/// and SKIPS any guid the return pass already moved (one move leg per creature per tick). Mutually
/// exclusive with return (that moves creatures OUTSIDE the leash, this inside it).
///
/// Work-item 230: SCOPED to `active` — an idle creature outside every player's active-cell
/// neighborhood is DORMANT (no hop rolled). Coherent by construction: the `leg_in_flight` ETA gate and
/// the `WANDER_CHANCE_PCT` roll are both evaluated fresh each active tick against the CURRENT stored
/// `leg_ends_ms`/position, so a creature that goes dormant mid-loiter simply resumes the same idle
/// rhythm whenever it re-enters an active cell — no state depends on how many ticks were skipped.
/// Work-item 229: instance scope inherited from `active`. The hop step stays `walk * 4.0` (the SENSE
/// cadence, ~4s for EVERY row per `is_sense_tick_for_interval`) — not the per-firing `tick_secs` —
/// because this pass runs on sense ticks only. Returns candidates visited (past the active gate).
pub(crate) fn pass_wander(
    ctx: &ReducerContext,
    moved_this_tick: &std::collections::HashSet<u64>,
    active: &std::collections::HashSet<u64>,
) -> usize {
    let mut visited = 0usize;
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    let entities = ctx.db.game_world_entity();
    let waypoints = ctx.db.game_creature_waypoint();

    // Wander pass (vanilla creature AI: an idle creature near its post loiters — hops to random
    // nearby points). For each ALIVE creature (no PLAYER bit, not dead) that is NOT engaged, has NO
    // patrol waypoints, and whose spawn home exists AND it is WITHIN `RETURN_LEASH_SQ` of that home,
    // roll a `WANDER_CHANCE_PCT` chance to pick a random point within `WANDER_RADIUS` of HOME and walk
    // toward it (one short leg, like a patrol leg). The ~1/3 chance gives a pause-then-hop loiter, not
    // a constant jog.
    //
    // MUTUALLY EXCLUSIVE with return-to-spawn: that pass moves a creature displaced FARTHER than
    // `RETURN_LEASH_SQ` from home; this one moves only a creature WITHIN it. The two ranges partition
    // the non-engaged, waypoint-less creatures, so a creature that is RETURNING this tick is outside
    // the leash and never also wanders (and a wanderer, always ≤ `WANDER_RADIUS` = 6 yd from home, is
    // far inside the 15-yd leash, so it never trips return next tick). Same disjointness from
    // chase/flee as return-to-spawn: those move only ENGAGED attackers, this one only NON-engaged
    // creatures — one leg per creature per tick. Snapshot the legs first (collect-then-mutate) so we
    // never write the entity table while iterating it — mirrors the patrol/chase/return passes.
    //
    // It runs AFTER return-to-spawn (so a creature outside the leash already returned and isn't also
    // considered here) and BEFORE regen (a wanderer is out of combat, so regen's `in_combat` gate does
    // NOT skip it — but, exactly like return-to-spawn, this pass only moves x,y,z/grid/last_move and
    // emits a leg, never HP/power, so regen re-writing the row from its own snapshot heals HP without
    // reverting the move). Flee still runs LAST and only touches ENGAGED creatures, so it can't fight a
    // wanderer.
    let spawns_wander = ctx.db.game_creature_spawn();
    // (creature_guid, dest_x, dest_y, z_fallback=home_z, duration_ms) — 181: snap + leg-start via
    // `emit_creature_leg` in the drain loop below.
    let mut to_wander: Vec<PendingLeg> = Vec::new();
    // ACTIVE CELLS (work-item 230): iterate the pre-computed active-cell guid set instead of the full
    // entity table.
    for guid in active.iter().copied() {
        visited += 1;
        let Some(c) = entities.guid().find(guid) else {
            continue;
        };
        // Creatures only (no PLAYER bit), alive — same predicate as the return/chase/flee passes.
        if c.is_player() || c.dead {
            continue;
        }
        // Crowd control: a creature whose self-movement is suppressed — FROZEN (stun/root/poly) or FEARED
        // (the fear-flee pass walks it instead) — doesn't wander (a movement pass). Baseline-safe:
        // `false` without a CC aura → an un-CC'd idle creature loiters exactly as before.
        if crate::spell::is_self_movement_suppressed(ctx, c.guid) {
            continue;
        }
        // Engaged creatures belong to chase/flee — never wander them.
        if crate::combat::is_engaged(ctx, c.guid) {
            continue;
        }
        // A creature the return pass already moved this tick must not also wander — one move leg per
        // creature per tick (two legs share `spline_id` and the client rejects the second).
        if moved_this_tick.contains(&c.guid) {
            continue;
        }
        // A creature with patrol waypoints is governed by the patrol pass (same `by_creature` lookup
        // the return pass uses); skip it so wander doesn't fight its route.
        if waypoints.by_creature().filter(&c.guid).next().is_some() {
            continue;
        }
        // Its persistent home; no spawn row → nowhere to wander around.
        let Some(home) = spawns_wander.guid().find(c.guid) else {
            continue;
        };
        // Only cmangos MovementType RANDOM free-wanders. IDLE creatures hold their spawn post (quest
        // givers, vendors, guards), and WAYPOINT creatures are driven by the patrol pass above — so the
        // wander pass must not move either. Un-reimported rows default to IDLE → stationary.
        if home.movement_type != MOVEMENT_RANDOM {
            continue;
        }
        // Only wander a creature that is home-enough (within the return leash). One displaced farther
        // is RETURNING this tick (handled above) and must not also wander — mutually exclusive.
        let (hdx, hdy) = (home.x - c.x, home.y - c.y);
        if hdx * hdx + hdy * hdy > RETURN_LEASH_SQ {
            continue;
        }
        // ETA gate: a wander hop is still in flight — don't roll a NEW point mid-hop (it would redirect
        // the leg). Commit to the current hop until it lands, THEN consider the next.
        if leg_in_flight(now_ms, c.leg_ends_ms) {
            continue;
        }
        // Pause between hops: ~2/3 of ticks the creature stands still (idle loiter, not a constant jog).
        // (wander runs on the ~4s SENSE cadence, so this roll keeps its original hop frequency.)
        if ctx.random::<u32>() % 100 >= WANDER_CHANCE_PCT {
            continue;
        }
        // Random destination within `WANDER_RADIUS` of HOME (two rolls → angle + radius). Anchoring on
        // home (not the current position) keeps the wander circle centred on the post, so the creature
        // never drifts away over many hops — every destination is provably ≤ `WANDER_RADIUS` from home,
        // far inside the leash.
        let (destx, desty) =
            wander_point(home.x, home.y, ctx.random(), ctx.random(), WANDER_RADIUS);
        // Walk toward it up to one tick of WALK (idle stroll, not a run), landing AT the point if within
        // reach (`stop_dist` 0.0 — arrive on it, like the return leg lands on home). Snare-aware (a snared
        // wanderer ambles slower); no snare → WALK exactly.
        let walk = crate::combat::effective_move_speed(ctx, c.guid, constants::speeds::WALK);
        let (nx, ny) =
            crate::nav::nav_step(ctx, c.map_id, (c.x, c.y), (destx, desty), walk * 4.0, 0.0, c.z);
        // Ground-snap the hop (in the writer, work-item 174); off-slice keeps the old behavior — the
        // home z (flat-ish ground around the post, like the return leg carries the spawn z).
        let Some((dx2, dy2, duration_ms)) = leg_toward((c.x, c.y), (nx, ny), walk) else {
            continue; // no-op (already on the point / can't close) — skip the zero-length leg.
        };
        to_wander.push(PendingLeg {
            guid: c.guid,
            dest: (dx2, dy2),
            z_fallback: home.z,
            duration_ms,
        });
    }
    // Wander STROLLS (walk) and DOES arm the ETA gate (hold the hop until it lands, no mid-hop re-roll)
    // — the two idle passes (patrol + wander) are the only `set_leg_ends = true` callers.
    drain_legs(ctx, to_wander, false, true, now_ms);
    visited
}

/// Pass 11 — flee: a meleeing HUMANOID below ~15% HP routs — it runs away from its target while STAYING
/// in combat (a shared combat state; both sides keep their engagement). Runs LAST (after both regen
/// passes): the still-engaged runner is skipped by regen's in-combat gate, so regen never re-writes (and
/// reverts) the fled position this tick.
///
/// The rout is BOUNDED and once per engagement: this pass stamps `MeleeAttack::rout_ends_ms` when a rout
/// starts and runs the leg only while that window is open. When it closes the creature is an ordinary
/// engaged attacker again — chase closes the gap, the swing pass resolves its blows — and it fights to
/// the death at whatever health it has, with no evade and no heal.
///
/// Work-item 230 classification: ALWAYS ACTIVE, no active-cell gate — every candidate must currently be
/// the ATTACKER in a `game_melee_attack` row (combat-engaged; fleeing is a SHARED COMBAT STATE per the
/// doc above, not a disengage). Gating this on active cells would risk freezing a routing creature
/// mid-flight the instant it — or the player chasing it — stepped past the active-cell edge, which is
/// exactly the "mid-leg flee... finishes" outcome the item requires, not "freezes."
///
/// Work-item 233: outer-loops `game_melee_attack` directly (same precedent as `pass_chase`/
/// `pass_cast`) instead of `entities.iter()` + a per-row `.find(&c.guid)` gate. VERIFIED before
/// switching: "fleeing" carries NO separate state on `WorldEntity` (no `is_fleeing`/`fleeing_since`
/// field — `WorldEntity`'s only combat-adjacent fields are `dead`/`target_guid`/`unit_flags`); a
/// creature's rout status comes from exactly two things — "is it the attacker in a melee row" and that
/// row's eligibility + rout window. Since the melee row is that same necessary condition the old per-row
/// `.find` already required, a creature whose melee row was ever deleted mid-rout could NEVER have
/// re-qualified anyway (this pass never deletes it — see the ordering note above), so outer-looping the
/// melee table instead of the entity table cannot miss a genuinely routing creature. It is also where
/// the rout clock lives, so the state is in hand with no extra lookup.
/// Work-item 229: gated per candidate on `scope.covers(c.instance_id)` (attacker's instance = the
/// pair's — same construction argument as pass_cast/pass_chase).
///
/// Issue #383: dropped the `_tick_secs` parameter, same reasoning and same committed-leg architecture
/// as `pass_chase` — the flee leg is one full `FLEE_LEG_YD` dash, re-rolled only when the previous
/// committed leg finishes (the `game_creature_spline` liveness check below), not sized per firing.
/// Returns covered candidates visited.
pub(crate) fn pass_flee(ctx: &ReducerContext, scope: &TickScope) -> usize {
    let mut visited = 0usize;
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    let entities = ctx.db.game_world_entity();
    // Flee pass (vanilla creature AI: a mob in melee that drops to ~15% HP routs for a bounded window).
    // For each ALIVE creature currently attacking (a `game_melee_attack` row keyed by its guid) that is
    // eligible and inside an open rout window, run one committed leg directly away from its target while
    // KEEPING the engagement live (it doesn't disengage — routing is a combat state). Moving x,y relays a
    // position change to clients, same as a patrol leg.
    //
    // ORDERING — this MUST run LAST, after both regen passes. A routing creature is still engaged, so the
    // regen passes' `in_combat` gate skips it — meaning regen never re-writes (and clobbers) the fled
    // position this tick. Were a rout ever to drop the engagement before regen ran, the health-regen pass
    // would re-write the whole row from its own snapshot and REVERT the move, pinning the runner in place
    // (it never leaves aggro range, re-engages, and grinds the target down). Snapshot the work first, then
    // mutate, so we never write the entity / melee tables while iterating them.
    let melee_flee = ctx.db.game_melee_attack();
    // Snapshot-then-mutate (never write tables mid-iteration). EVERY eligible fleeing creature re-stamps
    // combat each tick (kept alive across the long committed leg); only those whose committed leg has
    // FINISHED get a fresh leg emitted.
    let mut to_restamp: Vec<(u64, u64)> = Vec::new(); // (creature_guid, target_guid)
    let mut to_stamp: Vec<(u64, u32)> = Vec::new(); // (attacker_guid, rout window close ms)
    let mut to_flee: Vec<PendingLeg> = Vec::new();
    // Work-item 233: outer-loop the small melee-engaged table instead of every entity — `row.attacker_guid`
    // is by construction "currently a melee attacker", the exact gate the old `.find(&c.guid)` applied
    // after scanning every entity, so this visits the identical candidate set.
    for row in melee_flee.iter() {
        // Issue #383: the shared gate ladder (creature, alive, this firing's covered instance).
        let Some(c) = movable_creature(ctx, row.attacker_guid, scope) else {
            continue;
        };
        visited += 1;
        // Crowd control: a creature whose self-movement is suppressed — FROZEN (stun/root/poly) or FEARED
        // — does NOT run this low-HP flee leg. A frozen one stays engaged (we don't disengage it here) and
        // locked in place; a rooted low-HP creature keeps swinging but can't run. A FEARED low-HP creature
        // is moved by the fear-flee pass instead (which keeps it engaged, not disengaged), so excluding it
        // here avoids a double-move. Baseline-safe: `false` without a CC aura → an un-CC'd near-dead
        // creature flees exactly as before.
        if crate::spell::is_self_movement_suppressed(ctx, c.guid) {
            continue;
        }
        // Below the flee threshold AND flee-eligible. Fleeing is SELECTIVE in vanilla, not universal: only
        // HUMANOIDS rout at low HP; BEASTS (wolves/boars) etc. fight to the death. A non-eligible near-dead
        // creature is never moved here and instead keeps chasing/swinging via the earlier passes (a missing
        // template ⇒ not eligible ⇒ stands and fights — safe default).
        let eligible = rout_eligible(ctx, &c);
        // START the rout, or ride the one already running. This pass is the ONLY writer of the window, so
        // "eligible and unstamped" is the single moment a rout begins; once the window closes the creature
        // is a normal engaged attacker again (chase closes the gap, the swing pass resolves its blows) and
        // no later health drop can start a second one.
        let starting = may_start_rout(eligible, row.rout_ends_ms);
        if !starting && !(eligible && rout_window_open(now_ms, row.rout_ends_ms)) {
            continue; // not eligible, or the window is spent → it stands and fights
        }
        // Run directly away from whoever it's fighting (the melee row's target). Resolved BEFORE the
        // window is stamped, so a vanished target can never spend the rout without a leg being run.
        let Some(target) = entities.guid().find(row.target_guid) else {
            continue;
        };
        if starting {
            to_stamp.push((row.attacker_guid, rout_close_ms(now_ms)));
        }
        // SHARED COMBAT STATE: re-stamp EVERY tick (even mid-leg) so combat doesn't drop during the long
        // committed run. (Was in the emit loop, which now fires only on re-roll.)
        to_restamp.push((c.guid, row.target_guid));
        // COMMIT one long leg: re-roll ONLY when the previous leg has finished (no active spline).
        // Re-picking an away-direction every tick snap-rotated the client's facing (the "flee spin");
        // committing to one ~28yd leg (the vanilla flee-leg band) gives one stable travel tangent.
        if ctx.db.game_creature_spline().guid().find(c.guid).is_some() {
            continue;
        }
        // A single FLEE_LEG_YD leg directly away from the threat, nav-routed, at the wounded-slow run
        // speed; duration = the FULL travel time for that leg (not one
        // tick), so the client interpolates the whole committed run.
        let run = crate::combat::effective_move_speed(ctx, c.guid, constants::speeds::RUN)
            * wounded_slow_factor(c.health, c.max_health);
        let (fx, fy) = flee_step(c.x, c.y, target.x, target.y, FLEE_LEG_YD);
        let (nx, ny) =
            crate::nav::nav_step(ctx, c.map_id, (c.x, c.y), (fx, fy), FLEE_LEG_YD, 0.0, c.z);
        let Some((dx2, dy2, duration_ms)) = leg_toward((c.x, c.y), (nx, ny), run) else {
            continue;
        };
        to_flee.push(PendingLeg {
            guid: c.guid,
            dest: (dx2, dy2),
            z_fallback: c.z,
            duration_ms,
        });
    }
    // Combat re-stamp EVERY tick for every routing creature (not just re-roll ticks) — routing is a
    // SHARED COMBAT STATE; without this the 6s combat-drop fires mid committed-leg and both sides untarget
    // ("they get away or untarget each other and combat just ends"). The rout ends when its window closes,
    // or sooner if the player catches the wounded-slow mob and kills it.
    for (guid, target_guid) in to_restamp {
        crate::combat::enter_combat(ctx, guid);
        crate::combat::enter_combat(ctx, target_guid);
    }
    // Stamp the newly started rout windows (re-find the LIVE row — the table was only read above).
    for (attacker_guid, ends_ms) in to_stamp {
        if let Some(mut row) = melee_flee.attacker_guid().find(attacker_guid) {
            row.rout_ends_ms = ends_ms;
            melee_flee.attacker_guid().update(row);
        }
    }
    // Emit the committed RUN legs (only the finished-leg creatures reach here). The client interpolates
    // the whole ~28yd run; pass_advance_splines advances the authoritative position along it.
    drain_legs(ctx, to_flee, true, false, now_ms);
    visited
}

/// Pass 12 — fear-flee (crowd control): a FEARED creature is force-walked AWAY from its fear source
/// each tick with jitter. Runs LAST alongside flee for the same regen-ordering reason; unlike flee it
/// does NOT disengage (fear is temporary). The normal movement passes all skip a feared creature
/// (`is_self_movement_suppressed`), so this is its SOLE mover.
///
/// Work-item 230 classification: ALWAYS ACTIVE, no active-cell gate — a feared creature carries a live
/// CC aura (someone cast fear on it, which requires combat/targeting contact), so it's combat-adjacent
/// by construction and the population is inherently small and rare regardless of world size. Gating it
/// would risk freezing a panicked creature mid-route the instant it crossed the active-cell edge.
///
/// Work-item 233: outer-loops the `game_aura` table (filtered to `A_CONTROL(M_FEAR)` rows) instead of
/// `entities.iter()` + a per-entity `fear_source` probe. `game_aura` has no by-KIND index (only
/// `by_target`/`by_expiry`), so a target-side lookup can't narrow this the way the melee table narrows
/// cast/flee — but the aura table is itself small relative to the full entity population (fear uptime
/// is rare), so scanning IT with an `eff_kind`/`eff_p0` filter and DEDUPING the target guids is the
/// cheap, correct narrowing. `fear_source` is still called per candidate, UNCHANGED, to resolve the
/// caster — this only changes how the candidate SET is discovered (aura-table scan + dedup, vs.
/// entity-table scan + per-row probe), not which caster wins if a target somehow carries more than one
/// live fear aura (that tie-break is `fear_source`'s own `.find()` order, untouched by this pass).
/// Work-item 229: gated per candidate on `scope.covers(c.instance_id)` (the FEARED creature's own
/// instance — its sole mover must fire exactly once per its covering row). The dash distance scales
/// with `tick_secs` (`fear_step_for_tick` — verbatim `FEAR_STEP` on the default row) so a fast
/// dedicated row doesn't multiply terror speed. Returns covered candidates visited.
pub(crate) fn pass_fear_flee(ctx: &ReducerContext, scope: &TickScope, tick_secs: f32) -> usize {
    let mut visited = 0usize;
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    let entities = ctx.db.game_world_entity();
    // Fear-flee pass (crowd control: a FEARED creature "flees in terror" — it cannot act, and instead of
    // steering its own movement it is force-walked AWAY from whoever feared it each tick). For each ALIVE
    // creature carrying an `A_CONTROL(M_FEAR)` aura (detected via `fear_source`, which also yields the
    // caster to flee FROM) that is NOT otherwise FROZEN (`is_movement_blocked`: a stun/root/poly aura
    // outranks fear and pins it in place), step it `FEAR_STEP` yards away from the fear source with
    // angular jitter (zig-zag) and emit a RUN leg. Unlike the low-HP flee it does NOT disengage — fear is
    // temporary; when the aura expires the creature resumes attacking (its melee row is untouched), and
    // the action gates keep it from swinging/casting meanwhile.
    //
    // SOURCE position: the fear caster's (if still in world); failing that the creature's current melee
    // target (it still flees *something*); failing both its OWN position, which `feared_flee_step` turns
    // into a jittered +x bearing — so a feared creature always moves.
    //
    // ORDERING — like the low-HP flee it runs LAST, after both regen passes. A feared+engaged creature is
    // still `in_combat` (melee row live) so regen skips it; a feared+non-engaged one is moved here AFTER
    // regen ran, so regen never reverts the leg (this pass writes only x/y/z/grid/last_move, never
    // HP/power). The normal movement passes ALL skip feared creatures (`is_self_movement_suppressed`), so
    // this is the SOLE mover for a feared creature — one leg, one `spline_id`, per tick. Snapshot first
    // (collect-then-mutate), like every other movement pass.
    let melee_fear = ctx.db.game_melee_attack();
    let auras_fear = ctx.db.game_aura();
    let mut to_fear_flee: Vec<PendingLeg> = Vec::new();
    // Work-item 233: the small candidate SET — every distinct `target_guid` carrying a live
    // `A_CONTROL(M_FEAR)` aura — discovered by scanning the (small) aura table instead of every entity.
    // Sorted + deduped so a target with more than one live fear aura (e.g. two different casters) is
    // still visited exactly once, matching the old loop's one-decision-per-entity shape.
    let mut feared_guids: Vec<u64> = auras_fear
        .iter()
        .filter(|a| a.eff_kind == crate::spell::A_CONTROL && a.eff_p0 == crate::spell::M_FEAR)
        .map(|a| a.target_guid)
        .collect();
    feared_guids.sort_unstable();
    feared_guids.dedup();
    for guid in feared_guids {
        // Issue #383: the shared gate ladder (creature, alive, this firing's covered instance).
        let Some(c) = movable_creature(ctx, guid, scope) else {
            continue;
        };
        visited += 1;
        // Must be FEARED; `fear_source` is `Some(caster)` exactly when an A_CONTROL(M_FEAR) aura is up —
        // unchanged call, re-resolving the caster (not read off the aura row above) so the "which caster"
        // tie-break is untouched by this pass.
        let Some(src_guid) = crate::spell::fear_source(ctx, c.guid) else {
            continue;
        };
        // A stun/root/poly aura OUTRANKS fear and freezes the creature — it stays put, doesn't flee.
        if crate::spell::is_movement_blocked(ctx, c.guid) {
            continue;
        }
        // Source position: the fear caster if in world, else the current melee target, else self (→ a
        // jittered +x fallback bearing in `feared_flee_step`, so a feared creature always moves).
        let src = entities.guid().find(src_guid).or_else(|| {
            melee_fear
                .attacker_guid()
                .find(c.guid)
                .and_then(|r| entities.guid().find(r.target_guid))
        });
        let (sx, sy) = src.map(|s| (s.x, s.y)).unwrap_or((c.x, c.y));
        let (nx, ny) = feared_flee_step(
            c.x,
            c.y,
            sx,
            sy,
            fear_step_for_tick(tick_secs),
            ctx.random(),
        );
        // The writer ground-snaps the panic hop (work-item 174); off-slice stays the flat z-unchanged
        // leg (c.z fallback).
        let Some((dx2, dy2, duration_ms)) =
            leg_toward((c.x, c.y), (nx, ny), constants::speeds::RUN)
        else {
            continue; // degenerate zero-length leg (the client rejects it) — skip.
        };
        to_fear_flee.push(PendingLeg {
            guid: c.guid,
            dest: (dx2, dy2),
            z_fallback: c.z,
            duration_ms,
        });
    }
    // Feared creatures RUN in terror; no ETA gate (re-stepped each tick until the aura expires).
    drain_legs(ctx, to_fear_flee, true, false, now_ms);
    visited
}

#[cfg(test)]
mod facing_tests {
    use super::*;
    use std::f32::consts::{FRAC_PI_2, PI, TAU};

    // #518: pure-function coverage for the stand-and-swing facing correction. `pass_chase` itself
    // needs a live `ReducerContext` (a real melee-attack row + entity table), which is exercised by
    // the wire suite; `angle_diff` and the epsilon threshold are the whole decision and are cheap to
    // pin here directly.

    #[test]
    fn zero_drift_is_zero() {
        assert_eq!(angle_diff(1.0, 1.0), 0.0);
    }

    #[test]
    fn a_quarter_turn_reads_as_a_quarter_turn_either_direction() {
        assert!((angle_diff(0.0, FRAC_PI_2) - FRAC_PI_2).abs() < 1e-6);
        assert!((angle_diff(0.0, -FRAC_PI_2) - (-FRAC_PI_2)).abs() < 1e-6);
    }

    #[test]
    fn the_pi_seam_takes_the_short_way_round() {
        // orientation just past +PI, target bearing just past -PI: only ~0.2 rad apart going
        // "outward" across the seam, NOT the ~2*PI-0.2 rad the naive subtraction would give.
        let from = PI - 0.1;
        let to = -PI + 0.1;
        let d = angle_diff(from, to);
        assert!(d.abs() < 0.3, "expected a short turn across the seam, got {d}");
    }

    #[test]
    fn a_full_turn_collapses_to_no_turn() {
        assert!(angle_diff(0.5, 0.5 + TAU).abs() < 1e-4);
    }

    #[test]
    fn the_epsilon_gate_is_silent_below_threshold_and_fires_above_it() {
        let orientation = 0.0_f32;
        let just_under = FACING_EPSILON_RAD - 0.01;
        let just_over = FACING_EPSILON_RAD + 0.01;
        assert!(angle_diff(orientation, just_under).abs() <= FACING_EPSILON_RAD);
        assert!(angle_diff(orientation, just_over).abs() > FACING_EPSILON_RAD);
    }
}
