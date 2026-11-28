use spacetimedb::ReducerContext;

use crate::{
    game_creature_template, game_item_instance, game_item_template, game_spell, game_spell_effect,
    game_world_entity, ItemInstance, WorldEntity,
};

use super::queued_strike::{QueuedStrike, QueuedStrikeDamage};
use super::tables::scale_swing_for_attack_power_multiplier;
use super::*;
use crate::form::CombatProfile;
use crate::spell::SCHOOL_MASK_PHYSICAL;

pub(crate) fn attack_power_modifiers(ctx: &ReducerContext, guid: u64, field: u8) -> u32 {
    let (positive, negative) = crate::spell::auras_on(ctx, guid)
        .filter(|aura| {
            aura.eff_kind == crate::spell::A_MOD_COMBAT && aura.eff_p0 == i32::from(field)
        })
        .map(|aura| crate::modifiers::stacked_amount(aura.amount, aura.stacks))
        .fold((0i32, 0i32), |(positive, negative), amount| {
            (
                positive.saturating_add(amount.max(0)),
                negative.saturating_add(amount.min(0)),
            )
        });
    u32::from(positive as u16) | (u32::from(negative as u16) << 16)
}

pub(crate) fn seal_holy_on_swing(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: u64,
) -> (u32, u32) {
    let (amount, spell) = crate::spell::seal_amount_and_spell(ctx, attacker.guid);
    if let Some(header) = ctx.db.game_spell().spell_id().find(spell).filter(|header| {
        header.cast_flags & lyracore_shared::spell::taxonomy::CAST_FLAG_RIGHTEOUSNESS != 0
    }) {
        let Some(effect) = ctx
            .db
            .game_spell_effect()
            .by_spell()
            .filter(spell)
            .find(|effect| effect.kind == crate::spell::A_SEAL && effect.effect_index == 0)
        else {
            return (0, 0);
        };
        let speed =
            crate::items::item_in_slot(ctx, attacker.guid, crate::items::equip_slot::MAINHAND)
                .and_then(|item| ctx.db.game_item_template().entry().find(item.entry))
                .map_or(2000, |weapon| weapon.delay_ms) as f32
                / 1000.0;
        let min = amount as f32 / 87.0;
        let max = amount as f32 / 25.0;
        let mut base = (max - min) * ((speed - 1.5) / 2.5) + min;
        for aura in crate::spell::auras_on(ctx, attacker.guid)
            .filter(|aura| [20224, 20225, 20330, 20331, 20332].contains(&aura.spell_id))
        {
            if aura.eff_kind == lyracore_shared::spell::taxonomy::A_SPELLMOD_PCT
                && aura.eff_p0 == 8
                && aura.amount > 0
            {
                base *= (100 + crate::modifiers::stacked_amount(aura.amount, aura.stacks)) as f32
                    / 100.0;
            }
        }
        let done = crate::spell::bonuses::spell_damage_done_float(
            ctx,
            &header,
            &effect,
            attacker.guid,
            base,
            false,
        );
        let taken = crate::spell::bonuses::spell_damage_taken(
            ctx,
            &header,
            &effect,
            attacker.guid,
            target,
            done,
            1,
        );
        let damage = crate::spell::bonuses::round_damage(ctx, target, taken).max(0) as u32;
        let triggered = match spell {
            20154 | 21084 => 25742,
            20287 => 25740,
            20288 => 25739,
            20289 => 25738,
            20290 => 25737,
            20291 => 25736,
            20292 => 25735,
            20293 => 25713,
            _ => return (0, 0),
        };
        return (damage, triggered);
    }
    (
        seal_swing_holy(amount, main_hand_attack_time(ctx, attacker)),
        spell,
    )
}

fn seal_swing_holy(seal: i32, attack_time_ms: u32) -> u32 {
    if seal <= 0 {
        return 0;
    }
    ((seal as u64 * attack_time_ms as u64) / 100_000) as u32
}

pub(crate) const REACT_WINDOW_MS: u64 = 5000;

#[derive(Clone, Copy)]
pub(crate) enum ReactKind {
    Overpower,
    Revenge,
}

pub(crate) fn react_window_active(until_ms: u64, now_ms: u64) -> bool {
    until_ms > now_ms
}

pub(crate) fn arm_react(ctx: &ReducerContext, guid: u64, kind: ReactKind) {
    let entities = ctx.db.game_world_entity();
    let now_ms = crate::time::now_ms(ctx);
    if let Some(mut e) = entities.guid().find(guid) {
        let until = now_ms + REACT_WINDOW_MS;
        match kind {
            ReactKind::Overpower => e.overpower_until_ms = until,
            ReactKind::Revenge => e.revenge_until_ms = until,
        }
        e.update(ctx);
    }
}

fn effective_block_value(ctx: &ReducerContext, defender: &WorldEntity) -> u32 {
    let Some(inst) =
        crate::items::item_in_slot(ctx, defender.guid, crate::items::equip_slot::OFFHAND)
    else {
        return 0;
    };
    let Some(tmpl) = ctx.db.game_item_template().entry().find(inst.entry) else {
        return 0;
    };
    if tmpl.inventory_type != crate::items::invtype::SHIELD
        || crate::items::item_is_broken(&tmpl, &inst)
    {
        return 0;
    }
    let from_strength =
        crate::modifiers::effective_stat(ctx, defender, crate::spell::STAT_STR) / 20;
    (tmpl.block_value.max(0) as u32).saturating_add(from_strength)
}

pub(crate) fn effective_crit_bp(ctx: &ReducerContext, attacker: &WorldEntity) -> u32 {
    crit_bp_for_weapon_skill(
        ctx,
        attacker,
        crate::skill::effective_weapon_skill(ctx, attacker),
    )
}

