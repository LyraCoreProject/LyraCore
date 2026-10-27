use spacetimedb::{reducer, ReducerContext};

use crate::game_taxi_node;

#[reducer]
pub fn debug_learn_taxi_node(
    ctx: &ReducerContext,
    character_guid: u64,
    node_id: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::helpers::require_character(ctx, character_guid)?;
    ctx.db
        .game_taxi_node()
        .id()
        .find(node_id)
        .ok_or_else(|| format!("taxi node {node_id} does not exist"))?;
    crate::taxi::discover(ctx, character_guid, node_id);
    Ok(())
}
