use spacetimedb::{reducer, table, ConnectionId, Identity, ReducerContext, Table};

use super::{game_native_player, game_native_world_connection, NativeWorldState};
use crate::{game_account, game_world_entity};
mod projection;

#[table(accessor = game_native_conformance_connection)]
pub struct NativeConformanceConnection {
    #[primary_key]
    pub connection_id: ConnectionId,
    pub account_id: u64,
    pub owner_identity: Identity,
}

#[table(accessor = game_native_conformance_create)]
pub struct NativeConformanceCreate {
    #[primary_key]
    pub packet_id: u64,
    pub newly_added: bool,
    pub layer: u8,
    pub gameobject_type: u8,
}

#[table(accessor = game_native_conformance_birth)]
pub struct NativeConformanceBirth {
    #[primary_key]
    pub guid: u64,
    pub viewers: Vec<u64>,
    pub added_at: spacetimedb::Timestamp,
}

pub(crate) fn record_birth(
    ctx: &ReducerContext,
    guid: u64,
    map_id: u32,
    instance_id: u64,
    cell: i64,
) {
    let entity = ctx.db.game_world_entity().guid().find(guid);
    let stealthed = entity
        .as_ref()
        .is_some_and(|_| crate::spell::is_stealthed(ctx, guid));
    let viewers = crate::aoi::nearby_viewers(ctx, map_id, instance_id, cell)
        .filter(|viewer| {
            ctx.db
                .game_native_player()
                .character_guid()
                .find(viewer.guid)
                .is_some()
                && entity
                    .as_ref()
                    .is_none_or(|entity| viewer.sees_entity(entity, stealthed))
        })
        .map(|viewer| viewer.guid)
        .collect();
    let births = ctx.db.game_native_conformance_birth();
    births.guid().delete(guid);
    births.insert(NativeConformanceBirth {
        guid,
        viewers,
        added_at: ctx.timestamp,
    });
}

pub(super) fn note_create(
    ctx: &ReducerContext,
    packet_id: u64,
    object_guid: u64,
    viewer_guid: u64,
    layer: u8,
    gameobject_type: u8,
) {
    let births = ctx.db.game_native_conformance_birth();
    let mut birth = births.guid().find(object_guid);
    let newly_added = birth.as_mut().is_some_and(|birth| {
        if birth.added_at != ctx.timestamp {
            return false;
        }
        if let Some(index) = birth
            .viewers
            .iter()
            .position(|viewer| *viewer == viewer_guid)
        {
            birth.viewers.remove(index);
            true
        } else {
            false
        }
    });
    if let Some(birth) = birth {
        births.guid().update(birth);
    }
    ctx.db
        .game_native_conformance_create()
        .insert(NativeConformanceCreate {
            packet_id,
            newly_added,
            layer,
            gameobject_type,
        });
}

pub(super) fn account(
    ctx: &ReducerContext,
    connection_id: ConnectionId,
    account: &mut crate::Account,
) -> Result<bool, String> {
    let Some(fixture) = ctx
        .db
        .game_native_conformance_connection()
        .connection_id()
        .find(connection_id)
    else {
        return Ok(false);
    };
    if fixture.account_id != account.id || account.banned {
        return Err("conformance Account is no longer available".into());
    }
    // World entry needs an owner; this copy never changes the Account's login identity.
    account.identity = Some(fixture.owner_identity);
    Ok(true)
}

pub(super) fn close(ctx: &ReducerContext, connection_id: ConnectionId) {
    projection::close(ctx, connection_id);
    ctx.db
        .game_native_conformance_connection()
        .connection_id()
        .delete(connection_id);
}

#[reducer]
pub fn debug_native_enter_world(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    let connection_id = super::connection_id(ctx)?;
    let connection = ctx
        .db
        .game_native_world_connection()
        .connection_id()
        .find(connection_id)
        .ok_or("native connection is not open")?;
    if !matches!(connection.state, NativeWorldState::AwaitingProof) {
        return Err("debug world entry requires an unauthenticated connection".into());
    }
    let character = crate::helpers::character_by_guid(ctx, character_guid)
        .ok_or("Character no longer exists")?;
    let account = ctx
        .db
        .game_account()
        .id()
        .find(character.account_id)
        .ok_or("Account no longer exists")?;
    if account.banned {
        return Err("Account is banned".into());
    }
    ctx.db
        .game_native_conformance_connection()
        .insert(NativeConformanceConnection {
            connection_id,
            account_id: account.id,
            owner_identity: account.identity.unwrap_or(ctx.sender()),
        });
    super::world::enter_character(
        ctx,
        connection,
        account.id,
        character_guid,
        crate::world::WorldEntry::Login,
        0,
    )
}
