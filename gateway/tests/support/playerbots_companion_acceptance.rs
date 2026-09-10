use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use wow_srp::normalized_string::NormalizedString;
use wow_srp::server::SrpVerifier;

use super::support;

pub const GROUP: u64 = 5_098_000;
pub const ENTRY_TRIGGER: u32 = 78;
pub const EXIT_TRIGGER: u32 = 119;
pub const DUNGEON_MAP: u32 = 36;
pub const REWARD_QUEST: u32 = 50_910;
pub const ENTRY_SOURCE: (f32, f32, f32) = (-11_208.5, 1_685.34, 25.7612);
pub const ENTRY_LANDING: (f32, f32, f32) = (-14.5732, -385.475, 62.4561);
pub const EXIT_SOURCE: (f32, f32, f32) = (-14.3628, -393.38, 64.5605);
pub const EXIT_LANDING: (f32, f32, f32) = (-11_208.7, 1_675.9, 24.5733);

const ACCOUNT: &str = "PB011ROUTE";
const PASSWORD: &str = "PASSWORD";
const TANK_LEVEL: u32 = 10;
const POLL: Duration = Duration::from_secs(60);
// Permanent auras use Timestamp(i64::MAX). SpacetimeDB 2.7.1's text SQL formatter cannot render
// that value as RFC 3339, so evidence names every other Aura column explicitly.
const AURA_EVIDENCE_COLUMNS: &str = "id, target_guid, caster_guid, spell_id, slot, level, flags, \
    applied_at, effect_id, eff_kind, amount, eff_p0, eff_p0_kind, eff_p1, period_ms, \
    amount_remaining, stacks, next_tick_micros, channel_target, enters_combat, proc_flags, \
    proc_chance, proc_ppm, proc_ex, proc_school_mask, proc_family_name, proc_family_flags, \
    proc_charges, proc_icd_ms, proc_ready_micros";

#[derive(Clone, Debug)]
pub struct Party {
    pub leader: u64,
    pub warrior: u64,
    pub priest: u64,
    pub mage_one: u64,
    pub mage_two: u64,
    pub enemies: [u64; 3],
    pub leader_name: String,
}

impl Party {
    pub fn bots(&self) -> [u64; 4] {
        [self.warrior, self.priest, self.mage_one, self.mage_two]
    }

    pub fn all(&self) -> [u64; 5] {
        [
            self.leader,
            self.warrior,
            self.priest,
            self.mage_one,
            self.mage_two,
        ]
    }
}

pub struct CompanionTopology {
    pub node: support::Standalone,
    pub source: String,
    pub destination: String,
    pub realm: String,
    pub party: Party,
    pub evidence_dir: PathBuf,
    logon_port: u16,
    world_port: u16,
}

impl CompanionTopology {
    pub fn stage(name: &str) -> Self {
        let mut node = support::Standalone::start_persistent(name);
        let source = node.shard_name().to_owned();
        let destination = format!("{source}-instances");
        let realm = format!("{source}-realm");
        let wasm = support::module_bytes();
        node.publish_module_bytes(wasm);
        node.publish_named_module_bytes(&destination, wasm);
        node.publish_named_module_bytes(&realm, wasm);
        let evidence_dir = support::log_dir().join(format!("{source}-companion-acceptance"));
        fs::create_dir_all(&evidence_dir).expect("failed to create companion evidence directory");
        let mut topology = Self {
            node,
            source,
            destination,
            realm,
            party: Party {
                leader: 0,
                warrior: 0,
                priest: 0,
                mage_one: 0,
                mage_two: 0,
                enemies: [0; 3],
                leader_name: String::new(),
            },
            evidence_dir,
            logon_port: reserve_port(),
            world_port: reserve_port(),
        };
        topology.stage_inputs();
        topology
    }

    fn stage_priest_stats(&self) {
        // Synthetic level-5 Priest inputs preserve the fixture's 120 health and zero attributes.
        // The 100 mana must survive ordinary aura recalculation and destination materialization.
        for database in [&self.source, &self.destination] {
            for (table, key, columns, values) in [
                (
                    "game_class_level_stats",
                    "class_level = 1285",
                    "class_level, class, level, base_health, base_mana",
                    "1285, 5, 5, 120, 100",
                ),
                (
                    "game_level_stats",
                    "race_class_level = 66821",
                    "race_class_level, race, class, level, strength, agility, stamina, intellect, spirit",
                    "66821, 1, 5, 5, 0, 0, 0, 0, 0",
                ),
            ] {
                assert!(
                    self.query(database, &format!("SELECT * FROM {table} WHERE {key}"))
                        .is_empty(),
                    "companion stat fixture refuses an existing {table} row"
                );
                self.node.assert_sql_database(
                    database,
                    &format!("INSERT INTO {table} ({columns}) VALUES ({values})"),
                );
            }
        }
    }

