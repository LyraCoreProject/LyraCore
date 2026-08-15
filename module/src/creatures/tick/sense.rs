//! The sensing/AI-decision passes (issue #383 split of tick.rs): aggro + assist (typed `AggroEvent`,
//! split into `pass_aggro` + `pass_assist` behind the one `pass_aggro_assist` pipeline call site),
//! cast, threat-retarget, regen. See `tick/mod.rs`'s module doc for the pipeline's load-bearing pass
//! ORDER; this split does not change it (every pass here keeps its original name and signature,
//! called by bare name via `tick/mod.rs`'s `use sense::*;`).

use spacetimedb::{ReducerContext, Table};

use crate::{game_faction_template, game_melee_attack, game_world_entity, MeleeAttack, WorldEntity};

use super::*;

/// One creature's proximity aggro this tick — issue #383's typed replacement for the anonymous
/// `(guid, x, y, map, faction, instance, target)` 7-tuple `pass_aggro`/`pass_assist` used to thread
/// with positional wildcard patterns (`|(ag_guid, ag_x, ag_y, ag_map, ag_ft, ag_instance, _)| ...`).
/// The aggroer's position/faction/instance are what the assist scan measures range and same-kind
/// from — see `pass_assist`.
struct AggroEvent {
    guid: u64,
    x: f32,
    y: f32,
    map_id: u32,
    faction_template: u32,
    instance_id: u64,
    target_player: u64,
}

/// The per-creature CANDIDATE-SELECTION heart of `pass_aggro`, extracted so the outer loop reads as
/// its gate ladder + this one call: the nearest ALIVE, HOSTILE, in-range, visible player `c` (with
/// its resolved template `tmpl`) should aggro onto this tick, out of the tick's `players` snapshot
/// (each paired with its stealth flag). Vanilla proximity aggro: the radius SCALES with the level
/// difference (`aggro_radius`), shrinks under Mind Soothe (`detect_range_mod`), grades stealth
/// detection down to `stealth_detect_range`, and requires both hostility (`compute_hostile`) and line
/// of sight (`has_los`, work-item 243). `None` = nothing to aggro onto this tick.
fn best_aggro_target(
    ctx: &ReducerContext,
    c: &WorldEntity,
    tmpl: &CreatureTemplate,
    players: &[(WorldEntity, bool)],
) -> Option<u64> {
    let factions = ctx.db.game_faction_template();
    // Missing faction row on either side ⇒ NOT hostile (safe — never aggro on missing data).
    let c_ft = factions.id().find(c.faction_template);
    // Mind Soothe (A_MOD_DETECT_RANGE): a signed additive modifier (yards) on a soothed creature's
    // aggro/detection radius (negative — it SHRINKS the radius). 0 for an un-soothed creature →
    // radius unchanged (baseline-safe); clamped ≥ 0 so a large soothe can't go negative.
    let detect_mod = crate::spell::detect_range_mod(ctx, c.guid);
    players
        .iter()
        .filter(|(p, _)| p.map_id == c.map_id && p.instance_id == c.instance_id)
        .filter_map(|(p, stealthed)| {
            let radius = (aggro_radius(c.level, p.level, tmpl.aggro_range) + detect_mod).max(0.0);
            if radius <= 0.0 {
                return None; // grey / no proximity aggro for this (creature, player) pair (or fully soothed)
            }
            let (dx, dy, dz) = (p.x - c.x, p.y - c.y, p.z - c.z);
            let d2 = dx * dx + dy * dy + dz * dz;
            if d2 > radius * radius {
                return None;
            }
            // Stealth: a stealthed candidate is detected only when CLOSE enough for THIS creature's
            // level (graded — `stealth_detect_range`). Outside its detect range a stealthed target
            // stays invisible (skip). A non-stealthed player skips this block (byte-identical path).
            if *stealthed {
                let detect = stealth_detect_range(c.level, p.level);
                if d2 > detect * detect {
                    return None;
                }
            }
            let hostile = match (&c_ft, factions.id().find(p.faction_template)) {
                (Some(a), Some(b)) => crate::faction::compute_hostile(a, &b),
                _ => false,
            };
            if !hostile {
                return None;
            }
            // 243: proximity aggro requires line of sight — a hostile behind the abbey wall is not
            // "seen". `has_los` is `true` whenever nav is off (byte-identical pre-243 behavior) and
            // only raymarches pairs already inside aggro radius.
            if !crate::nav::has_los(ctx, c.map_id, (c.x, c.y, c.z), (p.x, p.y, p.z)) {
                return None;
            }
            Some((p.guid, d2))
        })
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(guid, _)| guid)
}

