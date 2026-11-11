use spacetimedb::ReducerContext;

use crate::game_world_entity;

use crate::game_melee_attack;

use super::{
    add_flat_pool_bonus, dispel_category_matches, drain_shields, game_aura, game_spell,
    rederive_pool, shed_auras, should_arm_spell_retaliation, Aura, A_ABSORB, COMBAT_ATTACK_POWER,
};

pub(crate) fn arm_spell_retaliation(ctx: &ReducerContext, caster_guid: u64, target_guid: u64) {
    crate::combat::engage_passive_creature(ctx, target_guid, caster_guid);
    let entities = ctx.db.game_world_entity();
    let caster_is_player = entities
        .guid()
        .find(caster_guid)
        .map(|c| c.is_player())
        .unwrap_or(false);
    let target_is_player = entities
        .guid()
        .find(target_guid)
        .map(|t| t.is_player())
        .unwrap_or(true);
    let melee = ctx.db.game_melee_attack();
    let already_engaged = melee.attacker_guid().find(target_guid).is_some();
    if should_arm_spell_retaliation(caster_is_player, target_is_player, already_engaged) {
        crate::combat::arm_creature_engagement(ctx, target_guid, caster_guid, false);
    }
}

pub(crate) fn dispel_target(
    ctx: &ReducerContext,
    caster_guid: u64,
    target_guid: u64,
    category: u8,
) -> u32 {
    let entities = ctx.db.game_world_entity();
    let (Some(caster), Some(target)) = (
        entities.guid().find(caster_guid),
        entities.guid().find(target_guid),
    ) else {
        return 0;
    };
    let friendly = matches!(
        crate::combat::relation(ctx, &target, &caster),
        crate::combat::Relation::SelfUnit | crate::combat::Relation::Friendly
    );
    let spells = ctx.db.game_spell();
    let dispelled: Vec<Aura> = ctx
        .db
        .game_aura()
        .by_target()
        .filter(target_guid)
        .filter(|aura| {
            spells.spell_id().find(aura.spell_id).is_some_and(|spell| {
                spell.dispel_type != 0
                    && dispel_category_matches(category, spell.dispel_type)
                    && (!matches!(spell.dispel_type, 1 | 4) || spell.negative() == friendly)
            })
        })
        .collect();
    let removed = dispelled.len() as u32;
    shed_auras(ctx, dispelled);
    removed
}

pub(crate) fn recompute_vitals(ctx: &ReducerContext, unit_guid: u64) {
    let entities = ctx.db.game_world_entity();
    let Some(mut e) = entities.guid().find(unit_guid) else {
        return;
    };
    if !e.is_player() {
        return;
    }
    let (race, class, _gender, _pt) = lyracore_shared::packing::unpack4(e.unit_bytes_0);
    let [stamina, intellect, new_spirit] =
        crate::modifiers::VITALS_STATS.map(|stat| crate::modifiers::effective_stat(ctx, &e, stat));
    let gear = |which| crate::items::equipped_stat_bonus(ctx, unit_guid, which);

    let new_max_health = add_flat_pool_bonus(
        rederive_pool(
            crate::stats::max_health_for(ctx, race, class, e.level),
            e.stamina,
            stat_bonus(e.stamina, stamina),
            crate::stats::hp_from_stamina,
        ),
        gear(crate::items::EquipStat::Health),
    );

    let is_mana = lyracore_shared::packing::power_type::for_class(class)
        == lyracore_shared::packing::power_type::MANA;
    let new_max_power = if is_mana {
        add_flat_pool_bonus(
            rederive_pool(
                crate::stats::max_power_for(ctx, race, class, e.level),
                e.intellect,
                stat_bonus(e.intellect, intellect),
                crate::stats::mana_from_intellect,
            ),
            gear(crate::items::EquipStat::Mana),
        )
    } else {
        e.max_power
    };

    if new_max_health == e.max_health && new_max_power == e.max_power && new_spirit == e.spirit {
        return;
    }

    e.max_health = new_max_health;
    e.health = e.health.min(new_max_health);
    e.max_power = new_max_power;
    e.power = e.power.min(new_max_power);
    e.spirit = new_spirit;
    e.update(ctx);
}

