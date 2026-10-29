use lyracore_protocol::codec;
use lyracore_protocol::messages::vanilla::opcodes::ServerOpcodeMessage;
use spacetimedb::{ConnectionId, ReducerContext, Table};

use super::game_native_player;
use super::{game_native_packet, NativeAudience, NativePacket};
use crate::aoi::{AoiGrant, CORPSE, DYNAMIC_OBJECT, GAME_OBJECT, TRANSPORT, WORLD_ENTITY};
use crate::{game_corpse, game_dynamic_object, game_gameobject, game_world_entity, WorldEntity};

pub(crate) fn grant_added(ctx: &ReducerContext, grant: &AoiGrant, newly_added: bool) {
    if let Some(player) = ctx
        .db
        .game_native_player()
        .character_guid()
        .find(grant.viewer_guid)
    {
        if player.teleport == Some(super::teleport::NativeTeleport::WorldPort) {
            return;
        }
        if let Err(error) = create(ctx, player.connection_id, grant, newly_added) {
            spacetimedb::log::error!(
                "native CREATE failed for entity {}: {error}",
                grant.entity_guid
            );
            super::disconnect(ctx, player.connection_id);
        }
    }
}

pub(crate) fn grant_removed(ctx: &ReducerContext, grant: &AoiGrant) {
    if let Some(player) = ctx
        .db
        .game_native_player()
        .character_guid()
        .find(grant.viewer_guid)
    {
        if player.teleport == Some(super::teleport::NativeTeleport::WorldPort) {
            return;
        }
        let result = lyracore_protocol::world::encode(ServerOpcodeMessage::SMSG_DESTROY_OBJECT(
            codec::build_destroy_object(grant.entity_guid),
        ));
        match result {
            Ok(packet) => {
                recipient_packet(ctx, player.connection_id, grant, Some(packet), false);
                if grant.layer == WORLD_ENTITY {
                    super::pet::removed(
                        ctx,
                        player.connection_id,
                        grant.viewer_guid,
                        grant.entity_guid,
                    );
                }
            }
            Err(error) => {
                spacetimedb::log::error!(
                    "native DESTROY failed for entity {}: {error}",
                    grant.entity_guid
                );
                super::disconnect(ctx, player.connection_id);
            }
        }
    }
}

fn recipient_packet(
    ctx: &ReducerContext,
    connection: ConnectionId,
    grant: &AoiGrant,
    packet: Option<lyracore_protocol::world::Packet>,
    create: bool,
) {
    let recipient = super::NativeRecipient {
        connection_id: connection,
        layer: grant.layer,
        entity_guid: grant.entity_guid,
    };
    ctx.db.game_native_packet().insert(NativePacket {
        id: 0,
        audience: if create {
            NativeAudience::Grant(recipient)
        } else {
            NativeAudience::Revoke(recipient)
        },
        opcode: packet.as_ref().map(|packet| packet.opcode),
        body: packet.map_or_else(Vec::new, |packet| packet.body),
        close_after: false,
        not_before_micros: 0,
        authentication_complete: false,
    });
}

