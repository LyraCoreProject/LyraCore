//! A Realm split into a Realm Shard and a World Shard. The Realm Shard keeps logon, Accounts,
//! Sessions and the Login Queue. Character selection and the world run on the World Shard; the
//! Realm Shard relays selection requests to it and hands World Sessions to it to enter the world.

use spacetimedb::sats::bsatn;
use spacetimedb::{reducer, table, ConnectionId, ReducerContext, Table};

use super::handoff::{game_native_handoff, NativeHandoff};
use super::{
    game_native_cipher, game_native_world_connection, NativeActive, NativeWorldConnection,
    NativeWorldState, Rollback,
};
use crate::auth::{Account, Session};
use crate::peer::{realm_shard, require_sender, world_shard};
use crate::remote_bindings::peer as remote;
use crate::{game_account, game_session};

/// An Account whose World Session this Realm Shard handed to its World Shard.
#[table(accessor = game_native_away)]
pub struct NativeAway {
    #[primary_key]
    pub account_id: u64,
    pub connection_id: ConnectionId,
}

pub(crate) fn away_accounts(ctx: &ReducerContext) -> Vec<u64> {
    ctx.db
        .game_native_away()
        .iter()
        .map(|away| away.account_id)
        .collect()
}

fn at_selection(
    ctx: &ReducerContext,
    connection_id: ConnectionId,
) -> Option<NativeWorldConnection> {
    ctx.db
        .game_native_world_connection()
        .connection_id()
        .find(connection_id)
        .filter(|connection| {
            matches!(
                connection.state,
                NativeWorldState::CharacterSelection { .. }
            )
        })
}

fn blob<T: spacetimedb::Serialize>(row: &T) -> Vec<u8> {
    bsatn::to_vec(row).expect("a table row encodes")
}

// The World Shard keeps copies of the Accounts and Sessions of the World Sessions it holds.
fn store_account(ctx: &ReducerContext, account: &[u8]) -> Result<Account, String> {
    let account: Account = bsatn::from_slice(account).map_err(|error| error.to_string())?;
    let accounts = ctx.db.game_account();
    accounts.id().delete(account.id);
    accounts.username().delete(&account.username);
    Ok(accounts.insert(account))
}

fn store_session(ctx: &ReducerContext, session: &[u8]) -> Result<(), String> {
    let session: Session = bsatn::from_slice(session).map_err(|error| error.to_string())?;
    let sessions = ctx.db.game_session();
    sessions.account_id().delete(session.account_id);
    sessions.insert(session);
    Ok(())
}

// Realm Shard → World Shard.

#[table(accessor = game_selection_request, outbox(remote::idc_selection_request))]
pub struct SelectionRequest {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub connection: u128,
    pub account: Vec<u8>,
    pub opcode: u32,
    pub body: Vec<u8>,
}

pub(super) fn relay_selection(
    ctx: &ReducerContext,
    connection_id: ConnectionId,
    world: spacetimedb::Identity,
    account_id: u64,
    opcode: u32,
    body: Vec<u8>,
) -> Result<(), String> {
    let account = ctx
        .db
        .game_account()
        .id()
        .find(account_id)
        .ok_or("Account no longer exists")?;
    ctx.db.game_selection_request().insert(SelectionRequest {
        id: 0,
        database: remote::Identity(world),
        connection: connection_id.to_u128(),
        account: blob(&account),
        opcode,
        body,
    });
    Ok(())
}

// A refused request rolls back on the World Shard; its error carries the refusal packets.
#[reducer(on_result(game_selection_request))]
pub fn selection_answered(
    ctx: &ReducerContext,
    request: SelectionRequest,
    result: Result<(), String>,
) {
    let connection_id = ConnectionId::from_u128(request.connection);
    let Err(error) = result else {
        return;
    };
    let Some(packets) = super::rollback::replies_in(&error) else {
        spacetimedb::log::warn!("World Shard refused a selection request: {error}");
        return;
    };
    if at_selection(ctx, connection_id).is_none() {
        return;
    }
    for (opcode, body) in packets {
        super::send_raw(ctx, connection_id, opcode, body, false);
    }
}

#[table(accessor = game_world_entry, outbox(remote::idc_world_entry))]
pub struct WorldEntry {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub connection: u128,
    pub server_seed: u32,
    pub account: Vec<u8>,
    pub session: Vec<u8>,
    pub client_seed: u32,
    pub client_proof: Vec<u8>,
    pub character_guid: u64,
}

