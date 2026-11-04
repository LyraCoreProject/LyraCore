pub(crate) mod package;

use std::collections::BTreeSet;

use lyracore_protocol::{
    codec,
    messages::vanilla::{
        opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
        PartyResult, SMSG_NOTIFICATION,
    },
};
use lyracore_shared::chat::chat_kind;
use lyracore_shared::group::{event_kind, GroupRefusal, TargetIcon};
use spacetimedb::{
    reducer, sats::bsatn, table, ConnectionId, Identity, ReducerContext, ScheduleAt, SpacetimeType,
    Table, TimeDuration,
};

use super::directory::game_character_directory;
use super::{directory, game_native_player, realm_shard::game_native_away, Rollback};
use crate::{
    group::{
        self, game_group, game_group_event, game_group_invite, game_group_member,
        game_group_target_icon, Group, GroupAction, GroupMember, Plane,
    },
    peer::{realm_shard, require_sender, world_shard},
    remote_bindings::peer as remote,
};

// Opcode zero carries a Package Group command instead of client packet bytes.
const PACKAGE_COMMAND: u32 = 0;

#[table(accessor = game_party_character)]
#[derive(Clone)]
pub struct PartyCharacter {
    #[primary_key]
    pub guid: u64,
    pub account_id: u64,
    pub connection: u128,
    pub world: Identity,
    pub name: String,
    pub race: u8,
    pub class: u8,
    pub chat_tag: u8,
    pub identity: Identity,
    pub online: bool,
    pub stats: Option<super::member_stats::MemberEntity>,
    pub revision: u64,
}

#[table(accessor = game_party_membership, index(accessor = by_group, btree(columns = [group_id])))]
pub struct PartyMembership {
    #[primary_key]
    pub guid: u64,
    pub group_id: u64,
    pub raid_slot: u8,
    pub revision: u64,
}

#[table(accessor = game_party_projection)]
pub struct PartyProjection {
    #[primary_key]
    pub group_id: u64,
    pub revision: u64,
}

#[derive(SpacetimeType)]
pub struct Member {
    guid: u64,
    raid_slot: u8,
}

#[derive(SpacetimeType)]
pub struct Roster {
    group_id: u64,
    leader: u64,
    loot_method: u8,
    loot_threshold: u8,
    master: u64,
    kind: u8,
    members: Vec<Member>,
    icons: Vec<Icon>,
    disband: Option<PartyDisband>,
}

#[derive(SpacetimeType)]
pub struct Icon {
    icon: u8,
    target: u64,
}

#[derive(SpacetimeType)]
pub struct Event {
    recipient: u64,
    connection: u128,
    kind: u8,
    other: u64,
    payload: String,
}

#[derive(SpacetimeType)]
pub struct Update {
    revision: u64,
    characters: Vec<PartyCharacter>,
    rosters: Vec<Roster>,
    events: Vec<Event>,
    absent: Vec<u64>,
    events_expire_micros: i64,
}

fn encode<T: spacetimedb::Serialize>(value: &T) -> Vec<u8> {
    bsatn::to_vec(value).expect("party state encodes")
}

pub(crate) fn name(ctx: &ReducerContext, guid: u64) -> Option<String> {
    crate::helpers::character_by_guid(ctx, guid)
        .map(|character| character.name)
        .or_else(|| {
            ctx.db
                .game_party_character()
                .guid()
                .find(guid)
                .map(|character| character.name)
        })
}

pub(super) fn current(ctx: &ReducerContext, character: &PartyCharacter) -> Option<Identity> {
    let entry = ctx
        .db
        .game_character_directory()
        .guid()
        .find(character.guid)?;
    if character.connection == 0 {
        return (entry.account_id == character.account_id
            && entry.world == character.world
            && package::current(ctx, character))
        .then_some(entry.world);
    }
    let away = ctx
        .db
        .game_native_away()
        .account_id()
        .find(character.account_id)?;
    (entry.account_id == character.account_id
        && away.character_guid == character.guid
        && away.connection_id.to_u128() == character.connection
        && away.world == Some(entry.world))
    .then_some(entry.world)
}

