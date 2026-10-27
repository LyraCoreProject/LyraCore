use spacetimedb::ReducerContext;

use crate::{
    game_aura, game_creature_template, game_item_instance, game_item_template, game_spell_effect,
    game_world_entity, ItemInstance, WorldEntity,
};

use super::*;

fn fold_band(
    ctx: &ReducerContext,
    target_guid: u64,
    base: u32,
    field: u8,
    gear: crate::items::EquipStat,
) -> u32 {
    (i64::from(base)
        + i64::from(crate::spell::combat_field_bonus(ctx, target_guid, field))
        + i64::from(crate::items::equipped_stat_bonus(ctx, target_guid, gear)))
    .clamp(0, 10_000) as u32
}

pub(crate) fn effective_dodge_bp(ctx: &ReducerContext, target: &WorldEntity, sd: u32) -> u32 {
    fold_band(
        ctx,
        target.guid,
        dodge_chance_bp(effective_agility(ctx, target), sd),
        crate::spell::COMBAT_DODGE,
        crate::items::EquipStat::Dodge,
    )
}

pub(crate) fn effective_parry_bp(ctx: &ReducerContext, target: &WorldEntity, sd: u32) -> u32 {
    fold_band(
        ctx,
        target.guid,
        parry_chance_bp(sd),
        crate::spell::COMBAT_PARRY,
        crate::items::EquipStat::Parry,
    )
}

pub(crate) fn effective_block_bp(
    ctx: &ReducerContext,
    target: &WorldEntity,
    sd: u32,
    block_value: u32,
) -> u32 {
    if block_value > 0 {
        fold_band(
            ctx,
            target.guid,
            block_chance_bp(sd),
            crate::spell::COMBAT_BLOCK,
            crate::items::EquipStat::Block,
        )
    } else {
        0
    }
}

pub(crate) fn aura_attack_power_bonus(ctx: &ReducerContext, unit_guid: u64) -> u32 {
    crate::spell::combat_field_bonus(ctx, unit_guid, crate::spell::COMBAT_ATTACK_POWER).max(0)
        as u32
}

pub(crate) fn seal_holy_on_swing(ctx: &ReducerContext, attacker: &WorldEntity) -> (u32, u32) {
    let (amount, spell) = crate::spell::seal_amount_and_spell(ctx, attacker.guid);
    (seal_swing_holy(amount, attacker.base_attack_time_ms), spell)
}

fn seal_swing_holy(seal: i32, attack_time_ms: u32) -> u32 {
    if seal <= 0 {
        return 0;
    }
    ((seal as u64 * attack_time_ms as u64) / 100_000) as u32
}

fn queued_strike_bonus(queued_spell_id: u32, base_points: i32) -> u32 {
    if queued_spell_id == 0 {
        return 0;
    }
    base_points.max(0) as u32
}

pub(crate) fn queued_strike_on_swing(ctx: &ReducerContext, attacker: &WorldEntity) -> u32 {
    let queued = attacker.next_swing_spell;
    if queued == 0 {
        return 0;
    }
    let base_points = ctx
        .db
        .game_spell_effect()
        .by_spell()
        .filter(&queued)
        .find(|e| e.kind == crate::spell::E_NEXT_SWING)
        .map(|e| e.base_points)
        .unwrap_or(0);
    queued_strike_bonus(queued, base_points)
}

pub(crate) fn clear_queued_strike(ctx: &ReducerContext, guid: u64) {
    let entities = ctx.db.game_world_entity();
    if let Some(mut e) = entities.guid().find(guid) {
        if e.next_swing_spell != 0 {
            e.next_swing_spell = 0;
            e.update(ctx);
        }
    }
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
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64;
    if let Some(mut e) = entities.guid().find(guid) {
        let until = now_ms + REACT_WINDOW_MS;
        match kind {
            ReactKind::Overpower => e.overpower_until_ms = until,
            ReactKind::Revenge => e.revenge_until_ms = until,
        }
        e.update(ctx);
    }
}