pub(crate) fn effective_ranged_crit_bp(ctx: &ReducerContext, attacker: &WorldEntity) -> u32 {
    crit_bp_for_weapon_skill(
        ctx,
        attacker,
        crate::skill::effective_ranged_weapon_skill(ctx, attacker),
    )
}

fn crit_bp_for_weapon_skill(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    weapon_skill: u32,
) -> u32 {
    let bonus = crate::modifiers::combat_field_bonus(ctx, attacker.guid, crate::spell::COMBAT_CRIT)
        + crate::items::equipped_stat_bonus(ctx, attacker.guid, crate::items::EquipStat::Crit);
    if !attacker.is_player() {
        return (500 + bonus).clamp(0, 10_000) as u32;
    }
    let rate = crate::stats::crit_per_agility_for(ctx, attacker.class(), attacker.level);
    super::tables::sheet_crit_bp(
        crate::modifiers::effective_stat(ctx, attacker, crate::spell::STAT_AGI),
        attacker.class(),
        attacker.level,
        rate,
        weapon_skill as i32,
        crate::skill::skill_cap_for_level(attacker.level) as i32,
        bonus,
    )
}

pub(crate) fn effective_swing_time(ctx: &ReducerContext, attacker: &WorldEntity) -> u32 {
    let speed = crate::modifiers::speed_bonus(ctx, attacker.guid, crate::spell::SPEED_SWING);
    swing_time_with_haste(main_hand_attack_time(ctx, attacker), speed)
}

/// The unhasted attack time of the hand or weapon that made `source`; vmangos Unit::GetAttackTime.
pub(crate) fn attack_time(ctx: &ReducerContext, unit: &WorldEntity, source: HitSource) -> u32 {
    match source {
        HitSource::OffHand => off_hand_attack_time(ctx, unit),
        HitSource::Ranged => ranged_attack_time(ctx, unit),
        _ => main_hand_attack_time(ctx, unit),
    }
}

/// A Character's main hand swings at its usable weapon's delay; `base_attack_time_ms` is unarmed.
pub(crate) fn main_hand_attack_time(ctx: &ReducerContext, unit: &WorldEntity) -> u32 {
    let profile = player_profile(ctx, unit);
    if let Some(form) = natural_attack_time(profile) {
        return form;
    }
    let holds_weapon = unit.is_player()
        && crate::spell::chance_on_hit::holds_weapon(
            profile,
            lyracore_shared::constants::starter_item::MAINHAND_SLOT,
            || crate::spell::is_disarmed(ctx, unit.guid),
        );
    holds_weapon
        .then(|| equipped_weapon_damage(ctx, unit.guid))
        .flatten()
        .map(|(_, _, delay)| delay)
        .filter(|delay| *delay != 0)
        .unwrap_or(unit.base_attack_time_ms)
}

pub(crate) fn off_hand_attack_time(ctx: &ReducerContext, unit: &WorldEntity) -> u32 {
    if let Some(form) = natural_attack_time(player_profile(ctx, unit)) {
        return form;
    }
    if !unit.is_player() {
        return unit.base_attack_time_ms;
    }
    equipped_offhand_weapon_damage(ctx, unit.guid)
        .map(|(_, _, delay)| delay)
        .filter(|delay| *delay != 0)
        .unwrap_or(DEFAULT_ATTACK_TIME_MS)
}

fn ranged_attack_time(ctx: &ReducerContext, unit: &WorldEntity) -> u32 {
    if !unit.is_player() {
        return unit.base_attack_time_ms;
    }
    equipped_ranged_weapon(ctx, unit.guid)
        .map(|(_, _, delay, _)| delay)
        .filter(|delay| *delay != 0)
        .unwrap_or(DEFAULT_ATTACK_TIME_MS)
}

/// A Character's Combat Profile; a creature keeps the weapon profile and its template's swing.
fn player_profile(ctx: &ReducerContext, unit: &WorldEntity) -> CombatProfile {
    if unit.is_player() {
        crate::form::combat_profile(ctx, unit.guid)
    } else {
        CombatProfile::Weapon
    }
}

/// vmangos Player::InitDataForForm swings both hands of a Character's natural attacks at the
/// Form's speed.
fn natural_attack_time(profile: CombatProfile) -> Option<u32> {
    match profile {
        CombatProfile::Natural { attack_ms, .. } => Some(attack_ms),
        CombatProfile::Weapon => None,
    }
}

fn weapon_profile_in_slot(
    ctx: &ReducerContext,
    player_guid: u64,
    slot: u8,
) -> Option<(u32, u32, u32, u8)> {
    let inst = crate::items::item_in_slot(ctx, player_guid, slot)?;
    let tmpl = ctx.db.game_item_template().entry().find(inst.entry)?;
    if tmpl.class != lyracore_shared::constants::starter_item::CLASS_WEAPON {
        return None;
    }
    if crate::items::item_is_broken(&tmpl, &inst) {
        return None;
    }
    let bonus =
        crate::items::item_stat_bonus(ctx, &inst, &tmpl, crate::items::EquipStat::WeaponDamage);
    Some((
        add_weapon_damage(tmpl.damage_min, bonus),
        add_weapon_damage(tmpl.damage_max, bonus),
        tmpl.delay_ms,
        tmpl.subclass,
    ))
}

fn add_weapon_damage(base: f32, bonus: i32) -> u32 {
    (base.round() as i64 + i64::from(bonus)).clamp(0, i64::from(u32::MAX)) as u32
}

