use lyracore_protocol::{
    codec,
    messages::vanilla::{opcodes::ServerOpcodeMessage, WeatherChangeType},
};
use spacetimedb::{ConnectionId, ReducerContext};

use super::game_native_player;
use crate::{game_zone_weather, BreathRelayEvent, WorldEntity, ZoneWeather};

fn weather_packet(
    weather: codec::ZoneWeatherView,
    change: WeatherChangeType,
) -> lyracore_protocol::world::Packet {
    lyracore_protocol::world::encode(ServerOpcodeMessage::SMSG_WEATHER(Box::new(
        codec::build_weather(weather, change),
    )))
    .expect("weather fits a world frame")
}

pub(super) fn enter_zone(ctx: &ReducerContext, connection: ConnectionId, map: u32, zone: u32) {
    let (opcode, body) = codec::build_init_world_states(map, zone);
    super::send_raw(ctx, connection, opcode, body, false);
    let weather = ctx
        .db
        .game_zone_weather()
        .zone_id()
        .find(zone)
        .map(|row| codec::ZoneWeatherView {
            weather_type: row.weather_type,
            intensity: row.intensity,
        })
        .unwrap_or_default();
    let packet = weather_packet(weather, WeatherChangeType::Smooth);
    super::send_raw(ctx, connection, packet.opcode, packet.body, false);
}

pub(super) fn zone_changed(ctx: &ReducerContext, entity: &WorldEntity) {
    if let Some(mut owner) = ctx
        .db
        .game_native_player()
        .character_guid()
        .find(entity.guid)
    {
        owner.zone_id = entity.zone_id;
        let connection = owner.connection_id;
        ctx.db.game_native_player().connection_id().update(owner);
        enter_zone(ctx, connection, entity.map_id, entity.zone_id);
    }
}

pub(crate) fn weather_changed(ctx: &ReducerContext, row: &ZoneWeather) {
    let packet = weather_packet(
        codec::ZoneWeatherView {
            weather_type: row.weather_type,
            intensity: row.intensity,
        },
        WeatherChangeType::Smooth,
    );
    for owner in ctx.db.game_native_player().by_zone().filter(row.zone_id) {
        super::send_raw(
            ctx,
            owner.connection_id,
            packet.opcode,
            packet.body.clone(),
            false,
        );
    }
}

pub(crate) fn breath_event(ctx: &ReducerContext, row: &BreathRelayEvent) {
    let Some(owner) = ctx
        .db
        .game_native_player()
        .character_guid()
        .find(row.character_guid)
    else {
        return;
    };
    let message = match row.kind {
        crate::breath_relay::KIND_START => {
            codec::build_breath_timer_start(row.time_remaining_ms, row.duration_ms)
        }
        crate::breath_relay::KIND_STOP => codec::build_breath_timer_stop(),
        crate::breath_relay::KIND_DROWNING_DAMAGE => {
            codec::build_drowning_damage_log(row.character_guid, row.damage)
        }
        crate::breath_relay::KIND_FATIGUE_DRAIN => {
            codec::build_fatigue_timer_start(true, row.time_remaining_ms, row.duration_ms)
        }
        crate::breath_relay::KIND_FATIGUE_RECOVER => {
            codec::build_fatigue_timer_start(false, row.time_remaining_ms, row.duration_ms)
        }
        crate::breath_relay::KIND_FATIGUE_STOP => codec::build_fatigue_timer_stop(),
        crate::breath_relay::KIND_EXHAUSTION_DAMAGE => {
            codec::build_exhaustion_damage_log(row.character_guid, row.damage)
        }
        _ => return,
    };
    let packet =
        lyracore_protocol::world::encode(message).expect("breath event fits a world frame");
    super::send_raw(ctx, owner.connection_id, packet.opcode, packet.body, false);
}

pub(crate) fn fall_damage(ctx: &ReducerContext, guid: u64, damage: u32) {
    if damage == 0 {
        return;
    }
    if let Some(owner) = ctx.db.game_native_player().character_guid().find(guid) {
        let message = ServerOpcodeMessage::SMSG_ENVIRONMENTAL_DAMAGE_LOG(Box::new(
            lyracore_protocol::messages::vanilla::SMSG_ENVIRONMENTAL_DAMAGE_LOG {
                guid: lyracore_protocol::messages::Guid::new(guid),
                damage_type: lyracore_protocol::messages::vanilla::EnvironmentalDamageType::Fall,
                damage,
                absorb: 0,
                resist: 0,
            },
        ));
        if let Err(error) = super::send(ctx, owner.connection_id, message, false) {
            spacetimedb::log::error!("native fall damage failed for {guid}: {error}");
        }
    }
}
