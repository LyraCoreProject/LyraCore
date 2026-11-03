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
            if super::realm_party::submit_admitted(
                ctx,
                connection,
                guid,
                0x292,
                area.to_le_bytes().to_vec(),
            )? {
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
            meeting_stone::join(ctx, guid, area, &facts).map_err(join_error)?;
            return Ok(());
        }
        ClientOpcodeMessage::CMSG_MEETINGSTONE_LEAVE => {
            if super::realm_party::submit_admitted(ctx, connection, guid, 0x293, Vec::new())? {
                return Ok(());
            }
            meeting_stone::leave(ctx, guid).map_err(|error| match error {
                StoneOpError::Refused(refusal) => Rollback::silent(refusal.as_tag()),
                StoneOpError::Invalid(cause) => Rollback::from(cause),
            })?;
            return Ok(());
        }
        ClientOpcodeMessage::CMSG_MEETINGSTONE_INFO => {
            if super::realm_party::submit_admitted(ctx, connection, guid, 0x296, Vec::new())? {
                return Ok(());
            }
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

pub(super) fn is_realm_request(opcode: u32) -> bool {
    matches!(opcode, 0x292 | 0x293 | 0x296)
}

// JOIN carries the World-admitted area as u32, not the client's GameObject guid.
pub(super) fn apply_realm(
    ctx: &ReducerContext,
    guid: u64,
    opcode: u32,
    body: &[u8],
) -> Result<(), Rollback> {
    use super::directory::game_character_profile;
    use super::realm_party::game_party_character;
    match opcode {
        0x292 => {
            let area = u32::from_le_bytes(
                body.try_into()
                    .map_err(|_| "invalid admitted Meeting Stone area")?,
            );
            if Area::try_from(area).is_err() {
                return Err("unknown Meeting Stone area".into());
            }
            let mut guids = vec![guid];
            if let Some(member) = crate::group::group_of(ctx, guid) {
                guids.extend(
                    crate::group::members_of(ctx, member.group_id)
                        .iter()
                        .map(|member| member.character_guid),
                );
            }
            guids.sort_unstable();
            guids.dedup();
            let facts: Vec<_> = guids
                .into_iter()
                .filter_map(|guid| {
                    ctx.db
                        .game_party_character()
                        .guid()
                        .find(guid)
                        .map(|character| (character.race, character.class))
                        .or_else(|| {
                            ctx.db
                                .game_character_profile()
                                .guid()
                                .find(guid)
                                .filter(|character| !character.deleted)
                                .map(|character| (character.race, character.class))
                        })
                        .map(|(race, class)| SeekerFacts {
                            character_guid: guid,
                            race,
                            class,
                        })
                })
                .collect();
            meeting_stone::join(ctx, guid, area, &facts).map_err(join_error)
        }
        0x293 if body.is_empty() => meeting_stone::leave(ctx, guid).map_err(|error| match error {
            StoneOpError::Refused(refusal) => Rollback::silent(refusal.as_tag()),
            StoneOpError::Invalid(cause) => Rollback::from(cause),
        }),
        0x296 if body.is_empty() => {
            let (area, status) = ctx
                .db
                .game_meeting_stone_seeker()
                .character_guid()
                .find(guid)
                .map_or((0, queue_status::NONE), |seeker| {
                    (seeker.area_id, queue_status::JOINED_QUEUE)
                });
            crate::group::push_event(
                ctx,
                guid,
                lyracore_shared::meeting_stone::event_kind::QUEUE,
                0,
                lyracore_shared::meeting_stone::encode_queue(area, status),
            );
            Ok(())
        }
        _ => Err("invalid Meeting Stone request".into()),
    }
}

fn join_error(error: StoneOpError) -> Rollback {
    match error {
        StoneOpError::Refused(refusal) => {
            match join_failure_for(refusal).and_then(codec::build_meetingstone_joinfailed) {
                Some(packet) => Rollback::reply(
                    ServerOpcodeMessage::SMSG_MEETINGSTONE_JOINFAILED(packet),
                    refusal.as_tag(),
                ),
                None => Rollback::silent(refusal.as_tag()),
            }
        }
        StoneOpError::Invalid(cause) => Rollback::from(cause),
    }
}
