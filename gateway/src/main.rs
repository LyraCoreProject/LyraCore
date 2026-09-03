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
mod fd_limit;
mod load_sample;
mod logon;
mod movement_batch_metrics;
mod provision_cli;
mod read_deadline;
mod realm_core;
mod stdb;
#[cfg(test)]
mod test_scan;
mod world;

use anyhow::Result;
use config::GatewayConfig;
use provision_cli::{parse_gateway_mode, read_password_line, GatewayMode};

// Heap profiling: `--features dhat-heap` swaps in dhat's allocator and
// writes dhat-heap.json at exit. Never for production — it records every allocation and is slow.
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

    // Bot-initiated (serendipity) invites: a playerbot's goal tick has no client and no
    // player connection to ride, so it is picked up here — on the coordinator, independent of any
    // session — rather than from a per-player relay.
    coordinator.spawn_bot_invite_relay();

    // Session-less Shard crossings, armed here for the same reason: a bot following its party
    // through a portal has no loading screen for the escrowed transfer to run inside.
    coordinator.spawn_bot_transfer_relay();

    // A Character deleted on a Shard (a despawned bot, a deleted character) must also leave the
    // party realm-core still lists it in; no client and no Shard reducer can reach realm-core.
    coordinator.spawn_character_gone_relay();

    // Keep this gateway's lease alive. The module's lease reaper despawns every session bound to
    // a lease that stops heartbeating, so the heartbeat is what bounds ghost lifetime after a
    // gateway crash — and its ABSENCE while sessions are bound is what would despawn a healthy
    // gateway's players, which is why it spawns unconditionally at startup, not lazily on first
    // use.
    coordinator.spawn_gateway_heartbeat();

    // Run both listeners concurrently. Each admitted socket gets its own blocking task.
    let logon = tokio::spawn(logon::run(cfg.clone(), coordinator.clone()));
    let world = tokio::spawn(world::run(cfg.clone(), coordinator.clone()));

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
        return Ok(());
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

    // REALM-CORE's copy, which is the one that can actually log you in. The SRP6 challenge is
    // answered from realm-core and never from a world shard's cache — pinned by
    // `the_srp6_challenge_material_comes_from_realm_core_never_the_world_shards_cache`
    // (`realm_core.rs`). Without this write an account provisioned on a SHARDED realm exists,
    // reports success, and can never log in: the logon server looks it up on realm-core and finds
    // nothing. It cost a whole capacity-benchmark run (200 accounts, every login refused) before
    // anyone read the account tables on both databases.
    //
    // Unsharded, `realm_core()` IS this handle and the second call would be a duplicate, so it is
    // skipped by name. `?` rather than best-effort: a provision that half-succeeded is exactly the
    // state this comment exists to describe, and reporting success for it is what made the failure
    // so hard to see.
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

/// Bot-invite-relay wiring tripwires: five behavioural tests (`gateway/src/world/party_tests.rs`)
/// already pin `world::party::run_bot_invite` — a pure function called directly, with no coordinator, no
/// subscription, no `main` in the loop. Nothing in that suite would notice any of the SIX call sites
/// below vanishing; a live coordinator (the only thing that WOULD notice) is out of reach for a unit
/// test. So each site's PRESENCE — not its sense, there is no boolean guard here to invert — is pinned
/// by a source scan, following `character_fence_tripwire`'s precedent and `test_scan::code_of`'s
/// comment-stripping (a bare `.contains()` on an un-stripped body is exactly what a trailing-comment
/// needle defeats). Tests 4-5 additionally pin ORDER, not just presence — the
/// find-offset ordering-tripwire style `world_store.rs`'s `routing_call_site_tests` use — because
/// the reconnect hook only works if it runs after the connection swap it depends on.
///
/// Confirmed by mutation: deleting each of the six independently turns exactly one of these red, with
/// every other suite (including the 5 `run_bot_invite` tests) staying fully green.
/// A provisioned account has to exist on BOTH databases, for two unrelated reasons, and dropping
/// either one produces a failure that looks like something else entirely.
///
/// This is a source scan for the same reason the tripwires below are: `provision` needs a live
/// `Coordinator` against a real node, which no unit test can construct. A scan cannot catch a sense
/// inversion — but there is no boolean here to invert, only two call sites whose *presence* is the
/// whole behaviour, which is precisely what a scan does catch.
#[cfg(test)]
mod provision_writes_both_databases_tripwire {
    use crate::test_scan::code_of;

