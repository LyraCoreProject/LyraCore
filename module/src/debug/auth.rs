use spacetimedb::{reducer, ReducerContext, Table, TimeDuration};

use crate::auth::{game_session, game_session_reaper_schedule};

#[reducer]
pub(crate) fn debug_accelerate_session_reaper(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let schedules = ctx.db.game_session_reaper_schedule();
    let mut schedule = schedules
        .iter()
        .next()
        .ok_or_else(|| "Session reaper is not armed".to_string())?;
    schedule.scheduled_at =
        crate::time::schedule_interval(ctx, TimeDuration::from_micros(1_000_000));
    schedules.scheduled_id().update(schedule);
    Ok(())
}

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
