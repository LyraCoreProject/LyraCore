use spacetimedb::{reducer, table, ConnectionId, Identity, ReducerContext, SpacetimeType, Table};

use super::transfer::game_native_transfer_pending;
use super::{
    game_native_cipher, game_native_packet, game_native_world_connection, NativeCipher,
    NativeWorldConnection, NativeWorldState,
};
use crate::transfer::game_transfer_in;
use crate::{game_account, game_session};

#[derive(SpacetimeType)]
pub struct NativeHandoffTarget {
    pub connection_id: ConnectionId,
    pub database: Identity,
}

/// A World Session that this Shard adopts when the Native Host attaches its connection.
#[table(accessor = game_native_handoff)]
pub struct NativeHandoff {
    #[primary_key]
    pub connection_id: ConnectionId,
    pub server_seed: u32,
    pub account_id: u64,
    pub client_seed: u32,
    pub client_proof: Vec<u8>,
    /// The Character the World Session enters on arrival; without one it arrives at selection.
    #[default(None::<u64>)]
    pub character_guid: Option<u64>,
    #[default(false)]
    pub world_port: bool,
    #[default(0u32)]
    pub teleport_counter: u32,
    #[default(0u64)]
    pub transfer_id: u64,
    #[default(None::<Identity>)]
    pub transfer_source: Option<Identity>,
}

crate::outside_table_catalog!(game_native_handoff);

/// How long a Handoff waits for its connection. It outlasts the Realm Shard's entering window, so
/// a late world entry answer cannot write a Handoff marker for a Handoff that has already expired.
const ADOPTION_MICROS: i64 = 30_000_000;

/// Writes the Handoff that waits for its connection, replacing an earlier one.
pub(super) fn expect(ctx: &ReducerContext, handoff: NativeHandoff, request_opcode: u32) {
    let connection_id = handoff.connection_id;
    let handoffs = ctx.db.game_native_handoff();
    handoffs.connection_id().delete(connection_id);
    handoffs.insert(handoff);
    await_adoption(ctx, connection_id, request_opcode);
}

/// Gives the connection's Handoff its adoption deadline: a `game_pending_selection` row for
/// `request_opcode`, the client request that started the Handoff.
pub(super) fn await_adoption(
    ctx: &ReducerContext,
    connection_id: ConnectionId,
    request_opcode: u32,
) {
    super::realm_shard::record_pending(
        ctx,
        connection_id,
        request_opcode,
        Vec::new(),
        ADOPTION_MICROS,
    );
}

/// Drops the Handoff that waits for this connection and reports its World Session closed to the
/// Realm Shard. A Handoff that its Transfer still fences waits for the Transfer release.
pub(super) fn abandon(ctx: &ReducerContext, connection_id: ConnectionId) {
    let handoffs = ctx.db.game_native_handoff();
    let Some(handoff) = handoffs.connection_id().find(connection_id) else {
        return;
    };
    if fenced(ctx, &handoff) {
        return;
    }
    handoffs.connection_id().delete(connection_id);
    super::realm_shard::cancel_selection(ctx, connection_id);
    if let Some(guid) = handoff
        .character_guid
        .filter(|guid| handoff.transfer_id != 0 && !super::has_world_session(ctx, *guid))
    {
        super::transfer::clear_pending_playtest(ctx, guid);
    }
    super::realm_shard::report_closed(ctx, connection_id, handoff.account_id);
}

/// Whether a Character Transfer still fences the Handoff, until the Transfer release.
pub(super) fn fenced(ctx: &ReducerContext, handoff: &NativeHandoff) -> bool {
    handoff.transfer_id != 0
        && ctx
            .db
            .game_transfer_in()
            .transfer_id()
            .find(handoff.transfer_id)
            .is_some()
}

pub(super) fn abandon_account(ctx: &ReducerContext, account_id: u64) {
    let waiting: Vec<ConnectionId> = ctx
        .db
        .game_native_handoff()
        .iter()
        .filter(|handoff| handoff.account_id == account_id)
        .map(|handoff| handoff.connection_id)
        .collect();
    for connection_id in waiting {
        abandon(ctx, connection_id);
    }
}

/// The Accounts whose World Session waits for adoption on this Shard.
pub(super) fn waiting_accounts(ctx: &ReducerContext) -> Vec<u64> {
    ctx.db
        .game_native_handoff()
        .iter()
        .map(|handoff| handoff.account_id)
        .collect()
}