    /// Losing the realm-core write makes every account on a SHARDED realm unloggable while
    /// `provision` still reports success: the SRP6 challenge is answered from realm-core
    /// (`realm_core.rs::the_srp6_challenge_material_comes_from_realm_core_never_the_world_shards_cache`),
    /// so the logon server looks the account up there and finds nothing. It cost a full
    /// capacity-benchmark run — 200 accounts, every login refused, `rung 50: 0 in world, 50 failed`.
    #[test]
    fn provision_writes_the_account_to_realm_core_too() {
        let src = include_str!("main.rs");
        let body = code_of(src, "async fn provision(cfg: &GatewayConfig, username: &str, password: &[u8]) -> Result<()> {");
        assert!(
            body.contains("let rc = coordinator.realm_core()?;")
                && body.contains("rc.provision_account(&user, &salt, &pw_verifier)?;"),
            "`provision` no longer writes the account to realm-core. On a sharded realm the account \
             is then invisible to the logon server — provisioning reports success and every login \
             is refused. Body was:\n{body}"
        );
    }

    /// And losing the world-shard write breaks the other half: `Account.id` is `#[auto_inc]`, a
    /// per-database surrogate key, and `characters`/`player_login`/`create_character` all mean the
    /// WORLD shard's id. Without it the account authenticates and then owns no characters.
    #[test]
    fn provision_still_writes_the_account_to_the_world_shard() {
        let src = include_str!("main.rs");
        let body = code_of(src, "async fn provision(cfg: &GatewayConfig, username: &str, password: &[u8]) -> Result<()> {");
        assert!(
            body.contains("coordinator.provision_account(&user, &salt, &pw_verifier)?;"),
            "`provision` no longer writes the account to the world shard — the `#[auto_inc]` \
             account id that owns characters is never created there. Body was:\n{body}"
        );
    }
}

/// The predicate's cases are pinned directly in `config::realm_address_tests`. What no unit test can
/// reach is whether anything ever ASKS it: the check needs a live coordinator, so its presence in
/// `main` is pinned by a scan the way every other startup wiring here is.
#[cfg(test)]
mod advertised_address_warning_wiring_tripwire {
    use crate::test_scan::code_of;

    #[test]
    fn main_checks_the_advertised_address_at_startup() {
        let src = include_str!("main.rs");
        let body = code_of(src, "async fn run() -> Result<()> {");
        assert!(
            body.contains("warn_on_unreachable_realm_address(&coordinator, &cfg);"),
            "`main` no longer checks the advertised realm address, so a realm seeded with the \
             loopback default starts entirely green while no remote client can enter the world. \
             Body was:\n{body}"
        );
    }

    /// The check must sit after the coordinator exists, or it reads no realm row and silently
    /// never fires.
    #[test]
    fn the_check_runs_after_the_coordinator_is_connected() {
        let src = include_str!("main.rs");
        let body = code_of(src, "async fn run() -> Result<()> {");
        let connect = body
            .find("let coordinator = stdb::Coordinator::connect(&cfg).await?;")
            .expect("main still connects the coordinator");
        let check = body
            .find("warn_on_unreachable_realm_address(")
            .expect("main still checks the advertised address");
        assert!(
            connect < check,
            "the advertised-address check now runs BEFORE the coordinator connects, so it reads \
             no realm row and the warning can never fire"
        );
    }

    /// A warning naming neither address, or no way to fix it, sends the reader back to the docs —
    /// which is the hour this line exists to save.
    #[test]
    fn the_warning_names_both_addresses_and_the_way_to_change_it() {
        let src = include_str!("main.rs");
        let body = code_of(src, "fn warn_on_unreachable_realm_address(");
        for needle in ["{advertised}", "cfg.world_bind", "set_realm_address"] {
            assert!(
                body.contains(needle),
                "the warning no longer carries `{needle}`, so it cannot be acted on without \
                 opening documentation. Body was:\n{body}"
            );
        }
    }
}

#[cfg(test)]
mod bot_invite_relay_wiring_tripwire {
    use crate::test_scan::code_of;