pub(super) fn enter_world(
    ctx: &ReducerContext,
    connection: NativeWorldConnection,
    world: spacetimedb::Identity,
    body: &[u8],
) -> Result<(), Rollback> {
    let NativeWorldState::CharacterSelection { account_id } = connection.state else {
        return Err("character login requires character selection".into());
    };
    let character_guid = u64::from_le_bytes(
        body.try_into()
            .map_err(|_| "invalid character login packet")?,
    );
    let account = ctx
        .db
        .game_account()
        .id()
        .find(account_id)
        .ok_or("Account no longer exists")?;
    let session = ctx
        .db
        .game_session()
        .account_id()
        .find(account_id)
        .ok_or("Session no longer exists")?;
    let cipher = ctx
        .db
        .game_native_cipher()
        .connection_id()
        .find(connection.connection_id)
        .ok_or("world proof is missing")?;
    ctx.db.game_world_entry().insert(WorldEntry {
        id: 0,
        database: remote::Identity(world),
        connection: connection.connection_id.to_u128(),
        server_seed: connection.server_seed,
        account: blob(&account),
        session: blob(&session),
        client_seed: cipher.client_seed,
        client_proof: cipher.client_proof,
        character_guid,
    });
    Ok(())
}

#[reducer(on_result(game_world_entry))]
pub fn world_entry_answered(ctx: &ReducerContext, entry: WorldEntry, result: Result<(), String>) {
    let connection_id = ConnectionId::from_u128(entry.connection);
    let Some(connection) = at_selection(ctx, connection_id) else {
        return;
    };
    let NativeWorldState::CharacterSelection { account_id } = connection.state else {
        return;
    };
    if let Err(error) = result {
        spacetimedb::log::debug!("World Shard refused world entry: {error}");
        let _ = super::send(
            ctx,
            connection_id,
            lyracore_protocol::messages::vanilla::opcodes::ServerOpcodeMessage::SMSG_CHARACTER_LOGIN_FAILED(
                lyracore_protocol::codec::build_character_login_failed(),
            ),
            false,
        );
        return;
    }
    let aways = ctx.db.game_native_away();
    aways.account_id().delete(account_id);
    aways.insert(NativeAway {
        account_id,
        connection_id,
    });
    super::handoff::write_marker(ctx, connection_id, entry.database.0);
}

#[table(accessor = game_account_update, outbox(remote::idc_account_update))]
pub struct AccountUpdate {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub account: Vec<u8>,
    pub session: Vec<u8>,
}

/// Sends the World Shard a changed Account or Session of a World Session it holds, so its next
/// request checks the current one.
pub(crate) fn account_changed(ctx: &ReducerContext, account_id: u64) {
    let Some(world) = world_shard(ctx) else {
        return;
    };
    if ctx
        .db
        .game_native_away()
        .account_id()
        .find(account_id)
        .is_none()
    {
        return;
    }
    let (Some(account), Some(session)) = (
        ctx.db.game_account().id().find(account_id),
        ctx.db.game_session().account_id().find(account_id),
    ) else {
        return;
    };
    ctx.db.game_account_update().insert(AccountUpdate {
        id: 0,
        database: remote::Identity(world),
        account: blob(&account),
        session: blob(&session),
    });
}

#[reducer]
pub fn idc_selection_request(
    ctx: &ReducerContext,
    connection: u128,
    account: Vec<u8>,
    opcode: u32,
    body: Vec<u8>,
) -> Result<(), String> {
    require_sender(ctx, realm_shard(ctx))?;
    let account = store_account(ctx, &account)?;
    let reply = super::selection_reply(ctx, account.id, opcode, &body)
        .map_err(Rollback::into_reducer_error)?;
    if let Some(message) = reply {
        let packet =
            lyracore_protocol::world::encode(message).map_err(|error| error.to_string())?;
        ctx.db.game_selection_reply().insert(SelectionReply {
            id: 0,
            database: remote::Identity(ctx.sender()),
            connection,
            opcode: packet.opcode,
            body: packet.body,
        });
    }
    Ok(())
}

#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn idc_world_entry(
    ctx: &ReducerContext,
    connection: u128,
    server_seed: u32,
    account: Vec<u8>,
    session: Vec<u8>,
    client_seed: u32,
    client_proof: Vec<u8>,
    character_guid: u64,
) -> Result<(), String> {
    require_sender(ctx, realm_shard(ctx))?;
    let account = store_account(ctx, &account)?;
    store_session(ctx, &session)?;
    let character = crate::helpers::character_by_guid(ctx, character_guid)
        .ok_or("Character no longer exists")?;
    if character.account_id != account.id || account.banned {
        return Err("Account cannot log in this Character".into());
    }
    let connection_id = ConnectionId::from_u128(connection);
    let handoffs = ctx.db.game_native_handoff();
    handoffs.connection_id().delete(connection_id);
    handoffs.insert(NativeHandoff {
        connection_id,
        server_seed,
        account_id: account.id,
        client_seed,
        client_proof,
        character_guid: Some(character_guid),
    });
    Ok(())
}