/// Pass 4a — aggro (vanilla creature AI: mobs aggro you on sight). For each ALIVE creature in `active`
/// whose template has `aggro_range > 0` and that is NOT already attacking (no `game_melee_attack` row
/// as attacker — so an engaged creature is never re-armed each tick; idempotent), arm a
/// creature→player melee row + point the creature's `target_guid` at whatever `best_aggro_target`
/// picks. `tick_melee`'s swing pass then makes the creature swing (range/timer gated there) — same
/// arming shape as retaliation and `start_attack`. Collect-then-mutate so we never insert into
/// `game_melee_attack` while iterating any table.
///
/// Issue #383: split out of the combined aggro+assist pass so each half reads as its own gate ladder
/// plus one candidate-selection call; still called from `pass_aggro_assist` at the ONE original
/// pipeline call site (byte-identical pass ordering/stats). Returns `(this tick's aggro events,
/// candidates visited)` — the events feed `pass_assist`.
///
/// Work-item 230: SCOPED to `active` (creature guids reachable from some player's `by_grid`
/// neighborhood) instead of the full `game_world_entity` table. Work-item 229: instance scope is
/// inherited from `active` (covered players seed it, and the per-pair `instance_id` equality in
/// `best_aggro_target` keeps pairing instance-local) — no separate gate needed.
fn pass_aggro(
    ctx: &ReducerContext,
    active: &std::collections::HashSet<u64>,
) -> (Vec<AggroEvent>, usize) {
    let mut visited = 0usize;
    let mut aggro_events: Vec<AggroEvent> = Vec::new();
    let entities = ctx.db.game_world_entity();
    let templates = ctx.db.game_creature_template();
    let melee = ctx.db.game_melee_attack();
    // Snapshot ALIVE players (same shape as the leash/loot range checks elsewhere), each paired with its
    // stealth flag — computed ONCE per player here (not re-scanned N times per creature).
    // `is_aggro_candidate` (not a hand-inlined `!e.dead`): it also excludes a GODMODED GM, who would
    // otherwise be an immortal aggro magnet — nothing ever kills them, so nothing ever disengages, and
    // creatures accumulate on them without bound. See the predicate's doc.
    let players: Vec<(WorldEntity, bool)> = entities
        .iter()
        .filter(|e| e.is_player() && crate::creatures::ai::is_aggro_candidate(e.dead, e.godmode))
        .map(|e| {
            let stealthed = crate::spell::is_stealthed(ctx, e.guid);
            (e, stealthed)
        })
        .collect();
    if players.is_empty() {
        return (aggro_events, visited);
    }
    let mut to_arm: Vec<(u64, u64)> = Vec::new(); // (creature, player)
                                                   // ACTIVE CELLS (work-item 230): iterate the pre-computed active-cell guid set — reachable from
                                                   // SOME player's neighborhood via `by_grid` — instead of the full entity table.
    for guid in active.iter().copied() {
        visited += 1;
        let Some(c) = entities.guid().find(guid) else {
            continue;
        };
        // Creatures only (no PLAYER bit — `active` never contains one anyway, belt-and-suspenders),
        // alive, not already attacking someone. A PET (owner_guid != 0) is skipped: it must NOT
        // proximity-aggro on the player's behalf — `pass_pet` arms its target off the OWNER's combat,
        // not on sight. Baseline-safe (every wild creature has owner_guid == 0).
        if c.is_player()
            || c.dead
            || c.owner_guid != 0
            || melee.attacker_guid().find(c.guid).is_some()
        {
            continue;
        }
        // Don't re-arm a near-dead creature that would rout the moment it engaged: it would be pulled
        // straight back into a fight it is trying to leave. Eligibility, not `creature_is_routing` — a
        // creature reaching here has no engagement row, so it cannot be routing yet.
        if rout_eligible(ctx, &c) {
            continue;
        }
        // Crowd control: an ACTION-blocked creature (stun/poly/fear) cannot ACT — it doesn't aggro on
        // sight (root is NOT gated here — a rooted creature can still aggro, it just can't close).
        if crate::spell::is_action_blocked(ctx, c.guid) {
            continue;
        }
        let Some(tmpl) = templates.entry().find(c.entry) else {
            continue;
        };
        if let Some(player_guid) = best_aggro_target(ctx, &c, &tmpl, &players) {
            to_arm.push((c.guid, player_guid));
            // Record this aggro for the assist pass. Map/faction/instance ride along because the
            // neighbor loop scans the WHOLE active set: without them, same-faction creatures on
            // DIFFERENT maps whose local coordinates coincidentally overlap within `ASSIST_RADIUS`
            // would cross-assist (190 slice 1 review finding — a latent pre-existing gap).
            aggro_events.push(AggroEvent {
                guid: c.guid,
                x: c.x,
                y: c.y,
                map_id: c.map_id,
                faction_template: c.faction_template,
                instance_id: c.instance_id,
                target_player: player_guid,
            });
        }
    }
    for (creature, player) in to_arm {
        // Arm the engagement — same shape as retaliation/`start_attack`. `find`-then-insert is a no-op
        // guard if a row appeared concurrently (it can't here, but keeps the pass idempotent).
        if melee.attacker_guid().find(creature).is_none() {
            melee.insert(MeleeAttack {
                attacker_guid: creature,
                target_guid: player,
                last_swing_ms: 0,   // swing on the next melee tick
                ranged_spell_id: 0, // creature melee aggro
                last_offhand_swing_ms: 0,
                rout_ends_ms: 0,
                pursuit_ends_ms: 0,
                leash_x: 0.0,
                leash_y: 0.0,
            });
        }
        // Point the creature at its target (the established target_guid pattern), so observers see
        // it facing/targeting the player; `tick_melee` owns the actual swinging.
        if let Some(mut c) = entities.guid().find(creature) {
            if c.target_guid != player {
                c.target_guid = player;
                entities.guid().update(c);
            }
        }
        // Notify-hook: direct proximity aggro — "the world noticed you".
        crate::hooks::fire_on_aggro(
            ctx,
            &crate::hooks::AggroPayload {
                creature_guid: creature,
                target_guid: player,
                assist: false,
            },
        );
    }
    (aggro_events, visited)
}