pub(crate) fn online(ctx: &ReducerContext, guid: u64) -> bool {
    ctx.db
        .game_party_character()
        .guid()
        .find(guid)
        .is_some_and(|character| {
            character.online && (world_shard(ctx).is_none() || current(ctx, &character).is_some())
        })
}

pub(super) fn named(ctx: &ReducerContext, name: &str) -> Option<u64> {
    let characters = ctx.db.game_party_character();
    let mut matches = characters.iter().filter(|character| {
        character.name.eq_ignore_ascii_case(name) && online(ctx, character.guid)
    });
    let first = matches.next()?;
    matches.next().is_none().then_some(first.guid)
}

pub(super) fn facts(ctx: &ReducerContext, guid: u64, online: bool) -> Option<PartyCharacter> {
    let player = ctx.db.game_native_player().character_guid().find(guid)?;
    let character = crate::helpers::character_by_guid(ctx, guid)?;
    Some(PartyCharacter {
        guid,
        account_id: player.account_id,
        connection: player.connection_id.to_u128(),
        world: ctx.database_identity(),
        name: character.name,
        race: character.race,
        class: character.class,
        chat_tag: super::available_entity(ctx, guid).map_or(0, |entity| {
            lyracore_shared::chat::chat_tag_for(entity.player_flags)
        }),
        identity: character.owner_identity,
        online,
        stats: online
            .then(|| super::member_stats::snapshot(ctx, guid))
            .flatten(),
        revision: directory::revision(ctx),
    })
}

#[table(accessor = game_party_presence, outbox(remote::idc_party_presence))]
pub struct PartyPresence {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub character: Vec<u8>,
}

pub(super) fn presence(ctx: &ReducerContext, guid: u64, online: bool) {
    if let (Some(realm), Some(character)) = (realm_shard(ctx), facts(ctx, guid, online)) {
        super::realm_guild::presence(ctx, character.clone());
        ctx.db.game_party_presence().insert(PartyPresence {
            id: 0,
            database: remote::Identity(realm),
            character: encode(&character),
        });
    }
}

fn store_character(ctx: &ReducerContext, character: PartyCharacter) {
    let table = ctx.db.game_party_character();
    let previous = table.guid().find(character.guid);
    if previous
        .as_ref()
        .is_some_and(|old| old.revision >= character.revision)
    {
        return;
    }
    directory::observe_revision(ctx, character.revision);
    if realm_shard(ctx).is_some() {
        super::social::projected_presence(ctx, previous.as_ref(), &character);
    }
    table.guid().delete(character.guid);
    table.insert(character);
}

pub(crate) fn event_mark(ctx: &ReducerContext) -> u64 {
    ctx.db
        .game_group_event()
        .iter()
        .map(|event| event.id)
        .max()
        .unwrap_or(0)
}

#[reducer]
pub fn idc_party_presence(ctx: &ReducerContext, character: Vec<u8>) -> Result<(), String> {
    directory::require_world(ctx)?;
    let character: PartyCharacter =
        bsatn::from_slice(&character).map_err(|error| error.to_string())?;
    let entry = ctx
        .db
        .game_character_directory()
        .guid()
        .find(character.guid)
        .ok_or("Character is not in the directory")?;
    if entry.world != ctx.sender()
        || entry.account_id != character.account_id
        || (character.online && current(ctx, &character) != Some(ctx.sender()))
    {
        return Err("World Shard does not hold this Character".into());
    }
    if ctx
        .db
        .game_party_character()
        .guid()
        .find(character.guid)
        .is_some_and(|old| old.revision >= character.revision)
    {
        return Ok(());
    }
    let presence_changed = ctx
        .db
        .game_party_character()
        .guid()
        .find(character.guid)
        .is_none_or(|old| {
            old.online != character.online
                || old.connection != character.connection
                || old.world != character.world
        });
    let guid = character.guid;
    let mark = event_mark(ctx);
    store_character(ctx, character);
    let groups = group::group_of(ctx, guid)
        .map(|member| member.group_id)
        .into_iter()
        .collect();
    if presence_changed {
        if let Some(member) = group::group_of(ctx, guid) {
            group::push_list_to_all(ctx, member.group_id);
        }
    }
    if presence_changed && online(ctx, guid) {
        crate::meeting_stone::resume_seeker(ctx, guid)?;
        if let Some(invite) = ctx.db.game_group_invite().by_target().filter(guid).next() {
            group::push_event(
                ctx,
                guid,
                event_kind::INVITE,
                invite.inviter_guid,
                String::new(),
            );
        }
    }
    publish(ctx, groups, &[guid], mark);
    Ok(())
}

