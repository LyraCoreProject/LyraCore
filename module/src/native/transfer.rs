use spacetimedb::sats::bsatn;
use spacetimedb::{
    reducer, table, ConnectionId, Identity, ReducerContext, ScheduleAt, Table, TimeDuration,
};

use super::directory::{
    self, game_character_directory, game_character_directory_version, game_map_owner,
};
use super::handoff::{game_native_handoff, NativeHandoff};
use super::realm_shard::{
    game_native_away, game_world_session_closed, store_account, store_session, WorldSessionClosed,
};
use super::teleport::NativeTeleportOrigin;
use super::{
    game_native_cipher, game_native_world_connection, NativePlayer, NativeWorldConnection,
};
use crate::peer::{is_peer, realm_shard};
use crate::remote_bindings::peer as remote;
use crate::transfer::{self, game_transfer_in, game_transfer_out, CtxShard};
use crate::{game_account, game_character, game_session};

#[table(accessor = game_native_transfer_pending)]
pub struct NativeTransferPending {
    #[primary_key]
    pub connection_id: ConnectionId,
}

pub(super) fn pending(ctx: &ReducerContext, connection_id: ConnectionId) -> bool {
    ctx.db
        .game_native_transfer_pending()
        .connection_id()
        .find(connection_id)
        .is_some()
}

#[table(accessor = game_native_transfer, outbox(remote::idc_native_transfer))]
pub struct NativeTransfer {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub realm: remote::Identity,
    pub transfer_id: u64,
    pub connection: u128,
    pub server_seed: u32,
    pub account: Vec<u8>,
    pub session: Vec<u8>,
    pub client_seed: u32,
    pub client_proof: Vec<u8>,
    pub character_guid: u64,
    pub teleport_counter: u32,
    pub blob: Vec<u8>,
    pub revision: u64,
    pub origin: Vec<u8>,
}

pub(super) fn begin(
    ctx: &ReducerContext,
    connection: &NativeWorldConnection,
    player: &NativePlayer,
) -> Result<bool, String> {
    let Some(character) = crate::helpers::character_by_guid(ctx, player.character_guid) else {
        return Err("Character no longer exists".into());
    };
    let Some(target) = directory::transfer_target(ctx, character.map_id) else {
        return Ok(false);
    };
    let realm = realm_shard(ctx).ok_or("Transfer requires a Realm Shard")?;
    let account = ctx
        .db
        .game_account()
        .id()
        .find(player.account_id)
        .ok_or("Account no longer exists")?;
    let session = ctx
        .db
        .game_session()
        .account_id()
        .find(player.account_id)
        .ok_or("Session no longer exists")?;
    let cipher = ctx
        .db
        .game_native_cipher()
        .connection_id()
        .find(connection.connection_id)
        .ok_or("world proof is missing")?;
    let destination = transfer::Destination {
        map_id: character.map_id,
        instance_id: character.pending_instance_id,
        x: character.x,
        y: character.y,
        z: character.z,
        o: character.orientation,
    };
    let transfer_id = *crate::auth::reserve_guids(ctx, 1, u64::MAX)?.start();
    super::world::leave(ctx, connection.connection_id);
    let mut character = crate::helpers::character_by_guid(ctx, player.character_guid)
        .ok_or("Character vanished during departure")?;
    character.relocate(destination);
    ctx.db.game_character().guid().update(character);
    if let Some(cast) = crate::spell::pending_cast(ctx, player.character_guid) {
        crate::spell::cancel_cast_attempt(ctx, player.character_guid, cast.scheduled_id);
    }
    transfer::apply_begin(
        &mut CtxShard { ctx },
        transfer_id,
        player.character_guid,
        destination,
        true,
    )?;
    let escrow = ctx
        .db
        .game_transfer_out()
        .transfer_id()
        .find(transfer_id)
        .ok_or("Transfer did not create Escrow")?;
    ctx.db
        .game_native_transfer_pending()
        .insert(NativeTransferPending {
            connection_id: connection.connection_id,
        });
    ctx.db.game_native_transfer().insert(NativeTransfer {
        id: 0,
        database: remote::Identity(target),
        realm: remote::Identity(realm),
        transfer_id,
        connection: connection.connection_id.to_u128(),
        server_seed: connection.server_seed,
        account: bsatn::to_vec(&account).map_err(|error| error.to_string())?,
        session: bsatn::to_vec(&session).map_err(|error| error.to_string())?,
        client_seed: cipher.client_seed,
        client_proof: cipher.client_proof,
        character_guid: player.character_guid,
        teleport_counter: player.teleport_counter,
        blob: escrow.blob,
        revision: directory::revision(ctx),
        origin: bsatn::to_vec(&player.teleport_origin).map_err(|error| error.to_string())?,
    });
    Ok(true)
}

