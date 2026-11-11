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

use super::{
    directory::game_character_profile,
    game_native_player, game_native_world_connection,
    realm_party::{game_party_character, PartyCharacter},
    NativeWorldConnection, Rollback,
};
use crate::{
    chat::game_character_contact, game_area, game_world_entity, guild::game_guild, Character,
};

pub(super) fn claims(message: &ClientOpcodeMessage) -> bool {
    matches!(
        message,
        ClientOpcodeMessage::CMSG_WHO(_)
            | ClientOpcodeMessage::CMSG_FRIEND_LIST
            | ClientOpcodeMessage::CMSG_ADD_FRIEND(_)
            | ClientOpcodeMessage::CMSG_DEL_FRIEND(_)
            | ClientOpcodeMessage::CMSG_ADD_IGNORE(_)
            | ClientOpcodeMessage::CMSG_DEL_IGNORE(_)
    )
}

pub(super) fn receive(
    ctx: &ReducerContext,
    mut connection: NativeWorldConnection,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), Rollback> {
    let actor = super::available_entity(ctx, guid)?;
    let id = connection.connection_id;
    match message {
        ClientOpcodeMessage::CMSG_WHO(request) => {
            let now = crate::time::now(ctx).to_micros_since_unix_epoch();
            if now < connection.who_until_micros {
                return Ok(());
            }
            connection.who_until_micros = now + 1_000_000;
            ctx.db
                .game_native_world_connection()
                .connection_id()
                .update(connection);
            who(ctx, id, actor.race(), &request)?;
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

pub(super) fn name_query(
    ctx: &ReducerContext,
    guid: u64,
) -> Result<Option<lyracore_protocol::messages::vanilla::SMSG_NAME_QUERY_RESPONSE>, String> {
    let facts = crate::helpers::character_by_guid(ctx, guid)
        .map(|character| {
            (
                character.name,
                character.race,
                character.gender,
                character.class,
            )
        })
        .or_else(|| {
            crate::peer::realm_shard(ctx)?;
            let profile = ctx
                .db
                .game_character_profile()
                .guid()
                .find(guid)
                .filter(|profile| !profile.deleted)?;
            Some((profile.name, profile.race, profile.gender, profile.class))
        });
    facts
        .map(|(name, race, gender, class)| {
            codec::build_name_query_response(guid, name, race, gender, class)
                .map_err(|error| error.to_string())
        })
        .transpose()
}

fn online(ctx: &ReducerContext, character: &Character) -> Option<codec::FriendOnline> {
    let owner = super::world_session(ctx, character.guid)?;
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

fn remote_online(ctx: &ReducerContext, guid: u64) -> Option<codec::FriendOnline> {
    let character = ctx.db.game_party_character().guid().find(guid)?;
    if !character.online
        || ctx
            .db
            .game_character_profile()
            .guid()
            .find(guid)
            .is_some_and(|profile| profile.deleted)
    {
        return None;
    }
    character
        .stats
        .as_ref()
        .map(|stats| stats.friend_online(character.class, character.chat_tag))
}

fn target_race(ctx: &ReducerContext, guid: u64) -> Option<u8> {
    if crate::peer::realm_shard(ctx).is_some() {
        if let Some(profile) = ctx.db.game_character_profile().guid().find(guid) {
            return (!profile.deleted).then_some(profile.race);
        }
    }
    crate::helpers::character_by_guid(ctx, guid).map(|character| character.race)
}

fn target_online(ctx: &ReducerContext, guid: u64) -> Option<codec::FriendOnline> {
    crate::helpers::character_by_guid(ctx, guid)
        .and_then(|character| online(ctx, &character))
        .or_else(|| crate::peer::realm_shard(ctx).and_then(|_| remote_online(ctx, guid)))
}

fn named(ctx: &ReducerContext, name: &str) -> Option<(u64, u8)> {
    let local = crate::helpers::character_by_name(ctx, name)
        .map(|character| (character.guid, character.race));
    if crate::peer::realm_shard(ctx).is_none() {
        return local;
    }
    let mut matches = std::collections::BTreeMap::new();
    if let Some((guid, race)) = local {
        matches.insert(guid, race);
    }
    for profile in ctx
        .db
        .game_character_profile()
        .iter()
        .filter(|profile| !profile.deleted && profile.name.eq_ignore_ascii_case(name))
    {
        matches.insert(profile.guid, profile.race);
    }
    (matches.len() == 1)
        .then(|| matches.into_iter().next())
        .flatten()
}

fn lists(
    ctx: &ReducerContext,
    connection: ConnectionId,
    actor: &crate::WorldEntity,
) -> Result<(), Rollback> {
    prune_deleted(ctx, actor.guid);
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
        let online = target_race(ctx, contact.target_guid)
            .filter(|race| same_team(actor.race(), *race))
            .and_then(|_| target_online(ctx, contact.target_guid));
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
    Ok(super::send(
        ctx,
        connection,
        ServerOpcodeMessage::SMSG_IGNORE_LIST(Box::new(codec::build_ignore_list_response(
            &ignored,
        ))),
        false,
    )?)
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

fn refused(refusal: ContactRefusal, guid: u64, is_ignore: bool) -> Rollback {
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
    Rollback::replies(
        vec![lyracore_protocol::world::Packet { opcode, body }],
        refusal.as_tag(),
    )
}

fn add(
    ctx: &ReducerContext,
    connection: ConnectionId,
    actor: crate::WorldEntity,
    name: &str,
    is_ignore: bool,
) -> Result<(), Rollback> {
    let Some((target_guid, race)) = named(ctx, name) else {
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
    crate::taxi::reject_action_while_in_flight(ctx, actor.guid)?;
    let guid = actor.guid;
    crate::chat::add_contact(ctx, actor, target_guid, is_ignore, Some(race))
        .map_err(|refusal| refused(refusal, target_guid, is_ignore))?;
    if is_ignore {
        super::realm_party::presence(ctx, guid, true);
    }
    let online = (!is_ignore)
        .then(|| target_online(ctx, target_guid))
        .flatten();
    let status = if is_ignore {
        FriendResult::IgnoreAdded
    } else if online.is_some() {
        FriendResult::AddedOnline
    } else {
        FriendResult::AddedOffline
    };
    result(ctx, connection, status, target_guid, online);
    Ok(())
}

fn remove(
    ctx: &ReducerContext,
    connection: ConnectionId,
    actor: crate::WorldEntity,
    target: u64,
    is_ignore: bool,
) -> Result<(), Rollback> {
    crate::taxi::reject_action_while_in_flight(ctx, actor.guid)?;
    let guid = actor.guid;
    crate::chat::remove_contact(ctx, actor, target, is_ignore)
        .map_err(|refusal| refused(refusal, target, is_ignore))?;
    if is_ignore {
        super::realm_party::presence(ctx, guid, true);
    }
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

pub(super) fn enter(
    ctx: &ReducerContext,
    account_id: u64,
    guid: u64,
    entry: crate::character::WorldEntry,
) {
    prune_deleted(ctx, guid);
    super::directory::changed(ctx, account_id, guid, true);
    if entry == crate::character::WorldEntry::Login {
        presence(ctx, guid, true);
    }
}

pub(super) fn presence(ctx: &ReducerContext, guid: u64, present: bool) {
    if crate::peer::realm_shard(ctx).is_some() {
        return;
    }
    let Some(subject) = crate::helpers::character_by_guid(ctx, guid) else {
        return;
    };
    let fields = if present { online(ctx, &subject) } else { None };
    notify_presence(ctx, guid, subject.race, fields);
}

fn notify_presence(ctx: &ReducerContext, guid: u64, race: u8, fields: Option<codec::FriendOnline>) {
    for contact in ctx
        .db
        .game_character_contact()
        .by_target()
        .filter(guid)
        .filter(|contact| !contact.is_ignore && contact.owner_guid != guid)
    {
        let Some(owner) = super::world_session(ctx, contact.owner_guid) else {
            continue;
        };
        if crate::helpers::character_by_guid(ctx, owner.character_guid)
            .is_some_and(|owner| same_team(owner.race, race))
        {
            result(
                ctx,
                owner.connection_id,
                if fields.is_some() {
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

pub(super) fn projected_presence(
    ctx: &ReducerContext,
    previous: Option<&PartyCharacter>,
    character: &PartyCharacter,
) {
    if previous.is_some_and(|old| old.online == character.online)
        || previous.is_none() && !character.online
    {
        return;
    }
    if ctx
        .db
        .game_character_profile()
        .guid()
        .find(character.guid)
        .is_some_and(|profile| profile.deleted)
    {
        return;
    }
    let fields = character
        .online
        .then(|| {
            character
                .stats
                .as_ref()
                .map(|stats| stats.friend_online(character.class, character.chat_tag))
        })
        .flatten();
    notify_presence(ctx, character.guid, character.race, fields);
}

fn prune_deleted(ctx: &ReducerContext, guid: u64) {
    if crate::peer::realm_shard(ctx).is_none() {
        return;
    }
    let contacts = ctx.db.game_character_contact();
    for contact in contacts.by_owner().filter(guid).collect::<Vec<_>>() {
        if ctx
            .db
            .game_character_profile()
            .guid()
            .find(contact.target_guid)
            .is_some_and(|profile| profile.deleted)
        {
            contacts.id().delete(contact.id);
        }
    }
}

pub(super) fn deleted(ctx: &ReducerContext, guid: u64) {
    let contacts = ctx.db.game_character_contact();
    for contact in contacts.by_target().filter(guid).collect::<Vec<_>>() {
        if let Some(owner) = super::world_session(ctx, contact.owner_guid) {
            result(
                ctx,
                owner.connection_id,
                if contact.is_ignore {
                    FriendResult::IgnoreRemoved
                } else {
                    FriendResult::Removed
                },
                guid,
                None,
            );
        }
        contacts.id().delete(contact.id);
    }
}

fn who(
    ctx: &ReducerContext,
    connection: ConnectionId,
    race: u8,
    request: &CMSG_WHO,
) -> Result<(), Rollback> {
    let Some(filter) = WhoFilter::from_request(race, request) else {
        return Ok(());
    };
    if super::realm_communication::who(ctx, connection, &filter)? {
        return Ok(());
    }
    let matches = who_matches(ctx, &filter)
        .into_iter()
        .map(|(_, view)| view)
        .collect::<Vec<_>>();
    let (opcode, body) = codec::build_who_response_raw(&matches);
    super::send_raw(ctx, connection, opcode, body, false);
    Ok(())
}

pub(super) fn who_matches(
    ctx: &ReducerContext,
    filter: &WhoFilter<'_>,
) -> Vec<(u64, codec::WhoPlayerView)> {
    let reads_zones = filter.search_strings.iter().any(|s| !s.is_empty());
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
            filter,
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
            matches.push((
                character.guid,
                codec::WhoPlayerView {
                    name: character.name,
                    guild,
                    level: entity.level.min(u32::from(u8::MAX)) as u8,
                    class: entity.class(),
                    race: entity.race(),
                    zone_id: entity.zone_id,
                },
            ));
        }
    }
    matches
}
