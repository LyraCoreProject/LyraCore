use std::cell::RefCell;
use std::collections::HashMap;

use lyracore_protocol::codec;
use lyracore_protocol::messages::vanilla::{
    opcodes::ServerOpcodeMessage, Object, UpdateMask, SMSG_UPDATE_OBJECT,
};
use spacetimedb::{ConnectionId, ReducerContext, Table};

use super::game_native_player;
use super::{game_native_packet, NativeAudience, NativePacket};
use crate::aoi::{AoiGrant, CORPSE, DYNAMIC_OBJECT, GAME_OBJECT, TRANSPORT, WORLD_ENTITY};
use crate::{game_corpse, game_dynamic_object, game_gameobject, game_world_entity, WorldEntity};

type PeerParts = (
    codec::EntityView,
    Vec<codec::ItemInstanceView>,
    Option<Vec<u8>>,
);

thread_local! {
    static PEER_PARTS: RefCell<Option<HashMap<u64, PeerParts>>> = const { RefCell::new(None) };
}

/// One subject's Grants fan out inside `fan_out`, so its CREATE parts are read once.
pub(crate) fn with_shared_creates<T>(fan_out: impl FnOnce() -> T) -> T {
    struct Restore(Option<HashMap<u64, PeerParts>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            PEER_PARTS.with(|parts| *parts.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(PEER_PARTS.with(|parts| parts.replace(Some(HashMap::new()))));
    fan_out()
}

fn peer_parts(ctx: &ReducerContext, entity: &WorldEntity) -> PeerParts {
    if let Some(hit) = PEER_PARTS.with(|parts| parts.borrow().as_ref()?.get(&entity.guid).cloned())
    {
        return hit;
    }
    let mut view = super::view::entity(ctx, entity);
    super::taxi::create_spline(ctx, entity.guid, &mut view);
    let items = if entity.is_player() {
        super::view::equipment(ctx, entity.guid)
    } else {
        Vec::new()
    };
    let parts = (
        view,
        items,
        super::transport::passenger_movement(ctx, entity),
    );
    PEER_PARTS.with(|cache| {
        if let Some(cache) = cache.borrow_mut().as_mut() {
            cache.insert(entity.guid, parts.clone());
        }
    });
    parts
}

pub(crate) fn grant_added(ctx: &ReducerContext, grant: &AoiGrant, newly_added: bool) {
    if let Some(player) = super::world_session(ctx, grant.viewer_guid) {
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
    if let Some(player) = super::world_session(ctx, grant.viewer_guid) {
        if player.teleport == Some(super::teleport::NativeTeleport::WorldPort) {
            return;
        }
        let result = lyracore_protocol::world::encode(&ServerOpcodeMessage::SMSG_DESTROY_OBJECT(
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
) -> u64 {
    let recipient = super::NativeRecipient {
        connection_id: connection,
        layer: grant.layer,
        entity_guid: grant.entity_guid,
    };
    ctx.db
        .game_native_packet()
        .insert(NativePacket {
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
        })
        .id
}

pub(super) fn create(
    ctx: &ReducerContext,
    connection: ConnectionId,
    grant: &AoiGrant,
    newly_added: bool,
) -> Result<(), String> {
    let mut rotation = None;
    let mut passenger_movement = None;
    let mut freshness = if newly_added {
        codec::CreateFreshness::New
    } else {
        codec::CreateFreshness::Known
    };
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
            if entity.is_player() {
                freshness = codec::CreateFreshness::Known;
            }
            let (mut view, items, movement) = peer_parts(ctx, &entity);
            view.dynamic_flags = super::loot::dynamic_flags(ctx, &entity, grant.viewer_guid);
            (view.health, view.max_health) =
                super::health::visible_health(ctx, &entity, grant.viewer_guid);
            passenger_movement = movement;
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
            if matches!(
                view.type_id,
                lyracore_shared::constants::go_type::ELEVATOR
                    | lyracore_shared::constants::go_type::MO_TRANSPORT
            ) {
                freshness = codec::CreateFreshness::Known;
            }
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
        TRANSPORT => {
            freshness = codec::CreateFreshness::Known;
            super::transport::create(ctx, grant.entity_guid)?
        }
        _ => return Err("unknown AOI layer".into()),
    };
    codec::set_create_freshness(&mut message, freshness);
    let packet = match passenger_movement {
        Some(movement) => lyracore_protocol::world::Packet {
            opcode: codec::SMSG_UPDATE_OBJECT_OPCODE,
            body: codec::create_with_movement(&message, &movement)
                .map_err(|error| error.to_string())?,
        },
        None => lyracore_protocol::world::encode(&ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
            Box::new(message),
        ))
        .map_err(|error| error.to_string())?,
    };
    let packet_id = recipient_packet(ctx, connection, grant, Some(packet), true);
    #[cfg(feature = "conformance_test_fixture")]
    {
        let gameobject_type = if grant.layer == GAME_OBJECT {
            ctx.db
                .game_gameobject()
                .guid()
                .find(grant.entity_guid)
                .and_then(|object| super::object::view(ctx, &object).ok())
                .map_or(0, |view| view.type_id)
        } else {
            0
        };
        super::debug::note_create(
            ctx,
            packet_id,
            grant.entity_guid,
            grant.viewer_guid,
            grant.layer,
            gameobject_type,
        );
    }
    #[cfg(not(feature = "conformance_test_fixture"))]
    let _ = packet_id;
    if let Some((opcode, body)) = rotation {
        super::send_raw(ctx, connection, opcode, body, false);
    }
    if grant.layer == WORLD_ENTITY {
        super::pet::replay(ctx, connection, grant.viewer_guid, grant.entity_guid)?;
        super::combat::replay(ctx, connection, grant.entity_guid, grant.viewer_guid)?;
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
            .then(|| super::world_session(ctx, guid).map(|p| p.connection_id))
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
            if !super::is_world_porting(ctx, new.guid) {
                super::send_to_character(
                    ctx,
                    new.guid,
                    ServerOpcodeMessage::SMSG_STANDSTATE_UPDATE(update),
                    super::EncodeFailure::Log,
                );
            }
        }
        if old.zone_id != new.zone_id {
            super::environment::zone_changed(ctx, new);
        }
        if old.zone_id != new.zone_id
            || old.level != new.level
            || lyracore_shared::chat::chat_tag_for(old.player_flags)
                != lyracore_shared::chat::chat_tag_for(new.player_flags)
        {
            super::realm_party::presence(ctx, new.guid, true);
        }
        if old.level != new.level {
            super::book::talent_points(ctx, new.guid, new.level);
        }
        if super::has_world_session(ctx, new.guid) {
            let mut private = Vec::new();
            if old.xp != new.xp || old.next_level_xp != new.next_level_xp {
                private.push(codec::build_player_xp_values(
                    new.guid,
                    (old.xp != new.xp).then_some(new.xp),
                    (old.next_level_xp != new.next_level_xp).then_some(new.next_level_xp),
                ));
            }
            if old.money != new.money {
                private.push(if old.bank_bag_slots != new.bank_bag_slots {
                    codec::build_bank_purchase_values(new.guid, new.money, new.player_bytes_2)
                } else {
                    codec::build_coinage_values(new.guid, new.money)
                });
            }
            for values in private {
                super::send_to_character(
                    ctx,
                    new.guid,
                    ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(values)),
                    super::EncodeFailure::Log,
                );
            }
        }
        if old.armor != new.armor && super::has_world_session(ctx, new.guid) {
            let current: Vec<_> = crate::spell::auras_on(ctx, new.guid).collect();
            if let Some(packet) = super::aura::armor(ctx, new.guid, &current) {
                super::send_raw_to_character(ctx, new.guid, packet.opcode, packet.body);
            }
        }
        let sheet = super::view::sheet_stats(new);
        if super::view::sheet_stats(old) != sheet {
            super::send_to_character(
                ctx,
                new.guid,
                ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(codec::build_sheet_stats_values(
                    new.guid, &sheet,
                ))),
                super::EncodeFailure::Log,
            );
        }
    }
    if old.dynamic_flags != new.dynamic_flags || old.dead != new.dead {
        loot_flags(ctx, new);
    }
    for update in updates(old, new) {
        let message = match update {
            Update::Shared(message) => message,
            Update::Raw(opcode, body) => {
                broadcast(ctx, new.guid, opcode, body, false);
                continue;
            }
            Update::Health => {
                super::health::send(ctx, new);
                continue;
            }
        };
        let (message, exclude_self) = match message {
            ServerOpcodeMessage::SMSG_UPDATE_OBJECT(ref update)
                if old.money != new.money
                    && old.bank_bag_slots != new.bank_bag_slots
                    && update.objects.iter().any(|object| {
                        matches!(object, Object::Values { mask1: UpdateMask::Player(fields), .. }
                            if fields.player_bytes_2().is_some())
                    }) =>
            {
                (message, true)
            }
            ServerOpcodeMessage::SMSG_FORCE_RUN_SPEED_CHANGE(change) => {
                let observed =
                    codec::build_spline_speed(new.guid, codec::ObservedSpeed::Run, change.speed);
                if !super::is_world_porting(ctx, new.guid) {
                    super::send_to_character(
                        ctx,
                        new.guid,
                        ServerOpcodeMessage::SMSG_FORCE_RUN_SPEED_CHANGE(change),
                        super::EncodeFailure::Log,
                    );
                }
                (observed, true)
            }
            message => (message, false),
        };
        match lyracore_protocol::world::encode(&message) {
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
        if super::has_world_session(ctx, grant.viewer_guid) {
            let flags = super::loot::dynamic_flags(ctx, entity, grant.viewer_guid);
            match lyracore_protocol::world::encode(&ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
                Box::new(codec::build_dynamic_flags_values(entity.guid, flags)),
            )) {
                Ok(packet) => {
                    super::send_raw_to_character(
                        ctx,
                        grant.viewer_guid,
                        packet.opcode,
                        packet.body,
                    );
                }
                Err(error) => spacetimedb::log::error!(
                    "native Loot Tag values failed for {}: {error}",
                    entity.guid
                ),
            }
        }
    }
}

#[derive(Debug)]
enum Update {
    Shared(ServerOpcodeMessage),
    Raw(u16, Vec<u8>),
    Health,
}

/// The power type and every pool, shown or reserved, that changed. A pool that appears sends its
/// current value, as vmangos `Unit::SetPowerType` sets it, and a pool that ended reads zero; a
/// creature's current power is never sent.
fn power_values(old: &WorldEntity, new: &WorldEntity, is_player: bool) -> Option<(u16, Vec<u8>)> {
    use lyracore_shared::packing::power_type;
    let pools: Vec<_> = (power_type::MANA..=power_type::HAPPINESS)
        .filter_map(|power_type| {
            let (was, now) = (old.pool(power_type), new.pool(power_type));
            let current = now.map_or(0, |pool| pool.current);
            let max = now.map(|pool| pool.max);
            let fields = codec::PowerFields {
                power_type,
                current: (is_player
                    && (was.is_none() && now.is_some()
                        || was.map_or(0, |pool| pool.current) != current))
                    .then_some(current),
                max: max.filter(|_| was.map(|pool| pool.max) != max),
            };
            (fields.current.is_some() || fields.max.is_some()).then_some(fields)
        })
        .collect();
    let unit_bytes_0 = (old.unit_bytes_0 != new.unit_bytes_0).then_some(new.unit_bytes_0);
    (unit_bytes_0.is_some() || !pools.is_empty())
        .then(|| codec::build_power_values(new.guid, unit_bytes_0, &pools))
}

/// A creature that takes another template in place sends its entry, scale, level, faction and NPC
/// flags in one mask.
fn creature_template_values(old: &WorldEntity, new: &WorldEntity) -> Option<(u16, Vec<u8>)> {
    use lyracore_protocol::codec::update_mask::{idx, UpdateMaskValues};
    let changed = old.entry != new.entry
        || old.scale_x != new.scale_x
        || old.level != new.level
        || old.faction_template != new.faction_template
        || old.npc_flags != new.npc_flags;
    if new.is_player() || !changed {
        return None;
    }
    let mut mask = UpdateMaskValues::new();
    mask.set_u32(idx::OBJECT_ENTRY, new.entry)
        .set_f32(idx::OBJECT_SCALE_X, new.scale_x)
        .set_u32(idx::UNIT_LEVEL, new.level)
        .set_u32(idx::UNIT_FACTIONTEMPLATE, new.faction_template)
        .set_u32(idx::UNIT_NPC_FLAGS, new.npc_flags);
    Some(codec::build_values_update_raw(new.guid, &mask))
}

/// A Character's scale; a creature's rides in `creature_template_values`.
fn player_scale_values(old: &WorldEntity, new: &WorldEntity) -> Option<(u16, Vec<u8>)> {
    use lyracore_protocol::codec::update_mask::{idx, UpdateMaskValues};
    if !new.is_player() || old.scale_x == new.scale_x {
        return None;
    }
    let mut mask = UpdateMaskValues::new();
    mask.set_f32(idx::OBJECT_SCALE_X, new.scale_x);
    Some(codec::build_values_update_raw(new.guid, &mask))
}

fn values(update: SMSG_UPDATE_OBJECT) -> Update {
    Update::Shared(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(update)))
}

