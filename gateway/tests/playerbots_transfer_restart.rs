#![cfg(target_os = "linux")]

//! Real Gateway process recovery for one session-less cross-Shard Transfer.

#[path = "support/playerbots_transfer_destination.rs"]
mod playerbots_transfer_destination;
#[path = "../../module/tests/support/mod.rs"]
mod support;

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::os::unix::process::ExitStatusExt;
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
            topology.call(database, "playerbots_transfer_gateway_identity_stage", &[]);
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
        self.capture_transfer(bot);
    }

    fn capture_transfer(&self, bot: &mut TransferredBot) {
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
            "databases": {
                "source": self.source_db,
                "destination": self.destination_db,
                "realm": self.realm_db,
            },
            "bot": {
                "guid": bot.guid,
                "leader_guid": bot.leader_guid,
                "intent_id": bot.intent_id,
                "generation": bot.generation,
                "objective_identity": bot.objective_identity,
            },
            "source": {
                "module_identity": self.query(&self.source_db, "SELECT * FROM pkg_playerbots_transfer_gateway_identity"),
                "character": self.query(&self.source_db, &format!("SELECT guid, map_id, pending_instance_id FROM game_character WHERE guid = {}", bot.guid)),
                "live": self.query(&self.source_db, &format!("SELECT guid, map_id, instance_id FROM game_world_entity WHERE guid = {}", bot.guid)),
                "escrow": self.query(&self.source_db, &format!("SELECT * FROM game_transfer_out WHERE character_guid = {}", bot.guid)),
                "intent": self.query(&self.source_db, &format!("SELECT * FROM game_bot_transfer_intent WHERE bot_guid = {}", bot.guid)),
                "runner": self.query(&self.source_db, &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {}", bot.guid)),
                "actions": self.query(&self.source_db, &format!("SELECT * FROM pkg_playerbots_action WHERE character_guid = {}", bot.guid)),
                "movement": self.query(&self.source_db, &format!("SELECT * FROM game_creature_spline WHERE guid = {}", bot.guid)),
                "pending_cast": self.query(&self.source_db, &format!("SELECT * FROM game_pending_cast WHERE caster_guid = {}", bot.guid)),
                "melee": self.query(&self.source_db, &format!("SELECT * FROM game_melee_attack WHERE attacker_guid = {}", bot.guid)),
                "instance": self.query(&self.source_db, &format!("SELECT * FROM game_instance WHERE instance_id = {DESTINATION_INSTANCE}")),
            },
            "destination": {
                "module_identity": self.query(&self.destination_db, "SELECT * FROM pkg_playerbots_transfer_gateway_identity"),
                "character": self.query(&self.destination_db, &format!("SELECT guid, map_id, pending_instance_id FROM game_character WHERE guid = {}", bot.guid)),
                "live": self.query(&self.destination_db, &format!("SELECT guid, map_id, instance_id FROM game_world_entity WHERE guid = {}", bot.guid)),
                "arrival": self.query(&self.destination_db, &format!("SELECT * FROM game_transfer_in WHERE character_guid = {}", bot.guid)),
                "runner": self.query(&self.destination_db, &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {}", bot.guid)),
                "actions": self.query(&self.destination_db, &format!("SELECT * FROM pkg_playerbots_action WHERE character_guid = {}", bot.guid)),
                "movement": self.query(&self.destination_db, &format!("SELECT * FROM game_creature_spline WHERE guid = {}", bot.guid)),
                "pending_cast": self.query(&self.destination_db, &format!("SELECT * FROM game_pending_cast WHERE caster_guid = {}", bot.guid)),
                "melee": self.query(&self.destination_db, &format!("SELECT * FROM game_melee_attack WHERE attacker_guid = {}", bot.guid)),
                "group": self.query(&self.destination_db, &format!("SELECT * FROM game_group WHERE group_id = {GROUP}")),
                "roster": self.query(&self.destination_db, &format!("SELECT * FROM game_group_roster_revision WHERE group_id = {GROUP}")),
                "members": self.query(&self.destination_db, &format!("SELECT * FROM game_group_member WHERE group_id = {GROUP}")),
                "partitions": self.query(&self.destination_db, &format!("SELECT * FROM game_group_member_partition WHERE group_id = {GROUP}")),
                "instance": self.query(&self.destination_db, &format!("SELECT * FROM game_instance WHERE instance_id = {DESTINATION_INSTANCE}")),
            },
            "realm": {
                "module_identity": self.query(&self.realm_db, "SELECT * FROM pkg_playerbots_transfer_gateway_identity"),
                "locator": self.query(&self.realm_db, &format!("SELECT * FROM game_character_shard WHERE character_guid = {}", bot.guid)),
                "group": self.query(&self.realm_db, &format!("SELECT * FROM game_group WHERE group_id = {GROUP}")),
                "roster": self.query(&self.realm_db, &format!("SELECT * FROM game_group_roster_revision WHERE group_id = {GROUP}")),
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

    fn wait_for_exit(&mut self, step: &str) -> serde_json::Value {
        let pid = self.child.as_ref().unwrap().id();
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
        let log = self.log();
        let exit = serde_json::json!({
            "pid": pid,
            "success": status.success(),
            "code": status.code(),
            "signal": status.signal(),
            "raw_status": status.into_raw(),
            "log": log,
        });
        let _ = self.child.take();
        exit
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

fn row<'a>(evidence: &'a serde_json::Value, path: &[&str]) -> &'a serde_json::Value {
    let rows = rows(evidence, path);
    assert_eq!(rows.len(), 1, "expected one row at {path:?}: {evidence}");
    &rows[0]
}

fn text_field<'a>(row: &'a serde_json::Value, field: &str) -> &'a str {
    row[field]
        .as_str()
        .unwrap_or_else(|| panic!("missing {field} in {row}"))
}

fn assert_u64_field(row: &serde_json::Value, field: &str, expected: u64) {
    assert_eq!(text_field(row, field), expected.to_string(), "{row}");
}

fn timestamp_micros(value: &str) -> i64 {
    let number = |range: std::ops::Range<usize>| value[range].parse::<i64>().unwrap();
    assert!(value.ends_with("+00:00"), "unexpected timestamp {value}");
    let year = number(0..4);
    let month = number(5..7);
    let day = number(8..10);
    let hour = number(11..13);
    let minute = number(14..16);
    let second = number(17..19);
    let micros = number(20..26);
    let shifted_year = year - i64::from(month <= 2);
    let era = if shifted_year >= 0 {
        shifted_year
    } else {
        shifted_year - 399
    } / 400;
    let year_of_era = shifted_year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days_since_epoch = era * 146_097 + day_of_era - 719_468;
    (((days_since_epoch * 24 + hour) * 60 + minute) * 60 + second) * 1_000_000 + micros
}

fn sorted_rows(evidence: &serde_json::Value, path: &[&str], key: &str) -> Vec<serde_json::Value> {
    let mut rows = rows(evidence, path).clone();
    rows.sort_by(|left, right| text_field(left, key).cmp(text_field(right, key)));
    rows
}

fn assert_same_fields(
    left: &serde_json::Value,
    right: &serde_json::Value,
    fields: &[&str],
    evidence: &serde_json::Value,
) {
    for field in fields {
        assert_eq!(left[*field], right[*field], "{field} differs: {evidence}");
    }
}

fn assert_abort(evidence: &serde_json::Value, step: &str) {
    let exit = &evidence["extra"]["gateway_exit"];
    assert!(
        exit["pid"].as_u64().is_some_and(|pid| pid > 0),
        "{evidence}"
    );
    assert_eq!(exit["success"], false, "{evidence}");
    assert_eq!(
        exit["signal"], 6,
        "Gateway did not exit by SIGABRT: {evidence}"
    );
    assert!(
        exit["raw_status"].is_i64(),
        "missing raw wait status: {evidence}"
    );
    let log = exit["log"].as_str().unwrap_or_default();
    assert!(
        log.contains(&format!("LYRACORE_TRANSFER_ABORT_AFTER={step}"))
            && log.contains("step committed"),
        "Gateway exited at another boundary after {step}: {evidence}"
    );
}

fn assert_crossing_identity(evidence: &serde_json::Value, position: usize) {
    let intent = row(evidence, &["state", "source", "intent"]);
    let bot = &evidence["state"]["bot"];
    let source_identity = row(evidence, &["state", "source", "module_identity"]);
    let destination_identity = row(evidence, &["state", "destination", "module_identity"]);
    let realm_identity = row(evidence, &["state", "realm", "module_identity"]);
    assert_eq!(
        intent["source_module_identity"], source_identity["identity"],
        "{evidence}"
    );
    assert_ne!(
        source_identity["identity"], destination_identity["identity"],
        "{evidence}"
    );
    assert_ne!(
        source_identity["identity"], realm_identity["identity"],
        "{evidence}"
    );
    assert_ne!(
        destination_identity["identity"], realm_identity["identity"],
        "{evidence}"
    );
    for database in ["source", "destination", "realm"] {
        assert!(
            evidence["state"]["databases"][database]
                .as_str()
                .is_some_and(|name| !name.is_empty()),
            "missing {database} database name: {evidence}"
        );
    }
    assert_u64_field(intent, "id", bot["intent_id"].as_u64().unwrap());
    assert_u64_field(intent, "bot_guid", bot["guid"].as_u64().unwrap());
    assert_u64_field(
        intent,
        "controller_generation",
        bot["generation"].as_u64().unwrap(),
    );
    if position == 0 {
        assert_ne!(intent["source_locator_revision"], "0", "{evidence}");
    }
    if (4..10).contains(&position) {
        let arrival = row(evidence, &["state", "destination", "arrival"]);
        assert_u64_field(arrival, "transfer_id", bot["guid"].as_u64().unwrap());
        assert_u64_field(arrival, "character_guid", bot["guid"].as_u64().unwrap());
        assert_same_fields(intent, arrival, &["source_locator_revision"], evidence);
        assert_eq!(
            intent["source_module_identity"], arrival["bot_intent_source"],
            "{evidence}"
        );
        assert_eq!(intent["id"], arrival["bot_intent_id"], "{evidence}");
        assert_eq!(
            intent["controller_generation"], arrival["bot_controller_generation"],
            "{evidence}"
        );
        assert_eq!(intent["source_map"], arrival["source_map_id"], "{evidence}");
        assert_eq!(
            intent["source_instance"], arrival["source_instance_id"],
            "{evidence}"
        );
        assert_eq!(
            arrival["bot_intent_created_micros"],
            timestamp_micros(text_field(intent, "created_at")).to_string(),
            "{evidence}"
        );
    }
}

fn assert_locator(evidence: &serde_json::Value, position: usize) {
    let intent = row(evidence, &["state", "source", "intent"]);
    let locator = row(evidence, &["state", "realm", "locator"]);
    let source_revision = text_field(intent, "source_locator_revision")
        .parse::<u64>()
        .unwrap();
    if position == 0 {
        assert_eq!(locator["transfer_pending"], "false", "{evidence}");
        assert_eq!(locator["map_id"], intent["source_map"], "{evidence}");
        assert_eq!(
            locator["instance_id"], intent["source_instance"],
            "{evidence}"
        );
        assert_eq!(
            locator["revision"], intent["source_locator_revision"],
            "{evidence}"
        );
    } else if position < 7 {
        assert_eq!(locator["transfer_pending"], "true", "{evidence}");
        assert_eq!(locator["map_id"], intent["source_map"], "{evidence}");
        assert_eq!(
            locator["instance_id"], intent["source_instance"],
            "{evidence}"
        );
        assert_eq!(
            locator["pending_destination_map"], intent["destination_map"],
            "{evidence}"
        );
        assert_eq!(
            locator["pending_destination_instance"], intent["destination_instance"],
            "{evidence}"
        );
        assert_u64_field(locator, "revision", source_revision);
    } else {
        assert_eq!(locator["transfer_pending"], "false", "{evidence}");
        assert_eq!(locator["map_id"], DESTINATION_MAP.to_string(), "{evidence}");
        assert_eq!(
            locator["instance_id"],
            DESTINATION_INSTANCE.to_string(),
            "{evidence}"
        );
        assert_u64_field(
            locator,
            "revision",
            source_revision
                .checked_add(1)
                .expect("fixture revision exhausted"),
        );
        assert_eq!(locator["pending_destination_map"], "0", "{evidence}");
        assert_eq!(locator["pending_destination_instance"], "0", "{evidence}");
    }
    if position > 0 {
        assert_eq!(
            locator["bot_source_identity"], intent["source_module_identity"],
            "{evidence}"
        );
        assert_eq!(
            locator["bot_transfer_intent_id"], intent["id"],
            "{evidence}"
        );
        assert_eq!(
            locator["bot_controller_generation"], intent["controller_generation"],
            "{evidence}"
        );
    }
}

fn assert_party_mirror(evidence: &serde_json::Value) {
    let bot = &evidence["state"]["bot"];
    let bot_guid = bot["guid"].as_u64().unwrap();
    let leader_guid = bot["leader_guid"].as_u64().unwrap();
    assert_same_fields(
        row(evidence, &["state", "destination", "group"]),
        row(evidence, &["state", "realm", "group"]),
        &[
            "group_id",
            "leader_guid",
            "loot_method",
            "loot_threshold",
            "rr_cursor",
            "master_looter_guid",
        ],
        evidence,
    );
    assert_same_fields(
        row(evidence, &["state", "destination", "roster"]),
        row(evidence, &["state", "realm", "roster"]),
        &["group_id", "revision", "active"],
        evidence,
    );
    assert_eq!(
        sorted_rows(
            evidence,
            &["state", "destination", "members"],
            "character_guid",
        ),
        sorted_rows(evidence, &["state", "realm", "members"], "character_guid"),
        "{evidence}"
    );
    assert_eq!(
        sorted_rows(
            evidence,
            &["state", "destination", "partitions"],
            "character_guid",
        ),
        sorted_rows(
            evidence,
            &["state", "realm", "partitions"],
            "character_guid",
        ),
        "{evidence}"
    );
    let realm_group = row(evidence, &["state", "realm", "group"]);
    assert_u64_field(realm_group, "group_id", GROUP);
    assert_u64_field(realm_group, "leader_guid", leader_guid);
    let members = sorted_rows(evidence, &["state", "realm", "members"], "character_guid");
    let mut member_guids: Vec<_> = members
        .iter()
        .map(|member| text_field(member, "character_guid").parse::<u64>().unwrap())
        .collect();
    member_guids.sort_unstable();
    let mut expected_guids = vec![bot_guid, leader_guid];
    expected_guids.sort_unstable();
    assert_eq!(member_guids, expected_guids, "{evidence}");
    let partitions = rows(evidence, &["state", "realm", "partitions"]);
    let mut partition_guids: Vec<_> = partitions
        .iter()
        .map(|partition| {
            text_field(partition, "character_guid")
                .parse::<u64>()
                .unwrap()
        })
        .collect();
    partition_guids.sort_unstable();
    assert_eq!(partition_guids, expected_guids, "{evidence}");
    for partition in partitions {
        let guid = text_field(partition, "character_guid")
            .parse::<u64>()
            .unwrap();
        assert!(expected_guids.contains(&guid), "{evidence}");
        assert_u64_field(partition, "map_id", u64::from(DESTINATION_MAP));
        assert_u64_field(partition, "instance_id", DESTINATION_INSTANCE);
    }
}

fn assert_gameplay_fences(evidence: &serde_json::Value, position: usize) {
    if position < 10 {
        assert!(
            rows(evidence, &["state", "destination", "live"]).is_empty(),
            "arrival became live before release: {evidence}"
        );
        for table in ["actions", "movement", "pending_cast", "melee"] {
            assert!(
                rows(evidence, &["state", "destination", table]).is_empty(),
                "destination gameplay started before release: {evidence}"
            );
        }
    }
    if position >= 2 {
        for table in ["runner", "actions", "movement", "pending_cast", "melee"] {
            assert!(
                rows(evidence, &["state", "source", table]).is_empty(),
                "source-local work survived Escrow: {evidence}"
            );
        }
    }
}

fn assert_durable_phase(evidence: &serde_json::Value, step: &str) {
    let position = ABORT_STEPS
        .iter()
        .position(|candidate| *candidate == step)
        .unwrap();
    let source_character = rows(evidence, &["state", "source", "character"]);
    let destination_character = rows(evidence, &["state", "destination", "character"]);
    assert!(
        source_character.len() + destination_character.len() >= 1,
        "no durable Character after {step}: {evidence}"
    );
    assert!(
        rows(evidence, &["state", "source", "live"]).is_empty()
            || rows(evidence, &["state", "destination", "live"]).is_empty(),
        "two live bodies after {step}: {evidence}"
    );
    assert_eq!(
        rows(evidence, &["state", "source", "intent"]).len(),
        1,
        "the crashed worker consumed its intent after {step}: {evidence}"
    );
    assert_crossing_identity(evidence, position);
    assert_locator(evidence, position);
    assert_gameplay_fences(evidence, position);

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
        assert_party_mirror(evidence);
    }
    if position >= 9 {
        assert_eq!(
            row(evidence, &["state", "source", "intent"])["arrival_ready"],
            "true",
            "{evidence}"
        );
    }
}

