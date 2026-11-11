use std::collections::BTreeMap;

use lyracore_protocol::{
    codec,
    messages::vanilla::{
        opcodes::ServerOpcodeMessage, SMSG_CHAT_PLAYER_NOT_FOUND, SMSG_NOTIFICATION,
    },
    who::WhoFilter,
    world::Packet,
};
use lyracore_shared::chat::chat_kind;
use spacetimedb::{
    reducer, sats::bsatn, table, ConnectionId, Identity, ReducerContext, ScheduleAt, SpacetimeType,
    Table, TimeDuration,
};

use super::{
    directory::{self, game_character_directory},
    game_native_player, game_native_world_connection,
    realm_shard::game_native_away,
    NativeActive, NativeWorldState, Rollback,
};
use crate::{
    away::game_character_away,
    peer::{realm_shard, require_sender},
    realm_chat::{SpeakerFacts, WhisperRequest, WhisperTargetFacts},
    remote_bindings::peer as remote,
};

const REQUEST_LIFETIME_MICROS: i64 = 15_000_000;
const DELIVERY_RETRY_MICROS: i64 = 250_000;

#[derive(SpacetimeType, Clone)]
pub struct Participant {
    pub(super) account_id: u64,
    pub(super) connection: u128,
    pub(super) guid: u64,
}

#[derive(SpacetimeType, Clone)]
pub struct Whisper {
    pub speaker: SpeakerFacts,
    pub language: u32,
    pub message: String,
    pub target_name: String,
}

#[derive(SpacetimeType, Clone)]
pub struct Who {
    team: u32,
    min_level: u8,
    max_level: u8,
    name: String,
    guild: String,
    races: u32,
    classes: u32,
    zones: Vec<u32>,
    search: Vec<String>,
}

impl Who {
    fn filter(&self) -> WhoFilter<'_> {
        WhoFilter {
            requester_team: self.team,
            min_level: self.min_level,
            max_level: self.max_level,
            name: &self.name,
            guild_name: &self.guild,
            race_mask: self.races,
            class_mask: self.classes,
            zones: &self.zones,
            search_strings: &self.search,
        }
    }
}

#[derive(SpacetimeType, Clone)]
pub enum Command {
    Who(Who),
    Whisper(Whisper),
}

#[derive(SpacetimeType, Clone)]
pub struct Request {
    actor: Participant,
    command: Command,
}

#[derive(SpacetimeType, Clone)]
pub struct OnlineCharacter {
    participant: Participant,
    name: String,
    guild: String,
    race: u8,
    class: u8,
    level: u8,
    zone: u32,
    ignores_speaker: bool,
    away_kind: u8,
    away_message: String,
}

fn encode<T: spacetimedb::Serialize>(value: &T) -> Vec<u8> {
    bsatn::to_vec(value).expect("Realm communication data encodes")
}

fn local_participant(ctx: &ReducerContext, connection: ConnectionId) -> Option<Participant> {
    ctx.db
        .game_native_player()
        .connection_id()
        .find(connection)
        .map(|player| Participant {
            account_id: player.account_id,
            connection: connection.to_u128(),
            guid: player.character_guid,
        })
}

fn current_world(ctx: &ReducerContext, participant: &Participant) -> Option<Identity> {
    let away = ctx
        .db
        .game_native_away()
        .account_id()
        .find(participant.account_id)?;
    if away.connection_id.to_u128() != participant.connection
        || away.character_guid != participant.guid
    {
        return None;
    }
    let entry = ctx
        .db
        .game_character_directory()
        .guid()
        .find(participant.guid)?;
    (entry.account_id == participant.account_id && away.world == Some(entry.world))
        .then_some(entry.world)
}

pub(super) fn who(
    ctx: &ReducerContext,
    connection: ConnectionId,
    filter: &WhoFilter<'_>,
) -> Result<bool, Rollback> {
    submit(
        ctx,
        connection,
        Command::Who(Who {
            team: filter.requester_team,
            min_level: filter.min_level,
            max_level: filter.max_level,
            name: filter.name.to_string(),
            guild: filter.guild_name.to_string(),
            races: filter.race_mask,
            classes: filter.class_mask,
            zones: filter.zones.to_vec(),
            search: filter.search_strings.to_vec(),
        }),
    )
}