    fn stage_inputs(&mut self) {
        for database in [&self.source, &self.destination, &self.realm] {
            self.call(database, "claim_operator", &[]);
        }
        // Account ids are local to each Shard. Admission must resolve the authenticated name.
        self.call(
            &self.realm,
            "provision_account",
            &["\"PB011SPARE\"", "[]", "[]"],
        );
        let (salt, verifier) = account_material();
        for database in [&self.realm, &self.source] {
            self.call(
                database,
                "provision_account",
                &[&json!(ACCOUNT).to_string(), &salt, &verifier],
            );
        }
        let account_query = format!("SELECT id FROM game_account WHERE username = '{ACCOUNT}'");
        let source_accounts = self.query(&self.source, &account_query);
        let realm_accounts = self.query(&self.realm, &account_query);
        fs::write(
            self.evidence_dir.join("account-identities.json"),
            serde_json::to_vec_pretty(&json!({
                "source": source_accounts,
                "realm": realm_accounts,
            }))
            .unwrap(),
        )
        .expect("retain distinct World Shard and Realm Account identities");
        assert_ne!(
            one(&source_accounts, "source Account")["id"],
            one(&realm_accounts, "Realm Account")["id"],
            "the companion route must exercise distinct local Account ids"
        );
        self.call(&self.source, "install_guid_range", &["1000000"]);
        self.stage_priest_stats();
        for (count, class, role) in [("2", "1", "0"), ("1", "5", "1"), ("2", "8", "2")] {
            self.call(
                &self.source,
                "playerbots_spawn_class_role",
                &[count, "1200", "1200", "50", class, role],
            );
        }
        let bots = self.query(
            &self.source,
            "SELECT character_guid, class, role FROM pkg_playerbots_bot",
        );
        let mut warriors = role_guids(&bots, "1", "0");
        let priests = role_guids(&bots, "5", "1");
        let mut mages = role_guids(&bots, "8", "2");
        warriors.sort_unstable();
        mages.sort_unstable();
        assert_eq!((warriors.len(), priests.len(), mages.len()), (2, 1, 2));
        self.party.warrior = warriors[0];
        self.party.leader = warriors[1];
        self.party.priest = priests[0];
        self.party.mage_one = mages[0];
        self.party.mage_two = mages[1];
        self.call(&self.source, "playerbots_fixture_prepare", &[]);
        let party_args = self.party_args();
        self.call(
            &self.source,
            "playerbots_fixture_interaction_stage",
            &[&self.party.warrior.to_string(), "false"],
        );
        self.call(
            &self.source,
            "playerbots_fixture_free_slot",
            &[&self.party.warrior.to_string()],
        );
        self.call(
            &self.source,
            "playerbots_fixture_interact",
            &[&self.party.warrior.to_string(), "false", "0"],
        );
        let nav = flat_route_nav();
        for database in [&self.source, &self.destination] {
            self.call(database, "import_nav_chunks", &[&nav]);
            self.call(database, "debug_set_nav_enabled", &["true"]);
        }
        self.call(
            &self.source,
            "playerbots_companion_acceptance_stage",
            &[
                &party_args[0],
                &party_args[1],
                &party_args[2],
                &party_args[3],
                &party_args[4],
                &REWARD_QUEST.to_string(),
            ],
        );
        self.call(
            &self.source,
            "playerbots_companion_acceptance_accept_retained_quest",
            &[],
        );
        for _ in 0..8 {
            self.call(
                &self.source,
                "playerbots_fixture_free_slot",
                &[&self.party.warrior.to_string()],
            );
        }
        for database in [&self.source, &self.destination] {
            self.call(database, "playerbots_fixture_provision_catalog", &[]);
        }
        // Taunt is the supported threat-recovery tool and requires level 10.
        self.call(
            &self.source,
            "debug_set_level",
            &[&self.party.warrior.to_string(), &TANK_LEVEL.to_string()],
        );
        self.call(
            &self.source,
            "playerbots_fixture_roles_prepare_fortitude",
            &[&self.party.priest.to_string()],
        );
        for guid in self.party.bots() {
            self.call(
                &self.source,
                "playerbots_fixture_provision_steps",
                &[&guid.to_string(), "64"],
            );
        }
        let account = one(
            &self.query(
                &self.source,
                &format!("SELECT id FROM game_account WHERE username = '{ACCOUNT}'"),
            ),
            "source account",
        )["id"]
            .clone();
        self.call(
            &self.source,
            "playerbots_fixture_orders_account",
            &[&self.party.leader.to_string(), &account],
        );
        self.party.leader_name = one(
            &self.query(
                &self.source,
                &format!(
                    "SELECT name FROM game_character WHERE guid = {}",
                    self.party.leader
                ),
            ),
            "human leader",
        )["name"]
            .clone();
        self.call(
            &self.realm,
            "playerbots_companion_acceptance_realm_stage",
            &[
                &party_args[0],
                &party_args[1],
                &party_args[2],
                &party_args[3],
                &party_args[4],
            ],
        );
        self.call(
            &self.destination,
            "playerbots_companion_acceptance_destination_stage",
            &[],
        );
        let mut enemies: Vec<_> = self
            .query(
                &self.source,
                "SELECT guid FROM game_world_entity WHERE entry >= 5098001 AND entry <= 5098003",
            )
            .into_iter()
            .map(|row| parse_u64(&row, "guid"))
            .collect();
        enemies.sort_unstable();
        assert_eq!(enemies.len(), 3, "companion pull roster changed");
        self.party.enemies.copy_from_slice(&enemies);
        let staged = self.save("staged", json!({}));
        self.assert_tank_toolkit(&staged);
        for guid in self.party.enemies {
            let guid = guid.to_string();
            let enemy = staged["source"]["enemies"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["guid"].as_str() == Some(guid.as_str()))
                .expect("staged pull target missing");
            let spawn = staged["source"]["enemy_spawns"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["guid"].as_str() == Some(guid.as_str()))
                .expect("staged pull target spawn missing");
            for field in ["entry", "map_id", "x", "y", "z", "orientation"] {
                assert_eq!(
                    enemy[field], spawn[field],
                    "declared pull target {guid} has a different spawn {field}"
                );
            }
        }
    }

