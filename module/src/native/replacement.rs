//! A newer connection for an Account replaces the Account's World Session that is away on a World
//! Shard. See docs/architecture.md#host-listeners-and-admission for the wait and its bound.

use spacetimedb::{
    reducer, table, ConnectionId, Identity, ReducerContext, ScheduleAt, Table, TimeDuration,
    Timestamp,
};

use super::handoff::{self, game_native_handoff};
use super::realm_shard::{self, game_native_away, NativeAway};
use super::{game_native_world_connection, NativeActive, NativeWorldState};
use crate::peer::{is_peer, realm_shard as realm, require_sender, world_shard};
use crate::remote_bindings::peer as remote;

/// How long a newer ticket waits for the away World Session to end: two resends of a refused end.
const END_MICROS: i64 = 90_000_000;
/// A refused end waits before it is sent again, as a refused Transfer release does.
const RESEND_MICROS: i64 = 30_000_000;

#[table(accessor = game_world_session_end, outbox(remote::idc_end_world_session))]
pub struct WorldSessionEnd {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub connection: u128,
    pub account_id: u64,
}

/// The end that a newer ticket of the Account waits for. It fires to resend a refused end, and at
/// the deadline, when the Realm Shard stops waiting.
#[table(accessor = game_world_session_end_retry, scheduled(retry_world_session_end))]
pub struct WorldSessionEndRetry {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    #[unique]
    pub account_id: u64,
    pub call: WorldSessionEnd,
    pub deadline: Timestamp,
}
crate::virtual_clock_exempt!(game_world_session_end_retry);

crate::outside_table_catalog!(game_world_session_end, game_world_session_end_retry);

#[derive(Debug, PartialEq, Eq)]
enum Replacement {
    /// Ask the World Shard to end the away World Session.
    Send(Identity),
    /// The end is already asked for.
    Wait,
    /// No Peer World Shard can end it; stop waiting.
    GiveUp,
}

/// `sent` is the connection and World Shard of the end already asked for.
fn replacement(
    away_connection: u128,
    world: Option<Identity>,
    peer: impl Fn(Identity) -> bool,
    sent: Option<(u128, Identity)>,
) -> Replacement {
    match world {
        Some(world) if peer(world) => {
            if sent == Some((away_connection, world)) {
                Replacement::Wait
            } else {
                Replacement::Send(world)
            }
        }
        _ => Replacement::GiveUp,
    }
}

/// Whether an earlier World Session of the Account is still away on a World Shard. The first call
/// for an away World Session asks that World Shard to end it. A World Session on its way back to
/// this Shard is replaced at once.
pub(super) fn replacing(ctx: &ReducerContext, account_id: u64) -> bool {
    let Some(away) = ctx.db.game_native_away().account_id().find(account_id) else {
        handoff::abandon_account(ctx, account_id);
        return false;
    };
    let sent = ctx
        .db
        .game_world_session_end_retry()
        .account_id()
        .find(account_id)
        .map(|retry| (retry.call.connection, retry.call.database.0));
    match replacement(
        away.connection_id.to_u128(),
        away.world.or_else(|| world_shard(ctx)),
        |world| is_peer(ctx, world),
        sent,
    ) {
        Replacement::Send(world) => {
            let deadline = ctx.timestamp + TimeDuration::from_micros(END_MICROS);
            send(ctx, end_of(world, &away), deadline);
            true
        }
        Replacement::Wait => true,
        Replacement::GiveUp => {
            give_up(ctx, &away);
            false
        }
    }
}

/// Asks `world` to end the away World Session, without a ticket that waits for it.
pub(super) fn end_away(ctx: &ReducerContext, world: Identity, away: &NativeAway) {
    ctx.db.game_world_session_end().insert(end_of(world, away));
}

fn end_of(world: Identity, away: &NativeAway) -> WorldSessionEnd {
    WorldSessionEnd {
        id: 0,
        database: remote::Identity(world),
        connection: away.connection_id.to_u128(),
        account_id: away.account_id,
    }
}

