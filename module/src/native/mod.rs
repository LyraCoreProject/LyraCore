use lyracore_protocol::codec;

mod addon;
pub(crate) mod admission;
mod auction;
mod aura;
mod book;
mod cast;
mod channel;
mod character;
mod character_names;
pub(crate) use character_names::account_has_room as realm_account_has_room;
mod chat;
mod chat_flood;
mod combat;
mod creature;
mod death;
#[cfg(feature = "conformance_test_fixture")]
mod debug;
pub(crate) mod realm_auction;
#[cfg(feature = "conformance_test_fixture")]
pub(crate) use debug::record_birth as record_conformance_birth;
pub(crate) mod directory;
mod duel;
mod economy;
mod environment;
mod gossip;
mod guild;
mod handoff;
mod realm_communication;
mod realm_guild;
pub(crate) mod realm_mail;
pub(crate) mod realm_party;
pub(crate) mod realm_petition;
mod realm_shard;
pub(crate) use realm_shard::account_changed;
mod instance;
pub(crate) mod instance_allocation;
mod instance_reset;
pub(crate) mod instance_shard;
pub(crate) use instance_shard::{
    sweep_delete_game_instance_entry, sweep_transfer_game_instance_entry,
};
mod inventory;
pub(crate) mod logon;
mod loot;
mod mail;
mod meeting_stone;
pub(crate) mod member_stats;
mod motion_permit;
mod object;
mod package_actor;
pub(crate) mod package_creation;
pub(crate) mod package_guild;
pub(crate) mod package_relocation;
mod party;
mod pet;
mod petition;
mod progression;
mod query;
mod quest;
mod reputation;
mod rollback;
mod social;
mod spell;
mod taxi;
mod teleport;
mod trade;
mod trainer;
mod transfer;
mod transport;
mod unavailable;
mod view;
mod visibility;
mod world;
pub(crate) use addon::appeared as addon_message;
pub(crate) use auction::notice as auction_notice;
pub(crate) use aura::changed as aura_changed;
pub(crate) use book::{
    forgotten as spell_forgotten, learned as spell_learned, skill_changed, talent_points,
};
pub(crate) use channel::notice as channel_notice;
pub(crate) use chat::{creature_sound, realm_chat_line, speech, system_message, text_emote};
pub(crate) use combat::{combat_event, combo_points_changed, engagement_changed};
pub(crate) use creature::{
    changed as creature_spline_changed, equipment_changed as creature_equipment_changed,
};
pub(crate) use death::{
    corpse_changed, corpse_created, corpse_removed, resurrect_offered, self_resurrection_changed,
};
pub(crate) use duel::event as duel_event;
pub(crate) use economy::{
    buyback_destroyed, buyback_removed, purchase_started as vendor_purchase_started,
};
pub(crate) use environment::{breath_event, fall_damage, weather_changed};
pub(crate) use guild::{event as guild_event, membership_changed as guild_membership_changed};
pub(crate) use instance::removal_changed as instance_removal_changed;
pub(crate) use inventory::changed as item_changed;
pub(crate) use inventory::item_equip_cooldown;
pub(crate) use inventory::temporary_enchantment_changed;
pub(crate) use inventory::{item_purchase_started, item_purchased, item_restored, item_sold};
pub(crate) use mail::arrived as mail_arrived;
pub(crate) use member_stats::{
    sweep_delete_game_native_member_stats, sweep_transfer_game_native_member_stats,
};
pub(crate) use motion_permit::{
    refresh_character as refresh_motion_permit, sweep_delete_game_native_motion_permit,
    sweep_transfer_game_native_motion_permit,
};
pub(crate) use object::changed as gameobject_changed;
pub(crate) use party::roster as party_roster;
pub(crate) use pet::changed as hunter_pet_changed;
pub(crate) use petition::charter_opened as guild_charter_opened;
pub(crate) use progression::{explored, levelup, xp};
pub(crate) use query::{broadcast_creature_query, broadcast_item_query};
pub(crate) use quest::{changed as quest_changed, event_credit as quest_event_credit};
pub(crate) use realm_guild::member_changed as guild_member_changed;
pub(crate) use reputation::{changed as reputation_changed, watched as watched_faction};
pub(crate) use spell::{cast_event, spell_cooldown, spell_impact};
pub(crate) use taxi::{
    aborted as taxi_aborted, landed as taxi_landed, resumed as taxi_resumed,
    spline_changed as taxi_spline_changed,
};
pub(crate) use teleport::{
    begin as begin_teleport, changed as teleport_changed, expire_near_teleports_pass,
};
pub(crate) use trade::event as trade_event;
pub(crate) use visibility::{
    entity_changed, grant_added, grant_removed, loot_flags, motion, with_shared_creates,
};