pub(super) fn write_marker(ctx: &ReducerContext, connection_id: ConnectionId, database: Identity) {
    ctx.db.game_native_packet().insert(super::NativePacket {
        id: 0,
        audience: super::NativeAudience::Handoff(NativeHandoffTarget {
            connection_id,
            database,
        }),
        opcode: None,
        body: Vec::new(),
        close_after: false,
        not_before_micros: 0,
        authentication_complete: false,
    });
}

#[reducer]
pub fn native_world_adopt(ctx: &ReducerContext) -> Result<(), String> {
    let connection_id = super::connection_id(ctx)?;
    let handoffs = ctx.db.game_native_handoff();
    let handoff = handoffs
        .connection_id()
        .find(connection_id)
        .ok_or("no Handoff waits for this connection")?;
    handoffs.connection_id().delete(connection_id);
    super::realm_shard::cancel_selection(ctx, connection_id);
    ctx.db
        .game_native_transfer_pending()
        .connection_id()
        .delete(connection_id);
    let account = ctx
        .db
        .game_account()
        .id()
        .find(handoff.account_id)
        .ok_or("Account no longer exists")?;
    let session = ctx
        .db
        .game_session()
        .account_id()
        .find(account.id)
        .ok_or("Session no longer exists")?;
    ctx.db.game_native_cipher().insert(NativeCipher {
        connection_id,
        username: account.username,
        session_key: session.session_key,
        client_seed: handoff.client_seed,
        client_proof: handoff.client_proof,
    });
    let connection = ctx
        .db
        .game_native_world_connection()
        .insert(NativeWorldConnection {
            connection_id,
            server_seed: handoff.server_seed,
            state: NativeWorldState::CharacterSelection {
                account_id: account.id,
            },
            unavailable_notices: 0,
            chat_flood: None,
            who_until_micros: 0,
            group_broadcasts: None,
            addon_limit: None,
        });
    match handoff.character_guid {
        Some(guid) => super::world::enter_character(
            ctx,
            connection,
            account.id,
            guid,
            if handoff.world_port {
                crate::character::WorldEntry::WorldPort
            } else {
                crate::character::WorldEntry::Login
            },
            handoff.teleport_counter,
        ),
        None => {
            super::current_account(ctx, &connection, account.id)?;
            if crate::peer::world_shard(ctx).is_some() {
                if !super::admission::returned(ctx, connection_id, account.id) {
                    super::disconnect(ctx, connection_id);
                    return Ok(());
                }
                for message in lyracore_protocol::codec::logout_sequence() {
                    super::send(ctx, connection_id, message, false)?;
                }
            }
            Ok(())
        }
    }
}

/// Sends PONGs around a Handoff marker; only the first reaches the client.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_native_handoff(
    ctx: &ReducerContext,
    account_id: u64,
    database: Identity,
) -> Result<(), String> {
    use lyracore_protocol::messages::vanilla::{opcodes::ServerOpcodeMessage, SMSG_PONG};

    crate::helpers::require_operator(ctx)?;
    let connection_id = ctx
        .db
        .game_native_world_connection()
        .iter()
        .find(|connection| {
            matches!(connection.state, NativeWorldState::CharacterSelection { account_id: id } if id == account_id)
        })
        .ok_or("no World Session of this Account is at Character selection")?
        .connection_id;
    let pong = |sequence_id| {
        super::send(
            ctx,
            connection_id,
            ServerOpcodeMessage::SMSG_PONG(SMSG_PONG { sequence_id }),
            false,
        )
    };
    pong(0xbef0)?;
    write_marker(ctx, connection_id, database);
    pong(0xaf7e)
}

/// Prepares a selection Handoff for the named Account.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_native_expect_handoff(
    ctx: &ReducerContext,
    connection_id: ConnectionId,
    server_seed: u32,
    username: String,
    client_seed: u32,
    client_proof: Vec<u8>,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let account = ctx
        .db
        .game_account()
        .username()
        .find(username.to_ascii_uppercase())
        .ok_or("Account no longer exists")?;
    ctx.db.game_native_handoff().insert(NativeHandoff {
        connection_id,
        server_seed,
        account_id: account.id,
        client_seed,
        client_proof,
        character_guid: None,
        world_port: false,
        teleport_counter: 0,
        transfer_id: 0,
        transfer_source: None,
    });
    Ok(())
}