#[table(accessor = game_realm_party_command, outbox(remote::idc_realm_party_command))]
pub struct RealmPartyCommand {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub character: Vec<u8>,
    pub opcode: u32,
    pub body: Vec<u8>,
}

pub(super) fn submit(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    message: &ClientOpcodeMessage,
) -> Result<bool, Rollback> {
    if realm_shard(ctx).is_none() {
        return Ok(false);
    }
    let mut frame = Vec::new();
    message
        .write_unencrypted_client(&mut frame)
        .map_err(|error| error.to_string())?;
    let opcode = u32::from_le_bytes(frame[2..6].try_into().expect("encoded client header"));
    submit_admitted(ctx, connection, guid, opcode, frame[6..].to_vec())
}

pub(super) fn submit_admitted(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    opcode: u32,
    body: Vec<u8>,
) -> Result<bool, Rollback> {
    let Some(realm) = realm_shard(ctx) else {
        return Ok(false);
    };
    let character = facts(ctx, guid, true).ok_or("Character is not in world")?;
    if character.connection != connection.to_u128() {
        return Err("World Session changed".into());
    }
    ctx.db.game_realm_party_command().insert(RealmPartyCommand {
        id: 0,
        database: remote::Identity(realm),
        character: encode(&character),
        opcode,
        body,
    });
    Ok(true)
}

#[reducer]
pub fn idc_realm_party_command(
    ctx: &ReducerContext,
    character: Vec<u8>,
    opcode: u32,
    body: Vec<u8>,
) -> Result<(), String> {
    directory::require_world(ctx)?;
    let actor: PartyCharacter = bsatn::from_slice(&character).map_err(|error| error.to_string())?;
    if opcode == PACKAGE_COMMAND {
        return package::apply(ctx, actor, &body);
    }
    if actor.connection == 0 || current(ctx, &actor) != Some(ctx.sender()) {
        return Err("World Session changed".into());
    }
    let guid = actor.guid;
    let class = actor.class;
    store_character(ctx, actor.clone());
    if opcode == 0x31d {
        return super::instance_reset::dispatch(ctx, actor, &body);
    }
    if super::meeting_stone::is_realm_request(opcode) {
        let mark = event_mark(ctx);
        super::meeting_stone::apply_realm(ctx, guid, opcode, &body)
            .map_err(Rollback::into_reducer_error)?;
        publish(ctx, BTreeSet::new(), &[guid], mark);
        return Ok(());
    }
    let message =
        lyracore_protocol::world::decode(opcode, &body).map_err(|error| error.to_string())?;
    if let ClientOpcodeMessage::CMSG_MESSAGECHAT(chat) = message {
        let kind = super::chat::kind(&chat.chat_type)
            .filter(|kind| is_realm_chat(*kind))
            .ok_or("not a Realm chat request")?;
        return crate::realm_chat::apply_chat(
            ctx,
            guid,
            crate::realm_chat::RealmChatRequest {
                kind,
                language: chat.language.as_int(),
                channel_name: String::new(),
                target_guid: 0,
                message: chat.message,
                speaker: crate::realm_chat::SpeakerFacts {
                    race: actor.race,
                    chat_tag: actor.chat_tag,
                },
            },
        )
        .map_err(|refusal| {
            Rollback::replies(
                super::chat::refusal_packet(guid, kind, String::new(), refusal)
                    .into_iter()
                    .collect(),
                "party chat refused",
            )
            .into_reducer_error()
        });
    }
    let Some((action, reply)) =
        super::party::command(ctx, guid, class, message).map_err(Rollback::into_reducer_error)?
    else {
        return Ok(());
    };
    let target = match action {
        GroupAction::Invite(target) | GroupAction::SetLeader(target) => Some(target),
        GroupAction::Accept { .. } => ctx
            .db
            .game_group_invite()
            .by_target()
            .filter(guid)
            .next()
            .map(|invite| invite.inviter_guid),
        _ => None,
    };
    if let Some(target) = target {
        let refusal = if !online(ctx, target) {
            Some(GroupRefusal::TargetOffline)
        } else if ctx
            .db
            .game_party_character()
            .guid()
            .find(target)
            .is_some_and(|target| !lyracore_shared::faction::same_team(actor.race, target.race))
        {
            Some(GroupRefusal::WrongFaction)
        } else {
            None
        };
        if let Some(refusal) = refusal {
            return Err(reply.refused(refusal.into()).into_reducer_error());
        }
    }
    let mut groups = group::action_groups(ctx, guid, action);
    let mark = event_mark(ctx);
    group::apply_action(ctx, Plane::RealmCore, guid, action)
        .map_err(|error| reply.refused(error).into_reducer_error())?;
    groups.extend(group::action_groups(ctx, guid, action));
    let mut characters = vec![guid];
    characters.extend(target);
    publish(ctx, groups, &characters, mark);
    Ok(())
}