pub(crate) fn is_world_porting(ctx: &ReducerContext, guid: u64) -> bool {
    world_session(ctx, guid)
        .is_some_and(|player| player.teleport == Some(teleport::NativeTeleport::WorldPort))
}

pub(crate) fn has_world_session(ctx: &ReducerContext, guid: u64) -> bool {
    world_session(ctx, guid).is_some()
}

pub(crate) fn is_teleporting(ctx: &ReducerContext, guid: u64) -> bool {
    world_session(ctx, guid).is_some_and(|player| player.teleport.is_some())
}

pub(crate) fn world_session_characters(ctx: &ReducerContext) -> Vec<u64> {
    ctx.db
        .game_native_player()
        .iter()
        .map(|player| player.character_guid)
        .collect()
}

pub(crate) fn end_world_session(ctx: &ReducerContext, guid: u64) {
    if let Some(player) = world_session(ctx, guid) {
        disconnect(ctx, player.connection_id);
    }
}

fn world_session(ctx: &ReducerContext, guid: u64) -> Option<NativePlayer> {
    ctx.db.game_native_player().character_guid().find(guid)
}

#[derive(SpacetimeType)]
pub enum NativeAudience {
    Connection(ConnectionId),
    Aoi(NativeAoi),
    Grant(NativeRecipient),
    Revoke(NativeRecipient),
    Clear(ConnectionId),
    Handoff(handoff::NativeHandoffTarget),
}
#[derive(SpacetimeType)]
pub struct NativeAoi {
    pub layer: u8,
    pub entity_guid: u64,
    pub exclude_connection: Option<ConnectionId>,
}

#[derive(SpacetimeType)]
pub struct NativeRecipient {
    pub connection_id: ConnectionId,
    pub layer: u8,
    pub entity_guid: u64,
}

#[derive(SpacetimeType, Clone, Copy)]
pub struct NativeActive {
    pub account_id: u64,
    pub character_guid: u64,
}

use lyracore_protocol::messages::vanilla::{
    opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
    SMSG_AUTH_CHALLENGE, SMSG_AUTH_RESPONSE, SMSG_PONG, SMSG_QUERY_TIME_RESPONSE,
};
use lyracore_protocol::world::Packet;
use spacetimedb::{reducer, table, ConnectionId, ReducerContext, SpacetimeType, Table};

use crate::{game_account, game_character, game_session, Refusal};
use rollback::Rollback;
use transfer::game_native_transfer_pending;

#[derive(SpacetimeType)]
pub enum NativeWorldState {
    AwaitingProof,
    Queued { account_id: u64 },
    CharacterSelection { account_id: u64 },
    InWorld(NativeActive),
}

#[table(accessor = game_native_world_connection)]
pub struct NativeWorldConnection {
    #[primary_key]
    pub connection_id: ConnectionId,
    pub server_seed: u32,
    pub state: NativeWorldState,
    pub unavailable_notices: u8,
    #[default(None::<chat_flood::ChatFlood>)]
    pub chat_flood: Option<chat_flood::ChatFlood>,
    #[default(0i64)]
    pub who_until_micros: i64,
    #[default(None::<party::BroadcastCooldowns>)]
    pub group_broadcasts: Option<party::BroadcastCooldowns>,
    #[default(None::<addon::AddonLimit>)]
    pub addon_limit: Option<addon::AddonLimit>,
}

