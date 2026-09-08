use super::*;
use crate::accept::BlockingTaskCapacity;
use crate::config::GatewayConfig;
use crate::durable_test_support::{poll_until, Standalone, POLL_TIMEOUT};
use crate::world::party::PartyOutcome;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct PrivateCli {
    config: PathBuf,
}

impl PrivateCli {
    fn new(owner_token: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let config = std::env::temp_dir().join(format!(
            "lyracore-party-cleanup-cli-{}-{nonce}.toml",
            std::process::id()
        ));
        std::fs::write(&config, format!("spacetimedb_token = {owner_token:?}\n")).unwrap();
        Self { config }
    }

    fn command(&self) -> Command {
        let binary = std::env::var_os("SPACETIME_BIN").unwrap_or_else(|| "spacetime".into());
        let mut command = Command::new(binary);
        command.args(["--config-path", self.config.to_str().unwrap()]);
        command
    }

    fn publish(&self, server: &str, database: &str) -> Output {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        self.command()
            .current_dir(workspace)
            .args([
                "publish",
                "-s",
                server,
                "--module-path",
                workspace.join("module").to_str().unwrap(),
                "--build-options=--features=debug_reducers",
                "-y",
                database,
            ])
            .output()
            .unwrap()
    }

    fn call(&self, server: &str, database: &str, reducer: &str, args: &[&str]) -> Output {
        let mut command = self.command();
        command.args(["call", "-s", server, database, reducer, "--"]);
        command.args(args);
        command.output().unwrap()
    }
}

impl Drop for PrivateCli {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.config);
    }
}

struct TopologyEnv {
    shard_map: Option<std::ffi::OsString>,
    realm_core: Option<std::ffi::OsString>,
}

impl TopologyEnv {
    fn install(shard_map: &str, realm_core: &str) -> Self {
        let previous = Self {
            shard_map: std::env::var_os("LYRACORE_SHARD_MAP"),
            realm_core: std::env::var_os("LYRACORE_REALM_CORE"),
        };
        std::env::set_var("LYRACORE_SHARD_MAP", shard_map);
        std::env::set_var("LYRACORE_REALM_CORE", realm_core);
        previous
    }
}

impl Drop for TopologyEnv {
    fn drop(&mut self) {
        match &self.shard_map {
            Some(value) => std::env::set_var("LYRACORE_SHARD_MAP", value),
            None => std::env::remove_var("LYRACORE_SHARD_MAP"),
        }
        match &self.realm_core {
            Some(value) => std::env::set_var("LYRACORE_REALM_CORE", value),
            None => std::env::remove_var("LYRACORE_REALM_CORE"),
        }
    }
}

fn reconciliation_is_idle(coordinator: &Coordinator) -> bool {
    !coordinator
        .1
        .party_reconciliation_requested
        .load(Ordering::Acquire)
        && !coordinator
            .1
            .party_reconciliation_running
            .load(Ordering::Acquire)
}