fn updates(old: &WorldEntity, new: &WorldEntity) -> Vec<Update> {
    let mut out = Vec::new();
    let is_player = new.type_mask & lyracore_shared::constants::type_mask::PLAYER_BIT != 0;
    if is_player && old.player_bytes_2 != new.player_bytes_2 {
        let m = codec::build_bank_bag_slots_values(new.guid, new.player_bytes_2);
        out.push(values(m));
    }
    if is_player && old.player_bytes_3 != new.player_bytes_3 {
        let m = codec::build_player_bytes_3_values(new.guid, new.player_bytes_3);
        out.push(values(m));
    }
    if is_player && old.level != new.level {
        let m = codec::build_level_values(new.guid, new.level);
        out.push(values(m));
    }
    if old.health != new.health || old.max_health != new.max_health {
        out.push(Update::Health);
    }
    if old.target_guid != new.target_guid {
        let m = codec::build_target_values(new.guid, new.target_guid);
        out.push(values(m));
    }
    if let Some((opcode, body)) = power_values(old, new, is_player) {
        out.push(Update::Raw(opcode, body));
    }
    if let Some((opcode, body)) = creature_template_values(old, new) {
        out.push(Update::Raw(opcode, body));
    }
    if let Some((opcode, body)) = player_scale_values(old, new) {
        out.push(Update::Raw(opcode, body));
    }
    // Mount display and TAXI_FLIGHT must reach the client in one mask.
    let taxi_flight_toggled = (old.unit_flags ^ new.unit_flags)
        & lyracore_shared::constants::unit_flags::TAXI_FLIGHT
        != 0;
    if old.mount_display_id != new.mount_display_id && taxi_flight_toggled {
        let m =
            codec::build_taxi_presentation_values(new.guid, new.mount_display_id, new.unit_flags);
        out.push(values(m));
    } else {
        if old.mount_display_id != new.mount_display_id {
            let m = codec::build_mount_display_values(new.guid, new.mount_display_id);
            out.push(values(m));
        }
        if old.unit_flags != new.unit_flags {
            let m = codec::build_unit_flags_values(new.guid, new.unit_flags);
            out.push(values(m));
        }
    }
    if old.unit_bytes_2 != new.unit_bytes_2 {
        let m = codec::build_sheath_values(new.guid, new.unit_bytes_2);
        out.push(values(m));
    }
    if is_player && (old.player_flags != new.player_flags || old.unit_bytes_1 != new.unit_bytes_1) {
        let m = codec::build_player_flags_values(
            new.guid,
            (old.player_flags != new.player_flags).then_some(new.player_flags),
            (old.unit_bytes_1 != new.unit_bytes_1).then_some(new.unit_bytes_1),
        );
        out.push(values(m));
    }
    if old.run_speed_mult_bp != new.run_speed_mult_bp {
        const BASE_RUN_SPEED: f32 = 7.0;
        let speed = BASE_RUN_SPEED * (new.run_speed_mult_bp as f32 / 10_000.0);
        out.push(Update::Shared(
            ServerOpcodeMessage::SMSG_FORCE_RUN_SPEED_CHANGE(Box::new(
                codec::build_force_run_speed(new.guid, speed),
            )),
        ));
    }
    if old.display_id != new.display_id {
        out.push(values(codec::build_display_values(
            new.guid,
            new.display_id,
        )));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_protocol::messages::vanilla::{Object, UpdateMask};

    fn power_fields(old: &WorldEntity, new: &WorldEntity) -> Vec<(u16, u32)> {
        let Some((_, body)) = power_values(old, new, new.is_player()) else {
            return Vec::new();
        };
        lyracore_protocol::codec::values_mask::parse_values_updates(&body)
            .remove(0)
            .fields
    }

    fn druid() -> WorldEntity {
        let mut druid = crate::helpers::tests::entity(5, 0, 0, 0, 0);
        druid.type_mask = lyracore_shared::constants::type_mask::PLAYER_BIT;
        druid.unit_bytes_0 = lyracore_shared::packing::unit_bytes_0(4, 11, 0, 0);
        druid.power = 700;
        druid.max_power = 900;
        druid
    }

    // Build 5875: POWER1 23, POWER2 24, MAXPOWER1 29, MAXPOWER2 30, UNIT_FIELD_BYTES_0 36.
    #[test]
    fn showing_a_form_pool_sends_the_power_type_and_the_new_pool() {
        let old = druid();
        let mut new = druid();
        new.show_pool(lyracore_shared::packing::power_type::RAGE);
        assert_eq!(
            power_fields(&old, &new),
            [(24, 0), (30, 1000), (36, new.unit_bytes_0)],
            "mana stays as the client has it; rage starts at zero"
        );
    }

    #[test]
    fn after_a_create_entering_an_energy_form_shows_zero_energy() {
        use lyracore_protocol::codec::{self, CreateKind};
        use lyracore_protocol::messages::vanilla::ServerMessage;
        let caster = druid();
        let mut cat = druid();
        cat.show_pool(lyracore_shared::packing::power_type::ENERGY);
        let view = codec::EntityView {
            guid: caster.guid,
            type_mask: caster.type_mask,
            scale_x: 1.0,
            health: 1,
            max_health: 1,
            level: 1,
            run_speed_mult_bp: 10_000,
            unit_bytes_0: caster.unit_bytes_0,
            power: caster.power,
            max_power: caster.max_power,
            ..Default::default()
        };
        let create = codec::build_create_object(&view, CreateKind::Peer, &[], &[]).unwrap();
        let mask2 = match create.objects[0].clone() {
            Object::CreateObject { mask2, .. } | Object::CreateObject2 { mask2, .. } => mask2,
            other => panic!("expected a CREATE, got {other:?}"),
        };
        let as_values = lyracore_protocol::messages::vanilla::SMSG_UPDATE_OBJECT {
            has_transport: 0,
            objects: vec![Object::Values {
                guid1: lyracore_protocol::messages::Guid::new(caster.guid),
                mask1: mask2,
            }],
        };
        let mut bytes = Vec::new();
        as_values.write_unencrypted_server(&mut bytes).unwrap();
        let mut client: std::collections::BTreeMap<u16, u32> =
            codec::values_mask::parse_values_updates(&bytes[4..])
                .remove(0)
                .fields
                .into_iter()
                .collect();
        assert_eq!(
            (client[&26], client[&32]),
            (100, 100),
            "CREATE: POWER4, MAXPOWER4"
        );
        client.extend(power_fields(&caster, &cat));
        assert_eq!(
            (client[&26], client[&32]),
            (0, 100),
            "Cat Form starts at zero energy"
        );
    }

    // Build 5875: OBJECT_ENTRY 3, OBJECT_SCALE_X 4, UNIT_LEVEL 34, UNIT_FACTIONTEMPLATE 35,
    // UNIT_NPC_FLAGS 147.
    #[test]
    fn a_creature_that_takes_another_template_sends_its_new_identity() {
        let villager = crate::helpers::tests::entity(7, 0, 0, 0, 0);
        let mut worgen = crate::helpers::tests::entity(7, 0, 0, 0, 0);
        worgen.entry = 1_892;
        worgen.scale_x = 1.5;
        worgen.level = 21;
        worgen.faction_template = 16;
        worgen.npc_flags = 0;
        let (_, body) = creature_template_values(&villager, &worgen).unwrap();
        let fields = lyracore_protocol::codec::values_mask::parse_values_updates(&body)
            .remove(0)
            .fields;
        assert_eq!(
            fields,
            [
                (3, 1_892),
                (4, 1.5f32.to_bits()),
                (34, 21),
                (35, 16),
                (147, 0)
            ]
        );
        assert!(
            creature_template_values(&villager, &crate::helpers::tests::entity(7, 0, 0, 0, 0))
                .is_none()
        );
        assert!(creature_template_values(&druid(), &{
            let mut leveled = druid();
            leveled.level += 1;
            leveled
        })
        .is_none());
    }

    #[test]
    fn a_character_whose_scale_changes_sends_only_the_scale() {
        let mut cat = druid();
        cat.scale_x = 0.8;
        let (_, body) = player_scale_values(&druid(), &cat).unwrap();
        let fields = lyracore_protocol::codec::values_mask::parse_values_updates(&body)
            .remove(0)
            .fields;
        assert_eq!(fields, [(4, 0.8f32.to_bits())]);
        assert!(player_scale_values(&druid(), &druid()).is_none());
    }

    #[test]
    fn reserved_mana_and_an_ended_form_pool_reach_the_client() {
        let in_bear_form = |reserved_mana| {
            let mut druid = druid();
            druid.show_pool(lyracore_shared::packing::power_type::RAGE);
            druid.power = 40;
            druid.reserve_power = reserved_mana;
            druid
        };
        assert_eq!(
            power_fields(&in_bear_form(700), &in_bear_form(720)),
            [(23, 720)]
        );

        let mut left = in_bear_form(720);
        left.show_pool(lyracore_shared::packing::power_type::MANA);
        assert_eq!(
            power_fields(&in_bear_form(720), &left),
            [(24, 0), (36, left.unit_bytes_0)],
            "rage ends at zero and mana is shown again"
        );
    }

    #[test]
    fn bank_slot_purchase_updates_public_fields_without_coinage() {
        let mut old = crate::helpers::tests::entity(5, 0, 0, 0, 0);
        old.type_mask = lyracore_shared::constants::type_mask::PLAYER_BIT;
        old.money = 10_000;
        old.player_bytes_2 = 0x0200_0000;
        let mut new = crate::helpers::tests::entity(5, 0, 0, 0, 0);
        new.type_mask = old.type_mask;
        new.money = 9_000;
        new.bank_bag_slots = 1;
        new.player_bytes_2 = 0x0201_0000;

        let messages = updates(&old, &new);
        let [Update::Shared(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(update))] = messages.as_slice()
        else {
            panic!("the purchase must update the public bank slot field: {messages:?}");
        };
        let [Object::Values {
            guid1,
            mask1: UpdateMask::Player(fields),
        }] = update.objects.as_slice()
        else {
            panic!("expected Character VALUES: {update:?}");
        };
        assert_eq!(guid1.guid(), 5);
        assert_eq!(fields.player_bytes_2(), Some((0, 0xee, 1, 2)));
        assert_eq!(fields.player_field_coinage(), None);
    }

    #[test]
    fn a_drunk_value_change_updates_player_bytes_3_alone() {
        let mut old = crate::helpers::tests::entity(5, 0, 0, 0, 0);
        old.type_mask = lyracore_shared::constants::type_mask::PLAYER_BIT;
        old.player_bytes_3 = 1;
        let mut new = crate::helpers::tests::entity(5, 0, 0, 0, 0);
        new.type_mask = old.type_mask;
        new.player_bytes_3 = 0x3201;

        let messages = updates(&old, &new);
        let [Update::Shared(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(update))] = messages.as_slice()
        else {
            panic!("the drunk value must update PLAYER_BYTES_3: {messages:?}");
        };
        let [Object::Values {
            mask1: UpdateMask::Player(fields),
            ..
        }] = update.objects.as_slice()
        else {
            panic!("expected Character VALUES: {update:?}");
        };
        assert_eq!(fields.player_bytes_3(), Some((0x01, 0x32, 0, 0)));
    }
}
