use spacetimedb::{reducer, ReducerContext};

use crate::auth::game_session;

/// Expire an Account's Session so the handshake and scheduled reaper can be verified.
#[reducer]
pub fn debug_expire_session(ctx: &ReducerContext, account_id: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let sessions = ctx.db.game_session();
    let mut session = sessions
        .account_id()
        .find(account_id)
        .ok_or_else(|| "Account has no Session".to_string())?;
    session.expires_at = session.created_at;
    sessions.account_id().update(session);
    Ok(())
}