pub(crate) fn effective_strength(ctx: &ReducerContext, e: &WorldEntity) -> u32 {
    effective_stat(
        e.strength as i32,
        crate::spell::stat_bonus(ctx, e.guid, crate::spell::STAT_STR),
        crate::items::equipped_stat_bonus(ctx, e.guid, crate::items::EquipStat::Strength),
    )
}

pub(crate) fn effective_agility(ctx: &ReducerContext, e: &WorldEntity) -> u32 {
    effective_stat(
        e.agility as i32,
        crate::spell::stat_bonus(ctx, e.guid, crate::spell::STAT_AGI),
        crate::items::equipped_stat_bonus(ctx, e.guid, crate::items::EquipStat::Agility),
    )
}

pub(crate) fn effective_armor(ctx: &ReducerContext, e: &WorldEntity) -> u32 {
    lyracore_shared::resistance::effective_resistance(
        e.armor,
        crate::spell::resistance_bonus(ctx, e.guid, crate::spell::RESIST_ARMOR),
    )
}

pub(crate) fn effective_block_value(ctx: &ReducerContext, defender: &WorldEntity) -> u32 {
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
    let from_strength = effective_strength(ctx, defender) / 20;
    (tmpl.block_value.max(0) as u32).saturating_add(from_strength)
}

pub(crate) fn effective_crit_bp(ctx: &ReducerContext, attacker: &WorldEntity) -> u32 {
    let bonus = crate::spell::combat_field_bonus(ctx, attacker.guid, crate::spell::COMBAT_CRIT)
        + crate::items::equipped_stat_bonus(ctx, attacker.guid, crate::items::EquipStat::Crit)
        + agility_crit_bp(effective_agility(ctx, attacker), attacker.level) as i32;
    crit_bp_with_bonus(bonus)
}

pub(crate) fn effective_miss_bp(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    skill_diff: u32,
) -> u32 {
    let bonus = crate::spell::combat_field_bonus(ctx, attacker.guid, crate::spell::COMBAT_HIT)
        + crate::items::equipped_stat_bonus(ctx, attacker.guid, crate::items::EquipStat::Hit);
    miss_bp_with_bonus(miss_chance_bp(skill_diff), bonus)
}

pub(crate) fn effective_swing_time(ctx: &ReducerContext, attacker: &WorldEntity) -> u32 {
    let speed = crate::spell::speed_bonus(ctx, attacker.guid, crate::spell::SPEED_SWING);
    swing_time_with_haste(attacker.base_attack_time_ms, speed)
}

pub(crate) fn effective_move_speed(ctx: &ReducerContext, guid: u64, base: f32) -> f32 {
    lyracore_shared::aura::run_speed(
        base,
        crate::spell::auras_on(ctx, guid).map(|aura| lyracore_shared::aura::AuraAmount {
            kind: aura.eff_kind,
            parameter: aura.eff_p0,
            amount: aura.amount,
            stacks: aura.stacks,
        }),
    )
}

#[cfg(test)]
use lyracore_shared::aura::mounted_move_pct;

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

pub(crate) fn find_ammo(ctx: &ReducerContext, player_guid: u64) -> Option<ItemInstance> {
    let templates = ctx.db.game_item_template();
    lyracore_shared::item::loaded_ammo(
        ctx.db
            .game_item_instance()
            .by_owner_guid()
            .filter(&player_guid),
        |item| {
            (
                item.slot,
                item.stack_count,
                templates.entry().find(item.entry).map(|tmpl| tmpl.class),
            )
        },
    )
}

const DURABILITY_WEAR_CHANCE_PCT: u32 = 10;

pub(crate) fn wear_weapon(ctx: &ReducerContext, player_guid: u64) {
    if let Some(mut inst) = crate::items::item_in_slot(
        ctx,
        player_guid,
        lyracore_shared::constants::starter_item::MAINHAND_SLOT,
    ) {
        if inst.durability > 0 && ctx.random::<u32>() % 100 < DURABILITY_WEAR_CHANCE_PCT {
            inst.durability -= 1;
            inst.update(ctx);
        }
    }
}

