#![cfg(target_os = "linux")]

//! Real Gateway process recovery for one session-less cross-Shard Transfer.

#[path = "../../module/tests/support/mod.rs"]
mod support;

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const DESTINATION_MAP: u32 = 36;
const DESTINATION_INSTANCE: u64 = 5_098_078;
const GROUP: u64 = 5_098_000;
const ABORT_STEPS: [&str; 12] = [
    "bind_bot_transfer_locator",
    "sync_transfer_pending",
    "begin_transfer",
    "ensure_instance",
    "import_character_blob",
    "confirm_import",
    "finish_transfer",
    "publish_shard_index",
    "sync_transfer_arrival",
    "mark_bot_transfer_arrival_ready",
    "release_transfer",
    "evict_instance_population",
];

pub(crate) struct TransferTopology {
    pub(crate) node: support::Standalone,
    pub(crate) source_db: String,
    pub(crate) destination_db: String,
    pub(crate) realm_db: String,
}

pub(crate) struct TransferredBot {
    pub(crate) guid: u64,
    pub(crate) leader_guid: u64,
    pub(crate) intent_id: u64,
    pub(crate) generation: u64,
    pub(crate) objective_identity: u64,
}

impl TransferTopology {
    fn stage(name: &str) -> (Self, TransferredBot) {
        let mut node = support::Standalone::start_persistent(name);
        let source_db = node.shard_name().to_owned();
        let destination_db = format!("{source_db}-destination");
        let realm_db = format!("{source_db}-realm");
        let wasm = support::module_bytes();
        node.publish_module_bytes(wasm);
        node.publish_named_module_bytes(&destination_db, wasm);
        node.publish_named_module_bytes(&realm_db, wasm);
        let topology = Self {
            node,
            source_db,
            destination_db,
            realm_db,
        };
        for database in [
            topology.source_db.as_str(),
            topology.destination_db.as_str(),
            topology.realm_db.as_str(),
        ] {
            topology.call(database, "claim_operator", &[]);
        }
        topology.call(&topology.source_db, "install_guid_range", &["1000000"]);
        topology.call(
            &topology.source_db,
            "playerbots_spawn",
            &["4", "1200", "1200", "50"],
        );
        let mut bots = topology.query(
            &topology.source_db,
            "SELECT character_guid, class, role FROM pkg_playerbots_bot",
        );
        bots.sort_by_key(|bot| bot["character_guid"].parse::<u64>().unwrap());
        let companion = bots
            .iter()
            .find(|bot| bot["class"] == "1" && bot["role"] == "0")
            .expect("Warrior Tank missing")["character_guid"]
            .parse::<u64>()
            .unwrap();
        let leader = bots
            .iter()
            .rev()
            .find(|bot| bot["class"] == "1" && bot["role"] == "0")
            .expect("Warrior leader missing")["character_guid"]
            .parse::<u64>()
            .unwrap();
        let priest = bots
            .iter()
            .find(|bot| bot["class"] == "5" && bot["role"] == "1")
            .expect("Priest Healer missing")["character_guid"]
            .clone();
        let mage = bots
            .iter()
            .find(|bot| bot["class"] == "8" && bot["role"] == "2")
            .expect("Mage Damage missing")["character_guid"]
            .clone();
        topology.call(
            &topology.source_db,
            "playerbots_fixture_roles_stage",
            &[&companion.to_string(), &priest, &mage, &leader.to_string()],
        );
        for guid in [companion.to_string(), priest, mage] {
            topology.call(
                &topology.source_db,
                "playerbots_fixture_provision_steps",
                &[&guid, "32"],
            );
        }
        topology.call(
            &topology.realm_db,
            "playerbots_transfer_gateway_realm_stage",
            &[
                &companion.to_string(),
                &leader.to_string(),
                "0",
                "0",
                &DESTINATION_MAP.to_string(),
                &DESTINATION_INSTANCE.to_string(),
            ],
        );
        (
            topology,
            TransferredBot {
                guid: companion,
                leader_guid: leader,
                intent_id: 0,
                generation: 0,
                objective_identity: 0,
            },
        )
    }

