use lyracore_shared::spell::taxonomy::{
    A_MOD_HEALING, A_SEAL, A_SPELLMOD_FLAT, A_SPELLMOD_PCT, COMBAT_SPELL_POWER, STAT_INT,
};
use spacetimedb::ReducerContext;

use super::formulas::int_spell_power;
use super::{auras_on, Aura, Spell};
use crate::{game_spell, game_world_entity};

#[cfg(feature = "debug_reducers")]
pub(crate) fn spell_crit_bp(ctx: &ReducerContext, caster_guid: u64) -> u32 {
    (spell_crit_chance(ctx, caster_guid) * 100.0) as u32
}

pub(crate) fn spell_crit_chance(ctx: &ReducerContext, caster_guid: u64) -> f32 {
    let Some(caster) = ctx.db.game_world_entity().guid().find(caster_guid) else {
        return 0.0;
    };
    if !caster.is_player()
        && (caster.owner_guid == 0 || lyracore_shared::guid::high(caster.owner_guid) != 0)
    {
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
    let intellect = ctx
        .db
        .game_world_entity()
        .guid()
        .find(caster_guid)
        .map_or(0, |caster| {
            crate::modifiers::effective_stat(ctx, &caster, STAT_INT)
        });
    int_spell_power(i32::try_from(intellect).unwrap_or(i32::MAX))
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
    spell_power(ctx, caster_guid, school_mask)
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

pub(crate) fn spell_mod(
    ctx: &ReducerContext,
    caster_guid: u64,
    op: i32,
    hdr: &Spell,
) -> (i32, i32) {
    if hdr.family_name == 0 || hdr.family_flags == 0 {
        return (0, 0);
    }
    let spells = ctx.db.game_spell();
    let family_ok = |a: &Aura| {
        a.eff_p0 == op
            && spells.spell_id().find(a.spell_id).is_some_and(|src| {
                src.family_name == hdr.family_name
                    && hdr.family_flags & (a.eff_p1 as u32 as u64) != 0
            })
    };
    let flat = crate::modifiers::aura_total(ctx, caster_guid, |a| {
        a.eff_kind == A_SPELLMOD_FLAT && family_ok(a)
    });
    let pct = crate::modifiers::aura_total(ctx, caster_guid, |a| {
        a.eff_kind == A_SPELLMOD_PCT && family_ok(a)
    });
    (flat, pct)
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