pub(crate) fn swing_range_ctx(ctx: &ReducerContext, attacker: &WorldEntity) -> (u32, u32) {
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
        if disarmed {
            let pct = crate::spell::DISARM_SWING_RETAINED_PCT;
            return (min * pct / 100, max * pct / 100);
        }
        return (min, max);
    }
    let class = attacker.class();
    let ap = melee_attack_power_for(
        class,
        effective_strength(ctx, attacker),
        effective_agility(ctx, attacker),
        attacker.level,
    ) + aura_attack_power_bonus(ctx, attacker.guid);
    if !disarmed {
        if let Some((dmin, dmax, delay)) = equipped_weapon_damage(ctx, attacker.guid) {
            return weapon_swing_range_ap(ap, dmin, dmax, delay);
        }
    }
    player_swing_range_ap(ap, attacker.base_attack_time_ms)
}

pub(crate) fn weapon_strike_damage(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    target: &WorldEntity,
    flat_bonus: i32,
) -> (u32, bool) {
    let (min, max) = swing_range_ctx(ctx, caster);
    let base = min + ctx.random::<u32>() % (max - min + 1);
    let total = (base as i32 + flat_bonus).max(1) as u32;
    let crit = (ctx.random::<u32>() % 10_000) < effective_crit_bp(ctx, caster);
    let pre_mit = if crit { total * CRIT_MULT } else { total };
    let mit_pct = armor_mitigation_pct(effective_armor(ctx, target), caster.level);
    ((pre_mit * (100 - mit_pct) / 100).max(1), crit)
}

pub(crate) fn retaliate_on_hit(ctx: &ReducerContext, victim_guid: u64, attacker_guid: u64) {
    let carries_retaliate = ctx
        .db
        .game_aura()
        .by_target()
        .filter(&victim_guid)
        .any(|a| a.eff_kind == crate::spell::A_RETALIATE);
    if !carries_retaliate {
        return;
    }
    let entities = ctx.db.game_world_entity();
    let (Some(victim), Some(attacker)) = (
        entities.guid().find(victim_guid),
        entities.guid().find(attacker_guid),
    ) else {
        return;
    };
    if victim.dead || attacker.dead {
        return;
    }
    let (dmg, _crit) = weapon_strike_damage(ctx, &victim, &attacker, 0);
    crate::spell::apply_target_damage(
        ctx,
        attacker_guid,
        victim_guid,
        dmg as i32,
        crate::combat::Hit::triggered(),
    );
}

pub(crate) fn roll_swing(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
) -> (u32, u8, u32) {
    let range = crate::creatures::scale_hunter_pet_swing(
        ctx,
        attacker.guid,
        swing_range_ctx(ctx, attacker),
    );
    roll_swing_with_range(ctx, attacker, target, range)
}

pub(crate) fn roll_swing_with_range(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    range: (u32, u32),
) -> (u32, u8, u32) {
    let block_value = effective_block_value(ctx, target);
    let sd = crate::skill::skill_diff_ctx(ctx, attacker, target);
    let table = AttackTable {
        miss: effective_miss_bp(ctx, attacker, sd),
        dodge: effective_dodge_bp(ctx, target, sd),
        parry: effective_parry_bp(ctx, target, sd),
        glancing: glancing_chance_bp(attacker.level, target.level),
        block: effective_block_bp(ctx, target, sd, block_value),
        crit: effective_crit_bp(ctx, attacker),
        crushing: if !attacker.is_player() && target.is_player() {
            crushing_chance_bp(attacker.level, target.level)
        } else {
            0
        },
    };
    let roll = ctx.random::<u32>() % 10_000;
    let outcome = attack_outcome(roll, &table);
    if matches!(outcome, HIT_MISS | HIT_DODGE | HIT_PARRY) {
        return (0, outcome, 0);
    }
    let (min, max) = range;
    let base = min + ctx.random::<u32>() % (max - min + 1);
    let pre_mit = match outcome {
        HIT_CRIT => base * CRIT_MULT,
        HIT_GLANCING => (base * GLANCING_DAMAGE_PCT / 100).max(1),
        HIT_CRUSHING => base * CRUSH_DAMAGE_PCT / 100,
        _ => base,
    };
    let mit_pct = armor_mitigation_pct(effective_armor(ctx, target), attacker.level);
    let dmg = (pre_mit * (100 - mit_pct) / 100).max(1);
    if outcome == HIT_BLOCK {
        let blocked = block_value.min(dmg);
        return (dmg - blocked, HIT_BLOCK, blocked);
    }
    (dmg, outcome, 0)
}