#[reducer(on_result(game_realm_party_command))]
pub fn realm_party_command_answered(
    ctx: &ReducerContext,
    call: RealmPartyCommand,
    result: Result<(), String>,
) -> Result<(), String> {
    let actor: PartyCharacter =
        bsatn::from_slice(&call.character).map_err(|error| error.to_string())?;
    if call.opcode == PACKAGE_COMMAND {
        if let Err(error) = result {
            spacetimedb::log::warn!("Package Group request for {} refused: {error}", actor.guid);
        }
        return Ok(());
    }
    let connection = ConnectionId::from_u128(actor.connection);
    if !ctx
        .db
        .game_native_player()
        .connection_id()
        .find(connection)
        .is_some_and(|player| player.character_guid == actor.guid)
    {
        return Ok(());
    }
    match result {
        Ok(()) => {
            if call.opcode == 0x31d || super::meeting_stone::is_realm_request(call.opcode) {
                return Ok(());
            }
            let message = lyracore_protocol::world::decode(call.opcode, &call.body)
                .map_err(|error| error.to_string())?;
            // Resolve the reply from the request, even if its named target moved during IDC.
            let reply = match message {
                ClientOpcodeMessage::CMSG_GROUP_INVITE(request) => {
                    super::party::CommandReply::Invite(request.name)
                }
                ClientOpcodeMessage::CMSG_GROUP_RAID_CONVERT => super::party::CommandReply::Convert,
                _ => super::party::CommandReply::Silent,
            };
            if let Some(message) = reply.packet(PartyResult::Success) {
                super::send(ctx, connection, message, false)?;
            }
        }
        Err(error) => {
            if let Some(packets) = super::rollback::replies_in(&error) {
                for (opcode, body) in packets {
                    super::send_raw(ctx, connection, opcode, body, false);
                }
            } else {
                super::send(
                    ctx,
                    connection,
                    ServerOpcodeMessage::SMSG_NOTIFICATION(Box::new(SMSG_NOTIFICATION {
                        notification: if call.opcode == 0x31d {
                            "Instance reset request unavailable. Please try again."
                        } else if super::meeting_stone::is_realm_request(call.opcode) {
                            "Meeting Stone request unavailable. Please try again."
                        } else {
                            "Party request unavailable. Please try again."
                        }
                        .into(),
                    })),
                    false,
                )?;
            }
        }
    }
    Ok(())
}