fn equipped_weapon_damage(ctx: &ReducerContext, player_guid: u64) -> Option<(u32, u32, u32)> {
    weapon_profile_in_slot(
        ctx,
        player_guid,
        lyracore_shared::constants::starter_item::MAINHAND_SLOT,
    )
    .map(|(dmin, dmax, delay, _)| (dmin, dmax, delay))
}

pub(crate) fn equipped_offhand_weapon_damage(
    ctx: &ReducerContext,
    player_guid: u64,
) -> Option<(u32, u32, u32)> {
    weapon_profile_in_slot(ctx, player_guid, crate::items::equip_slot::OFFHAND)
        .map(|(dmin, dmax, delay, _)| (dmin, dmax, delay))
}

pub(crate) fn equipped_ranged_weapon(
    ctx: &ReducerContext,
    player_guid: u64,
) -> Option<(u32, u32, u32, u8)> {
    weapon_profile_in_slot(ctx, player_guid, crate::items::equip_slot::RANGED)
}

// Skips an offered stack; see the offered_item_stays_in_the_trade Deliberate Deviation.
pub(crate) fn find_ammo(ctx: &ReducerContext, player_guid: u64) -> Option<ItemInstance> {
    let templates = ctx.db.game_item_template();
    let offered = crate::trade::offered_items(ctx, player_guid);
    lyracore_shared::item::loaded_ammo(
        ctx.db
            .game_item_instance()
            .by_owner_guid()
            .filter(&player_guid)
            .filter(|item| !offered.contains(&item.guid)),
        |item| {
            (
                item.slot,
                item.stack_count,
                templates.entry().find(item.entry).map(|tmpl| tmpl.class),
            )
        },
    )
}

pub(crate) fn swing_range_ctx(ctx: &ReducerContext, attacker: &WorldEntity) -> (u32, u32) {
    melee_range(ctx, attacker, false)
}

fn melee_range(ctx: &ReducerContext, attacker: &WorldEntity, normalized: bool) -> (u32, u32) {
    let (min, max) = base_melee_range(ctx, attacker, normalized);
    let apply = |base| {
        crate::spell::bonuses::weapon_damage(
            ctx,
            attacker,
            crate::items::equip_slot::MAINHAND,
            base as f32,
        ) as u32
    };
    (apply(min), apply(max))
}

fn base_melee_range(ctx: &ReducerContext, attacker: &WorldEntity, normalized: bool) -> (u32, u32) {
    let disarmed = crate::spell::is_disarmed(ctx, attacker.guid);
    if !attacker.is_player() {
        let tmpl = ctx.db.game_creature_template().entry().find(attacker.entry);
        let (base_min, base_max) = match &tmpl {
            Some(t) if t.damage_max > 0 => (t.damage_min, t.damage_max),
            _ => (CREATURE_MELEE_MIN, CREATURE_MELEE_MAX),
        };
        let (base_min, base_max) =
            match (&tmpl, crate::creatures::live_pet_kind(ctx, attacker.guid)) {
                (Some(t), crate::creatures::PetKind::Hunter { .. }) => {
                    crate::creatures::scale_creature_damage_for_level(
                        base_min,
                        base_max,
                        t.level,
                        attacker.level,
                    )
                }
                _ => (base_min, base_max),
            };
        let stats = tmpl
            .as_ref()
            .and_then(|t| crate::creatures::catalogue::level_stats(ctx, t, attacker.level));
        let derived = stats.is_some()
            || tmpl.as_ref().is_some_and(|t| {
                crate::creatures::catalogue::level_stats(ctx, t, t.level).is_some()
            });
        let (base_min, base_max) = stats.map_or((base_min, base_max), |stats| {
            (stats.damage_min, stats.damage_max)
        });
        let rank = tmpl.filter(|_| !derived).map(|t| t.rank).unwrap_or(0);
        let (min, max) = scale_swing_for_rank(base_min, base_max, rank);
        let (min, max) = scale_swing_for_attack_power_multiplier(
            (min, max),
            melee_attack_power_multiplier(ctx, attacker.guid),
        );
        if disarmed {
            let pct = crate::spell::DISARM_SWING_RETAINED_PCT;
            return (min * pct / 100, max * pct / 100);
        }
        return (min, max);
    }
    let profile = crate::form::combat_profile(ctx, attacker.guid);
    let ap = melee_attack_power_in(ctx, attacker, profile);
    if let Some(form_ms) = natural_attack_time(profile) {
        // vmangos normalizes by the equipped weapon; no 1.12 druid form ability normalizes.
        let speed_ms = if normalized {
            NORMALIZED_ONE_HAND_MS
        } else {
            form_ms
        };
        return natural_swing_range_ap(ap, attacker.level, speed_ms);
    }
    let mainhand = lyracore_shared::constants::starter_item::MAINHAND_SLOT;
    let speed = |delay| {
        if normalized {
            normalized_speed_ms(ctx, attacker.guid, mainhand)
        } else {
            delay
        }
    };
    if !disarmed {
        if let Some((dmin, dmax, delay)) = equipped_weapon_damage(ctx, attacker.guid) {
            return weapon_swing_range_ap(ap, dmin, dmax, speed(delay));
        }
    }
    player_swing_range_ap(ap, speed(attacker.base_attack_time_ms))
}

const NORMALIZED_ONE_HAND_MS: u32 = 2400;
const NORMALIZED_TWO_HAND_MS: u32 = 3300;
const NORMALIZED_DAGGER_MS: u32 = 1700;
const NORMALIZED_RANGED_MS: u32 = 2800;

