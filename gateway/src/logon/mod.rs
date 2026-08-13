//! Logon listener (TCP 3724): SRP6 + realm list.
//!
//! The protocol logic is implemented as a transport- and storage-agnostic state machine
//! ([`handle_logon`]) over a blocking `Read + Write` stream, driven by `wow_login_messages`
//! codecs and `wow_srp` for all SRP6 math (never hand-rolled). Storage/coordination is behind
//! the [`LogonStore`] trait so the crypto can be unit-tested end-to-end without a database or a
//! socket (see the tests below, which drive a full handshake with `wow_srp`'s client side).
//!
//! The only state this touches is the `game_account` read (salt/verifier) and the session write
//! (K) — both via `LogonStore`. Everything else is per-connection handshake scratch.

use crate::accept::{classify_accept_error, AcceptBackoff, AcceptOutcome};
use crate::{config::GatewayConfig, stdb::Coordinator};
use anyhow::{anyhow, Result};
use std::io::{Read, Write};
use tokio::net::TcpListener;
use wow_login_messages::all::CMD_AUTH_LOGON_CHALLENGE_Client;
use wow_login_messages::errors::ExpectedOpcodeError;
// Build 5875 is vanilla and connects with `protocol_version = 3`, so the whole logon flow speaks
// gtker's `version_3` message set — the module purpose-built for 1.12.1.5875. It is the exact fit:
//  - its CMD_AUTH_LOGON_CHALLENGE_Server appends the trailing `security_flag` byte the real client
//    REQUIRES (version_2 omits it → the client rejects the challenge and disconnects);
//  - its CMD_AUTH_LOGON_PROOF_Server is a re-export of version_2's short 26-byte proof (version_8's
//    proof adds account_flag + survey + 14×padding ≈ 60 bytes, which desyncs the client right after
//    it shows "Success!");
//  - its realm list re-exports version_2's vanilla u8-count format;
//  - its proof_client reader consumes the trailing security_flag byte the real client sends.
use wow_login_messages::version_3::opcodes::ClientOpcodeMessage;
use wow_login_messages::version_3::{
    CMD_AUTH_LOGON_CHALLENGE_Server, CMD_AUTH_LOGON_CHALLENGE_Server_SecurityFlag,
    CMD_AUTH_LOGON_PROOF_Server, CMD_REALM_LIST_Server, Realm, RealmCategory, RealmFlag, RealmType,
};
use wow_login_messages::Message;
use wow_srp::normalized_string::NormalizedString;
use wow_srp::server::{SrpProof, SrpVerifier};
use wow_srp::{PublicKey, GENERATOR, LARGE_SAFE_PRIME_LITTLE_ENDIAN};

/// SRP6 material for one account, read from `game_account`.
#[derive(Clone, Debug)]
pub struct LogonAccount {
    pub id: u64,
    pub salt: [u8; 32],
    pub verifier: [u8; 32],
    pub banned: bool,
}

/// One realm for the realm-list reply, read from `game_realm`.
#[derive(Clone, Debug)]
pub struct RealmInfo {
    pub id: u8,
    pub name: String,
    pub address: String, // "ip:port"
    pub realm_type: u32,
    pub population: f32,
    pub number_of_characters: u8,
}

/// Storage/coordination the logon flow needs. Implemented by the SpacetimeDB-backed
/// [`CoordinatorStore`] in production and by an in-memory fake in tests.
pub trait LogonStore: Send + Sync {
    /// Look up an account by (already uppercased) username.
    fn account(&self, username: &str) -> Result<Option<LogonAccount>>;
    /// The per-account SpacetimeDB identity the world connection will authenticate as.
    ///
    /// Takes `username` for the same reason [`LogonStore::save_session`] does: the connection this
    /// identity belongs to lives on the WORLD shard and is keyed by the WORLD shard's account id,
    /// while `account_id` is the authenticating (realm-core) database's. Handing the wrong id here
    /// binds the account's characters to a connection the world phase never uses — and, when two
    /// accounts' ids happen to be transposed between the two databases, to ANOTHER account's
    /// connection.
    fn bound_identity(&self, account_id: u64, username: &str) -> Result<[u8; 32]>;
    /// Persist the shared session key K (writes `game_session` via `establish_session`).
    ///
    /// `username` is passed alongside `account_id` because the two are NOT interchangeable across
    /// databases: `account_id` is the authenticating database's `#[auto_inc]` surrogate key,
    /// while the username is the realm-wide identity of the account. An implementation that has to
    /// write more than one database (the realm-core split) must re-resolve the id per database from
    /// the username; it may never carry one database's id to another.
    fn save_session(
        &self,
        account_id: u64,
        username: &str,
        session_key: &[u8; 40],
        bound_identity: [u8; 32],
    ) -> Result<()>;
    /// Realms to advertise (with each realm's character count for this account). `username` again:
    /// the character count is a WORLD-shard fact (`game_character.account_id`), so it must be
    /// counted against the world shard's id, not the authenticating database's.
    fn realms(&self, account_id: u64, username: &str) -> Result<Vec<RealmInfo>>;
}

/// Drive one logon connection to completion (challenge -> proof -> realm list). Returns when
/// the client disconnects. Pure protocol logic; no async, no direct IO beyond `stream`.
pub fn handle_logon<S: Read + Write, St: LogonStore + ?Sized>(
    stream: &mut S,
    store: &St,
) -> Result<()> {
    // Per-connection scratch: the in-flight SRP proof + which account it is for (id AND the
    // normalized username — see `LogonStore::save_session`). `SrpProof` is consumed by
    // `into_server`, so it lives here between the challenge and the proof.
    let mut pending: Option<(SrpProof, u64, String)> = None;
    // The account (id AND normalized username) once the proof has succeeded — both drive the
    // realm-list reply, because the character count it carries is counted on a database that may
    // not be the one that issued `id`.
    let mut authenticated: Option<(u64, String)> = None;

    loop {
        let msg = match ClientOpcodeMessage::read(&mut *stream) {
            Ok(m) => m,
            // Clean client disconnect.
            Err(ExpectedOpcodeError::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Ok(());
            }
            Err(e) => return Err(anyhow!("logon read error: {e}")),
        };

        match msg {
            ClientOpcodeMessage::CMD_AUTH_LOGON_CHALLENGE(c) => {
                pending = handle_challenge(stream, store, &c)?;
            }
            ClientOpcodeMessage::CMD_AUTH_LOGON_PROOF(p) => {
                let Some((proof, account_id, username)) = pending.take() else {
                    return Err(anyhow!("CMD_AUTH_LOGON_PROOF before a challenge"));
                };
                authenticated = handle_proof(
                    stream,
                    store,
                    proof,
                    account_id,
                    &username,
                    p.client_public_key,
                    p.client_proof,
                )?
                .map(|id| (id, username));
            }
            ClientOpcodeMessage::CMD_REALM_LIST(_) => {
                // Without a completed proof we don't know the account; advertise nothing.
                let realms = match &authenticated {
                    Some((id, username)) => store.realms(*id, username)?,
                    None => Vec::new(),
                };
                let reply = CMD_REALM_LIST_Server {
                    realms: realms.iter().map(to_realm).collect(),
                };
                let mut bytes = Vec::new();
                reply.write(&mut bytes)?;
                log::info!(
                    "logon: realm list ({} realm(s), {} bytes) to account {:?}: {:02x?}",
                    realms.len(),
                    bytes.len(),
                    authenticated.as_ref().map(|(id, _)| id),
                    bytes
                );
                stream.write_all(&bytes)?;
            }
            other => log::debug!("logon: ignoring opcode {other}"),
        }
    }
}

/// Look up the account, run SRP6 to the point of the server public key, and reply.
fn handle_challenge<S: Read + Write, St: LogonStore + ?Sized>(
    stream: &mut S,
    store: &St,
    c: &CMD_AUTH_LOGON_CHALLENGE_Client,
) -> Result<Option<(SrpProof, u64, String)>> {
    let username = c.account_name.to_uppercase();
    let account = match store.account(&username)? {
        Some(a) if a.banned => {
            CMD_AUTH_LOGON_CHALLENGE_Server::FailBanned.write(&mut *stream)?;
            return Ok(None);
        }
        Some(a) => a,
        None => {
            CMD_AUTH_LOGON_CHALLENGE_Server::FailUnknownAccount.write(&mut *stream)?;
            return Ok(None);
        }
    };

    log::info!(
        "logon: challenge for {username} (account {}), client protocol_version={:?}",
        account.id,
        c.protocol_version
    );
    // An un-provisioned account has an all-zero verifier (the `init` placeholder). `wow_srp`'s
    // key math panics on an all-zero key, so reject it cleanly as unknown rather than letting it
    // crash the connection. Provision the account (`gateway provision <user> --password-stdin`)
    // first.
    if account.verifier.iter().all(|&b| b == 0) {
        log::warn!(
            "logon: account {} ({username}) has no SRP6 verifier (not provisioned) — rejecting",
            account.id
        );
        CMD_AUTH_LOGON_CHALLENGE_Server::FailUnknownAccount.write(&mut *stream)?;
        return Ok(None);
    }
    let ns = NormalizedString::new(&username).map_err(|e| anyhow!("bad username: {e}"))?;
    let proof = SrpVerifier::from_database_values(ns, account.verifier, account.salt).into_proof();

    CMD_AUTH_LOGON_CHALLENGE_Server::Success {
        crc_salt: [0u8; 16],
        generator: vec![GENERATOR],
        large_safe_prime: LARGE_SAFE_PRIME_LITTLE_ENDIAN.to_vec(),
        salt: *proof.salt(),
        server_public_key: *proof.server_public_key(),
        // Vanilla clients require this trailing byte; `None` = no PIN/matrix challenge.
        security_flag: CMD_AUTH_LOGON_CHALLENGE_Server_SecurityFlag::None,
    }
    .write(&mut *stream)?;

    Ok(Some((proof, account.id, username)))
}