#[table(accessor = game_native_player,
    index(accessor = by_open_loot, btree(columns = [open_loot])),
    index(accessor = by_zone, btree(columns = [zone_id]))
)]
pub struct NativePlayer {
    #[primary_key]
    pub connection_id: ConnectionId,
    #[unique]
    pub character_guid: u64,
    #[unique]
    pub account_id: u64,
    pub open_loot: u64,
    pub gossip_menu: Option<gossip::NativeGossipMenu>,
    pub teleport: Option<teleport::NativeTeleport>,
    pub teleport_counter: u32,
    #[default(0u32)]
    pub zone_id: u32,
    #[default(0u32)]
    pub gossip_root_menu_id: u32,
    #[default(0i64)]
    pub teleport_started_micros: i64,
    #[default(None::<teleport::NativeTeleportOrigin>)]
    pub teleport_origin: Option<teleport::NativeTeleportOrigin>,
}

pub(crate) fn group_event(
    ctx: &ReducerContext,
    recipient: u64,
    kind: u8,
    other: u64,
    payload: &str,
) {
    party::group_event(ctx, recipient, kind, other, payload);
    death::group_event(ctx, recipient, kind);
    quest::group_event(ctx, recipient, kind, other, payload);
    loot::group_event(ctx, recipient, kind, other, payload);
}

crate::character_owned!(delete, fn sweep_delete_game_native_player(ctx, character_guid) {
    end_world_session(ctx, character_guid);
});

// A World Session belongs to its native connection.
crate::character_owned!(not_transported, fn sweep_transfer_game_native_player());

#[table(accessor = game_native_cipher)]
pub struct NativeCipher {
    #[primary_key]
    pub connection_id: ConnectionId,
    pub username: String,
    pub session_key: Vec<u8>,
    pub client_seed: u32,
    pub client_proof: Vec<u8>,
}

#[table(accessor = game_native_packet, event)]
pub struct NativePacket {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub audience: NativeAudience,
    pub opcode: Option<u16>,
    pub body: Vec<u8>,
    pub close_after: bool,
    pub not_before_micros: i64,
    pub authentication_complete: bool,
}

fn connection_id(ctx: &ReducerContext) -> Result<ConnectionId, String> {
    crate::helpers::require_operator(ctx)?;
    ctx.connection_id()
        .ok_or_else(|| "native request requires a connection".into())
}

fn send(
    ctx: &ReducerContext,
    connection_id: ConnectionId,
    message: ServerOpcodeMessage,
    close_after: bool,
) -> Result<(), String> {
    let packet = lyracore_protocol::world::encode(message).map_err(|error| error.to_string())?;
    send_raw(ctx, connection_id, packet.opcode, packet.body, close_after);
    Ok(())
}

fn send_raw(
    ctx: &ReducerContext,
    connection_id: ConnectionId,
    opcode: u16,
    body: Vec<u8>,
    close_after: bool,
) {
    ctx.db.game_native_packet().insert(NativePacket {
        id: 0,
        audience: NativeAudience::Connection(connection_id),
        opcode: Some(opcode),
        body,
        close_after,
        not_before_micros: 0,
        authentication_complete: false,
    });
}

/// Encodes a server message whose size the Module bounds; a broken bound panics.
fn bounded(message: ServerOpcodeMessage) -> Packet {
    lyracore_protocol::world::encode(message).expect("bounded server message fits a world frame")
}

/// What delivery does with a server message that does not fit a world frame.
#[derive(Clone, Copy)]
enum EncodeFailure {
    /// The Module bounds the message, so the failure is a broken invariant.
    Panic,
    /// Log it and end the World Session.
    Disconnect,
    /// Log it; the World Session continues without the message.
    Log,
}

/// Sends `message` to `guid`'s World Session; a Character without one on this Shard gets
/// nothing. Returns whether it had one.
fn send_to_character(
    ctx: &ReducerContext,
    guid: u64,
    message: ServerOpcodeMessage,
    on_failure: EncodeFailure,
) -> bool {
    let Some(player) = world_session(ctx, guid) else {
        return false;
    };
    let packet = match on_failure {
        EncodeFailure::Panic => bounded(message),
        EncodeFailure::Disconnect | EncodeFailure::Log => {
            match lyracore_protocol::world::encode(message) {
                Ok(packet) => packet,
                Err(error) => {
                    spacetimedb::log::error!("native message for {guid} does not encode: {error}");
                    if matches!(on_failure, EncodeFailure::Disconnect) {
                        disconnect(ctx, player.connection_id);
                    }
                    return true;
                }
            }
        }
    };
    send_raw(ctx, player.connection_id, packet.opcode, packet.body, false);
    true
}