fn stat_bonus(base: u32, effective: u32) -> i32 {
    (i64::from(effective) - i64::from(base)).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

pub(crate) fn recompute_sheet(ctx: &ReducerContext, unit_guid: u64) {
    let entities = ctx.db.game_world_entity();
    let Some(mut e) = entities.guid().find(unit_guid) else {
        return;
    };
    if !e.is_player() {
        return;
    }
    let (_, class, _, _) = lyracore_shared::packing::unpack4(e.unit_bytes_0);
    let eff_str =
        crate::modifiers::effective_stat(ctx, &e, lyracore_shared::spell::taxonomy::STAT_STR);
    let eff_agi =
        crate::modifiers::effective_stat(ctx, &e, lyracore_shared::spell::taxonomy::STAT_AGI);
    let [str_bonus, agi_bonus, sta_bonus, int_bonus, spi_bonus] =
        crate::modifiers::SHEET_STATS.map(|stat| crate::modifiers::sheet_stat_bonus(ctx, &e, stat));

    let ap_base = crate::combat::melee_attack_power_for(class, eff_str, eff_agi, e.level);
    let ap_mods = crate::modifiers::combat_field_bonus(ctx, unit_guid, COMBAT_ATTACK_POWER);
    let (dmg_min, dmg_max) = crate::combat::swing_range_ctx(ctx, &e);
    let ap_modifiers = crate::combat::attack_power_modifiers(ctx, unit_guid, COMBAT_ATTACK_POWER);
    // Priest, mage and warlock ignore ranged AP modifiers.
    let ranged_ap_modifiers = if matches!(class, 5 | 8 | 9) {
        0
    } else {
        crate::combat::attack_power_modifiers(
            ctx,
            unit_guid,
            lyracore_shared::spell::taxonomy::COMBAT_RANGED_ATTACK_POWER,
        )
    };
    let (ranged_ap, ranged_dmg_min, ranged_dmg_max) = crate::combat::ranged_sheet_values(
        class,
        eff_agi,
        e.level,
        crate::combat::equipped_ranged_weapon(ctx, unit_guid),
        crate::modifiers::combat_field_bonus(
            ctx,
            unit_guid,
            lyracore_shared::spell::taxonomy::COMBAT_RANGED_ATTACK_POWER,
        ),
    );
    let ranged_min_fraction = ranged_dmg_min.fract();
    let ranged_max_fraction = ranged_dmg_max.fract();
    let (ranged_dmg_min, ranged_dmg_max) = (ranged_dmg_min as u32, ranged_dmg_max as u32);
    let crit_bp = crate::combat::effective_crit_bp(ctx, &e);
    let ranged_crit_bp = crate::combat::effective_ranged_crit_bp(ctx, &e);

    if str_bonus == e.sheet_str_bonus
        && agi_bonus == e.sheet_agi_bonus
        && sta_bonus == e.sheet_sta_bonus
        && int_bonus == e.sheet_int_bonus
        && spi_bonus == e.sheet_spi_bonus
        && ap_base == e.sheet_ap_base
        && ap_mods == e.sheet_ap_mods
        && ap_modifiers == e.sheet_ap_modifiers
        && ranged_ap_modifiers == e.sheet_ranged_ap_modifiers
        && dmg_min == e.sheet_dmg_min
        && dmg_max == e.sheet_dmg_max
        && crit_bp == e.sheet_crit_bp
        && ranged_crit_bp == e.sheet_ranged_crit_bp
        && ranged_ap == e.sheet_ranged_ap
        && ranged_dmg_min == e.sheet_ranged_dmg_min
        && ranged_dmg_max == e.sheet_ranged_dmg_max
        && ranged_min_fraction == e.sheet_ranged_dmg_min_fraction
        && ranged_max_fraction == e.sheet_ranged_dmg_max_fraction
    {
        return;
    }
    e.sheet_str_bonus = str_bonus;
    e.sheet_agi_bonus = agi_bonus;
    e.sheet_sta_bonus = sta_bonus;
    e.sheet_int_bonus = int_bonus;
    e.sheet_spi_bonus = spi_bonus;
    e.sheet_ap_base = ap_base;
    e.sheet_ap_mods = ap_mods;
    e.sheet_ap_modifiers = ap_modifiers;
    e.sheet_ranged_ap_modifiers = ranged_ap_modifiers;
    e.sheet_dmg_min = dmg_min;
    e.sheet_dmg_max = dmg_max;
    e.sheet_crit_bp = crit_bp;
    e.sheet_ranged_crit_bp = ranged_crit_bp;
    e.sheet_ranged_ap = ranged_ap;
    e.sheet_ranged_dmg_min = ranged_dmg_min;
    e.sheet_ranged_dmg_max = ranged_dmg_max;
    e.sheet_ranged_dmg_min_fraction = ranged_min_fraction;
    e.sheet_ranged_dmg_max_fraction = ranged_max_fraction;
    e.update(ctx);
}

pub(crate) fn absorb_incoming(ctx: &ReducerContext, target_guid: u64, incoming: u32) -> u32 {
    if incoming == 0 {
        return 0;
    }
    let auras = ctx.db.game_aura();
    let mut shields: Vec<Aura> = auras
        .by_target()
        .filter(&target_guid)
        .filter(|a| a.eff_kind == A_ABSORB)
        .collect();
    if shields.is_empty() {
        return incoming;
    }
    shields.sort_by_key(|a| a.id);
    let (updated, deleted, remaining_damage) = drain_shields(shields, incoming);
    for shield in updated {
        shield.update(ctx);
    }
    let depleted = deleted
        .into_iter()
        .filter_map(|id| ctx.db.game_aura().id().find(id))
        .collect();
    shed_auras(ctx, depleted);
    remaining_damage
}
