use std::collections::BTreeSet;

use lyracore_protocol::{
    codec,
    messages::vanilla::opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
};
use lyracore_shared::{faction::team_for_race, guild::event_kind};
use spacetimedb::{
    reducer, sats::bsatn, table, ConnectionId, ReducerContext, ScheduleAt, SpacetimeType, Table,
    TimeDuration,
};

use super::{
    directory, game_native_player,
    realm_party::{self, PartyCharacter},
    Rollback,
};
use crate::{
    guild::{
        self, game_guild, game_guild_event, game_guild_member, game_guild_rank, Guild, GuildMember,
        GuildRank,
    },
    peer::{realm_shard, require_sender, world_shard},
    remote_bindings::peer as remote,
};

// Opcode zero carries the GM command's explicit leader instead of client packet bytes.
const CREATE_FOR_LEADER: u32 = 0;

#[derive(SpacetimeType)]
pub struct Facts {
    pub(super) character: PartyCharacter,
    ignores: Vec<u64>,
    level: u8,
    zone: u32,
    logout_micros: i64,
}

#[table(accessor = game_guild_character)]
pub struct GuildCharacter {
    #[primary_key]
    pub guid: u64,
    pub facts: Facts,
    pub level: u8,
    pub zone: u32,
    pub logout_micros: i64,
}

#[table(accessor = game_guild_projection)]
pub struct GuildProjection {
    #[primary_key]
    pub guild_id: u32,
    pub revision: u64,
}

#[table(accessor = game_guild_membership_projection, index(accessor = by_guild, btree(columns = [guild_id])))]
pub struct GuildMembershipProjection {
    #[primary_key]
    pub guid: u64,
    pub revision: u64,
    pub guild_id: u32,
    pub member: Option<GuildMember>,
}

#[derive(SpacetimeType)]
pub struct Snapshot {
    guild_id: u32,
    guild: Option<Guild>,
    ranks: Vec<GuildRank>,
    members: Vec<GuildMember>,
}

#[derive(SpacetimeType)]
pub struct Update {
    revision: u64,
    guilds: Vec<Snapshot>,
    members: Vec<GuildMembershipProjection>,
}

fn encode<T: spacetimedb::Serialize>(value: &T) -> Vec<u8> {
    bsatn::to_vec(value).expect("guild state encodes")
}

pub(super) fn facts(ctx: &ReducerContext, character: PartyCharacter) -> Option<Facts> {
    use crate::chat::game_character_contact;
    use crate::package_account::game_package_account;
    let stored = crate::helpers::character_by_guid(ctx, character.guid)?;
    let stats = character
        .stats
        .as_ref()
        .map(|stats| stats.friend_online(character.class, 0));
    Some(Facts {
        level: stats.as_ref().map_or(stored.level, |stats| stats.level),
        zone: stats.as_ref().map_or(stored.zone_id, |stats| stats.zone_id),
        logout_micros: if !character.online
            && (character.connection != 0
                || ctx
                    .db
                    .game_package_account()
                    .account_id()
                    .find(character.account_id)
                    .is_some())
        {
            ctx.timestamp.to_micros_since_unix_epoch()
        } else {
            stored.last_logout_micros.min(i64::MAX as u64) as i64
        },
        ignores: ctx
            .db
            .game_character_contact()
            .by_owner()
            .filter(character.guid)
            .filter(|contact| contact.is_ignore)
            .map(|contact| contact.target_guid)
            .collect(),
        character,
    })
}

#[table(accessor = game_realm_guild_presence, outbox(remote::idc_realm_guild_presence))]
pub struct RealmGuildPresence {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub facts: Vec<u8>,
}

pub(super) fn presence(ctx: &ReducerContext, character: PartyCharacter) {
    if let (Some(realm), Some(facts)) = (realm_shard(ctx), facts(ctx, character)) {
        ctx.db
            .game_realm_guild_presence()
            .insert(RealmGuildPresence {
                id: 0,
                database: remote::Identity(realm),
                facts: encode(&facts),
            });
    }
}

#[reducer]
pub fn operator_report_guild_characters(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if realm_shard(ctx).is_none() || ctx.db.game_native_player().iter().next().is_some() {
        return Err("guild roster backfill requires a World Shard with gameplay closed".into());
    }
    for member in ctx.db.game_guild_member().iter() {
        let Some(character) = crate::helpers::character_by_guid(ctx, member.character_guid) else {
            continue;
        };
        presence(
            ctx,
            PartyCharacter {
                guid: character.guid,
                account_id: character.account_id,
                connection: 0,
                world: ctx.database_identity(),
                name: character.name,
                race: character.race,
                class: character.class,
                chat_tag: 0,
                identity: character.owner_identity,
                online: false,
                stats: None,
                revision: directory::revision(ctx),
            },
        );
    }
    Ok(())
}