fn send_raw_to_character(ctx: &ReducerContext, guid: u64, opcode: u16, body: Vec<u8>) -> bool {
    send_packets_to_character(ctx, guid, [Packet { opcode, body }])
}

fn send_packets_to_character(
    ctx: &ReducerContext,
    guid: u64,
    packets: impl IntoIterator<Item = Packet>,
) -> bool {
    let Some(player) = world_session(ctx, guid) else {
        return false;
    };
    for packet in packets {
        send_raw(ctx, player.connection_id, packet.opcode, packet.body, false);
    }
    true
}

pub(crate) fn disconnect(ctx: &ReducerContext, connection_id: ConnectionId) {
    end_connection(ctx, connection_id, ConnectionEnd::Disconnect);
}

pub(crate) fn close(ctx: &ReducerContext) {
    if let Some(connection_id) = ctx.connection_id() {
        end_connection(ctx, connection_id, ConnectionEnd::Closed);
    }
}

enum ConnectionEnd {
    /// The Module ends the connection and asks the Host to close it.
    Disconnect,
    /// The Host has closed the connection.
    Closed,
}

fn end_connection(ctx: &ReducerContext, connection_id: ConnectionId, end: ConnectionEnd) {
    realm_shard::closing(ctx, connection_id);
    realm_shard::clear_return(ctx, connection_id);
    realm_shard::cancel_selection(ctx, connection_id);
    #[cfg(feature = "conformance_test_fixture")]
    debug::close(ctx, connection_id);
    match end {
        ConnectionEnd::Disconnect => {
            ctx.db.game_native_packet().insert(NativePacket {
                id: 0,
                audience: NativeAudience::Connection(connection_id),
                opcode: None,
                body: Vec::new(),
                close_after: true,
                not_before_micros: 0,
                authentication_complete: false,
            });
        }
        ConnectionEnd::Closed => logon::close(ctx, connection_id),
    }
    admission::remove(ctx, connection_id);
    world::leave(ctx, connection_id, crate::world::CharacterDeparture::Logout);
    ctx.db
        .game_native_transfer_pending()
        .connection_id()
        .delete(connection_id);
    ctx.db
        .game_native_cipher()
        .connection_id()
        .delete(connection_id);
    ctx.db
        .game_native_world_connection()
        .connection_id()
        .delete(connection_id);
}

fn clear_audience(ctx: &ReducerContext, connection_id: ConnectionId) {
    ctx.db.game_native_packet().insert(NativePacket {
        id: 0,
        audience: NativeAudience::Clear(connection_id),
        opcode: None,
        body: Vec::new(),
        close_after: false,
        not_before_micros: 0,
        authentication_complete: false,
    });
}

#[derive(SpacetimeType)]
pub struct NativeReply {
    pub opcode: u16,
    pub body: Vec<u8>,
}

// A failed request rolls back before the host submits its Module-encoded reply.
#[reducer]
pub fn native_world_reply(ctx: &ReducerContext, packets: Vec<NativeReply>) -> Result<(), String> {
    let connection_id = connection_id(ctx)?;
    if ctx
        .db
        .game_native_world_connection()
        .connection_id()
        .find(connection_id)
        .is_none()
    {
        return Err("native connection is not open".into());
    }
    if packets.len() > 256 {
        return Err("too many native refusal replies".into());
    }
    for NativeReply { opcode, body } in packets {
        if body.len() > usize::from(u16::MAX) - 2 {
            return Err("native reply exceeds a world frame".into());
        }
        send_raw(ctx, connection_id, opcode, body, false);
    }
    Ok(())
}

#[reducer]
pub fn native_world_open(ctx: &ReducerContext, server_seed: u32) -> Result<(), String> {
    let connection_id = connection_id(ctx)?;
    let connections = ctx.db.game_native_world_connection();
    if connections.connection_id().find(connection_id).is_some() {
        return Err("native connection is already open".into());
    }
    connections.insert(NativeWorldConnection {
        connection_id,
        server_seed,
        state: NativeWorldState::AwaitingProof,
        unavailable_notices: 0,
        chat_flood: None,
        who_until_micros: 0,
        group_broadcasts: None,
        addon_limit: None,
    });
    send(
        ctx,
        connection_id,
        ServerOpcodeMessage::SMSG_AUTH_CHALLENGE(SMSG_AUTH_CHALLENGE { server_seed }),
        false,
    )
}