    fn begin_transfer(&self, bot: &mut TransferredBot) {
        self.call(
            &self.source_db,
            "playerbots_transfer_fixture_stage",
            &[&bot.guid.to_string(), &bot.leader_guid.to_string(), "2"],
        );
        self.call(
            &self.source_db,
            "playerbots_fixture_runner_pass_once",
            &[&bot.guid.to_string()],
        );
        let intents = self.query(
            &self.source_db,
            &format!(
                "SELECT id, bot_guid, controller_generation FROM game_bot_transfer_intent \
                 WHERE bot_guid = {}",
                bot.guid
            ),
        );
        let intent = exactly_one(&intents, "source Transfer Intent");
        bot.intent_id = parse_u64(intent, "id");
        bot.generation = parse_u64(intent, "controller_generation");
        let runners = self.query(
            &self.source_db,
            &format!(
                "SELECT objective FROM pkg_playerbots_runner WHERE character_guid = {}",
                bot.guid
            ),
        );
        let runner = exactly_one(&runners, "source Runner");
        bot.objective_identity = embedded_u64(&runner["objective"], "identity").unwrap_or(0);
    }

    pub(crate) fn call(&self, database: &str, reducer: &str, args: &[&str]) {
        self.node.assert_call_database(database, reducer, args);
    }

    pub(crate) fn query(&self, database: &str, sql: &str) -> Vec<BTreeMap<String, String>> {
        self.node.query_database_rows(database, sql)
    }

