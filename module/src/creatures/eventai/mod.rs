//! Native creature EventAI rules and their durable state.

mod engine;
mod fixtures;
mod model;
mod tables;

mod combat;
mod edges;
mod mobility;

pub(crate) use combat::authored_combat;
#[cfg(test)]
pub(crate) use combat::AuthoredCombat;
#[allow(
    unused_imports,
    reason = "keeps the lifecycle cleanup reachable to sibling creature modules"
)]
pub(crate) use edges::reset_creature_lifecycle;
pub(crate) use edges::{
    creature_ai_on_aggro, creature_ai_on_creature_death, creature_ai_on_creature_spawn,
    reset_engagement, runs_eventai,
};
#[cfg(test)]
pub(crate) use engine::{evaluate, EventAiWorld};
pub(crate) use fixtures::seed_on_aggro_fixtures;
pub(crate) use mobility::{drop_summon_expiry, ranged_posture};
pub use mobility::{expire_eventai_summon, CreatureAiSummonExpiry};
pub(crate) use model::*;
pub use tables::*;

use spacetimedb::ReducerContext;

/// Run one explicit EventAI request against durable Module state.
pub(crate) fn evaluate_context(ctx: &ReducerContext, request: EventAiRequest) -> u64 {
    engine::evaluate(&mut engine::DatabaseWorld::new(ctx), request)
}
