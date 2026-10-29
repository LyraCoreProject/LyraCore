use lyracore_protocol::{
    codec,
    messages::vanilla::{
        opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
        DismountResult, MountResult, Vector3d, SMSG_DISMOUNTRESULT, SMSG_MOUNTRESULT,
    },
    world::Packet,
};
use lyracore_shared::constants::taxi_protocol;
use spacetimedb::{ConnectionId, ReducerContext};

use super::{game_native_player, Rollback};
use crate::taxi::TaxiPassengerSpline;
use crate::Refusal;

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

fn dismount_result(
    ctx: &ReducerContext,
    character_guid: u64,
    result: DismountResult,
) -> Option<ConnectionId> {
    let owner = ctx
        .db
        .game_native_player()
        .character_guid()
        .find(character_guid)?;
    if let Err(error) = super::send(
        ctx,
        owner.connection_id,
        ServerOpcodeMessage::SMSG_DISMOUNTRESULT(SMSG_DISMOUNTRESULT { result }),
        false,
    ) {
        spacetimedb::log::error!("taxi dismount delivery failed for {character_guid}: {error}");
        super::disconnect(ctx, owner.connection_id);
        return None;
    }
    Some(owner.connection_id)
}

pub(crate) fn aborted(ctx: &ReducerContext, character_guid: u64, mounted: bool) {
    let result = if mounted {
        DismountResult::Ok
    } else {
        DismountResult::NotMounted
    };
    dismount_result(ctx, character_guid, result);
}

pub(crate) fn landed(ctx: &ReducerContext, character_guid: u64) {
    let Some(connection) = dismount_result(ctx, character_guid, DismountResult::Ok) else {
        return;
    };
    if let Err(error) = super::send(
        ctx,
        connection,
        ServerOpcodeMessage::SMSG_CANCEL_COMBAT,
        false,
    ) {
        spacetimedb::log::error!("taxi landing delivery failed for {character_guid}: {error}");
        super::disconnect(ctx, connection);
    }
}

pub(super) fn create_spline(ctx: &ReducerContext, guid: u64, view: &mut codec::EntityView) {
    if let Some((spline, pose)) = crate::taxi::current_create_spline(ctx, guid) {
        [view.x, view.y, view.z, view.orientation] = pose;
        view.spline = Some(spline);
    }
}

pub(super) fn open(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    npc: u64,
) -> Result<(), Refusal> {
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
) -> Result<(), Rollback> {
    match message {
        ClientOpcodeMessage::CMSG_TAXINODE_STATUS_QUERY(c) => {
            super::actor(ctx, guid)?;
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
            Ok(open(ctx, connection, guid, c.guid.guid())?)
        }
        ClientOpcodeMessage::CMSG_ACTIVATETAXI(c) => {
            super::available_entity(ctx, guid)?;
            let reply = crate::taxi::activate_taxi(
                ctx,
                guid,
                c.guid.guid(),
                c.source_node,
                c.destination_node,
                0,
            );
            let accepted = reply.accepted && reply.result_code == taxi_protocol::ACTIVATE_OK;
            if accepted {
                super::send(
                    ctx,
                    connection,
                    ServerOpcodeMessage::SMSG_CANCEL_COMBAT,
                    false,
                )?;
            }
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
            if accepted {
                super::send(
                    ctx,
                    connection,
                    ServerOpcodeMessage::SMSG_MOUNTRESULT(SMSG_MOUNTRESULT {
                        result: MountResult::Ok,
                    }),
                    false,
                )?;
                crate::taxi::arm_taxi_flight(ctx, guid);
            }
            Ok(())
        }
        _ => Err("invalid taxi opcode".into()),
    }
}