pub(super) fn queue(
    ctx: &ReducerContext,
    guid: u64,
    opcode: u32,
    body: Vec<u8>,
) -> Result<RealmGuildCommand, String> {
    let realm = realm_shard(ctx).ok_or("World Shard has no Realm Shard")?;
    let character = realm_party::facts(ctx, guid, true)
        .or_else(|| {
            let character = crate::helpers::character_by_guid(ctx, guid)?;
            Some(PartyCharacter {
                guid,
                account_id: character.account_id,
                connection: 0,
                world: ctx.database_identity(),
                name: character.name,
                race: character.race,
                class: character.class,
                chat_tag: 0,
                identity: character.owner_identity,
                online: false,
                stats: None,
                revision: directory::revision(ctx),
            })
        })
        .ok_or("Character is not owned here")?;
    Ok(ctx.db.game_realm_guild_command().insert(RealmGuildCommand {
        id: 0,
        database: remote::Identity(realm),
        facts: encode(&facts(ctx, character).ok_or("Character is not owned here")?),
        opcode,
        body,
    }))
}

fn current(ctx: &ReducerContext, facts: &Facts) -> bool {
    facts.character.world == ctx.sender()
        && realm_party::current(ctx, &facts.character) == Some(ctx.sender())
}

fn store_character(ctx: &ReducerContext, facts: Facts) -> bool {
    let table = ctx.db.game_guild_character();
    let guid = facts.character.guid;
    let previous = table.guid().find(guid);
    if previous
        .as_ref()
        .is_some_and(|old| old.facts.character.revision >= facts.character.revision)
    {
        return false;
    }
    let changed = previous.as_ref().is_none_or(|old| {
        old.facts.character.online != facts.character.online
            || old.facts.character.connection != facts.character.connection
    });
    directory::observe_revision(ctx, facts.character.revision);
    table.guid().delete(guid);
    table.insert(GuildCharacter {
        guid,
        level: facts.level,
        zone: facts.zone,
        logout_micros: facts.logout_micros,
        facts,
    });
    changed
}

#[reducer]
pub fn idc_realm_guild_presence(ctx: &ReducerContext, facts: Vec<u8>) -> Result<(), String> {
    use super::directory::game_character_directory;
    directory::require_world(ctx)?;
    let facts: Facts = bsatn::from_slice(&facts).map_err(|error| error.to_string())?;
    let character = &facts.character;
    let entry = ctx
        .db
        .game_character_directory()
        .guid()
        .find(character.guid)
        .ok_or("Character is not in the directory")?;
    if entry.world != ctx.sender()
        || character.world != ctx.sender()
        || entry.account_id != character.account_id
        || (character.online && !current(ctx, &facts))
    {
        return Err("World Shard does not hold this Character".into());
    }
    if !character.online {
        use super::realm_shard::game_native_away;
        if ctx
            .db
            .game_native_away()
            .account_id()
            .find(character.account_id)
            .is_some_and(|away| {
                away.character_guid == character.guid
                    && away.connection_id.to_u128() != character.connection
            })
        {
            return Ok(());
        }
    }
    record_presence(ctx, facts)
}

pub(super) fn record_presence(ctx: &ReducerContext, facts: Facts) -> Result<(), String> {
    let character = &facts.character;
    let (guid, online, account, name) = (
        character.guid,
        character.online,
        character.account_id,
        character.name.clone(),
    );
    let changed = store_character(ctx, facts);
    let Some(member) = guild::member(ctx, guid) else {
        return Ok(());
    };
    if changed {
        let op = if online {
            guild::GuildOp::SignOn { actor_name: name }
        } else {
            guild::GuildOp::SignOff
        };
        guild::apply_op(ctx, guid, account, op).map_err(|error| error.as_tag().to_string())?;
        if online {
            if let Some(guild) = ctx.db.game_guild().guild_id().find(member.guild_id) {
                let packet = codec::build_guild_event_raw(event_kind::MOTD, &[guild.motd], 0);
                deliver(ctx, guid, vec![packet], None);
            }
        }
        publish(ctx, BTreeSet::from([member.guild_id]));
    }
    Ok(())
}

