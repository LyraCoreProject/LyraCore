pub(crate) mod cast;
pub(crate) mod chance_on_hit;
mod outcome;
mod presentation;
mod repair;
pub(crate) use repair::repair_cast_auras;
mod rolls;
pub(crate) use outcome::{
    cast_readiness, pending_cast, request_cast, CastFinish, CastGateRefusal, CastHandle,
    CastRefusal, CastRefusalKind, CastRefusalReason, CastStart,
};
pub(crate) use presentation::recompute_display;
mod control;
mod effects;
mod formulas;
mod power;
pub(crate) mod proc;
mod resist;
pub(crate) mod scheduler;
mod self_resurrect;
pub(crate) mod source_behavior;
pub(crate) mod spellbook;
pub(crate) mod stacking;
mod stance;
pub(crate) mod tables;
mod taxonomy;

#[cfg(test)]
mod tests;

pub(crate) use cast::{
    apply_direct_heal, apply_effect, apply_item_auras, apply_spell_auras, aura_apply, begin_cast,
    begin_cast_with_admission, begin_item_cast, cast_triggered, charge_spell_cost,
    check_cast_start_gates, check_pending_cast_los, clear_dead_callback_cast_admission,
    faction_target_violation, recipe_reagents, record_spell_cooldown, resolve_cast_at,
    revive_amount, select_targets, start_creature_spell, start_spell_cooldown,
    CreatureSpellCasterAdmission, CreatureSpellStart, CreatureSpellStartMode, CreatureSpellTarget,
    ItemGrantOutcome,
};
#[cfg(feature = "debug_reducers")]
pub(crate) use cast::{blink_forward, resolve_cast};
pub(crate) use control::{
    break_auras_on_damage, break_auras_on_move, break_channel, buff_status, cancel_cast_attempt,
    control_status, expire_cast_attempt, fear_source, has_aura, interrupt_cast,
    interrupt_cast_and_lock, is_action_blocked, is_disarmed, is_immune_to_mechanic,
    is_movement_blocked, is_non_melee_spell_casting, is_self_movement_suppressed, is_stunned,
    pushback_cast, remove_auras_on_death, spell_control,
};
#[cfg(test)]
pub(crate) use control::{breaks_on_damage, cc_blocks};
pub use control::{BuffStatus, BuffUnavailableReason, ControlReadError, UnitControl};
pub(crate) use effects::{
    absorb_incoming, arm_spell_retaliation, dispel_target, recompute_sheet, recompute_vitals,
};
#[cfg(test)]
pub(crate) use formulas::{absorb_draw, int_spell_power};
pub(crate) use formulas::{
    add_flat_pool_bonus, apply_damage_pct, apply_spell_crit, apply_spell_mod, compose_magnitude,
    cost_gate_blocks, damaged_value, dispel_category_matches, drain_shields, effect_amount,
    energized_value, healed_value, level_gate_blocks, rederive_pool, school_index,
    should_arm_spell_retaliation, DISPEL_MAGIC, DISPEL_POISON,
};
#[cfg(feature = "debug_reducers")]
pub(crate) use power::spell_crit_bp;
pub(crate) use power::{
    healing_power, seal_amount, seal_amount_and_spell, spell_crit_chance, spell_mod, spell_power,
};
pub(crate) use resist::{apply_resistance, resist_magic_damage};
#[cfg(feature = "debug_reducers")]
pub(crate) use rolls::magic_miss_bp;
#[cfg(feature = "debug_reducers")]
pub(crate) use scheduler::arm_ground_area_tick;
pub(crate) use scheduler::is_due_for_expiry;
pub(crate) use scheduler::{
    apply_cast_spell_at, do_cancel_aura, do_cancel_cast, do_cast_spell, do_resurrect_response,
    fire_pending_cast,
};
pub use scheduler::{fire_spell_impact, tick_auras, tick_ground_areas};
pub(crate) use self_resurrect::{
    clear_resurrect_request_and_option, do_self_resurrect, record_self_resurrect_option,
};
#[cfg(test)]
pub(crate) use stance::stance_damage_taken_pct;
pub(crate) use stance::{
    client_form_for_stance, damage_taken_bonus, form_recast_toggles_off, stance_allows,
    stance_damage_done_pct, stance_switch_clears_power, stance_threat_pct,
};

pub(crate) use spellbook::{
    apply_starting_passives, first_rank, forget_spell, knows_spell, learn_spell,
    learn_spell_with_dependents, strip_spell_auras,
};

pub(crate) use tables::{
    auras_on, break_stealth, is_stealthed, remove_seal_auras, shed_auras, AuraProjections,
};
pub use tables::{
    CAST_FAIL_INTERRUPTED, CAST_FAIL_NO_POWER, CAST_MISS_DODGE, CAST_MISS_MISS, CAST_MISS_NONE,
    CAST_MISS_PARRY, CAST_MISS_RESIST,
};

