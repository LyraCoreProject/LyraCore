use lyracore_protocol::{
    codec,
    messages::vanilla::{
        opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
        FriendResult, CMSG_WHO,
    },
    who::{WhoCandidate, WhoFilter},
    AwayStatus,
};
use lyracore_shared::{chat::chat_tag_for, faction::same_team, social::ContactRefusal};
use spacetimedb::{ConnectionId, ReducerContext, Table};

use super::{game_native_player, game_native_world_connection, NativeWorldConnection};
use crate::{
    chat::game_character_contact, game_area, game_world_entity, guild::game_guild, Character,
};

pub(super) fn receive(
    ctx: &ReducerContext,
    mut connection: NativeWorldConnection,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), String> {
    let actor = crate::helpers::acting_entity_by_guid(ctx, guid)
        .ok_or_else(|| super::rollback_silent("Character is not available".into()))?;
    let id = connection.connection_id;
    match message {
        ClientOpcodeMessage::CMSG_WHO(request) => {
            let now = ctx.timestamp.to_micros_since_unix_epoch();
            if now < connection.who_until_micros {
                return Ok(());
            }
            connection.who_until_micros = now + 1_000_000;
            ctx.db
                .game_native_world_connection()
                .connection_id()
                .update(connection);
            who(ctx, id, actor.race(), &request);
            Ok(())
        }
        ClientOpcodeMessage::CMSG_FRIEND_LIST => lists(ctx, id, &actor),
        ClientOpcodeMessage::CMSG_ADD_FRIEND(request) => add(ctx, id, actor, &request.name, false),
        ClientOpcodeMessage::CMSG_ADD_IGNORE(request) => add(ctx, id, actor, &request.name, true),
        ClientOpcodeMessage::CMSG_DEL_FRIEND(request) => {
            remove(ctx, id, actor, request.guid.guid(), false)
        }
        ClientOpcodeMessage::CMSG_DEL_IGNORE(request) => {
            remove(ctx, id, actor, request.guid.guid(), true)
        }
        _ => Err("opcode is not a contact or /who request".into()),
    }
}

fn online(ctx: &ReducerContext, character: &Character) -> Option<codec::FriendOnline> {
    let owner = ctx
        .db
        .game_native_player()
        .character_guid()
        .find(character.guid)?;
    let entity = ctx
        .db
        .game_world_entity()
        .guid()
        .find(character.guid)
        .filter(|_| owner.teleport != Some(super::teleport::NativeTeleport::WorldPort));
    Some(match entity {
        Some(entity) => codec::FriendOnline {
            away: match chat_tag_for(entity.player_flags) {
                1 => AwayStatus::Afk,
                2 => AwayStatus::Dnd,
                _ => AwayStatus::None,
            },
            zone_id: entity.zone_id,
            level: entity.level.min(u32::from(u8::MAX)) as u8,
            class: entity.class(),
        },
        None => codec::FriendOnline {
            away: AwayStatus::None,
            zone_id: character.zone_id,
            level: character.level,
            class: character.class,
        },
    })
}

fn lists(
    ctx: &ReducerContext,
    connection: ConnectionId,
    actor: &crate::WorldEntity,
) -> Result<(), String> {
    let mut friends = Vec::new();
    let mut ignored = Vec::new();
    for contact in ctx
        .db
        .game_character_contact()
        .by_owner()
        .filter(actor.guid)
    {
        if contact.is_ignore {
            ignored.push(contact.target_guid);
            continue;
        }
        let online = crate::helpers::character_by_guid(ctx, contact.target_guid)
            .filter(|target| same_team(actor.race(), target.race))
            .and_then(|target| online(ctx, &target));
        friends.push(codec::FriendView {
            guid: contact.target_guid,
            online: online.is_some(),
            away: online
                .as_ref()
                .map_or(AwayStatus::None, |online| online.away),
            zone_id: online.as_ref().map_or(0, |online| online.zone_id),
            level: online.as_ref().map_or(0, |online| online.level),
            class: online.as_ref().map_or(0, |online| online.class),
        });
    }
    super::send(
        ctx,
        connection,
        ServerOpcodeMessage::SMSG_FRIEND_LIST(Box::new(codec::build_friend_list_response(
            &friends,
        ))),
        false,
    )?;
    super::send(
        ctx,
        connection,
        ServerOpcodeMessage::SMSG_IGNORE_LIST(Box::new(codec::build_ignore_list_response(
            &ignored,
        ))),
        false,
    )
}

fn result(
    ctx: &ReducerContext,
    connection: ConnectionId,
    result: FriendResult,
    guid: u64,
    online: Option<codec::FriendOnline>,
) {
    let (opcode, body) = codec::build_friend_status_raw(result, guid, online);
    super::send_raw(ctx, connection, opcode, body, false);
}

