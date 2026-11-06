use spacetimedb::{ReducerContext, Table};

use super::*;
use crate::{game_character_talent, game_player_spell, game_talent};
use lyracore_shared::spell::taxonomy::P_COMBAT_PERCENT;

pub(crate) fn repair_cast_auras(ctx: &ReducerContext) {
    let auras: Vec<_> = ctx
        .db
        .game_aura()
        .iter()
        .filter(|aura| aura.effect_id != 0 && aura.item_guid(ctx) == 0)
        .collect();
    for mut aura in auras {
        let definition = ctx
            .db
            .game_spell_effect()
            .id()
            .find(aura.effect_id)
            .zip(ctx.db.game_spell().spell_id().find(aura.spell_id));
        let Some((effect, header)) = definition.filter(|(effect, _)| {
            effect.spell_id == aura.spell_id && effect.kind & KIND_AURA_BIT != 0
        }) else {
            shed_auras(ctx, aura.target_guid, vec![aura]);
            continue;
        };
        let p1 = if matches!(effect.kind, A_PERIODIC_TRIGGER | A_PROC_TRIGGER) {
            effect.trigger_spell as i32
        } else {
            effect.p1
        };
        let period_ms = if effect.period_ms == 0 && header.cast_flags & SPELL_ATTR_CHANNELED != 0 {
            100
        } else {
            effect.period_ms
        };
        let visible =
            super::cast::aura_has_visible_slot(ctx, &header, aura.caster_guid, aura.target_guid);
        let wrong_slot = if visible {
            aura.slot >= 48 || (aura.slot >= 32) != header.negative()
        } else {
            aura.slot != u8::MAX
        };
        if !wrong_slot
            && (
                aura.eff_kind,
                aura.eff_p0,
                aura.eff_p0_kind,
                aura.eff_p1,
                aura.period_ms,
                aura.enters_combat,
            ) == (
                effect.kind,
                effect.p0,
                effect.p0_kind,
                p1,
                period_ms,
                effect.enters_combat,
            )
        {
            continue;
        }
        if wrong_slot && !visible {
            aura.slot = u8::MAX;
        } else if wrong_slot {
            let mut slots = if header.negative() { 32..48 } else { 0..32 };
            let current: Vec<_> = ctx
                .db
                .game_aura()
                .by_target()
                .filter(aura.target_guid)
                .collect();
            let slot = current
                .iter()
                .find(|other| other.spell_id == aura.spell_id && slots.contains(&other.slot))
                .map(|other| other.slot)
                .or_else(|| slots.find(|slot| current.iter().all(|other| other.slot != *slot)));
            let Some(slot) = slot else {
                shed_auras(ctx, aura.target_guid, vec![aura]);
                continue;
            };
            aura.slot = slot;
        }
        if aura.eff_p0_kind != P_COMBAT_PERCENT && effect.p0_kind == P_COMBAT_PERCENT {
            aura.amount = aura.amount.saturating_mul(100);
        } else if aura.eff_p0_kind == P_COMBAT_PERCENT && effect.p0_kind != P_COMBAT_PERCENT {
            aura.amount /= 100;
        }
        if aura.spell_id == 12051
            && effect.effect_index == 0
            && aura.eff_kind == A_PERIODIC_ENERGIZE
            && effect.kind == A_MOD_POWER_REGEN_PCT
        {
            aura.amount = effect.base_points;
        }
        if effect.kind != A_ABSORB {
            aura.amount_remaining = 0;
        } else if aura.eff_kind != A_ABSORB {
            aura.amount_remaining = aura.amount.max(0);
        }
        if aura.period_ms != period_ms {
            aura.next_tick_micros = if period_ms == 0 {
                0
            } else {
                crate::time::now(ctx).to_micros_since_unix_epoch() + i64::from(period_ms) * 1000
            };
        }
        aura.eff_kind = effect.kind;
        aura.eff_p0 = effect.p0;
        aura.eff_p0_kind = effect.p0_kind;
        aura.eff_p1 = p1;
        aura.period_ms = period_ms;
        aura.enters_combat = effect.enters_combat;
        let target = aura.target_guid;
        aura.update(ctx);
        recompute_vitals(ctx, target);
        recompute_sheet(ctx, target);
    }
    restore_missing_passives(ctx);
}

fn restore_missing_passives(ctx: &ReducerContext) {
    for learned in ctx.db.game_player_spell().iter() {
        let Some(header) = ctx
            .db
            .game_spell()
            .spell_id()
            .find(learned.spell_id)
            .filter(|header| header.attributes & SPELL_ATTR_PASSIVE != 0)
        else {
            continue;
        };
        let Some(character) = crate::helpers::character_by_guid(ctx, learned.character_guid) else {
            continue;
        };
        let rank = ctx
            .db
            .game_character_talent()
            .by_character()
            .filter(character.guid)
            .find_map(|learned| {
                let talent = ctx.db.game_talent().talent_id().find(learned.talent_id)?;
                (talent.tab_id == 0
                    && crate::talent::talent_rank_spell(&talent, learned.rank) == header.spell_id)
                    .then_some(learned.rank)
            })
            .unwrap_or(1);
        let mut header = header;
        if header.duration_ms == 0 {
            header.duration_ms = u32::MAX;
        }
        for effect in ctx
            .db
            .game_spell_effect()
            .by_spell()
            .filter(header.spell_id)
            .filter(|effect| effect.kind & KIND_AURA_BIT != 0)
        {
            if ctx
                .db
                .game_aura()
                .by_target()
                .filter(character.guid)
                .any(|aura| {
                    aura.effect_id == effect.id
                        && aura.caster_guid == character.guid
                        && aura.item_guid(ctx) == 0
                })
            {
                continue;
            }
            aura_apply(
                ctx,
                &effect,
                &header,
                character.guid,
                character.guid,
                character.guid,
                character.level,
                i32::from(rank) * effect.base_points,
                0,
            );
        }
    }
}