#[table(accessor = game_realm_guild_command, outbox(remote::idc_realm_guild_command))]
pub struct RealmGuildCommand {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub facts: Vec<u8>,
    pub opcode: u32,
    pub body: Vec<u8>,
}

pub(super) fn submit(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    message: &ClientOpcodeMessage,
) -> Result<bool, Rollback> {
    let Some(realm) = realm_shard(ctx) else {
        return Ok(false);
    };
    if has_local_guilds(ctx) {
        return Err(Rollback::reply(
            unavailable(),
            "local guild state requires migration",
        ));
    }
    let character = realm_party::facts(ctx, guid, true).ok_or("Character is not in world")?;
    if character.connection != connection.to_u128() {
        return Err("World Session changed".into());
    }
    let mut frame = Vec::new();
    message
        .write_unencrypted_client(&mut frame)
        .map_err(|error| error.to_string())?;
    let opcode = u32::from_le_bytes(frame[2..6].try_into().expect("encoded client header"));
    ctx.db.game_realm_guild_command().insert(RealmGuildCommand {
        id: 0,
        database: remote::Identity(realm),
        facts: encode(&facts(ctx, character).ok_or("Character is not owned here")?),
        opcode,
        body: frame[6..].to_vec(),
    });
    Ok(true)
}

fn invitee(ctx: &ReducerContext, actor: u64, name: &str) -> Option<super::guild::Invitee> {
    let characters = ctx.db.game_guild_character();
    let mut matches = characters.iter().filter(|row| {
        let character = &row.facts.character;
        character.name.eq_ignore_ascii_case(name)
            && character.online
            && realm_party::current(ctx, character) == Some(character.world)
    });
    let target = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    invitee_by_guid(ctx, actor, target.guid)
}

pub(super) fn invitee_by_guid(
    ctx: &ReducerContext,
    actor: u64,
    guid: u64,
) -> Option<super::guild::Invitee> {
    let target = ctx.db.game_guild_character().guid().find(guid)?;
    let character = &target.facts.character;
    if !character.online || realm_party::current(ctx, character) != Some(character.world) {
        return None;
    }
    Some(super::guild::Invitee {
        guid: target.guid,
        name: target.facts.character.name,
        team: team_for_race(target.facts.character.race),
        ignores_actor: target.facts.ignores.contains(&actor),
    })
}

#[reducer]
pub fn idc_realm_guild_command(
    ctx: &ReducerContext,
    facts: Vec<u8>,
    opcode: u32,
    body: Vec<u8>,
) -> Result<(), String> {
    directory::require_world(ctx)?;
    let facts: Facts = bsatn::from_slice(&facts).map_err(|error| error.to_string())?;
    let operation = (opcode == super::realm_petition::TRANSACTION)
        .then(|| bsatn::from_slice::<super::realm_petition::Operation>(&body))
        .transpose()
        .map_err(|error| error.to_string())?;
    let background = operation
        .as_ref()
        .is_some_and(|operation| operation.background());
    if background {
        use directory::game_character_directory;
        if facts.character.world != ctx.sender()
            || ctx
                .db
                .game_character_directory()
                .guid()
                .find(facts.character.guid)
                .is_none_or(|entry| {
                    entry.world != ctx.sender() || entry.account_id != facts.character.account_id
                })
        {
            return Err("World Shard does not hold this Character".into());
        }
    } else if !current(ctx, &facts) || !facts.character.online {
        return Err("World Session changed".into());
    }
    let actor = super::guild::Actor {
        guid: facts.character.guid,
        account_id: facts.character.account_id,
        name: facts.character.name.clone(),
        team: team_for_race(facts.character.race),
    };
    if facts.character.online && current(ctx, &facts) {
        record_presence(ctx, facts)?;
    }
    let mut guilds: BTreeSet<_> = guild::member(ctx, actor.guid)
        .map(|member| member.guild_id)
        .into_iter()
        .collect();
    let mark = ctx
        .db
        .game_guild_event()
        .iter()
        .map(|event| event.id)
        .max()
        .unwrap_or(0);
    let reply = if let Some(operation) = operation {
        super::realm_petition::apply(ctx, &actor, operation)
            .map_err(Rollback::into_reducer_error)?
    } else if opcode == CREATE_FOR_LEADER {
        let leader = create(ctx, &actor, &body).map_err(Rollback::into_reducer_error)?;
        guilds.extend(guild::member(ctx, leader).map(|member| member.guild_id));
        None
    } else {
        let message =
            lyracore_protocol::world::decode(opcode, &body).map_err(|error| error.to_string())?;
        if super::realm_petition::is_request(&message) {
            super::realm_petition::request(ctx, &actor, message)
                .map_err(Rollback::into_reducer_error)?
        } else {
            super::guild::request(
                ctx,
                &actor,
                crate::auth::gm_level(ctx, actor.account_id),
                message,
                |name| invitee(ctx, actor.guid, name),
            )
            .map_err(Rollback::into_reducer_error)?
        }
    };
    guilds.extend(guild::member(ctx, actor.guid).map(|member| member.guild_id));
    guilds.extend(
        ctx.db
            .game_guild_event()
            .iter()
            .filter(|event| event.id > mark)
            .map(|event| event.guild_id),
    );
    publish(ctx, guilds);
    if let Some(reply) = reply {
        let packet = lyracore_protocol::world::encode(reply).map_err(|error| error.to_string())?;
        deliver(ctx, actor.guid, vec![(packet.opcode, packet.body)], None);
    }
    Ok(())
}