    /// 1. Nothing arms the relay at all unless `main` calls it once at startup.
    #[test]
    fn main_arms_the_bot_invite_relay_at_startup() {
        let src = include_str!("main.rs");
        let body = code_of(src, "async fn run() -> Result<()> {");
        assert!(
            body.contains("coordinator.spawn_bot_invite_relay();"),
            "`main` no longer calls `spawn_bot_invite_relay` — nothing subscribes \
             `game_bot_invite_intent` at all, so a bot's serendipity invite is never picked up by \
             ANY gateway, on ANY topology. Body was:\n{body}"
        );
    }

    /// 2. The subscription itself: without this the coordinator never even asks the node for the
    ///    table's rows, so `arm_bot_invite_relay`'s `on_insert` never fires regardless of #1.
    #[test]
    fn coordinator_subscribes_the_bot_invite_intent_table() {
        let src = include_str!("stdb/connection.rs");
        let body = code_of(
            src,
            "fn coordinator_queries(sharded_tables: bool) -> Vec<&'static str> {",
        );
        assert!(
            body.contains("\"SELECT * FROM game_bot_invite_intent\","),
            "`coordinator_queries` no longer subscribes `game_bot_invite_intent` — the table exists \
             and the module writes to it, but no gateway connection is asking for its rows. Body \
             was:\n{body}"
        );
    }

    /// 3. The consumer: the `on_insert` closure that turns a subscribed row into the actual party op.
    ///    Without this, #1 and #2 together still deliver the row to the coordinator's cache — and
    ///    nothing acts on it.
    #[test]
    fn the_on_insert_callback_actually_executes_the_intent() {
        let src = include_str!("stdb/subscriptions.rs");
        let body = code_of(src, "fn arm_bot_invite_relay(&self) {");
        let normalized: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            normalized.contains(
                "let (intent_id, op, inviter_guid, target_guid) = (row.id, row.op, \
                 row.inviter_guid, row.target_guid);"
            ) && normalized.contains(
                "crate::world::party::run_bot_invite_intent( &store, intent_id, op, \
                 inviter_guid, target_guid, )"
            ),
            "`arm_bot_invite_relay`'s `on_insert` callback no longer calls \
             `run_bot_invite_intent` with the subscribed row. A bot's \
             invite decision arrives at the gateway and is silently dropped. Body was:\n{body}"
        );
    }

    /// 4. The reconnect re-arm — the fix for the ONLY coordinator relay with no self-healing path.
    ///    Every sibling relay is armed from inside a per-session login, so a login after a reconnect
    ///    re-arms it implicitly; this one runs once at startup and would otherwise stay bound to a dead
    ///    `LiveConn` forever. The watchdog treats a SpacetimeDB migration — i.e. a routine module
    ///    republish — as a reconnect, so losing this hook means serendipity invites work for one session
    ///    and then go silent, logged at `debug!` where nobody sees it. That is the exact failure this
    ///    whole issue is about, so the fix for it gets pinned too.
    #[test]
    fn spawn_bot_invite_relay_installs_the_reconnect_hook() {
        let src = include_str!("stdb/subscriptions.rs");
        let body = code_of(src, "pub fn spawn_bot_invite_relay(&self) {");
        assert!(
            body.contains("on_reconnect") && body.contains("arm_bot_invite_relay();"),
            "`spawn_bot_invite_relay` no longer installs the per-shard `on_reconnect` re-arm. The \
             relay still works until the first coordinator reconnect, then goes permanently silent \
             with no error — the same symptom, and the same invisibility, as the bug this relay's own fix addressed. \
             Body was:\n{body}"
        );
    }

    /// 5. The other half of #4: the watchdog has to actually CALL the hook, and call it AFTER the
    ///    connection swap — the hook re-reads `coord()`, so invoking it before the swap would re-arm on
    ///    the dead connection and look identical to not re-arming at all. Position is asserted, not just
    ///    presence, in the style of `world_store.rs`'s find-offset ordering tripwires.
    #[test]
    fn the_watchdog_invokes_the_reconnect_hook_after_the_swap() {
        let src = include_str!("stdb/connection.rs");
        let body = code_of(src, "fn spawn_coordinator_watchdog(");
        let replacement_at = body.find("replace_live_conn(").expect(
            "the watchdog no longer installs the fresh LiveConn through the shared replacement \
             operation — reconnect is broken",
        );
        let hook_at = body.find("hook()").expect(
            "the watchdog no longer invokes the `on_reconnect` hook, so every one-shot coordinator \
             relay (today: the bot-invite and bot-transfer relays) silently dies at the first \
             reconnect",
        );
        assert!(
            replacement_at < hook_at,
            "the `on_reconnect` hook is invoked BEFORE the shared connection replacement. The hook re-reads \
             `coord()`, so it would re-arm on the dead connection — indistinguishable at runtime \
             from never re-arming, and just as silent."
        );
    }
}