/// Pass 4b — assist (vanilla social aggro / pack behavior): for each creature that aggroed THIS tick
/// (`aggro_events`, from `pass_aggro`), nearby SAME-FACTION neighbors in `active` pile onto the same
/// player — even passive ones the player never got close enough to aggro directly. Range is measured
/// from the AGGROER (the one calling for help), not from the player, so a far-flung pack-mate isn't
/// pulled in. `aggro_events.is_empty()` (no aggro this tick — the common case) short-circuits before
/// touching `active` at all, byte-identical to the pre-#383 combined pass's own `if !aggro_events.is_
/// empty()` guard.
///
/// Additive + baseline-safe: a LONE aggroer has no same-faction neighbor in `ASSIST_RADIUS`, so
/// `to_assist` stays empty (the seeded login Chicken is alone → calm login unchanged). The assist
/// deliberately does NOT gate on the neighbor's `aggro_range` — a passive neighbor still answers a
/// pack-mate's call.
///
/// Work-item 230: same `active` set `pass_aggro` used, not a re-narrowed one — `active_cell_radius`
/// already folds `ASSIST_RADIUS` on top of the aggro ceiling specifically so a real assist neighbor is
/// always inside it. Returns candidates visited.
fn pass_assist(
    ctx: &ReducerContext,
    active: &std::collections::HashSet<u64>,
    mut aggro_events: Vec<AggroEvent>,
) -> usize {
    let mut visited = 0usize;
    if aggro_events.is_empty() {
        return visited;
    }
    let entities = ctx.db.game_world_entity();
    let melee = ctx.db.game_melee_attack();
    // Work-item 233 (cosmetic, free): sort by aggroer guid BEFORE the scan below picks one via
    // `.iter().find(..)` — `active` is a `HashSet`, so push order (and therefore which same-faction
    // aggroer a neighbor in range of TWO of them answers) was hash-iteration-order-dependent.
    // Sorting makes the tie-break the LOWEST aggroer guid: stable, cheap (small list), and changes
    // nothing when there's no tie (the overwhelmingly common case).
    aggro_events.sort_unstable_by_key(|e| e.guid);
    let mut to_assist: Vec<(u64, u64)> = Vec::new(); // (neighbor, target player)
    let mut already_assigned: std::collections::HashSet<u64> = std::collections::HashSet::new();
    for guid in active.iter().copied() {
        visited += 1;
        let Some(n) = entities.guid().find(guid) else {
            continue;
        };
        // Same creature predicate as the aggro pass: creatures only (no PLAYER bit), alive, not
        // already an attacker, not near-dead. A PET never answers a wild-creature assist call.
        if n.is_player()
            || n.dead
            || n.owner_guid != 0
            || melee.attacker_guid().find(n.guid).is_some()
            || rout_eligible(ctx, &n)
        {
            continue;
        }
        // Crowd control: an ACTION-blocked neighbor (stun/poly/fear) cannot ACT — it doesn't answer a
        // pack-mate's call (root still assists, it just can't close; fear routs).
        if crate::spell::is_action_blocked(ctx, n.guid) {
            continue;
        }
        // Find an aggroer this neighbor should assist: a DIFFERENT creature, same map and faction
        // (a simple, safe "same kind" — real assist uses faction friendship, a future refinement),
        // same instance (work-item 190 slice 1), within `ASSIST_RADIUS`, and visible (work-item 243 —
        // no assist calls through the abbey wall).
        let assist = aggro_events.iter().find(|ev| {
            ev.guid != n.guid
                && ev.map_id == n.map_id
                && ev.faction_template == n.faction_template
                && ev.instance_id == n.instance_id
                && within_assist_radius(ev.x, ev.y, n.x, n.y)
                && crate::nav::has_los(ctx, n.map_id, (n.x, n.y, n.z), (ev.x, ev.y, n.z))
        });
        if let Some(ev) = assist {
            // Guard a neighbor in range of two aggroers from being queued twice this tick.
            if already_assigned.insert(n.guid) {
                to_assist.push((n.guid, ev.target_player));
            }
        }
    }
    for (neighbor, player) in to_assist {
        // Re-check no melee row appeared (collect-then-mutate means the table was stable while we
        // scanned; this keeps the insert idempotent, matching the direct-aggro arming).
        if melee.attacker_guid().find(neighbor).is_none() {
            melee.insert(MeleeAttack {
                attacker_guid: neighbor,
                target_guid: player,
                last_swing_ms: 0,   // swing on the next melee tick
                ranged_spell_id: 0, // assist aggro is melee
                last_offhand_swing_ms: 0,
                rout_ends_ms: 0,
                pursuit_ends_ms: 0,
                leash_x: 0.0,
                leash_y: 0.0,
            });
        }
        if let Some(mut c) = entities.guid().find(neighbor) {
            if c.target_guid != player {
                c.target_guid = player;
                entities.guid().update(c);
            }
        }
        // Notify-hook: pack-assist aggro (a neighbor answering the call).
        crate::hooks::fire_on_aggro(
            ctx,
            &crate::hooks::AggroPayload {
                creature_guid: neighbor,
                target_guid: player,
                assist: true,
            },
        );
    }
    visited
}