// vmangos GetAPMultiplier: normalized damage adds attack power at a fixed speed per weapon type.
fn normalized_speed_ms(ctx: &ReducerContext, player_guid: u64, slot: u8) -> u32 {
    normalized_speed_for(
        crate::items::item_in_slot(ctx, player_guid, slot)
            .and_then(|item| ctx.db.game_item_template().entry().find(item.entry))
            .map(|tmpl| (tmpl.inventory_type, tmpl.subclass)),
    )
}

fn normalized_speed_for(weapon: Option<(u8, u8)>) -> u32 {
    use crate::items::invtype as t;
    match weapon {
        Some((t::TWO_HAND_WEAPON, _)) => NORMALIZED_TWO_HAND_MS,
        Some((t::RANGED | t::RANGED_RIGHT | t::THROWN, _)) => NORMALIZED_RANGED_MS,
        Some((_, crate::items::weapon_subclass::DAGGER)) => NORMALIZED_DAGGER_MS,
        _ => NORMALIZED_ONE_HAND_MS,
    }
}

/// One weapon spell's damage terms; `percent` scales the weapon roll and `bonus` together.
#[derive(Clone, Copy)]
pub(crate) struct WeaponStrike {
    pub(crate) bonus: i32,
    pub(crate) percent: u32,
    pub(crate) normalized: bool,
    pub(crate) ranged: bool,
}

pub(crate) fn weapon_strike_spell_damage(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    strike: WeaponStrike,
    spell: &crate::Spell,
    crit: bool,
) -> u32 {
    let (min, max) = strike_weapon_range(ctx, attacker, strike);
    let weapon = min + ctx.random::<u32>() % (max.saturating_sub(min) + 1);
    let mods = crate::spell::spell_mod(ctx, attacker.guid, crate::spell::SPELLMOD_OP_DAMAGE, spell);
    let mit_pct = if spell.school_mask == SCHOOL_MASK_PHYSICAL
        && spell.custom_flags & lyracore_shared::spell_custom::IGNORE_ARMOR == 0
    {
        armor_mitigation_pct(
            crate::modifiers::effective_armor(ctx, target),
            attacker.level,
        )
    } else {
        0
    };
    let slot = if strike.ranged {
        crate::items::equip_slot::RANGED
    } else {
        crate::items::equip_slot::MAINHAND
    };
    let bonus =
        crate::spell::bonuses::weapon_strike_bonus(ctx, attacker, slot, strike.bonus as f32);
    let base = strike_amount(weapon, bonus as i32, strike.percent, mods, false, 0);
    let base = crate::spell::bonuses::weapon_spell_damage_taken(ctx, spell, target.guid, base);
    armor_mitigated(if crit { base * CRIT_MULT } else { base }, mit_pct)
}

fn predatory_strikes_pct(ctx: &ReducerContext, unit: &WorldEntity, profile: CombatProfile) -> u32 {
    if unit.class() != CLASS_DRUID || !profile.is_natural() {
        return 0;
    }
    crate::spell::auras_on(ctx, unit.guid)
        .find(|aura| crate::modifiers::is_predatory_strikes(aura.spell_id, aura.eff_kind))
        .map_or(0, |aura| aura.amount.max(0) as u32)
}

/// Base AP before modifiers from the given effective Strength and Agility; may be negative.
pub(crate) fn base_melee_attack_power(
    ctx: &ReducerContext,
    unit: &WorldEntity,
    strength: u32,
    agility: u32,
) -> i32 {
    base_melee_attack_power_in(
        ctx,
        unit,
        crate::form::combat_profile(ctx, unit.guid),
        strength,
        agility,
    )
}

fn base_melee_attack_power_in(
    ctx: &ReducerContext,
    unit: &WorldEntity,
    profile: CombatProfile,
    strength: u32,
    agility: u32,
) -> i32 {
    base_melee_attack_power_for(
        unit.class(),
        profile,
        strength,
        agility,
        unit.level,
        predatory_strikes_pct(ctx, unit, profile),
    )
}

pub(crate) fn melee_attack_power(ctx: &ReducerContext, unit: &WorldEntity) -> f32 {
    melee_attack_power_in(ctx, unit, crate::form::combat_profile(ctx, unit.guid))
}

fn melee_attack_power_in(ctx: &ReducerContext, unit: &WorldEntity, profile: CombatProfile) -> f32 {
    total_attack_power(
        base_melee_attack_power_in(
            ctx,
            unit,
            profile,
            crate::modifiers::effective_stat(ctx, unit, crate::spell::STAT_STR),
            crate::modifiers::effective_stat(ctx, unit, crate::spell::STAT_AGI),
        ),
        crate::modifiers::combat_field_bonus(ctx, unit.guid, crate::spell::COMBAT_ATTACK_POWER),
        melee_attack_power_multiplier(ctx, unit.guid),
    )
}

/// The wire value of UNIT_FIELD_ATTACK_POWER_MULTIPLIER: the percent Auras' product minus 1.
pub(crate) fn melee_attack_power_multiplier(ctx: &ReducerContext, unit_guid: u64) -> f32 {
    crate::modifiers::attack_power_multiplier(
        ctx,
        unit_guid,
        lyracore_shared::spell::taxonomy::COMBAT_ATTACK_POWER_PCT,
    )
}

/// The wire value of UNIT_FIELD_RANGED_ATTACK_POWER_MULTIPLIER; wand users ignore it.
pub(crate) fn ranged_attack_power_multiplier(ctx: &ReducerContext, unit: &WorldEntity) -> f32 {
    if is_wand_user(unit.class()) {
        return 0.0;
    }
    crate::modifiers::attack_power_multiplier(
        ctx,
        unit.guid,
        lyracore_shared::spell::taxonomy::COMBAT_RANGED_ATTACK_POWER_PCT,
    )
}

