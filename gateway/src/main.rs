//! Gateway — the edge/protocol tier. Terminates the unmodified WoW 1.12.1 client protocol
//! and translates it to/from SpacetimeDB. **Holds no game state**: everything it knows is
//! per-connection protocol scratch or a read of a config/session/entity table.
//!
//! Two listeners:
//!   - logon  (TCP 3724): SRP6 + realm list  -> `logon`
//!   - world  (TCP 8085): handshake, header crypto, char enum, login, movement -> `world`
//!
//! Each connection is a state machine over `stdb::Coordinator` (module reads + reducer calls,
//! `stdb/`) and the `codec` translators (`codec/`, one file per wire-message family); `world`
//! additionally owns the in-gateway AOI scoping (`stdb::aoi` + the shared `world_view` index)
//! and the stuck-state relay planes (quest/item/rep/xp/level-up/teleport). See `docs/danger-zones.md` §3 for how to
//! launch this against the real five-database realm — do not hand-roll the launch.

mod accept;
mod codec;
mod config;
#[cfg(test)]
use lyracore_test_support as durable_test_support;
mod fd_limit;
mod load_sample;
mod logon;
mod movement_batch_metrics;
mod provision_cli;
mod read_deadline;
mod realm_core;
mod stdb;
mod world;

use anyhow::Result;
use config::GatewayConfig;
use provision_cli::{parse_gateway_mode, read_password_line, GatewayMode};

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

/// Startup, in the order the three things have to happen:
///
/// 1. **Logging first**, so everything below is visible.
/// 2. **`RLIMIT_NOFILE` raised before anything opens a descriptor.** Best-effort; a failure logs and
///    continues — see `fd_limit`.
/// 3. **The runtime built by hand rather than by `#[tokio::main]`**, purely so
///    `max_blocking_threads` is a configured, logged number instead of tokio's silently inherited
///    512. Every other builder setting matches what `#[tokio::main]` does
///    (`new_multi_thread().enable_all()`). The same configured number sizes the listeners' shared,
///    non-waiting blocking-task capacity.
///
/// The `dhat` profiler guard is declared before the runtime so it drops *after* it — a profiler that
/// stops recording while the runtime is still tearing down would under-report the shutdown path.
fn main() -> Result<()> {
    #[cfg(feature = "dhat-heap")]
    let _dhat = dhat::Profiler::new_heap();

    env_logger::init();

    // A stock container gives us soft 1024 against a hard limit of 524288, and the
    // gateway dies at ~200 sessions with EMFILE while 512x of headroom sits unclaimed.
    fd_limit::raise_nofile_soft_to_hard();

    // THE ceiling on concurrent players: every World Session parks one blocking thread for its
    // whole life, and logon handshakes draw from the same pool. The listeners use the same number
    // for a shared non-waiting capacity, so they reject excess connections before submission.
    let max_blocking_threads = config::max_blocking_threads();
    log::info!(
        "tokio blocking pool: max_blocking_threads={max_blocking_threads} \
         (LYRACORE_MAX_BLOCKING_THREADS; default {}). One world session holds one thread for its \
         entire life, shared with in-flight logon handshakes. Excess connection tasks are rejected \
         before the blocking-task queue.",
        config::DEFAULT_MAX_BLOCKING_THREADS
    );

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .max_blocking_threads(max_blocking_threads)
        .build()?;
    runtime.block_on(run())
}

