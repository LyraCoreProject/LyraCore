use crate::aoi::game_aoi_grant;
use crate::group::game_group_member;
use crate::items::game_item_instance;
use crate::native::{self, game_native_player};
use crate::skill::game_player_skill;
use crate::{game_corpse, game_dynamic_object, game_gameobject, game_world_entity};
use lyracore_protocol::{
    codec,
    messages::vanilla::{opcodes::ServerOpcodeMessage, SMSG_UPDATE_OBJECT},
};
use spacetimedb::{reducer, table, ConnectionId, ReducerContext, Table};

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

#[table(accessor = game_native_conformance_cache, index(accessor = by_connection, btree(columns = [connection_id])))]
pub struct NativeConformanceCache {
    #[primary_key]
    pub key: String,
    pub connection_id: ConnectionId,
    pub guid: u64,
    pub source: String,
}

pub(super) fn close(ctx: &ReducerContext, connection_id: ConnectionId) {
    ctx.db
        .game_native_conformance_cache()
        .by_connection()
        .delete(connection_id);
}

fn encode(message: SMSG_UPDATE_OBJECT) -> Result<Vec<u8>, String> {
    lyracore_protocol::world::encode(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(message)))
        .map(|packet| packet.body)
        .map_err(|error| error.to_string())
}