#[table(accessor = game_world_party_update, outbox(remote::idc_world_party_update))]
pub struct WorldPartyUpdate {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub update: Vec<u8>,
}

fn roster(ctx: &ReducerContext, id: u64, guids: &mut BTreeSet<u64>) -> Roster {
    let group = ctx.db.game_group().group_id().find(id);
    let members = group::members_of(ctx, id)
        .into_iter()
        .map(|member| {
            guids.insert(member.character_guid);
            Member {
                guid: member.character_guid,
                raid_slot: member.raid_slot,
            }
        })
        .collect();
    Roster {
        group_id: id,
        leader: group.as_ref().map_or(0, |g| g.leader_guid),
        loot_method: group.as_ref().map_or(0, |g| g.loot_method),
        loot_threshold: group.as_ref().map_or(0, |g| g.loot_threshold),
        master: group.as_ref().map_or(0, |g| g.master_looter_guid),
        kind: group.as_ref().map_or(0, |g| g.group_type),
        members,
        disband: ctx.db.game_party_disband().group_id().find(id),
        icons: group::target_icons_of(ctx, id)
            .into_iter()
            .map(|icon| Icon {
                icon: icon.icon,
                target: icon.target_guid,
            })
            .collect(),
    }
}

pub(crate) fn publish(
    ctx: &ReducerContext,
    mut groups: BTreeSet<u64>,
    extra_characters: &[u64],
    mark: u64,
) {
    let mut guids: BTreeSet<u64> = extra_characters.iter().copied().collect();
    let events = ctx
        .db
        .game_group_event()
        .iter()
        .filter(|event| event.id > mark)
        .filter_map(|event| {
            guids.insert(event.recipient_guid);
            if event.other_guid != 0 {
                guids.insert(event.other_guid);
            }
            let recipient = ctx
                .db
                .game_party_character()
                .guid()
                .find(event.recipient_guid)?;
            current(ctx, &recipient)?;
            Some(Event {
                recipient: event.recipient_guid,
                connection: recipient.connection,
                kind: event.kind,
                other: event.other_guid,
                payload: if event.kind == event_kind::INVITE && recipient.connection == 0 {
                    ctx.db
                        .game_group_invite()
                        .by_target()
                        .filter(event.recipient_guid)
                        .next()
                        .map_or_else(String::new, |invite| invite.id.to_string())
                } else {
                    event.payload
                },
            })
        })
        .collect();
    groups.extend(
        guids
            .iter()
            .filter_map(|guid| group::group_of(ctx, *guid).map(|member| member.group_id)),
    );
    let rosters = groups
        .into_iter()
        .map(|id| roster(ctx, id, &mut guids))
        .collect();
    let absent = guids
        .iter()
        .copied()
        .filter(|guid| group::group_of(ctx, *guid).is_none())
        .collect();
    let characters = guids
        .into_iter()
        .filter_map(|guid| ctx.db.game_party_character().guid().find(guid))
        .collect();
    let update = encode(&Update {
        revision: directory::revision(ctx),
        events_expire_micros: ctx.timestamp.to_micros_since_unix_epoch() + 15_000_000,
        characters,
        rosters,
        events,
        absent,
    });
    let worlds = directory::worlds(ctx);
    for world in worlds {
        ctx.db.game_world_party_update().insert(WorldPartyUpdate {
            id: 0,
            database: remote::Identity(world),
            update: update.clone(),
        });
    }
}

#[reducer]
pub fn idc_world_party_update(ctx: &ReducerContext, update: Vec<u8>) -> Result<(), String> {
    require_sender(ctx, realm_shard(ctx))?;
    apply_snapshot(ctx, &update)
}

