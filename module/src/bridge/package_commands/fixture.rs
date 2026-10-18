#![cfg(feature = "debug_reducers")]

use spacetimedb::ReducerContext;

pub(super) fn handle(
    ctx: &ReducerContext,
    actor_guid: u64,
    action: &str,
    _payload: &str,
) -> Result<String, String> {
    match action {
        "status" => Ok(actor_guid.to_string()),
        "refuse" => {
            super::super::send(ctx, actor_guid, "fixture.partial", "must roll back");
            Err("FixtureRefusal".into())
        }
        "oversized" => Ok("x".repeat(1025)),
        _ => Err("UnknownAction".into()),
    }
}