#[reducer(on_result(game_realm_guild_command))]
pub fn realm_guild_command_answered(
    ctx: &ReducerContext,
    call: RealmGuildCommand,
    result: Result<(), String>,
) -> Result<(), String> {
    let facts: Facts = bsatn::from_slice(&call.facts).map_err(|error| error.to_string())?;
    if call.opcode == super::realm_petition::TRANSACTION {
        return super::realm_petition::answered(
            ctx,
            &call,
            facts.character.guid,
            facts.character.connection,
            result,
        );
    }
    let Err(error) = result else { return Ok(()) };
    let connection = ConnectionId::from_u128(facts.character.connection);
    if ctx
        .db
        .game_native_player()
        .connection_id()
        .find(connection)
        .is_none_or(|player| player.character_guid != facts.character.guid)
    {
        return Ok(());
    }
    if let Some(packets) = super::rollback::replies_in(&error) {
        for (opcode, body) in packets {
            super::send_raw(ctx, connection, opcode, body, false);
        }
    } else {
        super::send(ctx, connection, unavailable(), false)?;
    }
    Ok(())
}

pub(super) fn unavailable() -> ServerOpcodeMessage {
    ServerOpcodeMessage::SMSG_NOTIFICATION(Box::new(
        lyracore_protocol::messages::vanilla::SMSG_NOTIFICATION {
            notification: "Guild request unavailable. Please try again.".into(),
        },
    ))
}

fn snapshot(ctx: &ReducerContext, guild_id: u32) -> Snapshot {
    Snapshot {
        guild_id,
        guild: ctx.db.game_guild().guild_id().find(guild_id),
        ranks: guild::ranks(ctx, guild_id),
        members: ctx
            .db
            .game_guild_member()
            .by_guild()
            .filter(guild_id)
            .collect(),
    }
}

#[table(accessor = game_world_guild_update, outbox(remote::idc_world_guild_update))]
pub struct WorldGuildUpdate {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub update: Vec<u8>,
}

pub(super) fn publish(ctx: &ReducerContext, guilds: BTreeSet<u32>) {
    let guilds: Vec<_> = guilds
        .into_iter()
        .filter(|id| *id != 0)
        .map(|id| snapshot(ctx, id))
        .collect();
    if guilds.is_empty() {
        return;
    }
    let revision = directory::revision(ctx);
    let mut members = Vec::new();
    for state in &guilds {
        members.extend(
            ctx.db
                .game_guild_membership_projection()
                .by_guild()
                .filter(state.guild_id),
        );
        ctx.db
            .game_guild_projection()
            .guild_id()
            .delete(state.guild_id);
        ctx.db.game_guild_projection().insert(GuildProjection {
            guild_id: state.guild_id,
            revision,
        });
    }
    let update = encode(&Update {
        revision,
        guilds,
        members,
    });
    let worlds = directory::worlds(ctx);
    for world in worlds {
        ctx.db.game_world_guild_update().insert(WorldGuildUpdate {
            id: 0,
            database: remote::Identity(world),
            update: update.clone(),
        });
    }
}