pub(super) fn create(
    ctx: &ReducerContext,
    connection: ConnectionId,
    grant: &AoiGrant,
    newly_added: bool,
) -> Result<(), String> {
    let mut rotation = None;
    let mut passenger_movement = None;
    let mut message = match grant.layer {
        WORLD_ENTITY => {
            if grant.viewer_guid == grant.entity_guid {
                recipient_packet(ctx, connection, grant, None, true);
                super::aura::replay(ctx, connection, grant.entity_guid, true);
                super::creature::replay(ctx, connection, grant.entity_guid);
                if let Some(entity) = ctx.db.game_world_entity().guid().find(grant.entity_guid) {
                    super::book::talent_points(ctx, entity.guid, entity.level);
                }
                return Ok(());
            }
            let entity = ctx
                .db
                .game_world_entity()
                .guid()
                .find(grant.entity_guid)
                .ok_or("AOI Grant refers to missing WorldEntity")?;
            let mut view = super::view::entity(ctx, &entity);
            super::taxi::create_spline(ctx, entity.guid, &mut view);
            view.dynamic_flags = super::loot::dynamic_flags(ctx, &entity, grant.viewer_guid);
            let items = if entity.is_player() {
                super::view::items(ctx, entity.guid)
            } else {
                Vec::new()
            };
            passenger_movement = super::transport::passenger_movement(ctx, &entity);
            codec::build_create_object(&view, codec::CreateKind::Peer, &items, &[])
                .map_err(|error| error.to_string())?
        }
        GAME_OBJECT => {
            let object = ctx
                .db
                .game_gameobject()
                .guid()
                .find(grant.entity_guid)
                .ok_or("AOI Grant refers to missing GameObject")?;
            let view = super::object::view(ctx, &object)?;
            rotation = Some(codec::build_gameobject_rotation_values(&view));
            codec::build_gameobject_create_object(&view)
        }
        CORPSE => {
            let corpse = ctx
                .db
                .game_corpse()
                .guid()
                .find(grant.entity_guid)
                .ok_or("AOI Grant refers to missing Corpse")?;
            super::death::create(&corpse)
        }
        DYNAMIC_OBJECT => {
            let object = ctx
                .db
                .game_dynamic_object()
                .guid()
                .find(grant.entity_guid)
                .ok_or("AOI Grant refers to missing DynamicObject")?;
            codec::build_dynamicobject_create_object(
                object.guid,
                object.caster_guid,
                object.spell_id,
                object.x,
                object.y,
                object.z,
                object.radius_yd,
            )
        }
        TRANSPORT => super::transport::create(ctx, grant.entity_guid)?,
        _ => return Err("unknown AOI layer".into()),
    };
    codec::set_create_freshness(
        &mut message,
        if newly_added {
            codec::CreateFreshness::New
        } else {
            codec::CreateFreshness::Known
        },
    );
    let packet = match passenger_movement {
        Some(movement) => lyracore_protocol::world::Packet {
            opcode: codec::SMSG_UPDATE_OBJECT_OPCODE,
            body: codec::create_with_movement(&message, &movement)
                .map_err(|error| error.to_string())?,
        },
        None => lyracore_protocol::world::encode(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
            Box::new(message),
        ))
        .map_err(|error| error.to_string())?,
    };
    recipient_packet(ctx, connection, grant, Some(packet), true);
    if let Some((opcode, body)) = rotation {
        super::send_raw(ctx, connection, opcode, body, false);
    }
    if grant.layer == WORLD_ENTITY {
        super::pet::replay(ctx, connection, grant.viewer_guid, grant.entity_guid)?;
        super::combat::replay(ctx, connection, grant.entity_guid)?;
        super::aura::replay(ctx, connection, grant.entity_guid, false);
        super::creature::replay(ctx, connection, grant.entity_guid);
    }
    Ok(())
}

pub(super) fn broadcast(
    ctx: &ReducerContext,
    guid: u64,
    opcode: u16,
    body: Vec<u8>,
    exclude_self: bool,
) {
    if ctx.db.game_native_player().count() == 0 {
        return;
    }
    let audience = super::NativeAoi {
        layer: WORLD_ENTITY,
        entity_guid: guid,
        exclude_connection: exclude_self
            .then(|| {
                ctx.db
                    .game_native_player()
                    .character_guid()
                    .find(guid)
                    .map(|p| p.connection_id)
            })
            .flatten(),
    };
    broadcast_aoi(ctx, audience, opcode, body);
}

pub(super) fn broadcast_aoi(
    ctx: &ReducerContext,
    audience: super::NativeAoi,
    opcode: u16,
    body: Vec<u8>,
) {
    ctx.db.game_native_packet().insert(NativePacket {
        id: 0,
        audience: NativeAudience::Aoi(audience),
        opcode: Some(opcode),
        body,
        close_after: false,
        not_before_micros: 0,
        authentication_complete: false,
    });
}

pub(crate) fn motion(ctx: &ReducerContext, motion: &crate::motion::PendingMotion) {
    if ctx.db.game_native_player().count() == 0 {
        return;
    }
    if let Some((opcode, body)) = codec::build_movement_relay_raw(
        u32::from(motion.opcode),
        motion.guid,
        &motion.movement_info,
    ) {
        broadcast(ctx, motion.guid, opcode, body, true);
    }
}

