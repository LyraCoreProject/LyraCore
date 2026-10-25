use std::io::Cursor;

use lyracore_protocol::login_messages::{
    all::CMD_AUTH_LOGON_CHALLENGE_Client,
    version_3::{
        opcodes::ClientOpcodeMessage, CMD_AUTH_LOGON_CHALLENGE_Server,
        CMD_AUTH_LOGON_CHALLENGE_Server_SecurityFlag, CMD_AUTH_LOGON_PROOF_Client,
        CMD_AUTH_LOGON_PROOF_Server, CMD_REALM_LIST_Server, Realm, RealmCategory, RealmFlag,
        RealmType,
    },
    Message,
};
use lyracore_protocol::srp;
use spacetimedb::{
    reducer, table, ConnectionId, Identity, ReducerContext, ScheduleAt, SpacetimeType, Table,
    TimeDuration,
};

use super::{game_native_packet, NativeAudience, NativePacket};
use crate::{game_account, game_character, game_realm};

#[derive(SpacetimeType)]
pub struct NativeLogonProof {
    pub account_id: u64,
    pub username: String,
    pub salt: Vec<u8>,
    pub verifier: Vec<u8>,
    pub private_key: Vec<u8>,
}

#[derive(SpacetimeType)]
pub enum NativeLogonState {
    AwaitingChallenge,
    Proving(NativeLogonProof),
    Authenticated { account_id: u64 },
}

#[table(accessor = game_native_logon_connection)]
pub struct NativeLogonConnection {
    #[primary_key]
    pub connection_id: ConnectionId,
    pub peer: String,
    pub attempts: u8,
    pub state: NativeLogonState,
}

#[table(accessor = game_native_logon_address, index(accessor = by_expiry, btree(columns = [expires_micros])))]
pub struct NativeLogonAddress {
    #[primary_key]
    pub peer: String,
    pub active: u32,
    pub failures: u32,
    pub expires_micros: i64,
}

#[table(accessor = game_native_logon_reaper, scheduled(native_reap_logon_addresses))]
pub struct NativeLogonReaper {
    #[primary_key]
    pub id: u64,
    pub scheduled_at: ScheduleAt,
}

const WINDOW_MICROS: i64 = 60_000_000;

fn address(ctx: &ReducerContext, peer: &str) -> NativeLogonAddress {
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    let mut row = ctx
        .db
        .game_native_logon_address()
        .peer()
        .find(peer.to_string())
        .unwrap_or_else(|| NativeLogonAddress {
            peer: peer.into(),
            active: 0,
            failures: 0,
            expires_micros: now + WINDOW_MICROS,
        });
    if row.expires_micros <= now {
        row.failures = 0;
        row.expires_micros = now + WINDOW_MICROS;
    }
    row
}

fn save_address(ctx: &ReducerContext, row: NativeLogonAddress) {
    let addresses = ctx.db.game_native_logon_address();
    if addresses.peer().find(&row.peer).is_some() {
        addresses.peer().update(row);
    } else {
        addresses.insert(row);
    }
}

#[reducer]
pub fn native_logon_open(ctx: &ReducerContext, peer: String) -> Result<(), String> {
    let connection_id = super::connection_id(ctx)?;
    let peer = peer
        .parse::<std::net::IpAddr>()
        .map_err(|_| "invalid logon address")?
        .to_string();
    if ctx
        .db
        .game_native_logon_connection()
        .connection_id()
        .find(connection_id)
        .is_some()
    {
        return Err("native logon connection is already open".into());
    }
    let mut row = address(ctx, &peer);
    if row.active >= 8 || row.failures >= 10 {
        return Err("logon address limit reached".into());
    }
    row.active += 1;
    save_address(ctx, row);
    ctx.db
        .game_native_logon_connection()
        .insert(NativeLogonConnection {
            connection_id,
            peer,
            attempts: 0,
            state: NativeLogonState::AwaitingChallenge,
        });
    if ctx.db.game_native_logon_reaper().id().find(0).is_none() {
        ctx.db.game_native_logon_reaper().insert(NativeLogonReaper {
            id: 0,
            scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(WINDOW_MICROS)),
        });
    }
    Ok(())
}