/// The session-less crossing relay, pinned the same four ways as its invite sibling above. The
/// fifth check — that the watchdog invokes the reconnect hook after the connection swap — is the
/// same watchdog and the same hook list, so
/// `the_watchdog_invokes_the_reconnect_hook_after_the_swap` covers both relays.
#[cfg(test)]
mod bot_transfer_relay_wiring_tripwire {
    use crate::test_scan::code_of;

    /// 1. Nothing arms the relay at all unless `main` calls it once at startup.
    #[test]
    fn main_arms_the_bot_transfer_relay_at_startup() {
        let src = include_str!("main.rs");
        let body = code_of(src, "async fn run() -> Result<()> {");
        assert!(
            body.contains("coordinator.spawn_bot_transfer_relay();"),
            "`main` no longer calls `spawn_bot_transfer_relay` — nothing subscribes \
             `game_bot_transfer_intent`, so a session-less character can never cross a Shard \
             boundary on ANY topology. Body was:\n{body}"
        );
    }

    /// 2. The subscription itself: without it the coordinator never asks the node for the table's
    ///    rows, so the `on_insert` never fires regardless of #1.
    #[test]
    fn coordinator_subscribes_the_bot_transfer_intent_table() {
        let src = include_str!("stdb/connection.rs");
        let body = code_of(
            src,
            "fn coordinator_queries(sharded_tables: bool) -> Vec<&'static str> {",
        );
        assert!(
            body.contains("\"SELECT * FROM game_bot_transfer_intent\","),
            "`coordinator_queries` no longer subscribes `game_bot_transfer_intent` — the module \
             writes the row and no gateway connection is asking for it. Body was:\n{body}"
        );
    }

    /// 3. The consumer: the `on_insert` closure that turns a subscribed row into the actual
    ///    escrowed transfer. Without it the row reaches the coordinator's cache and nothing acts on
    ///    it.
    #[test]
    fn the_on_insert_callback_actually_executes_the_intent() {
        let src = include_str!("stdb/subscriptions.rs");
        let body = code_of(src, "fn arm_bot_transfer_relay(&self) {");
        assert!(
            body.contains("crate::world::transfer::run_bot_transfer("),
            "`arm_bot_transfer_relay`'s `on_insert` callback no longer calls `run_bot_transfer` — a \
             bot's crossing decision arrives at the gateway and is silently dropped, leaving it \
             behind on the Shard its party has left. Body was:\n{body}"
        );
    }

    /// 4. The reconnect re-arm. Armed once at startup, this relay has no per-session login to
    ///    re-arm it, and the watchdog treats a routine module republish as a reconnect: without the
    ///    hook, crossings work until the first republish and then stop, with no error anywhere.
    ///    That is the lesson the invite relay learned live, and it applies here unchanged.
    #[test]
    fn spawn_bot_transfer_relay_installs_the_reconnect_hook() {
        let src = include_str!("stdb/subscriptions.rs");
        let body = code_of(src, "pub fn spawn_bot_transfer_relay(&self) {");
        assert!(
            body.contains("on_reconnect") && body.contains("arm_bot_transfer_relay();"),
            "`spawn_bot_transfer_relay` no longer installs the per-shard `on_reconnect` re-arm. \
             Crossings keep working until the first coordinator reconnect and then go permanently \
             silent — the exact failure the bot-invite relay's own hook exists to prevent. Body \
             was:\n{body}"
        );
    }
}

/// The character-gone relay: the same three-part contract as the transfer relay above.
#[cfg(test)]
mod character_gone_relay_tripwires {
    use crate::test_scan::code_of;

