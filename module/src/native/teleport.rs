use lyracore_protocol::{
    codec,
    messages::vanilla::opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
};
use spacetimedb::{ReducerContext, SpacetimeType, Table};

use super::{game_native_player, NativePlayer, NativeWorldConnection};
use crate::{game_world_entity, world::TeleportEvent};

const NEAR_ACK_TIMEOUT_MICROS: i64 = 20_000_000;

#[derive(SpacetimeType, Clone, Copy, PartialEq, Eq)]
pub enum NativeTeleport {
    Near { counter: u32 },
    WorldPort,
}

pub(crate) fn begin(ctx: &ReducerContext, guid: u64, cross_map: bool) -> Option<u32> {
    let mut player = ctx.db.game_native_player().character_guid().find(guid)?;
    player.teleport_counter = player.teleport_counter.wrapping_add(1);
    player.teleport_started_micros = ctx.timestamp.to_micros_since_unix_epoch();
    player.teleport = Some(if cross_map {
        super::clear_audience(ctx, player.connection_id);
        NativeTeleport::WorldPort
    } else {
        NativeTeleport::Near {
            counter: player.teleport_counter,
        }
    });
    player.open_loot = 0;
    player.gossip_menu = None;
    let counter = player.teleport_counter;
    ctx.db.game_native_player().connection_id().update(player);
    Some(counter)
}

pub(crate) fn changed(
    ctx: &ReducerContext,
    event: &TeleportEvent,
    counter: Option<u32>,
    aboard: Option<&crate::transport::Aboard>,
) {
    let Some(player) = ctx
        .db
        .game_native_player()
        .character_guid()
        .find(event.mover_guid)
    else {
        return;
    };
    if counter != Some(player.teleport_counter) {
        return;
    }
    let messages = if let Some(aboard) = aboard {
        let passenger = &aboard.passenger;
        codec::build_cross_map_teleport(
            event.map_id,
            passenger.x,
            passenger.y,
            passenger.z,
            passenger.orientation,
            Some(codec::TransportCrossing {
                entry: passenger.transport_entry,
                from_map_id: aboard.from_map_id,
            }),
        )
        .map(Vec::from)
    } else if event.cross_map {
        codec::build_cross_map_teleport(
            event.map_id,
            event.x,
            event.y,
            event.z,
            event.orientation,
            None,
        )
        .map(Vec::from)
    } else {
        Ok(vec![ServerOpcodeMessage::MSG_MOVE_TELEPORT_ACK(Box::new(
            codec::build_teleport_ack(
                event.mover_guid,
                player.teleport_counter,
                event.x,
                event.y,
                event.z,
                event.orientation,
            ),
        ))])
    };
    let result = messages
        .map_err(|error| error.to_string())
        .and_then(|messages| {
            for message in messages {
                super::send(ctx, player.connection_id, message, false)?;
            }
            Ok(())
        });
    if let Err(error) = result {
        spacetimedb::log::error!("native teleport failed for {}: {error}", event.mover_guid);
        super::disconnect(ctx, player.connection_id);
    }
}

pub(super) fn acknowledge(
    ctx: &ReducerContext,
    connection: NativeWorldConnection,
    player: NativePlayer,
    opcode: u32,
    body: &[u8],
) -> Result<(), String> {
    let message =
        lyracore_protocol::world::decode(opcode, body).map_err(|error| error.to_string())?;
    match (message, player.teleport) {
        (
            ClientOpcodeMessage::MSG_MOVE_TELEPORT_ACK(ack),
            Some(NativeTeleport::Near { counter }),
        ) if ack.guid.guid() == player.character_guid && ack.movement_counter == counter => {
            finish_near(ctx, player, counter, false).map(|_| ())
        }
        (ClientOpcodeMessage::MSG_MOVE_WORLDPORT_ACK, Some(NativeTeleport::WorldPort)) => {
            // Rebuild without a recipient; a Passenger's Transport, then self CREATE, precede AOI.
            ctx.db
                .game_native_player()
                .connection_id()
                .delete(player.connection_id);
            super::world::enter_character(
                ctx,
                connection,
                player.account_id,
                player.character_guid,
                crate::world::WorldEntry::WorldPort,
                player.teleport_counter,
            )
        }
        _ => Ok(()),
    }
}

fn finish_near(
    ctx: &ReducerContext,
    mut player: NativePlayer,
    counter: u32,
    send_heartbeat: bool,
) -> Result<NativePlayer, String> {
    let entity = ctx
        .db
        .game_world_entity()
        .guid()
        .find(player.character_guid)
        .ok_or("teleport mover is missing")?;
    let destination = codec::build_teleport_ack(
        entity.guid,
        counter,
        entity.x,
        entity.y,
        entity.z,
        entity.orientation,
    );
    let mut packet = lyracore_protocol::world::encode(ServerOpcodeMessage::MSG_MOVE_TELEPORT_ACK(
        Box::new(destination),
    ))
    .map_err(|error| error.to_string())?;
    let counter_offset = 1 + packet.body[0].count_ones() as usize;
    packet.body.drain(counter_offset..counter_offset + 4);
    player.teleport = None;
    player.teleport_started_micros = 0;
    let connection_id = player.connection_id;
    ctx.db.game_native_player().connection_id().update(player);
    if send_heartbeat {
        super::send_raw(
            ctx,
            connection_id,
            lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT as u16,
            packet.body.clone(),
            false,
        );
    }
    // MSG_MOVE_TELEPORT has packed GUID and MovementInfo without an acknowledgement counter.
    super::visibility::broadcast(ctx, entity.guid, 0x00c5, packet.body, true);
    ctx.db
        .game_native_player()
        .connection_id()
        .find(connection_id)
        .ok_or_else(|| "teleport World Session is missing".to_string())
}

pub(super) fn expire(ctx: &ReducerContext, player: NativePlayer) -> Result<NativePlayer, String> {
    let Some(NativeTeleport::Near { counter }) = player.teleport else {
        return Ok(player);
    };
    if ctx
        .timestamp
        .to_micros_since_unix_epoch()
        .saturating_sub(player.teleport_started_micros)
        < NEAR_ACK_TIMEOUT_MICROS
    {
        return Ok(player);
    }
    spacetimedb::log::warn!(
        "near teleport acknowledgement expired for {} at counter {counter}",
        player.character_guid
    );
    finish_near(ctx, player, counter, true)
}

crate::game_tick_pass!(fn expire_near_teleports_pass(ctx) {
    for player in ctx.db.game_native_player().iter() {
        let connection_id = player.connection_id;
        if let Err(error) = expire(ctx, player) {
            spacetimedb::log::error!("near teleport expiry failed for {connection_id:?}: {error}");
            super::disconnect(ctx, connection_id);
        }
    }
});