fn require_world_peer(ctx: &ReducerContext, realm: Identity) -> Result<(), String> {
    if realm_shard(ctx) == Some(realm) && ctx.sender() != realm && is_peer(ctx, ctx.sender()) {
        Ok(())
    } else {
        Err("caller is not a Peer Shard".into())
    }
}

#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn idc_native_transfer(
    ctx: &ReducerContext,
    realm: Identity,
    transfer_id: u64,
    connection: u128,
    server_seed: u32,
    account: Vec<u8>,
    session: Vec<u8>,
    client_seed: u32,
    client_proof: Vec<u8>,
    character_guid: u64,
    teleport_counter: u32,
    blob: Vec<u8>,
    revision: u64,
    origin: Vec<u8>,
) -> Result<(), String> {
    require_world_peer(ctx, realm)?;
    let _ = origin;
    let decoded = transfer::decode_blob(transfer_id, &blob)?;
    let character = transfer::decode_character_row(transfer_id, &decoded.character_row)?;
    if decoded.transfer_id != transfer_id
        || decoded.character_guid != character_guid
        || character.guid != character_guid
        || !ctx
            .db
            .game_map_owner()
            .map_id()
            .find(decoded.dest_map_id)
            .is_some_and(|owner| owner.world == ctx.database_identity())
    {
        return Err("Transfer destination or Character does not match".into());
    }
    let account = store_account(ctx, &account)?;
    let session_row: crate::auth::Session =
        bsatn::from_slice(&session).map_err(|error| error.to_string())?;
    if character.account_id != account.id || session_row.account_id != account.id || account.banned
    {
        return Err("Account cannot Transfer this Character".into());
    }
    store_session(ctx, &session)?;
    if ctx
        .db
        .game_transfer_in()
        .transfer_id()
        .find(transfer_id)
        .is_none()
        && (transfer::is_in_transit(ctx, character_guid)
            || crate::helpers::character_by_guid(ctx, character_guid).is_some())
    {
        return Err("Transfer target already holds this Character".into());
    }
    transfer::apply_import_blob(&mut CtxShard { ctx }, transfer_id, blob)?;
    directory::observe_revision(ctx, revision);
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
        world_port: true,
        teleport_counter,
        transfer_id,
        transfer_source: Some(ctx.sender()),
    });
    Ok(())
}

#[table(accessor = game_native_transfer_moved, outbox(remote::idc_native_transfer_moved))]
pub struct NativeTransferMoved {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub world: remote::Identity,
    pub transfer_id: u64,
    pub character_guid: u64,
    pub account_id: u64,
    pub connection: u128,
    pub revision: u64,
}

#[reducer(on_result(game_native_transfer))]
pub fn native_transfer_answered(
    ctx: &ReducerContext,
    call: NativeTransfer,
    result: Result<(), String>,
) -> Result<(), String> {
    let connection_id = ConnectionId::from_u128(call.connection);
    if let Err(error) = result {
        spacetimedb::log::warn!("Transfer {} refused: {error}", call.transfer_id);
        transfer::apply_abort(&mut CtxShard { ctx }, call.transfer_id)?;
        let origin: Option<NativeTeleportOrigin> =
            bsatn::from_slice(&call.origin).map_err(|error| error.to_string())?;
        super::teleport::restore_origin(ctx, call.character_guid, origin);
        ctx.db
            .game_native_transfer_pending()
            .connection_id()
            .delete(connection_id);
        super::disconnect(ctx, connection_id);
        return Ok(());
    }
    transfer::apply_confirm(&mut CtxShard { ctx }, call.transfer_id)?;
    transfer::apply_finish_step(&mut CtxShard { ctx }, call.transfer_id)?;
    let account: crate::auth::Account =
        bsatn::from_slice(&call.account).map_err(|error| error.to_string())?;
    ctx.db
        .game_native_transfer_moved()
        .insert(NativeTransferMoved {
            id: 0,
            database: call.realm,
            world: call.database,
            transfer_id: call.transfer_id,
            character_guid: call.character_guid,
            account_id: account.id,
            connection: call.connection,
            revision: call.revision,
        });
    Ok(())
}