#[reducer]
pub fn native_logon_receive(
    ctx: &ReducerContext,
    frame: Vec<u8>,
    entropy: Vec<u8>,
) -> Result<(), String> {
    let connection_id = super::connection_id(ctx)?;
    let connection = ctx
        .db
        .game_native_logon_connection()
        .connection_id()
        .find(connection_id)
        .ok_or("native logon connection is not open")?;
    if frame.len() > 8192 {
        return Err("logon frame exceeds its bound".into());
    }
    let mut reader = Cursor::new(&frame);
    let message = ClientOpcodeMessage::read(&mut reader).map_err(|error| error.to_string())?;
    if reader.position() as usize != frame.len() {
        return Err("trailing logon bytes".into());
    }
    match message {
        ClientOpcodeMessage::CMD_AUTH_LOGON_CHALLENGE(request) => {
            challenge(ctx, connection, request, entropy)
        }
        ClientOpcodeMessage::CMD_AUTH_LOGON_PROOF(request) => prove(ctx, connection, request),
        ClientOpcodeMessage::CMD_REALM_LIST(_) => {
            let mut realms = Vec::new();
            if let NativeLogonState::Authenticated { account_id } = connection.state {
                let account = ctx
                    .db
                    .game_account()
                    .id()
                    .find(account_id)
                    .ok_or("Account no longer exists")?;
                if account.banned {
                    return Err("Account is banned".into());
                }
                let count = ctx
                    .db
                    .game_character()
                    .by_account()
                    .filter(account_id)
                    .count()
                    .min(255) as u8;
                for realm in ctx.db.game_realm().iter() {
                    realms.push(Realm {
                        realm_type: match realm.realm_type {
                            1 => RealmType::PlayerVsPlayer,
                            6 => RealmType::Roleplaying,
                            8 => RealmType::RoleplayingPlayerVsPlayer,
                            _ => RealmType::PlayerVsEnvironment,
                        },
                        flag: RealmFlag::empty(),
                        name: realm.name,
                        address: realm.address,
                        population: realm.population.into(),
                        number_of_characters_on_realm: count,
                        category: RealmCategory::Default,
                        realm_id: realm.id,
                    });
                }
            }
            send(
                ctx,
                connection_id,
                CMD_REALM_LIST_Server { realms },
                false,
                0,
                false,
            )
        }
        _ => Err("unsupported native logon request".into()),
    }
}

fn send(
    ctx: &ReducerContext,
    connection_id: ConnectionId,
    message: impl Message,
    close_after: bool,
    delay_micros: i64,
    authentication_complete: bool,
) -> Result<(), String> {
    let mut body = Vec::new();
    message
        .write(&mut body)
        .map_err(|error| error.to_string())?;
    ctx.db.game_native_packet().insert(NativePacket {
        id: 0,
        audience: NativeAudience::Connection(connection_id),
        opcode: None,
        body,
        close_after,
        not_before_micros: ctx.timestamp.to_micros_since_unix_epoch() + delay_micros,
        authentication_complete,
    });
    Ok(())
}

fn reject(
    ctx: &ReducerContext,
    mut connection: NativeLogonConnection,
    message: impl Message,
    delay_micros: i64,
) -> Result<(), String> {
    let mut row = address(ctx, &connection.peer);
    if row.failures == 0 {
        row.expires_micros = ctx.timestamp.to_micros_since_unix_epoch() + WINDOW_MICROS;
    }
    row.failures += 1;
    let close_after = row.failures >= 10 || connection.attempts >= 3;
    save_address(ctx, row);
    send(
        ctx,
        connection.connection_id,
        message,
        close_after,
        delay_micros,
        false,
    )?;
    connection.state = NativeLogonState::AwaitingChallenge;
    ctx.db
        .game_native_logon_connection()
        .connection_id()
        .update(connection);
    Ok(())
}

fn challenge(
    ctx: &ReducerContext,
    mut connection: NativeLogonConnection,
    request: CMD_AUTH_LOGON_CHALLENGE_Client,
    entropy: Vec<u8>,
) -> Result<(), String> {
    if connection.attempts >= 3 || address(ctx, &connection.peer).failures >= 10 {
        return Err("logon attempt limit reached".into());
    }
    connection.attempts += 1;
    if request.version.build != 5875 {
        return reject(
            ctx,
            connection,
            CMD_AUTH_LOGON_CHALLENGE_Server::FailVersionInvalid,
            0,
        );
    }
    let Ok(username) = srp::normalize(&request.account_name) else {
        return reject(
            ctx,
            connection,
            CMD_AUTH_LOGON_CHALLENGE_Server::FailUnknownAccount,
            0,
        );
    };
    let Some(account) = ctx.db.game_account().username().find(username.clone()) else {
        return reject(
            ctx,
            connection,
            CMD_AUTH_LOGON_CHALLENGE_Server::FailUnknownAccount,
            0,
        );
    };
    if account.banned {
        return reject(
            ctx,
            connection,
            CMD_AUTH_LOGON_CHALLENGE_Server::FailBanned,
            0,
        );
    }
    let private_key: &[u8; 32] = entropy
        .as_slice()
        .try_into()
        .map_err(|_| "host entropy must have 32 bytes")?;
    let verifier: Result<&[u8; 32], _> = account.verifier.as_slice().try_into();
    let salt: Result<[u8; 32], _> = account.salt.as_slice().try_into();
    let (Ok(verifier), Ok(salt)) = (verifier, salt) else {
        return reject(
            ctx,
            connection,
            CMD_AUTH_LOGON_CHALLENGE_Server::FailUnknownAccount,
            0,
        );
    };
    let Ok(server_public_key) = srp::server_public_key(verifier, private_key) else {
        return reject(
            ctx,
            connection,
            CMD_AUTH_LOGON_CHALLENGE_Server::FailUnknownAccount,
            0,
        );
    };
    send(
        ctx,
        connection.connection_id,
        CMD_AUTH_LOGON_CHALLENGE_Server::Success {
            crc_salt: [0; 16],
            generator: vec![srp::GENERATOR],
            large_safe_prime: srp::PRIME.to_vec(),
            salt,
            server_public_key,
            security_flag: CMD_AUTH_LOGON_CHALLENGE_Server_SecurityFlag::None,
        },
        false,
        0,
        false,
    )?;
    connection.state = NativeLogonState::Proving(NativeLogonProof {
        account_id: account.id,
        username,
        salt: salt.to_vec(),
        verifier: account.verifier,
        private_key: entropy,
    });
    ctx.db
        .game_native_logon_connection()
        .connection_id()
        .update(connection);
    Ok(())
}