/// The flat ranged attack power Auras add; wand users ignore them.
fn ranged_attack_power_bonus(ctx: &ReducerContext, unit: &WorldEntity) -> i32 {
    if is_wand_user(unit.class()) {
        return 0;
    }
    crate::modifiers::combat_field_bonus(
        ctx,
        unit.guid,
        lyracore_shared::spell::taxonomy::COMBAT_RANGED_ATTACK_POWER,
    )
}

/// `ranged_sheet_values` from the unit's stats and Auras; `weapon` may carry a normalized delay.
pub(crate) fn ranged_values(
    ctx: &ReducerContext,
    unit: &WorldEntity,
    weapon: Option<(u32, u32, u32, u8)>,
) -> (i32, f32, f32) {
    ranged_sheet_values(
        unit.class(),
        crate::form::combat_profile(ctx, unit.guid),
        crate::modifiers::effective_stat(ctx, unit, crate::spell::STAT_AGI),
        unit.level,
        weapon,
        ranged_attack_power_bonus(ctx, unit),
        ranged_attack_power_multiplier(ctx, unit),
    )
}

fn strike_weapon_range(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    strike: WeaponStrike,
) -> (u32, u32) {
    if !attacker.is_player() {
        return crate::creatures::scale_hunter_pet_swing(
            ctx,
            attacker.guid,
            swing_range_ctx(ctx, attacker),
        );
    }
    if !strike.ranged {
        return melee_range(ctx, attacker, strike.normalized);
    }
    let Some((dmin, dmax, delay, subclass)) = equipped_ranged_weapon(ctx, attacker.guid) else {
        return (0, 0);
    };
    let delay = if strike.normalized {
        normalized_speed_ms(ctx, attacker.guid, crate::items::equip_slot::RANGED)
    } else {
        delay
    };
    let (_, min, max) = ranged_values(ctx, attacker, Some((dmin, dmax, delay, subclass)));
    (
        crate::spell::bonuses::weapon_damage(ctx, attacker, crate::items::equip_slot::RANGED, min)
            as u32,
        crate::spell::bonuses::weapon_damage(ctx, attacker, crate::items::equip_slot::RANGED, max)
            as u32,
    )
}

fn strike_amount(
    weapon: u32,
    bonus: i32,
    percent: u32,
    (mod_flat, mod_pct): (i32, i32),
    crit: bool,
    mit_pct: u32,
) -> u32 {
    let scaled = (i64::from(weapon) + i64::from(bonus)).max(0) * i64::from(percent) / 100;
    let base =
        crate::spell::apply_spell_mod(scaled.min(i64::from(u32::MAX)) as u32, mod_flat, mod_pct);
    let pre_mit = u64::from(base) * if crit { u64::from(CRIT_MULT) } else { 1 };
    (pre_mit * u64::from(100 - mit_pct.min(100)) / 100).min(u64::from(u32::MAX)) as u32
}

// vmangos TakeAmmo: a thrown stack loses one; an unstackable thrown weapon loses durability.
pub(crate) fn spend_ranged_ammo(ctx: &ReducerContext, player_guid: u64) {
    let Some(mut weapon) =
        crate::items::item_in_slot(ctx, player_guid, crate::items::equip_slot::RANGED)
    else {
        return;
    };
    let Some(tmpl) = ctx.db.game_item_template().entry().find(weapon.entry) else {
        return;
    };
    if tmpl.inventory_type == crate::items::invtype::THROWN {
        if tmpl.max_stack > 1 {
            spend_one(ctx, weapon);
        } else if weapon.durability > 0 {
            weapon.durability -= 1;
            weapon.update(ctx);
        }
    } else if tmpl.subclass != crate::items::weapon_subclass::WAND {
        if let Some(ammo) = find_ammo(ctx, player_guid) {
            spend_one(ctx, ammo);
        }
    }
}

pub(crate) fn spend_one(ctx: &ReducerContext, mut item: ItemInstance) {
    item.stack_count = item.stack_count.saturating_sub(1);
    if item.stack_count == 0 {
        crate::ItemInstance::delete(ctx, item.guid);
    } else {
        item.update(ctx);
    }
}

// vmangos WriteAmmoToPacket: a thrown weapon flies as itself; a launcher shows its loaded ammo.
pub(crate) fn ranged_projectile(ctx: &ReducerContext, player_guid: u64) -> Option<(u32, u32)> {
    let templates = ctx.db.game_item_template();
    let weapon = crate::items::item_in_slot(ctx, player_guid, crate::items::equip_slot::RANGED)?;
    let tmpl = templates.entry().find(weapon.entry)?;
    if tmpl.inventory_type == crate::items::invtype::THROWN {
        return Some((tmpl.display_id, u32::from(tmpl.inventory_type)));
    }
    if tmpl.subclass == crate::items::weapon_subclass::WAND {
        return None;
    }
    let ammo = templates.entry().find(find_ammo(ctx, player_guid)?.entry)?;
    Some((ammo.display_id, u32::from(ammo.inventory_type)))
}

// `strike` is a firing queued strike; `None` rolls a white swing.
pub(crate) fn roll_swing(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    strike: Option<QueuedStrike>,
) -> (u32, AttackOutcome, u32) {
    let range = crate::creatures::scale_hunter_pet_swing(
        ctx,
        attacker.guid,
        swing_range_ctx(ctx, attacker),
    );
    roll_swing_with_range(ctx, attacker, target, range, strike)
}