async fn run() -> Result<()> {
    let mode = parse_gateway_mode(std::env::args().skip(1))?;
    let cfg = GatewayConfig::from_env();

    // Bring-up helper: the password is accepted only over stdin so it never appears in argv or the
    // process list. It computes the same SRP6 salt/verifier the logon path verifies, calls the
    // `provision_account` reducer, and exits.
    if let GatewayMode::Provision { username } = mode {
        let stdin = std::io::stdin();
        let mut stdin = stdin.lock();
        let password = read_password_line(&mut stdin)?;
        return provision(&cfg, &username, &password).await;
    }

    let login_queue = std::sync::Arc::new(world::login_queue::LoginQueue::from_env()?);

    log::info!(
        "gateway starting: logon={} world={} db={}@{}",
        cfg.logon_bind,
        cfg.world_bind,
        cfg.module_name,
        cfg.stdb_uri
    );

    // The privileged coordination connection: reads config/session/account, calls
    // establish_session / provision_account. Stateless gateways share K through the DB.
    let coordinator = stdb::Coordinator::connect(&cfg).await?;

    // Here because this is the first point the realm row is readable.
    warn_on_unreachable_realm_address(&coordinator, &cfg);

    // Bot-initiated (serendipity) invites: a session-less Character's invite has no client and no
    // player connection to ride, so it is picked up here — on the coordinator, independent of any
    // session — rather than from a per-player relay.
    coordinator.spawn_bot_invite_relay();
    coordinator.spawn_party_command_relay();

    // Session-less Shard crossings, armed here for the same reason: a bot following its party
    // through a portal has no loading screen for the escrowed transfer to run inside.
    coordinator.spawn_bot_transfer_relay();

    // A Character deleted on a Shard (a despawned bot, a deleted character) must also leave the
    // party realm-core still lists it in; no client and no Shard reducer can reach realm-core.
    coordinator.spawn_character_gone_relay();

    // Realm-core can change a party no session acted in. Push each Roster Revision to the World
    // Shard mirrors that lack it.
    coordinator.spawn_roster_revision_relay();

    // Keep this gateway's lease alive. The module's lease reaper despawns every session bound to
    // a lease that stops heartbeating, so the heartbeat is what bounds ghost lifetime after a
    // gateway crash — and its ABSENCE while sessions are bound is what would despawn a healthy
    // gateway's players, which is why it spawns unconditionally at startup, not lazily on first
    // use.
    coordinator.spawn_gateway_heartbeat();

    // Run both listeners concurrently. Each admitted socket gets its own blocking task.
    let logon = tokio::spawn(logon::run(cfg.clone(), coordinator.clone()));
    let world = tokio::spawn(world::run(cfg.clone(), coordinator.clone(), login_queue));

    // Heap profiling (feature-gated): a `dhat::Profiler` writes its report on DROP,
    // and a signal-killed process never drops anything. So under this feature the gateway runs for
    // `LYRACORE_PROFILE_SECS` and then RETURNS from main, letting the profiler write dhat-heap.json.
    #[cfg(feature = "dhat-heap")]
    {
        let secs: u64 = std::env::var("LYRACORE_PROFILE_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(120);
        log::warn!("dhat-heap build: profiling for {secs}s, then exiting to write dhat-heap.json");
        tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
        logon.abort();
        world.abort();
        Ok(())
    }
    #[cfg(not(feature = "dhat-heap"))]
    {
        let (l, w) = tokio::try_join!(logon, world)?;
        l?;
        w?;
        Ok(())
    }
}

/// Report a realm advertising loopback while listening for remote clients — otherwise the symptom
/// is a login loop with every startup marker green. An unreadable row is not fatal: this only warns.
fn warn_on_unreachable_realm_address(coordinator: &stdb::Coordinator, cfg: &GatewayConfig) {
    let Ok(realm) = coordinator.realm() else {
        return;
    };
    let advertised = config::advertised_realm_address_or(realm.address);
    if config::advertised_address_is_unreachable(&advertised, &cfg.world_bind) {
        log::warn!(
            "realm advertises {advertised} but the world listener is bound to {} — a client that \
             logs in is told to connect to its own machine and will bounce back to realm select. \
             Set it on every database with the `set_realm_address` reducer.",
            cfg.world_bind
        );
    }
}

/// Compute SRP6 credentials for `username`/`password` and write them via the `provision_account`
/// reducer. The client uppercases the account name, so the verifier is computed over the uppercased
/// username to match the proof the client will send.
async fn provision(cfg: &GatewayConfig, username: &str, password: &[u8]) -> Result<()> {
    use wow_srp::server::SrpVerifier;

    let (username, password) = provision_cli::normalize_provision_credentials(username, password)?;
    let user = username.as_ref().to_owned();
    let verifier = SrpVerifier::from_username_and_password(username, password);
    let salt = *verifier.salt();
    let pw_verifier = *verifier.password_verifier();

    let coordinator = stdb::Coordinator::connect(cfg).await?;
    // The WORLD shard's copy. `Account.id` is `#[auto_inc]`, so it is a per-database surrogate key,
    // and `characters` / `player_login` / `create_character` all mean THIS database's id — a
    // character is owned by the world shard's account row.
    coordinator.provision_account(&user, &salt, &pw_verifier)?;

    // SRP6 credentials belong to Realm-core. Unsharded, it is the world handle already written.
    let rc = coordinator.realm_core()?;
    if rc.shard_name() != coordinator.shard_name() {
        rc.provision_account(&user, &salt, &pw_verifier)?;
    }
    log::info!(
        "provisioned account {user} on {} and {} (salt {} bytes, verifier {} bytes)",
        coordinator.shard_name(),
        rc.shard_name(),
        salt.len(),
        pw_verifier.len()
    );
    Ok(())
}