fn prove(
    ctx: &ReducerContext,
    mut connection: NativeLogonConnection,
    request: CMD_AUTH_LOGON_PROOF_Client,
) -> Result<(), String> {
    let NativeLogonState::Proving(proof) = &connection.state else {
        return Err("logon proof requires a challenge".into());
    };
    let account_id = proof.account_id;
    let account = ctx
        .db
        .game_account()
        .id()
        .find(account_id)
        .ok_or("Account no longer exists")?;
    let verified =
        if !account.banned && account.salt == proof.salt && account.verifier == proof.verifier {
            srp::verify(
                &proof.username,
                proof
                    .salt
                    .as_slice()
                    .try_into()
                    .map_err(|_| "invalid stored salt")?,
                proof
                    .verifier
                    .as_slice()
                    .try_into()
                    .map_err(|_| "invalid stored verifier")?,
                proof
                    .private_key
                    .as_slice()
                    .try_into()
                    .map_err(|_| "invalid stored private key")?,
                &request.client_public_key,
                &request.client_proof,
            )
            .ok()
        } else {
            None
        };
    let Some(verified) = verified else {
        return reject(
            ctx,
            connection,
            CMD_AUTH_LOGON_PROOF_Server::FailIncorrectPassword,
            200_000,
        );
    };
    let mut binding = blake3::Hasher::new();
    binding.update(b"lyracore-native-account");
    binding.update(&ctx.database_identity().to_byte_array());
    binding.update(&account_id.to_le_bytes());
    let identity = Identity::from_byte_array(*binding.finalize().as_bytes());
    crate::auth::establish_session(ctx, account_id, verified.session_key.to_vec(), identity)?;
    send(
        ctx,
        connection.connection_id,
        CMD_AUTH_LOGON_PROOF_Server::Success {
            hardware_survey_id: 0,
            server_proof: verified.server_proof,
        },
        false,
        0,
        true,
    )?;
    connection.state = NativeLogonState::Authenticated { account_id };
    ctx.db
        .game_native_logon_connection()
        .connection_id()
        .update(connection);
    Ok(())
}

pub(super) fn close(ctx: &ReducerContext, connection: ConnectionId) {
    let connections = ctx.db.game_native_logon_connection();
    let Some(connection) = connections.connection_id().find(connection) else {
        return;
    };
    connections.connection_id().delete(connection.connection_id);
    let mut row = address(ctx, &connection.peer);
    row.active = row.active.saturating_sub(1);
    if row.active == 0 && row.failures == 0 {
        ctx.db.game_native_logon_address().peer().delete(row.peer);
    } else {
        save_address(ctx, row);
    }
}

#[reducer]
pub fn native_reap_logon_addresses(
    ctx: &ReducerContext,
    _schedule: NativeLogonReaper,
) -> Result<(), String> {
    if ctx.sender() != ctx.database_identity() {
        return Err("scheduler only".into());
    }
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    let addresses = ctx.db.game_native_logon_address();
    let expired: Vec<_> = addresses.by_expiry().filter(..=now).collect();
    for mut row in expired {
        if row.active == 0 {
            addresses.peer().delete(row.peer);
        } else {
            row.failures = 0;
            row.expires_micros = now + WINDOW_MICROS;
            addresses.peer().update(row);
        }
    }
    Ok(())
}