pub(super) fn whisper(
    ctx: &ReducerContext,
    connection: ConnectionId,
    request: Whisper,
) -> Result<bool, Rollback> {
    submit(ctx, connection, Command::Whisper(request))
}

fn submit(
    ctx: &ReducerContext,
    connection: ConnectionId,
    command: Command,
) -> Result<bool, Rollback> {
    let Some(realm) = realm_shard(ctx) else {
        return Ok(false);
    };
    let actor = local_participant(ctx, connection).ok_or("Character is not in the world")?;
    ctx.db
        .game_realm_communication_request()
        .insert(RealmCommunicationRequest {
            id: 0,
            database: remote::Identity(realm),
            request: encode(&Request { actor, command }),
        });
    Ok(true)
}

#[table(accessor = game_realm_communication_request, outbox(remote::idc_realm_communication))]
pub struct RealmCommunicationRequest {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub request: Vec<u8>,
}

#[table(accessor = game_realm_query, scheduled(expire_realm_query))]
pub struct RealmQuery {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub scheduled_at: ScheduleAt,
    pub request: Request,
    pub remaining: Vec<Identity>,
    pub answers: Vec<WorldAnswer>,
}
crate::virtual_clock_exempt!(game_realm_query);

#[derive(SpacetimeType, Clone)]
pub struct WorldAnswer {
    world: Identity,
    character: OnlineCharacter,
}

#[table(accessor = game_world_query, outbox(remote::idc_world_query))]
pub struct WorldQuery {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub query_id: u64,
    pub request: Vec<u8>,
}

#[table(accessor = game_world_query_answer, outbox(remote::idc_world_query_answer))]
pub struct WorldQueryAnswer {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub query_id: u64,
    pub characters: Vec<u8>,
}

#[reducer]
pub fn idc_realm_communication(ctx: &ReducerContext, request: Vec<u8>) -> Result<(), String> {
    directory::require_world(ctx)?;
    let request: Request = bsatn::from_slice(&request).map_err(|error| error.to_string())?;
    if current_world(ctx, &request.actor) != Some(ctx.sender()) {
        return Err("World Shard no longer holds this Character's World Session".into());
    }
    let worlds = directory::worlds(ctx);
    let query = ctx.db.game_realm_query().insert(RealmQuery {
        id: 0,
        scheduled_at: (ctx.timestamp + TimeDuration::from_micros(REQUEST_LIFETIME_MICROS)).into(),
        request,
        remaining: worlds.into_iter().collect(),
        answers: Vec::new(),
    });
    for world in &query.remaining {
        ctx.db.game_world_query().insert(WorldQuery {
            id: 0,
            database: remote::Identity(*world),
            query_id: query.id,
            request: encode(&query.request),
        });
    }
    Ok(())
}

fn candidate(
    ctx: &ReducerContext,
    guid: u64,
    view: codec::WhoPlayerView,
) -> Option<OnlineCharacter> {
    let player = ctx.db.game_native_player().character_guid().find(guid)?;
    if super::transfer::pending(ctx, player.connection_id) {
        return None;
    }
    Some(OnlineCharacter {
        participant: Participant {
            account_id: player.account_id,
            connection: player.connection_id.to_u128(),
            guid,
        },
        name: view.name,
        guild: view.guild,
        race: view.race,
        class: view.class,
        level: view.level,
        zone: view.zone_id,
        ignores_speaker: false,
        away_kind: 0,
        away_message: String::new(),
    })
}

