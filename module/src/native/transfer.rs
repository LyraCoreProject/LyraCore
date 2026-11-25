use lyracore_protocol::codec::{CMSG_PLAYER_LOGIN_OPCODE, MSG_MOVE_WORLDPORT_ACK_OPCODE};
use spacetimedb::sats::bsatn;
use spacetimedb::{
    reducer, table, ConnectionId, Identity, ReducerContext, ScheduleAt, Table, TimeDuration,
};

use super::directory::{
    self, game_character_directory, game_character_directory_version, CharacterDirectory,
    CharacterDirectoryVersion,
};
use super::handoff::{game_native_handoff, NativeHandoff};
use super::realm_shard::{store_account, store_session};
use super::teleport::NativeTeleportOrigin;
use super::{
    game_native_cipher, game_native_world_connection, NativePlayer, NativeWorldConnection,
};
use crate::items::game_buyback_state;
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
    #[default(true)]
    pub world_port: bool,
}

pub(super) fn begin(
    ctx: &ReducerContext,
    connection: &NativeWorldConnection,
    player: &NativePlayer,
) -> Result<bool, String> {
    begin_character(
        ctx,
        connection,
        player.account_id,
        player.character_guid,
        player.teleport_counter,
        player.teleport_origin,
        true,
    )
}

pub(super) fn begin_character(
    ctx: &ReducerContext,
    connection: &NativeWorldConnection,
    account_id: u64,
    guid: u64,
    teleport_counter: u32,
    origin: Option<NativeTeleportOrigin>,
    world_port: bool,
) -> Result<bool, String> {
    let Some(character) = crate::helpers::character_by_guid(ctx, guid) else {
        return Err("Character no longer exists".into());
    };
    let Some(target) =
        directory::transfer_destination(ctx, guid, character.map_id, character.pending_instance_id)
    else {
        return Ok(false);
    };
    if !is_peer(ctx, target) {
        return Err("destination World Shard is not a Peer Shard".into());
    }
    let realm = realm_shard(ctx).ok_or("Transfer requires a Realm Shard")?;
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
    let destination = transfer::Destination {
        map_id: character.map_id,
        instance_id: character.pending_instance_id,
        x: character.x,
        y: character.y,
        z: character.z,
        o: character.orientation,
    };
    let transfer_id = *crate::auth::reserve_guids(ctx, 1, u64::MAX)?.start();
    super::world::leave(
        ctx,
        connection.connection_id,
        crate::character::CharacterDeparture::Transfer,
    );
    let mut character = crate::helpers::character_by_guid(ctx, guid)
        .ok_or("Character vanished during departure")?;
    character.relocate(destination);
    ctx.db.game_character().guid().update(character);
    transfer::apply_begin(&mut CtxShard { ctx }, transfer_id, guid, destination, true)?;
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
        character_guid: guid,
        teleport_counter,
        blob: escrow.blob,
        revision: directory::revision(ctx),
        origin: bsatn::to_vec(&origin).map_err(|error| error.to_string())?,
        world_port,
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
    world_port: bool,
) -> Result<(), String> {
    require_world_peer(ctx, realm)?;
    let _ = origin;
    let decoded = transfer::decode_blob(transfer_id, &blob)?;
    let character = transfer::decode_character_row(transfer_id, &decoded.character_row)?;
    if decoded.transfer_id != transfer_id
        || decoded.character_guid != character_guid
        || character.guid != character_guid
        || !super::instance_allocation::hosts(ctx, decoded.dest_map_id, ctx.database_identity())
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
    let first_import = ctx
        .db
        .game_transfer_in()
        .transfer_id()
        .find(transfer_id)
        .is_none();
    transfer::apply_import_blob(&mut CtxShard { ctx }, transfer_id, blob)?;
    if world_port
        && ctx
            .db
            .game_buyback_state()
            .character_guid()
            .find(character_guid)
            .is_none()
    {
        return Err("World Port Transfer has no buyback World Session state".into());
    }
    if first_import {
        super::instance_shard::imported(
            ctx,
            character_guid,
            connection,
            decoded.dest_map_id,
            decoded.dest_instance_id,
            None,
        )?;
    }
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
        world_port,
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
        clear_pending_playtest(ctx, call.character_guid);
        let origin: Option<NativeTeleportOrigin> =
            bsatn::from_slice(&call.origin).map_err(|error| error.to_string())?;
        super::teleport::restore_origin(ctx, call.character_guid, origin);
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

#[derive(Debug, PartialEq, Eq)]
enum MovePlan {
    Apply,
    AlreadyApplied,
}

fn plan_move(
    entry: &CharacterDirectory,
    version: &CharacterDirectoryVersion,
    sender: Identity,
    target: Identity,
    account_id: u64,
    revision: u64,
) -> Result<MovePlan, String> {
    if entry.account_id != account_id {
        return Err("Character directory Account cannot change".into());
    }
    if entry.world == target && version.world == target {
        return Ok(MovePlan::AlreadyApplied);
    }
    if version.world != sender {
        return Err("only the current World Shard can Transfer a Character directory entry".into());
    }
    if revision <= version.revision {
        return Err("Character directory holds a newer revision than this Transfer".into());
    }
    Ok(MovePlan::Apply)
}

/// Ok means the directory names `world` afterwards: the source then releases the Character.
#[reducer]
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
    if plan_move(&entry, &version, ctx.sender(), world, account_id, revision)?
        == MovePlan::AlreadyApplied
    {
        return Ok(());
    }
    entry.world = world;
    version.world = world;
    version.revision = revision;
    directory.guid().update(entry);
    versions.guid().update(version);
    super::replacement::moved(ctx, account_id, connection, ctx.sender(), world);
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
crate::virtual_clock_exempt!(game_native_transfer_moved_retry);

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
crate::virtual_clock_exempt!(game_native_transfer_release_retry);

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

crate::outside_table_catalog!(
    game_native_transfer_pending,
    game_native_transfer,
    game_native_transfer_moved,
    game_native_transfer_moved_retry,
    game_native_transfer_release_retry,
    game_native_transfer_release
);

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
    // A released Handoff that this Shard abandoned has already reported its World Session closed.
    if handoff.is_none()
        && ctx
            .db
            .game_transfer_in()
            .transfer_id()
            .find(transfer_id)
            .is_none()
    {
        if !connected && !super::has_world_session(ctx, character_guid) {
            clear_pending_playtest(ctx, character_guid);
        }
        return Ok(());
    }
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
    if connected {
        super::handoff::await_adoption(
            ctx,
            handoff.connection_id,
            if handoff.world_port {
                MSG_MOVE_WORLDPORT_ACK_OPCODE
            } else {
                CMSG_PLAYER_LOGIN_OPCODE
            },
        );
    } else {
        super::handoff::abandon(ctx, handoff.connection_id);
    }
    Ok(())
}

