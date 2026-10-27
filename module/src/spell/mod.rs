pub(crate) mod cast;
mod outcome;
mod presentation;
pub(crate) use outcome::{
    cast_readiness, pending_cast, request_cast, CastFinish, CastGateRefusal, CastHandle,
    CastRefusal, CastRefusalKind, CastRefusalReason, CastStart,
};
pub(crate) use presentation::recompute_display;
mod control;
mod effects;
mod math;
pub(crate) mod proc;
mod scheduler;
mod self_resurrect;
pub(crate) mod spellbook;
pub(crate) mod stacking;
mod tables;
mod taxonomy;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use cast::passive_applies_effect_kind;
pub(crate) use cast::{
    apply_direct_heal, apply_effect, apply_spell_auras, aura_apply, begin_cast,
    begin_cast_with_admission, cast_triggered, charge_spell_cost, check_cast_start_gates,
    check_pending_cast_los, clear_dead_callback_cast_admission, faction_target_violation,
    recipe_reagents, resolve_cast_at, resolve_cast_at_typed, revive_amount, select_targets,
    start_creature_spell, start_spell_cooldown, CreatureSpellCasterAdmission, CreatureSpellStart,
    CreatureSpellStartMode, CreatureSpellTarget, ItemGrantOutcome,
};
#[cfg(feature = "debug_reducers")]
pub(crate) use cast::{blink_forward, resolve_cast};
pub(crate) use control::{
    break_auras_on_damage, break_auras_on_move, break_channel, buff_status, cancel_cast_attempt,
    control_status, expire_cast_attempt, fear_source, has_aura, interrupt_cast,
    interrupt_cast_and_lock, is_action_blocked, is_disarmed, is_immune_to_mechanic,
    is_movement_blocked, is_non_melee_spell_casting, is_self_movement_suppressed, pushback_cast,
    remove_auras_on_death, shed_auras, spell_control,
};
#[cfg(test)]
pub(crate) use control::{breaks_on_damage, cc_blocks};
pub use control::{BuffStatus, BuffUnavailableReason, ControlReadError, UnitControl};
pub(crate) use effects::{
    absorb_incoming, apply_target_damage, arm_spell_retaliation, dispel_target, recompute_sheet,
    recompute_vitals,
};
#[cfg(test)]
pub(crate) use math::{
    absorb_draw, can_afford, int_spell_power, intellect_crit_bp, spell_crit_bp_with_bonus,
    stance_damage_taken_pct,
};
pub(crate) use math::{
    add_flat_pool_bonus, apply_damage_pct, apply_resistance, apply_spell_crit, apply_spell_mod,
    aura_moves_sheet, aura_moves_vitals, auras_on, break_stealth, client_form_for_stance,
    combat_field_bonus, combat_health_regen_pct, compose_magnitude, cost_gate_blocks,
    damage_taken_bonus, damaged_value, detect_range_mod, dispel_category_matches, distance_3d,
    drain_shields, effect_amount, energized_value, form_recast_toggles_off, healed_value,
    healing_power, is_behind, is_facing, is_on_cooldown, is_ready_at, is_stealthed,
    level_gate_blocks, power_regen_rate, projectile_travel_ms, rederive_pool, remove_seal_auras,
    resist_magic_damage, resistance_bonus, school_index, school_locked, seal_amount,
    seal_amount_and_spell, should_arm_spell_retaliation, speed_bonus, spell_crit_bp,
    spell_miss_chance_bp, spell_mod, spell_power, stance_allows, stance_damage_done_pct,
    stance_switch_clears_power, stance_threat_pct, stat_bonus, stat_pct_bonus,
};
#[cfg(feature = "debug_reducers")]
pub(crate) use scheduler::arm_ground_area_tick;
pub(crate) use scheduler::is_due_for_expiry;
pub(crate) use scheduler::{
    apply_cast_spell_at, do_cancel_aura, do_cancel_cast, do_cast_spell, do_resurrect_response,
    fire_pending_cast,
};
pub use scheduler::{fire_spell_impact, tick_auras, tick_ground_areas};
pub(crate) use self_resurrect::{
    clear_resurrect_request, clear_resurrect_request_and_option, do_self_resurrect,
    record_self_resurrect_option,
};

pub(crate) use spellbook::{
    apply_racial_passives, forget_spell, knows_spell, learn_spell, learn_spell_with_dependents,
    strip_spell_auras,
};
pub use spellbook::{
    game_createinfo_spell, game_player_spell, game_spell_chain, game_spell_learn, CreateinfoSpell,
    PlayerSpell, SpellChain, SpellLearn,
};

pub use tables::{
    game_aura, game_aura_schedule, game_dynamic_object, game_ground_area,
    game_ground_area_schedule, game_pending_cast, game_pending_spell_impact,
    game_resurrect_request, game_school_lockout, game_self_resurrect_option, game_spell,
    game_spell_cast_event, game_spell_cast_event_live, game_spell_cd, game_spell_cooldown,
    game_spell_effect, game_spell_impact_event, game_spell_impact_event_live,
    game_spell_proc_event, game_spell_reagent, Aura, AuraSchedule, DynamicObject, GroundArea,
    GroundAreaSchedule, PendingCast, PendingSpellImpact, ResurrectRequest, SchoolLockout,
    SelfResurrectOption, Spell, SpellCastEvent, SpellCd, SpellCooldown, SpellEffect,
    SpellImpactEvent, SpellProcEvent, SpellReagent, CAST_FAIL_INTERRUPTED, CAST_FAIL_NO_POWER,
    CAST_MISS_DODGE, CAST_MISS_MISS, CAST_MISS_NONE, CAST_MISS_PARRY, CAST_MISS_RESIST,
};