/// Verify the client proof; on success derive K, persist the session, and reply with M2.
/// Returns the authenticated account id on success, `None` on a (clean) proof failure.
fn handle_proof<S: Read + Write, St: LogonStore + ?Sized>(
    stream: &mut S,
    store: &St,
    proof: SrpProof,
    account_id: u64,
    username: &str,
    client_public_key: [u8; 32],
    client_proof: [u8; 20],
) -> Result<Option<u64>> {
    let client_public_key = match PublicKey::from_le_bytes(client_public_key) {
        Ok(k) => k,
        Err(_) => {
            CMD_AUTH_LOGON_PROOF_Server::FailIncorrectPassword.write(&mut *stream)?;
            return Ok(None);
        }
    };

    let (server, server_proof) = match proof.into_server(client_public_key, client_proof) {
        Ok(v) => v,
        Err(_) => {
            // Proof mismatch == wrong password. Common; not an error.
            CMD_AUTH_LOGON_PROOF_Server::FailIncorrectPassword.write(&mut *stream)?;
            return Ok(None);
        }
    };

    let session_key = *server.session_key();
    let bound = store.bound_identity(account_id, username)?;
    store.save_session(account_id, username, &session_key, bound)?;
    log::info!("logon: account {account_id} authenticated (SRP6 proof OK, K + session persisted)");

    CMD_AUTH_LOGON_PROOF_Server::Success {
        hardware_survey_id: 0,
        server_proof,
    }
    .write(&mut *stream)?;
    Ok(Some(account_id))
}

fn to_realm(r: &RealmInfo) -> Realm {
    Realm {
        realm_type: match r.realm_type {
            1 => RealmType::PlayerVsPlayer,
            6 => RealmType::Roleplaying,
            8 => RealmType::RoleplayingPlayerVsPlayer,
            _ => RealmType::PlayerVsEnvironment,
        },
        flag: RealmFlag::empty(),
        name: r.name.clone(),
        address: r.address.clone(),
        population: r.population.into(),
        number_of_characters_on_realm: r.number_of_characters,
        category: RealmCategory::Default,
        realm_id: r.id,
    }
}

// ===========================================================================================
//  Async listener (bridges accepted sockets to the blocking handler)
// ===========================================================================================

pub async fn run(cfg: GatewayConfig, coordinator: Coordinator) -> Result<()> {
    let listener = TcpListener::bind(&cfg.logon_bind).await?;
    log::info!("logon listening on {}", cfg.logon_bind);
    // A transient accept errno must cost ONE connection, not the realm. See `crate::accept`
    // for the policy and for the errno that actually killed the gateway on 2026-08-07.
    let mut backoff = AcceptBackoff::new();
    loop {
        let (sock, peer) = match listener.accept().await {
            Ok(pair) => {
                backoff.record_success();
                pair
            }
            Err(e) => match classify_accept_error(&e) {
                AcceptOutcome::Fatal => {
                    log::error!(
                        "logon listener is unusable and cannot accept again: {e} — ending the \
                         logon task"
                    );
                    return Err(e.into());
                }
                AcceptOutcome::Retry => {
                    let delay = backoff.record_failure();
                    log::warn!(
                        "logon accept failed ({e}); skipping this connection (consecutive={}, \
                         backing off {}ms)",
                        backoff.consecutive(),
                        delay.as_millis()
                    );
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                    continue;
                }
            },
        };
        let coord = coordinator.clone();
        // wow_login_messages uses blocking std::io codecs, so run the per-connection state
        // machine on a blocking task with the socket in blocking mode.
        // These two are per-SOCKET (a dup and an fcntl on the fd we just accepted), so they fail
        // for the same reasons accept does — EMFILE above all. Drop the one connection.
        let std_sock = match sock.into_std().and_then(|s| {
            s.set_nonblocking(false)?;
            Ok(s)
        }) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("logon connection {peer} could not be handed to a blocking task: {e}");
                continue;
            }
        };
        tokio::task::spawn_blocking(move || {
            let store = CoordinatorStore::new(coord);
            let mut s = std_sock;
            if let Err(e) = handle_logon(&mut s, &store) {
                log::warn!("logon session {peer} ended: {e:#}");
            }
        });
    }
}

/// `LogonStore` backed by the SpacetimeDB coordinator connections.
///
/// # Which database answers what
///
/// | question | database | why |
/// |---|---|---|
/// | SRP6 salt/verifier/banned | **realm-core** | the account is realm-wide state |
/// | session key K | **realm-core** | so any gateway completes any handshake (the stateless-gateway invariant, now realm-scoped) |
/// | the world connection's bound identity | world shard | it is that shard's connection |
/// | `game_character.owner_identity` binding | world shard | characters are shard state |
/// | realm list + per-realm character count | world shard | `game_realm` / `game_character` |
///
/// With `LYRACORE_REALM_CORE` unset, `Coordinator::realm_core()` returns the world handle and this table
/// collapses to one database — today's behavior, byte for byte.
///
/// Every row in the "world shard" column takes an account id, and every one of those ids is
/// re-resolved from the username by [`CoordinatorStore::world_account_id`]. The id this store is
/// handed came from realm-core and is meaningless anywhere else.
///
/// Generic over the STORE rather than hard-wired to `Coordinator`, because `Coordinator` wraps a
/// live SpacetimeDB websocket and nothing in the crate could execute these bodies without a node —
/// which is why four mutations against them, including "read the SRP6 material off the world DB",
/// survived the realm-core PR's own suite before this fix. Production binds `D = Coordinator`;
/// `realm_core::tests` binds `D = fake::Handle` and runs THESE bodies, not a model of them.
pub(crate) struct CoordinatorStore<D: crate::realm_core::RealmDb> {
    coordinator: D,
}

impl<D: crate::realm_core::RealmDb> CoordinatorStore<D> {
    pub(crate) fn new(coordinator: D) -> Self {
        Self { coordinator }
    }

    /// Translate the AUTHENTICATING database's account id into the WORLD shard's, by username.
    ///
    /// Every world-side call in this file takes an id, and every one of those ids has to be the
    /// world shard's own `#[auto_inc]` key — `game_character.account_id` references it,
    /// `player_conn` is cached under it, and `account_by_identity` resolves back through it. The id
    /// that arrives here came from realm-core. This is the one function allowed to bridge them, and
    /// the rule it applies is [`world_account_id`], which is pure and tested.
    fn world_account_id(&self, realm_account_id: u64, username: &str) -> Result<u64> {
        world_account_id(
            realm_account_id,
            username,
            self.coordinator.realm_core()?.shard_name() == self.coordinator.shard_name(),
            self.coordinator.shard_name(),
            || {
                Ok(self
                    .coordinator
                    .account_by_username(username)?
                    .map(|a| a.id))
            },
        )
    }
}

/// The cross-database account-key rule, extracted so it can be tested without a node.
///
/// `Account.id` is `#[auto_inc]`, so it is a *per-database* surrogate key; the USERNAME is the only
/// realm-wide account identity. Given realm-core's id for `username`, answer with the id the WORLD
/// shard issued for the same username:
///
/// - `same_database` (realm-core unconfigured, or naming the world database) → the two ids are the
///   same id, so pass it straight through and do not spend a lookup. This is what keeps the
///   unconfigured path identical to the pre-realm-core gateway.
/// - otherwise → whatever the world shard's own `game_account` says.
/// - **never** the realm-core id on a world-shard call. A missing world-side account is an `Err`,
///   not a fallback: falling back would hand the world shard an id that, on any deployment whose
///   two databases were not seeded in the same order, names a DIFFERENT player's account — binding
///   this login's session key and connection identity to that player's characters.
fn world_account_id(
    realm_account_id: u64,
    username: &str,
    same_database: bool,
    world_db: &str,
    lookup: impl FnOnce() -> Result<Option<u64>>,
) -> Result<u64> {
    if same_database {
        return Ok(realm_account_id);
    }
    lookup()?.ok_or_else(|| {
        anyhow!(
            "account {username} exists on realm-core but not on world shard {world_db} — provision \
             it there before it can log in (accounts are keyed across databases by USERNAME; ids \
             are per-database and must never be carried between them)"
        )
    })
}

