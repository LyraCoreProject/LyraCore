use lyracore_shared::packing::power_type;
use spacetimedb::{table, ReducerContext, Table};

use crate::stats::game_class_level_stats;
use crate::{game_spell, game_spell_effect, game_world_entity, WorldEntity};

use super::{STANCE_BEAR, STANCE_CAT, STANCE_DIRE_BEAR};

#[table(accessor = game_druid_form_mana)]
pub struct DruidFormMana {
    #[primary_key]
    pub character_guid: u64,
    pub mana: u32,
}

crate::character_owned!(delete, fn sweep_delete_game_druid_form_mana(ctx, character_guid) {
    ctx.db.game_druid_form_mana().character_guid().delete(character_guid);
});
crate::character_owned!(not_transported, fn sweep_transfer_game_druid_form_mana());

pub(crate) fn is_bear(entity: &WorldEntity) -> bool {
    entity.class() == 11 && matches!(entity.stance, STANCE_BEAR | STANCE_DIRE_BEAR)
}

pub(crate) fn power_kind(class: u8, stance: u8) -> u8 {
    if class == 11 {
        match stance {
            STANCE_BEAR | STANCE_DIRE_BEAR => return power_type::RAGE,
            STANCE_CAT => return power_type::ENERGY,
            _ => {}
        }
    }
    power_type::for_class(class)
}

pub(crate) fn mana_for_persist(ctx: &ReducerContext, entity: &WorldEntity) -> u32 {
    ctx.db
        .game_druid_form_mana()
        .character_guid()
        .find(entity.guid)
        .filter(|_| entity.class() == 11 && entity.power_type() != power_type::MANA)
        .map_or(entity.power, |saved| saved.mana)
}

pub(crate) fn forget(ctx: &ReducerContext, guid: u64) {
    ctx.db.game_druid_form_mana().character_guid().delete(guid);
}

pub(crate) fn refill_mana(ctx: &ReducerContext, body: &WorldEntity) {
    if let Some(mut saved) = ctx
        .db
        .game_druid_form_mana()
        .character_guid()
        .find(body.guid)
    {
        saved.mana = super::effects::max_mana(ctx, body);
        ctx.db.game_druid_form_mana().character_guid().update(saved);
    }
}

fn bear_shift(spell: &super::Spell) -> Option<u8> {
    match spell.spell_id {
        5487 => Some(STANCE_BEAR),
        9634 => Some(STANCE_DIRE_BEAR),
        _ => None,
    }
}

pub(crate) fn wrong_resource(caster: &WorldEntity, spell: &super::Spell) -> bool {
    caster.class() == 11
        && caster.power_type() != spell.power_type
        && spell.cost > 0
        && bear_shift(spell).is_none()
}

pub(crate) fn cast_cost(ctx: &ReducerContext, caster: &WorldEntity, spell: &super::Spell) -> u32 {
    let Some(stance) = bear_shift(spell).filter(|_| caster.class() == 11) else {
        return spell.cost;
    };
    if caster.stance == stance {
        return 0;
    }
    // Spell.dbc 5487/9634 charge 55% base mana; see docs/wave-defense.md for form data.
    ctx.db
        .game_class_level_stats()
        .class_level()
        .find(crate::stats::class_level_key(caster.class(), caster.level))
        .map_or(0, |stats| (u64::from(stats.base_mana) * 55 / 100) as u32)
}

pub(crate) fn cast_power(ctx: &ReducerContext, caster: &WorldEntity, spell: &super::Spell) -> u32 {
    if bear_shift(spell).is_some() && caster.class() == 11 {
        mana_for_persist(ctx, caster)
    } else {
        caster.power
    }
}

pub(crate) fn spend_cast_power(
    ctx: &ReducerContext,
    caster: &mut WorldEntity,
    spell: &super::Spell,
    cost: u32,
) {
    if bear_shift(spell).is_some()
        && caster.class() == 11
        && caster.power_type() != power_type::MANA
    {
        if let Some(mut saved) = ctx
            .db
            .game_druid_form_mana()
            .character_guid()
            .find(caster.guid)
        {
            saved.mana = saved.mana.saturating_sub(cost);
            ctx.db.game_druid_form_mana().character_guid().update(saved);
        }
    } else {
        caster.power = caster.power.saturating_sub(cost);
    }
}

pub(crate) fn clear(ctx: &ReducerContext, guid: u64) {
    if ctx
        .db
        .game_world_entity()
        .guid()
        .find(guid)
        .is_some_and(|body| body.class() == 11 && body.stance != 0)
    {
        change(ctx, guid, 0);
    }
    forget(ctx, guid);
}