#[reducer]
pub fn idc_account_update(
    ctx: &ReducerContext,
    account: Vec<u8>,
    session: Vec<u8>,
) -> Result<(), String> {
    require_sender(ctx, realm_shard(ctx))?;
    store_account(ctx, &account)?;
    store_session(ctx, &session)
}

// World Shard → Realm Shard.

#[table(accessor = game_selection_reply, outbox(remote::idc_selection_reply))]
pub struct SelectionReply {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub connection: u128,
    pub opcode: u16,
    pub body: Vec<u8>,
}

#[table(accessor = game_return_to_selection, outbox(remote::idc_return_to_selection))]
pub struct ReturnToSelection {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub connection: u128,
    pub server_seed: u32,
    pub account_id: u64,
    pub client_seed: u32,
    pub client_proof: Vec<u8>,
}

/// After logout on a World Shard, hands the World Session back to the Realm Shard's Character
/// selection.
pub(super) fn return_to_selection(
    ctx: &ReducerContext,
    connection_id: ConnectionId,
    server_seed: u32,
    account_id: u64,
) -> Result<(), String> {
    let Some(realm) = realm_shard(ctx) else {
        return Ok(());
    };
    let cipher = ctx
        .db
        .game_native_cipher()
        .connection_id()
        .find(connection_id)
        .ok_or("world proof is missing")?;
    ctx.db.game_return_to_selection().insert(ReturnToSelection {
        id: 0,
        database: remote::Identity(realm),
        connection: connection_id.to_u128(),
        server_seed,
        account_id,
        client_seed: cipher.client_seed,
        client_proof: cipher.client_proof,
    });
    Ok(())
}

#[reducer(on_result(game_return_to_selection))]
pub fn return_answered(ctx: &ReducerContext, call: ReturnToSelection, result: Result<(), String>) {
    let connection_id = ConnectionId::from_u128(call.connection);
    if at_selection(ctx, connection_id).is_none() {
        return;
    }
    match result {
        Ok(()) => super::handoff::write_marker(ctx, connection_id, call.database.0),
        Err(error) => {
            spacetimedb::log::debug!("Realm Shard refused the return to selection: {error}");
            super::disconnect(ctx, connection_id);
        }
    }
}

#[table(accessor = game_world_session_closed, outbox(remote::idc_world_session_closed))]
pub struct WorldSessionClosed {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub connection: u128,
    pub account_id: u64,
}

/// Tells the Realm Shard that a World Session it handed here ended in the world.
pub(super) fn closing(ctx: &ReducerContext, connection_id: ConnectionId) {
    let Some(realm) = realm_shard(ctx) else {
        return;
    };
    let Some(connection) = ctx
        .db
        .game_native_world_connection()
        .connection_id()
        .find(connection_id)
    else {
        return;
    };
    if let NativeWorldState::InWorld(NativeActive { account_id, .. }) = connection.state {
        ctx.db
            .game_world_session_closed()
            .insert(WorldSessionClosed {
                id: 0,
                database: remote::Identity(realm),
                connection: connection_id.to_u128(),
                account_id,
            });
    }
}

#[reducer]
pub fn idc_selection_reply(
    ctx: &ReducerContext,
    connection: u128,
    opcode: u16,
    body: Vec<u8>,
) -> Result<(), String> {
    require_sender(ctx, world_shard(ctx))?;
    let connection_id = ConnectionId::from_u128(connection);
    if at_selection(ctx, connection_id).is_some() {
        super::send_raw(ctx, connection_id, opcode, body, false);
    }
    Ok(())
}

#[reducer]
pub fn idc_return_to_selection(
    ctx: &ReducerContext,
    connection: u128,
    server_seed: u32,
    account_id: u64,
    client_seed: u32,
    client_proof: Vec<u8>,
) -> Result<(), String> {
    require_sender(ctx, world_shard(ctx))?;
    let connection_id = ConnectionId::from_u128(connection);
    ctx.db.game_native_away().account_id().delete(account_id);
    let handoffs = ctx.db.game_native_handoff();
    handoffs.connection_id().delete(connection_id);
    handoffs.insert(NativeHandoff {
        connection_id,
        server_seed,
        account_id,
        client_seed,
        client_proof,
        character_guid: None,
    });
    Ok(())
}

#[reducer]
pub fn idc_world_session_closed(
    ctx: &ReducerContext,
    connection: u128,
    account_id: u64,
) -> Result<(), String> {
    require_sender(ctx, world_shard(ctx))?;
    let aways = ctx.db.game_native_away();
    if aways
        .account_id()
        .find(account_id)
        .is_some_and(|away| away.connection_id == ConnectionId::from_u128(connection))
    {
        aways.account_id().delete(account_id);
    }
    Ok(())
}