    fn assert_tank_toolkit(&self, staged: &Value) {
        let warrior = self.party.warrior.to_string();
        for family in ["characters", "bodies"] {
            let row = staged["source"][family]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["guid"].as_str() == Some(warrior.as_str()))
                .expect("staged Warrior missing");
            assert_eq!(row["level"], TANK_LEVEL.to_string());
        }
        let taunt = staged["source"]["spell_headers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["spell_id"] == "355")
            .expect("Taunt header missing");
        assert_eq!(taunt["spell_level"], TANK_LEVEL.to_string());
        assert_eq!(
            staged["source"]["spellbook"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|row| {
                    row["character_guid"].as_str() == Some(warrior.as_str())
                        && row["spell_id"] == "355"
                })
                .count(),
            1,
            "ordinary provisioning did not train the tank's Taunt"
        );
    }

    pub fn begin(&self) {
        self.call(&self.source, "playerbots_companion_acceptance_begin", &[]);
    }

    pub fn apply_fault_when_due(&self, fault: u8) -> Vec<Value> {
        let database = self.current_world(self.party.warrior);
        let plans = self.query(
            &database,
            "SELECT begun_micros FROM pkg_playerbots_companion_acceptance",
        );
        let faults = self.query(
            &database,
            &format!(
                "SELECT due_offset_micros FROM pkg_playerbots_companion_fault WHERE id = {fault}"
            ),
        );
        let due = parse_u64(one(&plans, "begun companion plan"), "begun_micros")
            .checked_add(parse_u64(
                one(&faults, "declared fault"),
                "due_offset_micros",
            ))
            .expect("declared fault deadline exhausted");
        let not_due = format!("companion acceptance fault is not due until {due}");
        let mut attempts = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            let output = self.node.call_database(
                &database,
                "playerbots_companion_acceptance_apply_fault",
                &[&fault.to_string()],
            );
            attempts.push(json!({
                "success": output.status.success(),
                "stdout": String::from_utf8_lossy(&output.stdout),
                "stderr": String::from_utf8_lossy(&output.stderr),
            }));
            fs::write(
                self.evidence_dir
                    .join(format!("fault-{fault}-attempts.json")),
                serde_json::to_vec_pretty(&attempts).unwrap(),
            )
            .expect("failed to retain fault attempts");
            if output.status.success() {
                return attempts;
            }
            assert!(
                String::from_utf8_lossy(&output.stderr).contains(&not_due),
                "fault {fault} failed: {}",
                attempts.last().unwrap()
            );
            assert!(Instant::now() < deadline, "fault {fault} never became due");
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    pub fn call(&self, database: &str, reducer: &str, args: &[&str]) {
        self.node.assert_call_database(database, reducer, args);
    }

    pub fn query(&self, database: &str, sql: &str) -> Vec<BTreeMap<String, String>> {
        self.node.query_database_rows(database, sql)
    }

    pub fn current_world(&self, guid: u64) -> String {
        let locator = one(
            &self.query(
                &self.realm,
                &format!("SELECT map_id FROM game_character_shard WHERE character_guid = {guid}"),
            ),
            "Realm Character locator",
        )
        .clone();
        if locator["map_id"] == DUNGEON_MAP.to_string() {
            self.destination.clone()
        } else {
            self.source.clone()
        }
    }

    pub fn gateway(&self, command_abort: bool, label: &str) -> GatewayProcess {
        GatewayProcess::spawn(self, command_abort, label)
    }

    pub fn wire(&self, label: &str) -> WireControl {
        WireControl::spawn(self, label)
    }

    pub fn wait_for_map(&self, map: u32) {
        self.wait_until("party did not settle on the expected map", || {
            self.party.all().into_iter().all(|guid| {
                self.query(
                    &self.realm,
                    &format!(
                        "SELECT character_guid FROM game_character_shard WHERE character_guid = \
                         {guid} AND map_id = {map} AND transfer_pending = false"
                    ),
                )
                .len()
                    == 1
            })
        });
    }

    pub fn wait_until(&self, description: &str, ready: impl FnMut() -> bool) {
        if !poll_until(ready) {
            eprintln!("companion wait failed: {description}");
            self.save("failed-wait", json!({"condition": description}));
            panic!("{description}");
        }
    }

    pub fn restart_module_process(&mut self) -> (u32, u32) {
        let before = self.node.process_id();
        self.node.restart_persistent();
        let after = self.node.process_id();
        assert_ne!(before, after, "persistent Module process did not change");
        (before, after)
    }

    pub fn save(&self, phase: &str, extra: Value) -> Value {
        let core = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let package = core.join("packages/playerbots");
        let evidence = json!({
            "phase": phase,
            "tested_core": git(core, &["rev-parse", "HEAD"]),
            "tested_package": git(&package, &["rev-parse", "HEAD"]),
            "core_dirty": !git(core, &["status", "--porcelain"]).is_empty(),
            "package_dirty": !git(&package, &["status", "--porcelain"]).is_empty(),
            "module_wasm_bytes": support::module_bytes().len(),
            "databases": {"source": self.source, "destination": self.destination, "realm": self.realm},
            "party": self.party_snapshot(),
            "source": self.world_snapshot(&self.source),
            "destination": self.world_snapshot(&self.destination),
            "realm": {
                "group": self.query(&self.realm, &format!("SELECT * FROM game_group WHERE group_id = {GROUP}")),
                "roster": self.query(&self.realm, &format!("SELECT * FROM game_group_roster_revision WHERE group_id = {GROUP}")),
                "members": self.query(&self.realm, &format!("SELECT * FROM game_group_member WHERE group_id = {GROUP}")),
                "partitions": self.query(&self.realm, &format!("SELECT * FROM game_group_member_partition WHERE group_id = {GROUP}")),
                "locators": self.query(&self.realm, "SELECT * FROM game_character_shard"),
            },
            "extra": extra,
        });
        let path = self
            .evidence_dir
            .join(format!("{}.json", phase.replace('_', "-")));
        fs::write(path, serde_json::to_vec_pretty(&evidence).unwrap())
            .expect("failed to save companion evidence");
        evidence
    }

    pub fn save_restart_point(&self, phase: &str, guid: u64, leader_move: Value) -> Value {
        let evidence = json!({
            "phase": phase,
            "leader_move": leader_move,
            "runner": self.query(
                &self.destination,
                &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
            ),
            "spline": self.query(
                &self.destination,
                &format!("SELECT * FROM game_creature_spline WHERE guid = {guid}"),
            ),
            "body": self.query(
                &self.destination,
                &format!("SELECT * FROM game_world_entity WHERE guid = {guid}"),
            ),
            "leader_body": self.query(
                &self.destination,
                &format!("SELECT * FROM game_world_entity WHERE guid = {}", self.party.leader),
            ),
            "quest": self.query(
                &self.destination,
                &format!(
                    "SELECT * FROM game_character_quest WHERE character_guid = {guid} AND \
                     quest_entry = 50911"
                ),
            ),
        });
        let path = self
            .evidence_dir
            .join(format!("{}.json", phase.replace('_', "-")));
        fs::write(path, serde_json::to_vec_pretty(&evidence).unwrap())
            .expect("failed to save companion restart evidence");
        evidence
    }

    fn party_snapshot(&self) -> Value {
        json!({
            "leader": self.party.leader,
            "warrior": self.party.warrior,
            "priest": self.party.priest,
            "mage_one": self.party.mage_one,
            "mage_two": self.party.mage_two,
            "enemies": self.party.enemies,
            "leader_name": self.party.leader_name,
        })
    }

    fn world_snapshot(&self, database: &str) -> Value {
        let enemy_predicate = self
            .party
            .enemies
            .into_iter()
            .map(|guid| format!("guid = {guid}"))
            .collect::<Vec<_>>()
            .join(" OR ");
        let guid_predicate = self
            .party
            .all()
            .into_iter()
            .map(|guid| format!("guid = {guid}"))
            .collect::<Vec<_>>()
            .join(" OR ");
        let owner_predicate = self
            .party
            .all()
            .into_iter()
            .map(|guid| format!("owner_guid = {guid}"))
            .collect::<Vec<_>>()
            .join(" OR ");
        let target_predicate = self
            .party
            .all()
            .into_iter()
            .map(|guid| format!("target_guid = {guid}"))
            .collect::<Vec<_>>()
            .join(" OR ");
        let caster_predicate = self
            .party
            .all()
            .into_iter()
            .map(|guid| format!("caster_guid = {guid}"))
            .collect::<Vec<_>>()
            .join(" OR ");
        let attacker_predicate = self
            .party
            .all()
            .into_iter()
            .map(|guid| format!("attacker_guid = {guid}"))
            .collect::<Vec<_>>()
            .join(" OR ");
        let quest_predicate = self
            .party
            .all()
            .into_iter()
            .map(|guid| format!("character_guid = {guid}"))
            .collect::<Vec<_>>()
            .join(" OR ");
        let items = self.query(
            database,
            &format!("SELECT * FROM game_item_instance WHERE {owner_predicate}"),
        );
        let mut item_entries = items
            .iter()
            .filter_map(|row| row.get("entry"))
            .cloned()
            .collect::<Vec<_>>();
        item_entries.sort();
        item_entries.dedup();
        let item_templates = if item_entries.is_empty() {
            Vec::new()
        } else {
            let entry_predicate = item_entries
                .iter()
                .map(|entry| format!("entry = {entry}"))
                .collect::<Vec<_>>()
                .join(" OR ");
            self.query(
                database,
                &format!(
                    "SELECT entry, max_durability FROM game_item_template WHERE {entry_predicate}"
                ),
            )
        };
        json!({
            "priest_class_stats": self.query(database, "SELECT * FROM game_class_level_stats WHERE class_level = 1285"),
            "priest_level_stats": self.query(database, "SELECT * FROM game_level_stats WHERE race_class_level = 66821"),
            "characters": self.query(database, &format!("SELECT * FROM game_character WHERE {guid_predicate}")),
            "bodies": self.query(database, &format!("SELECT * FROM game_world_entity WHERE {guid_predicate}")),
            "enemies": self.query(database, &format!("SELECT * FROM game_world_entity WHERE {enemy_predicate}")),
            "enemy_spawns": self.query(database, &format!("SELECT guid, entry, map_id, x, y, z, orientation, life_seq FROM game_creature_spawn WHERE {enemy_predicate}")),
            "bots": self.query(database, "SELECT * FROM pkg_playerbots_bot"),
            "roles": self.query(database, "SELECT character_guid, class, role FROM pkg_playerbots_bot"),
            "rotations": self.query(database, "SELECT * FROM pkg_playerbots_rotation"),
            "spellbook": self.query(database, &format!("SELECT character_guid, spell_id FROM game_player_spell WHERE {quest_predicate}")),
            "spell_headers": self.query(database, "SELECT spell_id, spell_level, cost, range_yd FROM game_spell WHERE spell_id = 355 OR spell_id = 7386 OR spell_id = 6673 OR spell_id = 2050 OR spell_id = 1243 OR spell_id = 133"),
            "threat": self.query(database, "SELECT * FROM game_threat"),
            "orders": self.query(database, "SELECT * FROM pkg_playerbots_companion_order"),
            "runners": self.query(database, "SELECT * FROM pkg_playerbots_runner"),
            "actions": self.query(database, "SELECT * FROM pkg_playerbots_action"),
            "splines": self.query(database, &format!("SELECT * FROM game_creature_spline WHERE {guid_predicate}")),
            "casts": self.query(database, &format!("SELECT * FROM game_pending_cast WHERE {caster_predicate}")),
            "auras": self.query(
                database,
                &format!(
                    "SELECT {AURA_EVIDENCE_COLUMNS} FROM game_aura WHERE {target_predicate}"
                ),
            ),
            "melee": self.query(database, &format!("SELECT * FROM game_melee_attack WHERE {attacker_predicate}")),
            "items": items,
            "item_templates": item_templates,
            "provisioning": self.query(database, "SELECT * FROM pkg_playerbots_provisioning"),
            "quests": self.query(database, &format!("SELECT * FROM game_character_quest WHERE {quest_predicate}")),
            "command_intents": self.query(database, "SELECT * FROM game_party_command_intent"),
            "command_receipts": self.query(database, "SELECT * FROM game_party_command_receipt"),
            "addon_results": self.query(database, "SELECT * FROM game_addon_message WHERE cmd = 'playerbots.order.result'"),
            "group": self.query(database, &format!("SELECT * FROM game_group WHERE group_id = {GROUP}")),
            "members": self.query(database, &format!("SELECT * FROM game_group_member WHERE group_id = {GROUP}")),
            "partitions": self.query(database, &format!("SELECT * FROM game_group_member_partition WHERE group_id = {GROUP}")),
            "faults": self.query(database, "SELECT * FROM pkg_playerbots_companion_fault"),
            "combat_receipts": self.query(database, "SELECT * FROM pkg_playerbots_companion_combat_receipt"),
            "cast_receipts": self.query(database, "SELECT * FROM pkg_playerbots_companion_cast_receipt"),
            "impact_receipts": self.query(database, "SELECT * FROM pkg_playerbots_companion_impact_receipt"),
            "impact_status": self.query(database, "SELECT * FROM pkg_playerbots_companion_impact_status"),
            "acceptance": self.query(database, "SELECT * FROM pkg_playerbots_companion_acceptance"),
        })
    }

    fn party_args(&self) -> [String; 5] {
        [
            self.party.leader.to_string(),
            self.party.warrior.to_string(),
            self.party.priest.to_string(),
            self.party.mage_one.to_string(),
            self.party.mage_two.to_string(),
        ]
    }
}

pub struct GatewayProcess {
    child: Option<Child>,
    log_path: PathBuf,
}

impl GatewayProcess {
    fn spawn(topology: &CompanionTopology, command_abort: bool, label: &str) -> Self {
        let log_path = topology.evidence_dir.join(format!("gateway-{label}.log"));
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
            .env("LYRACORE_DATABASE", &topology.source)
            .env("LYRACORE_REALM_CORE", &topology.realm)
            .env(
                "LYRACORE_SHARD_MAP",
                format!("{DUNGEON_MAP}:*={}", topology.destination),
            )
            .env("LYRACORE_COORDINATOR_TOKEN", topology.node.owner_token())
            .env(
                "LYRACORE_LOGON_BIND",
                format!("127.0.0.1:{}", topology.logon_port),
            )
            .env(
                "LYRACORE_WORLD_BIND",
                format!("127.0.0.1:{}", topology.world_port),
            )
            .env(
                "LYRACORE_REALM_ADDRESS",
                format!("127.0.0.1:{}", topology.world_port),
            )
            .env("LYRACORE_GATEWAY_ID", "pb011-companion-acceptance")
            .env("RUST_LOG", "info")
            .env_remove("LYRACORE_SHARD_MAP_FILE")
            .env_remove("LYRACORE_TRANSFER_ABORT_AFTER")
            .env_remove("LYRACORE_PARTY_COMMAND_ABORT_AFTER")
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(stderr));
        if command_abort {
            command.env("LYRACORE_PARTY_COMMAND_ABORT_AFTER", "apply_party_command");
        }
        let child = command.spawn().expect("failed to start private Gateway");
        let process = Self {
            child: Some(child),
            log_path,
        };
        wait_port(topology.logon_port, "logon");
        wait_port(topology.world_port, "world");
        process
    }

