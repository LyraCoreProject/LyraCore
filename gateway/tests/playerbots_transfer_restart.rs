#![cfg(target_os = "linux")]

//! Real Gateway process recovery for one session-less cross-Shard Transfer.

#[path = "support/playerbots_transfer_destination.rs"]
mod playerbots_transfer_destination;
#[path = "../../module/tests/support/mod.rs"]
mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const DESTINATION_MAP: u32 = 36;
const DESTINATION_INSTANCE: u64 = 5_098_078;
const GROUP: u64 = 5_098_000;
const ASSIST_PRIEST: (f32, f32, f32) = (-10.0, -385.475, 62.4561);
const ASSIST_LEADER: (f32, f32, f32) = (-14.5732, -410.475, 62.4561);
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
    pub(crate) priest_guid: u64,
    pub(crate) mage_guid: u64,
    pub(crate) intent_id: u64,
    pub(crate) generation: u64,
    pub(crate) objective_identity: u64,
}

impl TransferTopology {
    fn stage(name: &str) -> (Self, TransferredBot) {
        Self::stage_with_destination(name, true)
    }

    fn stage_assist(name: &str) -> (Self, TransferredBot) {
        Self::stage_with_destination(name, false)
    }

    fn stage_with_destination(
        name: &str,
        stage_generic_destination: bool,
    ) -> (Self, TransferredBot) {
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
            .parse::<u64>()
            .unwrap();
        let mage = bots
            .iter()
            .find(|bot| bot["class"] == "8" && bot["role"] == "2")
            .expect("Mage Damage missing")["character_guid"]
            .parse::<u64>()
            .unwrap();
        topology.call(
            &topology.source_db,
            "playerbots_fixture_roles_stage",
            &[
                &companion.to_string(),
                &priest.to_string(),
                &mage.to_string(),
                &leader.to_string(),
            ],
        );
        for guid in [companion, priest, mage] {
            topology.call(
                &topology.source_db,
                "playerbots_fixture_provision_steps",
                &[&guid.to_string(), "32"],
            );
        }
        if stage_generic_destination {
            topology.call(&topology.destination_db, "install_guid_range", &["1000000"]);
            topology.call(
                &topology.destination_db,
                "playerbots_spawn",
                &["4", "1200", "1200", "50"],
            );
            let destination_roles = topology.query(
                &topology.destination_db,
                "SELECT character_guid, class, role FROM pkg_playerbots_bot",
            );
            assert_eq!(
                destination_roles.len(),
                4,
                "destination role roster changed: {destination_roles:?}"
            );
            for (guid, class, role) in [
                (companion, 1, 0),
                (leader, 1, 0),
                (priest, 5, 1),
                (mage, 8, 2),
            ] {
                assert!(
                    destination_roles.iter().any(|row| {
                        parse_u64(row, "character_guid") == guid
                            && row["class"] == class.to_string()
                            && row["role"] == role.to_string()
                    }),
                    "destination role identity changed: {destination_roles:?}"
                );
            }
            for guid in [companion, priest, mage] {
                topology.call(
                    &topology.destination_db,
                    "debug_delete_character",
                    &[&guid.to_string()],
                );
            }
            topology.call(
                &topology.destination_db,
                "playerbots_transfer_gateway_destination_leader_stage",
                &[
                    &companion.to_string(),
                    &leader.to_string(),
                    &priest.to_string(),
                    &mage.to_string(),
                    "0",
                    "0",
                    &DESTINATION_MAP.to_string(),
                    &DESTINATION_INSTANCE.to_string(),
                ],
            );
        }
        topology.call(
            &topology.realm_db,
            "playerbots_transfer_gateway_realm_stage",
            &[
                &companion.to_string(),
                &leader.to_string(),
                &priest.to_string(),
                &mage.to_string(),
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
                priest_guid: priest,
                mage_guid: mage,
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
                "priest_guid": bot.priest_guid,
                "mage_guid": bot.mage_guid,
                "intent_id": bot.intent_id,
                "generation": bot.generation,
                "objective_identity": bot.objective_identity,
            },
            "source": {
                "module_identity": self.query(&self.source_db, "SELECT * FROM pkg_playerbots_transfer_gateway_identity"),
                "character": self.query(&self.source_db, &format!("SELECT guid, map_id, pending_instance_id FROM game_character WHERE guid = {}", bot.guid)),
                "leader_character": self.query(&self.source_db, &format!("SELECT guid, map_id, pending_instance_id FROM game_character WHERE guid = {}", bot.leader_guid)),
                "priest_character": self.query(&self.source_db, &format!("SELECT guid, map_id, pending_instance_id FROM game_character WHERE guid = {}", bot.priest_guid)),
                "mage_character": self.query(&self.source_db, &format!("SELECT guid, map_id, pending_instance_id FROM game_character WHERE guid = {}", bot.mage_guid)),
                "live": self.query(&self.source_db, &format!("SELECT guid, map_id, instance_id FROM game_world_entity WHERE guid = {}", bot.guid)),
                "escrow": self.query(&self.source_db, &format!("SELECT * FROM game_transfer_out WHERE character_guid = {}", bot.guid)),
                "intent": self.query(&self.source_db, &format!("SELECT * FROM game_bot_transfer_intent WHERE bot_guid = {}", bot.guid)),
                "runner": self.query(&self.source_db, &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {}", bot.guid)),
                "actions": self.query(&self.source_db, &format!("SELECT * FROM pkg_playerbots_action WHERE character_guid = {}", bot.guid)),
                "movement": self.query(&self.source_db, &format!("SELECT * FROM game_creature_spline WHERE guid = {}", bot.guid)),
                "pending_cast": self.query(&self.source_db, &format!("SELECT * FROM game_pending_cast WHERE caster_guid = {}", bot.guid)),
                "melee": self.query(&self.source_db, &format!("SELECT * FROM game_melee_attack WHERE attacker_guid = {}", bot.guid)),
                "instance": self.query(&self.source_db, &format!("SELECT * FROM game_instance WHERE instance_id = {DESTINATION_INSTANCE}")),
                "assist": self.query(&self.source_db, "SELECT * FROM pkg_playerbots_transfer_assist_source WHERE id = 0"),
                "order": self.query(&self.source_db, &format!("SELECT * FROM pkg_playerbots_companion_order WHERE character_guid = {}", bot.guid)),
                "group": self.query(&self.source_db, &format!("SELECT * FROM game_group WHERE group_id = {GROUP}")),
                "roster": self.query(&self.source_db, &format!("SELECT * FROM game_group_roster_revision WHERE group_id = {GROUP}")),
                "members": self.query(&self.source_db, &format!("SELECT * FROM game_group_member WHERE group_id = {GROUP}")),
                "partitions": self.query(&self.source_db, &format!("SELECT * FROM game_group_member_partition WHERE group_id = {GROUP}")),
                "leader_live": self.query(&self.source_db, &format!("SELECT guid, map_id, instance_id, x, y, z FROM game_world_entity WHERE guid = {}", bot.leader_guid)),
                "priest_live": self.query(&self.source_db, &format!("SELECT guid, map_id, instance_id, x, y, z FROM game_world_entity WHERE guid = {}", bot.priest_guid)),
                "leader_bot": self.query(&self.source_db, &format!("SELECT character_guid, next_think_micros FROM pkg_playerbots_bot WHERE character_guid = {}", bot.leader_guid)),
                "priest_bot": self.query(&self.source_db, &format!("SELECT character_guid, next_think_micros FROM pkg_playerbots_bot WHERE character_guid = {}", bot.priest_guid)),
            },
            "destination": {
                "module_identity": self.query(&self.destination_db, "SELECT * FROM pkg_playerbots_transfer_gateway_identity"),
                "character": self.query(&self.destination_db, &format!("SELECT guid, map_id, pending_instance_id FROM game_character WHERE guid = {}", bot.guid)),
                "leader_character": self.query(&self.destination_db, &format!("SELECT guid, map_id, pending_instance_id FROM game_character WHERE guid = {}", bot.leader_guid)),
                "priest_character": self.query(&self.destination_db, &format!("SELECT guid, map_id, pending_instance_id FROM game_character WHERE guid = {}", bot.priest_guid)),
                "mage_character": self.query(&self.destination_db, &format!("SELECT guid, map_id, pending_instance_id FROM game_character WHERE guid = {}", bot.mage_guid)),
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
                "order": self.query(&self.destination_db, &format!("SELECT * FROM pkg_playerbots_companion_order WHERE character_guid = {}", bot.guid)),
                "leader_live": self.query(&self.destination_db, &format!("SELECT guid, map_id, instance_id, x, y, z FROM game_world_entity WHERE guid = {}", bot.leader_guid)),
                "priest_live": self.query(&self.destination_db, &format!("SELECT guid, map_id, instance_id, x, y, z FROM game_world_entity WHERE guid = {}", bot.priest_guid)),
                "leader_bot": self.query(&self.destination_db, &format!("SELECT character_guid, next_think_micros FROM pkg_playerbots_bot WHERE character_guid = {}", bot.leader_guid)),
                "priest_bot": self.query(&self.destination_db, &format!("SELECT character_guid, next_think_micros FROM pkg_playerbots_bot WHERE character_guid = {}", bot.priest_guid)),
                "companion_bot": self.query(&self.destination_db, &format!("SELECT character_guid, next_think_micros FROM pkg_playerbots_bot WHERE character_guid = {}", bot.guid)),
                "movement_tick": self.query(&self.destination_db, "SELECT * FROM game_creature_move_schedule"),
            },
            "realm": {
                "module_identity": self.query(&self.realm_db, "SELECT * FROM pkg_playerbots_transfer_gateway_identity"),
                "locator": self.query(&self.realm_db, &format!("SELECT * FROM game_character_shard WHERE character_guid = {}", bot.guid)),
                "leader_locator": self.query(&self.realm_db, &format!("SELECT * FROM game_character_shard WHERE character_guid = {}", bot.leader_guid)),
                "priest_locator": self.query(&self.realm_db, &format!("SELECT * FROM game_character_shard WHERE character_guid = {}", bot.priest_guid)),
                "mage_locator": self.query(&self.realm_db, &format!("SELECT * FROM game_character_shard WHERE character_guid = {}", bot.mage_guid)),
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

fn issue_assist_order(topology: &TransferTopology, bot: &TransferredBot) {
    topology.call(
        &topology.source_db,
        "provision_account",
        &[r#""PB010ASSIST""#, "[]", "[]"],
    );
    let account = exactly_one(
        &topology.query(
            &topology.source_db,
            "SELECT id FROM game_account WHERE username = 'PB010ASSIST'",
        ),
        "Assist leader account",
    )["id"]
        .clone();
    topology.call(
        &topology.source_db,
        "playerbots_fixture_orders_account",
        &[&bot.leader_guid.to_string(), &account],
    );
    topology.call(
        &topology.source_db,
        "claim_account",
        &[&account, &bot.leader_guid.to_string(), "5098012"],
    );
    let generation = exactly_one(
        &topology.query(
            &topology.source_db,
            &format!("SELECT generation FROM game_account_claim WHERE account_id = {account}"),
        ),
        "Assist leader Account Claim",
    )["generation"]
        .clone();
    let actor = serde_json::json!({
        "guid": bot.leader_guid,
        "ownership": {"some": {
            "account_id": account.parse::<u64>().unwrap(),
            "generation": generation.parse::<u64>().unwrap(),
            "request_nonce": 5_098_012,
        }},
    })
    .to_string();
    let payload = format!("assist|{}|{}", bot.guid, bot.priest_guid);
    topology.call(
        &topology.source_db,
        "gw_client_command",
        &[
            &actor,
            r#""playerbots.order""#,
            &serde_json::to_string(&payload).unwrap(),
        ],
    );
    let intents = topology.query(
        &topology.source_db,
        "SELECT id FROM game_party_command_intent WHERE pending = true",
    );
    let intent = parse_u64(exactly_one(&intents, "Assist command intent"), "id");
    topology.call(
        &topology.source_db,
        "playerbots_fixture_orders_drive",
        &[
            &intent.to_string(),
            &intent
                .checked_add(5_098_100)
                .expect("Assist fixture claim token exhausted")
                .to_string(),
            &bot.guid.to_string(),
            "true",
        ],
    );
}

fn stage_assist_destination(
    topology: &TransferTopology,
    bot: &TransferredBot,
) -> serde_json::Value {
    topology.call(&topology.destination_db, "install_guid_range", &["1000000"]);
    topology.call(
        &topology.destination_db,
        "playerbots_spawn",
        &["4", "1200", "1200", "50"],
    );
    let mut destination = topology.query(
        &topology.destination_db,
        "SELECT character_guid, class, role FROM pkg_playerbots_bot",
    );
    destination.sort_by_key(|row| parse_u64(row, "character_guid"));
    let matching_role_identities = destination.len() == 4
        && [
            (bot.guid, 1, 0),
            (bot.leader_guid, 1, 0),
            (bot.priest_guid, 5, 1),
            (bot.mage_guid, 8, 2),
        ]
        .into_iter()
        .all(|(guid, class, role)| {
            destination.iter().any(|row| {
                parse_u64(row, "character_guid") == guid
                    && row["class"] == class.to_string()
                    && row["role"] == role.to_string()
            })
        });
    topology.call(
        &topology.destination_db,
        "debug_delete_character",
        &[&bot.guid.to_string()],
    );
    topology.call(
        &topology.destination_db,
        "debug_delete_character",
        &[&bot.mage_guid.to_string()],
    );
    topology.call(
        &topology.destination_db,
        "playerbots_transfer_assist_destination_stage",
        &[
            &bot.guid.to_string(),
            &bot.leader_guid.to_string(),
            &bot.priest_guid.to_string(),
            &bot.mage_guid.to_string(),
        ],
    );
    let points = [(-14.5732, -385.475, 62.4561), ASSIST_PRIEST, ASSIST_LEADER];
    topology.call(&topology.destination_db, "debug_set_nav_enabled", &["true"]);
    let mut cells = BTreeSet::new();
    for (x, y, z) in points {
        let cell_x = lyracore_shared::terrain::cell_index(x).unwrap();
        let cell_y = lyracore_shared::terrain::cell_index(y).unwrap();
        if cells.insert((cell_x, cell_y)) {
            let chunk = format!("{DESTINATION_MAP},{cell_x},{cell_y},{z},,");
            topology.call(
                &topology.destination_db,
                "import_nav_chunks_append",
                &[&chunk],
            );
        }
    }
    serde_json::json!({
        "expected_roles": [
            {"guid": bot.guid, "class": 1, "role": 0, "kind": "companion"},
            {"guid": bot.leader_guid, "class": 1, "role": 0, "kind": "human-leader"},
            {"guid": bot.priest_guid, "class": 5, "role": 1, "kind": "selected-priest"},
            {"guid": bot.mage_guid, "class": 8, "role": 2, "kind": "unused-mage"},
        ],
        "destination_roles": destination,
        "matching_role_identities": matching_role_identities,
        "navigation_cells": cells
            .into_iter()
            .map(|(cell_x, cell_y)| serde_json::json!({
                "map_id": DESTINATION_MAP,
                "cell_x": cell_x,
                "cell_y": cell_y,
            }))
            .collect::<Vec<_>>(),
    })
}

fn sorted_rows(evidence: &serde_json::Value, path: &[&str], key: &str) -> Vec<serde_json::Value> {
    let mut rows = rows(evidence, path).clone();
    rows.sort_by(|left, right| text_field(left, key).cmp(text_field(right, key)));
    rows
}

fn member_keys(members: &[serde_json::Value]) -> Vec<(String, String)> {
    let mut keys: Vec<_> = members
        .iter()
        .map(|member| {
            (
                text_field(member, "group_id").to_owned(),
                text_field(member, "character_guid").to_owned(),
            )
        })
        .collect();
    keys.sort();
    keys
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
    let realm_members = rows(evidence, &["state", "realm", "members"]);
    for shard in ["source", "destination"] {
        assert_same_fields(
            row(evidence, &["state", shard, "group"]),
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
            row(evidence, &["state", shard, "roster"]),
            row(evidence, &["state", "realm", "roster"]),
            &["group_id", "revision", "active"],
            evidence,
        );
        let shard_members = rows(evidence, &["state", shard, "members"]);
        assert_eq!(
            member_keys(shard_members),
            member_keys(realm_members),
            "{evidence}"
        );
        assert_eq!(
            sorted_rows(evidence, &["state", shard, "partitions"], "character_guid"),
            sorted_rows(
                evidence,
                &["state", "realm", "partitions"],
                "character_guid",
            ),
            "{evidence}"
        );
    }
    let realm_group = row(evidence, &["state", "realm", "group"]);
    assert_u64_field(realm_group, "group_id", GROUP);
    assert_u64_field(realm_group, "leader_guid", leader_guid);
    let members = sorted_rows(evidence, &["state", "realm", "members"], "character_guid");
    let mut member_guids: Vec<_> = members
        .iter()
        .map(|member| text_field(member, "character_guid").parse::<u64>().unwrap())
        .collect();
    member_guids.sort_unstable();
    let mut expected_guids = vec![
        bot_guid,
        leader_guid,
        bot["priest_guid"].as_u64().unwrap(),
        bot["mage_guid"].as_u64().unwrap(),
    ];
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
        let member = members
            .iter()
            .find(|member| text_field(member, "character_guid").parse::<u64>().unwrap() == guid)
            .unwrap();
        assert_eq!(partition["membership_revision"], member["id"], "{evidence}");
        if guid == bot_guid || guid == leader_guid {
            assert_u64_field(partition, "map_id", u64::from(DESTINATION_MAP));
            assert_u64_field(partition, "instance_id", DESTINATION_INSTANCE);
        } else {
            assert_u64_field(partition, "map_id", 0);
            assert_u64_field(partition, "instance_id", 0);
        }
    }
    for (guid, locator_name, character_name) in [
        (bot_guid, "locator", "character"),
        (leader_guid, "leader_locator", "leader_character"),
        (
            bot["priest_guid"].as_u64().unwrap(),
            "priest_locator",
            "priest_character",
        ),
        (
            bot["mage_guid"].as_u64().unwrap(),
            "mage_locator",
            "mage_character",
        ),
    ] {
        let locator = row(evidence, &["state", "realm", locator_name]);
        let holder = if text_field(locator, "map_id").parse::<u32>().unwrap() == DESTINATION_MAP {
            "destination"
        } else {
            "source"
        };
        let character = row(evidence, &["state", holder, character_name]);
        assert_u64_field(character, "guid", guid);
        assert_eq!(character["map_id"], locator["map_id"], "{evidence}");
        assert_eq!(
            character["pending_instance_id"], locator["instance_id"],
            "{evidence}"
        );
    }
}

fn assert_normalized_source_runner(evidence: &serde_json::Value) {
    let runner = row(evidence, &["state", "source", "runner"]);
    for (field, expected) in [
        ("foreground", "(none = ())"),
        ("chosen", "(none = ())"),
        ("candidate_order", ""),
        ("movement_progress", "(none = ())"),
        ("combat_progress", "(none = ())"),
        ("cast_progress", "(none = ())"),
        ("quest_progress", ""),
        ("last_target_health", "(none = ())"),
        ("defense_target", "(none = ())"),
        ("companion_heal_target_guid", "(none = ())"),
        ("companion_fight_target_guid", "(none = ())"),
        ("companion_buff_target_guid", "(none = ())"),
        ("deferred_destinations", ""),
        ("recovery", "(none = ())"),
        ("retry_candidate", "(none = ())"),
        ("transitions", "0"),
        ("route_expansions", "0"),
        ("route_budget", "0"),
    ] {
        assert_eq!(
            runner[field], expected,
            "{field} survived Escrow: {evidence}"
        );
    }
    let intent = row(evidence, &["state", "source", "intent"]);
    assert_eq!(
        runner["generation"], intent["controller_generation"],
        "{evidence}"
    );
    let checkpoint = text_field(runner, "transfer_checkpoint");
    for field in [
        format!("intent_id = {},", text_field(intent, "id")),
        format!(
            "controller_generation = {},",
            text_field(intent, "controller_generation")
        ),
        format!("source_map = {},", text_field(intent, "source_map")),
        format!(
            "source_instance = {},",
            text_field(intent, "source_instance")
        ),
        format!(
            "destination_map = {},",
            text_field(intent, "destination_map")
        ),
        format!(
            "destination_instance = {},",
            text_field(intent, "destination_instance")
        ),
    ] {
        assert!(checkpoint.contains(&field), "{field} missing: {evidence}");
    }
}

fn assert_source_transfer_receipt(evidence: &serde_json::Value, position: usize) {
    let actions = rows(evidence, &["state", "source", "actions"]);
    assert_eq!(
        actions.len(),
        usize::from(position < 6),
        "source Transfer receipt phase differs: {evidence}"
    );
    if position >= 6 {
        return;
    }
    let action = &actions[0];
    let intent = row(evidence, &["state", "source", "intent"]);
    let bot = &evidence["state"]["bot"];
    assert_u64_field(action, "character_guid", bot["guid"].as_u64().unwrap());
    assert_eq!(action["kind"], "(transfer = ())", "{evidence}");
    assert_eq!(
        action["outcome"],
        format!("(transferAccepted = {})", text_field(intent, "id")),
        "{evidence}"
    );
    assert_u64_field(action, "target_guid", DESTINATION_INSTANCE);
    assert_u64_field(action, "quest_entry", u64::from(DESTINATION_MAP));
    assert_u64_field(action, "spell_id", 78);
    assert_eq!(
        action["started_micros"], action["observed_micros"],
        "{evidence}"
    );
}

fn assert_gameplay_fences(evidence: &serde_json::Value, position: usize) {
    assert_source_transfer_receipt(evidence, position);
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
        for table in ["movement", "pending_cast", "melee"] {
            assert!(
                rows(evidence, &["state", "source", table]).is_empty(),
                "source-local work survived Escrow: {evidence}"
            );
        }
        if position < 6 {
            assert_normalized_source_runner(evidence);
        } else {
            assert!(
                rows(evidence, &["state", "source", "runner"]).is_empty(),
                "source Runner survived finish_transfer: {evidence}"
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
    assert_eq!(
        source_character.len(),
        usize::from(position < 6),
        "source Character phase differs after {step}: {evidence}"
    );
    assert_eq!(
        destination_character.len(),
        usize::from(position >= 4),
        "destination Character phase differs after {step}: {evidence}"
    );
    assert_eq!(
        rows(evidence, &["state", "source", "live"]).len(),
        0,
        "source live body returned after {step}: {evidence}"
    );
    assert_eq!(
        rows(evidence, &["state", "source", "intent"]).len(),
        1,
        "the crashed worker consumed its intent after {step}: {evidence}"
    );
    assert_crossing_identity(evidence, position);
    assert_locator(evidence, position);
    assert_gameplay_fences(evidence, position);

    assert_eq!(
        rows(evidence, &["state", "source", "escrow"]).len(),
        usize::from((2..6).contains(&position)),
        "source Escrow phase differs after {step}: {evidence}"
    );
    assert_eq!(
        rows(evidence, &["state", "destination", "instance"]).len(),
        usize::from(position >= 3),
        "destination instance phase differs after {step}: {evidence}"
    );
    assert_eq!(
        rows(evidence, &["state", "destination", "arrival"]).len(),
        usize::from((4..10).contains(&position)),
        "destination arrival phase differs after {step}: {evidence}"
    );
    if position >= 8 {
        assert_party_mirror(evidence);
    }
    assert_eq!(
        row(evidence, &["state", "source", "intent"])["arrival_ready"],
        if position >= 9 { "true" } else { "false" },
        "arrival-ready phase differs after {step}: {evidence}"
    );
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
        &[
            "true",
            &bot.guid.to_string(),
            &bot.leader_guid.to_string(),
            &bot.priest_guid.to_string(),
            &bot.mage_guid.to_string(),
        ],
    );
    let faulted = topology.save(&bot, "mirror-fault-staged", serde_json::json!({}));
    assert_same_fields(
        row(&faulted, &["state", "source", "group"]),
        row(&faulted, &["state", "realm", "group"]),
        &[
            "group_id",
            "leader_guid",
            "loot_method",
            "loot_threshold",
            "rr_cursor",
            "master_looter_guid",
        ],
        &faulted,
    );
    assert_same_fields(
        row(&faulted, &["state", "source", "roster"]),
        row(&faulted, &["state", "realm", "roster"]),
        &["group_id", "revision", "active"],
        &faulted,
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
    assert_eq!(destination_group["loot_method"], "3", "{failed}");
    assert_eq!(realm_group["loot_method"], "0", "{failed}");
    assert_same_fields(
        destination_group,
        realm_group,
        &[
            "group_id",
            "leader_guid",
            "loot_threshold",
            "rr_cursor",
            "master_looter_guid",
        ],
        &failed,
    );

    topology.call(
        &topology.destination_db,
        "playerbots_transfer_gateway_mirror_fault",
        &[
            "false",
            &bot.guid.to_string(),
            &bot.leader_guid.to_string(),
            &bot.priest_guid.to_string(),
            &bot.mage_guid.to_string(),
        ],
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
        4,
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

fn assert_assist_source_ready(evidence: &serde_json::Value) {
    let bot = &evidence["state"]["bot"];
    let guid = bot["guid"].as_u64().unwrap();
    let leader = bot["leader_guid"].as_u64().unwrap();
    let priest = bot["priest_guid"].as_u64().unwrap();
    let assist = row(evidence, &["state", "source", "assist"]);
    assert_u64_field(assist, "bot_guid", guid);
    assert_u64_field(assist, "leader_guid", leader);
    assert_u64_field(assist, "selected_priest_guid", priest);
    assert_eq!(assist["destination_map"], DESTINATION_MAP.to_string());
    assert_eq!(
        assist["destination_instance"],
        DESTINATION_INSTANCE.to_string()
    );
    let order = row(evidence, &["state", "source", "order"]);
    assert_eq!(order["active"], "true", "{evidence}");
    assert_eq!(order["issuer_guid"], leader.to_string(), "{evidence}");
    assert_eq!(
        order["order"],
        format!("(assist = (member_guid = {priest}))"),
        "{evidence}"
    );
    let intent = row(evidence, &["state", "source", "intent"]);
    assert_eq!(intent["destination_map"], DESTINATION_MAP.to_string());
    assert_eq!(
        intent["destination_instance"],
        DESTINATION_INSTANCE.to_string()
    );
    let transfer_actions: Vec<_> = rows(evidence, &["state", "source", "actions"])
        .iter()
        .filter(|action| action["kind"] == "(transfer = ())")
        .collect();
    assert_eq!(
        transfer_actions.len(),
        1,
        "expected one ordinary source Transfer action: {evidence}"
    );
    assert!(
        text_field(transfer_actions[0], "outcome")
            .contains(&format!("transferAccepted = {}", text_field(intent, "id"))),
        "source Transfer action does not own the exact intent: {evidence}"
    );
    let runner = row(evidence, &["state", "source", "runner"]);
    let checkpoint = text_field(runner, "transfer_checkpoint");
    assert!(
        checkpoint.contains("(companion =")
            && checkpoint.contains(&format!("member_guid = {priest}")),
        "{evidence}"
    );
    assert_assist_source_bodies_absent(evidence);
    assert!(
        rows(evidence, &["state", "source", "leader_bot"]).is_empty(),
        "source leader is still a bot: {evidence}"
    );
    assert_eq!(
        row(evidence, &["state", "source", "priest_bot"])["next_think_micros"],
        i64::MAX.to_string(),
        "source Priest could rebuild its removed body: {evidence}"
    );
    assert!(
        rows(evidence, &["state", "destination", "leader_bot"]).is_empty(),
        "destination leader is represented as a bot: {evidence}"
    );
    assert_eq!(
        row(evidence, &["state", "destination", "priest_bot"])["next_think_micros"],
        i64::MAX.to_string(),
        "destination Priest is not parked: {evidence}"
    );
    let destination_leader = row(evidence, &["state", "destination", "leader_live"]);
    let destination_priest = row(evidence, &["state", "destination", "priest_live"]);
    for (body, guid, position) in [
        (destination_leader, leader, ASSIST_LEADER),
        (destination_priest, priest, ASSIST_PRIEST),
    ] {
        assert_u64_field(body, "guid", guid);
        assert_u64_field(body, "map_id", u64::from(DESTINATION_MAP));
        assert_u64_field(body, "instance_id", DESTINATION_INSTANCE);
        for (field, expected) in [("x", position.0), ("y", position.1), ("z", position.2)] {
            let actual = text_field(body, field).parse::<f32>().unwrap();
            assert!((actual - expected).abs() < 0.01, "{field}: {evidence}");
        }
    }
    for member in [leader, priest] {
        let realm_partition = rows(evidence, &["state", "realm", "partitions"])
            .iter()
            .find(|row| text_field(row, "character_guid").parse::<u64>().unwrap() == member)
            .unwrap_or_else(|| panic!("Realm partition {member} absent: {evidence}"));
        assert_eq!(realm_partition["map_id"], DESTINATION_MAP.to_string());
        assert_eq!(
            realm_partition["instance_id"],
            DESTINATION_INSTANCE.to_string()
        );
        assert_eq!(realm_partition["locator_revision"], "2");
        assert_eq!(realm_partition["state"], "(known = ())");
    }
}

fn assert_assist_source_bodies_absent(evidence: &serde_json::Value) {
    assert!(
        rows(evidence, &["state", "source", "leader_live"]).is_empty()
            && rows(evidence, &["state", "source", "priest_live"]).is_empty(),
        "source retained or rebuilt a remote party body: {evidence}"
    );
}

fn assert_assist_party_mirror(evidence: &serde_json::Value) {
    for table in ["group", "roster"] {
        assert_eq!(
            rows(evidence, &["state", "destination", table]),
            rows(evidence, &["state", "realm", table]),
            "Assist destination {table} differs from Realm: {evidence}"
        );
    }
    let destination_members = rows(evidence, &["state", "destination", "members"]);
    let realm_members = rows(evidence, &["state", "realm", "members"]);
    assert_eq!(
        member_keys(destination_members),
        member_keys(realm_members),
        "{evidence}"
    );
    let bot = &evidence["state"]["bot"];
    let expected = [
        (
            bot["guid"].as_u64().unwrap(),
            "locator",
            "character",
            DESTINATION_MAP,
            DESTINATION_INSTANCE,
        ),
        (
            bot["leader_guid"].as_u64().unwrap(),
            "leader_locator",
            "leader_character",
            DESTINATION_MAP,
            DESTINATION_INSTANCE,
        ),
        (
            bot["priest_guid"].as_u64().unwrap(),
            "priest_locator",
            "priest_character",
            DESTINATION_MAP,
            DESTINATION_INSTANCE,
        ),
        (
            bot["mage_guid"].as_u64().unwrap(),
            "mage_locator",
            "mage_character",
            0,
            0,
        ),
    ];
    let partitions = rows(evidence, &["state", "destination", "partitions"]);
    assert_eq!(partitions.len(), expected.len(), "{evidence}");
    for (guid, locator_name, character_name, map_id, instance_id) in expected {
        let locator = row(evidence, &["state", "realm", locator_name]);
        let partition = partitions
            .iter()
            .find(|partition| {
                text_field(partition, "character_guid")
                    .parse::<u64>()
                    .unwrap()
                    == guid
            })
            .unwrap_or_else(|| panic!("destination partition {guid} absent: {evidence}"));
        assert_eq!(partition["group_id"], GROUP.to_string(), "{evidence}");
        let authority_member = realm_members
            .iter()
            .find(|member| text_field(member, "character_guid").parse::<u64>().unwrap() == guid)
            .unwrap();
        assert_eq!(
            partition["membership_revision"], authority_member["id"],
            "{evidence}"
        );
        assert_eq!(partition["member_active"], "true", "{evidence}");
        assert_eq!(partition["state"], "(known = ())", "{evidence}");
        assert_eq!(locator["map_id"], map_id.to_string(), "{evidence}");
        assert_eq!(
            locator["instance_id"],
            instance_id.to_string(),
            "{evidence}"
        );
        assert_eq!(partition["map_id"], locator["map_id"], "{evidence}");
        assert_eq!(
            partition["instance_id"], locator["instance_id"],
            "{evidence}"
        );
        assert_eq!(
            partition["locator_revision"], locator["revision"],
            "{evidence}"
        );
        let holder = if map_id == DESTINATION_MAP {
            "destination"
        } else {
            "source"
        };
        let character = row(evidence, &["state", holder, character_name]);
        assert_u64_field(character, "guid", guid);
        assert_eq!(character["map_id"], locator["map_id"], "{evidence}");
        assert_eq!(
            character["pending_instance_id"], locator["instance_id"],
            "{evidence}"
        );
    }
}

fn assist_follow_observations(
    topology: &TransferTopology,
    bot: &TransferredBot,
) -> serde_json::Value {
    let deadline = Instant::now() + support::POLL_TIMEOUT;
    let mut samples = Vec::new();
    loop {
        // The declared Core tick also drives Package decisions. Run and park one real Runner
        // pass while that tick is delayed, then let Core move the captured leg independently.
        topology.call(
            &topology.destination_db,
            "playerbots_fixture_runner_pass_once",
            &[&bot.guid.to_string()],
        );
        let runner = topology.query(
            &topology.destination_db,
            &format!(
                "SELECT foreground FROM pkg_playerbots_runner WHERE character_guid = {}",
                bot.guid
            ),
        );
        let movement = topology.query(
            &topology.destination_db,
            &format!(
                "SELECT * FROM game_creature_spline WHERE guid = {}",
                bot.guid
            ),
        );
        let selected = runner.first().is_some_and(|row| {
            row["foreground"].contains(&format!("action = (move = (entity = {}))", bot.priest_guid))
                && row["foreground"].contains("reason = (follow = ())")
        }) && movement.len() == 1;
        if selected {
            return serde_json::json!({
                "selected_priest": true,
                "decision_driver": "controlled_runner_pass",
                "passes": samples.len() + 1,
                "samples": samples,
                "selected": {
                    "runner": runner,
                    "movement": movement,
                },
                "parked": {
                    "runner": topology.query(
                        &topology.destination_db,
                        &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {}", bot.guid),
                    ),
                    "movement": topology.query(
                        &topology.destination_db,
                        &format!("SELECT * FROM game_creature_spline WHERE guid = {}", bot.guid),
                    ),
                    "bot": topology.query(
                        &topology.destination_db,
                        &format!("SELECT character_guid, next_think_micros FROM pkg_playerbots_bot WHERE character_guid = {}", bot.guid),
                    ),
                    "movement_tick": topology.query(
                        &topology.destination_db,
                        "SELECT * FROM game_creature_move_schedule",
                    ),
                },
            });
        }
        samples.push(serde_json::json!({
            "runner": runner,
            "movement": movement,
            "body": topology.query(
                &topology.destination_db,
                &format!("SELECT guid, x, y, z FROM game_world_entity WHERE guid = {}", bot.guid),
            ),
            "movement_tick": topology.query(
                &topology.destination_db,
                "SELECT * FROM game_creature_move_schedule",
            ),
            "selected_priest": selected,
        }));
        if Instant::now() >= deadline {
            return serde_json::json!({
                "selected_priest": false,
                "decision_driver": "controlled_runner_pass",
                "passes": samples.len(),
                "samples": samples,
            });
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn assist_progress_observations(
    topology: &TransferTopology,
    bot: &TransferredBot,
    start: (f64, f64, f64),
    priest: (f64, f64, f64),
    leader: (f64, f64, f64),
) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(75);
    let mut samples = Vec::new();
    loop {
        let body = topology.query(
            &topology.destination_db,
            &format!(
                "SELECT guid, x, y, z FROM game_world_entity WHERE guid = {}",
                bot.guid
            ),
        );
        let position = body
            .first()
            .map(|row| {
                (
                    row["x"].parse::<f64>().unwrap(),
                    row["y"].parse::<f64>().unwrap(),
                    row["z"].parse::<f64>().unwrap(),
                )
            })
            .unwrap_or(start);
        let progressed = distance(position, priest) + 0.1 < distance(start, priest)
            && distance(position, leader) > distance(start, leader);
        samples.push(serde_json::json!({
            "body": body,
            "movement": topology.query(
                &topology.destination_db,
                &format!("SELECT * FROM game_creature_spline WHERE guid = {}", bot.guid),
            ),
            "movement_tick": topology.query(
                &topology.destination_db,
                "SELECT * FROM game_creature_move_schedule",
            ),
            "progressed_toward_priest": progressed,
        }));
        if progressed || Instant::now() >= deadline {
            return serde_json::json!({
                "progressed_toward_priest": progressed,
                "samples": samples,
            });
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn point(row: &serde_json::Value, fields: [&str; 3]) -> (f64, f64, f64) {
    let value = |field| text_field(row, field).parse::<f64>().unwrap();
    (value(fields[0]), value(fields[1]), value(fields[2]))
}

fn distance(left: (f64, f64, f64), right: (f64, f64, f64)) -> f64 {
    ((left.0 - right.0).powi(2) + (left.1 - right.1).powi(2) + (left.2 - right.2).powi(2)).sqrt()
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, the playerbots Package, and the Gateway binary"]
fn playerbots_assist_keeps_its_selected_member_after_arrival() {
    let (topology, mut bot) = TransferTopology::stage_assist("playerbots-transfer-assist-arrival");
    topology.call(
        &topology.source_db,
        "playerbots_transfer_fixture_entry_route_stage",
        &[&bot.guid.to_string(), &bot.leader_guid.to_string()],
    );
    issue_assist_order(&topology, &bot);
    topology.save(&bot, "assist-order-admitted", serde_json::json!({}));
    topology.call(
        &topology.source_db,
        "playerbots_transfer_assist_source_stage",
        &[
            &bot.guid.to_string(),
            &bot.leader_guid.to_string(),
            &bot.priest_guid.to_string(),
        ],
    );
    topology.call(
        &topology.realm_db,
        "playerbots_transfer_gateway_assist_realm_stage",
        &[
            &bot.guid.to_string(),
            &bot.leader_guid.to_string(),
            &bot.priest_guid.to_string(),
            &bot.mage_guid.to_string(),
            "0",
            "0",
            &DESTINATION_MAP.to_string(),
            &DESTINATION_INSTANCE.to_string(),
        ],
    );
    let destination_stage = stage_assist_destination(&topology, &bot);
    topology.call(
        &topology.source_db,
        "playerbots_fixture_runner_pass_once",
        &[&bot.guid.to_string()],
    );
    topology.capture_transfer(&mut bot);
    let ready = topology.save(
        &bot,
        "assist-source-ready",
        serde_json::json!({ "destination_stage": destination_stage }),
    );
    assert_eq!(
        ready["extra"]["destination_stage"]["matching_role_identities"], true,
        "{ready}"
    );
    assert_assist_source_ready(&ready);

    let mut gateway = topology.gateway(None, "assist-crossing");
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
    let arrived = topology.save(
        &bot,
        "assist-arrived",
        serde_json::json!({
            "completed": completed,
            "gateway_log": gateway.log(),
        }),
    );
    gateway.stop();
    assert!(completed, "Assist Transfer did not complete: {arrived}");
    assert_assist_source_bodies_absent(&arrived);
    assert_assist_party_mirror(&arrived);
    assert!(
        rows(&arrived, &["state", "source", "character"]).is_empty(),
        "{arrived}"
    );
    let destination_order = row(&arrived, &["state", "destination", "order"]);
    assert_eq!(
        destination_order["order"],
        format!("(assist = (member_guid = {}))", bot.priest_guid),
        "{arrived}"
    );
    let observations = assist_follow_observations(&topology, &bot);
    let queued = topology.save(
        &bot,
        "assist-selected-priest",
        serde_json::json!({ "observations": observations }),
    );
    assert_eq!(
        queued["extra"]["observations"]["selected_priest"], true,
        "{queued}"
    );
    assert_eq!(
        ready["state"]["destination"]["movement_tick"],
        queued["state"]["destination"]["movement_tick"],
        "the declared Core movement tick fired before the selected leg was captured: {queued}"
    );
    let runner = row(&queued, &["extra", "observations", "selected", "runner"]);
    let foreground = text_field(runner, "foreground");
    assert!(
        foreground.contains(&format!("action = (move = (entity = {}))", bot.priest_guid))
            && !foreground.contains(&format!("action = (move = (entity = {}))", bot.leader_guid)),
        "{queued}"
    );
    let selected_movement = rows(&queued, &["extra", "observations", "selected", "movement"]);
    let movement = row(&queued, &["extra", "observations", "selected", "movement"]);
    let start = point(movement, ["sx", "sy", "sz"]);
    let destination = point(movement, ["dx", "dy", "dz"]);
    let priest = point(
        row(&queued, &["state", "destination", "priest_live"]),
        ["x", "y", "z"],
    );
    let leader = point(
        row(&queued, &["state", "destination", "leader_live"]),
        ["x", "y", "z"],
    );
    assert!(
        distance(destination, priest) + 0.1 < distance(start, priest),
        "movement did not approach selected Priest: {queued}"
    );
    assert!(
        distance(destination, leader) > distance(start, leader),
        "movement fell back toward party leader: {queued}"
    );
    assert_assist_source_bodies_absent(&queued);
    assert_eq!(
        text_field(
            row(&queued, &["state", "destination", "runner"]),
            "foreground",
        ),
        foreground,
        "parking changed the selected foreground: {queued}"
    );
    assert_eq!(
        text_field(
            row(&queued, &["extra", "observations", "parked", "runner"],),
            "foreground",
        ),
        foreground,
        "the parked Runner does not retain the selected foreground: {queued}"
    );
    assert_eq!(
        rows(&queued, &["extra", "observations", "parked", "movement"],),
        selected_movement,
        "parking changed the selected leg: {queued}"
    );
    assert_eq!(
        rows(&queued, &["state", "destination", "movement"]),
        selected_movement,
        "the selected leg changed before the saved observation: {queued}"
    );
    assert_eq!(
        queued["state"]["destination"]["movement_tick"],
        queued["extra"]["observations"]["parked"]["movement_tick"],
        "parking changed the declared Core movement tick: {queued}"
    );
    assert_eq!(
        row(&queued, &["state", "destination", "companion_bot"])["next_think_micros"],
        i64::MAX.to_string(),
        "companion decision scheduler was not parked: {queued}"
    );

    let progress = assist_progress_observations(&topology, &bot, start, priest, leader);
    let followed = topology.save(
        &bot,
        "assist-followed-selected-priest",
        serde_json::json!({ "progress": progress }),
    );
    assert_eq!(
        followed["extra"]["progress"]["progressed_toward_priest"], true,
        "{followed}"
    );
    assert_assist_source_bodies_absent(&followed);
    assert_ne!(
        followed["state"]["destination"]["movement_tick"],
        queued["state"]["destination"]["movement_tick"],
        "the ordinary Core movement tick did not execute: {followed}"
    );
}