impl<D: crate::realm_core::RealmDb> LogonStore for CoordinatorStore<D> {
    fn account(&self, username: &str) -> Result<Option<LogonAccount>> {
        // The SRP6 material is realm-core's, always. `realm_core()` errors (rather than falling
        // back to the world DB's auth cache) when realm-core is configured and unreachable, which
        // surfaces as a failed logon connection — the deliberate fail-closed posture.
        let Some(a) = self
            .coordinator
            .realm_core()?
            .account_by_username(username)?
        else {
            return Ok(None);
        };
        // game_account stores these as 32-byte vecs. A wrong-length blob (corrupt provisioning,
        // half-written migration) must surface as an error — silently coercing it to all-zeros
        // would forge the legitimate all-zero `init` verifier placeholder (see :160).
        let salt = a.salt.try_into().map_err(|v: Vec<u8>| {
            anyhow!("account {}: salt is {} bytes, expected 32", a.id, v.len())
        })?;
        let verifier = a.verifier.try_into().map_err(|v: Vec<u8>| {
            anyhow!(
                "account {}: verifier is {} bytes, expected 32",
                a.id,
                v.len()
            )
        })?;
        Ok(Some(LogonAccount {
            id: a.id,
            salt,
            verifier,
            banned: a.banned,
        }))
    }

    fn bound_identity(&self, account_id: u64, username: &str) -> Result<[u8; 32]> {
        // The stable per-account identity for this account, DERIVED by the world shard's key
        // (`synthetic_owner_identity`) — the world phase later checks out with the same id
        // (`WorldSession::account_id`, itself re-resolved by username in
        // `world_store::lookup_session`). Handing realm-core's id here would bind
        // `game_character.owner_identity` to an identity nothing ever presents again.
        let world_id = self.world_account_id(account_id, username)?;
        self.coordinator.bound_identity(world_id)
    }

    /// Two writes, in this order:
    ///
    /// 1. **realm-core** — the authoritative `game_session` row (K + the bound identity). This is
    ///    the row every world gateway later reads to complete a handshake, so it is what makes the
    ///    gateway stateless at REALM scope.
    /// 2. **the world shard** — the same reducer, whose real job here is binding
    ///    `game_character.owner_identity` for this account's characters so per-owner RLS lets the
    ///    player see them. Its `game_account`/`game_session` writes are the WRITE-THROUGH CACHE of
    ///    the realm-core rows: refreshed on every logon, never read for
    ///    authentication while realm-core is configured. That is why the tables stay in the world
    ///    schema — dropping them is a destructive migration, and they are load-bearing for the
    ///    character binding regardless.
    ///
    /// The world-shard id is re-resolved from the USERNAME. `account_id` is realm-core's
    /// `#[auto_inc]` key and means nothing on another database; see [`LogonStore::save_session`].
    fn save_session(
        &self,
        account_id: u64,
        username: &str,
        session_key: &[u8; 40],
        bound_identity: [u8; 32],
    ) -> Result<()> {
        let realm_core = self.coordinator.realm_core()?;
        realm_core.establish_session(account_id, session_key, bound_identity)?;
        if realm_core.shard_name() == self.coordinator.shard_name() {
            return Ok(()); // unconfigured realm-core: one database, one write — exactly as before
        }
        self.coordinator.establish_session(
            self.world_account_id(account_id, username)?,
            session_key,
            bound_identity,
        )
    }