pub(crate) fn roll_swing_with_range(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    range: (u32, u32),
    strike: Option<QueuedStrike>,
) -> (u32, AttackOutcome, u32) {
    // vmangos RollMeleeOutcomeAgainst and SpellHitResult answer evade before any other outcome.
    if crate::creatures::eventai::movement::returning_home(ctx, target.guid) {
        return (0, AttackOutcome::Evade, 0);
    }
    // vmangos refuses a white swing in CalculateMeleeDamage and a queued strike in SpellHitResult.
    if crate::spell::immune_to_school(ctx, target, SCHOOL_MASK_PHYSICAL) {
        return (0, AttackOutcome::Immune, 0);
    }
    let (outcome, table) = roll_attack(
        ctx,
        attacker,
        target,
        AttackType::Melee,
        strike.map(|strike| strike.spell),
    );
    let stream = crate::conformance_rolls::RollStream::Swing(attacker.guid);
    let rolls = SwingRolls {
        weapon: crate::conformance_rolls::draw(ctx, stream),
        block: crate::conformance_rolls::draw(ctx, stream) % 10_000,
    };
    let defender = Defender {
        taken: crate::spell::bonuses::WeaponDamageTaken::of(ctx, target.guid),
        armor_pct: armor_mitigation_pct(
            crate::modifiers::effective_armor(ctx, target),
            attacker.level,
        ),
        block_value: defender_block_value(ctx, target),
    };
    resolve_swing(
        outcome,
        table.partial_block,
        rolls,
        range,
        strike.map(|strike| strike.damage),
        defender,
    )
}

pub(crate) fn defender_block_value(ctx: &ReducerContext, target: &WorldEntity) -> u32 {
    if target.is_player() {
        effective_block_value(ctx, target)
    } else {
        target.level / 2
            + crate::modifiers::effective_stat(ctx, target, crate::spell::STAT_STR) / 20
    }
}

struct SwingRolls {
    weapon: u32,
    block: u32,
}

struct Defender {
    taken: crate::spell::bonuses::WeaponDamageTaken,
    armor_pct: u32,
    block_value: u32,
}

// Returns (damage, outcome, blocked).
fn resolve_swing(
    outcome: AttackOutcome,
    partial_block_bp: u32,
    rolls: SwingRolls,
    (min, max): (u32, u32),
    strike: Option<QueuedStrikeDamage>,
    defender: Defender,
) -> (u32, AttackOutcome, u32) {
    let weapon = min + rolls.weapon % (max - min + 1);
    let base = strike.map_or(weapon, |strike| {
        crate::spell::apply_spell_mod(
            weapon.saturating_add(strike.bonus),
            strike.mod_flat,
            strike.mod_pct,
        )
    });
    let base = defender.taken.apply(base);
    let pre_mit = match outcome {
        AttackOutcome::Miss
        | AttackOutcome::Dodge
        | AttackOutcome::Parry
        | AttackOutcome::Immune
        | AttackOutcome::Evade => return (0, outcome, 0),
        AttackOutcome::Crit => base * CRIT_MULT,
        AttackOutcome::Glancing if base > 0 => (base * GLANCING_DAMAGE_PCT / 100).max(1),
        AttackOutcome::Glancing => 0,
        AttackOutcome::Crushing => base * CRUSH_DAMAGE_PCT / 100,
        AttackOutcome::Normal | AttackOutcome::Block => base,
    };
    let dmg = armor_mitigated(pre_mit, defender.armor_pct);
    // vmangos rolls a queued strike's partial block after the table; it keeps its table outcome.
    let blocks = match strike {
        Some(_) => rolls.block < partial_block_bp,
        None => outcome == AttackOutcome::Block,
    };
    if !blocks {
        return (dmg, outcome, 0);
    }
    let blocked = defender.block_value.min(dmg);
    (dmg - blocked, outcome, blocked)
}

pub(crate) fn ranged_school(ctx: &ReducerContext, guid: u64) -> u8 {
    crate::items::item_in_slot(ctx, guid, crate::items::equip_slot::RANGED)
        .and_then(|item| ctx.db.game_item_template().entry().find(item.entry))
        .map_or(0, |weapon| weapon.damage_school.min(6))
}

pub(crate) struct RangedRoll {
    pub(crate) damage: u32,
    pub(crate) outcome: AttackOutcome,
    pub(crate) resisted: u32,
    pub(crate) blocked: u32,
}

impl RangedRoll {
    fn avoided(outcome: AttackOutcome) -> Self {
        Self {
            damage: 0,
            outcome,
            resisted: 0,
            blocked: 0,
        }
    }
}

pub(super) const SPELL_ATTR_NO_ACTIVE_DEFENSE: u32 = 0x0020_0000;
pub(super) const SPELL_ATTR_EX3_COMPLETELY_BLOCKED: u32 = 0x0000_0008;

// Only a melee or ranged damage-class spell blocks, not a wand's Shoot, nor one
// `IsSpellPartiallyBlocked` excludes by attribute.
fn rolls_spell_block(
    ctx: &ReducerContext,
    stream: crate::conformance_rolls::RollStream,
    spell_id: u32,
    partial_block_bp: u32,
) -> bool {
    partial_block_bp > 0
        && ctx
            .db
            .game_spell()
            .spell_id()
            .find(spell_id)
            .is_some_and(|spell| {
                matches!(
                    spell.damage_class,
                    crate::spell::proc::DAMAGE_CLASS_MELEE
                        | crate::spell::proc::DAMAGE_CLASS_RANGED
                ) && spell.attributes & SPELL_ATTR_NO_ACTIVE_DEFENSE == 0
                    && spell.attributes_ex_3 & SPELL_ATTR_EX3_COMPLETELY_BLOCKED == 0
            })
        && crate::conformance_rolls::draw(ctx, stream) % 10_000 < partial_block_bp
}

