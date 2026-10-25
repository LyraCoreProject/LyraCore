use lyracore_protocol::{
    codec,
    messages::vanilla::{
        opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
        Area,
    },
};
use lyracore_shared::{
    group::GROUP_MAX_MEMBERS,
    meeting_stone::{join_failure_for, queue_status},
};
use spacetimedb::{ConnectionId, ReducerContext};

use super::Rollback;
use crate::meeting_stone::{self, game_meeting_stone_seeker, SeekerFacts, StoneOpError};

pub(super) fn receive(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), Rollback> {
    super::actor(ctx, guid)?;
    let response = match message {
        ClientOpcodeMessage::CMSG_MEETINGSTONE_JOIN(request) => {
            let area = meeting_stone::admit(ctx, guid, request.guid.guid())
                .map_err(|refusal| Rollback::silent(refusal.as_tag()))?;
            if Area::try_from(area).is_err() {
                return Ok(());
            }
            let party = crate::group::group_with_members(ctx, guid)
                .map_err(|cause| format!("meeting stone group: {cause:?}"))?;
            let members = match party {
                Some((_, members)) if members.len() <= GROUP_MAX_MEMBERS => members
                    .into_iter()
                    .map(|member| member.character_guid)
                    .collect(),
                _ => vec![guid],
            };
            let facts = members
                .into_iter()
                .filter_map(|guid| {
                    crate::helpers::character_by_guid(ctx, guid).map(|character| SeekerFacts {
                        character_guid: guid,
                        race: character.race,
                        class: character.class,
                    })
                })
                .collect::<Vec<_>>();
            meeting_stone::join(ctx, guid, area, &facts).map_err(|error| match error {
                StoneOpError::Refused(refusal) => {
                    let cause = refusal.as_tag();
                    match join_failure_for(refusal).and_then(codec::build_meetingstone_joinfailed) {
                        Some(packet) => Rollback::reply(
                            ServerOpcodeMessage::SMSG_MEETINGSTONE_JOINFAILED(packet),
                            cause,
                        ),
                        None => Rollback::silent(cause),
                    }
                }
                StoneOpError::Invalid(cause) => Rollback::from(cause),
            })?;
            return Ok(());
        }
        ClientOpcodeMessage::CMSG_MEETINGSTONE_LEAVE => {
            meeting_stone::leave(ctx, guid).map_err(|error| match error {
                StoneOpError::Refused(refusal) => Rollback::silent(refusal.as_tag()),
                StoneOpError::Invalid(cause) => Rollback::from(cause),
            })?;
            return Ok(());
        }
        ClientOpcodeMessage::CMSG_MEETINGSTONE_INFO => {
            let (area, status) = ctx
                .db
                .game_meeting_stone_seeker()
                .character_guid()
                .find(guid)
                .map_or((0, queue_status::NONE), |seeker| {
                    (seeker.area_id, queue_status::JOINED_QUEUE)
                });
            let Some(packet) = codec::build_meetingstone_setqueue(area, status) else {
                spacetimedb::log::warn!("meeting stone seeker {guid} has an unknown area {area}");
                return Ok(());
            };
            ServerOpcodeMessage::SMSG_MEETINGSTONE_SETQUEUE(packet)
        }
        ClientOpcodeMessage::MSG_LOOKING_FOR_GROUP => {
            ServerOpcodeMessage::MSG_LOOKING_FOR_GROUP(codec::build_looking_for_group())
        }
        _ => return Err("opcode is not a meeting stone request".into()),
    };
    Ok(super::send(ctx, connection, response, false)?)
}
