use spacetimedb::sats::bsatn;
use spacetimedb::{
    reducer, table, ConnectionId, Identity, ReducerContext, ScheduleAt, Table, TimeDuration,
};

use super::directory::{self, game_character_directory, game_map_owner};
use super::handoff::{game_native_handoff, NativeHandoff};
use super::{
    game_native_cipher, game_native_world_connection, NativeActive, NativeWorldConnection,
    NativeWorldState, Rollback,
};
use crate::auth::{Account, Session};
use crate::game_character;
use crate::peer::{realm_shard, require_sender, world_shard};
use crate::remote_bindings::peer as remote;
use crate::{game_account, game_session};

/// An Account whose World Session this Realm Shard handed to a World Shard.
#[table(accessor = game_native_away)]
pub struct NativeAway {
    #[primary_key]
    pub account_id: u64,
    pub connection_id: ConnectionId,
    #[default(None::<Identity>)]
    pub world: Option<Identity>,
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
    #[default(0u64)]
    pub selection_id: u64,
}

fn relay_selection(
    ctx: &ReducerContext,
    connection_id: ConnectionId,
    world: spacetimedb::Identity,
    account_id: u64,
    opcode: u32,
    body: Vec<u8>,
    selection_id: u64,
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
        selection_id,
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
    if request.selection_id != 0 {
        if request.opcode == 0x37 || super::rollback::replies_in(&error).is_none() {
            fail_selection(ctx, request.selection_id);
            return;
        }
        cancel_selection(ctx, connection_id);
    }
    let Some(packets) = super::rollback::replies_in(&error) else {
        spacetimedb::log::warn!("World Shard refused a selection request: {error}");
        if at_selection(ctx, connection_id).is_some() {
            super::disconnect(ctx, connection_id);
        }
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
    body: &[u8],
) -> Result<(), Rollback> {
    let NativeWorldState::CharacterSelection { account_id } = connection.state else {
        return Err("character login requires character selection".into());
    };
    if ctx
        .db
        .game_pending_selection()
        .connection_id()
        .find(connection.connection_id)
        .is_some()
    {
        return Err(Rollback::silent("Character selection request is pending"));
    }
    let character_guid = u64::from_le_bytes(
        body.try_into()
            .map_err(|_| "invalid character login packet")?,
    );
    let Some(world) = directory::character_world(ctx, account_id, character_guid) else {
        return Err(Rollback::reply(
            lyracore_protocol::messages::vanilla::opcodes::ServerOpcodeMessage::SMSG_CHARACTER_LOGIN_FAILED(
                lyracore_protocol::codec::build_character_login_failed()),
            "Character directory has no Character for this Account"));
    };
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
        world: Some(entry.database.0),
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

/// Sends Account and Session changes to the World Shard that holds the World Session.
pub(crate) fn account_changed(ctx: &ReducerContext, account_id: u64) {
    let Some(away) = ctx.db.game_native_away().account_id().find(account_id) else {
        return;
    };
    let Some(world) = away.world.or_else(|| world_shard(ctx)) else {
        return;
    };
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
    selection_id: u64,
) -> Result<(), String> {
    require_sender(ctx, realm_shard(ctx))?;
    let account = store_account(ctx, &account)?;
    let before: Vec<u64> = ctx
        .db
        .game_character()
        .by_account()
        .filter(account.id)
        .map(|character| character.guid)
        .collect();
    let reply = super::selection_reply(ctx, account.id, opcode, &body)
        .map_err(Rollback::into_reducer_error)?;
    if let Some(message) = reply {
        let packet =
            lyracore_protocol::world::encode(message).map_err(|error| error.to_string())?;
        ctx.db.game_selection_result().insert(SelectionResult {
            id: 0,
            database: remote::Identity(ctx.sender()),
            connection,
            opcode: packet.opcode,
            body: packet.body,
            selection_id,
            account_id: account.id,
            directory_changes: {
                let after: Vec<u64> = ctx
                    .db
                    .game_character()
                    .by_account()
                    .filter(account.id)
                    .map(|character| character.guid)
                    .collect();
                let mut changes: Vec<(u64, bool)> = before
                    .into_iter()
                    .filter(|guid| !after.contains(guid))
                    .map(|guid| (guid, false))
                    .collect();
                changes.extend(after.into_iter().map(|guid| (guid, true)));
                blob(&changes)
            },
            revision: directory::revision(ctx),
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
    if !directory::owns_map(ctx, character.map_id) {
        return Err("World Shard does not own the Character map".into());
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

#[table(accessor = game_selection_result, outbox(remote::idc_selection_result))]
pub struct SelectionResult {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub connection: u128,
    pub opcode: u16,
    pub body: Vec<u8>,
    pub selection_id: u64,
    pub account_id: u64,
    pub directory_changes: Vec<u8>,
    pub revision: u64,
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

/// Hands a logged-out World Session back to the Realm Shard.
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
#[allow(clippy::too_many_arguments)]
pub fn idc_selection_result(
    ctx: &ReducerContext,
    connection: u128,
    opcode: u16,
    body: Vec<u8>,
    selection_id: u64,
    account_id: u64,
    directory_changes: Vec<u8>,
    revision: u64,
) -> Result<(), String> {
    directory::require_world(ctx)?;
    directory::apply(ctx, account_id, &directory_changes, revision)?;
    let connection_id = ConnectionId::from_u128(connection);
    if selection_id != 0 {
        selection_answer(ctx, selection_id, connection_id, opcode, body)?;
    } else if at_selection(ctx, connection_id).is_some() {
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
    directory::require_world(ctx)?;
    let connection_id = ConnectionId::from_u128(connection);
    let away = ctx
        .db
        .game_native_away()
        .account_id()
        .find(account_id)
        .ok_or("Account has no away World Session")?;
    if away.connection_id != connection_id
        || away.world.or_else(|| world_shard(ctx)) != Some(ctx.sender())
    {
        return Err("World Session belongs to another World Shard".into());
    }
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
    directory::require_world(ctx)?;
    let aways = ctx.db.game_native_away();
    if aways.account_id().find(account_id).is_some_and(|away| {
        away.connection_id == ConnectionId::from_u128(connection)
            && away.world.or_else(|| world_shard(ctx)) == Some(ctx.sender())
    }) {
        aways.account_id().delete(account_id);
    }
    Ok(())
}

#[table(accessor = game_pending_selection, scheduled(expire_pending_selection))]
pub struct PendingSelection {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[unique]
    pub connection_id: ConnectionId,
    pub scheduled_at: ScheduleAt,
    pub request_opcode: u32,
    pub remaining: Vec<Identity>,
    pub bodies: Vec<Vec<u8>>,
}

pub(super) fn cancel_selection(ctx: &ReducerContext, connection_id: ConnectionId) {
    ctx.db
        .game_pending_selection()
        .connection_id()
        .delete(connection_id);
}

fn fail_selection(ctx: &ReducerContext, id: u64) {
    if let Some(pending) = ctx.db.game_pending_selection().id().find(id) {
        cancel_selection(ctx, pending.connection_id);
        if at_selection(ctx, pending.connection_id).is_some() {
            super::disconnect(ctx, pending.connection_id);
        }
    }
}

#[reducer]
pub fn expire_pending_selection(
    ctx: &ReducerContext,
    pending: PendingSelection,
) -> Result<(), String> {
    if ctx.sender() != ctx.database_identity() {
        return Err("scheduler only".into());
    }
    fail_selection(ctx, pending.id);
    Ok(())
}

pub(super) fn route_selection(
    ctx: &ReducerContext,
    connection_id: ConnectionId,
    account_id: u64,
    opcode: u32,
    body: Vec<u8>,
) -> Result<(), Rollback> {
    use lyracore_protocol::codec;
    use lyracore_protocol::messages::vanilla::opcodes::ClientOpcodeMessage;
    use lyracore_protocol::messages::vanilla::opcodes::ServerOpcodeMessage;

    if ctx
        .db
        .game_pending_selection()
        .connection_id()
        .find(connection_id)
        .is_some()
    {
        return Err(Rollback::silent("Character selection request is pending"));
    }
    let message =
        lyracore_protocol::world::decode(opcode, &body).map_err(|error| error.to_string())?;
    let world = match message {
        ClientOpcodeMessage::CMSG_CHAR_ENUM => {
            let mut worlds: Vec<Identity> = ctx
                .db
                .game_character_directory()
                .by_account()
                .filter(account_id)
                .map(|entry| entry.world)
                .collect();
            if worlds.is_empty() && ctx.db.game_map_owner().iter().next().is_none() {
                worlds.extend(world_shard(ctx));
            }
            worlds.sort();
            worlds.dedup();
            if worlds.is_empty() {
                super::send_raw(ctx, connection_id, 0x3b, vec![0], false);
                return Ok(());
            }
            let pending = ctx.db.game_pending_selection().insert(PendingSelection {
                id: 0,
                connection_id,
                scheduled_at: (ctx.timestamp + TimeDuration::from_micros(10_000_000)).into(),
                request_opcode: opcode,
                remaining: worlds.clone(),
                bodies: Vec::new(),
            });
            for world in worlds {
                relay_selection(
                    ctx,
                    connection_id,
                    world,
                    account_id,
                    opcode,
                    body.clone(),
                    pending.id,
                )?;
            }
            return Ok(());
        }
        ClientOpcodeMessage::CMSG_CHAR_CREATE(request) => {
            let count = ctx
                .db
                .game_character_directory()
                .by_account()
                .filter(account_id)
                .count();
            if count >= crate::auth::CHARACTERS_PER_ACCOUNT {
                return Err(Rollback::reply(
                    ServerOpcodeMessage::SMSG_CHAR_CREATE(codec::build_char_create_response(
                        codec::CharCreateOutcome::ServerLimit,
                    )),
                    "Account is full",
                ));
            }
            directory::create_world(ctx, request.race.as_int(), request.class.as_int()).ok_or_else(
                || {
                    Rollback::reply(
                        ServerOpcodeMessage::SMSG_CHAR_CREATE(codec::build_char_create_response(
                            codec::CharCreateOutcome::Failed,
                        )),
                        "start map has no World Shard",
                    )
                },
            )?
        }
        ClientOpcodeMessage::CMSG_CHAR_DELETE(request) => {
            directory::character_world(ctx, account_id, request.guid.guid()).ok_or_else(|| {
                Rollback::reply(
                    ServerOpcodeMessage::SMSG_CHAR_DELETE(codec::build_char_delete_response(
                        codec::CharDeleteOutcome::Failed,
                    )),
                    "Character directory has no Character for this Account",
                )
            })?
        }
        _ => return Ok(()),
    };
    let pending = ctx.db.game_pending_selection().insert(PendingSelection {
        id: 0,
        connection_id,
        scheduled_at: (ctx.timestamp + TimeDuration::from_micros(10_000_000)).into(),
        request_opcode: opcode,
        remaining: vec![world],
        bodies: Vec::new(),
    });
    Ok(relay_selection(
        ctx,
        connection_id,
        world,
        account_id,
        opcode,
        body,
        pending.id,
    )?)
}

fn selection_answer(
    ctx: &ReducerContext,
    id: u64,
    connection_id: ConnectionId,
    opcode: u16,
    body: Vec<u8>,
) -> Result<(), String> {
    let Some(mut pending) = ctx.db.game_pending_selection().id().find(id) else {
        return Ok(());
    };
    if pending.connection_id != connection_id {
        return Err("Character selection connection does not match".into());
    }
    let Some(index) = pending
        .remaining
        .iter()
        .position(|world| *world == ctx.sender())
    else {
        return Err("unexpected World Shard selection reply".into());
    };
    if pending.request_opcode != 0x37 {
        let expected = match pending.request_opcode {
            0x36 => 0x3a,
            0x38 => 0x3c,
            _ => 0,
        };
        if opcode != expected {
            fail_selection(ctx, id);
            return Ok(());
        }
        cancel_selection(ctx, connection_id);
        if at_selection(ctx, connection_id).is_some() {
            super::send_raw(ctx, connection_id, opcode, body, false);
        }
        return Ok(());
    }
    if opcode != 0x3b || body.is_empty() {
        fail_selection(ctx, id);
        return Ok(());
    }
    pending.remaining.remove(index);
    pending.bodies.push(body);
    if pending.remaining.is_empty() {
        match merge_enumerations(&pending.bodies) {
            Ok(body) => {
                cancel_selection(ctx, connection_id);
                if at_selection(ctx, connection_id).is_some() {
                    super::send_raw(ctx, connection_id, 0x3b, body, false);
                }
            }
            Err(_) => fail_selection(ctx, id),
        }
    } else {
        ctx.db.game_pending_selection().id().update(pending);
    }
    Ok(())
}

fn merge_enumerations(bodies: &[Vec<u8>]) -> Result<Vec<u8>, &'static str> {
    let mut merged = vec![0u8];
    for body in bodies {
        let (&count, entries) = body.split_first().ok_or("missing Character count")?;
        merged[0] = merged[0]
            .checked_add(count)
            .ok_or("Character count overflow")?;
        if usize::from(merged[0]) > crate::auth::CHARACTERS_PER_ACCOUNT
            || (count == 0) != entries.is_empty()
        {
            return Err("invalid Character list");
        }
        merged.extend_from_slice(entries);
    }
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merged_character_list_has_one_count_and_preserves_entries() {
        assert_eq!(
            merge_enumerations(&[vec![1, 11, 12], vec![0], vec![2, 21, 22, 23]]),
            Ok(vec![3, 11, 12, 21, 22, 23])
        );
        assert_eq!(merge_enumerations(&[]), Ok(vec![0]));
    }

    #[test]
    fn malformed_or_oversized_lists_fail_instead_of_wrapping_the_count() {
        for bodies in [
            vec![vec![]],
            vec![vec![0, 1]],
            vec![vec![1]],
            vec![vec![10, 1], vec![1, 2]],
            vec![vec![255, 1], vec![1, 2]],
        ] {
            assert!(merge_enumerations(&bodies).is_err());
        }
    }
}

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

#[reducer]
pub fn idc_selection_reply(
    ctx: &ReducerContext,
    connection: u128,
    opcode: u16,
    body: Vec<u8>,
) -> Result<(), String> {
    directory::require_world(ctx)?;
    let connection_id = ConnectionId::from_u128(connection);
    if at_selection(ctx, connection_id).is_some() {
        super::send_raw(ctx, connection_id, opcode, body, false);
    }
    Ok(())
}