fn assert_postrelease_body(topology: &TransferTopology, bot: &TransferredBot, step: &str) {
    topology.call(
        &topology.destination_db,
        "playerbots_fixture_companion_due",
        &[&bot.guid.to_string()],
    );
    let observed = support::poll_until(support::POLL_TIMEOUT, || {
        topology
            .query(
                &topology.destination_db,
                &format!(
                    "SELECT guid FROM game_world_entity WHERE guid = {}",
                    bot.guid
                ),
            )
            .len()
            == 1
    });
    let evidence = topology.save(
        bot,
        &format!("{step}-postrelease-body"),
        serde_json::json!({
            "body_observed": observed,
            "normal_runner_due_requested": true,
        }),
    );
    assert!(observed, "destination body was not rebuilt: {evidence}");
    assert_eq!(
        rows(&evidence, &["state", "destination", "live"]).len(),
        1,
        "{evidence}"
    );
    for table in ["runner", "actions", "movement", "pending_cast", "melee"] {
        assert!(
            rows(&evidence, &["state", "source", table]).is_empty(),
            "source-local work returned after release: {evidence}"
        );
    }
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, the playerbots Package, and the Gateway binary"]
fn playerbots_gateway_process_restart_resumes_every_committed_transfer_phase() {
    let (topology, mut bot) = TransferTopology::stage("playerbots-transfer-gateway-restart");
    topology.begin_transfer(&mut bot);

    for step in ABORT_STEPS {
        let mut gateway = topology.gateway(Some(step), step);
        let gateway_exit = gateway.wait_for_exit(step);
        let evidence = topology.save(
            &bot,
            step,
            serde_json::json!({ "gateway_exit": gateway_exit }),
        );
        assert_abort(&evidence, step);
        assert_durable_phase(&evidence, step);
        if ABORT_STEPS
            .iter()
            .position(|candidate| *candidate == step)
            .unwrap()
            >= 10
        {
            assert_postrelease_body(&topology, &bot, step);
        }
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
    let first_exit = first.wait_for_exit("publish_shard_index");
    let settled = topology.save(
        &bot,
        "mirror-realm-settled",
        serde_json::json!({ "gateway_exit": first_exit }),
    );
    assert_abort(&settled, "publish_shard_index");
    assert_durable_phase(&settled, "publish_shard_index");

    topology.call(
        &topology.destination_db,
        "playerbots_transfer_gateway_mirror_fault",
        &["true", &bot.guid.to_string(), &bot.leader_guid.to_string()],
    );
    let faulted = topology.save(&bot, "mirror-fault-staged", serde_json::json!({}));
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
    for table in ["group", "roster", "members", "partitions"] {
        assert_eq!(
            rows(&faulted, &["state", "destination", table]),
            rows(&failed, &["state", "destination", table]),
            "failed mirror changed destination {table}: {failed}"
        );
    }
    assert_crossing_identity(&failed, 7);
    assert_locator(&failed, 7);
    assert_gameplay_fences(&failed, 8);
    assert_eq!(
        rows(&failed, &["state", "destination", "arrival"]).len(),
        1,
        "failed mirror released the arrival: {failed}"
    );
    let intent = &rows(&failed, &["state", "source", "intent"])[0];
    assert_eq!(intent["arrival_ready"], "false", "{failed}");
    let destination_group = row(&failed, &["state", "destination", "group"]);
    let realm_group = row(&failed, &["state", "realm", "group"]);
    assert_eq!(
        destination_group["rr_cursor"],
        u32::MAX.to_string(),
        "{failed}"
    );
    assert_eq!(realm_group["rr_cursor"], "0", "{failed}");
    assert_same_fields(
        destination_group,
        realm_group,
        &[
            "group_id",
            "leader_guid",
            "loot_method",
            "loot_threshold",
            "master_looter_guid",
        ],
        &failed,
    );

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
    assert_party_mirror(&repaired);
}

fn run_quest_destination_case(name: &str, mode: u8) {
    let (topology, mut bot) = TransferTopology::stage(name);
    let source = playerbots_transfer_destination::stage_retained_quest(&topology, &bot);
    topology.capture_transfer(&mut bot);
    let source_evidence = topology.save(
        &bot,
        "quest-source-transfer",
        serde_json::json!({ "quest_source": source }),
    );
    playerbots_transfer_destination::assert_retained_quest_stage(
        &source_evidence["extra"]["quest_source"],
    );

    let catalogue =
        playerbots_transfer_destination::stage_destination_catalogue(&topology, bot.guid, mode);
    let catalogue_evidence = topology.save(
        &bot,
        "quest-destination-catalogue",
        serde_json::json!({ "destination_catalogue": catalogue }),
    );
    playerbots_transfer_destination::assert_destination_catalogue(
        &catalogue_evidence["extra"]["destination_catalogue"],
    );

    let mut gateway = topology.gateway(None, "quest-transfer");
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
    let arrival = topology.save(
        &bot,
        "quest-destination-arrived",
        serde_json::json!({
            "completed": completed,
            "gateway_log": gateway.log(),
        }),
    );
    gateway.stop();
    assert!(completed, "Quest Transfer did not complete: {arrival}");
    assert_eq!(
        rows(&arrival, &["state", "destination", "character"]).len(),
        1,
        "{arrival}"
    );
    assert!(
        rows(&arrival, &["state", "source", "character"]).is_empty(),
        "{arrival}"
    );

    let result = if mode == 1 {
        playerbots_transfer_destination::case9_rebuilds_destination(&topology, &bot)
    } else {
        playerbots_transfer_destination::case10_records_incompatible_destination(&topology, &bot)
    };
    let evidence = topology.save(
        &bot,
        if mode == 1 {
            "quest-destination-rebuilt"
        } else {
            "quest-destination-replaced"
        },
        serde_json::json!({ "result": result }),
    );
    if mode == 1 {
        playerbots_transfer_destination::assert_case9_rebuilds_destination(
            &evidence["extra"]["result"],
        );
    } else {
        playerbots_transfer_destination::assert_case10_records_incompatible_destination(
            &evidence["extra"]["result"],
        );
    }
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, the playerbots Package, and the Gateway binary"]
fn playerbots_quest_transfer_rebuilds_its_destination_after_arrival() {
    run_quest_destination_case("playerbots-transfer-quest-rebuilt", 1);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, the playerbots Package, and the Gateway binary"]
fn playerbots_quest_transfer_records_an_incompatible_destination() {
    run_quest_destination_case("playerbots-transfer-quest-replaced", 2);
}