fn move_is_current(
    owner: Identity,
    current_revision: u64,
    sender: Identity,
    revision: u64,
) -> Result<bool, String> {
    if revision <= current_revision {
        return Ok(false);
    }
    if owner != sender {
        return Err("only the current World Shard can Transfer a Character directory entry".into());
    }
    Ok(true)
}

#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn idc_native_transfer_moved(
    ctx: &ReducerContext,
    world: Identity,
    transfer_id: u64,
    character_guid: u64,
    account_id: u64,
    connection: u128,
    revision: u64,
) -> Result<(), String> {
    directory::require_world(ctx)?;
    if !is_peer(ctx, world) || world == ctx.sender() {
        return Err("Transfer target is not another Peer World Shard".into());
    }
    let directory = ctx.db.game_character_directory();
    let mut entry = directory
        .guid()
        .find(character_guid)
        .ok_or("Character directory entry is missing")?;
    let versions = ctx.db.game_character_directory_version();
    let mut version = versions
        .guid()
        .find(character_guid)
        .ok_or("Character directory revision is missing")?;
    if !move_is_current(version.world, version.revision, ctx.sender(), revision)? {
        return Ok(());
    }
    if entry.account_id != account_id {
        return Err("Character directory Account cannot change".into());
    }
    entry.world = world;
    version.world = world;
    version.revision = revision;
    directory.guid().update(entry);
    versions.guid().update(version);
    let aways = ctx.db.game_native_away();
    if let Some(mut away) = aways.account_id().find(account_id) {
        if away.connection_id == ConnectionId::from_u128(connection)
            && away.world.or_else(|| crate::peer::world_shard(ctx)) == Some(ctx.sender())
        {
            away.world = Some(world);
            aways.account_id().update(away);
        }
    }
    directory::observe_revision(ctx, revision);
    let _ = transfer_id;
    Ok(())
}

/// A refused Realm update or release waits before it is sent again: an IDC error is final, so an
/// immediate resend would loop until the refusal clears.
const RETRY_MICROS: i64 = 30_000_000;

fn retry_at(ctx: &ReducerContext) -> ScheduleAt {
    (ctx.timestamp + TimeDuration::from_micros(RETRY_MICROS)).into()
}

#[table(accessor = game_native_transfer_moved_retry, scheduled(retry_native_transfer_moved))]
pub struct NativeTransferMovedRetry {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    pub call: NativeTransferMoved,
}

#[reducer]
pub fn retry_native_transfer_moved(
    ctx: &ReducerContext,
    retry: NativeTransferMovedRetry,
) -> Result<(), String> {
    if !crate::time::scheduler_authorized(ctx) {
        return Err("scheduler only".into());
    }
    ctx.db
        .game_native_transfer_moved()
        .insert(NativeTransferMoved {
            id: 0,
            ..retry.call
        });
    Ok(())
}

#[table(accessor = game_native_transfer_release_retry, scheduled(retry_native_transfer_release))]
pub struct NativeTransferReleaseRetry {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    pub call: NativeTransferRelease,
}

#[reducer]
pub fn retry_native_transfer_release(
    ctx: &ReducerContext,
    retry: NativeTransferReleaseRetry,
) -> Result<(), String> {
    if !crate::time::scheduler_authorized(ctx) {
        return Err("scheduler only".into());
    }
    ctx.db
        .game_native_transfer_release()
        .insert(NativeTransferRelease {
            id: 0,
            ..retry.call
        });
    Ok(())
}

