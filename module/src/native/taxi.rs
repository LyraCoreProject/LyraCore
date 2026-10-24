use lyracore_protocol::{
    codec,
    messages::vanilla::{
        opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
        Vector3d,
    },
    world::Packet,
};
use lyracore_shared::constants::taxi_protocol;
use spacetimedb::{ConnectionId, ReducerContext};

use crate::taxi::{game_taxi_passenger_spline, TaxiPassengerSpline};

fn spline_packet(row: &TaxiPassengerSpline) -> Option<Packet> {
    if row.points.is_empty() || !row.points.len().is_multiple_of(3) {
        return None;
    }
    let points = row
        .points
        .chunks_exact(3)
        .map(|point| Vector3d {
            x: point[0],
            y: point[1],
            z: point[2],
        })
        .collect();
    codec::build_taxi_move_raw(
        row.character_guid,
        Vector3d {
            x: row.start_x,
            y: row.start_y,
            z: row.start_z,
        },
        points,
        row.duration_ms,
        row.spline_id,
    )
    .map(|(opcode, body)| Packet { opcode, body })
}

pub(crate) fn spline_changed(ctx: &ReducerContext, row: &TaxiPassengerSpline) {
    if let Some(packet) = spline_packet(row) {
        super::visibility::broadcast(ctx, row.character_guid, packet.opcode, packet.body, false);
    }
}

pub(super) fn replay(ctx: &ReducerContext, connection: ConnectionId, guid: u64) {
    if let Some(packet) = ctx
        .db
        .game_taxi_passenger_spline()
        .character_guid()
        .find(guid)
        .and_then(|row| spline_packet(&row))
    {
        super::send_raw(ctx, connection, packet.opcode, packet.body, false);
    }
}

pub(super) fn open(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    npc: u64,
) -> Result<(), String> {
    super::actor(ctx, guid)?;
    let reply = crate::taxi::open_taxi(ctx, guid, npc, 0);
    if reply.accepted {
        super::send(
            ctx,
            connection,
            ServerOpcodeMessage::SMSG_SHOWTAXINODES(Box::new(codec::build_show_taxi_nodes(
                &codec::TaxiMapView {
                    npc_guid: npc,
                    source_client_node_id: reply.source_client_node_id,
                    available_client_node_ids: reply.available_client_node_ids,
                },
            ))),
            false,
        )?;
    }
    Ok(())
}

pub(super) fn receive(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), String> {
    match message {
        ClientOpcodeMessage::CMSG_TAXINODE_STATUS_QUERY(c) => {
            super::actor(ctx, guid).map_err(super::rollback_silent)?;
            let reply = crate::taxi::taxi_node_status(ctx, guid, c.guid.guid(), 0);
            if reply.accepted {
                super::send(
                    ctx,
                    connection,
                    ServerOpcodeMessage::SMSG_TAXINODE_STATUS(Box::new(
                        codec::build_taxi_node_status(codec::TaxiNodeStatusView {
                            npc_guid: reply.npc_guid,
                            known: reply.known,
                        }),
                    )),
                    false,
                )?;
            }
            Ok(())
        }
        ClientOpcodeMessage::CMSG_TAXIQUERYAVAILABLENODES(c) => {
            open(ctx, connection, guid, c.guid.guid()).map_err(super::rollback_silent)
        }
        ClientOpcodeMessage::CMSG_ACTIVATETAXI(c) => {
            if crate::helpers::acting_entity_by_guid(ctx, guid).is_none() {
                return Err(super::rollback_silent("Character is not available".into()));
            }
            let reply = crate::taxi::activate_taxi(
                ctx,
                guid,
                c.guid.guid(),
                c.source_node,
                c.destination_node,
                0,
            );
            super::send(
                ctx,
                connection,
                ServerOpcodeMessage::SMSG_ACTIVATETAXIREPLY(codec::build_activate_taxi_reply(
                    codec::TaxiActivationResult {
                        result_code: reply.result_code,
                    },
                )),
                false,
            )?;
            // The ordered feed must deliver the paid reply before the first flight spline.
            if reply.accepted && reply.result_code == taxi_protocol::ACTIVATE_OK {
                crate::taxi::arm_taxi_flight(ctx, guid);
            }
            Ok(())
        }
        _ => Err("invalid taxi opcode".into()),
    }
}