pub(crate) fn roll_ranged_swing(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    dmin: u32,
    dmax: u32,
) -> (u32, u8) {
    let sd = crate::skill::skill_diff_ctx(ctx, attacker, target);
    if (ctx.random::<u32>() % 10_000) < effective_miss_bp(ctx, attacker, sd) {
        return (0, HIT_MISS);
    }
    let is_crit = (ctx.random::<u32>() % 10_000) < effective_crit_bp(ctx, attacker);
    let base = dmin + ctx.random::<u32>() % (dmax.saturating_sub(dmin) + 1);
    let pre_mit = if is_crit { base * CRIT_MULT } else { base };
    let mit_pct = armor_mitigation_pct(effective_armor(ctx, target), attacker.level);
    let dmg = (pre_mit * (100 - mit_pct) / 100).max(1);
    (dmg, if is_crit { HIT_CRIT } else { HIT_NORMAL })
}

pub(crate) fn roll_money(ctx: &ReducerContext, min: u32, max: u32) -> u32 {
    if max == 0 || max < min {
        return 0;
    }
    min + ctx.random::<u32>() % (max - min + 1)
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
mod next_swing_tests {
    use super::queued_strike_bonus;

    #[test]
    fn no_queue_adds_nothing() {
        assert_eq!(queued_strike_bonus(0, 0), 0);
        assert_eq!(queued_strike_bonus(0, 137), 0);
    }

    #[test]
    fn queued_strike_adds_its_flat_base() {
        assert_eq!(queued_strike_bonus(78, 11), 11);
        assert_eq!(queued_strike_bonus(845, 5), 5);
    }

    #[test]
    fn non_positive_base_clamps_to_zero() {
        assert_eq!(queued_strike_bonus(78, 0), 0);
        assert_eq!(queued_strike_bonus(78, -50), 0);
    }
}

#[cfg(test)]
mod react_window_tests {
    use super::react_window_active;

    #[test]
    fn unarmed_window_is_closed() {
        assert!(!react_window_active(0, 1_000));
        assert!(!react_window_active(0, 0));
    }

    #[test]
    fn armed_window_is_open_until_deadline() {
        let until = 6_000u64;
        assert!(react_window_active(until, 1_000));
        assert!(react_window_active(until, 5_999));
        assert!(!react_window_active(until, 6_000));
        assert!(!react_window_active(until, 6_001));
    }
}

#[cfg(test)]
mod mounted_speed_tests {
    use super::{mounted_move_pct, move_speed_with_pct};

    #[test]
    fn mounted_speed_folds_only_while_mounted() {
        assert_eq!(mounted_move_pct(0, 60, true), 60);
        assert_eq!(mounted_move_pct(0, 100, true), 100);
        assert_eq!(mounted_move_pct(0, 60, false), 0);
        assert_eq!(mounted_move_pct(-20, 60, true), 40);
    }

    #[test]
    fn dismounting_keeps_an_ordinary_move_speed_buff_instead_of_forcing_bare_base() {
        assert_eq!(mounted_move_pct(20, 0, false), 20);
        assert_eq!(mounted_move_pct(20, 60, false), 20);
    }

    #[test]
    fn nominal_60_and_100_percent_mount_tiers_yield_the_exact_run_speed() {
        assert!((move_speed_with_pct(7.0, mounted_move_pct(0, 60, true)) - 11.2).abs() < 1e-4);
        assert!((move_speed_with_pct(7.0, mounted_move_pct(0, 100, true)) - 14.0).abs() < 1e-4);
    }
}