fn send(ctx: &ReducerContext, call: WorldSessionEnd, deadline: Timestamp) {
    let retries = ctx.db.game_world_session_end_retry();
    retries.account_id().delete(call.account_id);
    retries.insert(WorldSessionEndRetry {
        scheduled_id: 0,
        scheduled_at: deadline.into(),
        account_id: call.account_id,
        call: WorldSessionEnd { id: 0, ..call },
        deadline,
    });
    ctx.db
        .game_world_session_end()
        .insert(WorldSessionEnd { id: 0, ..call });
}

fn give_up(ctx: &ReducerContext, away: &NativeAway) {
    spacetimedb::log::warn!(
        "Account {} stops waiting for its away World Session to end",
        away.account_id
    );
    crate::meeting_stone::remove_solo_seeker(ctx, away.character_guid);
    ctx.db
        .game_native_away()
        .account_id()
        .delete(away.account_id);
    ctx.db
        .game_world_session_end_retry()
        .account_id()
        .delete(away.account_id);
}

#[reducer(on_result(game_world_session_end))]
pub fn world_session_end_answered(
    ctx: &ReducerContext,
    call: WorldSessionEnd,
    result: Result<(), String>,
) {
    let Err(error) = result else {
        return;
    };
    spacetimedb::log::warn!("World Shard refused to end a World Session: {error}");
    let retries = ctx.db.game_world_session_end_retry();
    let Some(retry) = retries.account_id().find(call.account_id).filter(|retry| {
        retry.call.connection == call.connection && retry.call.database == call.database
    }) else {
        return;
    };
    let resend = ctx.timestamp + TimeDuration::from_micros(RESEND_MICROS);
    let resend = if resend < retry.deadline {
        resend
    } else {
        retry.deadline
    };
    retries.account_id().delete(call.account_id);
    retries.insert(WorldSessionEndRetry {
        scheduled_id: 0,
        scheduled_at: resend.into(),
        ..retry
    });
}

#[reducer]
pub fn retry_world_session_end(
    ctx: &ReducerContext,
    retry: WorldSessionEndRetry,
) -> Result<(), String> {
    if ctx.sender() != ctx.database_identity() {
        return Err("scheduler only".into());
    }
    ctx.db
        .game_world_session_end_retry()
        .scheduled_id()
        .delete(retry.scheduled_id);
    let Some(away) = ctx
        .db
        .game_native_away()
        .account_id()
        .find(retry.account_id)
        .filter(|away| {
            away.connection_id.to_u128() == retry.call.connection
                && away.world.or_else(|| world_shard(ctx)) == Some(retry.call.database.0)
        })
    else {
        return Ok(());
    };
    if ctx.timestamp >= retry.deadline {
        give_up(ctx, &away);
    } else {
        send(ctx, retry.call, retry.deadline);
    }
    Ok(())
}

/// What a World Shard does with an end for one connection.
#[derive(Debug, PartialEq, Eq)]
enum End {
    /// End the open connection; `report` when its close does not report it already.
    Disconnect { report: bool },
    /// Abandon its Handoff, which reports it.
    Abandon,
    /// A Character Transfer still fences its Handoff; refuse, and the Realm Shard resends.
    Refuse,
    /// Nothing of it is here; report it closed.
    Report,
}

/// `open` is the state of the connection on this Shard, `handoff` the Account of its Handoff and
/// whether a Character Transfer fences it.
fn world_session_end(
    open: Option<&NativeWorldState>,
    handoff: Option<(u64, bool)>,
    account_id: u64,
) -> End {
    if let Some(state) = open.filter(|state| state.account_id() == Some(account_id)) {
        return End::Disconnect {
            report: !matches!(state, NativeWorldState::InWorld(NativeActive { .. })),
        };
    }
    match handoff {
        Some((handoff_account, true)) if handoff_account == account_id => End::Refuse,
        Some((handoff_account, false)) if handoff_account == account_id => End::Abandon,
        _ => End::Report,
    }
}

