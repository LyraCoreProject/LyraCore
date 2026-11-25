use std::collections::HashSet;

use spacetimedb::{
    reducer, table, ConnectionId, ReducerContext, ScheduleAt, Table, TimeDuration, Timestamp,
};

use super::{
    game_native_world_connection, NativeWorldState, ServerOpcodeMessage, SMSG_AUTH_RESPONSE,
};

#[table(accessor = game_native_admission_limits)]
pub struct NativeAdmissionLimits {
    #[primary_key]
    pub id: u64,
    pub max_sessions: u32,
    pub max_entering: u32,
    pub admissions_per_second: u32,
    pub window_started: Timestamp,
    pub admitted_in_window: u32,
}

#[table(accessor = game_native_admission)]
pub struct NativeAdmission {
    #[primary_key]
    #[auto_inc]
    pub ticket: u64,
    #[unique]
    pub connection_id: ConnectionId,
    #[unique]
    pub account_id: u64,
    pub admitted: bool,
    pub entering_until: Option<Timestamp>,
}

#[table(accessor = game_native_admission_tick, scheduled(native_admission_tick))]
pub struct NativeAdmissionTick {
    #[primary_key]
    pub id: u64,
    pub scheduled_at: ScheduleAt,
}

crate::outside_table_catalog!(
    game_native_admission_limits,
    game_native_admission,
    game_native_admission_tick
);

fn limits(ctx: &ReducerContext) -> NativeAdmissionLimits {
    ctx.db
        .game_native_admission_limits()
        .id()
        .find(0)
        .unwrap_or(NativeAdmissionLimits {
            id: 0,
            max_sessions: 0,
            max_entering: 0,
            admissions_per_second: 0,
            window_started: crate::time::now(ctx),
            admitted_in_window: 0,
        })
}

fn save(ctx: &ReducerContext, limits: NativeAdmissionLimits) {
    if ctx.db.game_native_admission_limits().id().find(0).is_some() {
        ctx.db.game_native_admission_limits().id().update(limits);
    } else {
        ctx.db.game_native_admission_limits().insert(limits);
    }
}

/// Zero disables a limit; reductions affect new admissions without evicting current World Sessions.
#[reducer]
pub fn native_set_admission_limits(
    ctx: &ReducerContext,
    max_sessions: u32,
    max_entering: u32,
    admissions_per_second: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let mut limits = limits(ctx);
    limits.max_sessions = max_sessions;
    limits.max_entering = max_entering;
    limits.admissions_per_second = admissions_per_second;
    save(ctx, limits);
    drain(ctx, None)
}

pub(super) fn request(
    ctx: &ReducerContext,
    connection_id: ConnectionId,
    account_id: u64,
) -> Result<(), String> {
    if let Some(previous) = ctx.db.game_native_admission().account_id().find(account_id) {
        super::disconnect(ctx, previous.connection_id);
    }
    ctx.db.game_native_admission().insert(NativeAdmission {
        ticket: 0,
        connection_id,
        account_id,
        admitted: false,
        entering_until: None,
    });
    drain(ctx, Some(connection_id))
}

pub(super) fn remove(ctx: &ReducerContext, connection_id: ConnectionId) {
    let admissions = ctx.db.game_native_admission();
    if let Some(admission) = admissions.connection_id().find(connection_id) {
        if !admission.admitted {
            super::replacement::stop_waiting(ctx, admission.account_id);
        }
        admissions.connection_id().delete(connection_id);
    }
}

/// Whether a ticket of the Account waits in the Login Queue.
pub(super) fn waiting(ctx: &ReducerContext, account_id: u64) -> bool {
    ctx.db
        .game_native_admission()
        .account_id()
        .find(account_id)
        .is_some_and(|admission| !admission.admitted)
}

/// Gives a World Session that returns from a World Shard the capacity slot its away record held,
/// without queueing. False means a newer connection for the Account replaces it.
pub(super) fn returned(ctx: &ReducerContext, connection_id: ConnectionId, account_id: u64) -> bool {
    let admissions = ctx.db.game_native_admission();
    if let Some(admission) = admissions.account_id().find(account_id) {
        return admission.connection_id == connection_id;
    }
    admissions.insert(NativeAdmission {
        ticket: 0,
        connection_id,
        account_id,
        admitted: true,
        entering_until: None,
    });
    true
}

pub(super) fn entered(ctx: &ReducerContext, connection_id: ConnectionId) {
    if let Some(mut admission) = ctx
        .db
        .game_native_admission()
        .connection_id()
        .find(connection_id)
    {
        admission.entering_until = None;
        ctx.db.game_native_admission().ticket().update(admission);
    }
}

#[reducer]
pub fn native_admission_tick(
    ctx: &ReducerContext,
    _tick: NativeAdmissionTick,
) -> Result<(), String> {
    if !crate::time::scheduler_authorized(ctx) {
        return Err("scheduler only".into());
    }
    drain(ctx, None)
}