#[reducer]
pub fn idc_world_guild_update(ctx: &ReducerContext, update: Vec<u8>) -> Result<(), String> {
    require_sender(ctx, realm_shard(ctx))?;
    let update: Update = bsatn::from_slice(&update).map_err(|error| error.to_string())?;
    directory::observe_revision(ctx, update.revision);
    for snapshot in update.guilds {
        project(ctx, snapshot, update.revision)?;
    }
    for member in update.members {
        project_member(ctx, member);
    }
    Ok(())
}

fn project(ctx: &ReducerContext, snapshot: Snapshot, revision: u64) -> Result<(), String> {
    let projections = ctx.db.game_guild_projection();
    if projections
        .guild_id()
        .find(snapshot.guild_id)
        .is_some_and(|old| old.revision >= revision)
    {
        return Ok(());
    }
    if projections.guild_id().find(snapshot.guild_id).is_none()
        && ctx
            .db
            .game_guild()
            .guild_id()
            .find(snapshot.guild_id)
            .is_some()
        && !same_import(&self::snapshot(ctx, snapshot.guild_id), &snapshot)
    {
        return Err("projection conflicts with unmigrated local guild state".into());
    }
    ctx.db.game_guild().guild_id().delete(snapshot.guild_id);
    if let Some(guild) = snapshot.guild {
        ctx.db.game_guild().insert(guild);
    }
    ctx.db
        .game_guild_rank()
        .by_guild()
        .delete(snapshot.guild_id);
    for rank in snapshot.ranks {
        ctx.db.game_guild_rank().insert(GuildRank {
            id: (u64::from(rank.guild_id) << 32) | (u64::from(rank.rank_id) + 1),
            ..rank
        });
    }
    projections.guild_id().delete(snapshot.guild_id);
    projections.insert(GuildProjection {
        guild_id: snapshot.guild_id,
        revision,
    });
    Ok(())
}

pub(super) fn deliver(
    ctx: &ReducerContext,
    guid: u64,
    packets: Vec<(u16, Vec<u8>)>,
    ignore: Option<u64>,
) {
    let Some(row) = ctx.db.game_guild_character().guid().find(guid) else {
        return;
    };
    let character = row.facts.character;
    if !character.online || realm_party::current(ctx, &character) != Some(character.world) {
        return;
    }
    super::realm_communication::deliver(
        ctx,
        super::realm_communication::Participant {
            account_id: character.account_id,
            connection: character.connection,
            guid,
        },
        packets,
        ignore,
    );
}

pub(super) fn event(ctx: &ReducerContext, event: &guild::GuildEvent) -> bool {
    if world_shard(ctx).is_none() {
        return false;
    }
    if event_kind::is_petition(event.kind) {
        if let Some(packet) = super::petition::event_packet(ctx, event) {
            let packet = super::bounded(packet);
            deliver(
                ctx,
                event.recipient_guid,
                vec![(packet.opcode, packet.body)],
                None,
            );
        }
        return true;
    }
    let recipients: Vec<_> = if event.recipient_guid != 0 {
        vec![event.recipient_guid]
    } else {
        ctx.db
            .game_guild_member()
            .by_guild()
            .filter(event.guild_id)
            .filter(|member| {
                event.kind != event_kind::SIGNED_ON || member.character_guid != event.subject_guid
            })
            .map(|member| member.character_guid)
            .collect()
    };
    for guid in recipients {
        let mut packets = Vec::new();
        if matches!(
            event.kind,
            event_kind::ROSTER_REFRESH | event_kind::ROSTER_TO_ACTOR
        ) {
            if event.kind == event_kind::ROSTER_REFRESH {
                if let Some(guild) = super::guild::view(ctx, event.guild_id) {
                    let packet = super::bounded(ServerOpcodeMessage::SMSG_GUILD_QUERY_RESPONSE(
                        Box::new(codec::build_guild_query_response(&guild)),
                    ));
                    packets.push((packet.opcode, packet.body));
                }
            }
            if let Some(member) =
                guild::member(ctx, guid).filter(|member| member.guild_id == event.guild_id)
            {
                if let Some(packet) = super::guild::roster(ctx, event.guild_id, member.rank_id) {
                    let packet = super::bounded(packet);
                    packets.push((packet.opcode, packet.body));
                }
            }
        } else if event.kind == event_kind::INVITE {
            let packet = super::bounded(ServerOpcodeMessage::SMSG_GUILD_INVITE(Box::new(
                codec::build_guild_invite(
                    event.strings.first().cloned().unwrap_or_default(),
                    event.strings.get(1).cloned().unwrap_or_default(),
                ),
            )));
            packets.push((packet.opcode, packet.body));
        } else if event.kind == event_kind::DECLINE {
            packets.push(codec::build_guild_decline_raw(
                event.strings.first().map_or("", String::as_str),
            ));
        } else {
            packets.push(codec::build_guild_event_raw(
                event.kind,
                &event.strings,
                event.subject_guid,
            ));
        }
        let ignore = if event.kind == event_kind::INVITE {
            use crate::guild::membership::game_guild_invite;
            ctx.db
                .game_guild_invite()
                .target_guid()
                .find(guid)
                .map(|invite| invite.inviter_guid)
        } else {
            None
        };
        deliver(ctx, guid, packets, ignore);
    }
    true
}