#[reducer]
pub fn idc_world_query(
    ctx: &ReducerContext,
    query_id: u64,
    request: Vec<u8>,
) -> Result<(), String> {
    require_sender(ctx, realm_shard(ctx))?;
    let request: Request = bsatn::from_slice(&request).map_err(|error| error.to_string())?;
    let characters = match request.command {
        Command::Who(query) => super::social::who_matches(ctx, &query.filter())
            .into_iter()
            .filter_map(|(guid, view)| candidate(ctx, guid, view))
            .collect::<Vec<_>>(),
        Command::Whisper(query) => crate::helpers::character_by_name(ctx, &query.target_name)
            .and_then(|character| {
                let mut target = candidate(
                    ctx,
                    character.guid,
                    codec::WhoPlayerView {
                        name: character.name,
                        guild: String::new(),
                        race: character.race,
                        class: character.class,
                        level: character.level,
                        zone_id: character.zone_id,
                    },
                )?;
                target.ignores_speaker =
                    super::chat::ignores(ctx, character.guid, request.actor.guid);
                if let Some(away) = ctx
                    .db
                    .game_character_away()
                    .character_guid()
                    .find(character.guid)
                {
                    target.away_kind = away.kind;
                    target.away_message = away.message;
                }
                Some(target)
            })
            .into_iter()
            .collect(),
    };
    ctx.db.game_world_query_answer().insert(WorldQueryAnswer {
        id: 0,
        database: remote::Identity(ctx.sender()),
        query_id,
        characters: encode(&characters),
    });
    Ok(())
}

#[reducer]
pub fn idc_world_query_answer(
    ctx: &ReducerContext,
    query_id: u64,
    characters: Vec<u8>,
) -> Result<(), String> {
    directory::require_world(ctx)?;
    let queries = ctx.db.game_realm_query();
    let Some(mut query) = queries.id().find(query_id) else {
        return Ok(());
    };
    if !query.remaining.contains(&ctx.sender()) {
        return Ok(());
    }
    let characters: Vec<OnlineCharacter> =
        bsatn::from_slice(&characters).map_err(|error| error.to_string())?;
    query
        .answers
        .extend(characters.into_iter().map(|character| WorldAnswer {
            world: ctx.sender(),
            character,
        }));
    query.remaining.retain(|world| *world != ctx.sender());
    if query.remaining.is_empty() {
        queries.id().delete(query.id);
        finish(ctx, query);
    } else {
        queries.id().update(query);
    }
    Ok(())
}

fn finish(ctx: &ReducerContext, query: RealmQuery) {
    let actor = query.request.actor;
    if current_world(ctx, &actor).is_none() {
        return;
    }
    let mut characters = BTreeMap::new();
    for WorldAnswer { world, character } in query.answers {
        if current_world(ctx, &character.participant) == Some(world) {
            characters.insert(character.participant.guid, character);
        }
    }
    match query.request.command {
        Command::Who(_) => {
            let views = characters
                .into_values()
                .map(|character| codec::WhoPlayerView {
                    name: character.name,
                    guild: character.guild,
                    race: character.race,
                    class: character.class,
                    level: character.level,
                    zone_id: character.zone,
                })
                .collect::<Vec<_>>();
            let (opcode, body) = codec::build_who_response_raw(&views);
            deliver(ctx, actor, vec![(opcode, body)], None);
        }
        Command::Whisper(request) => {
            if characters.len() != 1 {
                let message = if characters.is_empty() {
                    ServerOpcodeMessage::SMSG_CHAT_PLAYER_NOT_FOUND(Box::new(
                        SMSG_CHAT_PLAYER_NOT_FOUND {
                            name: request.target_name,
                        },
                    ))
                } else {
                    unavailable()
                };
                deliver_message(ctx, actor, message);
                return;
            }
            let target = characters.into_values().next().expect("one whisper target");
            let lines = crate::realm_chat::whisper_lines(
                actor.guid,
                WhisperRequest {
                    language: request.language,
                    message: request.message,
                    speaker: request.speaker,
                    target: WhisperTargetFacts {
                        guid: target.participant.guid,
                        race: target.race,
                        name: target.name,
                        ignores_speaker: target.ignores_speaker,
                        away_kind: target.away_kind,
                        away_message: target.away_message,
                    },
                },
            );
            match lines {
                Ok(lines) => {
                    let mut replies = Vec::new();
                    for line in lines {
                        if let Some(message) = codec::build_realm_chat_line(
                            line.kind,
                            line.speaker_guid,
                            line.language,
                            line.chat_tag,
                            line.channel_name.clone(),
                            line.message.clone(),
                        ) {
                            let packet =
                                packet(ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(message)));
                            if line.kind == chat_kind::WHISPER {
                                deliver(
                                    ctx,
                                    target.participant.clone(),
                                    vec![packet],
                                    Some(actor.guid),
                                );
                            } else {
                                replies.push(packet);
                            }
                        }
                        crate::realm_chat::emit(ctx, line);
                    }
                    deliver(ctx, actor, replies, None);
                }
                Err(refusal) => {
                    if let Some(Packet { opcode, body }) = super::chat::refusal_packet(
                        actor.guid,
                        chat_kind::WHISPER,
                        String::new(),
                        refusal,
                    ) {
                        deliver(ctx, actor, vec![(opcode, body)], None);
                    }
                }
            }
        }
    }
}