    pub fn wait_for_abort(&mut self) -> Value {
        let pid = self.child.as_ref().unwrap().id();
        let deadline = Instant::now() + POLL;
        let status = loop {
            if let Some(status) = self.child.as_mut().unwrap().try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "Gateway did not abort\n{}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(50));
        };
        let success = status.success();
        let code = status.code();
        let signal = status.signal();
        let raw_status = status.into_raw();
        let result = json!({
            "pid": pid,
            "success": success,
            "code": code,
            "signal": signal,
            "raw_status": raw_status,
            "log": self.log(),
        });
        self.child.take();
        result
    }

    pub fn log(&self) -> String {
        fs::read_to_string(&self.log_path).unwrap_or_default()
    }

    pub fn stop(&mut self) {
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
        let Some(mut child) = self.child.take() else {
            return;
        };
        if child.try_wait().ok().flatten().is_none() {
            let _ = child.kill();
        }
        let _ = child.wait();
    }
}

pub struct WireControl {
    child: Option<Child>,
    directory: PathBuf,
    next_command: u64,
}

impl WireControl {
    fn spawn(topology: &CompanionTopology, label: &str) -> Self {
        let directory = topology.evidence_dir.join(format!("wire-{label}"));
        fs::create_dir_all(&directory).unwrap();
        let (account_id, previous_claim) = wait_for_account_claim(topology, &directory);
        let log = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(directory.join("wire.log"))
            .unwrap();
        let stderr = log.try_clone().unwrap();
        let wire = std::env::var_os("LYRACORE_WIRE_BIN")
            .expect("LYRACORE_WIRE_BIN must name the pinned Headless Client");
        let child = Command::new(wire)
            .args(["scenario", "addon-control"])
            .arg(&directory)
            .arg("1800")
            .args([
                "--host",
                "127.0.0.1",
                "--logon-port",
                &topology.logon_port.to_string(),
                "--world-port",
                &topology.world_port.to_string(),
                "--account",
                ACCOUNT,
                "--character",
                &topology.party.leader_name,
                "--password-stdin",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("failed to start Headless Client control process");
        let mut control = Self {
            child: Some(child),
            directory,
            next_command: 1,
        };
        control
            .child
            .as_mut()
            .unwrap()
            .stdin
            .take()
            .unwrap()
            .write_all(format!("{PASSWORD}\n").as_bytes())
            .unwrap();
        wait_until("Headless Client did not become ready", || {
            if control.directory.join("ready.json").is_file() {
                return true;
            }
            let status = control.child.as_mut().unwrap().try_wait().unwrap();
            assert!(
                status.is_none(),
                "Headless Client exited before ready: {status:?}; log: {}",
                control.directory.join("wire.log").display()
            );
            false
        });
        let ready = read_json(&control.directory.join("ready.json"));
        assert_eq!(
            ready["character_guid"],
            json!(topology.party.leader),
            "Headless Client bound another Character: {ready}"
        );
        assert!(
            control
                .child
                .as_mut()
                .unwrap()
                .try_wait()
                .unwrap()
                .is_none(),
            "Headless Client exited at startup"
        );
        assert_replacement_claim(topology, &control.directory, &account_id, previous_claim);
        control
    }

    pub fn addon(&mut self, payload: &str, pause_after_send: bool) -> Value {
        let sequence = self.next_command;
        let text = format!("STC\tv1|playerbots.order|{sequence}|1/1|{payload}");
        self.operation(json!({
            "kind": "addon",
            "message": text,
            "reply_prefix": "STC\tv1|playerbots.order.result|",
            "pause_after_send": pause_after_send,
        }))
    }

    pub fn move_to(&mut self, from: (f32, f32, f32), to: (f32, f32, f32)) -> Value {
        self.operation(json!({"kind": "move", "from": from, "to": to, "speed": 7.0}))
    }

    pub fn area_trigger(&mut self, trigger: u32) -> Value {
        self.operation(json!({"kind": "areatrigger", "trigger_id": trigger}))
    }

    pub fn stop(&mut self) {
        if self.child.is_none() {
            return;
        }
        let evidence = self.operation(json!({"kind": "stop"}));
        assert!(
            evidence["result"]["stopped"].as_bool() == Some(true),
            "Headless Client did not confirm the stop operation"
        );
        let mut child = self.child.take().unwrap();
        if child.try_wait().unwrap().is_none() {
            child.kill().unwrap();
        }
        let _ = child.wait();
    }

    pub fn terminate(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        if child.try_wait().unwrap().is_none() {
            child.kill().unwrap();
        }
        let _ = child.wait();
    }

    fn operation(&mut self, mut body: Value) -> Value {
        let ordinal = self.next_command;
        self.next_command += 1;
        body.as_object_mut()
            .expect("wire command must be a JSON object")
            .insert("ordinal".to_string(), json!(ordinal));
        let pending = self
            .directory
            .join(format!("command-{ordinal}.json.pending"));
        let command = self.directory.join(format!("command-{ordinal}.json"));
        let authored_command = write_control_command(&pending, &command, &body);
        let sent = self.directory.join(format!("sent-{ordinal}.json"));
        wait_file(
            &sent,
            "Headless Client did not send the requested operation",
        );
        let sent_evidence = read_json(&sent);
        assert_eq!(sent_evidence["ordinal"], json!(ordinal));
        assert_eq!(sent_evidence["command"], authored_command);
        let mut evidence = json!({
            "ordinal": ordinal,
            "command": authored_command,
            "sent": sent_evidence,
        });
        let waits_for_result = evidence["command"]["kind"] != "addon"
            || !evidence["command"]["pause_after_send"]
                .as_bool()
                .unwrap_or(false);
        if waits_for_result {
            let result = self.directory.join(format!("result-{ordinal}.json"));
            wait_file(
                &result,
                "Headless Client did not retain the operation result",
            );
            let result = read_json(&result);
            assert_eq!(result["ordinal"], json!(ordinal));
            if matches!(
                evidence["command"]["kind"].as_str(),
                Some("move" | "areatrigger")
            ) {
                assert!(
                    result["sent"].as_bool() == Some(true),
                    "Headless Client did not confirm the sent operation"
                );
            }
            evidence["result"] = result;
        }
        evidence
    }
}

fn unix_micros() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros()
        .try_into()
        .unwrap()
}

fn wait_for_account_claim(
    topology: &CompanionTopology,
    directory: &Path,
) -> (String, Option<BTreeMap<String, String>>) {
    let accounts = topology.query(
        &topology.realm,
        &format!("SELECT id FROM game_account WHERE username = '{ACCOUNT}'"),
    );
    let account_id = one(&accounts, "companion Account")["id"].clone();
    let sql = format!("SELECT * FROM game_account_claim WHERE account_id = {account_id}");
    let mut previous: Option<BTreeMap<String, String>> = None;
    let mut observations = Vec::new();
    let available = support::poll_until(Duration::from_secs(90), || {
        let rows = topology.query(&topology.realm, &sql);
        let now = unix_micros();
        observations.push(json!({"observed_micros": now, "claims": rows}));
        fs::write(
            directory.join("account-before-login.json"),
            serde_json::to_vec_pretty(&observations).unwrap(),
        )
        .unwrap();
        assert!(rows.len() <= 1, "duplicate Account Claim: {rows:?}");
        let Some(claim) = rows.first() else {
            assert!(previous.is_none(), "retained Account Claim disappeared");
            return true;
        };
        assert_eq!(parse_u64(claim, "character_guid"), topology.party.leader);
        if let Some(prior) = &previous {
            for field in [
                "account_id",
                "generation",
                "request_nonce",
                "character_guid",
                "expires_micros",
            ] {
                assert_eq!(
                    claim[field], prior[field],
                    "dead Gateway claim changed: {field}"
                );
            }
        } else {
            previous = Some(claim.clone());
        }
        assert!(matches!(claim["closed"].as_str(), "true" | "false"));
        claim["closed"] == "true"
            || claim["expires_micros"]
                .parse::<i64>()
                .unwrap()
                .saturating_add(250_000)
                <= now
    });
    assert!(
        available,
        "prior Account Claim did not close or expire: {observations:?}"
    );
    (account_id, previous)
}

fn assert_replacement_claim(
    topology: &CompanionTopology,
    directory: &Path,
    account_id: &str,
    previous: Option<BTreeMap<String, String>>,
) {
    let rows = topology.query(
        &topology.realm,
        &format!("SELECT * FROM game_account_claim WHERE account_id = {account_id}"),
    );
    let now = unix_micros();
    fs::write(
        directory.join("account-after-login.json"),
        serde_json::to_vec_pretty(&json!({"observed_micros": now, "claims": rows})).unwrap(),
    )
    .unwrap();
    let claim = one(&rows, "new companion Account Claim");
    assert_eq!(claim["account_id"], account_id);
    assert_eq!(parse_u64(claim, "character_guid"), topology.party.leader);
    assert_eq!(claim["closed"], "false");
    assert!(claim["expires_micros"].parse::<i64>().unwrap() > now);
    assert_ne!(claim["request_nonce"].parse::<u128>().unwrap(), 0);
    let expected_generation = previous.as_ref().map_or(1, |prior| {
        assert_ne!(claim["request_nonce"], prior["request_nonce"]);
        parse_u64(prior, "generation").checked_add(1).unwrap()
    });
    assert_eq!(parse_u64(claim, "generation"), expected_generation);
}

impl Drop for WireControl {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        if child.try_wait().ok().flatten().is_none() {
            let _ = child.kill();
        }
        let _ = child.wait();
    }
}