#[reducer]
pub fn native_world_receive(
    ctx: &ReducerContext,
    opcode: u32,
    body: Vec<u8>,
) -> Result<(), String> {
    let connection_id = connection_id(ctx)?;
    receive(ctx, connection_id, MotionRelay::Module, opcode, body)
        .map_err(Rollback::into_reducer_error)
}

/// Which tier sends observers a mover's movement.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MotionRelay {
    Module,
    Host,
}

/// Applies movement the Host already relayed under the connection's Motion Permit.
#[reducer]
pub fn native_world_motion(ctx: &ReducerContext, opcode: u32, body: Vec<u8>) -> Result<(), String> {
    let connection_id = connection_id(ctx)?;
    if !crate::motion::is_client_motion(opcode) {
        return Err(format!("native motion carries opcode 0x{opcode:03x}"));
    }
    receive(ctx, connection_id, MotionRelay::Host, opcode, body)
        .map_err(Rollback::into_reducer_error)
}

fn receive(
    ctx: &ReducerContext,
    connection_id: ConnectionId,
    relay: MotionRelay,
    opcode: u32,
    body: Vec<u8>,
) -> Result<(), Rollback> {
    let connection = ctx
        .db
        .game_native_world_connection()
        .connection_id()
        .find(connection_id)
        .ok_or("native connection is not open")?;
    let account_id = match &connection.state {
        NativeWorldState::AwaitingProof => None,
        NativeWorldState::Queued { account_id }
        | NativeWorldState::CharacterSelection { account_id }
        | NativeWorldState::InWorld(NativeActive { account_id, .. }) => Some(*account_id),
    };
    if let Some(account_id) = account_id {
        current_account(ctx, &connection, account_id)?;
    }
    if transfer::pending(ctx, connection_id)
        || (realm_shard::returning(ctx, connection_id) && opcode != codec::CMSG_PING_OPCODE)
    {
        return Ok(());
    }
    match connection.state {
        NativeWorldState::AwaitingProof => Ok(authenticate(ctx, connection, opcode, &body)?),
        NativeWorldState::CharacterSelection { account_id } => {
            select(ctx, connection, account_id, opcode, body)
        }
        NativeWorldState::Queued { .. } => {
            if unavailable::intercept(ctx, connection_id, opcode, &body)? {
                return Ok(());
            }
            let message = lyracore_protocol::world::decode(opcode, &body)
                .map_err(|error| error.to_string())?;
            if let ClientOpcodeMessage::CMSG_PING(ping) = message {
                Ok(send(
                    ctx,
                    connection_id,
                    ServerOpcodeMessage::SMSG_PONG(SMSG_PONG {
                        sequence_id: ping.sequence_id,
                    }),
                    false,
                )?)
            } else {
                Err("World Session is waiting in the Login Queue".into())
            }
        }
        NativeWorldState::InWorld(NativeActive { character_guid, .. }) => {
            if unavailable::intercept(ctx, connection_id, opcode, &body)? {
                return Ok(());
            }
            world::receive(ctx, connection, character_guid, relay, opcode, &body)
        }
    }
}