    fn realms(&self, account_id: u64, username: &str) -> Result<Vec<RealmInfo>> {
        let realm = self.coordinator.realm()?;
        // `game_character.account_id` is the WORLD shard's key — counting realm-core's id here
        // would advertise a different account's character count on the realm-select screen.
        let n = self
            .coordinator
            .character_count(self.world_account_id(account_id, username)?)?;
        Ok(vec![RealmInfo {
            id: realm.id,
            name: realm.name,
            // The row says where the world listener is *for this deployment* only if nobody
            // told us otherwise. `LYRACORE_REALM_ADDRESS` is how `lyracore dev up --lan` makes the
            // realm reachable from another machine — see `config::advertised_realm_address`.
            address: crate::config::advertised_realm_address_or(realm.address),
            realm_type: realm.realm_type,
            population: realm.population,
            number_of_characters: n,
        }])
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    use std::sync::Mutex;
    use wow_login_messages::all::{Locale, Os, Platform, ProtocolVersion, Version};
    use wow_login_messages::version_3::opcodes::ServerOpcodeMessage;
    use wow_srp::client::SrpClientChallenge;

    /// In-memory `LogonStore` for end-to-end SRP6 tests.
    struct InMemoryStore {
        account: LogonAccount,
        username: String,
        saved: Mutex<Option<[u8; 40]>>,
        /// `(method, username)` for every store call that was told the account's name — the
        /// cross-database account key. A world-side implementation re-resolves its own id from it,
        /// so a method that stops being told it silently regresses to carrying realm-core's id.
        usernames: Mutex<Vec<(&'static str, String)>>,
    }

    impl InMemoryStore {
        fn told(&self, method: &str) -> Option<String> {
            let seen = self.usernames.lock().unwrap();
            seen.iter()
                .find(|(m, _)| *m == method)
                .map(|(_, u)| u.clone())
        }
    }

    impl LogonStore for InMemoryStore {
        fn account(&self, username: &str) -> Result<Option<LogonAccount>> {
            Ok((username == self.username).then(|| self.account.clone()))
        }
        fn bound_identity(&self, _account_id: u64, username: &str) -> Result<[u8; 32]> {
            self.usernames
                .lock()
                .unwrap()
                .push(("bound_identity", username.to_string()));
            Ok([7u8; 32])
        }
        fn save_session(
            &self,
            _id: u64,
            username: &str,
            key: &[u8; 40],
            _b: [u8; 32],
        ) -> Result<()> {
            *self.saved.lock().unwrap() = Some(*key);
            self.usernames
                .lock()
                .unwrap()
                .push(("save_session", username.to_string()));
            Ok(())
        }
        fn realms(&self, _account_id: u64, username: &str) -> Result<Vec<RealmInfo>> {
            self.usernames
                .lock()
                .unwrap()
                .push(("realms", username.to_string()));
            Ok(vec![RealmInfo {
                id: 1,
                name: "LyraCore".into(),
                address: "127.0.0.1:8085".into(),
                realm_type: 0,
                population: 0.0,
                number_of_characters: 1,
            }])
        }
    }

    fn ns(s: &str) -> NormalizedString {
        NormalizedString::new(s).unwrap()
    }

    fn challenge_client(account: &str) -> CMD_AUTH_LOGON_CHALLENGE_Client {
        CMD_AUTH_LOGON_CHALLENGE_Client {
            protocol_version: ProtocolVersion::Three,
            version: Version {
                major: 1,
                minor: 12,
                patch: 1,
                build: 5875,
            },
            platform: Platform::X86,
            os: Os::Windows,
            locale: Locale::EnUs,
            utc_timezone_offset: 0,
            client_ip_address: std::net::Ipv4Addr::new(127, 0, 0, 1),
            account_name: account.to_string(),
        }
    }

    #[test]
    fn full_srp6_handshake_and_realm_list() {
        // Seed an account whose verifier matches password "PASSWORD".
        let v = SrpVerifier::from_username_and_password(ns("TEST"), ns("PASSWORD"));
        let salt = *v.salt();
        let verifier = *v.password_verifier();
        let store = std::sync::Arc::new(InMemoryStore {
            account: LogonAccount {
                id: 1,
                salt,
                verifier,
                banned: false,
            },
            username: "TEST".into(),
            saved: Mutex::new(None),
            usernames: Mutex::new(Vec::new()),
        });

        let (mut client, server_end) = UnixStream::pair().unwrap();
        let server_store = store.clone();
        let server = std::thread::spawn(move || {
            let mut s = server_end;
            handle_logon(&mut s, server_store.as_ref()).unwrap();
        });

        // --- client: send challenge ---
        challenge_client("TEST").write(&mut client).unwrap();

        // --- client: read server challenge ---
        let (g, n, s_salt, server_pubkey) = match ServerOpcodeMessage::read(&mut client).unwrap() {
            ServerOpcodeMessage::CMD_AUTH_LOGON_CHALLENGE(
                CMD_AUTH_LOGON_CHALLENGE_Server::Success {
                    generator,
                    large_safe_prime,
                    salt,
                    server_public_key,
                    ..
                },
            ) => (generator, large_safe_prime, salt, server_public_key),
            other => panic!("expected challenge success, got {other:?}"),
        };
        assert_eq!(g, vec![7]);

        // --- client: compute proof with wow_srp client side ---
        let n: [u8; 32] = n.try_into().unwrap();
        let challenge = SrpClientChallenge::new(
            ns("TEST"),
            ns("PASSWORD"),
            g[0],
            n,
            PublicKey::from_le_bytes(server_pubkey).unwrap(),
            s_salt,
        );
        let client_public_key = *challenge.client_public_key();
        let client_proof = *challenge.client_proof();

        wow_login_messages::version_3::CMD_AUTH_LOGON_PROOF_Client {
            client_public_key,
            client_proof,
            crc_hash: [0u8; 20],
            telemetry_keys: vec![],
            security_flag:
                wow_login_messages::version_3::CMD_AUTH_LOGON_PROOF_Client_SecurityFlag::None,
        }
        .write(&mut client)
        .unwrap();

        // --- client: read M2 and verify the server also knows the password ---
        let server_proof = match ServerOpcodeMessage::read(&mut client).unwrap() {
            ServerOpcodeMessage::CMD_AUTH_LOGON_PROOF(CMD_AUTH_LOGON_PROOF_Server::Success {
                server_proof,
                ..
            }) => server_proof,
            other => panic!("expected proof success, got {other:?}"),
        };
        let srp_client = challenge.verify_server_proof(server_proof).unwrap();

        // Both sides derived the same session key K.
        assert_eq!(
            srp_client.session_key(),
            store.saved.lock().unwrap().as_ref().unwrap()
        );

        // --- client: request the realm list ---
        wow_login_messages::version_8::CMD_REALM_LIST_Client {}
            .write(&mut client)
            .unwrap();
        // The realm list is emitted in the vanilla (version_3 == version_2) format.
        match ServerOpcodeMessage::read(&mut client).unwrap() {
            ServerOpcodeMessage::CMD_REALM_LIST(reply) => {
                assert_eq!(reply.realms.len(), 1);
                assert_eq!(reply.realms[0].address, "127.0.0.1:8085");
                assert_eq!(reply.realms[0].number_of_characters_on_realm, 1);
            }
            other => panic!("expected realm list, got {other:?}"),
        }

        drop(client); // client disconnect -> server handler returns
        server.join().unwrap();
    }

    #[test]
    fn save_session_is_told_the_normalized_username_not_just_the_account_id() {
        // `CoordinatorStore::save_session` has to write TWO databases — realm-core (the
        // authoritative K) and the world shard (the character owner_identity binding) — and
        // `account_id` is only meaningful on the database that issued it (`#[auto_inc]`). The
        // username is the cross-database key, so the state machine must hand it down, already
        // uppercased exactly as the account lookup normalized it. Without this the world-side write
        // would have to reuse realm-core's id and could bind one account's session to another
        // account's characters.
        let v = SrpVerifier::from_username_and_password(ns("TEST"), ns("PASSWORD"));
        let store = std::sync::Arc::new(InMemoryStore {
            account: LogonAccount {
                id: 7,
                salt: *v.salt(),
                verifier: *v.password_verifier(),
                banned: false,
            },
            username: "TEST".into(),
            saved: Mutex::new(None),
            usernames: Mutex::new(Vec::new()),
        });

        let (mut client, server_end) = UnixStream::pair().unwrap();
        let server_store = store.clone();
        let server = std::thread::spawn(move || {
            let mut s = server_end;
            let _ = handle_logon(&mut s, server_store.as_ref());
        });

        // The client sends the account name in lowercase; the server uppercases it once, at the
        // challenge, and everything downstream must see THAT form.
        challenge_client("test").write(&mut client).unwrap();
        let (g, n, s_salt, server_pubkey) = match ServerOpcodeMessage::read(&mut client).unwrap() {
            ServerOpcodeMessage::CMD_AUTH_LOGON_CHALLENGE(
                CMD_AUTH_LOGON_CHALLENGE_Server::Success {
                    generator,
                    large_safe_prime,
                    salt,
                    server_public_key,
                    ..
                },
            ) => (generator, large_safe_prime, salt, server_public_key),
            other => panic!("expected challenge success, got {other:?}"),
        };
        let n: [u8; 32] = n.try_into().unwrap();
        let challenge = SrpClientChallenge::new(
            ns("TEST"),
            ns("PASSWORD"),
            g[0],
            n,
            PublicKey::from_le_bytes(server_pubkey).unwrap(),
            s_salt,
        );
        wow_login_messages::version_3::CMD_AUTH_LOGON_PROOF_Client {
            client_public_key: *challenge.client_public_key(),
            client_proof: *challenge.client_proof(),
            crc_hash: [0u8; 20],
            telemetry_keys: vec![],
            security_flag:
                wow_login_messages::version_3::CMD_AUTH_LOGON_PROOF_Client_SecurityFlag::None,
        }
        .write(&mut client)
        .unwrap();
        match ServerOpcodeMessage::read(&mut client).unwrap() {
            ServerOpcodeMessage::CMD_AUTH_LOGON_PROOF(CMD_AUTH_LOGON_PROOF_Server::Success {
                ..
            }) => {}
            other => panic!("expected proof success, got {other:?}"),
        }

        // Ask for the realm list too, so `realms` — the third world-side call — is exercised.
        wow_login_messages::version_8::CMD_REALM_LIST_Client {}
            .write(&mut client)
            .unwrap();
        match ServerOpcodeMessage::read(&mut client).unwrap() {
            ServerOpcodeMessage::CMD_REALM_LIST(_) => {}
            other => panic!("expected realm list, got {other:?}"),
        }

        // EVERY store call that then talks to the world shard must be told the username. Review
        // caught `bound_identity` and `realms` still running on realm-core's id after
        // `save_session` was fixed, so this asserts the whole set, not the one method.
        for method in ["bound_identity", "save_session", "realms"] {
            assert_eq!(
                store.told(method).as_deref(),
                Some("TEST"),
                "{method} must receive the normalized username — it is the only key valid across \
                 the realm-core and world databases, and {method} resolves a WORLD-shard id"
            );
        }
        drop(client);
        server.join().unwrap();
    }

    #[test]
    fn a_world_side_id_is_re_resolved_from_the_username_never_carried_from_realm_core() {
        // The rule behind the assertions above. Realm-core says this account is id 7; the
        // world shard issued id 4 for the same username. Every world-side call must use 4.
        assert_eq!(
            world_account_id(7, "TEST", false, "world", || Ok(Some(4))).unwrap(),
            4,
            "the world shard's own id for the username, never realm-core's"
        );
    }

    #[test]
    fn an_unconfigured_realm_core_passes_the_id_through_without_a_lookup() {
        // One database ⇒ the two ids ARE the same id. Not merely equal-valued: the lookup must not
        // even run, so the unconfigured logon costs exactly what it costs today.
        let id = world_account_id(7, "TEST", true, "world", || {
            panic!("no cross-database lookup may happen when realm-core is unconfigured")
        })
        .unwrap();
        assert_eq!(id, 7);
    }

    #[test]
    fn an_account_missing_on_the_world_shard_is_an_error_not_a_reused_realm_core_id() {
        // The dangerous fallback, stated as a test. If the world shard has no row for this
        // username, reusing realm-core's id would name whichever account the world shard happens
        // to have issued that id to — a different player. Refuse instead.
        let err = world_account_id(7, "TEST", false, "world", || Ok(None)).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("TEST") && msg.contains("world"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn unknown_account_fails_cleanly() {
        let store = InMemoryStore {
            account: LogonAccount {
                id: 1,
                salt: [0; 32],
                verifier: [0; 32],
                banned: false,
            },
            username: "TEST".into(),
            saved: Mutex::new(None),
            usernames: Mutex::new(Vec::new()),
        };
        let (mut client, server_end) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut s = server_end;
            // store moved in; handler returns when client drops.
            let _ = handle_logon(&mut s, &store);
        });

        challenge_client("NOBODY").write(&mut client).unwrap();
        match ServerOpcodeMessage::read(&mut client).unwrap() {
            ServerOpcodeMessage::CMD_AUTH_LOGON_CHALLENGE(
                CMD_AUTH_LOGON_CHALLENGE_Server::FailUnknownAccount,
            ) => {}
            other => panic!("expected FailUnknownAccount, got {other:?}"),
        }
        drop(client);
        server.join().unwrap();
    }

    /// A provisioned account (real salt/verifier for `password`), for the negative-path tests.
    fn provisioned_store(username: &str, password: &str, banned: bool) -> InMemoryStore {
        let v = SrpVerifier::from_username_and_password(ns(username), ns(password));
        InMemoryStore {
            account: LogonAccount {
                id: 1,
                salt: *v.salt(),
                verifier: *v.password_verifier(),
                banned,
            },
            username: username.into(),
            saved: Mutex::new(None),
            usernames: Mutex::new(Vec::new()),
        }
    }

    #[test]
    fn banned_account_challenge_replies_fail_banned() {
        let store = provisioned_store("TEST", "PASSWORD", true);
        let (mut client, server_end) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut s = server_end;
            let _ = handle_logon(&mut s, &store);
        });

        challenge_client("TEST").write(&mut client).unwrap();
        match ServerOpcodeMessage::read(&mut client).unwrap() {
            ServerOpcodeMessage::CMD_AUTH_LOGON_CHALLENGE(
                CMD_AUTH_LOGON_CHALLENGE_Server::FailBanned,
            ) => {}
            other => panic!("expected FailBanned, got {other:?}"),
        }
        drop(client);
        server.join().unwrap();
    }

