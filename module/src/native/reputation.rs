use lyracore_protocol::{codec, messages::vanilla::opcodes::ServerOpcodeMessage};
use spacetimedb::ReducerContext;

use super::game_native_player;
use crate::reputation::PlayerReputation;

pub(crate) fn changed(
    ctx: &ReducerContext,
    previous: Option<&PlayerReputation>,
    row: &PlayerReputation,
) {
    if !(0..64).contains(&row.reputation_index)
        || previous.is_some_and(|old| {
            (old.reputation_index, old.standing, old.at_war)
                == (row.reputation_index, row.standing, row.at_war)
        })
    {
        return;
    }
    if let Some(owner) = ctx
        .db
        .game_native_player()
        .character_guid()
        .find(row.character_guid)
    {
        if let Some((opcode, body)) =
            codec::build_set_faction_standing_raw(row.reputation_index as u32, row.standing)
        {
            super::send_raw(ctx, owner.connection_id, opcode, body, false);
        }
    }
}

pub(crate) fn watched(ctx: &ReducerContext, guid: u64, index: i32) {
    if let Some(owner) = ctx.db.game_native_player().character_guid().find(guid) {
        let packet = lyracore_protocol::world::encode(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
            Box::new(codec::build_watched_faction_values(guid, index)),
        ))
        .expect("fixed-size watched faction packet fits a world frame");
        super::send_raw(ctx, owner.connection_id, packet.opcode, packet.body, false);
    }
}

pub(super) fn receive(
    ctx: &ReducerContext,
    guid: u64,
    opcode: u32,
    body: &[u8],
) -> Result<(), String> {
    let result = match opcode {
        0x0125 => {
            if body.len() != 5 || body[4] > 1 {
                return Err("invalid faction at-war request".into());
            }
            let index =
                u32::from_le_bytes(body[..4].try_into().expect("checked faction index length"));
            super::actor(ctx, guid).and_then(|actor| {
                crate::reputation::apply_set_faction_at_war(ctx, actor, index, body[4] != 0)
            })
        }
        0x0318 => {
            let index = i32::from_le_bytes(
                body.try_into()
                    .map_err(|_| "invalid watched faction request")?,
            );
            super::actor(ctx, guid)
                .and_then(|_| crate::reputation::apply_set_watched_faction(ctx, guid, index))
        }
        _ => return Err("invalid reputation opcode".into()),
    };
    result.map_err(|reason| {
        super::rollback_reply(
            ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(codec::build_gm_system_message(
                reason.clone(),
            ))),
            reason,
        )
    })
}
