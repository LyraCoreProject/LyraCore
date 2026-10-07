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

/// Drive `package_account::create_package_character` for `package_name` without a Package present.
#[reducer]
pub fn debug_create_package_character(
    ctx: &ReducerContext,
    package_name: String,
    name: String,
    race: u8,
    class: u8,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::package_account::create_package_character(ctx, &package_name, &name, race, class)
        .map(|_| ())
}