/// Character selection's routing table. World requests can remain in flight after logout, so it
/// ignores, without decoding, every opcode it does not list.
fn select(
    ctx: &ReducerContext,
    connection: NativeWorldConnection,
    account_id: u64,
    opcode: u32,
    body: Vec<u8>,
) -> Result<(), Rollback> {
    use codec::{
        CMSG_CHAR_CREATE_OPCODE, CMSG_CHAR_DELETE_OPCODE, CMSG_CHAR_ENUM_OPCODE,
        CMSG_CHAR_RENAME_OPCODE, CMSG_PING_OPCODE, CMSG_PLAYER_LOGIN_OPCODE,
        CMSG_QUERY_TIME_OPCODE,
    };
    let is_realm_shard = crate::peer::world_shard(ctx).is_some();
    match opcode {
        CMSG_PLAYER_LOGIN_OPCODE if is_realm_shard => {
            realm_shard::enter_world(ctx, connection, &body)
        }
        CMSG_PLAYER_LOGIN_OPCODE => world::login(ctx, connection, &body),
        CMSG_CHAR_CREATE_OPCODE | CMSG_CHAR_ENUM_OPCODE | CMSG_CHAR_DELETE_OPCODE
            if is_realm_shard =>
        {
            realm_shard::route_selection(ctx, connection.connection_id, account_id, opcode, body)
        }
        CMSG_CHAR_CREATE_OPCODE
        | CMSG_CHAR_ENUM_OPCODE
        | CMSG_CHAR_DELETE_OPCODE
        | CMSG_CHAR_RENAME_OPCODE
        | CMSG_PING_OPCODE
        | CMSG_QUERY_TIME_OPCODE => {
            let Some(reply) = selection_reply(ctx, account_id, opcode, &body)? else {
                return Ok(());
            };
            Ok(send(ctx, connection.connection_id, reply, false)?)
        }
        _ => Ok(()),
    }
}

/// The reply to a Character selection request, or None for a request selection ignores.
fn selection_reply(
    ctx: &ReducerContext,
    account_id: u64,
    opcode: u32,
    body: &[u8],
) -> Result<Option<ServerOpcodeMessage>, Rollback> {
    let message =
        lyracore_protocol::world::decode(opcode, body).map_err(|error| error.to_string())?;
    Ok(Some(match message {
        ClientOpcodeMessage::CMSG_PING(ping) => ServerOpcodeMessage::SMSG_PONG(SMSG_PONG {
            sequence_id: ping.sequence_id,
        }),
        ClientOpcodeMessage::CMSG_QUERY_TIME => {
            ServerOpcodeMessage::SMSG_QUERY_TIME_RESPONSE(SMSG_QUERY_TIME_RESPONSE {
                time: crate::time::now_secs(ctx) as u32,
            })
        }
        ClientOpcodeMessage::CMSG_CHAR_ENUM => {
            let characters = ctx
                .db
                .game_character()
                .by_account()
                .filter(account_id)
                .map(|character| view::character(ctx, character))
                .collect::<Vec<_>>();
            ServerOpcodeMessage::SMSG_CHAR_ENUM(Box::new(
                codec::build_char_enum(&characters).map_err(|error| error.to_string())?,
            ))
        }
        ClientOpcodeMessage::CMSG_CHAR_CREATE(request) => {
            character::create(ctx, account_id, *request)?
        }
        ClientOpcodeMessage::CMSG_CHAR_DELETE(request) => {
            character::delete(ctx, account_id, request.guid.guid())?
        }
        ClientOpcodeMessage::CMSG_CHAR_RENAME(_) => {
            ServerOpcodeMessage::SMSG_CHAR_RENAME(Box::new(codec::build_char_rename_refusal()))
        }
        _ => return Ok(None),
    }))
}

