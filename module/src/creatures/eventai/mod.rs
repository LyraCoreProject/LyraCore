//! Native creature EventAI rules and their durable state.

mod engine;
mod fixtures;
mod loader;
mod model;
mod presentation;
mod relay;
mod tables;

mod combat;
mod death;
mod edges;
mod mobility;
pub(crate) mod movement;
mod quest_credit;
mod threat;

pub(crate) use combat::{authored_combat, current_definition_revision};
#[cfg(test)]
pub(crate) use combat::{beneficiary_guid, condition, AuthoredCombat};
pub(crate) use edges::reset_creature_lifecycle;
pub(crate) use edges::{
    begin_death_dispatch, creature_ai_on_aggro, creature_ai_on_creature_death,
    creature_ai_on_creature_spawn, creature_ai_on_unit_death, finish_death_dispatch,
    reset_engagement, runs_eventai,
};
#[allow(
    unused_imports,
    reason = "later EventAI actions call these typed edge producers"
)]
pub(crate) use edges::{
    eventai_on_evade, eventai_on_reached_home, eventai_on_receive_ai_event,
    eventai_on_receive_emote, eventai_on_spell_hit, eventai_on_target_not_reachable,
};
pub use edges::{
    game_creature_ai_reset_deferral, game_creature_ai_returning_home, CreatureAiResetDeferral,
    CreatureAiReturningHome,
};
#[cfg(test)]
pub(crate) use engine::{evaluate, EventAiWorld};
pub(crate) use fixtures::seed_on_aggro_fixtures;
#[cfg(feature = "debug_reducers")]
pub(crate) use loader::replace_definition_for_debug;
#[cfg(test)]
pub(crate) use mobility::summon_lifetime_after;
#[cfg(feature = "debug_reducers")]
pub(crate) use mobility::verify_summon_expiry_boundaries_for_debug;
pub(crate) use mobility::{drop_summon_expiry, ranged_posture, react_state};
pub use mobility::{
    game_creature_ai_forced_despawn, game_creature_ai_summon_expiry,
    game_creature_ai_summon_origin, CreatureAiForcedDespawn, CreatureAiSummonExpiry,
    CreatureAiSummonOrigin,
};
#[cfg(feature = "debug_reducers")]
pub(crate) use mobility::{mark_summon_origin_for_debug, remove_guardians};
pub(crate) use mobility::{place_temporary_summon, summon_life_seq};
pub(crate) use model::*;
pub use movement::{
    game_creature_ai_movement_intent, game_creature_ai_movement_path_waypoint,
    CreatureAiMovementIntent, CreatureAiMovementPathWaypoint,
};
use presentation::import_verified_rajaxx_spawn_protection;
pub(crate) use presentation::{
    CreaturePresentationInstruction, CreaturePresentationMount, FlagOverride,
};
pub use relay::*;
#[cfg(feature = "debug_reducers")]
pub(crate) use relay::{
    replace_relay_catalogue_for_debug, replace_relays_for_debug, replace_single_relay_for_debug,
};
pub use tables::*;

use spacetimedb::ReducerContext;

/// Run one explicit EventAI request against durable Module state.
pub(crate) fn evaluate_context(ctx: &ReducerContext, request: EventAiRequest) -> u64 {
    engine::evaluate(&mut engine::DatabaseWorld::new(ctx), request)
}
