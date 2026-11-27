use spacetimedb::ReducerContext;

use crate::game_world_entity;

use crate::game_melee_attack;

use super::formulas::{drain_mana_shields, ManaShield};
use super::{
    add_flat_pool_bonus, apply_spell_mod, bonuses, dispel_category_matches, drain_shields,
    rederive_pool, shed_auras, should_arm_spell_retaliation, A_ABSORB, COMBAT_ATTACK_POWER,
    DISPEL_MAGIC, DISPEL_POISON,
};
use crate::{game_aura, game_spell, Aura};
use lyracore_shared::packing::power_type;
use lyracore_shared::spell::taxonomy::SPELLMOD_OP_MULTIPLE_VALUE;

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
                    && (!matches!(spell.dispel_type, DISPEL_MAGIC | DISPEL_POISON)
                        || spell.negative() == friendly)
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
            stamina,
            crate::stats::hp_from_stamina,
        ),
        gear(crate::items::EquipStat::Health),
    );

    let is_mana = lyracore_shared::packing::power_type::for_class(class)
        == lyracore_shared::packing::power_type::MANA;
    let own = e.own_pool();
    let new_max_power = if is_mana {
        add_flat_pool_bonus(
            rederive_pool(
                crate::stats::max_power_for(ctx, race, class, e.level),
                e.intellect,
                intellect,
                crate::stats::mana_from_intellect,
            ),
            gear(crate::items::EquipStat::Mana),
        )
    } else {
        own.max
    };

    if new_max_health == e.max_health && new_max_power == own.max && new_spirit == e.spirit {
        return;
    }

    e.max_health = new_max_health;
    e.health = e.health.min(new_max_health);
    e.set_own_pool(own.current.min(new_max_power), new_max_power);
    e.spirit = new_spirit;
    e.update(ctx);
}

pub(crate) fn recompute_sheet(ctx: &ReducerContext, unit_guid: u64) {
    let entities = ctx.db.game_world_entity();
    let Some(mut e) = entities.guid().find(unit_guid) else {
        return;
    };
    if !e.is_player() {
        return;
    }
    let eff_str =
        crate::modifiers::effective_stat(ctx, &e, lyracore_shared::spell::taxonomy::STAT_STR);
    let eff_agi =
        crate::modifiers::effective_stat(ctx, &e, lyracore_shared::spell::taxonomy::STAT_AGI);
    let [str_bonus, agi_bonus, sta_bonus, int_bonus, spi_bonus] =
        crate::modifiers::SHEET_STATS.map(|stat| crate::modifiers::sheet_stat_bonus(ctx, &e, stat));

    let ap_base = crate::combat::base_melee_attack_power(ctx, &e, eff_str, eff_agi) as u32;
    let ap_mods = crate::modifiers::combat_field_bonus(ctx, unit_guid, COMBAT_ATTACK_POWER);
    let (dmg_min, dmg_max) = crate::combat::swing_range_ctx(ctx, &e);
    let ap_modifiers = crate::combat::attack_power_modifiers(ctx, unit_guid, COMBAT_ATTACK_POWER);
    // Priest, mage and warlock ignore ranged AP modifiers.
    let ranged_ap_modifiers = if crate::combat::is_wand_user(e.class()) {
        0
    } else {
        crate::combat::attack_power_modifiers(
            ctx,
            unit_guid,
            lyracore_shared::spell::taxonomy::COMBAT_RANGED_ATTACK_POWER,
        )
    };
    let (ranged_ap, ranged_dmg_min, ranged_dmg_max) = crate::combat::ranged_values(
        ctx,
        &e,
        crate::combat::equipped_ranged_weapon(ctx, unit_guid),
    );
    let ranged_ap = ranged_ap as u32;
    let ap_multiplier = crate::combat::melee_attack_power_multiplier(ctx, unit_guid);
    let ranged_ap_multiplier = crate::combat::ranged_attack_power_multiplier(ctx, &e);
    let ranged_dmg_min =
        bonuses::weapon_damage(ctx, &e, crate::items::equip_slot::RANGED, ranged_dmg_min);
    let ranged_dmg_max =
        bonuses::weapon_damage(ctx, &e, crate::items::equip_slot::RANGED, ranged_dmg_max);
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
        && ap_multiplier == e.sheet_ap_multiplier
        && ranged_ap_multiplier == e.sheet_ranged_ap_multiplier
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
    e.sheet_ap_multiplier = ap_multiplier;
    e.sheet_ranged_ap_multiplier = ranged_ap_multiplier;
    e.sheet_ranged_dmg_min = ranged_dmg_min;
    e.sheet_ranged_dmg_max = ranged_dmg_max;
    e.sheet_ranged_dmg_min_fraction = ranged_min_fraction;
    e.sheet_ranged_dmg_max_fraction = ranged_max_fraction;
    e.update(ctx);
}

/// Mana per point a Mana Shield absorbs, in hundredths, after the target's multiple-value mods.
fn mana_shield_cost_hundredths(ctx: &ReducerContext, shield: &Aura) -> u32 {
    let base = shield.eff_p1.max(0) as u32;
    let Some(hdr) = ctx.db.game_spell().spell_id().find(shield.spell_id) else {
        return base;
    };
    let (flat, pct) = super::spell_mod(ctx, shield.target_guid, SPELLMOD_OP_MULTIPLE_VALUE, &hdr);
    apply_spell_mod(base, flat.saturating_mul(100), pct)
}

/// vmangos `Unit::CalculateDamageAbsorbAndResist`: school absorbs whose mask covers `school_mask`
/// take damage first, then Mana Shields pay for it in mana. Returns the damage left.
pub(crate) fn absorb_incoming(
    ctx: &ReducerContext,
    target_guid: u64,
    incoming: u32,
    school_mask: u8,
) -> u32 {
    if incoming == 0 {
        return 0;
    }
    let mut shields: Vec<Aura> = ctx
        .db
        .game_aura()
        .by_target()
        .filter(&target_guid)
        .filter(|a| a.eff_kind == A_ABSORB && a.eff_p0 & i32::from(school_mask) != 0)
        .collect();
    if shields.is_empty() {
        return incoming;
    }
    shields.sort_by_key(|a| a.id);
    let (mana_shields, school_shields): (Vec<Aura>, Vec<Aura>) =
        shields.into_iter().partition(|a| a.eff_p1 != 0);
    let (mut updated, mut deleted, remaining_damage) = drain_shields(school_shields, incoming);
    let mana_shields = mana_shields
        .into_iter()
        .map(|aura| ManaShield {
            cost_hundredths: mana_shield_cost_hundredths(ctx, &aura),
            aura,
        })
        .collect();
    let target = ctx.db.game_world_entity().guid().find(target_guid);
    let mana = target
        .as_ref()
        .map_or(0, |target| target.spendable_power(power_type::MANA));
    let drain = drain_mana_shields(mana_shields, remaining_damage, mana);
    if let Some(mut target) = target.filter(|_| drain.mana_spent > 0) {
        target.spend_power(power_type::MANA, drain.mana_spent);
        target.update(ctx);
    }
    updated.extend(drain.updated);
    deleted.extend(drain.deleted);
    for shield in updated {
        shield.update(ctx);
    }
    let depleted = deleted
        .into_iter()
        .filter_map(|id| ctx.db.game_aura().id().find(id))
        .collect();
    shed_auras(ctx, depleted);
    drain.remaining_damage
}