fn authenticate(
    ctx: &ReducerContext,
    connection: NativeWorldConnection,
    opcode: u32,
    body: &[u8],
) -> Result<(), String> {
    let frame =
        lyracore_protocol::world::client_frame(opcode, body).map_err(|error| error.to_string())?;
    let claim = codec::auth_session::read_auth_session(&mut frame.as_slice())
        .map_err(|error| error.to_string())?;
    if body.get(..4) != Some(5875u32.to_le_bytes().as_slice()) {
        return auth_failure(
            ctx,
            connection.connection_id,
            SMSG_AUTH_RESPONSE::AuthVersionMismatch,
        );
    }
    let Some(account) = ctx
        .db
        .game_account()
        .username()
        .find(claim.username.to_ascii_uppercase())
    else {
        return auth_failure(
            ctx,
            connection.connection_id,
            SMSG_AUTH_RESPONSE::AuthUnknownAccount,
        );
    };
    if account.banned {
        return auth_failure(
            ctx,
            connection.connection_id,
            SMSG_AUTH_RESPONSE::AuthBanned,
        );
    }
    let Some(session) = ctx.db.game_session().account_id().find(account.id) else {
        return auth_failure(
            ctx,
            connection.connection_id,
            SMSG_AUTH_RESPONSE::AuthFailed,
        );
    };
    let Ok(key) = session.session_key.as_slice().try_into() else {
        return auth_failure(
            ctx,
            connection.connection_id,
            SMSG_AUTH_RESPONSE::AuthFailed,
        );
    };
    if session.expires_at <= crate::time::now(ctx)
        || session.identity.is_none()
        || session.identity != account.identity
        || !lyracore_protocol::world::matches_world_proof(
            &account.username,
            key,
            connection.server_seed,
            claim.client_seed,
            &claim.client_proof,
        )
    {
        return auth_failure(
            ctx,
            connection.connection_id,
            SMSG_AUTH_RESPONSE::AuthFailed,
        );
    }
    ctx.db.game_native_cipher().insert(NativeCipher {
        connection_id: connection.connection_id,
        username: account.username,
        session_key: session.session_key,
        client_seed: claim.client_seed,
        client_proof: claim.client_proof.to_vec(),
    });
    let connection_id = connection.connection_id;
    ctx.db
        .game_native_world_connection()
        .connection_id()
        .update(NativeWorldConnection {
            state: NativeWorldState::Queued {
                account_id: account.id,
            },
            ..connection
        });
    admission::request(ctx, connection_id, account.id)?;
    send(
        ctx,
        connection_id,
        ServerOpcodeMessage::SMSG_ADDON_INFO(Box::new(codec::auth_session::build_addon_info(
            &claim.addon_info,
        ))),
        false,
    )
}

fn auth_failure(
    ctx: &ReducerContext,
    connection_id: ConnectionId,
    reply: SMSG_AUTH_RESPONSE,
) -> Result<(), String> {
    ctx.db
        .game_native_world_connection()
        .connection_id()
        .delete(connection_id);
    send(
        ctx,
        connection_id,
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(Box::new(reply)),
        true,
    )
}

fn current_account(
    ctx: &ReducerContext,
    connection: &NativeWorldConnection,
    account_id: u64,
) -> Result<crate::Account, String> {
    let account = ctx
        .db
        .game_account()
        .id()
        .find(account_id)
        .ok_or("Account no longer exists")?;
    #[cfg(feature = "conformance_test_fixture")]
    let account = {
        let mut account = account;
        if debug::account(ctx, connection.connection_id, &mut account)? {
            return Ok(account);
        }
        account
    };
    let session = ctx
        .db
        .game_session()
        .account_id()
        .find(account_id)
        .ok_or("Session no longer exists")?;
    let cipher = ctx
        .db
        .game_native_cipher()
        .connection_id()
        .find(connection.connection_id)
        .ok_or("world proof is missing")?;
    let key = session
        .session_key
        .as_slice()
        .try_into()
        .map_err(|_| "invalid Session key")?;
    let proof = cipher
        .client_proof
        .as_slice()
        .try_into()
        .map_err(|_| "invalid world proof")?;
    if account.banned
        || session.identity.is_none()
        || session.identity != account.identity
        || !lyracore_protocol::world::matches_world_proof(
            &account.username,
            key,
            connection.server_seed,
            cipher.client_seed,
            proof,
        )
    {
        return Err("authenticated Session is no longer current".into());
    }
    Ok(account)
}

// Login-proof expiry does not revoke an established World Session; see docs/architecture.md.
pub(crate) fn active_session_accounts(ctx: &ReducerContext) -> std::collections::HashSet<u64> {
    let mut active = local_session_accounts(ctx);
    active.extend(realm_shard::away_accounts(ctx));
    active
}

pub(crate) fn vendor_session_population(ctx: &ReducerContext) -> usize {
    let mut active = active_session_accounts(ctx);
    for connection in ctx.db.game_native_world_connection().iter() {
        if let NativeWorldState::Queued { account_id } = connection.state {
            active.remove(&account_id);
        }
    }
    active.len()
}

fn local_session_accounts(ctx: &ReducerContext) -> std::collections::HashSet<u64> {
    ctx.db
        .game_native_world_connection()
        .iter()
        .filter_map(|connection| {
            let account_id = match connection.state {
                NativeWorldState::AwaitingProof => return None,
                NativeWorldState::Queued { account_id }
                | NativeWorldState::CharacterSelection { account_id }
                | NativeWorldState::InWorld(NativeActive { account_id, .. }) => account_id,
            };
            current_account(ctx, &connection, account_id)
                .is_ok()
                .then_some(account_id)
        })
        .collect()
}