/// Pass 4 — aggro + assist (KEPT AT ONE CALL SITE — see the pipeline doc — because they share
/// `aggro_events`): a hostile creature whose template `aggro_range` covers a nearby player
/// self-engages (`pass_aggro`), and same-faction neighbors within `ASSIST_RADIUS` of an aggroer pile
/// onto the same player (`pass_assist`). Issue #383 split the two into their own functions (each
/// under half the size, single responsibility); this thin wrapper preserves the ONE pipeline call
/// site and the ONE combined `rows-visited` stat the log line reports.
pub(crate) fn pass_aggro_assist(ctx: &ReducerContext, active: &std::collections::HashSet<u64>) -> usize {
    let (aggro_events, visited_aggro) = pass_aggro(ctx, active);
    let visited_assist = pass_assist(ctx, active, aggro_events);
    visited_aggro + visited_assist
}

// The per-caster work list the pass builds: `(caster, level, ordered candidates)`, documented by the comment above it.
#[allow(clippy::type_complexity)]
/// Pass 5 — cast: an engaged caster creature casts from its spell ROTATION (rank 20) — or its single
/// `game_creature_cast` spell when it has no rotation rows. Runs after aggro/assist, before chase (a
/// caster casts rather than only closing); `resolve_cast_at` gates GCD/cost/range/cooldown internally.
/// Never moves/disengages anything.
///
/// Work-item 230 classification: ALWAYS ACTIVE, no active-cell gate — every candidate here must
/// currently be the ATTACKER in a `game_melee_attack` row (combat-engaged), and the item requires an
/// engaged creature to never sleep regardless of distance ("a player could drag one far away").
///
/// Work-item 233: outer-loops `game_melee_attack` directly (the pass_chase/pass_threat_retarget
/// precedent) instead of `entities.iter()` + a per-row `melee.attacker_guid().find(..)` gate. Every
/// candidate here was ALREADY required to be a melee attacker (the `let Some(row) = ... else continue`
/// this replaces) — nothing that used to reach the cast logic is excluded, and nothing new is admitted;
/// only how the candidate set is DISCOVERED changed (small table outer loop vs full entity scan +
/// inline filter), which visits the identical set of creatures.
///
/// Work-item 229: each candidate is additionally gated on `scope.covers(c.instance_id)` — the caster
/// and its melee target share an instance by construction (arming is same-instance-gated everywhere),
/// so the ATTACKER's instance is the pair's. With only the catch-all row this admits every candidate
/// (equivalence: ai.rs `tick_scope_default_config_…`). Returns covered candidates visited.
pub(crate) fn pass_cast(ctx: &ReducerContext, scope: &TickScope) -> usize {
    let mut visited = 0usize;
    let entities = ctx.db.game_world_entity();

    // Cast pass (caster-type creature AI). For each ALIVE creature (no PLAYER bit, not dead) that is the
    // ATTACKER in a `game_melee_attack` row, choose an action:
    //   - ROTATION (rank 20): if its entry has `game_creature_spell` rows, evaluate them highest-priority
    //     first and collect the ones whose CONDITION holds (heal-when-low / buff-if-missing / debuff-if-
    //     missing / always-nuke), each with its derived cast target (self for heal/buff, the melee target
    //     for nuke/debuff). At cast time the survivors are attempted in priority order until one is ready.
    //   - LEGACY: no rotation rows → fall back to the single `game_creature_cast` spell at the target
    //     (existing single-spell casters are byte-identical — baseline-safe).
    //   - NEITHER → never casts (baseline-safe).
    // The cadence is each spell's OWN GCD/cooldown — `resolve_cast_at` enforces it and returns `Err` when
    // not ready, which we treat as "try the next candidate" (so a rotation whose top action is on cooldown
    // still fires a ready lower-priority one this tick), casting AT MOST ONE action per creature per tick.
    //
    // It runs AFTER aggro/assist (a creature aggroed/assisting THIS tick can already cast) and BEFORE
    // chase (a caster that CAN cast should cast, not merely close — casting is ranged). Casting never
    // moves the creature, touches its melee row, or disengages it.
    //
    // `resolve_cast_at` writes only `game_aura`/`game_world_entity`/`game_spell_cooldown` — NOT the
    // creature rows we iterate — but to stay safe against mutating `game_world_entity` while iterating it
    // we SNAPSHOT the per-creature candidate lists first (collect-then-call), then loop and cast. The
    // condition reads (HP, `game_aura`) happen in the snapshot phase (reads only).
    let casts = ctx.db.game_creature_cast();
    let rotations = ctx.db.game_creature_spell();
    let melee_cast = ctx.db.game_melee_attack();
    // (caster_guid, caster_level, ordered candidate (spell_id, cast_target) list)
    let mut to_cast: Vec<(u64, u8, Vec<(u32, u64)>)> = Vec::new();
    // Work-item 233: outer-loop the small melee-engaged table (PK `attacker_guid`, one row per
    // attacker) instead of every entity. `row.attacker_guid` is by construction "currently a melee
    // attacker" — exactly the gate the old `.find(&c.guid)` applied AFTER scanning every entity — so
    // this visits the identical candidate set via the smaller table.
    for row in melee_cast.iter() {
        // Issue #383: the shared gate ladder (creature, alive, this firing's covered instance) — same
        // check the aggro/chase/flee passes each hand-wrote. A player's own melee row (they auto-attack
        // too) is skipped here exactly as it was skipped by the old `entities.iter()` loop's `is_player`
        // check.
        let Some(c) = movable_creature(ctx, row.attacker_guid, scope) else {
            continue;
        };
        visited += 1;
        // Crowd control: an ACTION-blocked creature (stun/poly/fear) cannot ACT — it doesn't cast (a
        // ROOTED caster CAN still cast — ranged, no movement; a FEARED one routs and can't). Baseline-safe:
        // `false` without a stun/poly/fear aura → unchanged casting.
        if crate::spell::is_action_blocked(ctx, c.guid) {
            continue;
        }
        // 171: MID-CAST — a creature with a live `game_pending_cast` row is busy casting; skip it.
        // Load-bearing: `begin_cast` writes no cooldown until COMPLETION, so without this guard the
        // 500ms tick would re-enter begin_cast, whose stale-row sweep deletes + restarts the pending
        // cast every tick — the cast would never finish.
        {
            use crate::spell::game_pending_cast;
            if ctx
                .db
                .game_pending_cast()
                .iter()
                .any(|p| p.caster_guid == c.guid)
            {
                continue;
            }
        }
        let target_guid = row.target_guid;
        let mut rot: Vec<CreatureSpell> = rotations.by_entry().filter(&c.entry).collect();
        let candidates: Vec<(u32, u64)> = if rot.is_empty() {
            // Legacy single-spell fallback (no rotation rows).
            match casts.creature_entry().find(c.entry) {
                Some(cast) => vec![(cast.spell_id, target_guid)],
                None => continue,
            }
        } else {
            // Rotation: highest priority first (ties by id → deterministic), keep the eligible ones.
            rot.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.id.cmp(&b.id)));
            rot.iter()
                .filter_map(|r| creature_cast_eligibility(ctx, &c, r, target_guid))
                .collect()
        };
        // 243: a hostile cast needs line of sight to its victim — drop enemy-targeted
        // candidates when the melee target is LoS-blocked (self heals/buffs stay; the creature
        // melees/chases instead, and the chase leg paths around the geometry). `has_los` is
        // `true` whenever nav is off — byte-identical pre-243 casting.
        let candidates: Vec<(u32, u64)> = if candidates.iter().any(|&(_, t)| t != c.guid) {
            let los = entities
                .guid()
                .find(target_guid)
                .map(|t| crate::nav::has_los(ctx, c.map_id, (c.x, c.y, c.z), (t.x, t.y, t.z)))
                .unwrap_or(true);
            if los {
                candidates
            } else {
                candidates
                    .into_iter()
                    .filter(|&(_, t)| t == c.guid)
                    .collect()
            }
        } else {
            candidates
        };
        if !candidates.is_empty() {
            to_cast.push((c.guid, c.level as u8, candidates));
        }
    }
    for (caster_guid, level, candidates) in to_cast {
        // Attempt the eligible actions in priority order; cast AT MOST ONE per tick. `resolve_cast_at`
        // gates GCD/cost/range/cooldown (Err = not ready) — so the first READY action fires and the rest
        // are skipped; if none are ready the creature just melees this tick.
        for (spell_id, cast_target) in candidates {
            // 171: route through `begin_cast` so a TIMED creature spell gets a real `game_pending_cast`
            // row + a START event carrying `cast_time_ms` — observers see the mob's cast bar, and the
            // player-side interrupt machinery (Kick/Counterspell/pushback, all caster-guid-agnostic)
            // gets something to hit. Instant spells self-route to `resolve_cast_at` inside — byte-
            // identical to the old direct call.
            if crate::spell::begin_cast(ctx, caster_guid, spell_id, level, cast_target, false, None)
                .is_ok()
            {
                break;
            }
        }
    }
    visited
}

