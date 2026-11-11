use lyracore_protocol::{codec, messages::vanilla::opcodes::ServerOpcodeMessage};
use spacetimedb::ReducerContext;

use super::Rollback;
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
    if let Some((opcode, body)) =
        codec::build_set_faction_standing_raw(row.reputation_index as u32, row.standing)
    {
        super::send_raw_to_character(ctx, row.character_guid, opcode, body);
    }
}

pub(crate) fn watched_faction(ctx: &ReducerContext, guid: u64, index: i32) {
    super::send_to_character(
        ctx,
        guid,
        ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(codec::build_watched_faction_values(
            guid, index,
        ))),
        super::EncodeFailure::Panic,
    );
}

pub(super) fn receive(
    ctx: &ReducerContext,
    guid: u64,
    opcode: u32,
    body: &[u8],
) -> Result<(), Rollback> {
    let result = match opcode {
        codec::CMSG_SET_FACTION_ATWAR_OPCODE => {
            if body.len() != 5 || body[4] > 1 {
                return Err("invalid faction at-war request".into());
            }
            let index =
                u32::from_le_bytes(body[..4].try_into().expect("checked faction index length"));
            super::actor(ctx, guid)
                .map_err(String::from)
                .and_then(|actor| {
                    crate::reputation::apply_set_faction_at_war(ctx, actor, index, body[4] != 0)
                })
        }
        codec::CMSG_SET_WATCHED_FACTION_OPCODE => {
            let index = i32::from_le_bytes(
                body.try_into()
                    .map_err(|_| "invalid watched faction request")?,
            );
            super::actor(ctx, guid)
                .map_err(String::from)
                .and_then(|_| crate::reputation::apply_set_watched_faction(ctx, guid, index))
        }
        _ => return Err("invalid reputation opcode".into()),
    };
    result.map_err(|reason| {
        Rollback::reply(
            ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(codec::build_gm_system_message(
                reason.clone(),
            ))),
            reason,
        )
    })
}
