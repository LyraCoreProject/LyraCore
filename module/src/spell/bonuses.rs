use spacetimedb::ReducerContext;

use super::*;
use crate::{game_item_template, game_world_entity};

fn amount(aura: &Aura) -> i32 {
    lyracore_shared::aura::stacked_amount(aura.amount, aura.stacks)
}

fn unrestricted(header: &Spell) -> bool {
    header.equipped_item_class == -1 && header.equipped_item_inventory_mask == 0
}

fn fits_weapon(ctx: &ReducerContext, guid: u64, slot: u8, header: &Spell) -> bool {
    if header.equipped_item_class == -1 {
        return true;
    }
    if ctx
        .db
        .game_world_entity()
        .guid()
        .find(guid)
        .is_some_and(|unit| matches!(unit.stance, STANCE_BEAR | STANCE_DIRE_BEAR | STANCE_CAT))
        || (slot != crate::items::equip_slot::RANGED && super::is_disarmed(ctx, guid))
    {
        return false;
    }
    let Some(item) = crate::items::item_in_slot(ctx, guid, slot) else {
        return false;
    };
    let Some(template) = ctx.db.game_item_template().entry().find(item.entry) else {
        return false;
    };
    !crate::items::item_is_broken(&template, &item)
        && i32::from(template.class) == header.equipped_item_class
        && (header.equipped_item_subclass_mask == 0
            || header.equipped_item_subclass_mask
                & 1u32.checked_shl(u32::from(template.subclass)).unwrap_or(0)
                != 0)
        && (header.cast_flags & lyracore_shared::spell::taxonomy::SPELL_ATTR_ITEM_TARGET == 0
            || header.equipped_item_inventory_mask == 0
            || header.equipped_item_inventory_mask
                & 1u32
                    .checked_shl(u32::from(template.inventory_type))
                    .unwrap_or(0)
                != 0)
}

pub(crate) fn damage_power(ctx: &ReducerContext, guid: u64, school: u8) -> i32 {
    auras_on(ctx, guid)
        .filter(|aura| {
            aura.eff_kind == A_MOD_DAMAGE_DONE_FLAT && aura.eff_p0 & i32::from(school) != 0
        })
        .filter(|aura| {
            ctx.db
                .game_spell()
                .spell_id()
                .find(aura.spell_id)
                .is_some_and(|header| unrestricted(&header))
        })
        .map(|aura| amount(&aura))
        .sum()
}

pub(crate) fn school_crit(ctx: &ReducerContext, guid: u64, school: u8) -> f32 {
    if ctx
        .db
        .game_world_entity()
        .guid()
        .find(guid)
        .is_none_or(|unit| {
            !unit.is_player() && (unit.owner_guid == 0 || unit.owner_guid >> 48 != 0)
        })
    {
        return 0.0;
    }
    auras_on(ctx, guid)
        .filter(|aura| aura.eff_kind == A_MOD_SCHOOL_CRIT && aura.eff_p0 & i32::from(school) != 0)
        .map(|aura| amount(&aura) as f32)
        .sum()
}

pub(crate) fn weapon_damage(
    ctx: &ReducerContext,
    unit: &crate::WorldEntity,
    slot: u8,
    base: f32,
) -> f32 {
    let mut flat = 0;
    let mut multiplier = 1.0;
    for aura in auras_on(ctx, unit.guid).filter(|aura| {
        matches!(
            aura.eff_kind,
            A_MOD_DAMAGE_DONE_FLAT | A_MOD_DAMAGE_DONE_PCT
        )
    }) {
        let Some(header) = ctx.db.game_spell().spell_id().find(aura.spell_id) else {
            continue;
        };
        let applies = if !unit.is_player() || header.equipped_item_class == -1 {
            aura.eff_p0 & 1 != 0
        } else {
            (aura.eff_p0 & 1 != 0 || matches!(unit.class(), 5 | 8 | 9))
                && fits_weapon(ctx, unit.guid, slot, &header)
        };
        if applies {
            if aura.eff_kind == A_MOD_DAMAGE_DONE_FLAT {
                flat += amount(&aura);
            } else {
                multiplier *= (100 + amount(&aura)) as f32 / 100.0;
            }
        }
    }
    ((base + flat as f32) * multiplier).max(0.0)
}