pub(super) fn apply_snapshot(ctx: &ReducerContext, update: &[u8]) -> Result<(), String> {
    let update: Update = bsatn::from_slice(update).map_err(|error| error.to_string())?;
    let mut affected: BTreeSet<_> = update.absent.iter().copied().collect();
    for roster in &update.rosters {
        affected.extend(roster.members.iter().map(|member| member.guid));
        affected.extend(
            group::members_of(ctx, roster.group_id)
                .into_iter()
                .map(|member| member.character_guid),
        );
    }
    for character in &update.characters {
        if let Some(member) = group::group_of(ctx, character.guid) {
            affected.extend(
                group::members_of(ctx, member.group_id)
                    .into_iter()
                    .map(|member| member.character_guid),
            );
        }
    }
    let before: Vec<_> = affected
        .into_iter()
        .filter(|guid| {
            ctx.db
                .game_native_player()
                .character_guid()
                .find(*guid)
                .is_some()
        })
        .map(|guid| (guid, super::party::roster_state(ctx, guid)))
        .collect();
    directory::observe_revision(ctx, update.revision);
    for character in update.characters {
        store_character(ctx, character);
    }
    for guid in update.absent {
        let memberships = ctx.db.game_party_membership();
        if memberships
            .guid()
            .find(guid)
            .is_none_or(|old| old.revision <= update.revision)
        {
            memberships.guid().delete(guid);
            memberships.insert(PartyMembership {
                guid,
                group_id: 0,
                raid_slot: 0,
                revision: update.revision,
            });
            restore(ctx, guid);
            crate::instance::reconcile_instance_removal(ctx, guid);
        }
    }
    for roster in update.rosters {
        project(ctx, roster, update.revision);
    }
    for event in update.events {
        if event.connection == 0 && event.kind == event_kind::INVITE {
            if let Ok(id) = event.payload.parse() {
                let _ = package::submit(
                    ctx,
                    event.recipient,
                    package::Action::Answer(package::Invitation {
                        id,
                        inviter: event.other,
                    }),
                );
            }
            continue;
        }
        if matches!(event.kind, event_kind::LIST | event_kind::DESTROYED)
            || ctx.timestamp.to_micros_since_unix_epoch() >= update.events_expire_micros
        {
            continue;
        }

        if ctx
            .db
            .game_party_membership()
            .guid()
            .find(event.recipient)
            .is_some_and(|membership| membership.revision > update.revision)
        {
            continue;
        }
        if ctx
            .db
            .game_native_player()
            .connection_id()
            .find(ConnectionId::from_u128(event.connection))
            .is_some_and(|player| player.character_guid == event.recipient)
        {
            super::party::group_event(
                ctx,
                event.recipient,
                event.kind,
                event.other,
                &event.payload,
            );
        }
    }
    for (guid, previous) in before {
        let current = super::party::roster_state(ctx, guid);
        if current != previous {
            if current.is_some() {
                super::party::roster(ctx, guid, false);
            } else {
                super::party::group_event(ctx, guid, event_kind::DESTROYED, 0, "");
            }
        }
    }
    Ok(())
}