    #[test]
    fn main_arms_the_character_gone_relay_at_startup() {
        let src = include_str!("main.rs");
        let body = code_of(src, "async fn run() -> Result<()> {");
        assert!(
            body.contains("coordinator.spawn_character_gone_relay();"),
            "`main` no longer calls `spawn_character_gone_relay` — a Character deleted on a Shard \
             stays in its realm-core party forever. Body was:\n{body}"
        );
    }

    #[test]
    fn the_on_delete_callback_leaves_the_realm_core_party() {
        let src = include_str!("stdb/subscriptions.rs");
        let body = code_of(src, "fn arm_character_gone_relay(&self) {");
        assert!(
            body.contains("realm_op::LEAVE") && body.contains("character_anywhere("),
            "`arm_character_gone_relay` no longer checks every Shard and leaves on realm-core — \
             either a moved Character is kicked mid-Transfer, or a gone one is never removed. Body \
             was:\n{body}"
        );
    }

    #[test]
    fn spawn_character_gone_relay_installs_the_reconnect_hook() {
        let src = include_str!("stdb/subscriptions.rs");
        let body = code_of(src, "pub fn spawn_character_gone_relay(&self) {");
        assert!(
            body.contains("on_reconnect") && body.contains("arm_character_gone_relay();"),
            "`spawn_character_gone_relay` no longer installs the per-shard `on_reconnect` re-arm; \
             it goes silent after the first module republish. Body was:\n{body}"
        );
    }
}

/// The untrusted-input boundaries stay free of panic constructs.
///
/// The release-boundary hardening audit found ZERO reachable panics in the three files BOUNDARIES
/// actually scans below: the UNAUTHENTICATED logon SRP6 handshake, the one hand-rolled client-byte decoder
/// (`codec/addon.rs`), and the shard-map/env parsing (`config.rs`) all propagate errors instead.
/// That is a property worth KEEPING, not a one-time finding: a single `.unwrap()` added to any of
/// these files turns a malformed packet from "this session closes" into "this session thread
/// unwinds", or a typo'd env var from "config falls back" into "the process aborts at startup".
///
/// This is narrower than the untrusted-input surface as a whole: it does NOT cover the
/// post-authentication packet path (`world/mod.rs`, including its frame-read loop) or the
/// DB-callback files (`stdb/subscriptions.rs`, `stdb/world_view.rs`), which also turn
/// client- and DB-supplied bytes into values. A manual read (2026-08-10) found those files' hot paths
/// disciplined, but their release code carries dozens of real `.unwrap()`s overall (mostly
/// `RwLock`/`Mutex` poisoning, not remote-input parsing) — far past what BOUNDARIES' all-or-nothing
/// per-file gate can absorb without turning this into a refactor. A clean spot in one of those
/// files is a fact about that file today, not a property this test enforces; extending BOUNDARIES
/// to them is future work, not a documentation fix.
///
/// A source scan is the right tool for the same reason as the tripwires above — the alternative is
/// driving thread/FD exhaustion and every malformed-frame shape against a live node — and it is a
/// sound one for THIS property, because the forbidden constructs are lexical. Its ceiling is equally
/// lexical: a panic reached through a helper in another file, or through indexing (`v[i]`), is
/// invisible to it. It makes the lazy path fail; it is not an analysis.
#[cfg(test)]
mod boundary_panic_tripwire {
    /// `(path, source, what it parses)`. Every file whose RELEASE half turns bytes or configuration
    /// this server does not control into values.
    const BOUNDARIES: &[(&str, &str, &str)] = &[
        (
            "logon/mod.rs",
            include_str!("logon/mod.rs"),
            "the UNAUTHENTICATED SRP6 handshake — every byte here arrives before the client has \
             proven anything at all",
        ),
        (
            "codec/addon.rs",
            include_str!("codec/addon.rs"),
            "the only hand-rolled decoder of CLIENT bytes in `codec/` (everything else there builds \
             server frames); its slice math is driven by a client-supplied NUL position",
        ),
        (
            "codec/auth_session.rs",
            include_str!("codec/auth_session.rs"),
            "the plaintext CMSG_AUTH_SESSION frame — read before the world peer has proven \
             anything, sized by a client header",
        ),
        (
            "config.rs",
            include_str!("config.rs"),
            "`LYRACORE_SHARD_MAP` / `LYRACORE_REALM_CORE` parsing — a \
             typo in an operator's env must degrade, never abort the process at startup",
        ),
    ];