pub(crate) use lyracore_shared::spell::taxonomy::{
    A_ABSORB, A_CONTROL, A_DISARM, A_FLAG, A_IMMUNITY, A_MOD_COMBAT, A_MOD_RESISTANCE, A_MOD_SPEED,
    A_MOUNTED, A_PERIODIC_DAMAGE, A_PERIODIC_ENERGIZE, A_PERIODIC_HEAL, A_PERIODIC_TRIGGER,
    A_PROC_DAMAGE, A_PROC_TRIGGER, A_RETALIATE, A_SEAL, A_SELF_RESURRECT, A_STEALTH, BEAST_TYPE,
    CAST_FLAG_ALWAYS_HIT, CAST_FLAG_BINARY, CAST_FLAG_CANNOT_CRIT, CAST_FLAG_CHANNELED,
    CAST_FLAG_COMBO_FINISHER, CAST_FLAG_IGNORE_RESISTANCES, CAST_FLAG_INCAP_OPENER,
    CAST_FLAG_MAGIC_HIT, CAST_FLAG_REQ_BEHIND, CAST_FLAG_REQ_DAGGER, CAST_FLAG_REQ_OVERPOWER,
    CAST_FLAG_REQ_REVENGE, CAST_FLAG_REQ_STEALTH, CAST_FLAG_STEALTH_SAFE, CAST_FLAG_WEAPON_PROCS,
    COMBAT_ATTACK_POWER, COMBAT_BLOCK, COMBAT_CRIT, COMBAT_DEFENSE, COMBAT_DMG_DONE, COMBAT_DODGE,
    COMBAT_HIT, COMBAT_PARRY, COMBAT_THREAT, CRITTER_TYPE, E_ADD_COMBO, E_BLINK, E_CHARGE,
    E_CONVERT_RESOURCE, E_CREATE_ITEM, E_DAMAGE, E_DISMOUNT, E_DISTRACT, E_DUEL,
    E_ENCHANT_ITEM_TEMPORARY, E_ENERGIZE, E_FEED_PET, E_FINISHER_DAMAGE, E_HEAL, E_HEAL_MAX_HEALTH,
    E_INTERRUPT, E_JUDGEMENT, E_NEXT_SWING, E_PERSISTENT_AREA, E_PICKPOCKET, E_POWER_BURN,
    E_RECALL_HOME, E_REDUCE_THREAT, E_RESURRECT, E_SCRIPTED, E_SELF_RESURRECT, E_SET_STANCE,
    E_SUMMON_GUARDIANS, E_SUMMON_HOSTILE, E_SUMMON_PET, E_TAME_CREATURE, E_TAUNT, E_TRIGGER,
    E_WEAPON_STRIKE, HUMANOID_TYPE, KIND_AURA_BIT, M_FEAR, M_POLY, M_ROOT, M_STUN, P_DISPLAY_ID,
    P_ENTRY, P_GAMEOBJECT_ENTRY, P_MECHANIC, P_PCT_MAX_POWER, P_POWER_TYPE, P_SPEED_KIND,
    RESIST_ARMOR, SPEED_MOUNTED, SPEED_MOVE, SPEED_SWING, SPELLMOD_OP_CASTING_TIME,
    SPELLMOD_OP_DAMAGE, SPELL_ATTR_PASSIVE, SPELL_ATTR_USES_RANGED_SLOT, STANCE_BEAR, STAT_AGI,
    STAT_STR, T_AREA_ALLY, T_AREA_ENEMY, T_CHAIN_ENEMY, T_SELF, T_TARGET_ALLY, T_TARGET_ANY,
    T_TARGET_ENEMY,
};
#[cfg(test)]
pub(crate) use lyracore_shared::spell::taxonomy::{
    A_MOD_STAT, STANCE_BATTLE, STANCE_BERSERKER, STANCE_CAT, STANCE_DEFENSIVE, STANCE_DIRE_BEAR,
    T_SCRIPTED,
};
pub(crate) use tables::SpellCastEventKind;
#[cfg(test)]
pub(crate) use taxonomy::INT_SPELL_POWER_BASE;
pub(crate) use taxonomy::{DISARM_SWING_RETAINED_PCT, PROJECTILE_SPEED_YPS, SPELL_POWER_COEFF_PCT};

pub(crate) use tables::{
    sweep_delete_game_aura, sweep_delete_game_spell_cd, sweep_transfer_game_aura,
    sweep_transfer_game_spell_cd,
};

pub(crate) use spellbook::{
    sweep_delete_game_player_spell, sweep_restamp_game_player_spell,
    sweep_transfer_game_player_spell,
};
