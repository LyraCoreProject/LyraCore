use lyracore_protocol::{
    codec,
    messages::{
        vanilla::{
            opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
            DuelWinnerReason, SMSG_DUEL_COMPLETE, SMSG_DUEL_COUNTDOWN, SMSG_DUEL_REQUESTED,
            SMSG_DUEL_WINNER,
        },
        Guid,
    },
};
use lyracore_shared::duel::{completion_kind, event_kind};
use spacetimedb::{ConnectionId, ReducerContext};

use super::{game_native_player, Rollback};
use crate::{duel::DuelEvent, gameobject::game_gameobject_template};

pub(super) fn receive(
    ctx: &ReducerContext,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), Rollback> {
    super::actor(ctx, guid)?;
    match message {
        ClientOpcodeMessage::CMSG_DUEL_ACCEPTED(request) => {
            crate::duel::accept_duel(ctx, guid, request.guid.guid());
        }
        ClientOpcodeMessage::CMSG_DUEL_CANCELLED(request) => {
            crate::duel::cancel_duel(ctx, guid, request.guid.guid());
        }
        _ => return Err("opcode is not a duel request".into()),
    }
    Ok(())
}

pub(crate) fn event(ctx: &ReducerContext, row: &DuelEvent) {
    if let Some(player) = ctx
        .db
        .game_native_player()
        .character_guid()
        .find(row.recipient_guid)
    {
        participant(ctx, player.connection_id, row);
    }
    if row.kind != event_kind::COMPLETE || row.recipient_guid != row.initiator_guid {
        return;
    }
    let Some(winner) = winner(row) else {
        return;
    };
    let packet =
        lyracore_protocol::world::encode(ServerOpcodeMessage::SMSG_DUEL_WINNER(Box::new(winner)))
            .expect("duel winner names fit a world frame");
    let cell = lyracore_shared::spatial::cell_id_at(row.flag_x, row.flag_y);
    for viewer in crate::aoi::nearby_viewers(ctx, row.map_id, row.instance_id, cell) {
        if viewer.guid == row.initiator_guid || viewer.guid == row.challenged_guid {
            continue;
        }
        if let Some(player) = ctx
            .db
            .game_native_player()
            .character_guid()
            .find(viewer.guid)
        {
            super::send_raw(
                ctx,
                player.connection_id,
                packet.opcode,
                packet.body.clone(),
                false,
            );
        }
    }
}

fn participant(ctx: &ReducerContext, connection: ConnectionId, row: &DuelEvent) {
    let send = |message| {
        super::send(ctx, connection, message, false)
            .expect("bounded duel message fits a world frame");
    };
    let values = |guid, arbiter, team| {
        let (opcode, body) = codec::build_duel_player_values(guid, arbiter, team);
        super::send_raw(ctx, connection, opcode, body, false);
    };
    match row.kind {
        event_kind::REQUESTED => {
            let Some(template) = ctx
                .db
                .game_gameobject_template()
                .entry()
                .find(row.flag_entry)
            else {
                spacetimedb::log::warn!(
                    "duel {} has no flag template {}",
                    row.duel_id,
                    row.flag_entry
                );
                return;
            };
            let flag = codec::GameObjectView {
                guid: row.flag_guid,
                template_entry: row.flag_entry,
                x: row.flag_x,
                y: row.flag_y,
                z: row.flag_z,
                orientation: row.flag_orientation,
                state: 1,
                type_id: template.type_id,
                display_id: template.display_id,
                rotation_0: 0.0,
                rotation_1: 0.0,
                rotation_2: 0.0,
                rotation_3: 0.0,
                size: 1.0,
            };
            send(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                codec::build_gameobject_create_object(&flag),
            )));
            let (opcode, body) = codec::build_gameobject_rotation_values(&flag);
            super::send_raw(ctx, connection, opcode, body, false);
            values(row.initiator_guid, Some(row.flag_guid), None);
            values(row.challenged_guid, Some(row.flag_guid), None);
            send(ServerOpcodeMessage::SMSG_DUEL_REQUESTED(Box::new(
                SMSG_DUEL_REQUESTED {
                    // Vanilla uses the field named initiator for the arbiter GameObject.
                    initiator: Guid::new(row.flag_guid),
                    target: Guid::new(row.initiator_guid),
                },
            )));
        }
        event_kind::COUNTDOWN => {
            send(ServerOpcodeMessage::SMSG_DUEL_COUNTDOWN(
                SMSG_DUEL_COUNTDOWN {
                    // The binding writes seconds; the client reads this field as milliseconds.
                    time: std::time::Duration::from_secs(3_000),
                },
            ));
        }
        event_kind::ACTIVE => {
            values(row.initiator_guid, None, Some(1));
            values(row.challenged_guid, None, Some(2));
        }
        event_kind::OUT_OF_BOUNDS => send(ServerOpcodeMessage::SMSG_DUEL_OUTOFBOUNDS),
        event_kind::IN_BOUNDS => send(ServerOpcodeMessage::SMSG_DUEL_INBOUNDS),
        event_kind::COMPLETE => {
            send(ServerOpcodeMessage::SMSG_DUEL_COMPLETE(
                SMSG_DUEL_COMPLETE {
                    ended_without_interruption: row.completion_kind != completion_kind::INTERRUPTED,
                },
            ));
            if let Some(winner) = winner(row) {
                send(ServerOpcodeMessage::SMSG_DUEL_WINNER(Box::new(winner)));
            }
            values(row.initiator_guid, Some(0), Some(0));
            values(row.challenged_guid, Some(0), Some(0));
            send(ServerOpcodeMessage::SMSG_DESTROY_OBJECT(
                codec::build_destroy_object(row.flag_guid),
            ));
        }
        _ => spacetimedb::log::warn!("unknown duel {} event kind {}", row.duel_id, row.kind),
    }
}

fn winner(row: &DuelEvent) -> Option<SMSG_DUEL_WINNER> {
    if row.winner_guid == 0
        || row.loser_guid == 0
        || row.winner_name.is_empty()
        || row.loser_name.is_empty()
    {
        return None;
    }
    let reason = match row.completion_kind {
        completion_kind::WON => DuelWinnerReason::Won,
        completion_kind::FLED => DuelWinnerReason::Fled,
        _ => return None,
    };
    Some(SMSG_DUEL_WINNER {
        reason,
        // The first CString carries the winner despite its opponent_name field name.
        opponent_name: row.winner_name.clone(),
        initiator_name: row.loser_name.clone(),
    })
}