pub(crate) fn entity_changed(ctx: &ReducerContext, old: &WorldEntity, new: &WorldEntity) {
    if ctx.db.game_native_player().count() == 0 {
        return;
    }
    if new.is_player() {
        super::aura::life_changed(ctx, old, new);
        if let Some(update) = codec::stand_state_changed(old.unit_bytes_1, new.unit_bytes_1) {
            if let Some(owner) = ctx
                .db
                .game_native_player()
                .character_guid()
                .find(new.guid)
                .filter(|owner| owner.teleport != Some(super::teleport::NativeTeleport::WorldPort))
            {
                if let Err(error) = super::send(
                    ctx,
                    owner.connection_id,
                    ServerOpcodeMessage::SMSG_STANDSTATE_UPDATE(update),
                    false,
                ) {
                    spacetimedb::log::error!("native stand state failed for {}: {error}", new.guid);
                }
            }
        }
        if old.zone_id != new.zone_id {
            super::environment::zone_changed(ctx, new);
        }
        if old.level != new.level {
            super::book::talent_points(ctx, new.guid, new.level);
        }
        if let Some(owner) = ctx.db.game_native_player().character_guid().find(new.guid) {
            let mut private = Vec::new();
            if old.xp != new.xp || old.next_level_xp != new.next_level_xp {
                private.push(codec::build_player_xp_values(
                    new.guid,
                    new.xp,
                    new.next_level_xp,
                ));
            }
            if old.money != new.money {
                private.push(codec::build_coinage_values(new.guid, new.money));
            }
            for values in private {
                if let Err(error) = super::send(
                    ctx,
                    owner.connection_id,
                    ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(values)),
                    false,
                ) {
                    spacetimedb::log::error!(
                        "native private VALUES failed for {}: {error}",
                        new.guid
                    );
                }
            }
        }
        if old.player_bytes_2 != new.player_bytes_2 {
            if let Some(owner) = ctx.db.game_native_player().character_guid().find(new.guid) {
                let (opcode, body) = codec::build_rest_state_values(new.guid, new.player_bytes_2);
                super::send_raw(ctx, owner.connection_id, opcode, body, false);
            }
        }
        if old.armor != new.armor {
            if let Some(owner) = ctx.db.game_native_player().character_guid().find(new.guid) {
                let current: Vec<_> = crate::spell::auras_on(ctx, new.guid).collect();
                if let Some(packet) = super::aura::armor(ctx, new.guid, &current) {
                    super::send_raw(ctx, owner.connection_id, packet.opcode, packet.body, false);
                }
            }
        }
        let sheet = super::view::sheet_stats(new);
        if super::view::sheet_stats(old) != sheet {
            if let Some(owner) = ctx.db.game_native_player().character_guid().find(new.guid) {
                let message = ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                    codec::build_sheet_stats_values(new.guid, &sheet),
                ));
                if let Err(error) = super::send(ctx, owner.connection_id, message, false) {
                    spacetimedb::log::error!(
                        "native sheet values failed for {}: {error}",
                        new.guid
                    );
                }
            }
        }
    }
    if old.dynamic_flags != new.dynamic_flags || old.dead != new.dead {
        loot_flags(ctx, new);
    }
    for message in updates(old, new) {
        let (message, exclude_self) = match message {
            ServerOpcodeMessage::SMSG_FORCE_RUN_SPEED_CHANGE(change) => {
                let observed =
                    codec::build_spline_speed(new.guid, codec::ObservedSpeed::Run, change.speed);
                if let Some(owner) = ctx
                    .db
                    .game_native_player()
                    .character_guid()
                    .find(new.guid)
                    .filter(|owner| {
                        owner.teleport != Some(super::teleport::NativeTeleport::WorldPort)
                    })
                {
                    if let Err(error) = super::send(
                        ctx,
                        owner.connection_id,
                        ServerOpcodeMessage::SMSG_FORCE_RUN_SPEED_CHANGE(change),
                        false,
                    ) {
                        spacetimedb::log::error!(
                            "native run speed failed for {}: {error}",
                            new.guid
                        );
                    }
                }
                (observed, true)
            }
            message => (message, false),
        };
        match lyracore_protocol::world::encode(message) {
            Ok(packet) => broadcast(ctx, new.guid, packet.opcode, packet.body, exclude_self),
            Err(error) => {
                spacetimedb::log::error!("native VALUES failed for entity {}: {error}", new.guid)
            }
        }
    }
}
pub(crate) fn loot_flags(ctx: &ReducerContext, entity: &WorldEntity) {
    use crate::aoi::game_aoi_grant;
    if ctx.db.game_native_player().count() == 0 {
        return;
    }
    for grant in ctx
        .db
        .game_aoi_grant()
        .by_subject()
        .filter((WORLD_ENTITY, entity.guid))
    {
        if let Some(owner) = ctx
            .db
            .game_native_player()
            .character_guid()
            .find(grant.viewer_guid)
        {
            let flags = super::loot::dynamic_flags(ctx, entity, grant.viewer_guid);
            let message = ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                codec::build_dynamic_flags_values(entity.guid, flags),
            ));
            if let Err(error) = super::send(ctx, owner.connection_id, message, false) {
                spacetimedb::log::error!(
                    "native Loot Tag values failed for {}: {error}",
                    entity.guid
                );
            }
        }
    }
}