    #[test]
    fn unprovisioned_all_zero_verifier_is_rejected_as_unknown_account() {
        // The `init` placeholder account has an all-zero verifier. wow_srp's key math PANICS on an
        // all-zero key, so the challenge handler must reject it cleanly BEFORE the SRP6 setup — a
        // regression here is a gateway crash, not a wrong reply.
        let store = InMemoryStore {
            account: LogonAccount {
                id: 1,
                salt: [9; 32],
                verifier: [0; 32],
                banned: false,
            },
            username: "TEST".into(),
            saved: Mutex::new(None),
            usernames: Mutex::new(Vec::new()),
        };
        let (mut client, server_end) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut s = server_end;
            let _ = handle_logon(&mut s, &store);
        });

        challenge_client("TEST").write(&mut client).unwrap();
        match ServerOpcodeMessage::read(&mut client).unwrap() {
            ServerOpcodeMessage::CMD_AUTH_LOGON_CHALLENGE(
                CMD_AUTH_LOGON_CHALLENGE_Server::FailUnknownAccount,
            ) => {}
            other => {
                panic!("expected FailUnknownAccount for the unprovisioned account, got {other:?}")
            }
        }
        drop(client);
        server.join().unwrap();
    }

    #[test]
    fn wrong_password_proof_replies_fail_incorrect_password() {
        // The account's verifier is for "PASSWORD"; the client computes its proof with "WRONGPASS",
        // so `into_server` must reject the digest and the server replies FailIncorrectPassword
        // (a clean per-attempt failure — the connection stays open for a retry).
        let store = provisioned_store("TEST", "PASSWORD", false);
        let (mut client, server_end) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut s = server_end;
            let _ = handle_logon(&mut s, &store);
        });

        challenge_client("TEST").write(&mut client).unwrap();
        let (g, n, s_salt, server_pubkey) = match ServerOpcodeMessage::read(&mut client).unwrap() {
            ServerOpcodeMessage::CMD_AUTH_LOGON_CHALLENGE(
                CMD_AUTH_LOGON_CHALLENGE_Server::Success {
                    generator,
                    large_safe_prime,
                    salt,
                    server_public_key,
                    ..
                },
            ) => (generator, large_safe_prime, salt, server_public_key),
            other => panic!("expected challenge success, got {other:?}"),
        };
        let n: [u8; 32] = n.try_into().unwrap();
        let challenge = SrpClientChallenge::new(
            ns("TEST"),
            ns("WRONGPASS"), // ≠ the provisioned password
            g[0],
            n,
            PublicKey::from_le_bytes(server_pubkey).unwrap(),
            s_salt,
        );
        wow_login_messages::version_3::CMD_AUTH_LOGON_PROOF_Client {
            client_public_key: *challenge.client_public_key(),
            client_proof: *challenge.client_proof(),
            crc_hash: [0u8; 20],
            telemetry_keys: vec![],
            security_flag:
                wow_login_messages::version_3::CMD_AUTH_LOGON_PROOF_Client_SecurityFlag::None,
        }
        .write(&mut client)
        .unwrap();

        match ServerOpcodeMessage::read(&mut client).unwrap() {
            ServerOpcodeMessage::CMD_AUTH_LOGON_PROOF(
                CMD_AUTH_LOGON_PROOF_Server::FailIncorrectPassword,
            ) => {}
            other => panic!("expected FailIncorrectPassword, got {other:?}"),
        }
        drop(client);
        server.join().unwrap();
    }

    #[test]
    fn proof_before_a_challenge_is_a_protocol_error() {
        // CMD_AUTH_LOGON_PROOF with no pending challenge → `handle_logon` returns Err (there is no
        // SRP state to verify against) instead of limping on.
        let store = provisioned_store("TEST", "PASSWORD", false);
        let (mut client, server_end) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut s = server_end;
            handle_logon(&mut s, &store)
        });

        wow_login_messages::version_3::CMD_AUTH_LOGON_PROOF_Client {
            client_public_key: [1u8; 32],
            client_proof: [0u8; 20],
            crc_hash: [0u8; 20],
            telemetry_keys: vec![],
            security_flag:
                wow_login_messages::version_3::CMD_AUTH_LOGON_PROOF_Client_SecurityFlag::None,
        }
        .write(&mut client)
        .unwrap();

        let result = server.join().unwrap();
        let err = result.expect_err("an out-of-order proof must be a protocol error");
        assert!(
            err.to_string().contains("before a challenge"),
            "unexpected error: {err:#}"
        );
        drop(client);
    }

    #[test]
    fn realm_list_before_a_completed_proof_is_empty() {
        // Without a completed proof the account is unknown — the realm list must advertise nothing
        // (not leak the realm to unauthenticated probes).
        let store = provisioned_store("TEST", "PASSWORD", false);
        let (mut client, server_end) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut s = server_end;
            let _ = handle_logon(&mut s, &store);
        });

        wow_login_messages::version_8::CMD_REALM_LIST_Client {}
            .write(&mut client)
            .unwrap();
        match ServerOpcodeMessage::read(&mut client).unwrap() {
            ServerOpcodeMessage::CMD_REALM_LIST(reply) => {
                assert!(reply.realms.is_empty(), "no realms before authentication");
            }
            other => panic!("expected an empty realm list, got {other:?}"),
        }
        drop(client);
        server.join().unwrap();
    }

    #[test]
    fn to_realm_maps_the_realm_type_code() {
        fn realm(realm_type: u32) -> RealmInfo {
            RealmInfo {
                id: 1,
                name: "LyraCore".into(),
                address: "127.0.0.1:8085".into(),
                realm_type,
                population: 0.0,
                number_of_characters: 0,
            }
        }
        assert_eq!(to_realm(&realm(1)).realm_type, RealmType::PlayerVsPlayer);
        assert_eq!(to_realm(&realm(6)).realm_type, RealmType::Roleplaying);
        assert_eq!(
            to_realm(&realm(8)).realm_type,
            RealmType::RoleplayingPlayerVsPlayer
        );
        assert_eq!(
            to_realm(&realm(0)).realm_type,
            RealmType::PlayerVsEnvironment
        );
        // Any other code (not 1/6/8) falls back to PvE, not just the canonical 0.
        assert_eq!(
            to_realm(&realm(99)).realm_type,
            RealmType::PlayerVsEnvironment
        );
    }

    // ===================================================================================
    //  Credential normalization, store-failure handling, and replay
    // ===================================================================================

    /// Drive a complete SRP6 handshake for `typed_account` against a store provisioned under
    /// `stored_account`/`password`, and hand back the store so the caller can inspect what it was
    /// told. Panics with a specific message at whichever step diverges.
    fn handshake_as(
        stored_account: &str,
        password: &str,
        typed_account: &str,
    ) -> std::sync::Arc<InMemoryStore> {
        let store = std::sync::Arc::new(provisioned_store(stored_account, password, false));
        let (mut client, server_end) = UnixStream::pair().unwrap();
        let server_store = store.clone();
        let server = std::thread::spawn(move || {
            let mut s = server_end;
            handle_logon(&mut s, server_store.as_ref())
        });

        challenge_client(typed_account).write(&mut client).unwrap();
        let (g, n, s_salt, server_pubkey) = match ServerOpcodeMessage::read(&mut client).unwrap() {
            ServerOpcodeMessage::CMD_AUTH_LOGON_CHALLENGE(
                CMD_AUTH_LOGON_CHALLENGE_Server::Success {
                    generator,
                    large_safe_prime,
                    salt,
                    server_public_key,
                    ..
                },
            ) => (generator, large_safe_prime, salt, server_public_key),
            other => panic!(
                "account {typed_account:?} did not resolve to the account provisioned as \
                 {stored_account:?}: got {other:?}"
            ),
        };
        let n: [u8; 32] = n.try_into().unwrap();
        // The real client uppercases before computing its proof, so the proof is always over the
        // UPPERCASED name — that is the contract `gateway provision` matches by uppercasing too.
        let challenge = SrpClientChallenge::new(
            ns(&typed_account.to_uppercase()),
            ns(password),
            g[0],
            n,
            PublicKey::from_le_bytes(server_pubkey).unwrap(),
            s_salt,
        );
        wow_login_messages::version_3::CMD_AUTH_LOGON_PROOF_Client {
            client_public_key: *challenge.client_public_key(),
            client_proof: *challenge.client_proof(),
            crc_hash: [0u8; 20],
            telemetry_keys: vec![],
            security_flag:
                wow_login_messages::version_3::CMD_AUTH_LOGON_PROOF_Client_SecurityFlag::None,
        }
        .write(&mut client)
        .unwrap();

        match ServerOpcodeMessage::read(&mut client).unwrap() {
            ServerOpcodeMessage::CMD_AUTH_LOGON_PROOF(CMD_AUTH_LOGON_PROOF_Server::Success {
                ..
            }) => {}
            other => panic!("proof for {typed_account:?} was rejected: {other:?}"),
        }

        // The realm list is the first authenticated read, and the last store call told the
        // account's name — so drive it too rather than stopping at the proof.
        wow_login_messages::version_8::CMD_REALM_LIST_Client {}
            .write(&mut client)
            .unwrap();
        match ServerOpcodeMessage::read(&mut client).unwrap() {
            ServerOpcodeMessage::CMD_REALM_LIST(reply) => assert!(
                !reply.realms.is_empty(),
                "an authenticated {typed_account:?} must be shown the realm"
            ),
            other => panic!("expected the realm list, got {other:?}"),
        }
        drop(client);
        server.join().unwrap().expect("the session ends cleanly");
        store
    }

    /// The provisioning↔logon contract, end to end and offline. `gateway provision` computes the
    /// verifier over the UPPERCASED username (`main.rs`'s `provision`), and `handle_challenge`
    /// uppercases whatever the client sent before looking the account up. Those two `to_uppercase()`
    /// calls are the same agreement written twice, in files that never reference each other.
    ///
    /// If either side stopped normalizing, an account provisioned as `TESTER` would simply not be
    /// found for a player who typed `tester` — the account exists, the password is right, and every
    /// login is refused as "unknown account". Asserting it through a REAL SRP6 handshake (not a
    /// string comparison) is what makes this catch a one-sided change: the verifier is bound to the
    /// name, so a mismatch fails the proof even if the lookup somehow succeeded.
    #[test]
    fn an_account_provisioned_uppercase_authenticates_whatever_case_the_player_types() {
        for typed in ["tester", "TESTER", "TeStEr", "tESTER"] {
            let store = handshake_as("TESTER", "PASSWORD", typed);
            assert_eq!(
                store.told("save_session").as_deref(),
                Some("TESTER"),
                "typing {typed:?} must resolve to the canonical uppercased account name before any \
                 store call — a store keyed on the raw typed name writes a second session row per \
                 capitalization"
            );
            assert_eq!(
                store.told("realms").as_deref(),
                Some("TESTER"),
                "the realm-list lookup must use the canonical name too"
            );
        }
    }

    /// The SRP6 material `gateway provision` writes is what this handler later authenticates
    /// against, and the two are computed in files that never meet. `provision` normalizes through
    /// `provision_cli::normalize_provision_credentials` and stores `salt`/`password_verifier`; this
    /// drives that exact derivation — the real function, not a re-typed copy — through a real
    /// handshake.
    ///
    /// The salt must be RANDOM per provision — re-provisioning the same account must not reproduce
    /// the same salt, or the verifier becomes a password-equivalent value that is identical across
    /// every server sharing this code, and a stolen `game_account` table becomes a rainbow table.
    /// Both independently-salted verifiers must still authenticate the same password.
    #[test]
    fn provisioning_the_same_credentials_twice_yields_different_salts_that_both_authenticate() {
        let (user, pass) =
            crate::provision_cli::normalize_provision_credentials("tester", b"password")
                .expect("the fixture credentials are valid");
        let first = SrpVerifier::from_username_and_password(user.clone(), pass.clone());
        let second = SrpVerifier::from_username_and_password(user, pass);
        assert_ne!(
            first.salt(),
            second.salt(),
            "provisioning must draw a FRESH random salt each time. A fixed salt makes the stored \
             verifier a password-equivalent constant shared by every deployment of this code."
        );
        assert_ne!(
            first.password_verifier(),
            second.password_verifier(),
            "different salts must produce different verifiers, or the salt is not reaching the \
             derivation at all"
        );

        // Both are equally valid credentials: the password authenticates against either.
        for (which, v) in [("first", &first), ("second", &second)] {
            let store = std::sync::Arc::new(InMemoryStore {
                account: LogonAccount {
                    id: 1,
                    salt: *v.salt(),
                    verifier: *v.password_verifier(),
                    banned: false,
                },
                username: "TESTER".into(),
                saved: Mutex::new(None),
                usernames: Mutex::new(Vec::new()),
            });
            let (mut client, server_end) = UnixStream::pair().unwrap();
            let server_store = store.clone();
            let server = std::thread::spawn(move || {
                let mut s = server_end;
                handle_logon(&mut s, server_store.as_ref())
            });

            challenge_client("tester").write(&mut client).unwrap();
            let (g, n, s_salt, server_pubkey) =
                match ServerOpcodeMessage::read(&mut client).unwrap() {
                    ServerOpcodeMessage::CMD_AUTH_LOGON_CHALLENGE(
                        CMD_AUTH_LOGON_CHALLENGE_Server::Success {
                            generator,
                            large_safe_prime,
                            salt,
                            server_public_key,
                            ..
                        },
                    ) => (generator, large_safe_prime, salt, server_public_key),
                    other => panic!("{which} provisioning was not accepted: {other:?}"),
                };
            assert_eq!(
                s_salt,
                *v.salt(),
                "the challenge must echo the salt provisioning stored, or the client derives its \
                 proof from the wrong one"
            );
            let n: [u8; 32] = n.try_into().unwrap();
            let challenge = SrpClientChallenge::new(
                ns("TESTER"),
                ns("PASSWORD"),
                g[0],
                n,
                PublicKey::from_le_bytes(server_pubkey).unwrap(),
                s_salt,
            );
            wow_login_messages::version_3::CMD_AUTH_LOGON_PROOF_Client {
                client_public_key: *challenge.client_public_key(),
                client_proof: *challenge.client_proof(),
                crc_hash: [0u8; 20],
                telemetry_keys: vec![],
                security_flag:
                    wow_login_messages::version_3::CMD_AUTH_LOGON_PROOF_Client_SecurityFlag::None,
            }
            .write(&mut client)
            .unwrap();
            match ServerOpcodeMessage::read(&mut client).unwrap() {
                ServerOpcodeMessage::CMD_AUTH_LOGON_PROOF(
                    CMD_AUTH_LOGON_PROOF_Server::Success { .. },
                ) => {}
                other => panic!(
                    "the {which} independently-salted provisioning must authenticate the same \
                     password, got {other:?}"
                ),
            }
            assert!(
                store.saved.lock().unwrap().is_some(),
                "a completed proof must persist the session key K"
            );
            drop(client);
            server.join().unwrap().expect("the session ends cleanly");
        }
    }

    /// The PROVISIONING↔LOGON seam, driven end to end offline.
    ///
    /// `provision_cli`'s tests assert what `normalize_provision_credentials` returns (`"alice"` →
    /// `"ALICE"`); this file's tests assert what `handle_logon` accepts. Neither on its own catches
    /// the failure that matters: the two normalizing the same input DIFFERENTLY. An account is
    /// provisioned once and authenticated forever after, so a divergence is not a compile error and
    /// not a failing unit test — it is "the account exists, the password is right, every login is
    /// refused as unknown", which is precisely the class that once cost a 200-account benchmark run.
    ///
    /// So: run the real provisioning derivation over the raw bytes an operator would pipe in, store
    /// exactly what it produces, and complete a real SRP6 handshake against it — typing the account
    /// name in every casing a player might use.
    #[test]
    fn credentials_from_the_provisioning_path_authenticate_through_a_real_handshake() {
        // The exact inputs `gateway provision tester --password-stdin` would carry: an unnormalized
        // username from argv, and a password as raw bytes off stdin.
        let (user, pass) =
            crate::provision_cli::normalize_provision_credentials("tester", b"my pass phrase")
                .expect("an ordinary username/password must be accepted by provisioning");
        let verifier = SrpVerifier::from_username_and_password(user.clone(), pass);

        for typed in ["TESTER", "tester", "TeStEr"] {
            let store = std::sync::Arc::new(InMemoryStore {
                account: LogonAccount {
                    id: 1,
                    salt: *verifier.salt(),
                    verifier: *verifier.password_verifier(),
                    banned: false,
                },
                // The account row is keyed by whatever provisioning canonicalized the name to.
                username: user.as_ref().to_string(),
                saved: Mutex::new(None),
                usernames: Mutex::new(Vec::new()),
            });
            let (mut client, server_end) = UnixStream::pair().unwrap();
            let server_store = store.clone();
            let server = std::thread::spawn(move || {
                let mut s = server_end;
                handle_logon(&mut s, server_store.as_ref())
            });

            challenge_client(typed).write(&mut client).unwrap();
            let (g, n, s_salt, server_pubkey) =
                match ServerOpcodeMessage::read(&mut client).unwrap() {
                    ServerOpcodeMessage::CMD_AUTH_LOGON_CHALLENGE(
                        CMD_AUTH_LOGON_CHALLENGE_Server::Success {
                            generator,
                            large_safe_prime,
                            salt,
                            server_public_key,
                            ..
                        },
                    ) => (generator, large_safe_prime, salt, server_public_key),
                    other => panic!(
                    "an account provisioned as {:?} was not found when the player typed {typed:?} \
                     — the provisioning and logon tiers disagree about normalization, which \
                     presents to every affected player as 'unknown account'. Got {other:?}",
                    user.as_ref()
                ),
                };
            let n: [u8; 32] = n.try_into().unwrap();
            // The client uppercases and normalizes the same way — the password is case-folded too,
            // which is why an operator may pipe it in any casing.
            let challenge = SrpClientChallenge::new(
                ns(typed),
                ns("MY PASS PHRASE"),
                g[0],
                n,
                PublicKey::from_le_bytes(server_pubkey).unwrap(),
                s_salt,
            );
            wow_login_messages::version_3::CMD_AUTH_LOGON_PROOF_Client {
                client_public_key: *challenge.client_public_key(),
                client_proof: *challenge.client_proof(),
                crc_hash: [0u8; 20],
                telemetry_keys: vec![],
                security_flag:
                    wow_login_messages::version_3::CMD_AUTH_LOGON_PROOF_Client_SecurityFlag::None,
            }
            .write(&mut client)
            .unwrap();
            match ServerOpcodeMessage::read(&mut client).unwrap() {
                ServerOpcodeMessage::CMD_AUTH_LOGON_PROOF(
                    CMD_AUTH_LOGON_PROOF_Server::Success { .. },
                ) => {}
                other => panic!(
                    "the password provisioning derived its verifier from must authenticate when \
                     typed as {typed:?}; got {other:?}"
                ),
            }
            drop(client);
            server.join().unwrap().expect("the session ends cleanly");
        }
    }

    /// The other half of normalization: it must not merge accounts that are genuinely different.
    /// Case is the ONLY thing folded — `TESTER` and `TESTER2` stay distinct.
    #[test]
    fn normalization_folds_case_only_and_never_merges_two_different_accounts() {
        let store = provisioned_store("TESTER", "PASSWORD", false);
        let (mut client, server_end) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut s = server_end;
            let _ = handle_logon(&mut s, &store);
        });
        challenge_client("tester2").write(&mut client).unwrap();
        match ServerOpcodeMessage::read(&mut client).unwrap() {
            ServerOpcodeMessage::CMD_AUTH_LOGON_CHALLENGE(
                CMD_AUTH_LOGON_CHALLENGE_Server::FailUnknownAccount,
            ) => {}
            other => panic!("`tester2` must not resolve to the `TESTER` account, got {other:?}"),
        }
        drop(client);
        server.join().unwrap();
    }

    /// A store that fails every read — a dead realm-core, a torn subscription, a wrong-length blob.
    struct FailingStore {
        on: &'static str,
        account: LogonAccount,
        username: String,
    }

    impl LogonStore for FailingStore {
        fn account(&self, username: &str) -> Result<Option<LogonAccount>> {
            if self.on == "account" {
                return Err(anyhow!("realm-core unreachable"));
            }
            Ok((username == self.username).then(|| self.account.clone()))
        }
        fn bound_identity(&self, _account_id: u64, _username: &str) -> Result<[u8; 32]> {
            if self.on == "bound_identity" {
                return Err(anyhow!("realm-core unreachable"));
            }
            Ok([7u8; 32])
        }
        fn save_session(
            &self,
            _id: u64,
            _username: &str,
            _key: &[u8; 40],
            _b: [u8; 32],
        ) -> Result<()> {
            if self.on == "save_session" {
                return Err(anyhow!("realm-core unreachable"));
            }
            Ok(())
        }
        fn realms(&self, _account_id: u64, _username: &str) -> Result<Vec<RealmInfo>> {
            if self.on == "realms" {
                return Err(anyhow!("realm-core unreachable"));
            }
            Ok(Vec::new())
        }
    }

    fn failing_store(on: &'static str) -> FailingStore {
        let v = SrpVerifier::from_username_and_password(ns("TEST"), ns("PASSWORD"));
        FailingStore {
            on,
            account: LogonAccount {
                id: 1,
                salt: *v.salt(),
                verifier: *v.password_verifier(),
                banned: false,
            },
            username: "TEST".into(),
        }
    }

    /// An INFRASTRUCTURE failure during the challenge must not be reported to the client as
    /// `FailUnknownAccount`. Those two conditions look identical from the client's seat and are
    /// opposites for an operator: one means "you typed the wrong name", the other means "realm-core
    /// is down and every account on this realm is affected".
    ///
    /// `handle_challenge` uses `store.account(..)?`, so the error propagates out of `handle_logon`
    /// and the connection drops — loudly, in the log, with the store's own message. The regression
    /// this guards is someone "helpfully" turning that `?` into `.unwrap_or(None)`, which would
    /// turn a realm-wide outage into a silent, per-player wrong-password story.
    #[test]
    fn a_store_failure_during_the_challenge_is_an_error_never_an_unknown_account_reply() {
        let store = failing_store("account");
        let (mut client, server_end) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut s = server_end;
            handle_logon(&mut s, &store)
        });
        challenge_client("TEST").write(&mut client).unwrap();

        // Nothing may be sent: the handler returns Err before writing a reply, so the client sees
        // a closed socket rather than a verdict on its credentials.
        let seen = ServerOpcodeMessage::read(&mut client);
        assert!(
            seen.is_err(),
            "a store outage must not answer the challenge at all; the client was told {seen:?}"
        );
        let err = server
            .join()
            .unwrap()
            .expect_err("a store outage must fail the logon session");
        assert!(
            format!("{err:#}").contains("realm-core unreachable"),
            "the store's own message must reach the operator's log: {err:#}"
        );
        drop(client);
    }

    /// The same rule after a CORRECT proof: the credentials verified, and the realm list read then
    /// failed. Answering with an empty realm list would show the player "no realms available" — a
    /// dead end they cannot act on and an operator cannot see. It must be an error.
    #[test]
    fn a_realm_list_read_failure_after_a_good_proof_fails_loudly_not_with_an_empty_list() {
        let store = failing_store("realms");
        let (mut client, server_end) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut s = server_end;
            handle_logon(&mut s, &store)
        });

        challenge_client("TEST").write(&mut client).unwrap();
        let (g, n, s_salt, server_pubkey) = match ServerOpcodeMessage::read(&mut client).unwrap() {
            ServerOpcodeMessage::CMD_AUTH_LOGON_CHALLENGE(
                CMD_AUTH_LOGON_CHALLENGE_Server::Success {
                    generator,
                    large_safe_prime,
                    salt,
                    server_public_key,
                    ..
                },
            ) => (generator, large_safe_prime, salt, server_public_key),
            other => panic!("expected challenge success, got {other:?}"),
        };
        let n: [u8; 32] = n.try_into().unwrap();
        let challenge = SrpClientChallenge::new(
            ns("TEST"),
            ns("PASSWORD"),
            g[0],
            n,
            PublicKey::from_le_bytes(server_pubkey).unwrap(),
            s_salt,
        );
        wow_login_messages::version_3::CMD_AUTH_LOGON_PROOF_Client {
            client_public_key: *challenge.client_public_key(),
            client_proof: *challenge.client_proof(),
            crc_hash: [0u8; 20],
            telemetry_keys: vec![],
            security_flag:
                wow_login_messages::version_3::CMD_AUTH_LOGON_PROOF_Client_SecurityFlag::None,
        }
        .write(&mut client)
        .unwrap();
        // The proof itself succeeds...
        match ServerOpcodeMessage::read(&mut client).unwrap() {
            ServerOpcodeMessage::CMD_AUTH_LOGON_PROOF(CMD_AUTH_LOGON_PROOF_Server::Success {
                ..
            }) => {}
            other => panic!("the proof should have been accepted, got {other:?}"),
        }
        // ...and the realm list read is what fails.
        wow_login_messages::version_8::CMD_REALM_LIST_Client {}
            .write(&mut client)
            .unwrap();
        let err = server
            .join()
            .unwrap()
            .expect_err("a failed realm-list read must end the session, not send an empty list");
        assert!(
            format!("{err:#}").contains("realm-core unreachable"),
            "{err:#}"
        );
        drop(client);
    }

    /// A captured `CMD_AUTH_LOGON_PROOF` must not authenticate twice. `handle_logon` holds the SRP
    /// state in a `pending` slot and `take()`s it on use, so a replayed proof finds nothing to
    /// verify against and is the same protocol error as a proof with no challenge at all.
    ///
    /// Without the `take()`, the same captured proof bytes would re-derive the session key on
    /// demand — a replay attack against a plaintext-framed, pre-encryption handshake.
    #[test]
    fn a_replayed_proof_cannot_authenticate_a_second_time() {
        let store = provisioned_store("TEST", "PASSWORD", false);
        let (mut client, server_end) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut s = server_end;
            handle_logon(&mut s, &store)
        });

        challenge_client("TEST").write(&mut client).unwrap();
        let (g, n, s_salt, server_pubkey) = match ServerOpcodeMessage::read(&mut client).unwrap() {
            ServerOpcodeMessage::CMD_AUTH_LOGON_CHALLENGE(
                CMD_AUTH_LOGON_CHALLENGE_Server::Success {
                    generator,
                    large_safe_prime,
                    salt,
                    server_public_key,
                    ..
                },
            ) => (generator, large_safe_prime, salt, server_public_key),
            other => panic!("expected challenge success, got {other:?}"),
        };
        let n: [u8; 32] = n.try_into().unwrap();
        let challenge = SrpClientChallenge::new(
            ns("TEST"),
            ns("PASSWORD"),
            g[0],
            n,
            PublicKey::from_le_bytes(server_pubkey).unwrap(),
            s_salt,
        );
        let proof = wow_login_messages::version_3::CMD_AUTH_LOGON_PROOF_Client {
            client_public_key: *challenge.client_public_key(),
            client_proof: *challenge.client_proof(),
            crc_hash: [0u8; 20],
            telemetry_keys: vec![],
            security_flag:
                wow_login_messages::version_3::CMD_AUTH_LOGON_PROOF_Client_SecurityFlag::None,
        };
        proof.write(&mut client).unwrap();
        match ServerOpcodeMessage::read(&mut client).unwrap() {
            ServerOpcodeMessage::CMD_AUTH_LOGON_PROOF(CMD_AUTH_LOGON_PROOF_Server::Success {
                ..
            }) => {}
            other => panic!("the first proof must succeed, got {other:?}"),
        }

        // Byte-identical replay of the exact same proof.
        proof.write(&mut client).unwrap();
        let err = server
            .join()
            .unwrap()
            .expect_err("a replayed proof must be refused, not re-authenticated");
        assert!(
            err.to_string().contains("before a challenge"),
            "the replay must be rejected as having no pending SRP state; got: {err:#}"
        );
        drop(client);
    }

    /// The complement: a SECOND challenge must replace the pending SRP state, not be ignored in
    /// favour of the first. A client that retries its challenge (the 1.12.1 client does, on a
    /// timeout) computes its next proof against the SECOND server public key, so a handler that
    /// kept the first would reject a perfectly correct password.
    #[test]
    fn a_second_challenge_replaces_the_pending_srp_state_rather_than_stranding_the_client() {
        let store = provisioned_store("TEST", "PASSWORD", false);
        let (mut client, server_end) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut s = server_end;
            handle_logon(&mut s, &store)
        });

        // First challenge — its material is deliberately discarded.
        challenge_client("TEST").write(&mut client).unwrap();
        let first_pubkey = match ServerOpcodeMessage::read(&mut client).unwrap() {
            ServerOpcodeMessage::CMD_AUTH_LOGON_CHALLENGE(
                CMD_AUTH_LOGON_CHALLENGE_Server::Success {
                    server_public_key, ..
                },
            ) => server_public_key,
            other => panic!("expected challenge success, got {other:?}"),
        };

        // Second challenge on the same connection.
        challenge_client("TEST").write(&mut client).unwrap();
        let (g, n, s_salt, server_pubkey) = match ServerOpcodeMessage::read(&mut client).unwrap() {
            ServerOpcodeMessage::CMD_AUTH_LOGON_CHALLENGE(
                CMD_AUTH_LOGON_CHALLENGE_Server::Success {
                    generator,
                    large_safe_prime,
                    salt,
                    server_public_key,
                    ..
                },
            ) => (generator, large_safe_prime, salt, server_public_key),
            other => panic!("the second challenge must also be answered, got {other:?}"),
        };
        assert_ne!(
            server_pubkey, first_pubkey,
            "each challenge must mint fresh SRP6 material — reusing the ephemeral key across \
             challenges would make the handshake replayable"
        );

        // The proof is computed against the SECOND challenge, which is what a retrying client does.
        let n: [u8; 32] = n.try_into().unwrap();
        let challenge = SrpClientChallenge::new(
            ns("TEST"),
            ns("PASSWORD"),
            g[0],
            n,
            PublicKey::from_le_bytes(server_pubkey).unwrap(),
            s_salt,
        );
        wow_login_messages::version_3::CMD_AUTH_LOGON_PROOF_Client {
            client_public_key: *challenge.client_public_key(),
            client_proof: *challenge.client_proof(),
            crc_hash: [0u8; 20],
            telemetry_keys: vec![],
            security_flag:
                wow_login_messages::version_3::CMD_AUTH_LOGON_PROOF_Client_SecurityFlag::None,
        }
        .write(&mut client)
        .unwrap();
        match ServerOpcodeMessage::read(&mut client).unwrap() {
            ServerOpcodeMessage::CMD_AUTH_LOGON_PROOF(CMD_AUTH_LOGON_PROOF_Server::Success {
                ..
            }) => {}
            other => panic!(
                "a proof against the SECOND challenge must be accepted; the handler kept stale SRP \
                 state and answered {other:?}"
            ),
        }
        drop(client);
        server.join().unwrap().expect("the session ends cleanly");
    }

    /// An account name longer than SRP6's 16-byte `NormalizedString` limit. The client's field is a
    /// length-prefixed string with far more room than that, so this is reachable from the wire
    /// without any modified client — and it arrives BEFORE the account has proven anything.
    ///
    /// The lookup misses first, so the reply is a clean `FailUnknownAccount`. The point is that the
    /// oversized name never reaches `NormalizedString::new` as an unhandled case and never indexes
    /// past its fixed-size buffer.
    #[test]
    fn an_over_long_account_name_is_declined_cleanly_before_any_srp_math() {
        let store = provisioned_store("TEST", "PASSWORD", false);
        let (mut client, server_end) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut s = server_end;
            let _ = handle_logon(&mut s, &store);
        });
        challenge_client(&"A".repeat(200))
            .write(&mut client)
            .unwrap();
        match ServerOpcodeMessage::read(&mut client).unwrap() {
            ServerOpcodeMessage::CMD_AUTH_LOGON_CHALLENGE(
                CMD_AUTH_LOGON_CHALLENGE_Server::FailUnknownAccount,
            ) => {}
            other => panic!("expected a clean FailUnknownAccount, got {other:?}"),
        }
        drop(client);
        server.join().unwrap();
    }

    /// PROPERTY: the logon listener is the only surface on this server that parses bytes from
    /// a peer who has proven NOTHING — no session key, no header cipher, no account. Whatever
    /// arrives, `handle_logon` must return: `Ok(())` on a clean disconnect, `Err` on anything it
    /// cannot make sense of. It must never panic (an unwound thread per malformed connect is a
    /// denial-of-service with no rate limit) and never block forever.
    ///
    /// `main.rs`'s `boundary_panic_tripwire` already pins the LEXICAL half of this — that no
    /// `.unwrap()`/`panic!` is written in this file. This pins the behavioural half it cannot see: a
    /// panic reached through slicing, arithmetic, or a dependency. Both are needed; neither implies
    /// the other.
    ///
    /// Deterministic: a fixed seed, replayed identically every run (see
    /// `codec::property_tests::Rng` for why this is not `proptest`).
    #[test]
    fn any_byte_string_from_an_unauthenticated_peer_returns_rather_than_panicking() {
        use crate::codec::property_tests::Rng;
        let mut rng = Rng::new(0x4C4F_474F_4E00);
        for case in 0..300 {
            let store = provisioned_store("TEST", "PASSWORD", false);
            let (mut client, server_end) = UnixStream::pair().unwrap();
            let server = std::thread::spawn(move || {
                let mut s = server_end;
                handle_logon(&mut s, &store)
            });

            // Half pure noise; half noise behind a valid opcode byte, so the generator reaches the
            // per-message decoders instead of bouncing off the opcode dispatch every time.
            let mut buf = Vec::new();
            if case % 2 == 1 {
                buf.push([0u8, 1, 16][rng.below(3)]); // CHALLENGE / PROOF / REALM_LIST
            }
            let noise_len = rng.below(80);
            buf.extend_from_slice(&rng.bytes(noise_len));
            let _ = client.write_all(&buf);
            drop(client);

            // The assertion is that the thread finishes and yields a Result at all.
            let _resolved = server.join().unwrap_or_else(|_| {
                panic!(
                    "handle_logon panicked on unauthenticated input (case {case}): {buf:02X?} — a \
                     malformed connect must close the connection, not unwind the thread"
                )
            });
        }
    }
}

/// The realm list needs a live `Coordinator`, so its use of the shared resolution is pinned by a
/// scan rather than a unit test — the same reason `main.rs`'s startup wiring is.
#[cfg(test)]
mod realm_list_resolution_tripwire {
    use crate::test_scan::code_of;

    /// Both the realm list and the startup check must read one resolution. Inlining the override
    /// here again would let the warning describe an address no client was ever given.
    #[test]
    fn the_realm_list_advertises_through_the_shared_resolution() {
        let src = include_str!("mod.rs");
        let body = code_of(
            src,
            "fn realms(&self, account_id: u64, username: &str) -> Result<Vec<RealmInfo>> {",
        );
        assert!(
            body.contains("address: crate::config::advertised_realm_address_or(realm.address),"),
            "the realm list no longer resolves the advertised address through \
             `advertised_realm_address_or`, so the row and the startup warning can disagree. \
             Body was:\n{body}"
        );
    }
}