fn reply(
    ctx: &ReducerContext,
    connection_id: ConnectionId,
    response: SMSG_AUTH_RESPONSE,
) -> Result<(), String> {
    super::send(
        ctx,
        connection_id,
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(Box::new(response)),
        false,
    )
}

/// The World Sessions that hold a capacity slot: one per Account admitted on this Shard, away on a
/// World Shard or returning from one.
fn held_sessions(accounts: impl IntoIterator<Item = u64>) -> u32 {
    let accounts: HashSet<u64> = accounts.into_iter().collect();
    u32::try_from(accounts.len()).unwrap_or(u32::MAX)
}

/// Whether a waiting ticket is admitted. A ticket whose Account still holds an earlier World
/// Session on a World Shard waits until that World Session ends.
fn admits(limits: &NativeAdmissionLimits, active: u32, entering: u32, replacing: bool) -> bool {
    !replacing
        && (limits.max_sessions == 0 || active < limits.max_sessions)
        && (limits.max_entering == 0 || entering < limits.max_entering)
        && (limits.admissions_per_second == 0
            || limits.admitted_in_window < limits.admissions_per_second)
}

fn drain(ctx: &ReducerContext, new_connection: Option<ConnectionId>) -> Result<(), String> {
    let now = crate::time::now(ctx);
    let mut limits = limits(ctx);
    if now
        .duration_since(limits.window_started)
        .is_some_and(|elapsed| elapsed.as_micros() >= 1_000_000)
    {
        limits.window_started = now;
        limits.admitted_in_window = 0;
    }
    let mut admitted = Vec::new();
    let mut entering = 0;
    let mut waiting = Vec::new();
    for admission in ctx.db.game_native_admission().iter() {
        if admission.admitted {
            admitted.push(admission.account_id);
            entering += u32::from(admission.entering_until.is_some_and(|until| until > now));
        } else {
            waiting.push(admission);
        }
    }
    let mut active = held_sessions(
        admitted
            .into_iter()
            .chain(super::realm_shard::away_accounts(ctx))
            .chain(super::handoff::waiting_accounts(ctx)),
    );
    waiting.sort_unstable_by_key(|admission| admission.ticket);
    let mut position = 0;
    for mut admission in waiting {
        let Some(mut connection) = ctx
            .db
            .game_native_world_connection()
            .connection_id()
            .find(admission.connection_id)
        else {
            remove(ctx, admission.connection_id);
            continue;
        };
        if super::current_account(ctx, &connection, admission.account_id).is_err() {
            super::disconnect(ctx, admission.connection_id);
            continue;
        }
        let replacing = super::replacement::replacing(ctx, admission.account_id);
        if admits(&limits, active, entering, replacing) {
            active += 1;
            entering += 1;
            limits.admitted_in_window = limits.admitted_in_window.saturating_add(1);
            admission.admitted = true;
            admission.entering_until = Some(now + TimeDuration::from_micros(20_000_000));
            connection.state = NativeWorldState::CharacterSelection {
                account_id: admission.account_id,
            };
            ctx.db
                .game_native_world_connection()
                .connection_id()
                .update(connection);
            reply(
                ctx,
                admission.connection_id,
                SMSG_AUTH_RESPONSE::AuthOk {
                    billing_flags: 0,
                    billing_rested: 0,
                    billing_time: 0,
                },
            )?;
            ctx.db.game_native_admission().ticket().update(admission);
        } else {
            position += 1;
            if new_connection.is_none_or(|connection| connection == admission.connection_id) {
                reply(
                    ctx,
                    admission.connection_id,
                    SMSG_AUTH_RESPONSE::AuthWaitQueue {
                        queue_position: position,
                    },
                )?;
            }
        }
    }
    save(ctx, limits);
    let ticks = ctx.db.game_native_admission_tick();
    if position == 0 {
        ticks.id().delete(0);
    } else if ticks.id().find(0).is_none() {
        ticks.insert(NativeAdmissionTick {
            id: 0,
            scheduled_at: crate::time::schedule_interval(ctx, TimeDuration::from_micros(1_000_000)),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_limit(max_sessions: u32) -> NativeAdmissionLimits {
        NativeAdmissionLimits {
            id: 0,
            max_sessions,
            max_entering: 0,
            admissions_per_second: 0,
            window_started: Timestamp::from_micros_since_unix_epoch(0),
            admitted_in_window: 0,
        }
    }

    #[test]
    fn a_world_session_away_on_a_world_shard_holds_a_capacity_slot() {
        let admitted = [1];
        let away = [2];
        let active = held_sessions(admitted.into_iter().chain(away));
        assert_eq!(active, 2);
        assert!(!admits(&session_limit(2), active, 0, false));
        assert!(admits(&session_limit(3), active, 0, false));
    }

    #[test]
    fn an_account_admitted_and_away_holds_one_slot() {
        assert_eq!(held_sessions([7, 7, 7]), 1);
    }

    #[test]
    fn a_ticket_waits_while_its_account_is_away_even_with_free_capacity() {
        assert!(!admits(&session_limit(0), 0, 0, true));
        assert!(admits(&session_limit(0), 0, 0, false));
    }
}