fn project(ctx: &ReducerContext, roster: Roster, revision: u64) {
    let projections = ctx.db.game_party_projection();
    if projections
        .group_id()
        .find(roster.group_id)
        .is_some_and(|old| old.revision > revision)
    {
        return;
    }
    let memberships = ctx.db.game_party_membership();
    let mut affected: BTreeSet<_> = memberships
        .by_group()
        .filter(roster.group_id)
        .map(|member| member.guid)
        .collect();
    affected.extend(roster.members.iter().map(|member| member.guid));
    if roster.leader != 0 {
        let groups = ctx.db.game_group();
        let rr_cursor = groups
            .group_id()
            .find(roster.group_id)
            .map_or(0, |group| group.rr_cursor);
        groups.group_id().delete(roster.group_id);
        groups.insert(Group {
            group_id: roster.group_id,
            leader_guid: roster.leader,
            loot_method: roster.loot_method,
            loot_threshold: roster.loot_threshold,
            master_looter_guid: roster.master,
            rr_cursor,
            group_type: roster.kind,
        });
    } else {
        if let Some(disband) = roster.disband {
            crate::loot::force_resolve_rolls_for_disband(
                ctx,
                &disband.members,
                disband.survivor,
                Some(roster.group_id),
            );
        }
        ctx.db.game_group().group_id().delete(roster.group_id);
    }
    let icons = ctx.db.game_group_target_icon();
    for icon in icons.by_group().filter(roster.group_id).collect::<Vec<_>>() {
        icons.id().delete(icon.id);
    }
    for icon in roster.icons {
        group::write_target_icon(
            ctx,
            roster.group_id,
            TargetIcon {
                icon: icon.icon,
                target_guid: icon.target,
            },
        );
    }
    for guid in affected {
        if memberships
            .guid()
            .find(guid)
            .is_some_and(|old| old.revision > revision)
        {
            continue;
        }
        let member = roster.members.iter().find(|member| member.guid == guid);
        memberships.guid().delete(guid);
        memberships.insert(PartyMembership {
            guid,
            group_id: member.map_or(0, |_| roster.group_id),
            raid_slot: member.map_or(0, |member| member.raid_slot),
            revision,
        });
        restore(ctx, guid);
        package::membership_changed(ctx, guid);
        crate::instance::reconcile_instance_removal(ctx, guid);
    }
    projections.group_id().delete(roster.group_id);
    projections.insert(PartyProjection {
        group_id: roster.group_id,
        revision,
    });
}

pub(super) fn restore(ctx: &ReducerContext, guid: u64) {
    let Some(membership) = ctx.db.game_party_membership().guid().find(guid) else {
        return;
    };
    let members = ctx.db.game_group_member();
    let previous: Vec<_> = members.by_character().filter(guid).collect();
    if previous.len() == 1
        && previous[0].group_id == membership.group_id
        && membership.group_id != 0
    {
        let mut member = previous.into_iter().next().expect("one member checked");
        member.raid_slot = membership.raid_slot;
        member.owner_identity = identity(ctx, guid).unwrap_or(member.owner_identity);
        members.id().update(member);
        return;
    }
    for member in previous {
        if member.group_id != membership.group_id {
            crate::loot::tag::revoke_group_member(ctx, member.group_id, guid);
        }
        members.id().delete(member.id);
    }
    if membership.group_id != 0
        && ctx
            .db
            .game_group()
            .group_id()
            .find(membership.group_id)
            .is_some()
    {
        let identity = ctx
            .db
            .game_party_character()
            .guid()
            .find(guid)
            .map_or(ctx.database_identity(), |character| character.identity);
        members.insert(GroupMember {
            id: 0,
            group_id: membership.group_id,
            character_guid: guid,
            owner_identity: identity,
            raid_slot: membership.raid_slot,
        });
    }
}

pub(super) fn deleted(ctx: &ReducerContext, guid: u64) -> Result<(), String> {
    crate::meeting_stone::remove_solo_seeker(ctx, guid);
    let mark = event_mark(ctx);
    let action = GroupAction::Leave {
        cause: lyracore_shared::group::leave_cause::CHARACTER_DELETED,
    };
    let groups = group::action_groups(ctx, guid, action);
    if group::group_of(ctx, guid).is_some() {
        group::apply_action(ctx, Plane::RealmCore, guid, action)
            .map_err(|error| error.to_string())?;
    }
    if let Some(mut character) = ctx.db.game_party_character().guid().find(guid) {
        character.online = false;
        character.stats = None;
        character.revision = directory::revision(ctx);
        store_character(ctx, character);
    }
    publish(ctx, groups, &[guid], mark);
    ctx.db.game_party_character().guid().delete(guid);
    Ok(())
}

pub(super) fn is_realm_chat(kind: u8) -> bool {
    matches!(
        kind,
        chat_kind::PARTY
            | chat_kind::RAID
            | chat_kind::RAID_LEADER
            | chat_kind::RAID_WARNING
            | chat_kind::GUILD
            | chat_kind::OFFICER
    )
}