pub(crate) fn spell_damage_done_float(
    ctx: &ReducerContext,
    hdr: &Spell,
    effect: &SpellEffect,
    caster: u64,
    points: i32,
    periodic: bool,
) -> f32 {
    if hdr.custom_flags & lyracore_shared::spell_custom::FIXED_DAMAGE != 0
        || hdr.attributes_ex_3 & 0x2000_0000 != 0
        || (hdr.family_name == 3 && hdr.family_flags & 0x0800_0000 != 0)
    {
        return points as f32;
    }
    let power = if matches!(hdr.damage_class, 2 | 3) {
        auras_on(ctx, caster)
            .filter(|aura| {
                aura.eff_kind == A_MOD_DAMAGE_DONE_FLAT
                    && aura.eff_p0 & i32::from(hdr.school_mask) != 0
                    && (aura.eff_p0 & 1 != 0 || hdr.damage_class == 3)
            })
            .filter(|aura| {
                ctx.db
                    .game_spell()
                    .spell_id()
                    .find(aura.spell_id)
                    .is_some_and(|source| {
                        fits_weapon(
                            ctx,
                            caster,
                            if hdr.damage_class == 3 {
                                crate::items::equip_slot::RANGED
                            } else {
                                crate::items::equip_slot::MAINHAND
                            },
                            &source,
                        )
                    })
            })
            .map(|aura| amount(&aura))
            .sum()
    } else {
        spell_power(ctx, caster, hdr.school_mask)
    };
    let mut damage =
        points as f32 + power as f32 * spell_coefficient(ctx, hdr, effect, caster, true);
    for aura in auras_on(ctx, caster).filter(|aura| {
        aura.eff_kind == A_MOD_DAMAGE_DONE_PCT && aura.eff_p0 & i32::from(hdr.school_mask) != 0
    }) {
        if ctx
            .db
            .game_spell()
            .spell_id()
            .find(aura.spell_id)
            .is_some_and(|source| {
                if matches!(hdr.damage_class, 2 | 3) {
                    aura.eff_p0 & 1 != 0
                        && fits_weapon(
                            ctx,
                            caster,
                            if hdr.damage_class == 3 {
                                crate::items::equip_slot::RANGED
                            } else {
                                crate::items::equip_slot::MAINHAND
                            },
                            &source,
                        )
                } else {
                    unrestricted(&source) && hdr.equipped_item_class == -1
                }
            })
        {
            damage *= (100 + amount(&aura)) as f32 / 100.0;
        }
    }
    let (flat, percent) = spell_mod(
        ctx,
        caster,
        if periodic { 22 } else { SPELLMOD_OP_DAMAGE },
        hdr,
    );
    ((damage + flat as f32) * (100 + percent) as f32 / 100.0).max(0.0)
}

pub(crate) fn spell_damage_taken(
    ctx: &ReducerContext,
    hdr: &Spell,
    effect: &SpellEffect,
    _caster: u64,
    target: u64,
    damage: f32,
    stacks: u8,
) -> f32 {
    if hdr.attributes_ex_4 & 0x100 != 0
        || (hdr.family_name == 10 && hdr.damage_class == 2)
        || hdr.spell_id == 20424
    {
        return damage;
    }
    let flat: i32 = auras_on(ctx, target)
        .filter(|aura| {
            aura.eff_kind == A_MOD_DAMAGE_TAKEN_FLAT
                && aura.eff_p0 & i32::from(hdr.school_mask) != 0
        })
        .map(|aura| amount(&aura))
        .sum();
    let mut bonus =
        flat as f32 * spell_coefficient(ctx, hdr, effect, target, false) * f32::from(stacks.max(1));
    if !matches!(hdr.damage_class, 2 | 3) {
        bonus = bonus.max(-(damage / 2.0));
    }
    ((damage + bonus) * damage_taken_multiplier(ctx, target, hdr.school_mask)).max(0.0)
}

pub(crate) fn damage_taken_multiplier(ctx: &ReducerContext, target: u64, school: u8) -> f32 {
    auras_on(ctx, target)
        .filter(|aura| aura.eff_kind == A_MOD_DAMAGE_TAKEN && aura.eff_p0 & i32::from(school) != 0)
        .fold(1.0, |multiplier, aura| {
            multiplier * (100 + amount(&aura)) as f32 / 100.0
        })
}