fn refused(cause: String, guid: u64, is_ignore: bool) -> String {
    let Some(refusal) = ContactRefusal::parse_tag(&cause) else {
        return super::rollback_silent(cause);
    };
    let result = match (refusal, is_ignore) {
        (ContactRefusal::AddSelf, false) => FriendResult::SelfX,
        (ContactRefusal::AddSelf, true) => FriendResult::IgnoreSelf,
        (ContactRefusal::AlreadyOnList, false) => FriendResult::Already,
        (ContactRefusal::AlreadyOnList, true) => FriendResult::IgnoreAlready,
        (ContactRefusal::ListFull, false) => FriendResult::ListFull,
        (ContactRefusal::ListFull, true) => FriendResult::IgnoreFull,
        (ContactRefusal::NotOnList, true) => FriendResult::IgnoreNotFound,
        (ContactRefusal::NotOnList, false) | (ContactRefusal::ActorUnavailable, _) => {
            FriendResult::NotFound
        }
        (ContactRefusal::Enemy, _) => FriendResult::Enemy,
    };
    let (opcode, body) = codec::build_friend_status_raw(result, guid, None);
    super::rollback_packets(
        vec![lyracore_protocol::world::Packet { opcode, body }],
        cause,
    )
}

fn add(
    ctx: &ReducerContext,
    connection: ConnectionId,
    actor: crate::WorldEntity,
    name: &str,
    is_ignore: bool,
) -> Result<(), String> {
    let Some(target) = crate::helpers::character_by_name(ctx, name) else {
        result(
            ctx,
            connection,
            if is_ignore {
                FriendResult::IgnoreNotFound
            } else {
                FriendResult::NotFound
            },
            0,
            None,
        );
        return Ok(());
    };
    crate::taxi::reject_action_while_in_flight(ctx, actor.guid).map_err(super::rollback_silent)?;
    crate::chat::add_contact(ctx, actor, target.guid, is_ignore, Some(target.race))
        .map_err(|cause| refused(cause, target.guid, is_ignore))?;
    let online = (!is_ignore).then(|| online(ctx, &target)).flatten();
    let status = if is_ignore {
        FriendResult::IgnoreAdded
    } else if online.is_some() {
        FriendResult::AddedOnline
    } else {
        FriendResult::AddedOffline
    };
    result(ctx, connection, status, target.guid, online);
    Ok(())
}

fn remove(
    ctx: &ReducerContext,
    connection: ConnectionId,
    actor: crate::WorldEntity,
    target: u64,
    is_ignore: bool,
) -> Result<(), String> {
    crate::taxi::reject_action_while_in_flight(ctx, actor.guid).map_err(super::rollback_silent)?;
    crate::chat::remove_contact(ctx, actor, target, is_ignore)
        .map_err(|cause| refused(cause, target, is_ignore))?;
    result(
        ctx,
        connection,
        if is_ignore {
            FriendResult::IgnoreRemoved
        } else {
            FriendResult::Removed
        },
        target,
        None,
    );
    Ok(())
}

pub(super) fn presence(ctx: &ReducerContext, guid: u64, present: bool) {
    let Some(subject) = crate::helpers::character_by_guid(ctx, guid) else {
        return;
    };
    let fields = if present { online(ctx, &subject) } else { None };
    for contact in ctx
        .db
        .game_character_contact()
        .by_target()
        .filter(guid)
        .filter(|contact| !contact.is_ignore && contact.owner_guid != guid)
    {
        let Some(owner) = ctx
            .db
            .game_native_player()
            .character_guid()
            .find(contact.owner_guid)
        else {
            continue;
        };
        if crate::helpers::character_by_guid(ctx, owner.character_guid)
            .is_some_and(|owner| same_team(owner.race, subject.race))
        {
            result(
                ctx,
                owner.connection_id,
                if present {
                    FriendResult::Online
                } else {
                    FriendResult::Offline
                },
                guid,
                fields.clone(),
            );
        }
    }
}

fn who(ctx: &ReducerContext, connection: ConnectionId, race: u8, request: &CMSG_WHO) {
    let Some(filter) = WhoFilter::from_request(race, request) else {
        return;
    };
    let reads_zones = request.search_strings.iter().any(|s| !s.is_empty());
    let mut matches = Vec::new();
    for player in ctx.db.game_native_player().iter() {
        if player.teleport == Some(super::teleport::NativeTeleport::WorldPort) {
            continue;
        }
        let Some(entity) = ctx
            .db
            .game_world_entity()
            .guid()
            .find(player.character_guid)
        else {
            continue;
        };
        let Some(character) = crate::helpers::character_by_guid(ctx, player.character_guid) else {
            continue;
        };
        let guild = crate::guild::member(ctx, character.guid)
            .and_then(|member| ctx.db.game_guild().guild_id().find(member.guild_id))
            .map_or_else(String::new, |guild| guild.name);
        let zone_name = if reads_zones {
            ctx.db
                .game_area()
                .id()
                .find(entity.zone_id)
                .map_or_else(String::new, |area| area.name)
        } else {
            String::new()
        };
        if lyracore_protocol::who::matches(
            &filter,
            &WhoCandidate {
                name: &character.name,
                guild_name: &guild,
                race: entity.race(),
                class: entity.class(),
                level: entity.level.min(u32::from(u8::MAX)) as u8,
                zone_id: entity.zone_id,
                zone_name: &zone_name,
            },
        ) {
            matches.push(codec::WhoPlayerView {
                name: character.name,
                guild,
                level: entity.level.min(u32::from(u8::MAX)) as u8,
                class: entity.class(),
                race: entity.race(),
                zone_id: entity.zone_id,
            });
        }
    }
    let (opcode, body) = codec::build_who_response_raw(&matches);
    super::send_raw(ctx, connection, opcode, body, false);
}
