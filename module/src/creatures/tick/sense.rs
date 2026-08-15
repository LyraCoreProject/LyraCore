//! The one sensing pass still waiting for the behavior cycle: regen. `creatures::cycle::run_cycle`
//! owns WHEN it runs; aggro, assist, cast and threat retarget already live there.

use spacetimedb::{ReducerContext, Table};

use crate::{game_world_entity, WorldEntity};

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