pub(crate) use lyracore_shared::spell::taxonomy::{
    A_ABSORB, A_COMBAT_HEALTH_REGEN_PCT, A_CONTROL, A_DISARM, A_FLAG, A_IMMUNITY, A_MOD_COMBAT,
    A_MOD_DAMAGE_TAKEN, A_MOD_DETECT_RANGE, A_MOD_POWER_REGEN_PCT, A_MOD_RESISTANCE, A_MOD_SPEED,
    A_MOD_STAT, A_MOD_STAT_PCT, A_MOUNTED, A_PERIODIC_DAMAGE, A_PERIODIC_ENERGIZE, A_PERIODIC_HEAL,
    A_PERIODIC_TRIGGER, A_PROC_DAMAGE, A_PROC_TRIGGER, A_RETALIATE, A_SEAL, A_SELF_RESURRECT,
    A_SPELLMOD_FLAT, A_SPELLMOD_PCT, A_STEALTH, A_TRANSFORM, BEAST_TYPE, COMBAT_ATTACK_POWER,
    COMBAT_BLOCK, COMBAT_CRIT, COMBAT_DEFENSE, COMBAT_DMG_DONE, COMBAT_DODGE, COMBAT_HIT,
    COMBAT_PARRY, COMBAT_SPELL_POWER, COMBAT_THREAT, CRITTER_TYPE, E_ADD_COMBO, E_BLINK, E_CHARGE,
    E_CONVERT_RESOURCE, E_CREATE_ITEM, E_DAMAGE, E_DISMOUNT, E_DISTRACT, E_DUEL, E_ENERGIZE,
    E_FEED_PET, E_FINISHER_DAMAGE, E_HEAL, E_HEAL_MAX_HEALTH, E_INTERRUPT, E_JUDGEMENT,
    E_NEXT_SWING, E_PERSISTENT_AREA, E_PICKPOCKET, E_POWER_BURN, E_RECALL_HOME, E_REDUCE_THREAT,
    E_RESURRECT, E_SCRIPTED, E_SELF_RESURRECT, E_SET_STANCE, E_SUMMON_HOSTILE, E_SUMMON_PET,
    E_TAME_CREATURE, E_TAUNT, E_TRIGGER, E_WEAPON_STRIKE, HUMANOID_TYPE, KIND_AURA_BIT, M_FEAR,
    M_POLY, M_ROOT, M_STUN, P_DISPLAY_ID, P_ENTRY, P_GAMEOBJECT_ENTRY, P_MECHANIC, P_PCT_MAX_POWER,
    P_POWER_TYPE, P_SPEED_KIND, RESIST_ARMOR, SPEED_MOUNTED, SPEED_MOVE, SPEED_SWING,
    SPELLMOD_OP_CASTING_TIME, SPELLMOD_OP_DAMAGE, SPELL_ATTR_CHANNELED,
    SPELL_ATTR_FINISHER_DURATION, SPELL_ATTR_INCAP_OPENER, SPELL_ATTR_PASSIVE,
    SPELL_ATTR_REQ_BEHIND, SPELL_ATTR_REQ_DAGGER, SPELL_ATTR_REQ_OVERPOWER, SPELL_ATTR_REQ_REVENGE,
    SPELL_ATTR_REQ_STEALTH, SPELL_ATTR_STEALTH_SAFE, SPELL_ATTR_USES_RANGED_SLOT, STANCE_BEAR,
    STANCE_CAT, STANCE_DEFENSIVE, STANCE_DIRE_BEAR, STAT_AGI, STAT_ALL, STAT_INT, STAT_SPI,
    STAT_STA, STAT_STR, T_AREA_ALLY, T_AREA_ENEMY, T_CHAIN_ENEMY, T_SELF, T_TARGET_ALLY,
    T_TARGET_ANY, T_TARGET_ENEMY,
};
#[cfg(test)]
pub(crate) use lyracore_shared::spell::taxonomy::{
    P_NONE, P_SCHOOL_MASK, STANCE_BATTLE, STANCE_BERSERKER, T_SCRIPTED,
};
pub(crate) use tables::SpellCastEventKind;
pub(crate) use taxonomy::{
    DISARM_SWING_RETAINED_PCT, INT_PER_SPELL_POWER, INT_SPELL_POWER_BASE, PROJECTILE_SPEED_YPS,
    SPELL_POWER_COEFF_PCT, STANCE_DEFENSIVE_DMG_DONE_PCT, STANCE_DEFENSIVE_DR_PCT,
    STANCE_DEFENSIVE_THREAT_PCT,
};

pub(crate) use tables::{sweep_delete_game_aura, sweep_transfer_game_aura};

pub(crate) use spellbook::{
    sweep_delete_game_player_spell, sweep_restamp_game_player_spell,
    sweep_transfer_game_player_spell,
};