pub(super) fn roster_line(ctx: &ReducerContext, line: &mut codec::GuildRosterLine) {
    let Some(row) = ctx.db.game_guild_character().guid().find(line.guid) else {
        return;
    };
    let character = row.facts.character;
    line.name = character.name.clone();
    line.level = row.level;
    line.zone_id = row.zone;
    line.class = character.class;
    line.days_offline =
        if character.online && realm_party::current(ctx, &character) == Some(character.world) {
            None
        } else {
            Some(if row.logout_micros == 0 {
                0.0
            } else {
                ctx.timestamp
                    .to_micros_since_unix_epoch()
                    .saturating_sub(row.logout_micros)
                    .max(0) as f32
                    / 86_400_000_000.0
            })
        };
}

pub(super) fn deleted(ctx: &ReducerContext, guid: u64) {
    let guilds = guild::member(ctx, guid)
        .map(|member| member.guild_id)
        .into_iter()
        .collect();
    guild::forget_deleted_character(ctx, guid);
    publish(ctx, guilds);
    ctx.db.game_guild_character().guid().delete(guid);
}

#[table(accessor = game_guild_projection_retry, scheduled(retry_guild_projection))]
pub struct GuildProjectionRetry {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub scheduled_at: ScheduleAt,
    pub call: WorldGuildUpdate,
}
crate::virtual_clock_exempt!(game_guild_projection_retry);

#[reducer(on_result(game_world_guild_update))]
pub fn world_guild_update_answered(
    ctx: &ReducerContext,
    call: WorldGuildUpdate,
    result: Result<(), String>,
) {
    if let Err(error) = result {
        spacetimedb::log::warn!("Guild projection to {} waits: {error}", call.database.0);
        ctx.db
            .game_guild_projection_retry()
            .insert(GuildProjectionRetry {
                id: 0,
                scheduled_at: (ctx.timestamp + TimeDuration::from_micros(1_000_000)).into(),
                call,
            });
    }
}

#[reducer]
pub fn retry_guild_projection(ctx: &ReducerContext, retry: GuildProjectionRetry) {
    if crate::time::scheduler_authorized(ctx) && crate::peer::is_peer(ctx, retry.call.database.0) {
        ctx.db.game_world_guild_update().insert(WorldGuildUpdate {
            id: 0,
            ..retry.call
        });
    }
}

#[derive(SpacetimeType)]
pub struct Import {
    pub guild: Guild,
    pub ranks: Vec<GuildRank>,
    pub members: Vec<GuildMember>,
    pub invites: Vec<crate::guild::membership::GuildInvite>,
}