/// Evaluate one rotation row's CONDITION against the live state (rank 20): `Some((spell_id, cast_target))`
/// if the row should fire, else `None`. The cast target is derived from the condition — self for a heal
/// (SELF_HP_BELOW_PCT) / buff (SELF_MISSING_AURA), the melee `target_guid` for a nuke (ALWAYS) / debuff
/// (TARGET_MISSING_AURA). Reads only HP + `game_aura` (no writes). An unknown condition never fires
/// (forward-compatible). [server]
fn creature_cast_eligibility(
    ctx: &ReducerContext,
    creature: &WorldEntity,
    row: &CreatureSpell,
    target_guid: u64,
) -> Option<(u32, u64)> {
    let cast_target = match row.condition {
        cast_condition::ALWAYS => Some(target_guid),
        cast_condition::SELF_HP_BELOW_PCT => hp_pct_below(
            creature.health,
            creature.max_health,
            row.condition_value as u32,
        )
        .then_some(creature.guid),
        cast_condition::TARGET_MISSING_AURA => {
            (!crate::spell::has_aura(ctx, target_guid, row.spell_id)).then_some(target_guid)
        }
        cast_condition::SELF_MISSING_AURA => {
            (!crate::spell::has_aura(ctx, creature.guid, row.spell_id)).then_some(creature.guid)
        }
        _ => None,
    };
    cast_target.map(|t| (row.spell_id, t))
}