fn reconciliation_is_running(coordinator: &Coordinator) -> bool {
    coordinator
        .1
        .party_reconciliation_running
        .load(Ordering::Acquire)
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn another_gateway_waits_for_the_transfer_then_cleans_the_deleted_character() {
    const INSTANCES: &str = "party-cleanup-instances";
    const OTHER: &str = "party-cleanup-other";
    const REALM: &str = "party-cleanup-realm";
    const INSTANCES_SURVIVOR: u64 = 2;
    const OTHER_SURVIVOR: u64 = 1_000_000_001;

    let mut standalone = Standalone::start("party-cleanup-world");
    standalone.publish_module();
    let cli = PrivateCli::new(&standalone.owner_token());
    for database in [INSTANCES, OTHER, REALM] {
        let output = cli.publish(standalone.server(), database);
        standalone.assert_output_success(&output, &format!("publish {database}"));
    }
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["2000000000"]);
    for database in [INSTANCES, OTHER, REALM] {
        let output = cli.call(standalone.server(), database, "claim_operator", &[]);
        standalone.assert_output_success(&output, &format!("claim {database} operator"));
    }
    for database in [INSTANCES, OTHER] {
        let output = cli.call(
            standalone.server(),
            database,
            "debug_delete_character",
            &["1"],
        );
        standalone.assert_output_success(&output, &format!("clear Character 1 on {database}"));
    }
    for (database, base) in [(INSTANCES, "0"), (OTHER, "1000000000")] {
        let output = cli.call(standalone.server(), database, "install_guid_range", &[base]);
        standalone.assert_output_success(&output, &format!("install range on {database}"));
    }
    for (database, name) in [(INSTANCES, "PartyTwo"), (OTHER, "PartyOther")] {
        let output = cli.call(
            standalone.server(),
            database,
            "create_character",
            &["1", name, "1", "1", "0", "0", "0", "0", "0", "0"],
        );
        standalone.assert_output_success(&output, &format!("create {name}"));
    }
    standalone.assert_call("debug_spawn_player_entity", &["1"]);

    let _topology = TopologyEnv::install(&format!("36:*={INSTANCES}, 1:*={OTHER}"), REALM);
    let cfg = GatewayConfig {
        logon_bind: "127.0.0.1:0".into(),
        world_bind: "127.0.0.1:0".into(),
        stdb_uri: standalone.server().into(),
        module_name: standalone.shard_name().into(),
        coordinator_token: Some(standalone.owner_token()),
        gateway_id: "party-cleanup-test".into(),
        blocking_task_capacity: BlockingTaskCapacity::new(1),
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let world = runtime.block_on(Coordinator::connect(&cfg)).unwrap();
    let instances = world.shard_handle(INSTANCES).unwrap();
    let realm = world.realm_core().unwrap();
    let observer = runtime.block_on(Coordinator::connect(&cfg)).unwrap();
    let observer_instances = observer.shard_handle(INSTANCES).unwrap();
    observer.spawn_character_gone_relay();
    assert!(poll_until(POLL_TIMEOUT, || reconciliation_is_idle(
        &observer
    )));

    use lyracore_shared::group::realm_op;
    assert_eq!(
        realm
            .realm_group_op(realm_op::INVITE, 1, INSTANCES_SURVIVOR, 0, 0)
            .unwrap(),
        PartyOutcome::Ran
    );
    assert_eq!(
        realm
            .realm_group_op(realm_op::ACCEPT, INSTANCES_SURVIVOR, 0, 0, 0)
            .unwrap(),
        PartyOutcome::Ran
    );
    assert_eq!(
        realm
            .realm_group_op(realm_op::INVITE, 1, OTHER_SURVIVOR, 0, 0)
            .unwrap(),
        PartyOutcome::Ran
    );
    assert_eq!(
        realm
            .realm_group_op(realm_op::ACCEPT, OTHER_SURVIVOR, 0, 0, 0)
            .unwrap(),
        PartyOutcome::Ran
    );
    assert!(poll_until(POLL_TIMEOUT, || realm
        .group_roster(1)
        .is_some_and(
            |roster| roster.members == [1, INSTANCES_SURVIVOR, OTHER_SURVIVOR]
        )));
    let roster = realm.group_roster(1).unwrap();
    for (_, shard) in world.world_shards() {
        shard.sync_group_mirror(&roster).unwrap();
    }
    assert!(poll_until(POLL_TIMEOUT, || world
        .world_shards()
        .into_iter()
        .all(|(_, shard)| shard.group_roster(1).is_some_and(
            |roster| roster.members == [1, INSTANCES_SURVIVOR, OTHER_SURVIVOR]
        ))));

    let source_revision = observer
        .0
        .coord()
        .character_revision()
        .load(Ordering::Acquire);
    observer_instances.park_pump_for_test();
    let output = cli.call(
        standalone.server(),
        standalone.shard_name(),
        "debug_bot_transfer",
        &["1", "36", "1", "-11208", "1672", "24", "0", "party-test"],
    );
    standalone.assert_output_success(&output, "transfer Character");
    crate::world::transfer::run_bot_transfer(&world, 1, 36, 1, "party-test").unwrap();
    assert!(poll_until(POLL_TIMEOUT, || {
        world.character_by_guid(1).unwrap().is_none()
            && instances.character_by_guid(1).unwrap().is_some()
            && observer
                .0
                .coord()
                .character_revision()
                .load(Ordering::Acquire)
                > source_revision
    }));
    assert!(poll_until(POLL_TIMEOUT, || reconciliation_is_running(
        &observer
    )));
    std::thread::sleep(Duration::from_millis(250));
    assert!(realm.group_roster(1).is_some());
    observer_instances.resume_pump_for_test();
    assert!(poll_until(POLL_TIMEOUT, || reconciliation_is_idle(
        &observer
    )));
    assert!(realm.group_roster(1).is_some());

    let destination_revision = observer_instances
        .0
        .coord()
        .character_revision()
        .load(Ordering::Acquire);
    let output = cli.call(
        standalone.server(),
        INSTANCES,
        "debug_delete_character",
        &["1"],
    );
    standalone.assert_output_success(&output, "delete transferred Character");
    assert!(poll_until(POLL_TIMEOUT, || {
        observer_instances
            .0
            .coord()
            .character_revision()
            .load(Ordering::Acquire)
            > destination_revision
            && reconciliation_is_idle(&observer)
            && realm.group_roster(1).is_none()
    }));
    let survivors = realm.group_roster(INSTANCES_SURVIVOR).unwrap();
    assert_eq!(survivors.members, [INSTANCES_SURVIVOR, OTHER_SURVIVOR]);
    assert!(world.world_shards().into_iter().all(|(_, shard)| shard
        .group_roster(INSTANCES_SURVIVOR)
        .is_some_and(|roster| roster == survivors)));
}