#[reducer]
pub fn operator_import_guild(ctx: &ReducerContext, state: Import) -> Result<(), String> {
    use super::{directory::game_character_profile, realm_shard::game_native_away};
    use crate::guild::membership::game_guild_invite;
    crate::helpers::require_operator(ctx)?;
    if world_shard(ctx).is_none()
        || ctx
            .db
            .game_native_away()
            .iter()
            .any(|away| away.character_guid != 0)
    {
        return Err("guild import requires a Realm Shard with gameplay closed".into());
    }
    use lyracore_shared::guild::{
        fits_length, validate_guild_name, MAX_INFO, MAX_MOTD, MAX_NOTE, MAX_RANK_NAME,
    };
    if validate_guild_name(&state.guild.name).is_err()
        || !fits_length(&state.guild.motd, MAX_MOTD)
        || !fits_length(&state.guild.info, MAX_INFO)
        || state
            .ranks
            .iter()
            .any(|rank| !fits_length(&rank.name, MAX_RANK_NAME))
        || state.members.iter().any(|member| {
            !fits_length(&member.public_note, MAX_NOTE)
                || !fits_length(&member.officer_note, MAX_NOTE)
        })
    {
        return Err("guild text is outside protocol limits".into());
    }
    let id = state.guild.guild_id;
    if id == 0
        || state.guild.name_key != lyracore_shared::guild::name_key(&state.guild.name)
        || ctx.db.game_guild().guild_id().find(id).is_some()
        || ctx
            .db
            .game_guild()
            .name_key()
            .find(state.guild.name_key.clone())
            .is_some()
    {
        return Err("guild id or name conflicts with Realm state".into());
    }
    let ranks: BTreeSet<_> = state.ranks.iter().map(|rank| rank.rank_id).collect();
    if ranks.len() != state.ranks.len()
        || !(lyracore_shared::guild::MIN_RANKS..=lyracore_shared::guild::MAX_RANKS)
            .contains(&ranks.len())
        || ranks.iter().copied().ne(0..ranks.len() as u32)
        || state.ranks.iter().any(|rank| rank.guild_id != id)
    {
        return Err("guild ranks are inconsistent".into());
    }
    let members: BTreeSet<_> = state
        .members
        .iter()
        .map(|member| member.character_guid)
        .collect();
    if members.len() != state.members.len()
        || !state
            .members
            .iter()
            .any(|member| member.character_guid == state.guild.leader_guid && member.rank_id == 0)
    {
        return Err("guild membership or leader is inconsistent".into());
    }
    for member in &state.members {
        let profile = ctx
            .db
            .game_character_profile()
            .guid()
            .find(member.character_guid)
            .ok_or("guild member needs a Character profile")?;
        if member.guild_id != id
            || !ranks.contains(&member.rank_id)
            || profile.deleted
            || profile.account_id != member.realm_account_id
            || profile.name != member.name
            || team_for_race(profile.race) != state.guild.team
            || (member.rank_id == 0 && member.character_guid != state.guild.leader_guid)
            || guild::member(ctx, member.character_guid).is_some()
        {
            return Err("guild member conflicts with Realm state".into());
        }
    }
    let targets: BTreeSet<_> = state
        .invites
        .iter()
        .map(|invite| invite.target_guid)
        .collect();
    if targets.len() != state.invites.len()
        || state.invites.iter().any(|invite| {
            invite.guild_id != id
                || !members.contains(&invite.inviter_guid)
                || members.contains(&invite.target_guid)
                || guild::member(ctx, invite.target_guid).is_some()
                || ctx
                    .db
                    .game_guild_invite()
                    .target_guid()
                    .find(invite.target_guid)
                    .is_some()
        })
    {
        return Err("guild invitations conflict with Realm state".into());
    }
    guild::IdSequence::Guild.observe(ctx, id);
    ctx.db.game_guild().insert(state.guild);
    for rank in state.ranks {
        ctx.db.game_guild_rank().insert(GuildRank { id: 0, ..rank });
    }
    for member in state.members {
        member.insert(ctx);
    }
    for invite in state.invites {
        invite.insert(ctx);
    }
    publish(ctx, BTreeSet::from([id]));
    Ok(())
}

#[reducer]
pub fn operator_report_realm_guilds(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if world_shard(ctx).is_none() {
        return Err("guild replay requires a Realm Shard".into());
    }
    let mut ids: BTreeSet<_> = ctx
        .db
        .game_guild()
        .iter()
        .map(|guild| guild.guild_id)
        .collect();
    ids.extend(
        ctx.db
            .game_guild_projection()
            .iter()
            .map(|guild| guild.guild_id),
    );
    for id in ids {
        publish(ctx, BTreeSet::from([id]));
    }
    Ok(())
}

pub(super) fn has_local_guilds(ctx: &ReducerContext) -> bool {
    ctx.db.game_guild().iter().any(|guild| {
        ctx.db
            .game_guild_projection()
            .guild_id()
            .find(guild.guild_id)
            .is_none()
    })
}

fn same_import(left: &Snapshot, right: &Snapshot) -> bool {
    let ranks = |state: &Snapshot| {
        let mut ranks: Vec<_> = state
            .ranks
            .iter()
            .map(|rank| (rank.rank_id, rank.name.clone(), rank.rights))
            .collect();
        ranks.sort();
        ranks
    };
    let members = |state: &Snapshot| {
        let mut members: Vec<_> = state
            .members
            .iter()
            .map(|member| (member.character_guid, encode(member)))
            .collect();
        members.sort();
        members
    };
    encode(&left.guild) == encode(&right.guild)
        && ranks(left) == ranks(right)
        && members(left) == members(right)
}