/// Pass 6 — threat retarget: an engaged creature re-points at its HIGHEST-THREAT source (strict
/// hysteresis — switches only when a second source out-threats the current target). Neither moves nor
/// acts, so CC does not gate it.
///
/// Work-item 230 classification: ALWAYS ACTIVE, no active-cell gate — already iterates the melee
/// table's engaged rows (not the full entity table), and threat resolution must keep working for a
/// creature a player dragged far away, per the item's engaged-always-active rule.
/// Work-item 229: gated per candidate on `scope.covers(c.instance_id)` (attacker's instance = the
/// pair's, same as pass_cast). Returns covered candidates visited.
pub(crate) fn pass_threat_retarget(ctx: &ReducerContext, scope: &TickScope) -> usize {
    let mut visited = 0usize;
    let entities = ctx.db.game_world_entity();

    // Threat retarget pass (vanilla aggro: an engaged creature attacks its HIGHEST-THREAT source, not
    // merely whoever it first aggroed). For each ALIVE creature that is the attacker in a melee row, pick
    // the top VALID threat source (`threat::top_threat_target` — alive, same-map, in-world); if it
    // STRICTLY out-threats the creature's CURRENT target, re-point BOTH the melee row's target and the
    // entity's `target_guid` at it. The chase pass (next) and the swing tick then follow the new target.
    //
    // The strict-greater compare is HYSTERESIS: an empty table (no damage dealt yet) or a single-source
    // table (one attacker) leaves the target untouched — byte-identical to the proximity/retaliation
    // behavior. The switch fires only when a SECOND source out-threats the first (e.g. a healer/DPS pulls
    // aggro off the puller, or a taunt tops threat) — exactly the multiplayer threat mechanic. CC does NOT
    // gate this: choosing a target is neither moving nor acting (the swing/chase passes own the stun/root
    // gates); a stunned creature still tracks who it WILL attack when the stun lifts.
    //
    // Snapshot the engaged creatures from the melee table FIRST (collect `(creature, current_target)`),
    // then resolve top-threat + mutate — never write the melee/entity tables while iterating them. The
    // melee snapshot includes player→creature rows; those are filtered out by the `is_player` attacker
    // check (a player has no threat table and is never retargeted by threat).
    let melee_threat = ctx.db.game_melee_attack();
    let engaged: Vec<(u64, u64)> = melee_threat
        .iter()
        .map(|a| (a.attacker_guid, a.target_guid))
        .collect();
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64;
    let mut retargets: Vec<(u64, u64)> = Vec::new(); // (creature, new_target)
    for (creature, current_target) in engaged {
        // Issue #383: the shared gate ladder (creature, alive, this firing's covered instance) — same
        // predicate as the other engaged AI passes. This pass re-finds `creature` itself below (it
        // never needs the resolved row directly), so the gate is a bool check, not a binding.
        if movable_creature(ctx, creature, scope).is_none() {
            continue;
        }
        visited += 1;
        // TAUNT FORCED-TARGET window: a live lock PINS the creature on the taunter regardless of the
        // threat table — the vanilla taunt rule. Validity + lazy reaping of an expired/invalid lock
        // (window over, taunter dead/gone/cross-map) live in `threat::forced_target`; a `None` resumes
        // the normal top-threat compare below.
        if let Some(pinned) = crate::threat::forced_target(ctx, creature, now_ms) {
            if current_target != pinned {
                retargets.push((creature, pinned));
            }
            continue; // pinned — the threat compare is suspended for the window
        }
        // Top valid threat source; an empty/all-invalid table → keep the current target (None).
        let Some(top) = crate::threat::top_threat_target(ctx, creature) else {
            continue;
        };
        // Switch only on a STRICTLY higher threat than the current target (hysteresis — a tie never
        // flaps). A current target absent from the table reads 0, so any real damage-dealer out-threats a
        // pure proximity puller that never hit the creature.
        if top != current_target
            && crate::threat::threat_of(ctx, creature, top)
                > crate::threat::threat_of(ctx, creature, current_target)
        {
            retargets.push((creature, top));
        }
    }
    for (creature, new_target) in retargets {
        // Re-point the engagement (PK = attacker_guid, unchanged) — keep `last_swing_ms` so retargeting
        // does NOT reset the swing cadence (the creature keeps swinging on its own timer at the new foe).
        if let Some(mut row) = melee_threat.attacker_guid().find(creature) {
            row.target_guid = new_target;
            melee_threat.attacker_guid().update(row);
        }
        // Point the entity at the new target (observers see it face/target the new foe).
        if let Some(mut c) = entities.guid().find(creature) {
            if c.target_guid != new_target {
                c.target_guid = new_target;
                entities.guid().update(c);
            }
        }
    }
    visited
}

