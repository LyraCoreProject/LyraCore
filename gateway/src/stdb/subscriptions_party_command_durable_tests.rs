//! Private multi-database proof for companion-command replay and capacity.

use super::{party_command_intent, Coordinator, DURABLE_TOPOLOGY_ENV_LOCK};
use crate::accept::BlockingTaskCapacity;
use crate::config::GatewayConfig;
use crate::durable_test_support::{module_bytes, poll_until, Standalone, POLL_TIMEOUT};
use crate::stdb::bindings::GamePartyCommandIntentTableAccess;
use crate::world::party::{self, CompanionCommandOutcome};
use crate::world::WorldStore;
use spacetimedb_sdk::Table;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::MutexGuard;
use std::time::{SystemTime, UNIX_EPOCH};

const TANK: &str = "0";
const WARRIOR: &str = "1";
const GROUP: u64 = 5_098_000;

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

    fn json_rows(&self, server: &str, database: &str, query: &str) -> Vec<Vec<serde_json::Value>> {
        let output = self
            .command()
            .args(["sql", "-s", server, "--format", "json", database, query])
            .output()
            .unwrap();
        assert_success(&output, &format!("query {database}: {query}"));
        parse_json_rows(&String::from_utf8(output.stdout).unwrap())
            .unwrap_or_else(|error| panic!("query {database}: {query}: {error}"))
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

fn parse_json_rows(output: &str) -> Result<Vec<Vec<serde_json::Value>>, String> {
    let statements: serde_json::Value =
        serde_json::from_str(output).map_err(|error| format!("malformed SQL JSON: {error}"))?;
    let statements = statements
        .as_array()
        .ok_or("SQL JSON result is not an array")?;
    if statements.len() != 1 {
        return Err(format!(
            "SQL JSON returned {} statements instead of one",
            statements.len()
        ));
    }
    statements[0]
        .get("rows")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "SQL JSON statement has no row array".to_string())?
        .iter()
        .map(|row| {
            row.as_array()
                .cloned()
                .ok_or_else(|| "SQL JSON row is not an array".to_string())
        })
        .collect()
}

#[test]
fn structured_sql_rows_preserve_delimiters_and_escaped_quotes() {
    let rows = parse_json_rows(
        r#"[{"schema":{},"rows":[["1|Superseded","said \"yes\""]],"total_duration_micros":1}]"#,
    )
    .unwrap();
    assert_eq!(rows[0][0].as_str(), Some("1|Superseded"));
    assert_eq!(rows[0][1].as_str(), Some("said \"yes\""));
}

