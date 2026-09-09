//! Private multi-database proof for companion-command replay and capacity.

use super::{party_command_intent, Coordinator};
use crate::accept::BlockingTaskCapacity;
use crate::config::GatewayConfig;
use crate::durable_test_support::{module_bytes, poll_until, Standalone, POLL_TIMEOUT};
use crate::stdb::bindings::GamePartyCommandIntentTableAccess;
use crate::world::party::{self, CompanionCommandOutcome};
use spacetimedb_sdk::Table;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const TANK: &str = "0";
const WARRIOR: &str = "1";

struct PrivateCli {
    config: PathBuf,
    wasm: PathBuf,
    spacetime: OsString,
}

impl PrivateCli {
    fn new(owner_token: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "lyracore-party-command-cli-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let config = root.join("cli.toml");
        let wasm = root.join("lyracore-module.wasm");
        std::fs::write(&config, format!("spacetimedb_token = {owner_token:?}\n")).unwrap();
        std::fs::write(&wasm, module_bytes()).unwrap();
        Self {
            config,
            wasm,
            spacetime: std::env::var_os("SPACETIME_BIN").unwrap_or_else(|| "spacetime".into()),
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.spacetime);
        command.args(["--config-path", self.config.to_str().unwrap()]);
        command
    }

    fn publish(&self, server: &str, database: &str) {
        let output = self
            .command()
            .args([
                "publish",
                "-s",
                server,
                "--bin-path",
                self.wasm.to_str().unwrap(),
                "-y",
                database,
            ])
            .output()
            .unwrap();
        assert_success(&output, &format!("publish {database}"));
    }

    fn call(&self, server: &str, database: &str, reducer: &str, args: &[&str]) {
        let mut command = self.command();
        command.args(["call", "-s", server, database, reducer]);
        command.args(args);
        let output = command.output().unwrap();
        assert_success(&output, &format!("call {database}.{reducer}"));
    }

    fn rows(&self, server: &str, database: &str, query: &str) -> Vec<BTreeMap<String, String>> {
        let output = self
            .command()
            .args(["sql", "-s", server, "--format", "text", database, query])
            .output()
            .unwrap();
        assert_success(&output, &format!("query {database}: {query}"));
        parse_rows(&String::from_utf8(output.stdout).unwrap())
    }
}