fn unavailable() -> ServerOpcodeMessage {
    ServerOpcodeMessage::SMSG_NOTIFICATION(Box::new(SMSG_NOTIFICATION {
        notification: "Unable to complete that request. Please try again.".into(),
    }))
}

fn packet(message: ServerOpcodeMessage) -> (u16, Vec<u8>) {
    let packet = lyracore_protocol::world::encode(message).expect("bounded social reply encodes");
    (packet.opcode, packet.body)
}

fn deliver_message(ctx: &ReducerContext, actor: Participant, message: ServerOpcodeMessage) {
    deliver(ctx, actor, vec![packet(message)], None);
}

fn fail_query(ctx: &ReducerContext, query: RealmQuery) {
    ctx.db.game_realm_query().id().delete(query.id);
    deliver_message(ctx, query.request.actor, unavailable());
}

#[reducer]
pub fn expire_realm_query(ctx: &ReducerContext, query: RealmQuery) -> Result<(), String> {
    if !crate::time::scheduler_authorized(ctx) {
        return Err("scheduler only".into());
    }
    fail_query(ctx, query);
    Ok(())
}

#[reducer(on_result(game_world_query))]
pub fn world_query_answered(ctx: &ReducerContext, call: WorldQuery, result: Result<(), String>) {
    if result.is_err() {
        if let Some(query) = ctx.db.game_realm_query().id().find(call.query_id) {
            fail_query(ctx, query);
        }
    }
}

#[reducer(on_result(game_realm_communication_request))]
pub fn realm_communication_answered(
    ctx: &ReducerContext,
    call: RealmCommunicationRequest,
    result: Result<(), String>,
) -> Result<(), String> {
    if result.is_err() {
        let request: Request =
            bsatn::from_slice(&call.request).map_err(|error| error.to_string())?;
        let connection = ConnectionId::from_u128(request.actor.connection);
        if local_participant(ctx, connection).is_some_and(|actor| actor.guid == request.actor.guid)
        {
            super::send(ctx, connection, unavailable(), false)?;
        }
    }
    Ok(())
}

#[derive(SpacetimeType, Clone)]
pub struct ReplyPacket {
    opcode: u16,
    body: Vec<u8>,
}

#[derive(SpacetimeType, Clone)]
pub struct Delivery {
    recipient: Participant,
    packets: Vec<ReplyPacket>,
    ignores: Option<u64>,
    expires_micros: i64,
}

#[table(accessor = game_realm_delivery, scheduled(expire_realm_delivery))]
pub struct RealmDelivery {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub scheduled_at: ScheduleAt,
    pub delivery: Delivery,
}
crate::virtual_clock_exempt!(game_realm_delivery);

#[table(accessor = game_world_delivery, outbox(remote::idc_world_delivery))]
pub struct WorldDelivery {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub delivery_id: u64,
    pub delivery: Vec<u8>,
}

#[table(accessor = game_realm_delivery_retry, scheduled(retry_realm_delivery))]
pub struct RealmDeliveryRetry {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub scheduled_at: ScheduleAt,
    pub delivery_id: u64,
}
crate::virtual_clock_exempt!(game_realm_delivery_retry);