fn updates(old: &WorldEntity, new: &WorldEntity) -> Vec<ServerOpcodeMessage> {
    let mut out = Vec::new();
    let is_player = new.type_mask & lyracore_shared::constants::type_mask::PLAYER_BIT != 0;
    if is_player && old.level != new.level {
        let m = codec::build_level_values(new.guid, new.level);
        out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
    }
    if old.health != new.health {
        let m = codec::build_health_values(new.guid, new.health);
        out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
    }
    if old.target_guid != new.target_guid {
        let m = codec::build_target_values(new.guid, new.target_guid);
        out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
    }
    if old.max_health != new.max_health || old.max_power != new.max_power {
        let power_b = (new.unit_bytes_0 >> 24) as u8;
        let m = codec::build_max_vitals_values(new.guid, new.max_health, power_b, new.max_power);
        out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
    }
    // Mount display and TAXI_FLIGHT must reach the client in one mask.
    let taxi_flight_toggled = (old.unit_flags ^ new.unit_flags)
        & lyracore_shared::constants::unit_flags::TAXI_FLIGHT
        != 0;
    if old.mount_display_id != new.mount_display_id && taxi_flight_toggled {
        let m =
            codec::build_taxi_presentation_values(new.guid, new.mount_display_id, new.unit_flags);
        out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
    } else {
        if old.mount_display_id != new.mount_display_id {
            let m = codec::build_mount_display_values(new.guid, new.mount_display_id);
            out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
        }
        if old.unit_flags != new.unit_flags {
            let m = codec::build_unit_flags_values(new.guid, new.unit_flags);
            out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
        }
    }
    if old.unit_bytes_2 != new.unit_bytes_2 {
        let m = codec::build_sheath_values(new.guid, new.unit_bytes_2);
        out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
    }
    if is_player && (old.player_flags != new.player_flags || old.unit_bytes_1 != new.unit_bytes_1) {
        let m = codec::build_player_flags_values(
            new.guid,
            (old.player_flags != new.player_flags).then_some(new.player_flags),
            (old.unit_bytes_1 != new.unit_bytes_1).then_some(new.unit_bytes_1),
        );
        out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
    }
    if is_player && old.power != new.power {
        let power_b = (new.unit_bytes_0 >> 24) as u8;
        let m = codec::build_power_values(new.guid, power_b, new.power);
        out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
    }
    if old.run_speed_mult_bp != new.run_speed_mult_bp {
        const BASE_RUN_SPEED: f32 = 7.0;
        let speed = BASE_RUN_SPEED * (new.run_speed_mult_bp as f32 / 10_000.0);
        out.push(ServerOpcodeMessage::SMSG_FORCE_RUN_SPEED_CHANGE(Box::new(
            codec::build_force_run_speed(new.guid, speed),
        )));
    }
    if old.display_id != new.display_id {
        out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
            codec::build_display_values(new.guid, new.display_id),
        )));
    }
    out
}