impl Drop for PrivateCli {
    fn drop(&mut self) {
        if let Some(root) = self.config.parent() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

fn assert_success(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{context}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn parse_rows(output: &str) -> Vec<BTreeMap<String, String>> {
    let mut lines = output.lines().filter(|line| !line.trim().is_empty());
    let headers: Vec<_> = lines
        .next()
        .expect("SQL output missing header")
        .split('|')
        .map(str::trim)
        .collect();
    let separator = lines.next().expect("SQL output missing separator");
    assert!(separator.chars().all(|c| c == '-' || c == '+'));
    lines
        .map(|line| {
            let values: Vec<_> = line.split('|').map(str::trim).collect();
            assert_eq!(values.len(), headers.len());
            headers
                .iter()
                .zip(values)
                .map(|(header, value)| ((*header).to_string(), value.to_string()))
                .collect()
        })
        .collect()
}

struct TopologyEnv {
    previous: Vec<(&'static str, Option<OsString>)>,
}

impl TopologyEnv {
    fn install(shard_map: &str, realm: &str) -> Self {
        let values = [
            ("LYRACORE_SHARD_MAP", shard_map),
            ("LYRACORE_REALM_CORE", realm),
            ("LYRACORE_CALL_PIPES", "1"),
            ("LYRACORE_AOI", "0"),
        ];
        let previous = values
            .iter()
            .map(|(name, value)| {
                let prior = std::env::var_os(name);
                std::env::set_var(name, value);
                (*name, prior)
            })
            .collect();
        Self { previous }
    }
}

impl Drop for TopologyEnv {
    fn drop(&mut self) {
        for (name, value) in self.previous.drain(..) {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

#[derive(Clone)]
struct RoleParty {
    warrior: u64,
    priest: u64,
    mage: u64,
    leader: u64,
    map_id: u32,
}

fn stage_roles(cli: &PrivateCli, server: &str, database: &str, guid_base: u64) -> RoleParty {
    cli.call(server, database, "claim_operator", &[]);
    cli.call(
        server,
        database,
        "install_guid_range",
        &[&guid_base.to_string()],
    );
    cli.call(
        server,
        database,
        "playerbots_spawn",
        &["4", "1200", "1200", "50"],
    );
    let bots = cli.rows(
        server,
        database,
        "SELECT character_guid, class, role FROM pkg_playerbots_bot",
    );
    let warrior = bots
        .iter()
        .find(|row| row["class"] == WARRIOR && row["role"] == TANK)
        .unwrap()["character_guid"]
        .parse()
        .unwrap();
    let priest = bots.iter().find(|row| row["class"] == "5").unwrap()["character_guid"]
        .parse()
        .unwrap();
    let mage = bots.iter().find(|row| row["class"] == "8").unwrap()["character_guid"]
        .parse()
        .unwrap();
    let leader = bots
        .iter()
        .filter(|row| row["class"] == WARRIOR && row["role"] == TANK)
        .map(|row| row["character_guid"].parse::<u64>().unwrap())
        .find(|guid| *guid != warrior)
        .unwrap();
    let args = [
        warrior.to_string(),
        priest.to_string(),
        mage.to_string(),
        leader.to_string(),
    ];
    cli.call(
        server,
        database,
        "playerbots_fixture_roles_stage",
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    let map_id = cli.rows(
        server,
        database,
        &format!("SELECT map_id FROM game_world_entity WHERE guid = {warrior}"),
    )[0]["map_id"]
        .parse()
        .unwrap();
    RoleParty {
        warrior,
        priest,
        mage,
        leader,
        map_id,
    }
}

fn install_authority(
    cli: &PrivateCli,
    server: &str,
    realm: &str,
    target: &str,
    party: &RoleParty,
    leader: u64,
) {
    let args = [
        party.warrior.to_string(),
        party.priest.to_string(),
        party.mage.to_string(),
        leader.to_string(),
        "0".to_string(),
    ];
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    cli.call(server, realm, "playerbots_fixture_orders_party", &args);
    cli.call(server, target, "playerbots_fixture_orders_party", &args);
}

fn source_actor(
    cli: &PrivateCli,
    server: &str,
    source: &str,
    leader: u64,
    username: &str,
) -> String {
    cli.call(
        server,
        source,
        "provision_account",
        &[&format!("\"{username}\""), "[]", "[]"],
    );
    let account = cli.rows(
        server,
        source,
        &format!("SELECT id FROM game_account WHERE username = '{username}'"),
    )[0]["id"]
        .clone();
    cli.call(
        server,
        source,
        "playerbots_fixture_orders_account",
        &[&leader.to_string(), &account],
    );
    cli.call(
        server,
        source,
        "claim_account",
        &[&account, &leader.to_string(), "9009"],
    );
    let generation = cli.rows(
        server,
        source,
        &format!("SELECT generation FROM game_account_claim WHERE account_id = {account}"),
    )[0]["generation"]
        .clone();
    format!(
        r#"{{"guid":{leader},"ownership":{{"some":{{"account_id":{account},"generation":{generation},"request_nonce":9009}}}}}}"#
    )
}

fn queue(cli: &PrivateCli, server: &str, source: &str, actor: &str, payload: &str) -> u64 {
    let before = cli
        .rows(server, source, "SELECT id FROM game_party_command_intent")
        .into_iter()
        .map(|row| row["id"].parse::<u64>().unwrap())
        .max()
        .unwrap_or(0);
    cli.call(
        server,
        source,
        "gw_client_command",
        &[actor, r#""playerbots.order""#, &format!(r#""{payload}""#)],
    );
    cli.rows(server, source, "SELECT id FROM game_party_command_intent")
        .into_iter()
        .map(|row| row["id"].parse::<u64>().unwrap())
        .filter(|id| *id > before)
        .max()
        .expect("authenticated command did not queue")
}

fn config(server: &str, source: &str, token: &str, name: &str) -> GatewayConfig {
    GatewayConfig {
        logon_bind: "127.0.0.1:0".into(),
        world_bind: "127.0.0.1:0".into(),
        stdb_uri: server.into(),
        module_name: source.into(),
        coordinator_token: Some(token.into()),
        gateway_id: name.into(),
        blocking_task_capacity: BlockingTaskCapacity::new(2),
    }
}

fn cached_intent(source: &Coordinator, id: u64) -> party::PartyCommandIntent {
    cached_intent_when(source, id, |_| true)
}

fn cached_intent_when(
    source: &Coordinator,
    id: u64,
    predicate: impl Fn(&crate::stdb::bindings::PartyCommandIntent) -> bool,
) -> party::PartyCommandIntent {
    let mut found = None;
    assert!(poll_until(POLL_TIMEOUT, || {
        found = source
            .0
            .coord()
            .conn
            .db
            .game_party_command_intent()
            .iter()
            .find(|row| row.id == id && predicate(row))
            .map(|row| party_command_intent(&row));
        found.is_some()
    }));
    found.unwrap()
}

fn row(cli: &PrivateCli, server: &str, database: &str, query: &str) -> BTreeMap<String, String> {
    let rows = cli.rows(server, database, query);
    assert_eq!(rows.len(), 1, "{query}: {rows:?}");
    rows[0].clone()
}

fn terminal_messages(cli: &PrivateCli, server: &str, source: &str) -> usize {
    cli.rows(
        server,
        source,
        "SELECT id FROM game_addon_message WHERE cmd = 'playerbots.order.result'",
    )
    .len()
}

fn git(path: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(path)
        .args(args)
        .output()
        .unwrap();
    assert_success(&output, "git evidence input");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn evidence(topology: &CommandTopology, case: &str) {
    let core = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let package = core.join("packages/playerbots");
    let record = serde_json::json!({
        "case": case,
        "spacetimedb": "2.7.1",
        "rust": "1.93.0",
        "tested_core": git(core, &["rev-parse", "HEAD"]),
        "tested_collection": git(&package, &["rev-parse", "HEAD"]),
        "core_dirty": !git(core, &["status", "--porcelain"]).is_empty(),
        "collection_dirty": !git(&package, &["status", "--porcelain"]).is_empty(),
        "module_wasm_identity": blake3::hash(module_bytes()).to_hex().to_string(),
        "source_database": topology.source(),
        "target_database": topology.target,
        "realm_database": topology.realm,
        "source_intents": topology.cli.rows(topology.node.server(), topology.source(), "SELECT * FROM game_party_command_intent"),
        "source_results": topology.cli.rows(topology.node.server(), topology.source(), "SELECT * FROM game_addon_message WHERE cmd = 'playerbots.order.result'"),
        "target_receipts": topology.cli.rows(topology.node.server(), &topology.target, "SELECT * FROM game_party_command_receipt"),
        "target_orders": topology.cli.rows(topology.node.server(), &topology.target, "SELECT * FROM pkg_playerbots_companion_order"),
        "realm_groups": topology.cli.rows(topology.node.server(), &topology.realm, "SELECT * FROM game_group"),
        "realm_members": topology.cli.rows(topology.node.server(), &topology.realm, "SELECT * FROM game_group_member"),
        "content": {"revision": "playerbots-starter-roles-v1", "imported_content": null},
        "geometry": {"revision": "playerbots-synthetic-nav-v1", "client_geometry": null},
    });
    let path = crate::durable_test_support::log_dir()
        .join(format!("{}-{case}.json", topology.node.shard_name()));
    std::fs::write(path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
}

struct CommandTopology {
    node: Standalone,
    cli: PrivateCli,
    target: String,
    realm: String,
    source_two: String,
    target_party: RoleParty,
    source_one_party: RoleParty,
    source_two_party: RoleParty,
    actor_one: String,
    actor_two: String,
}

impl CommandTopology {
    fn new(name: &str) -> Self {
        let mut node = Standalone::start(name);
        node.publish_module();
        let cli = PrivateCli::new(&node.owner_token());
        let source = node.shard_name().to_string();
        let target = format!("{source}-target");
        let realm = format!("{source}-realm");
        let source_two = format!("{source}-source-two");
        for database in [&target, &realm, &source_two] {
            cli.publish(node.server(), database);
        }
        let source_one_party = stage_roles(&cli, node.server(), &source, 1_000_000);
        let target_party = stage_roles(&cli, node.server(), &target, 2_000_000);
        let source_two_party = stage_roles(&cli, node.server(), &source_two, 3_000_000);
        cli.call(node.server(), &realm, "claim_operator", &[]);
        install_authority(
            &cli,
            node.server(),
            &realm,
            &target,
            &target_party,
            source_one_party.leader,
        );
        let actor_one = source_actor(
            &cli,
            node.server(),
            &source,
            source_one_party.leader,
            "PB009SOURCEONE",
        );
        let actor_two = source_actor(
            &cli,
            node.server(),
            &source_two,
            source_two_party.leader,
            "PB009SOURCETWO",
        );
        Self {
            node,
            cli,
            target,
            realm,
            source_two,
            target_party,
            source_one_party,
            source_two_party,
            actor_one,
            actor_two,
        }
    }

    fn source(&self) -> &str {
        self.node.shard_name()
    }

    fn coordinator(&self, source: &str, name: &str) -> (tokio::runtime::Runtime, Coordinator) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let coordinator = runtime
            .block_on(Coordinator::connect(&config(
                self.node.server(),
                source,
                &self.node.owner_token(),
                name,
            )))
            .unwrap();
        (runtime, coordinator)
    }
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1, the full Package union, and the Wasm toolchain"]
fn companion_command_receipts_recover_both_gateway_crash_boundaries() {
    let topology = CommandTopology::new("party-command-crash-recovery");
    let shard_map = format!("{}:*={}", topology.target_party.map_id, topology.target);
    let _environment = TopologyEnv::install(&shard_map, &topology.realm);
    let (_runtime, source) = topology.coordinator(topology.source(), "party-command-source-one");
    let target = source.shard_handle(&topology.target).unwrap();

    let claimed = queue(
        &topology.cli,
        topology.node.server(),
        topology.source(),
        &topology.actor_one,
        &format!("follow|{}", topology.target_party.warrior),
    );
    let claimed_intent = cached_intent(&source, claimed);
    source.claim_party_command_intent(claimed, 101).unwrap();
    evidence(&topology, "claim-without-ack");
    assert_eq!(
        terminal_messages(&topology.cli, topology.node.server(), topology.source()),
        0
    );
    std::thread::sleep(std::time::Duration::from_millis(2_100));
    assert_eq!(
        party::run_party_command_intent(&source, &claimed_intent, 102).unwrap(),
        CompanionCommandOutcome::Applied
    );
    evidence(&topology, "claim-lease-recovered");
    assert_eq!(
        terminal_messages(&topology.cli, topology.node.server(), topology.source()),
        1
    );

    let after_claim = row(
        &topology.cli,
        topology.node.server(),
        &topology.target,
        &format!(
            "SELECT revision, history FROM pkg_playerbots_companion_order WHERE character_guid = {}",
            topology.target_party.warrior
        ),
    );
    let target_applied = queue(
        &topology.cli,
        topology.node.server(),
        topology.source(),
        &topology.actor_one,
        &format!("stay|{}", topology.target_party.warrior),
    );
    let target_intent = cached_intent(&source, target_applied);
    source
        .claim_party_command_intent(target_applied, 201)
        .unwrap();
    let authority = source
        .realm_core()
        .unwrap()
        .group_roster(topology.source_one_party.leader)
        .unwrap();
    let admitted = party::AdmittedCompanionCommand {
        source_identity: target_intent.source_identity,
        intent_id: target_intent.id,
        issuer_guid: target_intent.issuer_guid,
        group_id: authority.group_id,
        leader_guid: authority.leader_guid,
        members: authority.members,
        kind: target_intent.kind,
        bot_guid: target_intent.bot_guid,
        authority_member_guid: target_intent.authority_member_guid,
        exact_target_guid: target_intent.exact_target_guid,
        expires_micros: target_intent.expires_micros,
        receipt_retain_until_micros: target_intent.expires_micros.saturating_add(30_000_000),
    };
    assert_eq!(
        target.apply_admitted_party_command(&admitted).unwrap(),
        CompanionCommandOutcome::Applied
    );
    let after_target = row(
        &topology.cli,
        topology.node.server(),
        &topology.target,
        &format!(
            "SELECT revision, history FROM pkg_playerbots_companion_order WHERE character_guid = {}",
            topology.target_party.warrior
        ),
    );
    assert_ne!(after_target["revision"], after_claim["revision"]);
    assert_eq!(
        terminal_messages(&topology.cli, topology.node.server(), topology.source()),
        1
    );
    for sequence in 0..9 {
        let operation = if sequence % 2 == 0 { "follow" } else { "stay" };
        let later_id = queue(
            &topology.cli,
            topology.node.server(),
            topology.source(),
            &topology.actor_one,
            &format!("{operation}|{}", topology.target_party.warrior),
        );
        let later = cached_intent(&source, later_id);
        let outcome = party::run_party_command_intent(&source, &later, 220 + sequence).unwrap();
        assert!(matches!(
            outcome,
            CompanionCommandOutcome::Applied | CompanionCommandOutcome::Unchanged
        ));
    }
    let before_delayed_retry = row(
        &topology.cli,
        topology.node.server(),
        &topology.target,
        &format!(
            "SELECT revision, history FROM pkg_playerbots_companion_order WHERE character_guid = {}",
            topology.target_party.warrior
        ),
    );
    evidence(&topology, "target-receipt-after-history-rollover");
    assert_eq!(
        party::run_party_command_intent(&source, &target_intent, 201).unwrap(),
        CompanionCommandOutcome::Applied
    );
    let recovered = row(
        &topology.cli,
        topology.node.server(),
        &topology.target,
        &format!(
            "SELECT revision, history FROM pkg_playerbots_companion_order WHERE character_guid = {}",
            topology.target_party.warrior
        ),
    );
    evidence(&topology, "target-receipt-delayed-retry");
    assert_eq!(
        recovered, before_delayed_retry,
        "receipt retry reapplied the order"
    );
    assert_eq!(
        terminal_messages(&topology.cli, topology.node.server(), topology.source()),
        11
    );

    install_authority(
        &topology.cli,
        topology.node.server(),
        &topology.realm,
        &topology.target,
        &topology.target_party,
        topology.source_two_party.leader,
    );
    let (_runtime_two, source_two) =
        topology.coordinator(&topology.source_two, "party-command-source-two");
    let same_numeric_id = queue(
        &topology.cli,
        topology.node.server(),
        &topology.source_two,
        &topology.actor_two,
        &format!("follow|{}", topology.target_party.warrior),
    );
    assert_eq!(same_numeric_id, claimed);
    let second_source_intent = cached_intent(&source_two, same_numeric_id);
    assert_ne!(
        second_source_intent.source_identity,
        claimed_intent.source_identity
    );
    assert_eq!(
        party::run_party_command_intent(&source_two, &second_source_intent, 301).unwrap(),
        CompanionCommandOutcome::Applied
    );
    let receipts = topology.cli.rows(
        topology.node.server(),
        &topology.target,
        &format!("SELECT source_identity, intent_id FROM game_party_command_receipt WHERE intent_id = {claimed}"),
    );
    evidence(&topology, "same-intent-id-distinct-source-modules");
    assert_eq!(
        receipts.len(),
        2,
        "distinct source identities collided: {receipts:?}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1, the full Package union, and the Wasm toolchain"]
fn companion_command_capacity_waits_without_ack_then_recovers_or_expires() {
    let topology = CommandTopology::new("party-command-capacity");
    let shard_map = format!("{}:*={}", topology.target_party.map_id, topology.target);
    let _environment = TopologyEnv::install(&shard_map, &topology.realm);
    let (_runtime, source) = topology.coordinator(topology.source(), "party-command-capacity");
    let payload = format!("follow|{}", topology.target_party.warrior);

    for token in 1..=32 {
        let id = queue(
            &topology.cli,
            topology.node.server(),
            topology.source(),
            &topology.actor_one,
            &payload,
        );
        let intent = cached_intent(&source, id);
        let outcome = party::run_party_command_intent(&source, &intent, 1_000 + token).unwrap();
        assert!(matches!(
            outcome,
            CompanionCommandOutcome::Applied | CompanionCommandOutcome::Unchanged
        ));
    }
    let before = row(
        &topology.cli,
        topology.node.server(),
        &topology.target,
        &format!(
            "SELECT revision, history FROM pkg_playerbots_companion_order WHERE character_guid = {}",
            topology.target_party.warrior
        ),
    );
    let messages_before =
        terminal_messages(&topology.cli, topology.node.server(), topology.source());
    let waiting_id = queue(
        &topology.cli,
        topology.node.server(),
        topology.source(),
        &topology.actor_one,
        &payload,
    );
    let waiting = cached_intent(&source, waiting_id);
    assert_eq!(
        party::run_party_command_intent(&source, &waiting, 2_001).unwrap(),
        CompanionCommandOutcome::WaitingForCapacity
    );
    let pending = row(
        &topology.cli,
        topology.node.server(),
        topology.source(),
        &format!("SELECT pending, state FROM game_party_command_intent WHERE id = {waiting_id}"),
    );
    let after_wait = row(
        &topology.cli,
        topology.node.server(),
        &topology.target,
        &format!(
            "SELECT revision, history FROM pkg_playerbots_companion_order WHERE character_guid = {}",
            topology.target_party.warrior
        ),
    );
    evidence(&topology, "capacity-wait-no-ack");
    assert_eq!(pending["pending"], "true");
    assert!(pending["state"].to_ascii_lowercase().contains("pending"));
    assert_eq!(after_wait, before);
    assert_eq!(
        terminal_messages(&topology.cli, topology.node.server(), topology.source()),
        messages_before
    );

    topology.cli.call(
        topology.node.server(),
        &topology.target,
        "playerbots_fixture_command_release_receipt",
        &[&topology.target_party.warrior.to_string()],
    );
    assert_eq!(
        party::run_party_command_intent(&source, &waiting, 2_001).unwrap(),
        CompanionCommandOutcome::Unchanged
    );
    evidence(&topology, "capacity-retry-applied");
    assert_eq!(
        terminal_messages(&topology.cli, topology.node.server(), topology.source()),
        messages_before + 1
    );

    let expiry_id = queue(
        &topology.cli,
        topology.node.server(),
        topology.source(),
        &topology.actor_one,
        &payload,
    );
    let expiry = cached_intent(&source, expiry_id);
    assert_eq!(
        party::run_party_command_intent(&source, &expiry, 3_001).unwrap(),
        CompanionCommandOutcome::WaitingForCapacity
    );
    topology.cli.call(
        topology.node.server(),
        topology.source(),
        "playerbots_fixture_command_expire",
        &[&expiry_id.to_string()],
    );
    let expired = cached_intent_when(&source, expiry_id, |row| row.expires_micros == 0);
    assert_eq!(
        party::finish_expired_party_command_intent(&source, &expired, 3_002).unwrap(),
        CompanionCommandOutcome::Expired
    );
    let terminal = row(
        &topology.cli,
        topology.node.server(),
        topology.source(),
        &format!("SELECT pending, state FROM game_party_command_intent WHERE id = {expiry_id}"),
    );
    evidence(&topology, "capacity-expired-without-application");
    assert_eq!(terminal["pending"], "false");
    assert!(terminal["state"].to_ascii_lowercase().contains("expired"));
}