pub(super) fn deliver(
    ctx: &ReducerContext,
    recipient: Participant,
    packets: Vec<(u16, Vec<u8>)>,
    ignores: Option<u64>,
) {
    if packets.is_empty() || current_world(ctx, &recipient).is_none() {
        return;
    }
    let pending = ctx.db.game_realm_delivery().insert(RealmDelivery {
        id: 0,
        scheduled_at: (ctx.timestamp + TimeDuration::from_micros(REQUEST_LIFETIME_MICROS)).into(),
        delivery: Delivery {
            recipient,
            packets: packets
                .into_iter()
                .map(|(opcode, body)| ReplyPacket { opcode, body })
                .collect(),
            ignores,
            expires_micros: ctx.timestamp.to_micros_since_unix_epoch() + REQUEST_LIFETIME_MICROS,
        },
    });
    send_delivery(ctx, pending);
}

fn send_delivery(ctx: &ReducerContext, pending: RealmDelivery) {
    let world = current_world(ctx, &pending.delivery.recipient);
    if let Some(world) = world
        .filter(|_| ctx.timestamp.to_micros_since_unix_epoch() < pending.delivery.expires_micros)
    {
        ctx.db.game_world_delivery().insert(WorldDelivery {
            id: 0,
            database: remote::Identity(world),
            delivery_id: pending.id,
            delivery: encode(&pending.delivery),
        });
    } else {
        ctx.db.game_realm_delivery().id().delete(pending.id);
    }
}

#[reducer]
pub fn idc_world_delivery(
    ctx: &ReducerContext,
    delivery_id: u64,
    delivery: Vec<u8>,
) -> Result<(), String> {
    require_sender(ctx, realm_shard(ctx))?;
    let _ = delivery_id;
    let delivery: Delivery = bsatn::from_slice(&delivery).map_err(|error| error.to_string())?;
    if ctx.timestamp.to_micros_since_unix_epoch() >= delivery.expires_micros {
        return Ok(());
    }
    let recipient = delivery.recipient;
    let connection_id = ConnectionId::from_u128(recipient.connection);
    let connection = ctx
        .db
        .game_native_world_connection()
        .connection_id()
        .find(connection_id)
        .ok_or("recipient World Session is not on this World Shard")?;
    if !matches!(connection.state, NativeWorldState::InWorld(NativeActive { account_id, character_guid })
        if account_id == recipient.account_id && character_guid == recipient.guid)
        || super::transfer::pending(ctx, connection_id)
        || !local_participant(ctx, connection_id).is_some_and(|actor| actor.guid == recipient.guid)
    {
        return Err("recipient World Session is not ready".into());
    }
    super::current_account(ctx, &connection, recipient.account_id)?;
    if delivery
        .ignores
        .is_some_and(|speaker| super::chat::ignores(ctx, recipient.guid, speaker))
    {
        return Ok(());
    }
    for ReplyPacket { opcode, body } in delivery.packets {
        super::send_raw(ctx, connection_id, opcode, body, false);
    }
    Ok(())
}

#[reducer(on_result(game_world_delivery))]
pub fn world_delivery_answered(
    ctx: &ReducerContext,
    call: WorldDelivery,
    result: Result<(), String>,
) {
    let pending = ctx.db.game_realm_delivery();
    if result.is_ok() {
        pending.id().delete(call.delivery_id);
    } else if pending.id().find(call.delivery_id).is_some() {
        // Retry only a refused call; an in-flight IDC may already have delivered its packets.
        ctx.db
            .game_realm_delivery_retry()
            .insert(RealmDeliveryRetry {
                id: 0,
                scheduled_at: (ctx.timestamp + TimeDuration::from_micros(DELIVERY_RETRY_MICROS))
                    .into(),
                delivery_id: call.delivery_id,
            });
    }
}

#[reducer]
pub fn retry_realm_delivery(ctx: &ReducerContext, retry: RealmDeliveryRetry) -> Result<(), String> {
    if !crate::time::scheduler_authorized(ctx) {
        return Err("scheduler only".into());
    }
    if let Some(pending) = ctx.db.game_realm_delivery().id().find(retry.delivery_id) {
        send_delivery(ctx, pending);
    }
    Ok(())
}

#[reducer]
pub fn expire_realm_delivery(ctx: &ReducerContext, pending: RealmDelivery) -> Result<(), String> {
    if !crate::time::scheduler_authorized(ctx) {
        return Err("scheduler only".into());
    }
    ctx.db.game_realm_delivery().id().delete(pending.id);
    Ok(())
}