#[test]
fn structured_sql_rows_reject_malformed_output() {
    assert!(parse_json_rows("not json").is_err());
    assert!(parse_json_rows(r#"[{"rows":["not a row"]}]"#).is_err());
}

struct TopologyEnv {
    previous: Vec<(&'static str, Option<OsString>)>,
    _guard: MutexGuard<'static, ()>,
}

impl TopologyEnv {
    fn install(shard_map: &str, realm: &str) -> Self {
        let guard = DURABLE_TOPOLOGY_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        Self {
            previous,
            _guard: guard,
        }
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

fn stage_roles(
    cli: &PrivateCli,
    server: &str,
    database: &str,
    guid_base: u64,
    name_namespace: u8,
) -> RoleParty {
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
    let warrior: u64 = bots
        .iter()
        .find(|row| row["class"] == WARRIOR && row["role"] == TANK)
        .unwrap()["character_guid"]
        .parse()
        .unwrap();
    let priest: u64 = bots.iter().find(|row| row["class"] == "5").unwrap()["character_guid"]
        .parse()
        .unwrap();
    let mage: u64 = bots.iter().find(|row| row["class"] == "8").unwrap()["character_guid"]
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
    let mut name_args = args.to_vec();
    name_args.push(name_namespace.to_string());
    cli.call(
        server,
        database,
        "playerbots_fixture_orders_names",
        &name_args.iter().map(String::as_str).collect::<Vec<_>>(),
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
    leader_party: &RoleParty,
) {
    let leader = leader_party.leader;
    let retained = cli.rows(
        server,
        target,
        &format!("SELECT * FROM game_group_member_partition WHERE group_id = {GROUP}"),
    );
    let revision = [realm, target]
        .into_iter()
        .flat_map(|database| {
            cli.rows(
                server,
                database,
                &format!(
                    "SELECT revision FROM game_group_roster_revision WHERE group_id = {GROUP}"
                ),
            )
        })
        .map(|row| row["revision"].parse::<u64>().unwrap())
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .unwrap();
    let mut next_membership = retained
        .iter()
        .map(|row| row["membership_revision"].parse::<u64>().unwrap())
        .max()
        .unwrap_or(0);
    let mut partitions = Vec::new();
    for (guid, map_id) in [
        (party.warrior, party.map_id),
        (party.priest, party.map_id),
        (party.mage, party.map_id),
        (leader, leader_party.map_id),
    ] {
        let previous = retained.iter().find(|row| {
            row["character_guid"] == guid.to_string() && row["member_active"] == "true"
        });
        let membership = match previous {
            Some(row) => row["membership_revision"].parse::<u64>().unwrap(),
            None => {
                next_membership = next_membership.checked_add(1).unwrap();
                next_membership
            }
        };
        partitions.push(serde_json::json!({
            "character_guid": guid, "group_id": GROUP,
            "membership_revision": membership, "member_active": true,
            "map_id": map_id, "instance_id": 0, "locator_revision": 1,
            "state": {"known": []},
        }));
    }
    partitions.sort_by_key(|partition| partition["membership_revision"].as_u64().unwrap());
    let members: Vec<_> = partitions
        .iter()
        .map(|partition| partition["character_guid"].clone())
        .collect();
    let args = [
        GROUP.to_string(),
        leader.to_string(),
        "0".to_string(),
        "2".to_string(),
        "0".to_string(),
        serde_json::to_string(&members).unwrap(),
        serde_json::json!({"guid": leader, "ownership": {"none": []}}).to_string(),
        serde_json::to_string(&partitions).unwrap(),
        revision.to_string(),
    ];
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    for database in [realm, target] {
        cli.call(server, database, "sync_group_mirror", &args);
    }
}

fn publish_authority_locators(topology: &CommandTopology, coordinator: &Coordinator) {
    let realm = coordinator.realm_core().unwrap();
    let members = [
        (
            topology.target.as_str(),
            topology.target_party.warrior,
            topology.target_party.map_id,
        ),
        (
            topology.target.as_str(),
            topology.target_party.priest,
            topology.target_party.map_id,
        ),
        (
            topology.target.as_str(),
            topology.target_party.mage,
            topology.target_party.map_id,
        ),
        (
            topology.source(),
            topology.source_one_party.leader,
            topology.source_one_party.map_id,
        ),
        (
            topology.source_two.as_str(),
            topology.source_two_party.leader,
            topology.source_two_party.map_id,
        ),
    ];
    let mut expected = Vec::new();
    for (database, guid, declared_map) in members {
        let body = row(
            &topology.cli,
            topology.node.server(),
            database,
            &format!("SELECT map_id, instance_id FROM game_world_entity WHERE guid = {guid}"),
        );
        let map_id = body["map_id"].parse::<u32>().unwrap();
        let instance_id = body["instance_id"].parse::<u64>().unwrap();
        realm
            .set_character_shard(guid, map_id, instance_id)
            .unwrap();
        expected.push((guid, map_id, instance_id, declared_map));
    }
    let visible = poll_until(POLL_TIMEOUT, || {
        expected.iter().all(|(guid, map_id, instance_id, _)| {
            realm
                .realm_character_partition(*guid)
                .unwrap()
                .is_some_and(|locator| {
                    (locator.map_id, locator.instance_id) == (*map_id, *instance_id)
                        && !locator.transfer_pending
                })
        })
    });
    evidence(topology, "realm-locators-before-transfer");
    assert!(expected.iter().all(
        |(_, map_id, instance_id, declared_map)| *map_id == *declared_map && *instance_id == 0
    ));
    assert!(visible);
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

fn enter_transferred_actor(
    topology: &CommandTopology,
    runtime: &tokio::runtime::Runtime,
    shard: &Coordinator,
    character_guid: u64,
) -> (String, Coordinator) {
    let character = row(
        &topology.cli,
        topology.node.server(),
        shard.shard_name(),
        &format!("SELECT account_id FROM game_character WHERE guid = {character_guid}"),
    );
    let account_id: u64 = character["account_id"].parse().unwrap();
    let account = row(
        &topology.cli,
        topology.node.server(),
        shard.shard_name(),
        &format!("SELECT username FROM game_account WHERE id = {account_id}"),
    );
    let raw_username = account["username"].clone();
    let username = sql_string(&raw_username);
    let realm = shard.realm_core().unwrap();
    realm.provision_account(&username, &[], &[]).unwrap();
    let mut realm_account = None;
    assert!(poll_until(POLL_TIMEOUT, || {
        realm_account = realm.account_by_username(&username).unwrap();
        realm_account.is_some()
    }));
    let realm_account_id = realm_account.unwrap().id;
    let identity = realm.bound_identity(realm_account_id).unwrap();
    realm
        .establish_session(realm_account_id, &[7; 40], identity)
        .unwrap();
    shard
        .establish_session(account_id, &[7; 40], identity)
        .unwrap();
    assert_eq!(realm.shard_name(), topology.realm);
    let session_visible = poll_until(POLL_TIMEOUT, || {
        realm
            .account_by_username(&username)
            .unwrap()
            .is_some_and(|account| account.id == realm_account_id)
            && realm.session_key(realm_account_id).unwrap().is_some()
            && shard
                .account_by_username(&username)
                .unwrap()
                .is_some_and(|account| account.id == account_id)
            && shard.session_key(account_id).unwrap().is_some()
    });
    let realm_stored = row(
        &topology.cli,
        topology.node.server(),
        &topology.realm,
        &format!("SELECT id, username FROM game_account WHERE id = {realm_account_id}"),
    );
    let shard_stored = row(
        &topology.cli,
        topology.node.server(),
        shard.shard_name(),
        &format!("SELECT id, username FROM game_account WHERE id = {account_id}"),
    );
    let realm_cached = realm
        .account_by_username(&username)
        .unwrap()
        .map(|account| account.id);
    let shard_cached = shard
        .account_by_username(&username)
        .unwrap()
        .map(|account| account.id);
    let session_record = serde_json::json!({
        "raw_shard_username": raw_username,
        "decoded_username": username,
        "realm_database": realm.shard_name(),
        "shard_database": shard.shard_name(),
        "realm_sql_account": realm_stored,
        "shard_sql_account": shard_stored,
        "realm_cached_account_id": realm_cached,
        "shard_cached_account_id": shard_cached,
        "realm_session_visible": realm.session_key(realm_account_id).unwrap().is_some(),
        "shard_session_visible": shard.session_key(account_id).unwrap().is_some(),
    });
    let path = crate::durable_test_support::log_dir().join(format!(
        "{}-transferred-issuer-session.json",
        topology.node.shard_name()
    ));
    std::fs::write(path, serde_json::to_vec_pretty(&session_record).unwrap()).unwrap();
    assert_eq!(sql_string(&realm_stored["username"]), username);
    assert_eq!(sql_string(&shard_stored["username"]), username);
    assert!(
        session_visible,
        "session transaction was not visible in both Coordinator caches; Realm SQL: \
         {realm_stored:?}; Shard SQL: {shard_stored:?}"
    );
    assert_eq!(realm_cached, Some(realm_account_id));
    assert_eq!(shard_cached, Some(account_id));
    topology.cli.call(
        topology.node.server(),
        shard.shard_name(),
        "gw_heartbeat",
        &[],
    );
    let token = shard.claim_session(account_id, character_guid).unwrap();
    let bound = {
        let _entered = runtime.enter();
        shard.bind_session(token).unwrap()
    };
    bound.player_login(account_id, character_guid).unwrap();
    assert!(poll_until(POLL_TIMEOUT, || topology
        .cli
        .rows(
            topology.node.server(),
            shard.shard_name(),
            &format!("SELECT guid FROM game_world_entity WHERE guid = {character_guid}"),
        )
        .len()
        == 1));
    (
        format!(
            r#"{{"guid":{character_guid},"ownership":{{"some":{{"account_id":{},"generation":{},"request_nonce":{}}}}}}}"#,
            token.account_id, token.generation, token.request_nonce
        ),
        bound,
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

fn sql_string(cell: &str) -> String {
    serde_json::from_str(cell)
        .unwrap_or_else(|error| panic!("malformed SQL string cell {cell:?}: {error}"))
}

fn terminal_messages_for(
    cli: &PrivateCli,
    server: &str,
    source: &str,
    intent_id: u64,
) -> Vec<String> {
    cli.json_rows(
        server,
        source,
        "SELECT payload FROM game_addon_message WHERE cmd = 'playerbots.order.result'",
    )
    .into_iter()
    .filter_map(|row| {
        row.first()
            .and_then(serde_json::Value::as_str)
            .filter(|payload| payload.starts_with(&format!("{intent_id}|")))
            .map(str::to_string)
    })
    .collect()
}

fn transfer_destination_observation(
    topology: &CommandTopology,
    target: &dyn WorldStore,
    character_guid: u64,
    map_id: u32,
    instance_id: u64,
) -> (bool, BTreeMap<String, String>) {
    let visible = poll_until(POLL_TIMEOUT, || {
        target
            .character_destination(character_guid)
            .is_some_and(|plan| (plan.dest_map_id, plan.dest_instance_id) == (map_id, instance_id))
    });
    let committed = row(
        &topology.cli,
        topology.node.server(),
        &topology.target,
        &format!(
            "SELECT map_id, pending_instance_id FROM game_character WHERE guid = {character_guid}"
        ),
    );
    (visible, committed)
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
        "source_issuers": topology.cli.rows(topology.node.server(), topology.source(), "SELECT * FROM game_party_command_issuer"),
        "source_dispatch_lanes": topology.cli.rows(topology.node.server(), topology.source(), "SELECT * FROM game_party_command_dispatch_lane"),
        "source_results": topology.cli.json_rows(topology.node.server(), topology.source(), "SELECT * FROM game_addon_message WHERE cmd = 'playerbots.order.result'"),
        "source_accounts": topology.cli.rows(topology.node.server(), topology.source(), "SELECT id, username FROM game_account"),
        "source_account_claims": topology.cli.rows(topology.node.server(), topology.source(), "SELECT * FROM game_account_claim"),
        "source_characters": topology.cli.rows(topology.node.server(), topology.source(), "SELECT guid, name FROM game_character"),
        "source_guid_range": topology.cli.rows(topology.node.server(), topology.source(), "SELECT * FROM game_guid_range"),
        "source_two_intents": topology.cli.rows(topology.node.server(), &topology.source_two, "SELECT * FROM game_party_command_intent"),
        "source_two_issuers": topology.cli.rows(topology.node.server(), &topology.source_two, "SELECT * FROM game_party_command_issuer"),
        "source_two_results": topology.cli.json_rows(topology.node.server(), &topology.source_two, "SELECT * FROM game_addon_message WHERE cmd = 'playerbots.order.result'"),
        "source_two_characters": topology.cli.rows(topology.node.server(), &topology.source_two, "SELECT guid, name FROM game_character"),
        "source_two_guid_range": topology.cli.rows(topology.node.server(), &topology.source_two, "SELECT * FROM game_guid_range"),
        "target_receipts": topology.cli.rows(topology.node.server(), &topology.target, "SELECT * FROM game_party_command_receipt"),
        "target_intents": topology.cli.rows(topology.node.server(), &topology.target, "SELECT * FROM game_party_command_intent"),
        "target_dispatch_lanes": topology.cli.rows(topology.node.server(), &topology.target, "SELECT * FROM game_party_command_dispatch_lane"),
        "target_results": topology.cli.json_rows(topology.node.server(), &topology.target, "SELECT * FROM game_addon_message WHERE cmd = 'playerbots.order.result'"),
        "target_issuers": topology.cli.rows(topology.node.server(), &topology.target, "SELECT * FROM game_party_command_issuer"),
        "target_orders": topology.cli.rows(topology.node.server(), &topology.target, "SELECT * FROM pkg_playerbots_companion_order"),
        "target_accounts": topology.cli.rows(topology.node.server(), &topology.target, "SELECT id, username FROM game_account"),
        "target_characters": topology.cli.rows(topology.node.server(), &topology.target, "SELECT guid, name FROM game_character"),
        "target_guid_range": topology.cli.rows(topology.node.server(), &topology.target, "SELECT * FROM game_guid_range"),
        "source_receipts": topology.cli.rows(topology.node.server(), topology.source(), "SELECT * FROM game_party_command_receipt"),
        "source_orders": topology.cli.rows(topology.node.server(), topology.source(), "SELECT * FROM pkg_playerbots_companion_order"),
        "realm_groups": topology.cli.rows(topology.node.server(), &topology.realm, "SELECT * FROM game_group"),
        "realm_members": topology.cli.rows(topology.node.server(), &topology.realm, "SELECT * FROM game_group_member"),
        "realm_character_shards": topology.cli.rows(topology.node.server(), &topology.realm, "SELECT * FROM game_character_shard"),
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
        let source_one_party = stage_roles(&cli, node.server(), &source, 0, 0);
        let target_party = stage_roles(&cli, node.server(), &target, 1_000_000_000, 1);
        let source_two_party = stage_roles(&cli, node.server(), &source_two, 2_000_000_000, 2);
        cli.call(node.server(), &realm, "claim_operator", &[]);
        install_authority(
            &cli,
            node.server(),
            &realm,
            &target,
            &target_party,
            &source_one_party,
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
#[allow(clippy::too_many_lines)] // One durable step per crash, Transfer, and replay boundary.
fn companion_command_receipts_recover_both_gateway_crash_boundaries() {
    let topology = CommandTopology::new("party-command-crash-recovery");
    let destination_map = topology.target_party.map_id + 1;
    let shard_map = format!(
        "{}:*={},{}:*={}",
        topology.target_party.map_id,
        topology.target,
        destination_map,
        topology.source()
    );
    let _environment = TopologyEnv::install(&shard_map, &topology.realm);
    let (_runtime, source) = topology.coordinator(topology.source(), "party-command-source-one");
    let target = source.shard_handle(&topology.target).unwrap();
    publish_authority_locators(&topology, &source);

    let claimed = queue(
        &topology.cli,
        topology.node.server(),
        topology.source(),
        &topology.actor_one,
        &format!("follow|{}", topology.target_party.warrior),
    );
    let claimed_intent = cached_intent(&source, claimed);
    source.claim_party_command_intent(claimed, 101).unwrap();
    let claimed_replies = terminal_messages_for(
        &topology.cli,
        topology.node.server(),
        topology.source(),
        claimed,
    );
    evidence(&topology, "claim-without-ack");
    assert!(claimed_replies.is_empty());
    std::thread::sleep(std::time::Duration::from_millis(2_100));
    assert_eq!(
        party::run_party_command_intent(&source, &claimed_intent, 102).unwrap(),
        CompanionCommandOutcome::Applied
    );
    let claimed_replies = terminal_messages_for(
        &topology.cli,
        topology.node.server(),
        topology.source(),
        claimed,
    );
    evidence(&topology, "claim-lease-recovered");
    assert_eq!(claimed_replies, vec![format!("{claimed}|Applied")]);

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
        issuer_sequence: target_intent.issuer_sequence,
        group_id: authority.group_id,
        leader_guid: authority.leader_guid,
        members: authority.members,
        kind: target_intent.kind,
        bot_guid: target_intent.bot_guid,
        authority_member_guid: target_intent.authority_member_guid,
        exact_target_guid: target_intent.exact_target_guid,
        expires_micros: target_intent.expires_micros,
        receipt_retain_until_micros: target_intent
            .expires_micros
            .saturating_add(lyracore_shared::group::COMMAND_RESULT_WINDOW_MICROS),
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
    let target_replies = terminal_messages_for(
        &topology.cli,
        topology.node.server(),
        topology.source(),
        target_applied,
    );
    evidence(&topology, "target-receipt-without-source-ack");
    assert_ne!(after_target["revision"], after_claim["revision"]);
    assert!(target_replies.is_empty());

    let mut rollover_outcomes = Vec::new();
    for sequence in 0..9 {
        let mut rollover = admitted.clone();
        rollover.intent_id = target_applied + 1_000 + sequence;
        rollover.issuer_sequence = admitted.issuer_sequence + 1 + sequence;
        rollover.kind = if sequence % 2 == 0 { 0 } else { 1 };
        rollover_outcomes.push(target.apply_admitted_party_command(&rollover));
    }
    let after_rollover = row(
        &topology.cli,
        topology.node.server(),
        &topology.target,
        &format!(
            "SELECT revision, history FROM pkg_playerbots_companion_order WHERE character_guid = {}",
            topology.target_party.warrior
        ),
    );
    evidence(&topology, "target-history-rolled-before-transfer");
    assert!(
        rollover_outcomes
            .iter()
            .all(|outcome| matches!(outcome, Ok(CompanionCommandOutcome::Applied))),
        "history rollover outcomes: {rollover_outcomes:?}"
    );
    assert_ne!(after_rollover, after_target);
    assert!(!after_rollover["history"].contains(&format!("intent_id = {target_applied},")));

    install_authority(
        &topology.cli,
        topology.node.server(),
        &topology.realm,
        &topology.target,
        &topology.target_party,
        &topology.source_two_party,
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
    let second_source_intent = cached_intent(&source_two, same_numeric_id);
    let second_source_outcome =
        party::run_party_command_intent(&source_two, &second_source_intent, 301);
    let before_delayed_retry = row(
        &topology.cli,
        topology.node.server(),
        &topology.target,
        &format!(
            "SELECT revision, history FROM pkg_playerbots_companion_order WHERE character_guid = {}",
            topology.target_party.warrior
        ),
    );
    evidence(&topology, "distinct-source-receipt-before-transfer");
    assert_eq!(
        same_numeric_id, claimed,
        "fresh source databases must allocate the same numeric intent id for this collision case"
    );
    assert_ne!(
        second_source_intent.source_identity,
        claimed_intent.source_identity
    );
    assert_eq!(
        second_source_outcome.unwrap(),
        CompanionCommandOutcome::Applied
    );
    assert_ne!(before_delayed_retry, after_rollover);

    topology.cli.call(
        topology.node.server(),
        &topology.target,
        "debug_bot_transfer",
        &[
            &topology.target_party.warrior.to_string(),
            &destination_map.to_string(),
            "0",
            "1200",
            "1200",
            "50",
            "0",
            r#""party-command-receipt""#,
        ],
    );
    let (destination_visible, committed_destination) = transfer_destination_observation(
        &topology,
        &target,
        topology.target_party.warrior,
        destination_map,
        0,
    );
    evidence(&topology, "transfer-destination-visible-before-escrow");
    assert!(destination_visible);
    assert_eq!(committed_destination["map_id"], destination_map.to_string());
    assert_eq!(committed_destination["pending_instance_id"], "0");
    let transfer_plan = target
        .character_destination(topology.target_party.warrior)
        .expect("Gateway must retain the observed transfer destination");
    target.begin_transfer(&transfer_plan).unwrap();
    let mut delayed_admitted = admitted.clone();
    delayed_admitted.intent_id = target_applied.saturating_add(1_000_000);
    let in_transit = target
        .apply_admitted_party_command(&delayed_admitted)
        .unwrap_err();
    evidence(&topology, "old-holder-rejects-after-transfer-begins");
    assert!(in_transit.to_string().contains("TransferInProgress"));
    assert!(topology
        .cli
        .rows(
            topology.node.server(),
            &topology.target,
            &format!(
                "SELECT id FROM game_party_command_receipt WHERE intent_id = {}",
                delayed_admitted.intent_id
            ),
        )
        .is_empty());
    crate::world::transfer::run_bot_transfer(
        &target,
        topology.target_party.warrior,
        destination_map,
        0,
        "party-command-receipt",
    )
    .unwrap();
    assert!(poll_until(POLL_TIMEOUT, || topology
        .cli
        .rows(
            topology.node.server(),
            topology.source(),
            &format!(
                "SELECT guid FROM game_character WHERE guid = {}",
                topology.target_party.warrior
            ),
        )
        .len()
        == 1));
    let moved_receipt = row(
        &topology.cli,
        topology.node.server(),
        topology.source(),
        &format!(
            "SELECT outcome FROM game_party_command_receipt WHERE intent_id = {target_applied}"
        ),
    );
    evidence(&topology, "target-receipt-transferred-before-retry");
    assert!(moved_receipt["outcome"]
        .to_ascii_lowercase()
        .contains("applied"));
    assert!(topology
        .cli
        .rows(
            topology.node.server(),
            &topology.target,
            &format!(
                "SELECT id FROM game_party_command_receipt WHERE intent_id = {target_applied}"
            ),
        )
        .is_empty());
    let delayed_old_holder = target
        .apply_admitted_party_command(&delayed_admitted)
        .unwrap_err();
    evidence(&topology, "old-holder-rejects-after-transfer-finish");
    assert!(delayed_old_holder
        .to_string()
        .contains("NotCharacterHolder"));
    assert!(topology
        .cli
        .rows(
            topology.node.server(),
            &topology.target,
            &format!(
                "SELECT id FROM game_party_command_receipt WHERE intent_id = {}",
                delayed_admitted.intent_id
            ),
        )
        .is_empty());
    assert_eq!(
        party::run_party_command_intent(&source, &target_intent, 201).unwrap(),
        CompanionCommandOutcome::Applied
    );
    let after_transfer_retry = row(
        &topology.cli,
        topology.node.server(),
        topology.source(),
        &format!(
            "SELECT revision, history FROM pkg_playerbots_companion_order WHERE character_guid = {}",
            topology.target_party.warrior
        ),
    );
    let target_replies = terminal_messages_for(
        &topology.cli,
        topology.node.server(),
        topology.source(),
        target_applied,
    );
    evidence(&topology, "target-receipt-source-finalized");
    assert_eq!(after_transfer_retry, before_delayed_retry);
    assert_eq!(target_replies, vec![format!("{target_applied}|Applied")]);
    evidence(&topology, "target-receipt-after-history-rollover");
    assert_eq!(
        source.apply_admitted_party_command(&admitted).unwrap(),
        CompanionCommandOutcome::Applied
    );
    let recovered = row(
        &topology.cli,
        topology.node.server(),
        topology.source(),
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

    let receipts = topology.cli.rows(
        topology.node.server(),
        topology.source(),
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
#[allow(clippy::too_many_lines)] // One durable step per issuer Transfer and causal-order boundary.
fn companion_command_issuer_sequence_survives_transfer_and_fences_an_older_source() {
    let topology = CommandTopology::new("party-command-issuer-sequence-transfer");
    let shard_map = format!("{}:*={}", topology.target_party.map_id, topology.target);
    let _environment = TopologyEnv::install(&shard_map, &topology.realm);
    let (runtime, source) = topology.coordinator(topology.source(), "party-command-old-source");
    let target = source.shard_handle(&topology.target).unwrap();
    publish_authority_locators(&topology, &source);

    let older_id = queue(
        &topology.cli,
        topology.node.server(),
        topology.source(),
        &topology.actor_one,
        &format!("stay|{}", topology.target_party.warrior),
    );
    let older = cached_intent(&source, older_id);
    assert_eq!(older.issuer_sequence, 1);
    target
        .provision_account("PB009SOURCEONE", &[], &[])
        .unwrap();
    let target_account = row(
        &topology.cli,
        topology.node.server(),
        &topology.target,
        "SELECT id FROM game_account WHERE username = 'PB009SOURCEONE'",
    );
    let actor: serde_json::Value = serde_json::from_str(&topology.actor_one).unwrap();
    let source_account_id = actor["ownership"]["some"]["account_id"].to_string();
    let ownership = serde_json::to_string(&actor["ownership"]["some"]).unwrap();
    topology.cli.call(
        topology.node.server(),
        topology.source(),
        "release_account_claim",
        &[&ownership],
    );
    let released_claim = row(
        &topology.cli,
        topology.node.server(),
        topology.source(),
        &format!(
            "SELECT account_id, generation, request_nonce, character_guid, closed FROM game_account_claim WHERE character_guid = {}",
            topology.source_one_party.leader
        ),
    );
    let source_character = row(
        &topology.cli,
        topology.node.server(),
        topology.source(),
        &format!(
            "SELECT guid, name FROM game_character WHERE guid = {}",
            topology.source_one_party.leader
        ),
    );
    let source_character_name = sql_string(&source_character["name"]);
    let destination_name_collision = topology.cli.rows(
        topology.node.server(),
        &topology.target,
        &format!(
            "SELECT guid FROM game_character WHERE name = '{}'",
            source_character_name
        ),
    );
    let source_range = row(
        &topology.cli,
        topology.node.server(),
        topology.source(),
        "SELECT base, size FROM game_guid_range WHERE id = 0",
    );
    let destination_range = row(
        &topology.cli,
        topology.node.server(),
        &topology.target,
        "SELECT base, size FROM game_guid_range WHERE id = 0",
    );
    evidence(&topology, "issuer-command-queued-before-transfer");
    assert_eq!(released_claim["account_id"], source_account_id);
    assert_eq!(
        released_claim["generation"],
        actor["ownership"]["some"]["generation"].to_string()
    );
    assert_eq!(
        released_claim["request_nonce"],
        actor["ownership"]["some"]["request_nonce"].to_string()
    );
    assert_eq!(
        released_claim["character_guid"],
        topology.source_one_party.leader.to_string()
    );
    assert_eq!(released_claim["closed"], "true");
    assert_eq!(
        target_account["id"], source_account_id,
        "the private destination must preserve the transferred Account id"
    );
    assert!(destination_name_collision.is_empty());
    assert_eq!(source_range["base"], "0");
    assert_eq!(destination_range["base"], "1000000000");
    assert_eq!(source_range["size"], "1000000000");
    assert_eq!(destination_range["size"], "1000000000");

    topology.cli.call(
        topology.node.server(),
        topology.source(),
        "debug_bot_transfer",
        &[
            &topology.source_one_party.leader.to_string(),
            &topology.target_party.map_id.to_string(),
            "0",
            "1200",
            "1200",
            "50",
            "0",
            r#""party-command-issuer""#,
        ],
    );
    crate::world::transfer::run_bot_transfer(
        &source,
        topology.source_one_party.leader,
        topology.target_party.map_id,
        0,
        "party-command-issuer",
    )
    .unwrap();
    assert!(poll_until(POLL_TIMEOUT, || topology
        .cli
        .rows(
            topology.node.server(),
            &topology.target,
            &format!(
                "SELECT guid FROM game_character WHERE guid = {}",
                topology.source_one_party.leader
            ),
        )
        .len()
        == 1));
    assert!(topology
        .cli
        .rows(
            topology.node.server(),
            topology.source(),
            &format!(
                "SELECT character_guid FROM game_party_command_issuer WHERE character_guid = {}",
                topology.source_one_party.leader
            ),
        )
        .is_empty());
    let moved_issuer = row(
        &topology.cli,
        topology.node.server(),
        &topology.target,
        &format!(
            "SELECT last_sequence FROM game_party_command_issuer WHERE character_guid = {}",
            topology.source_one_party.leader
        ),
    );
    assert_eq!(moved_issuer["last_sequence"], "1");

    let (moved_actor, _moved_session) = enter_transferred_actor(
        &topology,
        &runtime,
        &target,
        topology.source_one_party.leader,
    );
    evidence(&topology, "issuer-transfer-session-visible");
    let newer_id = queue(
        &topology.cli,
        topology.node.server(),
        &topology.target,
        &moved_actor,
        &format!("follow|{}", topology.target_party.warrior),
    );
    let newer = cached_intent(&target, newer_id);
    assert_eq!(newer.issuer_guid, older.issuer_guid);
    assert_eq!(newer.issuer_sequence, 2);
    assert_ne!(newer.source_identity, older.source_identity);
    assert_eq!(
        party::run_party_command_intent(&target, &newer, 701).unwrap(),
        CompanionCommandOutcome::Applied
    );
    let after_newer = row(
        &topology.cli,
        topology.node.server(),
        &topology.target,
        &format!(
            "SELECT revision, order, history FROM pkg_playerbots_companion_order WHERE character_guid = {}",
            topology.target_party.warrior
        ),
    );

    assert_eq!(
        party::run_party_command_intent(&source, &older, 702).unwrap(),
        CompanionCommandOutcome::Superseded
    );
    let after_older = row(
        &topology.cli,
        topology.node.server(),
        &topology.target,
        &format!(
            "SELECT revision, order, history FROM pkg_playerbots_companion_order WHERE character_guid = {}",
            topology.target_party.warrior
        ),
    );
    let older_receipt = target
        .confirm_party_command_receipt(older.source_identity, older_id)
        .unwrap();
    let older_reply = topology
        .cli
        .json_rows(
            topology.node.server(),
            topology.source(),
            "SELECT payload FROM game_addon_message WHERE cmd = 'playerbots.order.result'",
        )
        .into_iter()
        .find(|row| {
            row.first()
                .and_then(serde_json::Value::as_str)
                .is_some_and(|payload| payload.starts_with(&format!("{older_id}|")))
        })
        .expect("old source result reply missing after issuer Transfer");
    evidence(
        &topology,
        "issuer-transfer-newer-command-fences-older-source",
    );
    assert_eq!(after_older, after_newer);
    assert!(after_older["order"].to_ascii_lowercase().contains("follow"));
    assert_eq!(older_receipt, Some(CompanionCommandOutcome::Superseded));
    assert!(older_reply[0]
        .as_str()
        .is_some_and(|payload| payload.contains("Superseded")));
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1, the full Package union, and the Wasm toolchain"]
fn companion_command_receipt_stays_with_a_same_database_transfer() {
    let topology = CommandTopology::new("party-command-same-database-transfer");
    let destination_map = topology.target_party.map_id + 1;
    let shard_map = format!(
        "{}:*={},{}:*={}",
        topology.target_party.map_id, topology.target, destination_map, topology.target
    );
    let _environment = TopologyEnv::install(&shard_map, &topology.realm);
    let (_runtime, source) =
        topology.coordinator(topology.source(), "party-command-same-database-source");
    let target = source.shard_handle(&topology.target).unwrap();
    publish_authority_locators(&topology, &source);
    let intent_id = queue(
        &topology.cli,
        topology.node.server(),
        topology.source(),
        &topology.actor_one,
        &format!("stay|{}", topology.target_party.warrior),
    );
    let intent = cached_intent(&source, intent_id);
    assert_eq!(
        party::run_party_command_intent(&source, &intent, 601).unwrap(),
        CompanionCommandOutcome::Applied
    );
    topology.cli.call(
        topology.node.server(),
        &topology.target,
        "debug_bot_transfer",
        &[
            &topology.target_party.warrior.to_string(),
            &destination_map.to_string(),
            "0",
            "1200",
            "1200",
            "50",
            "0",
            r#""party-command-same-database""#,
        ],
    );
    let (destination_visible, committed_destination) = transfer_destination_observation(
        &topology,
        &target,
        topology.target_party.warrior,
        destination_map,
        0,
    );
    evidence(&topology, "same-database-transfer-destination-visible");
    assert!(destination_visible);
    assert_eq!(committed_destination["map_id"], destination_map.to_string());
    assert_eq!(committed_destination["pending_instance_id"], "0");
    crate::world::transfer::run_bot_transfer(
        &target,
        topology.target_party.warrior,
        destination_map,
        0,
        "party-command-same-database",
    )
    .unwrap();
    let receipt = row(
        &topology.cli,
        topology.node.server(),
        &topology.target,
        &format!("SELECT outcome FROM game_party_command_receipt WHERE intent_id = {intent_id}"),
    );
    evidence(&topology, "same-database-transfer-retains-receipt");
    assert!(receipt["outcome"].to_ascii_lowercase().contains("applied"));
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1, the full Package union, and the Wasm toolchain"]
fn companion_command_lost_receipt_after_guarantee_reports_unknown_without_reapply() {
    let topology = CommandTopology::new("party-command-outcome-unknown");
    let shard_map = format!("{}:*={}", topology.target_party.map_id, topology.target);
    let _environment = TopologyEnv::install(&shard_map, &topology.realm);
    let (_runtime, source) =
        topology.coordinator(topology.source(), "party-command-unknown-source");
    let target = source.shard_handle(&topology.target).unwrap();
    let intent_id = queue(
        &topology.cli,
        topology.node.server(),
        topology.source(),
        &topology.actor_one,
        &format!("stay|{}", topology.target_party.warrior),
    );
    let intent = cached_intent(&source, intent_id);
    source.claim_party_command_intent(intent_id, 711).unwrap();
    let authority = source
        .realm_core()
        .unwrap()
        .group_roster(topology.source_one_party.leader)
        .unwrap();
    let admitted = party::AdmittedCompanionCommand {
        source_identity: intent.source_identity,
        intent_id,
        issuer_guid: intent.issuer_guid,
        issuer_sequence: intent.issuer_sequence,
        group_id: authority.group_id,
        leader_guid: authority.leader_guid,
        members: authority.members,
        kind: intent.kind,
        bot_guid: intent.bot_guid,
        authority_member_guid: intent.authority_member_guid,
        exact_target_guid: intent.exact_target_guid,
        expires_micros: intent.expires_micros,
        receipt_retain_until_micros: intent
            .expires_micros
            .saturating_add(lyracore_shared::group::COMMAND_RESULT_WINDOW_MICROS),
    };
    assert_eq!(
        target.apply_admitted_party_command(&admitted).unwrap(),
        CompanionCommandOutcome::Applied
    );
    let applied_order = row(
        &topology.cli,
        topology.node.server(),
        &topology.target,
        &format!(
            "SELECT revision, history FROM pkg_playerbots_companion_order WHERE character_guid = {}",
            topology.target_party.warrior
        ),
    );
    topology.cli.call(
        topology.node.server(),
        &topology.target,
        "playerbots_fixture_command_release_receipt",
        &[&topology.target_party.warrior.to_string()],
    );
    topology.cli.call(
        topology.node.server(),
        topology.source(),
        "playerbots_fixture_command_expire_after_receipt_window",
        &[&intent_id.to_string()],
    );
    assert!(poll_until(POLL_TIMEOUT, || {
        cached_intent(&source, intent_id).expires_micros < intent.expires_micros
    }));
    let expired = cached_intent(&source, intent_id);
    assert_eq!(
        party::finish_expired_party_command_intent(&source, &expired, 711).unwrap(),
        CompanionCommandOutcome::OutcomeUnknown
    );
    let after_unknown = row(
        &topology.cli,
        topology.node.server(),
        &topology.target,
        &format!(
            "SELECT revision, history FROM pkg_playerbots_companion_order WHERE character_guid = {}",
            topology.target_party.warrior
        ),
    );
    let responses = topology.cli.json_rows(
        topology.node.server(),
        topology.source(),
        "SELECT payload FROM game_addon_message WHERE cmd = 'playerbots.order.result'",
    );
    evidence(&topology, "applied-receipt-lost-after-guarantee");
    assert_eq!(after_unknown, applied_order);
    assert_eq!(
        responses,
        vec![vec![serde_json::Value::String(format!(
            "{intent_id}|OutcomeUnknown"
        ))]]
    );
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1, the full Package union, and the Wasm toolchain"]
fn companion_command_realm_admission_rejects_a_changed_unrelated_member() {
    let topology = CommandTopology::new("party-command-roster-certificate");
    let shard_map = format!("{}:*={}", topology.target_party.map_id, topology.target);
    let _environment = TopologyEnv::install(&shard_map, &topology.realm);
    let (_runtime, source) = topology.coordinator(topology.source(), "party-command-roster-source");
    let realm = source.realm_core().unwrap();
    let intent_id = queue(
        &topology.cli,
        topology.node.server(),
        topology.source(),
        &topology.actor_one,
        &format!("follow|{}", topology.target_party.warrior),
    );
    let intent = cached_intent(&source, intent_id);
    let authority = realm
        .group_roster(topology.source_one_party.leader)
        .unwrap();
    let args = [
        topology.target_party.warrior.to_string(),
        topology.target_party.priest.to_string(),
        topology.target_party.mage.to_string(),
        topology.source_one_party.leader.to_string(),
        "3".to_string(),
    ];
    topology.cli.call(
        topology.node.server(),
        &topology.realm,
        "playerbots_fixture_orders_party",
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
    );

    let outcome = realm
        .admit_party_command_authority(
            authority.group_id,
            intent.issuer_guid,
            intent.bot_guid,
            intent.authority_member_guid,
            authority.members,
        )
        .unwrap();

    evidence(&topology, "realm-roster-changed-before-admission");
    assert_eq!(outcome, CompanionCommandOutcome::StalePartyMirror);
    assert!(topology
        .cli
        .rows(
            topology.node.server(),
            &topology.target,
            "SELECT * FROM pkg_playerbots_companion_order",
        )
        .is_empty());
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
    let waiting_replies = terminal_messages_for(
        &topology.cli,
        topology.node.server(),
        topology.source(),
        waiting_id,
    );
    evidence(&topology, "capacity-wait-no-ack");
    assert_eq!(pending["pending"], "true");
    assert!(pending["state"].to_ascii_lowercase().contains("pending"));
    assert_eq!(after_wait, before);
    assert!(waiting_replies.is_empty());

    let mut blocked = Vec::new();
    for _ in 0..16 {
        blocked.push(queue(
            &topology.cli,
            topology.node.server(),
            topology.source(),
            &topology.actor_one,
            &payload,
        ));
    }
    let lane_count = u64::from(lyracore_shared::group::COMMAND_DISPATCH_LANES);
    let warrior_lane = topology.target_party.warrior % lane_count;
    let later_bot = [topology.target_party.priest, topology.target_party.mage]
        .into_iter()
        .find(|guid| *guid % lane_count != warrior_lane)
        .expect("role fixture needs a bot in another command dispatch lane");
    let later_id = queue(
        &topology.cli,
        topology.node.server(),
        topology.source(),
        &topology.actor_one,
        &format!("follow|{later_bot}"),
    );
    source.dispatch_party_command_intents();
    let later = row(
        &topology.cli,
        topology.node.server(),
        topology.source(),
        &format!("SELECT pending, state FROM game_party_command_intent WHERE id = {later_id}"),
    );
    evidence(&topology, "capacity-fair-dispatch-beyond-one-window");
    assert_eq!(later["pending"], "false");
    assert!(later["state"].to_ascii_lowercase().contains("applied"));
    assert!(blocked.iter().all(|id| {
        row(
            &topology.cli,
            topology.node.server(),
            topology.source(),
            &format!("SELECT pending FROM game_party_command_intent WHERE id = {id}"),
        )["pending"]
            == "true"
    }));

    for (offset, intent_id) in blocked.iter().enumerate() {
        let claim_token = 4_000 + offset as u64;
        source
            .claim_party_command_intent(*intent_id, claim_token)
            .unwrap();
        source
            .defer_party_command_intent(*intent_id, claim_token)
            .unwrap();
    }
    let retry_head = row(
        &topology.cli,
        topology.node.server(),
        topology.source(),
        &format!(
            "SELECT head_intent_id FROM game_party_command_dispatch_lane WHERE lane = {warrior_lane}"
        ),
    );
    evidence(&topology, "capacity-wait-rotated-back-to-head");
    assert_eq!(retry_head["head_intent_id"], waiting_id.to_string());

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
    let waiting_replies = terminal_messages_for(
        &topology.cli,
        topology.node.server(),
        topology.source(),
        waiting_id,
    );
    evidence(&topology, "capacity-retry-applied");
    assert_eq!(waiting_replies, vec![format!("{waiting_id}|Unchanged")]);

    let expiry_id = blocked[0];
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
    let expired = cached_intent_when(&source, expiry_id, |row| {
        row.expires_micros < expiry.expires_micros
    });
    let competing_finish =
        party::finish_expired_party_command_intent(&source, &expired, 3_002).unwrap_err();
    assert!(competing_finish.to_string().contains("ClaimLost"));
    assert_eq!(
        party::finish_expired_party_command_intent(&source, &expired, 3_001).unwrap(),
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