    fn snapshot(&self, bot: &TransferredBot, phase: &str) -> serde_json::Value {
        serde_json::json!({
            "phase": phase,
            "bot": {
                "guid": bot.guid,
                "leader_guid": bot.leader_guid,
                "intent_id": bot.intent_id,
                "generation": bot.generation,
                "objective_identity": bot.objective_identity,
            },
            "source": {
                "character": self.query(&self.source_db, &format!("SELECT guid, map_id, pending_instance_id FROM game_character WHERE guid = {}", bot.guid)),
                "live": self.query(&self.source_db, &format!("SELECT guid, map_id, instance_id FROM game_world_entity WHERE guid = {}", bot.guid)),
                "escrow": self.query(&self.source_db, &format!("SELECT * FROM game_transfer_out WHERE character_guid = {}", bot.guid)),
                "intent": self.query(&self.source_db, &format!("SELECT * FROM game_bot_transfer_intent WHERE bot_guid = {}", bot.guid)),
                "runner": self.query(&self.source_db, &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {}", bot.guid)),
                "instance": self.query(&self.source_db, &format!("SELECT * FROM game_instance WHERE instance_id = {DESTINATION_INSTANCE}")),
            },
            "destination": {
                "character": self.query(&self.destination_db, &format!("SELECT guid, map_id, pending_instance_id FROM game_character WHERE guid = {}", bot.guid)),
                "live": self.query(&self.destination_db, &format!("SELECT guid, map_id, instance_id FROM game_world_entity WHERE guid = {}", bot.guid)),
                "arrival": self.query(&self.destination_db, &format!("SELECT * FROM game_transfer_in WHERE character_guid = {}", bot.guid)),
                "runner": self.query(&self.destination_db, &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {}", bot.guid)),
                "group": self.query(&self.destination_db, &format!("SELECT * FROM game_group WHERE group_id = {GROUP}")),
                "members": self.query(&self.destination_db, &format!("SELECT * FROM game_group_member WHERE group_id = {GROUP}")),
                "partitions": self.query(&self.destination_db, &format!("SELECT * FROM game_group_member_partition WHERE group_id = {GROUP}")),
                "instance": self.query(&self.destination_db, &format!("SELECT * FROM game_instance WHERE instance_id = {DESTINATION_INSTANCE}")),
            },
            "realm": {
                "locator": self.query(&self.realm_db, &format!("SELECT * FROM game_character_shard WHERE character_guid = {}", bot.guid)),
                "group": self.query(&self.realm_db, &format!("SELECT * FROM game_group WHERE group_id = {GROUP}")),
                "members": self.query(&self.realm_db, &format!("SELECT * FROM game_group_member WHERE group_id = {GROUP}")),
                "partitions": self.query(&self.realm_db, &format!("SELECT * FROM game_group_member_partition WHERE group_id = {GROUP}")),
            },
        })
    }

    fn save(
        &self,
        bot: &TransferredBot,
        phase: &str,
        extra: serde_json::Value,
    ) -> serde_json::Value {
        let core = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let package = core.join("packages/playerbots");
        let evidence = serde_json::json!({
            "tested_core": git(core, &["rev-parse", "HEAD"]),
            "tested_package": git(&package, &["rev-parse", "HEAD"]),
            "core_dirty": !git(core, &["status", "--porcelain"]).is_empty(),
            "package_dirty": !git(&package, &["status", "--porcelain"]).is_empty(),
            "module_wasm_bytes": support::module_bytes().len(),
            "state": self.snapshot(bot, phase),
            "extra": extra,
        });
        let path = support::log_dir().join(format!(
            "{}-gateway-transfer-{}.json",
            self.node.shard_name(),
            phase.replace('_', "-")
        ));
        fs::write(path, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
        evidence
    }

    fn gateway(&self, abort_after: Option<&str>, phase: &str) -> GatewayProcess {
        GatewayProcess::spawn(self, abort_after, phase)
    }
}

struct GatewayProcess {
    child: Option<Child>,
    log_path: PathBuf,
}

impl GatewayProcess {
    fn spawn(topology: &TransferTopology, abort_after: Option<&str>, phase: &str) -> Self {
        let log_path = support::log_dir().join(format!(
            "{}-gateway-transfer-{}.log",
            topology.node.shard_name(),
            phase.replace('_', "-")
        ));
        let log = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&log_path)
            .unwrap();
        let stderr = log.try_clone().unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_lyracore-gateway"));
        command
            .env("LYRACORE_SPACETIMEDB_URL", topology.node.server())
            .env("LYRACORE_DATABASE", &topology.source_db)
            .env("LYRACORE_REALM_CORE", &topology.realm_db)
            .env(
                "LYRACORE_SHARD_MAP",
                format!("{DESTINATION_MAP}:*={}", topology.destination_db),
            )
            .env("LYRACORE_COORDINATOR_TOKEN", topology.node.owner_token())
            .env("LYRACORE_LOGON_BIND", "127.0.0.1:0")
            .env("LYRACORE_WORLD_BIND", "127.0.0.1:0")
            .env("LYRACORE_GATEWAY_ID", "pb010-private-gateway")
            .env("RUST_LOG", "info")
            .env_remove("LYRACORE_SHARD_MAP_FILE")
            .env_remove("LYRACORE_TRANSFER_ABORT_AFTER")
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(stderr));
        if let Some(step) = abort_after {
            command.env("LYRACORE_TRANSFER_ABORT_AFTER", step);
        }
        let child = command.spawn().expect("failed to start private Gateway");
        Self {
            child: Some(child),
            log_path,
        }
    }

    fn wait_for_abort(&mut self, step: &str) -> String {
        let deadline = Instant::now() + support::POLL_TIMEOUT;
        let status = loop {
            if let Some(status) = self.child.as_mut().unwrap().try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "Gateway did not abort after {step}\n{}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(50));
        };
        let _ = self.child.take();
        let log = self.log();
        assert!(
            !status.success(),
            "Gateway exited successfully after {step}\n{log}"
        );
        assert!(
            log.contains(&format!("LYRACORE_TRANSFER_ABORT_AFTER={step}"))
                && log.contains("step committed"),
            "Gateway exited at another boundary after {step}\n{log}"
        );
        log
    }

    fn log(&self) -> String {
        fs::read_to_string(&self.log_path).unwrap_or_default()
    }

    fn stop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        if child.try_wait().unwrap().is_none() {
            child.kill().unwrap();
            child.wait().unwrap();
        }
    }
}

impl Drop for GatewayProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

