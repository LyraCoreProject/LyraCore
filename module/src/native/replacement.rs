//! A newer connection for an Account replaces the Account's World Session that is away on a World
//! Shard. See docs/architecture.md#host-listeners-and-admission for the wait, its bound and the
//! end that stays owed past it.

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
/// An owed end goes out again after this, as a refused Transfer release does.
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

/// How long tickets of the Account wait for the away World Session `call` names; a newer ticket
/// keeps the time left. It fires at the deadline and gives up only while a ticket waits.
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

/// An end this Realm Shard owes `world` for one connection. Every 30 seconds it goes out again
/// unless an earlier send to `world` still waits for its answer. It lasts until `world` accepts
/// it, reports the World Session closed, or stops being a Peer Shard.
#[table(accessor = game_world_session_owed_end, scheduled(resend_world_session_end))]
pub struct WorldSessionOwedEnd {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    #[unique]
    pub connection: u128,
    #[index(btree)]
    pub account_id: u64,
    pub world: Identity,
}
crate::virtual_clock_exempt!(game_world_session_owed_end);

crate::outside_table_catalog!(
    game_world_session_end,
    game_world_session_end_retry,
    game_world_session_owed_end
);

#[derive(Debug, PartialEq, Eq)]
enum Replacement {
    /// Owe this World Shard the end of the away World Session and wait for it.
    Wait(Identity),
    /// No Peer World Shard can end it; stop waiting.
    GiveUp,
}

fn replacement(world: Option<Identity>, peer: impl Fn(Identity) -> bool) -> Replacement {
    match world {
        Some(world) if peer(world) => Replacement::Wait(world),
        _ => Replacement::GiveUp,
    }
}

/// Whether an earlier World Session of the Account is still away on a World Shard. The first call
/// for an away World Session owes that World Shard its end and starts the bound. A World Session
/// on its way back to this Shard is replaced at once.
pub(super) fn replacing(ctx: &ReducerContext, account_id: u64) -> bool {
    let waits = ctx.db.game_world_session_end_retry();
    let Some(away) = ctx.db.game_native_away().account_id().find(account_id) else {
        handoff::abandon_account(ctx, account_id);
        waits.account_id().delete(account_id);
        return false;
    };
    let world = match replacement(away.world.or_else(|| world_shard(ctx)), |world| {
        is_peer(ctx, world)
    }) {
        Replacement::Wait(world) => world,
        Replacement::GiveUp => {
            give_up(ctx, &away);
            return false;
        }
    };
    let connection = away.connection_id.to_u128();
    owe(ctx, world, connection, account_id);
    if waits
        .account_id()
        .find(account_id)
        .is_none_or(|wait| wait.call.connection != connection)
    {
        waits.account_id().delete(account_id);
        let deadline = ctx.timestamp + TimeDuration::from_micros(END_MICROS);
        waits.insert(WorldSessionEndRetry {
            scheduled_id: 0,
            scheduled_at: deadline.into(),
            account_id,
            call: WorldSessionEnd {
                id: 0,
                database: remote::Identity(world),
                connection,
                account_id,
            },
            deadline,
        });
    }
    true
}

/// Owes `world` the end of the away World Session, without a ticket that waits for it.
pub(super) fn end_away(ctx: &ReducerContext, world: Identity, away: &NativeAway) {
    owe(ctx, world, away.connection_id.to_u128(), away.account_id);
}

/// Sends `world` the end of `connection` unless it is already owed to `world`. An end owed to
/// another World Shard follows the World Session to `world`.
fn owe(ctx: &ReducerContext, world: Identity, connection: u128, account_id: u64) {
    let owed = ctx.db.game_world_session_owed_end();
    if owed
        .connection()
        .find(connection)
        .is_some_and(|end| end.world == world)
    {
        return;
    }
    owed.connection().delete(connection);
    owed.insert(WorldSessionOwedEnd {
        scheduled_id: 0,
        scheduled_at: (ctx.timestamp + TimeDuration::from_micros(RESEND_MICROS)).into(),
        connection,
        account_id,
        world,
    });
    send(ctx, world, connection, account_id);
}

fn send(ctx: &ReducerContext, world: Identity, connection: u128, account_id: u64) {
    ctx.db.game_world_session_end().insert(WorldSessionEnd {
        id: 0,
        database: remote::Identity(world),
        connection,
        account_id,
    });
}

