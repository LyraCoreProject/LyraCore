mod area_target;
mod attack_table;
pub(crate) mod damage;
pub(crate) mod damage_shield;
pub(crate) mod death;
pub(crate) mod energy;
pub(crate) mod engage;
pub(crate) mod regeneration;
pub(crate) use regeneration::{
    activate_character_regeneration, enter_character_regeneration,
    sweep_delete_game_character_regeneration, sweep_transfer_game_character_regeneration,
};
mod folds;
mod inactive_grid;
mod leash;
pub(crate) mod queued_strike;
pub(crate) mod reach;
pub(crate) mod swing;
mod tables;
mod wear;

pub(crate) use attack_table::{
    attack_table_for, roll_attack, AttackOutcome, AttackType, DeliveredSpell,
};
#[cfg_attr(
    not(any(has_packages, feature = "debug_reducers")),
    allow(unused_imports)
)]
pub(crate) use damage::{apply_hit, final_damage, fold_incoming_damage};
pub(crate) use damage::{
    clear_lethal_damage_floor, clear_stale_lethal_damage_floor, commit_death_prevention,
    damage_result, deal_damage, deal_environmental_damage, duel_finished_by,
    immune_to_environmental_damage, is_killing_blow, lethal_floor_protects, resolve_damage,
    set_lethal_damage_floor, DamageResult, EnvironmentalHit, FrozenDamage, Hit, HitSource,
};
pub(crate) use death::{
    force_creature_death, kill_creature, kill_player, kill_player_by_hit, DRAIN_SOUL_SPELL_ID,
    RESPAWN_MICROS, SOUL_SHARD_ENTRY,
};
pub(crate) use leash::{beyond_combat_backstop, pursuit_deadline_ms, should_evade};
#[cfg(test)]
pub(crate) use tables::PURSUIT_WINDOW_MS;

pub(crate) use area_target::area_may_harm;
pub use engage::AttackStart;
pub(crate) use engage::{
    answer_creature_assistance, answer_creature_help, apply_start_attack,
    apply_start_ranged_attack, arm_creature_engagement, break_own_attacks, disengage,
    engage_passive_creature, enter_combat, is_engaged, is_hostile_target, may_harm, may_help,
    refresh_leash, relation, request_attack, resume_creature_engagement, start_melee_attack,
    stop_attack_for, stop_duel_combat, suspend_hostile_references,
    sweep_delete_game_combat_check_schedule, sweep_transfer_game_combat_check_schedule,
    validate_attack_target, weapon_changed, AttackRefusal, Relation, COMBAT_DROP_MS,
    SWING_ERROR_BAD_FACING, SWING_ERROR_NONE, SWING_ERROR_NOT_IN_RANGE,
};
#[cfg(feature = "debug_reducers")]
pub(crate) use folds::defender_block_value;
pub(crate) use folds::{
    arm_react, attack_power_modifiers, attack_time, base_melee_attack_power, effective_crit_bp,
    effective_ranged_crit_bp, effective_swing_time, equipped_offhand_weapon_damage,
    equipped_ranged_weapon, find_ammo, melee_attack_power, melee_attack_power_multiplier,
    off_hand_attack_time, ranged_attack_power_multiplier, ranged_projectile, ranged_values,
    react_window_active, retaliate_on_hit, roll_money, roll_ranged_swing, roll_swing,
    roll_swing_with_range, seal_holy_on_swing, spend_ranged_ammo, swing_range_ctx,
    weapon_strike_spell_damage, ReactKind, WeaponStrike,
};

pub(crate) use energy::regenerate_energy;
pub(crate) use wear::wear_for_environmental_damage;

#[cfg(feature = "debug_reducers")]
pub(crate) use swing::offhand_swing_range;
pub(crate) use swing::{
    drop_extra_attacks, grant_extra_attacks, has_extra_attacks, launch_ranged_strike,
};
pub use swing::{ranged_impact, tick_melee};
pub(crate) use tables::{
    apply_offhand_penalty, armor_mitigated, armor_mitigation_pct, base_melee_attack_power_for,
    is_rage_user, is_translating, is_wand_user, natural_swing_range_ap, player_swing_range_ap,
    rage_from_damage, ranged_sheet_values, regen_entity_health, regen_entity_health_in_combat,
    regen_entity_power, scale_swing_for_rank, swing_time_with_haste, total_attack_power,
    weapon_swing_range_ap, RegenWindow, AUTO_SHOT_PROJECTILE_SPEED, CLASS_DRUID,
    CREATURE_MELEE_MAX, CREATURE_MELEE_MIN, CRIT_MULT, CRUSH_DAMAGE_PCT, GLANCING_DAMAGE_PCT,
    MELEE_RANGE_SQ, RANGED_INITIAL_SHOT_MS, RANGED_RANGE_SQ, WAND_PROJECTILE_SPEED,
};
#[cfg(feature = "debug_reducers")]
pub(crate) use tables::{regen_health_in_combat, swing_profile};

pub use tables::DEFAULT_ATTACK_TIME_MS;