fn exactly_one<'a>(
    rows: &'a [BTreeMap<String, String>],
    name: &str,
) -> &'a BTreeMap<String, String> {
    assert_eq!(rows.len(), 1, "expected one {name}, got {rows:?}");
    &rows[0]
}

fn parse_u64(row: &BTreeMap<String, String>, field: &str) -> u64 {
    row[field]
        .parse()
        .unwrap_or_else(|_| panic!("invalid {field} in {row:?}"))
}

fn embedded_u64(value: &str, field: &str) -> Option<u64> {
    let rest = value.split(&format!("{field} = ")).nth(1)?;
    rest.split(|character: char| !character.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

fn git(path: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(path)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn rows<'a>(evidence: &'a serde_json::Value, path: &[&str]) -> &'a Vec<serde_json::Value> {
    let mut value = evidence;
    for field in path {
        value = &value[*field];
    }
    value.as_array().unwrap()
}

fn assert_durable_phase(evidence: &serde_json::Value, step: &str) {
    let source_character = rows(evidence, &["state", "source", "character"]);
    let destination_character = rows(evidence, &["state", "destination", "character"]);
    let source_live = rows(evidence, &["state", "source", "live"]);
    let destination_live = rows(evidence, &["state", "destination", "live"]);
    assert!(
        source_character.len() + destination_character.len() >= 1,
        "no durable Character after {step}: {evidence}"
    );
    assert!(
        source_live.is_empty() || destination_live.is_empty(),
        "two live bodies after {step}: {evidence}"
    );
    assert_eq!(
        rows(evidence, &["state", "source", "intent"]).len(),
        1,
        "the crashed worker consumed its intent after {step}: {evidence}"
    );
    let position = ABORT_STEPS
        .iter()
        .position(|candidate| *candidate == step)
        .unwrap();
    let intent = &rows(evidence, &["state", "source", "intent"])[0];
    if position == 0 {
        assert_ne!(intent["source_locator_revision"], "0", "{evidence}");
    }
    if position >= 2 {
        assert_eq!(
            rows(evidence, &["state", "source", "escrow"]).len(),
            if position < 6 { 1 } else { 0 },
            "source Escrow phase differs after {step}: {evidence}"
        );
    }
    if position >= 3 {
        assert_eq!(
            rows(evidence, &["state", "destination", "instance"]).len(),
            1,
            "destination instance is absent after {step}: {evidence}"
        );
    }
    if position >= 4 {
        assert_eq!(
            destination_character.len(),
            1,
            "destination Character is absent after {step}: {evidence}"
        );
        assert_eq!(
            rows(evidence, &["state", "destination", "arrival"]).len(),
            if position < 10 { 1 } else { 0 },
            "destination arrival phase differs after {step}: {evidence}"
        );
    }
    if position >= 6 {
        assert!(
            source_character.is_empty(),
            "source Character remains after {step}: {evidence}"
        );
    }
    if position >= 8 {
        assert_eq!(
            rows(evidence, &["state", "destination", "group"]).len(),
            1,
            "party mirror is absent after {step}: {evidence}"
        );
    }
    let locator = &rows(evidence, &["state", "realm", "locator"])[0];
    if position == 1 {
        assert_eq!(locator["transfer_pending"], "true", "{evidence}");
    }
    if position >= 7 {
        assert_eq!(locator["transfer_pending"], "false", "{evidence}");
        assert_eq!(locator["map_id"], DESTINATION_MAP.to_string(), "{evidence}");
        assert_eq!(
            locator["instance_id"],
            DESTINATION_INSTANCE.to_string(),
            "{evidence}"
        );
    }
    if position >= 9 {
        assert_eq!(intent["arrival_ready"], "true", "{evidence}");
    }
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, the playerbots Package, and the Gateway binary"]
fn playerbots_gateway_process_restart_resumes_every_committed_transfer_phase() {
    let (topology, mut bot) = TransferTopology::stage("playerbots-transfer-gateway-restart");
    topology.begin_transfer(&mut bot);

    for step in ABORT_STEPS {
        let mut gateway = topology.gateway(Some(step), step);
        let log = gateway.wait_for_abort(step);
        let evidence = topology.save(&bot, step, serde_json::json!({ "gateway_log": log }));
        assert_durable_phase(&evidence, step);
    }

    let mut gateway = topology.gateway(None, "completed");
    let completed = support::poll_until(support::POLL_TIMEOUT, || {
        topology
            .query(
                &topology.source_db,
                &format!(
                    "SELECT id FROM game_bot_transfer_intent WHERE bot_guid = {}",
                    bot.guid
                ),
            )
            .is_empty()
            && topology
                .query(
                    &topology.destination_db,
                    &format!(
                        "SELECT transfer_id FROM game_transfer_in WHERE character_guid = {}",
                        bot.guid
                    ),
                )
                .is_empty()
    });
    let evidence = topology.save(
        &bot,
        "completed",
        serde_json::json!({ "gateway_log": gateway.log() }),
    );
    gateway.stop();
    assert!(completed, "final worker did not complete: {evidence}");
    assert_eq!(
        rows(&evidence, &["state", "destination", "character"]).len(),
        1,
        "{evidence}"
    );
    assert_eq!(
        rows(&evidence, &["state", "destination", "live"]).len(),
        1,
        "{evidence}"
    );
    assert!(
        rows(&evidence, &["state", "source", "character"]).is_empty(),
        "{evidence}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, the playerbots Package, and the Gateway binary"]
fn playerbots_gateway_restart_repairs_the_party_mirror_before_arrival_release() {
    let (topology, mut bot) = TransferTopology::stage("playerbots-transfer-gateway-mirror");
    topology.begin_transfer(&mut bot);
    let mut first = topology.gateway(Some("publish_shard_index"), "mirror-realm-settled");
    let first_log = first.wait_for_abort("publish_shard_index");
    let settled = topology.save(
        &bot,
        "mirror-realm-settled",
        serde_json::json!({ "gateway_log": first_log }),
    );
    assert_durable_phase(&settled, "publish_shard_index");

    topology.call(
        &topology.destination_db,
        "playerbots_transfer_gateway_mirror_fault",
        &["true", &bot.guid.to_string(), &bot.leader_guid.to_string()],
    );
    let mut interrupted = topology.gateway(None, "mirror-interrupted");
    let attempted = support::poll_until(support::POLL_TIMEOUT, || {
        let log = interrupted.log();
        log.contains("did not settle")
            && log.contains("group mirror conflicts with the accepted party rules")
    });
    let failed = topology.save(
        &bot,
        "mirror-interrupted",
        serde_json::json!({ "gateway_log": interrupted.log() }),
    );
    interrupted.stop();
    assert!(attempted, "mirror failure was not observed: {failed}");
    assert_eq!(
        rows(&failed, &["state", "destination", "arrival"]).len(),
        1,
        "failed mirror released the arrival: {failed}"
    );
    let intent = &rows(&failed, &["state", "source", "intent"])[0];
    assert_eq!(intent["arrival_ready"], "false", "{failed}");

    topology.call(
        &topology.destination_db,
        "playerbots_transfer_gateway_mirror_fault",
        &["false", &bot.guid.to_string(), &bot.leader_guid.to_string()],
    );
    let mut resumed = topology.gateway(None, "mirror-repaired");
    let completed = support::poll_until(support::POLL_TIMEOUT, || {
        topology
            .query(
                &topology.source_db,
                &format!(
                    "SELECT id FROM game_bot_transfer_intent WHERE bot_guid = {}",
                    bot.guid
                ),
            )
            .is_empty()
    });
    let repaired = topology.save(
        &bot,
        "mirror-repaired",
        serde_json::json!({ "gateway_log": resumed.log() }),
    );
    resumed.stop();
    assert!(completed, "repaired mirror did not complete: {repaired}");
    assert!(
        rows(&repaired, &["state", "destination", "arrival"]).is_empty(),
        "{repaired}"
    );
    assert_eq!(
        rows(&repaired, &["state", "destination", "members"]).len(),
        2,
        "{repaired}"
    );
}