fn actor(ctx: &ReducerContext, guid: u64) -> Result<crate::WorldEntity, Refusal> {
    if is_teleporting(ctx, guid) {
        return Err("Character is teleporting".into());
    }
    let entity = available_entity(ctx, guid)?;
    crate::taxi::reject_action_while_in_flight(ctx, guid)?;
    Ok(entity)
}

fn available_entity(ctx: &ReducerContext, guid: u64) -> Result<crate::WorldEntity, Refusal> {
    crate::helpers::acting_entity_by_guid(ctx, guid)
        .ok_or_else(|| "Character is not available".into())
}

fn available_character(ctx: &ReducerContext, guid: u64) -> Result<crate::Character, Refusal> {
    crate::helpers::character_by_guid(ctx, guid).ok_or_else(|| "Character is not available".into())
}

#[cfg(test)]
mod tests {
    use spacetimedb::sats::bsatn;
    use spacetimedb::ConnectionId;

    use super::{
        handoff::NativeHandoffTarget, NativeAoi, NativeAudience, NativeCipher, NativePacket,
        NativeRecipient,
    };

    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../native-host/src/delivery_rows.rs"
    ));

    const CONNECTION: ConnectionId = ConnectionId::from_u128(7);

    fn packet(
        id: u64,
        audience: NativeAudience,
        opcode: Option<u16>,
        body: Vec<u8>,
    ) -> NativePacket {
        NativePacket {
            id,
            audience,
            opcode,
            body,
            close_after: false,
            not_before_micros: 0,
            authentication_complete: false,
        }
    }

    #[test]
    fn cipher_row_matches_the_host_fixture() {
        let cipher = NativeCipher {
            connection_id: CONNECTION,
            username: "AB".into(),
            session_key: vec![1, 2],
            client_seed: 9,
            client_proof: vec![3],
        };
        assert_eq!(bsatn::to_vec(&cipher).unwrap(), CIPHER);
    }

    #[test]
    fn packet_rows_match_the_host_fixture() {
        let recipient = || NativeRecipient {
            connection_id: CONNECTION,
            layer: 2,
            entity_guid: 8,
        };
        let rows = [
            (
                NativePacket {
                    close_after: true,
                    not_before_micros: 6,
                    ..packet(
                        1,
                        NativeAudience::Connection(CONNECTION),
                        Some(0x1dd),
                        vec![42],
                    )
                },
                CONNECTION_PACKET,
            ),
            (
                NativePacket {
                    authentication_complete: true,
                    ..packet(
                        2,
                        NativeAudience::Aoi(NativeAoi {
                            layer: 1,
                            entity_guid: 8,
                            exclude_connection: Some(CONNECTION),
                        }),
                        Some(0xb5),
                        vec![43, 44],
                    )
                },
                AOI_PACKET,
            ),
            (
                packet(3, NativeAudience::Grant(recipient()), None, Vec::new()),
                GRANT_PACKET,
            ),
            (
                packet(4, NativeAudience::Revoke(recipient()), Some(0xaa), vec![8]),
                REVOKE_PACKET,
            ),
            (
                packet(5, NativeAudience::Clear(CONNECTION), None, Vec::new()),
                CLEAR_PACKET,
            ),
            (
                packet(
                    6,
                    NativeAudience::Handoff(NativeHandoffTarget {
                        connection_id: CONNECTION,
                        database: spacetimedb::Identity::from_byte_array([9; 32]),
                    }),
                    None,
                    Vec::new(),
                ),
                HANDOFF_PACKET,
            ),
        ];
        for (row, fixture) in rows {
            assert_eq!(bsatn::to_vec(&row).unwrap(), fixture, "packet {}", row.id);
        }
    }

    #[test]
    fn motion_permit_row_matches_the_host_fixture() {
        let permit = super::motion_permit::NativeMotionPermit {
            connection_id: CONNECTION,
            character_guid: 8,
        };
        assert_eq!(bsatn::to_vec(&permit).unwrap(), MOTION_PERMIT);
    }
}