/// Ends this World Shard's World Session or Handoff of the connection, and nothing else of the
/// Account. The Realm Shard learns that the World Session is closed, even when nothing of it was
/// here.
#[reducer]
pub fn idc_end_world_session(
    ctx: &ReducerContext,
    connection: u128,
    account_id: u64,
) -> Result<(), String> {
    require_sender(ctx, realm(ctx))?;
    let connection_id = ConnectionId::from_u128(connection);
    let open = ctx
        .db
        .game_native_world_connection()
        .connection_id()
        .find(connection_id)
        .map(|connection| connection.state);
    let handoff = ctx
        .db
        .game_native_handoff()
        .connection_id()
        .find(connection_id)
        .map(|handoff| (handoff.account_id, handoff::fenced(ctx, &handoff)));
    match world_session_end(open.as_ref(), handoff, account_id) {
        End::Disconnect { report } => {
            super::disconnect(ctx, connection_id);
            if report {
                realm_shard::report_closed(ctx, connection_id, account_id);
            }
        }
        End::Abandon => handoff::abandon(ctx, connection_id),
        End::Refuse => return Err("a Character Transfer still fences this World Session".into()),
        End::Report => realm_shard::report_closed(ctx, connection_id, account_id),
    }
    Ok(())
}

/// Sends this Realm Shard's World Shard an end for a connection, as a late duplicate arrives.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_end_world_session(
    ctx: &ReducerContext,
    connection_id: ConnectionId,
    account_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let world = world_shard(ctx).ok_or("this Shard has no World Shard")?;
    ctx.db.game_world_session_end().insert(WorldSessionEnd {
        id: 0,
        database: remote::Identity(world),
        connection: connection_id.to_u128(),
        account_id,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCOUNT: u64 = 7;

    fn world() -> Identity {
        Identity::from_byte_array([1; 32])
    }

    #[test]
    fn a_newer_ticket_asks_once_for_each_away_world_session() {
        assert_eq!(
            replacement(5, Some(world()), |_| true, None),
            Replacement::Send(world())
        );
        assert_eq!(
            replacement(5, Some(world()), |_| true, Some((5, world()))),
            Replacement::Wait
        );
        let moved = Identity::from_byte_array([2; 32]);
        assert_eq!(
            replacement(5, Some(moved), |_| true, Some((5, world()))),
            Replacement::Send(moved)
        );
        assert_eq!(
            replacement(6, Some(world()), |_| true, Some((5, world()))),
            Replacement::Send(world())
        );
    }

    #[test]
    fn a_ticket_stops_waiting_for_a_world_shard_that_is_not_a_peer() {
        assert_eq!(
            replacement(5, Some(world()), |_| false, Some((5, world()))),
            Replacement::GiveUp
        );
        assert_eq!(replacement(5, None, |_| true, None), Replacement::GiveUp);
    }

    #[test]
    fn an_end_targets_only_its_own_connection() {
        let in_world = NativeWorldState::InWorld(NativeActive {
            account_id: ACCOUNT,
            character_guid: 9,
        });
        assert_eq!(
            world_session_end(Some(&in_world), None, ACCOUNT),
            End::Disconnect { report: false }
        );
        let other = NativeWorldState::InWorld(NativeActive {
            account_id: ACCOUNT + 1,
            character_guid: 9,
        });
        assert_eq!(
            world_session_end(Some(&other), Some((ACCOUNT + 1, false)), ACCOUNT),
            End::Report
        );
    }

    #[test]
    fn an_end_always_reports_the_world_session_closed() {
        let selection = NativeWorldState::CharacterSelection {
            account_id: ACCOUNT,
        };
        assert_eq!(
            world_session_end(Some(&selection), None, ACCOUNT),
            End::Disconnect { report: true }
        );
        assert_eq!(
            world_session_end(None, Some((ACCOUNT, false)), ACCOUNT),
            End::Abandon
        );
        assert_eq!(world_session_end(None, None, ACCOUNT), End::Report);
    }

    #[test]
    fn an_end_waits_for_a_transfer_that_fences_the_handoff() {
        assert_eq!(
            world_session_end(None, Some((ACCOUNT, true)), ACCOUNT),
            End::Refuse
        );
    }
}