fn encoded(
    ctx: &ReducerContext,
    connection_id: ConnectionId,
    guid: u64,
    full: bool,
    source: &impl std::fmt::Debug,
    build: impl FnOnce() -> Result<(SMSG_UPDATE_OBJECT, Option<Vec<u8>>), String>,
) -> Result<(String, String), String> {
    // Derived Debug covers added inputs; float bit patterns also distinguish NaN payloads.
    let source = format!("{source:?}");
    let key = format!("{connection_id}:{guid}");
    let cache = ctx.db.game_native_conformance_cache();
    if !full
        && cache
            .key()
            .find(&key)
            .is_some_and(|old| old.source == source)
    {
        return Ok((String::new(), String::new()));
    }
    let (message, extra) = build()?;
    let body_hex = hex(&encode(message)?);
    let extra_hex = extra.map_or_else(String::new, |body| hex(&body));
    cache.key().delete(&key);
    cache.insert(NativeConformanceCache {
        key,
        connection_id,
        guid,
        source,
    });
    Ok((body_hex, extra_hex))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn entity_projection(
    ctx: &ReducerContext,
    connection_id: ConnectionId,
    guid: u64,
    viewer: u64,
    full: bool,
) -> Result<(String, String, u8, u64), String> {
    let entity = ctx
        .db
        .game_world_entity()
        .guid()
        .find(guid)
        .ok_or("projected WorldEntity is missing")?;
    let mut view = native::view::entity(ctx, &entity);
    let items = if entity.is_player() {
        native::view::items(ctx, guid)
    } else {
        Vec::new()
    };
    let skills = if viewer == guid {
        ctx.db
            .game_player_skill()
            .by_character()
            .filter(guid)
            .map(|skill| (skill.skill_line, skill.current, skill.max_rank))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let quests = if viewer == guid {
        native::view::quests(ctx, guid)
    } else {
        Vec::new()
    };
    if viewer != guid {
        view.dynamic_flags = native::loot::dynamic_flags(ctx, &entity, viewer);
    }
    // These properties compare update fields, not CREATE movement blocks.
    view.x = 0.0;
    view.y = 0.0;
    view.z = 0.0;
    view.orientation = 0.0;
    view.last_move_ms = 0;
    view.movement_flags = 0;
    view.run_speed_mult_bp = 10_000;
    let stats = (viewer == guid).then(|| native::view::sheet_stats(&entity));
    let (body, extra) = encoded(
        ctx,
        connection_id,
        guid,
        full,
        &(
            &view,
            view.scale_x.to_bits(),
            &items,
            &skills,
            &quests,
            &stats,
            stats.as_ref().map(|stats| stats.crit_pct.to_bits()),
        ),
        || {
            let message = if viewer == guid {
                codec::build_self_create_object(&view, &items, &skills, &quests)
            } else {
                codec::build_create_object(&view, codec::CreateKind::Peer, &items, &[])
            }
            .map_err(|error| error.to_string())?;
            let extra = stats
                .as_ref()
                .map(|stats| encode(codec::build_sheet_stats_values(guid, stats)))
                .transpose()?;
            Ok((message, extra))
        },
    )?;
    Ok((
        body,
        extra,
        if entity.is_player() { 4 } else { 3 },
        if entity.is_player() {
            guid
        } else {
            entity.owner_guid
        },
    ))
}

#[reducer]
pub fn debug_native_project(
    ctx: &ReducerContext,
    full: bool,
    advance_micros: u64,
) -> Result<(), String> {
    if advance_micros != 0 {
        crate::time::debug_advance_virtual_clock(
            ctx,
            advance_micros
                .try_into()
                .map_err(|_| "clock advance overflows")?,
        )?;
    }
    let connection_id = native::connection_id(ctx)?;
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
    let cache = ctx.db.game_native_conformance_cache();
    for old in cache.by_connection().filter(connection_id) {
        if !guids.contains(&old.guid) {
            cache.key().delete(old.key);
        }
    }
    let table = ctx.db.game_native_conformance_projection();
    for guid in guids {
        let grant = ctx
            .db
            .game_aoi_grant()
            .by_viewer()
            .filter(viewer)
            .find(|grant| grant.entity_guid == guid);
        let (body_hex, extra_hex, object_type, owner_guid) = if let Some(grant) = grant {
            match grant.layer {
                crate::aoi::WORLD_ENTITY => {
                    entity_projection(ctx, connection_id, guid, viewer, full)?
                }
                crate::aoi::GAME_OBJECT => {
                    let object = ctx
                        .db
                        .game_gameobject()
                        .guid()
                        .find(guid)
                        .ok_or("projected GameObject is missing")?;
                    let view = native::object::view(ctx, &object)?;
                    let (body, extra) = encoded(
                        ctx,
                        connection_id,
                        guid,
                        full,
                        &(
                            &view,
                            [
                                view.x,
                                view.y,
                                view.z,
                                view.orientation,
                                view.rotation_0,
                                view.rotation_1,
                                view.rotation_2,
                                view.rotation_3,
                                view.size,
                            ]
                            .map(f32::to_bits),
                        ),
                        || {
                            Ok((
                                codec::build_gameobject_create_object(&view),
                                Some(codec::build_gameobject_rotation_values(&view).1),
                            ))
                        },
                    )?;
                    (body, extra, 5, 0)
                }
                crate::aoi::CORPSE => {
                    let corpse = ctx
                        .db
                        .game_corpse()
                        .guid()
                        .find(guid)
                        .ok_or("projected Corpse is missing")?;
                    {
                        let source =
                            spacetimedb::sats::bsatn::to_vec(&corpse).map_err(|e| e.to_string())?;
                        let (body, extra) =
                            encoded(ctx, connection_id, guid, full, &source, || {
                                Ok((native::death::create(&corpse), None))
                            })?;
                        (body, extra, 7, corpse.owner_guid)
                    }
                }
                crate::aoi::DYNAMIC_OBJECT => {
                    let object = ctx
                        .db
                        .game_dynamic_object()
                        .guid()
                        .find(guid)
                        .ok_or("projected DynamicObject is missing")?;
                    let source =
                        spacetimedb::sats::bsatn::to_vec(&object).map_err(|e| e.to_string())?;
                    let (body, extra) = encoded(ctx, connection_id, guid, full, &source, || {
                        Ok((
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
                        ))
                    })?;
                    (body, extra, 6, object.caster_guid)
                }
                crate::aoi::TRANSPORT => (
                    hex(&encode(native::transport::create(ctx, guid)?)?),
                    String::new(),
                    5,
                    0,
                ),
                _ => return Err("unknown AOI layer".into()),
            }
        } else if let Some(item) = ctx.db.game_item_instance().guid().find(guid) {
            if item.owner_guid != viewer {
                return Err("item is not visible to conformance Character".into());
            }
            let view = native::view::item(ctx, &item);
            let (body, extra) = encoded(ctx, connection_id, guid, full, &view, || {
                Ok((codec::build_item_create_object(&view), None))
            })?;
            (body, extra, 1, item.owner_guid)
        } else {
            return Err(format!(
                "object {guid} is not visible to conformance Character"
            ));
        };
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
            body_hex,
            extra_hex,
        });
    }
    Ok(())
}
