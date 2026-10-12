//! Tier 2 (NPCs): server-authored creatures. Static template + patrol waypoints, the broadcast
//! creature-move event the gateway relays as `SMSG_MONSTER_MOVE`, and the scheduled patrol tick.
//! The live creature is a `game_world_entity` row of type Unit (see `world.rs`). [static]/[event]
//!
//! Split into submodules — every `crate::creatures::<sym>` path still resolves via the re-exports
//! below:
//!   - [`spawn`] — the creature tables + entity builders + stat rolls + the content importer
//!   - [`ai`]    — pure movement/flee/chase/wander geometry + the AI tuning consts (+ the unit tests)
//!   - [`tick`]  — the move-event tables + the `tick_creatures` pass pipeline
//!   - [`pet`]   — the warlock pet system (summon/despawn/follow), riding the creature machinery above
//!   - [`hunter_pet`] — durable Hunter-pet identity and the completed-tame transition
//!   - [`pet_progression`] — Hunter-pet XP, levels, live-stat refresh and loyalty
//!   - [`distraction`]: the Distraction a Distract cast puts on an idle Creature

mod ai;
pub(crate) mod cycle;
pub(crate) mod distraction;
mod eventai;
mod hostile_summon;
mod hunter_pet;
mod pet;
mod pet_care;
mod pet_progression;
pub(crate) mod population;
pub(crate) mod presentation;
mod spawn;
pub(crate) mod tick;

pub(crate) use ai::{
    beyond_combat_backstop, chase_step, combat_active_radius, flee_eligible,
    is_sense_tick_for_interval, pursuit_deadline_ms, rout_window_open,
    sense_period_secs_for_interval, should_evade, should_flee, tick_secs_for_interval, TickScope,
    GLOBAL_TICK_INSTANCE, MOVE_TICK_MICROS,
};
#[cfg(test)]
pub(crate) use ai::{MOVE_TICK_SECS, SENSE_EVERY_N_TICKS};
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
pub(crate) use eventai::{run_relay_arrival, start_imported_relay};

pub use eventai::{
    game_creature_ai_broadcast_text, game_creature_ai_definition, game_creature_ai_event,
    game_creature_ai_relay_arrival, game_creature_ai_relay_continuation,
    game_creature_ai_relay_definition, game_creature_ai_relay_run, game_creature_ai_rule_state,
    game_creature_ai_spell_metadata, game_creature_ai_state, game_creature_ai_summon,
    CreatureAiBroadcastText, CreatureAiDefinition, CreatureAiEvent, CreatureAiForcedDespawn,
    CreatureAiMovementIntent, CreatureAiMovementPathWaypoint, CreatureAiResetDeferral,
    CreatureAiReturningHome, CreatureAiRuleState, CreatureAiSpellMetadata, CreatureAiState,
    CreatureAiSummon, CreatureAiSummonExpiry, CreatureAiSummonOrigin, RelayArrival,
    RelayContinuation, RelayDefinition, RelayRun,
};

#[cfg(feature = "debug_reducers")]
pub(crate) use eventai::{
    mark_summon_origin_for_debug, remove_guardians, replace_definition_for_debug,
    replace_relay_catalogue_for_debug, replace_relays_for_debug, replace_single_relay_for_debug,
    verify_summon_expiry_boundaries_for_debug,
};
pub(crate) use hostile_summon::{check_hostile_summon, summon_hostile};
pub use hunter_pet::{
    game_hunter_pet, game_hunter_pet_protocol, game_live_pet_kind, HunterPet, HunterPetProtocol,
    LivePetKind, PetKind,
};

pub(crate) use hunter_pet::{
    clear_live_pet_kind, hunter_pet_for_owner, live_pet_kind, mark_summoned_pet,
    publish_hunter_pet_protocol, tame_creature, validate_tame,
};

pub(crate) use pet::{apply_pet_command, despawn_pets, pet_of, summon_pet};
pub use pet::{game_pet_command, PetCommand};

pub use pet_care::{game_pet_care_schedule, PetCareSchedule, CARE_INTERVAL_MICROS};

pub(crate) use pet_care::{apply_item_target_spell, on_pet_death, scale_hunter_pet_swing};

pub use pet_progression::PetHappinessState;
pub(crate) use pet_progression::{
    award_hunter_pet_kill_progression, pet_xp_to_next_level, update_pet_loyalty_from_care,
};
pub use presentation::{
    game_creature_presentation, game_creature_relay_temporary_faction, CreaturePresentation,
    CreatureRelayTemporaryFaction,
};

pub use spawn::{
    game_creature_cast, game_creature_family, game_creature_gossip_menu_override,
    game_creature_spawn, game_creature_spell, game_creature_template, game_creature_waypoint,
    game_gossip_menu, game_gossip_menu_profile, game_gossip_menu_profile_option,
    game_gossip_option, game_npc_text, game_npc_text_slot, CreatureCast, CreatureFamily,
    CreatureGossipMenuOverride, CreatureSpawn, CreatureSpawnMap, CreatureSpell, CreatureTemplate,
    CreatureWaypoint, GossipMenu, GossipMenuProfile, GossipMenuProfileOption, GossipOption,
    NpcText, NpcTextSlot, MOVEMENT_IDLE, MOVEMENT_RANDOM,
};

#[cfg(test)]
pub(crate) use spawn::rank_hp_multiplier_permille;
pub(crate) use spawn::{
    build_creature_entity, build_player_entity, clear_gossip_menu_overrides_for_instance,
    creature_spawn_evidence, current_life_seq, insert_creature_entity, rolled_creature_stats,
    scale_creature_damage_for_level, scale_health_for_rank, set_creature_gossip_menu, timer_never,
};

pub use tick::{
    game_creature_move_event, game_creature_move_schedule, game_creature_spline, CreatureMoveEvent,
    CreatureMoveSchedule, CreatureSpline,
};

pub(crate) use tick::despawn_creature_entity;

pub(crate) use hunter_pet::{
    sweep_delete_game_hunter_pet, sweep_delete_game_hunter_pet_protocol,
    sweep_transfer_game_hunter_pet, sweep_transfer_game_hunter_pet_protocol,
};

pub(crate) use pet::{sweep_delete_game_pet_command, sweep_transfer_game_pet_command};

pub(crate) use spawn::cast_condition;

pub use eventai::{game_creature_ai_reset_deferral, game_creature_ai_returning_home};

pub use eventai::{
    game_creature_ai_forced_despawn, game_creature_ai_summon_expiry, game_creature_ai_summon_origin,
};

pub use eventai::{game_creature_ai_movement_intent, game_creature_ai_movement_path_waypoint};
