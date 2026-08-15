//! The sensing/AI-decision passes still waiting for the behavior cycle: cast, threat-retarget and
//! regen. `creatures::cycle::run_cycle` owns WHEN each runs; aggro and assist already live there.

use spacetimedb::{ReducerContext, Table};

use crate::{game_melee_attack, game_world_entity, WorldEntity};

use super::*;

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
