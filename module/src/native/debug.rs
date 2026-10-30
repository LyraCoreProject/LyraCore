use spacetimedb::{reducer, table, ConnectionId, Identity, ReducerContext, Table};

use super::{game_native_player, game_native_world_connection, NativeWorldState};
use crate::aoi::game_aoi_grant;
use crate::group::game_group_member;
use crate::items::game_item_instance;
use crate::skill::game_player_skill;
use crate::{game_account, game_corpse, game_dynamic_object, game_gameobject, game_world_entity};
use lyracore_protocol::{codec, messages::vanilla::opcodes::ServerOpcodeMessage};

#[table(accessor = game_native_conformance_connection)]
pub struct NativeConformanceConnection {
    #[primary_key]
    pub connection_id: ConnectionId,
    pub account_id: u64,
    pub owner_identity: Identity,
}

#[table(accessor = game_native_conformance_projection, event)]
pub struct NativeConformanceProjection {
    #[primary_key]
    pub guid: u64,
    pub object_type: u8,
    pub owner_guid: u64,
    pub same_raid: bool,
    pub body_hex: String,
    pub extra_hex: String,
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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[reducer]
pub fn debug_native_project(ctx: &ReducerContext) -> Result<(), String> {
    let connection_id = super::connection_id(ctx)?;
    let viewer = ctx
        .db
        .game_native_player()
        .connection_id()
        .find(connection_id)
        .ok_or("conformance Character is not in world")?
        .character_guid;
    let guids = ctx
        .db
        .game_aoi_grant()
        .by_viewer()
        .filter(viewer)
        .map(|grant| grant.entity_guid)
        .chain(
            ctx.db
                .game_item_instance()
                .by_owner_guid()
                .filter(viewer)
                .map(|item| item.guid),
        )
        .collect::<std::collections::BTreeSet<_>>();
    let table = ctx.db.game_native_conformance_projection();
    for guid in guids {
        let grant = ctx
            .db
            .game_aoi_grant()
            .by_viewer()
            .filter(viewer)
            .find(|grant| grant.entity_guid == guid);
        let (message, extra, object_type, owner_guid) = if let Some(grant) = grant {
            match grant.layer {
                crate::aoi::WORLD_ENTITY => {
                    let entity = ctx
                        .db
                        .game_world_entity()
                        .guid()
                        .find(guid)
                        .ok_or("projected WorldEntity is missing")?;
                    let mut view = super::view::entity(ctx, &entity);
                    let items = if entity.is_player() {
                        super::view::items(ctx, guid)
                    } else {
                        Vec::new()
                    };
                    let message = if viewer == guid {
                        let skills = ctx
                            .db
                            .game_player_skill()
                            .by_character()
                            .filter(guid)
                            .map(|skill| (skill.skill_line, skill.current, skill.max_rank))
                            .collect::<Vec<_>>();
                        let quests = super::view::quests(ctx, guid);
                        codec::build_self_create_object(&view, &items, &skills, &quests)
                    } else {
                        super::taxi::create_spline(ctx, guid, &mut view);
                        view.dynamic_flags = super::loot::dynamic_flags(ctx, &entity, viewer);
                        codec::build_create_object(&view, codec::CreateKind::Peer, &items, &[])
                    }
                    .map_err(|error| error.to_string())?;
                    let extra = if viewer == guid {
                        let stats = codec::build_sheet_stats_values(
                            guid,
                            &super::view::sheet_stats(&entity),
                        );
                        let packet = lyracore_protocol::world::encode(
                            ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(stats)),
                        )
                        .map_err(|error| error.to_string())?;
                        Some((packet.opcode, packet.body))
                    } else {
                        None
                    };
                    (
                        message,
                        extra,
                        if entity.is_player() { 4 } else { 3 },
                        if entity.is_player() {
                            guid
                        } else {
                            entity.owner_guid
                        },
                    )
                }
                crate::aoi::GAME_OBJECT => {
                    let object = ctx
                        .db
                        .game_gameobject()
                        .guid()
                        .find(guid)
                        .ok_or("projected GameObject is missing")?;
                    let view = super::object::view(ctx, &object)?;
                    (
                        codec::build_gameobject_create_object(&view),
                        Some(codec::build_gameobject_rotation_values(&view)),
                        5,
                        0,
                    )
                }
                crate::aoi::CORPSE => {
                    let corpse = ctx
                        .db
                        .game_corpse()
                        .guid()
                        .find(guid)
                        .ok_or("projected Corpse is missing")?;
                    (super::death::create(&corpse), None, 7, corpse.owner_guid)
                }
                crate::aoi::DYNAMIC_OBJECT => {
                    let object = ctx
                        .db
                        .game_dynamic_object()
                        .guid()
                        .find(guid)
                        .ok_or("projected DynamicObject is missing")?;
                    (
                        codec::build_dynamicobject_create_object(
                            guid,
                            object.caster_guid,
                            object.spell_id,
                            object.x,
                            object.y,
                            object.z,
                            object.radius_yd,
                        ),
                        None,
                        6,
                        object.caster_guid,
                    )
                }
                crate::aoi::TRANSPORT => (super::transport::create(ctx, guid)?, None, 5, 0),
                _ => return Err("unknown AOI layer".into()),
            }
        } else if let Some(item) = ctx.db.game_item_instance().guid().find(guid) {
            if item.owner_guid != viewer {
                return Err("item is not visible to conformance Character".into());
            }
            (
                codec::build_item_create_object(&super::view::item(ctx, &item)),
                None,
                1,
                item.owner_guid,
            )
        } else {
            return Err(format!(
                "object {guid} is not visible to conformance Character"
            ));
        };
        let packet = lyracore_protocol::world::encode(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
            Box::new(message),
        ))
        .map_err(|error| error.to_string())?;
        let group_id = |character| {
            ctx.db
                .game_group_member()
                .by_character()
                .filter(character)
                .next()
                .map(|member| member.group_id)
        };
        let same_raid = group_id(viewer).is_some_and(|group| Some(group) == group_id(owner_guid));
        table.insert(NativeConformanceProjection {
            guid,
            object_type,
            owner_guid,
            same_raid,
            body_hex: hex(&packet.body),
            extra_hex: extra.map_or_else(String::new, |(_, body)| hex(&body)),
        });
    }
    Ok(())
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