/// A cast Spell's partial block: its damage after the block and the amount blocked.
/// The draw comes from the Caster stream, after the cast's hit roll and the effect's other draws.
pub(crate) fn block_cast_spell(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    spell: &crate::Spell,
    attack_type: AttackType,
    rolled: u32,
) -> (u32, u32) {
    if !matches!(
        spell.damage_class,
        crate::spell::proc::DAMAGE_CLASS_MELEE | crate::spell::proc::DAMAGE_CLASS_RANGED
    ) {
        return (rolled, 0);
    }
    let table = attack_table_for(
        ctx,
        attacker,
        target,
        attack_type,
        Some(DeliveredSpell::cast(spell)),
    );
    let stream = crate::conformance_rolls::RollStream::Caster(attacker.guid);
    if !rolls_spell_block(ctx, stream, spell.spell_id, table.partial_block) {
        return (rolled, 0);
    }
    let blocked = defender_block_value(ctx, target).min(rolled);
    (rolled - blocked, blocked)
}

/// Deals a cast Spell's damage after its partial block; returns the damage dealt and absorbed.
/// A block arms the target's Revenge, and a full block still fires an untriggered hit's procs.
pub(crate) fn deal_blocked_spell_damage(
    ctx: &ReducerContext,
    caster_guid: u64,
    target_guid: u64,
    damage: i32,
    blocked: u32,
    school_mask: u8,
    hit: Hit,
) -> (u32, u32) {
    let hit = hit.with_block(blocked);
    let dealt = deal_damage(ctx, caster_guid, target_guid, damage, school_mask, hit);
    if blocked == 0 {
        return dealt;
    }
    arm_react(ctx, target_guid, ReactKind::Revenge);
    if damage <= 0 && hit.source != HitSource::Triggered {
        crate::spell::proc::run_unlanded_proc_pass(
            ctx,
            caster_guid,
            target_guid,
            &hit,
            AttackOutcome::Normal,
            (0, 0),
        );
    }
    dealt
}

pub(crate) fn roll_ranged_swing(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    ranged_spell_id: u32,
    (dmin, dmax): (u32, u32),
) -> RangedRoll {
    let school = ranged_school(ctx, attacker.guid);
    let spell_school_mask = ctx
        .db
        .game_spell()
        .spell_id()
        .find(ranged_spell_id)
        .map_or(SCHOOL_MASK_PHYSICAL, |spell| spell.school_mask);
    if crate::creatures::eventai::movement::returning_home(ctx, target.guid) {
        return RangedRoll::avoided(AttackOutcome::Evade);
    }
    // vmangos SpellHitResult: IsImmuneToSpell takes Shoot's school, IsImmuneToDamage the wand's.
    if crate::spell::immune_to_school(ctx, target, spell_school_mask | (1 << school)) {
        return RangedRoll::avoided(AttackOutcome::Immune);
    }
    let (outcome, table) = roll_attack(ctx, attacker, target, AttackType::Ranged, None);
    let weapon = crate::conformance_rolls::draw(
        ctx,
        crate::conformance_rolls::RollStream::Swing(attacker.guid),
    );
    let base = dmin + weapon % (dmax.saturating_sub(dmin) + 1);
    let base = if school == 0 {
        crate::spell::bonuses::weapon_damage_taken(ctx, target.guid, base)
    } else {
        (base as f32
            * crate::spell::bonuses::damage_taken_multiplier(ctx, target.guid, 1 << school))
            as u32
    };
    let pre_mit = match outcome {
        AttackOutcome::Miss
        | AttackOutcome::Dodge
        | AttackOutcome::Parry
        | AttackOutcome::Immune
        | AttackOutcome::Evade => return RangedRoll::avoided(outcome),
        AttackOutcome::Crit if school == 0 => base * CRIT_MULT,
        AttackOutcome::Crit => base + base / 2,
        AttackOutcome::Normal
        | AttackOutcome::Glancing
        | AttackOutcome::Crushing
        | AttackOutcome::Block => base,
    };
    // The block draw follows the weapon draw.
    let blocks = rolls_spell_block(
        ctx,
        crate::conformance_rolls::RollStream::Swing(attacker.guid),
        ranged_spell_id,
        table.partial_block,
    );
    let block_value = || defender_block_value(ctx, target);
    if school != 0 {
        let blocked = if blocks {
            block_value().min(pre_mit)
        } else {
            0
        };
        let unblocked = pre_mit - blocked;
        let after_resist = crate::spell::apply_resistance(
            ctx,
            target.guid,
            attacker.guid,
            1 << school,
            unblocked as i32,
            0,
        )
        .max(0) as u32;
        return RangedRoll {
            damage: after_resist,
            outcome,
            resisted: unblocked.saturating_sub(after_resist),
            blocked,
        };
    }
    let mit_pct = armor_mitigation_pct(
        crate::modifiers::effective_armor(ctx, target),
        attacker.level,
    );
    let mitigated = armor_mitigated(pre_mit, mit_pct);
    let blocked = if blocks {
        block_value().min(mitigated)
    } else {
        0
    };
    RangedRoll {
        damage: mitigated - blocked,
        outcome,
        resisted: 0,
        blocked,
    }
}

pub(crate) fn roll_money(ctx: &ReducerContext, min: u32, max: u32) -> u32 {
    if max == 0 || max < min {
        return 0;
    }
    min + ctx.random::<u32>() % (max - min + 1)
}

#[cfg(test)]
mod weapon_strike_tests {
    use super::{normalized_speed_for, strike_amount};

    #[test]
    fn a_weapon_percent_scales_the_weapon_roll_and_the_flat_bonus_together() {
        // vmangos EffectWeaponDmg: (40 weapon + 10 bonus) at Backstab's 150%.
        assert_eq!(strike_amount(40, 10, 150, (0, 0), false, 0), 75);
    }