pub(super) fn clear_pending_playtest(ctx: &ReducerContext, guid: u64) {
    if let Some(mut character) = crate::helpers::character_by_guid(ctx, guid) {
        let (godmode, speed) = crate::character::persisted_gm_playtest(
            character.pending_godmode,
            character.pending_run_speed_mult_bp,
            true,
        );
        character.pending_godmode = godmode;
        character.pending_run_speed_mult_bp = speed;
        ctx.db.game_character().guid().update(character);
    }
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
    if ctx
        .db
        .game_native_world_connection()
        .connection_id()
        .find(connection_id)
        .is_some()
    {
        super::handoff::write_marker(ctx, connection_id, call.database.0);
    } else {
        ctx.db
            .game_native_transfer_pending()
            .connection_id()
            .delete(connection_id);
        if call.connected {
            call.id = 0;
            call.connected = false;
            ctx.db.game_native_transfer_release().insert(call);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUID: u64 = 42;
    const ACCOUNT: u64 = 7;

    fn source() -> Identity {
        Identity::from_byte_array([1; 32])
    }

    fn target() -> Identity {
        Identity::from_byte_array([2; 32])
    }

    fn directory_at(
        world: Identity,
        revision: u64,
    ) -> (CharacterDirectory, CharacterDirectoryVersion) {
        (
            CharacterDirectory {
                guid: GUID,
                account_id: ACCOUNT,
                world,
            },
            CharacterDirectoryVersion {
                guid: GUID,
                revision,
                world,
            },
        )
    }

    fn plan(
        world: Identity,
        current: u64,
        sender: Identity,
        revision: u64,
    ) -> Result<MovePlan, String> {
        let (entry, version) = directory_at(world, current);
        plan_move(&entry, &version, sender, target(), ACCOUNT, revision)
    }

    #[test]
    fn a_newer_move_from_the_current_owner_applies() {
        assert_eq!(plan(source(), 10, source(), 11), Ok(MovePlan::Apply));
    }

    #[test]
    fn a_move_the_directory_already_shows_is_accepted_without_change() {
        assert_eq!(
            plan(target(), 11, source(), 11),
            Ok(MovePlan::AlreadyApplied)
        );
    }

    #[test]
    fn an_older_move_from_the_owner_is_refused_while_the_directory_names_the_source() {
        assert!(plan(source(), 12, source(), 11).is_err());
        assert!(plan(source(), 11, source(), 11).is_err());
    }

    #[test]
    fn a_move_from_a_world_shard_that_does_not_own_the_entry_is_refused() {
        let other = Identity::from_byte_array([3; 32]);
        assert!(plan(other, 10, source(), 11).is_err());
    }

    #[test]
    fn a_move_for_another_account_is_refused_even_when_the_directory_names_the_target() {
        let (entry, version) = directory_at(target(), 11);
        assert!(plan_move(&entry, &version, source(), target(), ACCOUNT + 1, 11).is_err());
    }
}
