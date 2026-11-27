use lyracore_shared::spell::taxonomy::{A_MOD_HEALING, A_SEAL, COMBAT_SPELL_POWER, STAT_INT};
use spacetimedb::ReducerContext;

use super::auras_on;
use crate::game_world_entity;

#[cfg(feature = "debug_reducers")]
pub(crate) fn spell_crit_bp(ctx: &ReducerContext, caster_guid: u64) -> u32 {
    (spell_crit_chance(ctx, caster_guid) * 100.0) as u32
}

pub(crate) fn spell_crit_chance(ctx: &ReducerContext, caster_guid: u64) -> f32 {
    let Some(caster) = ctx.db.game_world_entity().guid().find(caster_guid) else {
        return 0.0;
    };
    if !caster.crits_with_spells() {
        return 0.0;
    }
    let base = super::rolls::intellect_crit_chance(
        caster.is_player().then(|| caster.class()),
        crate::modifiers::effective_stat(ctx, &caster, STAT_INT),
        caster.level,
    );
    (base
        + crate::modifiers::combat_field_bonus(
            ctx,
            caster_guid,
            lyracore_shared::spell::taxonomy::COMBAT_SPELL_CRIT,
        ) as f32
            / 100.0)
        .clamp(0.0, 100.0)
}

pub(crate) fn spell_power(ctx: &ReducerContext, caster_guid: u64, school_mask: u8) -> i32 {
    super::bonuses::damage_power(ctx, caster_guid, school_mask)
        .saturating_add(crate::modifiers::combat_field_bonus(
            ctx,
            caster_guid,
            COMBAT_SPELL_POWER,
        ))
        .saturating_add(crate::items::equipped_school_stat_bonus(
            ctx,
            caster_guid,
            crate::items::EquipStat::SpellPower,
            school_mask,
        ))
}

pub(crate) fn healing_power(ctx: &ReducerContext, caster_guid: u64, school_mask: u8) -> i32 {
    crate::modifiers::combat_field_bonus(ctx, caster_guid, COMBAT_SPELL_POWER)
        .saturating_add(crate::modifiers::aura_total(ctx, caster_guid, |aura| {
            aura.eff_kind == A_MOD_HEALING && aura.eff_p0 & i32::from(school_mask) != 0
        }))
        .saturating_add(crate::items::equipped_school_stat_bonus(
            ctx,
            caster_guid,
            crate::items::EquipStat::HealingPower,
            school_mask,
        ))
}

pub(crate) fn seal_amount(ctx: &ReducerContext, unit_guid: u64) -> i32 {
    crate::modifiers::aura_total(ctx, unit_guid, |a| a.eff_kind == A_SEAL)
}

pub(crate) fn seal_amount_and_spell(ctx: &ReducerContext, unit_guid: u64) -> (i32, u32) {
    auras_on(ctx, unit_guid)
        .filter(|a| a.eff_kind == A_SEAL)
        .fold((0, 0), |(amount, spell), a| {
            (
                amount + crate::modifiers::AuraAmount::from(&a).total(),
                if spell == 0 { a.spell_id } else { spell },
            )
        })
}
