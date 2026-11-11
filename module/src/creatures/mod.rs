mod ai;
pub(crate) mod catalogue;
pub(crate) mod cycle;
pub(crate) mod distraction;
pub(crate) mod eventai;
pub(crate) mod guardian;
mod hostile_summon;
pub(crate) mod hunter_pet;
pub(crate) mod leg;
pub(crate) mod pet;
pub(crate) mod pet_care;
mod pet_progression;
pub(crate) mod population;
pub(crate) mod presentation;
pub(crate) mod spawn;

pub(crate) mod tick;

pub(crate) use ai::{combat_active_radius, flee_eligible, rout_window_open, should_flee};
pub(crate) use eventai::{
    begin_death_dispatch, cancel_relay_runs_for_instance, cancel_relay_runs_for_new_life,
    cancel_relay_runs_for_source, creature_ai_on_aggro, creature_ai_on_creature_death,
    creature_ai_on_creature_spawn, creature_ai_on_unit_death, current_definition_revision,
    drop_summon_expiry, eventai_on_evade, eventai_on_reached_home, eventai_on_receive_emote,
    eventai_on_spell_hit, finish_death_dispatch, melee_enabled, place_temporary_summon,
    react_state, reset_creature_lifecycle, reset_engagement, seed_on_aggro_fixtures,
    CreatureInstruction, CreatureReactState, SummonLocation,
};
#[cfg(feature = "debug_reducers")]
pub(crate) use eventai::{
    run_relay_arrival, stage_script_failure_policies_for_debug, start_imported_relay,
};
pub(crate) use tick::cadence::{
    is_sense_tick_for_interval, sense_period_secs_for_interval, tick_secs_for_interval, TickScope,
    TickSlice, GLOBAL_TICK_INSTANCE, MOVE_TICK_MICROS, SLICED_TICK_MICROS,
};
#[cfg(test)]
pub(crate) use tick::cadence::{MOVE_TICK_SECS, SENSE_EVERY_N_TICKS};

#[cfg(feature = "debug_reducers")]
pub(crate) use eventai::{
    mark_summon_origin_for_debug, remove_guardians, replace_definition_for_debug,
    replace_relay_catalogue_for_debug, replace_relays_for_debug, replace_single_relay_for_debug,
    verify_summon_expiry_boundaries_for_debug,
};
pub(crate) use hostile_summon::{check_hostile_summon, summon_hostile};
pub use hunter_pet::PetKind;

pub(crate) use hunter_pet::{
    clear_live_pet_kind, hunter_pet_for_owner, live_pet_kind, mark_summoned_pet,
    publish_hunter_pet_protocol, rename_hunter_pet, tame_creature, validate_tame, PetRenameRefusal,
    PET_NAME_INVALID,
};

pub(crate) use pet::{apply_pet_command, despawn_pets, pet_guid_for, pet_of, summon_pet};

pub use pet_care::CARE_INTERVAL_MICROS;

pub(crate) use pet_care::{apply_item_target_spell, on_pet_death, scale_hunter_pet_swing};

pub use pet_progression::PetHappinessState;
pub(crate) use pet_progression::{
    award_hunter_pet_kill_progression, pet_xp_to_next_level, update_pet_loyalty_from_care,
};

pub use spawn::{MOVEMENT_IDLE, MOVEMENT_RANDOM, MOVEMENT_WAYPOINT};

#[cfg(feature = "debug_reducers")]
pub(crate) use spawn::build_creature_from_template;
pub(crate) use spawn::{
    build_creature_entity, clear_gossip_menu_override, clear_gossip_menu_overrides_for_instance,
    creature_spawn_evidence, current_life_seq, insert_creature_entity, rolled_creature_stats,
    scale_creature_damage_for_level, scale_health_for_rank, set_creature_gossip_menu,
};

pub(crate) use tick::despawn_creature_entity;

pub(crate) use hunter_pet::{
    sweep_delete_game_hunter_pet, sweep_delete_game_hunter_pet_protocol,
    sweep_transfer_game_hunter_pet, sweep_transfer_game_hunter_pet_protocol,
};

pub(crate) use pet::{sweep_delete_game_pet_command, sweep_transfer_game_pet_command};

pub(crate) use spawn::cast_condition;