    #[test]
    fn a_strike_crits_for_double_before_armor_mitigates_it() {
        assert_eq!(strike_amount(40, 10, 100, (0, 0), true, 20), 80);
    }

    #[test]
    fn a_damage_spell_mod_applies_after_the_weapon_percent() {
        assert_eq!(strike_amount(40, 10, 100, (5, 10), false, 0), 60);
    }

    #[test]
    fn normalized_speed_follows_the_weapon_type() {
        use crate::items::{invtype as t, weapon_subclass as ws};
        assert_eq!(normalized_speed_for(None), 2400);
        assert_eq!(normalized_speed_for(Some((t::WEAPON, ws::DAGGER))), 1700);
        assert_eq!(normalized_speed_for(Some((t::WEAPON, ws::SWORD_1H))), 2400);
        assert_eq!(
            normalized_speed_for(Some((t::TWO_HAND_WEAPON, ws::STAFF))),
            3300
        );
        assert_eq!(normalized_speed_for(Some((t::THROWN, ws::THROWN))), 2800);
    }
}

#[cfg(test)]
mod weapon_damage_tests {
    use super::add_weapon_damage;

    #[test]
    fn flat_weapon_damage_applies_after_template_rounding_and_clamps() {
        assert_eq!(add_weapon_damage(10.6, 20), 31);
        assert_eq!(add_weapon_damage(10.4, -20), 0);
        assert_eq!(add_weapon_damage(u32::MAX as f32, i32::MAX), u32::MAX);
    }
}

#[cfg(test)]
mod seal_tests {
    use super::seal_swing_holy;

    #[test]
    fn seal_swing_holy_scales_with_coeff_and_speed() {
        assert_eq!(seal_swing_holy(0, 2000), 0);
        assert_eq!(seal_swing_holy(108, 2000), 2);
        assert_eq!(seal_swing_holy(216, 2000), 4);
        assert_eq!(seal_swing_holy(216, 3600), 7);
    }
}

#[cfg(test)]
mod queued_strike_tests {
    use super::{QueuedStrikeDamage, SwingRolls};
    use crate::combat::AttackOutcome;

    fn resolve_swing(
        outcome: AttackOutcome,
        partial_block_bp: u32,
        rolls: SwingRolls,
        range: (u32, u32),
        strike: Option<QueuedStrikeDamage>,
        armor_pct: u32,
        block_value: u32,
    ) -> (u32, AttackOutcome, u32) {
        super::resolve_swing(
            outcome,
            partial_block_bp,
            rolls,
            range,
            strike,
            super::Defender {
                taken: crate::spell::bonuses::WeaponDamageTaken::NONE,
                armor_pct,
                block_value,
            },
        )
    }

    fn rolls(block: u32) -> SwingRolls {
        SwingRolls { weapon: 0, block }
    }

    fn heroic_strike(bonus: u32) -> Option<QueuedStrikeDamage> {
        Some(QueuedStrikeDamage {
            bonus,
            ..Default::default()
        })
    }

    #[test]
    fn a_queued_strike_can_crit_and_be_blocked() {
        // (100 + 11) doubled by the crit, halved by armor, then 30 blocked.
        assert_eq!(
            resolve_swing(
                AttackOutcome::Crit,
                10_000,
                rolls(0),
                (100, 100),
                heroic_strike(11),
                50,
                30
            ),
            (81, AttackOutcome::Crit, 30)
        );
    }

    #[test]
    fn a_blocked_queued_strike_keeps_its_glancing_outcome() {
        // (100 + 11) glances to 77, then 30 of it is blocked.
        assert_eq!(
            resolve_swing(
                AttackOutcome::Glancing,
                10_000,
                rolls(0),
                (100, 100),
                heroic_strike(11),
                0,
                30
            ),
            (47, AttackOutcome::Glancing, 30)
        );
    }

    #[test]
    fn a_white_swing_blocks_from_its_table_band() {
        assert_eq!(
            resolve_swing(AttackOutcome::Block, 0, rolls(0), (100, 100), None, 50, 30),
            (20, AttackOutcome::Block, 30)
        );
    }

    #[test]
    fn a_queued_strike_misses_its_block_roll_above_the_block_chance() {
        assert_eq!(
            resolve_swing(
                AttackOutcome::Normal,
                5_000,
                rolls(5_000),
                (100, 100),
                heroic_strike(11),
                0,
                30
            ),
            (111, AttackOutcome::Normal, 0)
        );
    }

    #[test]
    fn damage_modifiers_scale_the_weapon_and_the_bonus_together() {
        let maul = Some(QueuedStrikeDamage {
            bonus: 1000,
            mod_flat: 0,
            mod_pct: 100,
        });
        assert_eq!(
            resolve_swing(AttackOutcome::Normal, 0, rolls(9_999), (1, 1), maul, 0, 0),
            (2002, AttackOutcome::Normal, 0)
        );
    }

    #[test]
    fn zero_damage_stays_zero_on_a_glancing_hit() {
        assert_eq!(
            resolve_swing(AttackOutcome::Glancing, 0, rolls(9_999), (0, 0), None, 0, 0),
            (0, AttackOutcome::Glancing, 0)
        );
    }

    #[test]
    fn a_glancing_strike_reduces_the_whole_hit() {
        assert_eq!(
            resolve_swing(
                AttackOutcome::Glancing,
                0,
                rolls(9_999),
                (100, 100),
                heroic_strike(100),
                0,
                0
            ),
            (140, AttackOutcome::Glancing, 0)
        );
    }
}
