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
mod stage_timing;
mod stdb;
mod world;

use anyhow::Result;
use config::GatewayConfig;
use provision_cli::{parse_gateway_mode, read_password_line, GatewayMode};

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

fn main() -> Result<()> {
    #[cfg(feature = "dhat-heap")]
    let _dhat = dhat::Profiler::new_heap();

    env_logger::init();

    fd_limit::raise_nofile_soft_to_hard();

    let max_blocking_threads = config::max_blocking_threads();
    log::info!(
        "tokio blocking pool: max_blocking_threads={max_blocking_threads} \
         (LYRACORE_MAX_BLOCKING_THREADS; default {}). World Session socket I/O is asynchronous. \
         Game actions, Relay encoding and logon handshakes use bounded blocking work.",
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

    if let GatewayMode::Provision {
        username,
        create_only,
    } = mode
    {
        let stdin = std::io::stdin();
        let mut stdin = stdin.lock();
        let password = read_password_line(&mut stdin)?;
        return provision(&cfg, &username, &password, create_only).await;
    }

    let login_queue = std::sync::Arc::new(world::login_queue::LoginQueue::from_env()?);

    log::info!(
        "gateway starting: logon={} world={} db={}@{}",
        cfg.logon_bind,
        cfg.world_bind,
        cfg.module_name,
        cfg.stdb_uri
    );

    let coordinator = stdb::Coordinator::connect(&cfg).await?;

    warn_on_unreachable_realm_address(&coordinator, &cfg);

    coordinator.spawn_bot_invite_relay();
    coordinator.spawn_party_command_relay();

    coordinator.spawn_bot_transfer_relay();

    coordinator.spawn_character_gone_relay();

    coordinator.spawn_roster_revision_relay();

    coordinator.spawn_gateway_heartbeat();

    let logon = tokio::spawn(logon::run(cfg.clone(), coordinator.clone()));
    let world = tokio::spawn(world::run(cfg.clone(), coordinator.clone(), login_queue));

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

async fn provision(
    cfg: &GatewayConfig,
    username: &str,
    password: &[u8],
    create_only: bool,
) -> Result<()> {
    use wow_srp::server::SrpVerifier;

    let (username, password) = provision_cli::normalize_provision_credentials(username, password)?;
    let user = username.as_ref().to_owned();
    let verifier = SrpVerifier::from_username_and_password(username, password);
    let salt = *verifier.salt();
    let pw_verifier = *verifier.password_verifier();

    let coordinator = stdb::Coordinator::connect(cfg).await?;
    let rc = coordinator.realm_core()?;
    if create_only {
        // Across Shards a later Refusal can leave a new Account only on Realm-core.
        rc.create_account(&user, &salt, &pw_verifier)?;
        if rc.shard_name() != coordinator.shard_name() {
            coordinator.create_account(&user, &salt, &pw_verifier)?;
        }
    } else {
        coordinator.provision_account(&user, &salt, &pw_verifier)?;
        if rc.shard_name() != coordinator.shard_name() {
            rc.provision_account(&user, &salt, &pw_verifier)?;
        }
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