/// Pass 10 — regen (health + power TOGETHER — they share the `in_combat` snapshot): out-of-combat
/// HP recovery for any entity, then power regen/decay by power type. Runs before flee/fear-flee so a
/// still-engaged runner is skipped by the in-combat gate, not reverted by regen.
///
/// Work-item 230 classification: STAYS GLOBAL — HP/power regen isn't proximity-gated in vanilla either
/// (an out-of-view creature still heals toward full), and this pass covers PLAYERS too, so it's out of
/// this item's creature-ticking scope (mirrors `pass_combat_drop`'s reasoning).
/// Work-item 229: catch-all firing only, still covering ALL instances — the per-sense-tick regen
/// AMOUNT is cadence-quantized, so a second (faster) row running this would literally multiply
/// everyone's regen rate (see `TickScope::runs_global_passes`). Returns entity rows scanned — ONE pass
/// now feeds both the health and the power loop (perf catalog 1.6, partial), so this halved.
pub(crate) fn pass_regen(ctx: &ReducerContext) -> usize {
    let mut visited = 0usize;
    let entities = ctx.db.game_world_entity();
    // Derive now_ms once for the FSR (five-second rule) mana-regen gate.
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64;

    // Health regen: any entity below max HP recovers each tick — out of combat at the full
    // SPIRIT+level-scaled rate, IN combat at a reduced rate if the entity carries one or more
    // `A_COMBAT_HEALTH_REGEN_PCT` auras (e.g. the Troll Regeneration racial passive). Entities in
    // combat with NO such aura are skipped (zero combat health regen — today's behaviour). The heal
    // flows to clients via the game_world_entity on_update VALUES relay.
    // Perf catalog 1.6 (partial): ONE entity pass now feeds the health loop, the power loop AND the
    // IN_COMBAT half of the combatant set. This pass used to iterate the whole table THREE times per
    // sense tick — twice for two disjoint, tiny result sets, and a third time inside
    // `combatant_guids` — so at 50k creatures it cost ~150k row visits to touch a few dozen rows.
    // Byte-identical: the same predicates over the same rows, and the flag half is harvested from
    // THESE rows (read at exactly the point `combatant_guids` would have read them), not a stale
    // snapshot. The remaining single scan is the parked half of 1.6 (a damage-driven
    // `game_regen_pending` set) — see the catalog entry.
    let mut in_combat = crate::combat::melee_combatant_guids(ctx);
    let candidates: Vec<WorldEntity> = entities
        .iter()
        .inspect(|_| visited += 1)
        .filter(|e| {
            if e.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0 {
                in_combat.push(e.guid);
            }
            !e.dead && (e.health < e.max_health || e.max_power > 0)
        })
        .collect();
    for e in candidates.iter().filter(|e| e.health < e.max_health) {
        let is_in_combat = in_combat.contains(&e.guid);
        let next = if is_in_combat {
            // Sum active A_COMBAT_HEALTH_REGEN_PCT auras; skip if there are none.
            let pct = crate::spell::combat_health_regen_pct(ctx, e.guid);
            if pct <= 0 {
                continue; // no aura → zero combat regen (baseline-safe)
            }
            crate::combat::regen_health_in_combat(
                e.health,
                e.max_health,
                e.spirit,
                e.level,
                pct as u32,
            )
        } else {
            crate::combat::regen_entity_health(e)
        };
        if next != e.health {
            // Re-find the LIVE row and write ONLY health — never the snapshot's x/y/z — so a movement
            // write this tick can't be reverted by a stale full-row update (defense-in-depth atop the
            // in-combat gate, which makes the regen-vs-move ordering non-fragile).
            if let Some(mut live) = entities.guid().find(e.guid) {
                if live.health != next {
                    live.health = next;
                    entities.guid().update(live);
                }
            }
        }
    }

    // Power regen/decay by power type (only entities with a power bar — players; creatures carry
    // max_power 0 and are skipped). Mana ticks once the FSR window expires (now_ms >=
    // mana_regen_paused_until_ms), energy ticks always, rage decays out of combat. The change flows
    // to the owner via the on_update power VALUES relay (same path as combat rage). Re-uses
    // `in_combat` from the health pass above; passes `now_ms` for the FSR mana gate.
    for e in candidates.iter().filter(|e| e.max_power > 0) {
        let next = crate::combat::regen_entity_power(e, in_combat.contains(&e.guid), now_ms);
        if next != e.power {
            // Live re-find + power-only write (same reasoning as the health pass above).
            if let Some(mut live) = entities.guid().find(e.guid) {
                if live.power != next {
                    live.power = next;
                    entities.guid().update(live);
                }
            }
        }
    }
    visited
}