/// A Character Transfer moved `connection` from `source` to `target`; an end owed to the source
/// is owed to the target from its next send. The move arrives before the Transfer release, so the
/// target would refuse an end sent now.
pub(super) fn moved(ctx: &ReducerContext, connection: u128, source: Identity, target: Identity) {
    let owed = ctx.db.game_world_session_owed_end();
    if let Some(mut end) = owed
        .connection()
        .find(connection)
        .filter(|end| end.world == source)
    {
        end.world = target;
        owed.scheduled_id().update(end);
    }
}

/// `world` no longer holds the World Session of `connection`.
pub(super) fn closed(ctx: &ReducerContext, connection: u128, world: Identity) {
    let owed = ctx.db.game_world_session_owed_end();
    if owed
        .connection()
        .find(connection)
        .is_some_and(|end| end.world == world)
    {
        owed.connection().delete(connection);
    }
}

/// The World Shards that this Realm Shard still owes the end of a World Session of the Account.
pub(super) fn owed_worlds(ctx: &ReducerContext, account_id: u64) -> Vec<Identity> {
    ctx.db
        .game_world_session_owed_end()
        .account_id()
        .filter(account_id)
        .map(|end| end.world)
        .collect()
}

/// Stops waiting for the away World Session; an end owed for it stays owed.
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
    match result {
        Ok(()) => closed(ctx, call.connection, call.database.0),
        Err(error) => {
            spacetimedb::log::warn!("World Shard refused to end a World Session: {error}")
        }
    }
}

#[reducer]
pub fn resend_world_session_end(
    ctx: &ReducerContext,
    end: WorldSessionOwedEnd,
) -> Result<(), String> {
    if ctx.sender() != ctx.database_identity() {
        return Err("scheduler only".into());
    }
    let owed = ctx.db.game_world_session_owed_end();
    owed.scheduled_id().delete(end.scheduled_id);
    if !is_peer(ctx, end.world) {
        spacetimedb::log::warn!(
            "an end owed for Account {} stops: its World Shard is not a Peer Shard",
            end.account_id
        );
        return Ok(());
    }
    let answered = !ctx
        .db
        .game_world_session_end()
        .iter()
        .any(|call| call.connection == end.connection && call.database.0 == end.world);
    let end = owed.insert(WorldSessionOwedEnd {
        scheduled_id: 0,
        scheduled_at: (ctx.timestamp + TimeDuration::from_micros(RESEND_MICROS)).into(),
        ..end
    });
    if answered {
        send(ctx, end.world, end.connection, end.account_id);
    }
    Ok(())
}

#[reducer]
pub fn retry_world_session_end(
    ctx: &ReducerContext,
    retry: WorldSessionEndRetry,
) -> Result<(), String> {
    if ctx.sender() != ctx.database_identity() {
        return Err("scheduler only".into());
    }
    let waits = ctx.db.game_world_session_end_retry();
    waits.scheduled_id().delete(retry.scheduled_id);
    let Some(away) = ctx
        .db
        .game_native_away()
        .account_id()
        .find(retry.account_id)
        .filter(|away| {
            away.connection_id.to_u128() == retry.call.connection
                && super::admission::waiting(ctx, retry.account_id)
        })
    else {
        return Ok(());
    };
    if ctx.timestamp >= retry.deadline {
        give_up(ctx, &away);
    } else {
        // A row the previous Module wrote fires at its resend time and waits on to its deadline.
        waits.insert(WorldSessionEndRetry {
            scheduled_id: 0,
            scheduled_at: retry.deadline.into(),
            ..retry
        });
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

/// Owes this Realm Shard's World Shard the end of a connection, as a late or refused end does.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_end_world_session(
    ctx: &ReducerContext,
    connection_id: ConnectionId,
    account_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let world = world_shard(ctx).ok_or("this Shard has no World Shard")?;
    owe(ctx, world, connection_id.to_u128(), account_id);
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
    fn a_ticket_stops_waiting_for_a_world_shard_that_is_not_a_peer() {
        assert_eq!(
            replacement(Some(world()), |_| true),
            Replacement::Wait(world())
        );
        assert_eq!(replacement(Some(world()), |_| false), Replacement::GiveUp);
        assert_eq!(replacement(None, |_| true), Replacement::GiveUp);
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