pub(crate) fn relay_chat(ctx: &ReducerContext, line: &crate::realm_chat::RealmChatLine) {
    if world_shard(ctx).is_none() || !is_realm_chat(line.kind) {
        return;
    }
    let Some(message) = codec::build_realm_chat_line(
        line.kind,
        line.speaker_guid,
        line.language,
        line.chat_tag,
        line.channel_name.clone(),
        line.message.clone(),
    ) else {
        return;
    };
    let Ok(packet) =
        lyracore_protocol::world::encode(ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(message)))
    else {
        return;
    };
    for &guid in &line.recipients {
        let Some(character) = ctx.db.game_party_character().guid().find(guid) else {
            continue;
        };
        super::realm_communication::deliver(
            ctx,
            super::realm_communication::Participant {
                account_id: character.account_id,
                connection: character.connection,
                guid,
            },
            vec![(packet.opcode, packet.body.clone())],
            line.ignorable.then_some(line.speaker_guid),
        );
    }
}

pub(crate) fn identity(ctx: &ReducerContext, guid: u64) -> Option<Identity> {
    ctx.db
        .game_party_character()
        .guid()
        .find(guid)
        .map(|character| character.identity)
}

pub(super) fn snapshot(ctx: &ReducerContext, guid: u64) -> Vec<u8> {
    let mut guids = BTreeSet::from([guid]);
    let rosters = group::group_of(ctx, guid)
        .map(|member| roster(ctx, member.group_id, &mut guids))
        .into_iter()
        .collect();
    encode(&Update {
        revision: directory::revision(ctx),
        events_expire_micros: 0,
        rosters,
        characters: guids
            .into_iter()
            .filter_map(|guid| ctx.db.game_party_character().guid().find(guid))
            .collect(),
        events: Vec::new(),
        absent: if group::group_of(ctx, guid).is_none() {
            vec![guid]
        } else {
            Vec::new()
        },
    })
}

#[table(accessor = game_party_disband)]
pub struct PartyDisband {
    #[primary_key]
    pub group_id: u64,
    pub members: Vec<u64>,
    pub survivor: Option<u64>,
}

pub(crate) fn disbanded(
    ctx: &ReducerContext,
    group_id: u64,
    members: &[u64],
    survivor: Option<u64>,
) {
    if world_shard(ctx).is_some() {
        super::instance_allocation::forget(ctx, group_id, true);
        ctx.db.game_party_disband().group_id().delete(group_id);
        ctx.db.game_party_disband().insert(PartyDisband {
            group_id,
            members: members.to_vec(),
            survivor,
        });
    }
}

#[table(accessor = game_party_projection_retry, scheduled(retry_party_projection))]
pub struct PartyProjectionRetry {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub scheduled_at: ScheduleAt,
    pub call: WorldPartyUpdate,
}

#[reducer(on_result(game_world_party_update))]
pub fn world_party_update_answered(
    ctx: &ReducerContext,
    call: WorldPartyUpdate,
    result: Result<(), String>,
) {
    if let Err(error) = result {
        spacetimedb::log::warn!("Group projection to {} waits: {error}", call.database.0);
        ctx.db
            .game_party_projection_retry()
            .insert(PartyProjectionRetry {
                id: 0,
                scheduled_at: (ctx.timestamp + TimeDuration::from_micros(1_000_000)).into(),
                call,
            });
    }
}

#[reducer]
pub fn retry_party_projection(ctx: &ReducerContext, retry: PartyProjectionRetry) {
    if crate::time::scheduler_authorized(ctx) && crate::peer::is_peer(ctx, retry.call.database.0) {
        ctx.db.game_world_party_update().insert(WorldPartyUpdate {
            id: 0,
            ..retry.call
        });
    }
}

pub(crate) fn has_world_session(ctx: &ReducerContext, guid: u64) -> bool {
    ctx.db
        .game_character_directory()
        .guid()
        .find(guid)
        .and_then(|entry| {
            ctx.db
                .game_native_away()
                .account_id()
                .find(entry.account_id)
        })
        .is_some_and(|away| away.character_guid == guid)
}
