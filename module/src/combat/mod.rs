pub(crate) mod death;
mod energy;
mod engage;
mod folds;
mod swing;
mod tables;

pub(crate) use death::{
    apply_hit, clear_lethal_damage_floor, clear_stale_lethal_damage_floor, commit_death_prevention,
    final_damage, fold_incoming_damage, force_creature_death, is_killing_blow, kill_creature,
    kill_player, lethal_floor_protects, set_lethal_damage_floor, Hit, HitSource,
    CORPSE_DECAY_MICROS, DRAIN_SOUL_SPELL_ID, RESPAWN_MICROS, SOUL_SHARD_ENTRY,
};
#[cfg(test)]
pub(crate) use lyracore_shared::aura::move_speed_with_pct;

#[cfg(test)]
pub(crate) use engage::COMBAT_DROP_MS;
pub(crate) use engage::{
    apply_start_attack, apply_start_ranged_attack, arm_creature_engagement, break_own_attacks,
    disengage, enter_combat, is_engaged, is_hostile_target, may_harm, may_help, refresh_leash,
    request_attack, start_melee_attack, stop_attack_for, stop_duel_combat, validate_attack_target,
    AttackRefusal, SWING_ERROR_BAD_FACING, SWING_ERROR_NONE, SWING_ERROR_NOT_IN_RANGE,
};
pub use engage::{
    game_combat_event, game_combat_event_live, game_melee_attack, game_melee_schedule,
    game_ranged_impact_schedule, AttackStart, CombatEvent, MeleeAttack, MeleeSchedule,
    RangedImpactSchedule,
};
pub(crate) use folds::{
    arm_react, aura_attack_power_bonus, clear_queued_strike, effective_agility, effective_armor,
    effective_crit_bp, effective_move_speed, effective_strength, effective_swing_time,
    equipped_offhand_weapon_damage, equipped_ranged_weapon, find_ammo, finisher_strike_damage,
    interrupt_form_bound_queued_strike, interrupt_queued_strike_of, melee_attack_power,
    queued_strike_damage, ranged_projectile, react_window_active, retaliate_on_hit, roll_money,
    roll_ranged_swing, roll_strike_outcome, roll_swing, roll_swing_with_range, seal_holy_on_swing,
    spend_ranged_ammo, swing_range_ctx, weapon_strike_spell_damage, wear_weapon, ReactKind,
    WeaponStrike,
};
#[cfg(feature = "debug_reducers")]
pub(crate) use folds::{
    effective_block_bp, effective_block_value, effective_dodge_bp, effective_miss_bp,
    effective_parry_bp,
};

pub(crate) use energy::regenerate_energy;
pub use energy::{game_energy_clock, EnergyClock};
pub(crate) use swing::launch_ranged_strike;
#[cfg(feature = "debug_reducers")]
pub(crate) use swing::offhand_swing_range;
pub use swing::{ranged_impact, tick_melee};
pub(crate) use tables::{
    agility_crit_bp, apply_offhand_penalty, armor_mitigated, armor_mitigation_pct, attack_outcome,
    block_chance_bp, crit_bp_with_bonus, crushing_chance_bp, dodge_chance_bp, effective_stat,
    glancing_chance_bp, is_rage_user, is_translating, melee_attack_power_for, miss_bp_with_bonus,
    miss_chance_bp, parry_chance_bp, player_swing_range_ap, rage_from_damage, ranged_sheet_values,
    regen_entity_health, regen_entity_health_in_combat, regen_entity_power, resist_mitigation_pct,
    scale_swing_for_rank, swing_time_with_haste, weapon_swing_range_ap, AttackTable, RegenWindow,
    AUTO_SHOT_PROJECTILE_SPEED, COMBAT_BACKSTOP_SQ, CREATURE_MELEE_MAX, CREATURE_MELEE_MIN,
    CRIT_MULT, CRUSH_DAMAGE_PCT, GLANCING_DAMAGE_PCT, HIT_BLOCK, HIT_CRIT, HIT_CRUSHING, HIT_DODGE,
    HIT_GLANCING, HIT_MISS, HIT_NORMAL, HIT_PARRY, LEASH_RADIUS_SQ, MELEE_LEEWAY_WINDOW_MS,
    MELEE_RANGE_LEEWAY_CREATURE_SQ, MELEE_RANGE_LEEWAY_SQ, MELEE_RANGE_SQ, PURSUIT_WINDOW_MS,
    RANGED_INITIAL_SHOT_MS, RANGED_RANGE_SQ, WAND_PROJECTILE_SPEED,
};
#[cfg(feature = "debug_reducers")]
pub(crate) use tables::{regen_health_in_combat, swing_profile};

pub use tables::DEFAULT_ATTACK_TIME_MS;
