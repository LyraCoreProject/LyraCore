//! Native creature EventAI rules and their durable state.

mod engine;
mod fixtures;
mod model;
mod tables;

mod combat;
mod edges;
mod mobility;

pub(crate) use edges::creature_ai_on_aggro;
#[cfg(test)]
pub(crate) use engine::{evaluate, EventAiWorld};
pub(crate) use fixtures::seed_on_aggro_fixtures;
pub(crate) use model::*;
pub use tables::*;

use spacetimedb::ReducerContext;

/// Run one explicit EventAI request against durable Module state.
pub(crate) fn evaluate_context(ctx: &ReducerContext, request: EventAiRequest) -> u64 {
    engine::evaluate(&mut engine::DatabaseWorld::new(ctx), request)
}
