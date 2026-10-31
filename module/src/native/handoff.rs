use spacetimedb::{reducer, table, ConnectionId, Identity, ReducerContext, SpacetimeType, Table};

use super::{
    game_native_cipher, game_native_world_connection, NativeCipher, NativeWorldConnection,
    NativeWorldState,
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
}

// ponytail: only Character selection moves; entering the world on arrival needs the Character's Transfer.
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
    super::current_account(ctx, &connection, account.id)?;
    Ok(())
}

/// Moves the World Session of `account_id` to `database`. The PONG before the marker reaches the
/// client; the PONG after it must not.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_native_handoff(
    ctx: &ReducerContext,
    account_id: u64,
    database: Identity,
) -> Result<(), String> {
    use super::game_native_packet;
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
    pong(0xaf7e)
}

/// Records the World Session that another Shard hands to this one, for the Account named `username`.
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
    });
    Ok(())
}