fn gm_refusal(cause: impl Into<String>) -> Rollback {
    let cause = cause.into();
    Rollback::reply(
        ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(codec::build_gm_system_message(
            cause.clone(),
        ))),
        cause,
    )
}

pub(super) fn dot_command(
    ctx: &ReducerContext,
    actor: &crate::WorldEntity,
    text: &str,
) -> Result<(), Rollback> {
    let realm = realm_shard(ctx).ok_or("World Shard has no Realm Shard")?;
    if has_local_guilds(ctx) {
        return Err(gm_refusal("local guild state requires migration"));
    }
    let (leader, name) = super::guild::parse_create(text)
        .ok_or_else(|| gm_refusal("usage: .guild create [<leader>] \"<name>\""))?;
    let character = realm_party::facts(ctx, actor.guid, true).ok_or("Character is not in world")?;
    ctx.db.game_realm_guild_command().insert(RealmGuildCommand {
        id: 0,
        database: remote::Identity(realm),
        facts: encode(&facts(ctx, character).ok_or("Character is not owned here")?),
        opcode: CREATE_FOR_LEADER,
        body: encode(&(
            leader.map(str::to_owned),
            actor.target_guid,
            name.to_string(),
        )),
    });
    Ok(())
}

fn create(ctx: &ReducerContext, actor: &super::guild::Actor, body: &[u8]) -> Result<u64, Rollback> {
    use super::directory::game_character_profile;
    let gm_level = crate::auth::gm_level(ctx, actor.account_id);
    if gm_level == 0 {
        return Err(gm_refusal("permission denied"));
    }
    let (name, target, guild_name): (Option<String>, u64, String) =
        bsatn::from_slice(body).map_err(|error| error.to_string())?;
    let profiles = ctx.db.game_character_profile();
    let leader = if let Some(name) = name {
        let mut found = profiles
            .iter()
            .filter(|profile| !profile.deleted && profile.name.eq_ignore_ascii_case(&name));
        let leader = found
            .next()
            .ok_or_else(|| gm_refusal(format!("no player named {name}")))?;
        if found.next().is_some() {
            return Err(gm_refusal("Character name is ambiguous"));
        }
        Some(super::guild::Actor {
            guid: leader.guid,
            account_id: leader.account_id,
            name: leader.name,
            team: team_for_race(leader.race),
        })
    } else {
        profiles
            .guid()
            .find(target)
            .filter(|profile| !profile.deleted)
            .map(|leader| super::guild::Actor {
                guid: leader.guid,
                account_id: leader.account_id,
                name: leader.name,
                team: team_for_race(leader.race),
            })
    };
    let leader = leader.as_ref().unwrap_or(actor);
    let guid = leader.guid;
    guild::apply_op(
        ctx,
        actor.guid,
        actor.account_id,
        guild::GuildOp::GmCreate(guild::GuildGmCreate {
            leader_guid: guid,
            leader_name: leader.name.clone(),
            leader_team: leader.team,
            leader_realm_account: leader.account_id,
            gm_level,
            name: guild_name,
        }),
    )
    .map_err(|refusal| gm_refusal(refusal.as_tag()))?;
    Ok(guid)
}

pub(crate) fn member_changed(ctx: &ReducerContext, guid: u64) {
    if world_shard(ctx).is_none() {
        return;
    }
    let records = ctx.db.game_guild_membership_projection();
    let member = guild::member(ctx, guid);
    let guild_id = member
        .as_ref()
        .map(|member| member.guild_id)
        .or_else(|| records.guid().find(guid).map(|record| record.guild_id))
        .unwrap_or(0);
    records.guid().delete(guid);
    records.insert(GuildMembershipProjection {
        guid,
        revision: directory::revision(ctx),
        guild_id,
        member,
    });
}

fn project_member(ctx: &ReducerContext, record: GuildMembershipProjection) {
    let records = ctx.db.game_guild_membership_projection();
    if records
        .guid()
        .find(record.guid)
        .is_some_and(|old| old.revision >= record.revision)
    {
        return;
    }
    let guid = record.guid;
    records.guid().delete(guid);
    records.insert(record);
    let record = records
        .guid()
        .find(guid)
        .expect("membership revision was inserted");
    match record.member {
        Some(member) if guild::member(ctx, guid).is_some() => {
            member.update(ctx);
        }
        Some(member) => {
            member.insert(ctx);
        }
        None => GuildMember::delete(ctx, guid),
    }
}