#[cfg(test)]
mod aggro_tripwire {
    /// The WIRING, not the predicate (playbook section 8 — and this test exists because the first
    /// version of it pinned only the pure fn, and swapping the call site back to a bare `!e.dead`
    /// left all 561 tests green while a godmoded GM went back to collecting the whole valley).
    ///
    /// `pass_aggro`'s player snapshot must go through `is_aggro_candidate`, which is what excludes a
    /// GODMODED GM. An immortal target never dies, so creatures never disengage and accumulate on
    /// them without bound — 103 simultaneous attackers, observed live.
    ///
    /// Issue #383: moved from `tick.rs`'s combined `due_timer_tripwire` mod to `sense.rs` with the
    /// pass it pins — `pass_aggro` (which owns the player snapshot) lives here now, split out of the
    /// old combined aggro+assist pass; `pass_aggro_assist` is a thin wrapper and no longer contains
    /// this line at all.
    #[test]
    fn the_aggro_pass_skips_godmoded_players() {
        let body = crate::test_scan::code_of(include_str!("sense.rs"), "fn pass_aggro(");
        assert!(
            body.contains("is_aggro_candidate(e.dead, e.godmode)"),
            "the aggro pass no longer routes its player snapshot through `is_aggro_candidate`, so a \
             godmoded GM is an aggro target again. Body was:\n{body}"
        );
    }
}