pub fn one<'a>(rows: &'a [BTreeMap<String, String>], name: &str) -> &'a BTreeMap<String, String> {
    assert_eq!(rows.len(), 1, "expected one {name}, got {rows:?}");
    &rows[0]
}

pub fn parse_u64(row: &BTreeMap<String, String>, field: &str) -> u64 {
    row[field]
        .parse()
        .unwrap_or_else(|_| panic!("invalid {field} in {row:?}"))
}

fn wait_until(description: &str, ready: impl FnMut() -> bool) {
    assert!(poll_until(ready), "{description}");
}

fn poll_until(mut ready: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + POLL;
    while !ready() {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    true
}

fn account_material() -> (String, String) {
    let username = NormalizedString::new(ACCOUNT).unwrap();
    let password = NormalizedString::new(PASSWORD).unwrap();
    let verifier = SrpVerifier::from_username_and_password(username, password);
    (
        json!(verifier.salt().to_vec()).to_string(),
        json!(verifier.password_verifier().to_vec()).to_string(),
    )
}

fn role_guids(rows: &[BTreeMap<String, String>], class: &str, role: &str) -> Vec<u64> {
    rows.iter()
        .filter(|row| row["class"] == class && row["role"] == role)
        .map(|row| parse_u64(row, "character_guid"))
        .collect()
}

fn flat_route_nav() -> String {
    let points = [
        (0, ENTRY_SOURCE),
        (0, EXIT_LANDING),
        (DUNGEON_MAP, ENTRY_LANDING),
        (DUNGEON_MAP, EXIT_SOURCE),
    ];
    let mut cells = BTreeMap::new();
    for (map, (x, y, z)) in points {
        let cx = lyracore_shared::terrain::cell_index(x).unwrap();
        let cy = lyracore_shared::terrain::cell_index(y).unwrap();
        for cell_x in cx - 2..=cx + 2 {
            for cell_y in cy - 2..=cy + 2 {
                cells.entry((map, cell_x, cell_y)).or_insert(z);
            }
        }
    }
    cells
        .into_iter()
        .map(|((map, cell_x, cell_y), z)| format!("{map},{cell_x},{cell_y},{z},,"))
        .collect::<Vec<_>>()
        .join(";")
}

fn reserve_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn wait_port(port: u16, name: &str) {
    wait_until(&format!("Gateway {name} listener did not start"), || {
        TcpStream::connect(("127.0.0.1", port)).is_ok()
    });
}

fn wait_file(path: &Path, message: &str) {
    wait_until(message, || path.is_file());
}

fn write_control_command(pending: &Path, command: &Path, body: &Value) -> Value {
    let bytes = serde_json::to_vec_pretty(body).unwrap();
    let authored_command = serde_json::from_slice(&bytes).unwrap();
    fs::write(pending, bytes).unwrap();
    fs::rename(pending, command).unwrap();
    authored_command
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn serialized_move_command_preserves_requested_f32_coordinates() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "lyracore-companion-command-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&directory).unwrap();
    let pending = directory.join("command-1.json.pending");
    let command = directory.join("command-1.json");
    let from = (3.4267998_f32, -385.475_f32, 62.4561_f32);
    let to = (from.0 + 18.0, from.1, from.2);
    let body = json!({"kind": "move", "from": from, "to": to, "speed": 7.0, "ordinal": 1});

    let authored_command = write_control_command(&pending, &command, &body);

    assert_eq!(authored_command, read_json(&command));
    assert_eq!(
        fs::read(&command).unwrap(),
        serde_json::to_vec_pretty(&body).unwrap()
    );
    for (field, expected) in [("from", from), ("to", to)] {
        let actual = authored_command[field].as_array().unwrap();
        assert_eq!(actual.len(), 3);
        for (value, expected) in actual.iter().zip([expected.0, expected.1, expected.2]) {
            assert_eq!(value.as_f64().unwrap() as f32, expected);
        }
    }
    assert_eq!(authored_command["kind"], "move");
    assert_eq!(authored_command["ordinal"], 1);
    assert_eq!(authored_command["speed"].as_f64().unwrap() as f32, 7.0);
    fs::remove_dir_all(directory).unwrap();
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