    /// Constructs that turn a bad input into an unwind instead of an error. `.unwrap()` catches the
    /// `Option`/`Result` form; `unwrap_or` / `unwrap_or_else` / `unwrap_or_default` are deliberately
    /// NOT matched (they are the fix, not the problem) — hence the closing paren in the needle.
    const FORBIDDEN: &[&str] = &[
        ".unwrap()",
        ".expect(",
        "panic!(",
        "unreachable!(",
        "todo!(",
        "unimplemented!(",
    ];

    /// Drop every `#[cfg(test)]`-gated item, then every comment — so a doc comment that MENTIONS
    /// `panic!` and a test that legitimately unwraps both stay out of the scan.
    ///
    /// Brace-matched rather than truncate-at-the-first-`#[cfg(test)]`: these files may gate
    /// individual functions on `test` (config.rs historically did), not only a trailing
    /// `mod tests`, so a truncating scan would silently stop reading most of such a file.
    fn release_code(src: &str) -> String {
        let lines: Vec<&str> = src.lines().collect();
        let mut out = String::new();
        let mut i = 0;
        while i < lines.len() {
            let t = lines[i].trim();
            if t.starts_with("#[cfg(test)]") || t.starts_with("#[cfg(all(test") {
                i += 1;
                // Skip any further attributes / docs attached to the same item.
                while i < lines.len() {
                    let u = lines[i].trim();
                    if u.starts_with('#') || u.starts_with("//") || u.is_empty() {
                        i += 1;
                    } else {
                        break;
                    }
                }
                // Then the item itself: brace-matched, or up to the `;` of a `mod tests;`-style one.
                let mut depth = 0i32;
                let mut opened = false;
                while i < lines.len() {
                    let l = lines[i];
                    depth += l.matches('{').count() as i32;
                    if l.contains('{') {
                        opened = true;
                    }
                    depth -= l.matches('}').count() as i32;
                    i += 1;
                    if opened && depth <= 0 {
                        break;
                    }
                    if !opened && l.trim_end().ends_with(';') {
                        break;
                    }
                }
                continue;
            }
            out.push_str(crate::test_scan::strip_comment(lines[i]));
            out.push('\n');
            i += 1;
        }
        out
    }

    #[test]
    fn no_panic_construct_sits_on_an_untrusted_input_boundary() {
        for (path, src, what) in BOUNDARIES {
            let code = release_code(src);
            for needle in FORBIDDEN {
                assert!(
                    !code.contains(needle),
                    "`{path}` release code now contains `{needle}`. That file is {what}, so a panic \
                     there is a remote (or operator) input turning into an unwound thread instead of \
                     a closed connection and a log line — this file's boundary rule. Return an error \
                     (`?` / `map_err` / `ok_or_else`), or degrade with `unwrap_or*`. If it really is \
                     an impossible invariant, it does not belong in a file on this list: move the \
                     code, or take the file off BOUNDARIES with a written reason."
                );
            }
        }
    }

    /// The scan is only as good as its stripper: if `release_code` ever stopped removing
    /// `#[cfg(test)]` items or comments, the test above would fail on this crate's own tests and the
    /// tempting "fix" would be to shorten BOUNDARIES. Pin both behaviors directly.
    #[test]
    fn release_code_drops_cfg_test_items_and_comments() {
        let src = concat!(
            "fn keep() { let a = 1; }\n",
            "// a comment mentioning .unwrap()\n",
            "#[cfg(test)]\n",
            "mod tests {\n    fn t() { x.unwrap(); }\n}\n",
            "fn also_keep() {}\n",
        );
        let out = release_code(src);
        assert!(
            !out.contains(".unwrap()"),
            "stripper leaked an unwrap: {out}"
        );
        assert!(out.contains("fn keep()"), "stripper ate live code: {out}");
        assert!(
            out.contains("fn also_keep()"),
            "stripper did not resume after the cfg(test) item: {out}"
        );
    }
}
