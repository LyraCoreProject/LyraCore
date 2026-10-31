use spacetimedb::{reducer, table, ConnectionId, Identity, ReducerContext, SpacetimeType, Table};

use super::{
    game_native_cipher, game_native_packet, game_native_world_connection, NativeCipher,
    NativeWorldConnection, NativeWorldState,
};
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
                crate::world::WorldEntry::WorldPort
            } else {
                crate::world::WorldEntry::Login
            },
            handoff.teleport_counter,
        ),
        None => super::current_account(ctx, &connection, account.id).map(drop),
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