#[table(accessor = game_native_transfer_release, outbox(remote::idc_native_transfer_release))]
pub struct NativeTransferRelease {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub realm: remote::Identity,
    pub transfer_id: u64,
    pub character_guid: u64,
    pub connection: u128,
    pub connected: bool,
}

#[reducer(on_result(game_native_transfer_moved))]
pub fn native_transfer_moved_answered(
    ctx: &ReducerContext,
    call: NativeTransferMoved,
    result: Result<(), String>,
) {
    if let Err(error) = result {
        spacetimedb::log::error!(
            "Transfer {} directory update waits: {error}",
            call.transfer_id
        );
        ctx.db
            .game_native_transfer_moved_retry()
            .insert(NativeTransferMovedRetry {
                scheduled_id: 0,
                scheduled_at: retry_at(ctx),
                call,
            });
        return;
    }
    ctx.db
        .game_native_transfer_release()
        .insert(NativeTransferRelease {
            id: 0,
            database: call.world,
            realm: call.database,
            transfer_id: call.transfer_id,
            character_guid: call.character_guid,
            connection: call.connection,
            connected: true,
        });
}

#[reducer]
pub fn idc_native_transfer_release(
    ctx: &ReducerContext,
    realm: Identity,
    transfer_id: u64,
    character_guid: u64,
    connection: u128,
    connected: bool,
) -> Result<(), String> {
    require_world_peer(ctx, realm)?;
    let handoff = ctx
        .db
        .game_native_handoff()
        .connection_id()
        .find(ConnectionId::from_u128(connection));
    let handoff = handoff
        .filter(|handoff| {
            handoff.transfer_id == transfer_id
                && handoff.transfer_source == Some(ctx.sender())
                && handoff.character_guid == Some(character_guid)
        })
        .ok_or("Transfer release does not match its Handoff")?;
    if let Some(receipt) = ctx.db.game_transfer_in().transfer_id().find(transfer_id) {
        if receipt.character_guid != character_guid {
            return Err("Transfer receipt belongs to another Character".into());
        }
        transfer::apply_release(&mut CtxShard { ctx }, transfer_id)?;
    }
    if !connected {
        ctx.db
            .game_world_session_closed()
            .insert(WorldSessionClosed {
                id: 0,
                database: remote::Identity(realm),
                connection,
                account_id: handoff.account_id,
            });
        ctx.db
            .game_native_handoff()
            .connection_id()
            .delete(handoff.connection_id);
    }
    Ok(())
}

#[reducer(on_result(game_native_transfer_release))]
pub fn native_transfer_release_answered(
    ctx: &ReducerContext,
    mut call: NativeTransferRelease,
    result: Result<(), String>,
) {
    if let Err(error) = result {
        spacetimedb::log::error!("Transfer {} release waits: {error}", call.transfer_id);
        ctx.db
            .game_native_transfer_release_retry()
            .insert(NativeTransferReleaseRetry {
                scheduled_id: 0,
                scheduled_at: retry_at(ctx),
                call,
            });
        return;
    }
    let connection_id = ConnectionId::from_u128(call.connection);
    ctx.db
        .game_native_transfer_pending()
        .connection_id()
        .delete(connection_id);
    if ctx
        .db
        .game_native_world_connection()
        .connection_id()
        .find(connection_id)
        .is_some()
    {
        super::handoff::write_marker(ctx, connection_id, call.database.0);
    } else if call.connected {
        call.id = 0;
        call.connected = false;
        ctx.db.game_native_transfer_release().insert(call);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_transfer_requires_current_owner_and_ignores_old_moves() {
        let source = Identity::from_byte_array([1; 32]);
        let target = Identity::ZERO;
        assert_eq!(move_is_current(source, 10, source, 11), Ok(true));
        assert_eq!(move_is_current(target, 11, source, 11), Ok(false));
        assert_eq!(move_is_current(source, 13, target, 12), Ok(false));
        assert!(move_is_current(target, 11, source, 12).is_err());
    }
}