pub(crate) fn weapon_damage_taken(ctx: &ReducerContext, target: u64, damage: u32) -> u32 {
    if damage == 0 {
        return 0;
    }
    let flat: i32 = auras_on(ctx, target)
        .filter(|aura| aura.eff_kind == A_MOD_DAMAGE_TAKEN_FLAT && aura.eff_p0 & 1 != 0)
        .map(|aura| amount(&aura))
        .sum();
    ((damage as f32 + flat as f32) * damage_taken_multiplier(ctx, target, 1)).max(0.0) as u32
}

pub(crate) fn healing_taken(
    ctx: &ReducerContext,
    hdr: &Spell,
    effect: &SpellEffect,
    target: u64,
    healing: f32,
) -> f32 {
    if hdr.damage_class == 0 {
        return healing;
    }
    let flat: i32 = auras_on(ctx, target)
        .filter(|aura| {
            aura.eff_kind == A_MOD_HEALING_TAKEN && aura.eff_p0 & i32::from(hdr.school_mask) != 0
        })
        .map(|aura| amount(&aura))
        .sum();
    let bonus =
        (flat as f32 * spell_coefficient(ctx, hdr, effect, target, false)).max(-(healing / 2.0));
    (healing + bonus).max(0.0)
}

pub(crate) fn damage_fields(
    ctx: &ReducerContext,
    guid: u64,
) -> lyracore_protocol::codec::DamageBonusValues {
    let mut fields = lyracore_protocol::codec::DamageBonusValues::default();
    for aura in auras_on(ctx, guid).filter(|aura| {
        matches!(
            aura.eff_kind,
            A_MOD_DAMAGE_DONE_FLAT | A_MOD_DAMAGE_DONE_PCT
        )
    }) {
        let Some(header) = ctx.db.game_spell().spell_id().find(aura.spell_id) else {
            continue;
        };
        for school in 0..7 {
            if aura.eff_p0 & (1 << school) == 0 || (school != 0 && !unrestricted(&header)) {
                continue;
            }
            let amount = amount(&aura);
            if aura.eff_kind == A_MOD_DAMAGE_DONE_PCT {
                if unrestricted(&header) {
                    fields.percent[school] *= (100 + amount) as f32 / 100.0;
                }
            } else if header.negative() {
                fields.negative[school] += amount;
            } else {
                fields.positive[school] += amount;
            }
        }
    }
    fields
}

pub(crate) fn weapon_spell_damage_taken(
    ctx: &ReducerContext,
    spell: &Spell,
    target: u64,
    damage: u32,
) -> u32 {
    if damage == 0 || spell.attributes_ex_4 & 0x100 != 0 {
        return damage;
    }
    let flat: i32 = auras_on(ctx, target)
        .filter(|aura| {
            aura.eff_kind == A_MOD_DAMAGE_TAKEN_FLAT
                && aura.eff_p0 & i32::from(spell.school_mask) != 0
        })
        .map(|aura| amount(&aura))
        .sum();
    ((damage as f32 + flat as f32) * damage_taken_multiplier(ctx, target, spell.school_mask))
        .max(0.0) as u32
}

pub(crate) fn round_damage(ctx: &ReducerContext, target: u64, damage: f32) -> i32 {
    let whole = damage.max(0.0) as i32;
    let fraction = damage - whole as f32;
    if fraction > 0.0
        && f64::from(super::rolls::target_draw(ctx, target)) / 4_294_967_296.0 < f64::from(fraction)
    {
        whole + 1
    } else {
        whole
    }
}

pub(crate) fn spell_coefficient(
    ctx: &ReducerContext,
    hdr: &Spell,
    effect: &SpellEffect,
    modifier: u64,
    done: bool,
) -> f32 {
    let coefficient = if hdr.family_name == 10 && hdr.spell_id == 20424 {
        if done {
            0.20
        } else {
            0.29
        }
    } else {
        effect.bonus_coefficient.max(0.0)
    };
    let (flat, percent) = spell_mod(ctx, modifier, 24, hdr);
    (coefficient * 100.0 + flat as f32) * (100 + percent) as f32 / 10000.0
}