pub(crate) fn change(ctx: &ReducerContext, guid: u64, stance: u8) {
    let Some(mut body) = ctx.db.game_world_entity().guid().find(guid) else {
        return;
    };
    let old_kind = body.power_type();
    let new_kind = power_kind(body.class(), stance);
    if body.class() == 11 {
        if old_kind == power_type::MANA && new_kind != power_type::MANA {
            forget(ctx, guid);
            ctx.db.game_druid_form_mana().insert(DruidFormMana {
                character_guid: guid,
                mana: body.power,
            });
        }
        if new_kind == power_type::MANA && old_kind != power_type::MANA {
            body.power = mana_for_persist(ctx, &body);
            forget(ctx, guid);
        } else if stance != body.stance && new_kind != power_type::MANA {
            body.power = 0;
        }
    } else if super::stance_switch_clears_power(body.stance, stance, old_kind) {
        body.power = 0;
    }
    let old_health = body.health;
    let old_max_health = body.max_health;
    body.stance = stance;
    body.unit_bytes_0 = (body.unit_bytes_0 & 0x00FF_FFFF) | (u32::from(new_kind) << 24);
    // Druid caster form is zero; Warrior stance zero maps to client form 17.
    let form = if body.class() == 11 && stance == 0 {
        0
    } else {
        super::client_form_for_stance(stance)
    };
    // Byte 3 carries ghost visibility.
    body.unit_bytes_1 = (body.unit_bytes_1 & 0xFF00_0000) | (form << 16);
    body.update(ctx);
    super::recompute_vitals(ctx, guid);
    if let Some(mut body) = ctx.db.game_world_entity().guid().find(guid) {
        if body.max_health != old_max_health && old_max_health != 0 && !body.dead {
            body.health = ((u64::from(old_health) * u64::from(body.max_health)
                / u64::from(old_max_health)) as u32)
                .clamp(1, body.max_health);
            body.update(ctx);
        }
    }
    super::recompute_sheet(ctx, guid);
    super::recompute_display(ctx, guid);
}

fn boost_spell(entity: &WorldEntity) -> Option<u32> {
    if entity.class() != 11 {
        return None;
    }
    match entity.stance {
        STANCE_BEAR => Some(1178),
        STANCE_DIRE_BEAR => Some(9635),
        _ => None,
    }
}

// Classic form boosts use Spell.dbc 1178/9635; see docs/wave-defense.md for verification sources.
fn boost(ctx: &ReducerContext, entity: &WorldEntity, effect_index: u8) -> i32 {
    let Some(spell_id) = boost_spell(entity) else {
        return 0;
    };
    spell_boost(ctx, entity, spell_id, effect_index)
}

fn spell_boost(ctx: &ReducerContext, entity: &WorldEntity, spell_id: u32, effect_index: u8) -> i32 {
    let Some(spell) = ctx.db.game_spell().spell_id().find(spell_id) else {
        return 0;
    };
    let Some(effect) = ctx
        .db
        .game_spell_effect()
        .id()
        .find((u64::from(spell_id) << 2) | u64::from(effect_index))
    else {
        return 0;
    };
    super::effect_amount(
        effect.base_points,
        effect.die_sides,
        effect.per_level,
        entity.level,
        spell.spell_level,
        spell.max_level,
        0,
    )
}

pub(crate) fn health_bonus(ctx: &ReducerContext, entity: &WorldEntity) -> i32 {
    boost(ctx, entity, 1)
}

pub(crate) fn attack_power_bonus(ctx: &ReducerContext, entity: &WorldEntity) -> i32 {
    boost(ctx, entity, 2)
}

pub(crate) fn threat_bonus(ctx: &ReducerContext, entity: &WorldEntity) -> i32 {
    if is_bear(entity) {
        spell_boost(ctx, entity, 21178, 0)
    } else {
        0
    }
}

pub(crate) fn armor_bonus(ctx: &ReducerContext, entity: &WorldEntity) -> i32 {
    let percent = boost(ctx, entity, 0);
    if percent == 0 {
        return 0;
    }
    let armor = crate::items::equipped_stat_bonus(ctx, entity.guid, crate::items::EquipStat::Armor);
    (i64::from(armor.max(0)) * i64::from(percent) / 100).clamp(0, i64::from(i32::MAX)) as i32
}

pub(crate) fn display(entity: &WorldEntity) -> Option<u32> {
    is_bear(entity).then_some(if entity.race() == 4 { 2281 } else { 2289 })
}
