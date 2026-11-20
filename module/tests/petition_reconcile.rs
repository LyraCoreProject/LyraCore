mod support;

use std::io::Cursor;
use std::time::Duration;

use lyracore_package_delta::fixture_ids::FIXTURE_BLADE;
use serde_json::json;
use support::{module_bytes, number, poll_until, ReducerConnection, Standalone};
use wow_world_messages::vanilla::{ClientMessage, CMSG_AUTH_SESSION};

// SHA-1 for CONFORMANCE, client seed 0, server seed 42 and key [0x5a; 40].
const PROOF: [u8; 20] = [
    0, 219, 190, 39, 108, 63, 12, 6, 223, 186, 9, 97, 43, 173, 114, 133, 133, 42, 164, 178,
];
const GUILD_CHARTER: u32 = 5863;
const KALIMDOR: u32 = 1;
const VALLEY_OF_TRIALS: [f32; 3] = [-618.5, -4251.7, 38.7];
const DEEPRUN_TRAM: u32 = 369;
const TO_KALIMDOR: u32 = 900_001;
const TO_DEEPRUN_TRAM: u32 = 900_002;
const TO_EASTERN_KINGDOMS: u32 = 900_003;
const CMSG_AUTH_SESSION_OPCODE: u32 = 0x1ed;
const CMSG_PLAYER_LOGIN: u32 = 0x3d;
const CMSG_AREATRIGGER: u32 = 0xb4;
const CMSG_LOGOUT_REQUEST: u32 = 0x4b;
const CMSG_PETITION_BUY: u32 = 0x1bd;
const CMSG_SELL_ITEM: u32 = 0x1a0;
const CHARTER_VENDOR: &str = "51004";
const VENDOR_PETITIONER_AND_TABARD_DESIGNER: u32 = 0x4 | 0x200 | 0x400;
const MSG_MOVE_WORLDPORT_ACK: u32 = 0xdc;
const SMSG_NEW_WORLD: u16 = 0x3e;
const SMSG_NOTIFICATION: u16 = 0x1cb;
const CMSG_PING: u32 = 0x1dc;
const SMSG_PONG: u16 = 0x1dd;

fn peer(byte: &str) -> String {
    format!(r#"{{"__identity__":"0x{}"}}"#, byte.repeat(32))
}

fn database_identity(shard: &Standalone, database: &str) -> String {
    let decimal = &shard.query_database_rows(database, "SELECT database_identity FROM st_module")
        [0]["database_identity"];
    let mut limbs = [0u64; 4];
    for digit in decimal.bytes() {
        let mut carry = u128::from(digit - b'0');
        for limb in limbs.iter_mut().rev() {
            let wide = u128::from(*limb) * 10 + carry;
            *limb = wide as u64;
            carry = wide >> 64;
        }
    }
    let hex: String = limbs.iter().map(|limb| format!("{limb:016x}")).collect();
    format!(r#"{{"__identity__":"0x{hex}"}}"#)
}

struct World {
    shard: Standalone,
    account: String,
    character: u64,
}

fn with_character(shard: Standalone) -> World {
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    let credentials = serde_json::to_string(&vec![0u8; 32]).unwrap();
    shard.assert_call(
        "provision_account",
        &["\"CONFORMANCE\"", &credentials, &credentials],
    );
    let account = shard.query_rows("SELECT id FROM game_account WHERE username = 'CONFORMANCE'")[0]
        ["id"]
        .clone();
    shard.assert_call(
        "create_character",
        &[
            &account,
            "\"Petitioner\"",
            "1",
            "1",
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
        ],
    );
    let character = number(
        &shard.query_rows("SELECT guid FROM game_character WHERE name = 'Petitioner'")[0],
        "guid",
    );
    World {
        shard,
        account,
        character,
    }
}

/// A World Shard whose Realm Shard and other World Shard never answer.
fn world_shard(name: &str) -> World {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    let world = with_character(shard);
    let shard = &world.shard;
    let (realm, other_world) = (peer("11"), peer("22"));
    for world in [&realm, &other_world] {
        shard.assert_call("operator_add_peer_shard", &[world, "\"unreachable-peer\""]);
    }
    shard.assert_call(
        "operator_set_shard_role",
        &[&format!(r#"{{"some":{{"world":{realm}}}}}"#)],
    );
    let here = database_identity(shard, shard.shard_name());
    for map in ["0", "1"] {
        shard.assert_call("operator_set_map_owner", &[map, &here]);
    }
    shard.assert_call(
        "operator_set_map_owner",
        &[&DEEPRUN_TRAM.to_string(), &other_world],
    );
    world
}

struct Realm {
    world: World,
    database: String,
    world_identity: String,
}

/// A World Shard and its Realm Shard in one Host, both answering.
fn realm_and_world(name: &str) -> Realm {
    let mut shard = Standalone::start_persistent(name);
    shard.publish_module();
    let database = format!("{}-realm", shard.shard_name());
    shard.publish_named_module_bytes(&database, module_bytes());
    shard.assert_call_database(&database, "claim_operator", &[]);
    let world = with_character(shard);
    let shard = &world.shard;
    let world_identity = database_identity(shard, shard.shard_name());
    let realm_identity = database_identity(shard, &database);
    shard.assert_call("operator_add_peer_shard", &[&realm_identity, "\"realm\""]);
    shard.assert_call_database(
        &database,
        "operator_add_peer_shard",
        &[&world_identity, "\"world\""],
    );
    let confirmed = |database: &str| {
        shard
            .query_database_rows(database, "SELECT confirmed FROM game_peer_shard")
            .iter()
            .all(|row| row["confirmed"] == "true")
    };
    assert!(
        poll_until(Duration::from_secs(30), || confirmed(shard.shard_name())
            && confirmed(&database)),
        "the Realm and World Shards did not confirm each other"
    );
    shard.assert_call_database(
        &database,
        "operator_set_shard_role",
        &[&format!(r#"{{"some":{{"realm":{world_identity}}}}}"#)],
    );
    shard.assert_call(
        "operator_set_shard_role",
        &[&format!(r#"{{"some":{{"world":{realm_identity}}}}}"#)],
    );
    for map in ["0", "1"] {
        shard.assert_call("operator_set_map_owner", &[map, &world_identity]);
    }
    let realm = Realm {
        world,
        database,
        world_identity,
    };
    let directory = format!(
        "SELECT guid FROM game_character_directory WHERE guid = {}",
        realm.world.character
    );
    assert!(
        poll_until(Duration::from_secs(30), || !realm
            .world
            .shard
            .query_database_rows(&realm.database, &directory)
            .is_empty()),
        "the Realm Shard did not record the Character"
    );
    realm
}

impl Realm {
    /// Logs in and records the World Session on the Realm, as a Realm login handoff does.
    fn log_in(&self) -> ReducerConnection {
        let wire = self.world.log_in();
        self.world.shard.assert_call_database(
            &self.database,
            "debug_native_away",
            &[
                &self.world.account,
                &wire.connection_id().to_string(),
                &self.world_identity,
                &self.world.character.to_string(),
            ],
        );
        wire
    }

    fn charter_vendor(&self) -> u64 {
        let shard = &self.world.shard;
        shard.assert_sql(&format!(
            "UPDATE game_item_template SET entry = {GUILD_CHARTER}, sell_price = 13 WHERE entry = {FIXTURE_BLADE}"
        ));
        shard.assert_call(
            "debug_spawn_at_feet",
            &[&self.world.character.to_string(), CHARTER_VENDOR, "1"],
        );
        let vendor = shard.query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {CHARTER_VENDOR}"
        ))[0]["guid"]
            .clone();
        shard.assert_sql(&format!(
            "UPDATE game_world_entity SET npc_flags = {VENDOR_PETITIONER_AND_TABARD_DESIGNER} WHERE guid = {vendor}"
        ));
        shard.assert_call(
            "debug_set_money",
            &[&self.world.character.to_string(), "150000"],
        );
        vendor.parse().unwrap()
    }

    fn buy_charter(&self, wire: &mut ReducerConnection, vendor: u64, name: &str) {
        let mut body = vendor.to_le_bytes().to_vec();
        body.extend(0u32.to_le_bytes());
        body.extend(0u64.to_le_bytes());
        body.extend(name.as_bytes());
        body.push(0);
        body.extend([0u8; 51]);
        send(wire, CMSG_PETITION_BUY, body);
        self.settle();
    }

    fn settle(&self) {
        let shard = &self.world.shard;
        let pending = format!(
            "SELECT guid FROM game_guild_transaction WHERE guid = {}",
            self.world.character
        );
        assert!(
            poll_until(Duration::from_secs(30), || shard
                .query_rows(&pending)
                .is_empty()
                && shard
                    .query_rows("SELECT id FROM game_realm_guild_command")
                    .is_empty()),
            "the guild transaction did not settle: {:?}",
            shard.query_rows("SELECT * FROM game_guild_transaction")
        );
    }

    fn petition_charter(&self) -> Option<u64> {
        self.world
            .shard
            .query_database_rows(
                &self.database,
                &format!(
                    "SELECT charter_item_guid FROM game_guild_petition WHERE owner_guid = {}",
                    self.world.character
                ),
            )
            .first()
            .map(|row| number(row, "charter_item_guid"))
    }

    fn money(&self) -> u32 {
        number(&self.world.entity(), "money")
    }
}

impl World {
    fn entity(&self) -> support::Row {
        self.shard
            .query_rows(&format!(
                "SELECT * FROM game_world_entity WHERE guid = {}",
                self.character
            ))
            .remove(0)
    }

    fn portal(&self, trigger: u32, target_map: u32, [x, y, z]: [f32; 3]) {
        let at = self.entity();
        self.shard.insert_row(
            "game_area_trigger",
            &[
                ("id", &trigger),
                ("map_id", &number::<u32>(&at, "map_id")),
                ("x", &number::<f32>(&at, "x")),
                ("y", &number::<f32>(&at, "y")),
                ("z", &number::<f32>(&at, "z")),
                ("radius", &5.0f32),
                ("box_length", &0.0f32),
                ("box_width", &0.0f32),
                ("box_height", &0.0f32),
                ("box_yaw", &0.0f32),
            ],
        );
        self.shard.insert_row(
            "game_areatrigger_teleport",
            &[
                ("trigger_id", &trigger),
                ("target_map", &target_map),
                ("x", &x),
                ("y", &y),
                ("z", &z),
                ("o", &0.0f32),
                ("name", &"Fixture portal"),
            ],
        );
    }

    /// Logs in with a world proof, which the Transfer handoff requires.
    fn log_in(&self) -> ReducerConnection {
        self.shard.assert_call(
            "establish_session",
            &[
                &self.account,
                &serde_json::to_string(&vec![0x5au8; 40]).unwrap(),
                r#"{"__identity__":"0x1234"}"#,
            ],
        );
        let mut wire = self
            .shard
            .connect_reducers(&["SELECT * FROM game_native_packet"]);
        wire.call("native_world_open", json!([42])).unwrap();
        let mut frame = Vec::new();
        CMSG_AUTH_SESSION {
            build: 5875,
            server_id: 0,
            username: "CONFORMANCE".into(),
            client_seed: 0,
            client_proof: PROOF,
            addon_info: vec![],
        }
        .write_unencrypted_client(&mut Cursor::new(&mut frame))
        .unwrap();
        send(&mut wire, CMSG_AUTH_SESSION_OPCODE, frame[6..].to_vec());
        send(
            &mut wire,
            CMSG_PLAYER_LOGIN,
            self.character.to_le_bytes().to_vec(),
        );
        wire
    }

    /// Takes a portal to Kalimdor, a map this Shard owns, and arrives through a WorldPort entry.
    fn port_to_kalimdor(&self, wire: &mut ReducerConnection) {
        let [x, y, z] = VALLEY_OF_TRIALS;
        self.portal(TO_KALIMDOR, KALIMDOR, [x, y, z]);
        let replies = enter_trigger(wire, TO_KALIMDOR);
        assert!(
            replies.contains(&SMSG_NEW_WORLD),
            "the portal must not wait for the Realm: {replies:x?}"
        );
        send(wire, MSG_MOVE_WORLDPORT_ACK, Vec::new());
        assert_eq!(number::<u32>(&self.entity(), "map_id"), KALIMDOR);
        self.portal(TO_DEEPRUN_TRAM, DEEPRUN_TRAM, [0.0, 0.0, 0.0]);
    }

    fn transfers(&self) -> usize {
        self.shard
            .query_rows(&format!(
                "SELECT transfer_id FROM game_transfer_out WHERE character_guid = {}",
                self.character
            ))
            .len()
    }
}

fn send(wire: &mut ReducerConnection, opcode: u32, body: Vec<u8>) -> Vec<u16> {
    wire.call("native_world_receive", json!([opcode, body]))
        .unwrap();
    wire.drain_packets()
        .into_iter()
        .filter_map(|row| {
            let row: serde_json::Value = serde_json::from_str(row.get()).unwrap();
            let [tag, opcode] = &row[2].as_array().unwrap()[..] else {
                panic!("game_native_packet opcode is a sum value: {row}");
            };
            (tag == 0).then(|| opcode.as_u64().unwrap() as u16)
        })
        .collect()
}

fn enter_trigger(wire: &mut ReducerConnection, trigger: u32) -> Vec<u16> {
    send(wire, CMSG_AREATRIGGER, trigger.to_le_bytes().to_vec())
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_character_without_a_charter_zones_and_transfers_while_the_realm_is_unreachable() {
    let world = world_shard("reconcile-without-charter");
    let mut wire = world.log_in();
    world.port_to_kalimdor(&mut wire);
    let replies = enter_trigger(&mut wire, TO_DEEPRUN_TRAM);
    assert!(
        replies.contains(&SMSG_NEW_WORLD),
        "the portal to another World Shard must start at once: {replies:x?}"
    );
    send(&mut wire, MSG_MOVE_WORLDPORT_ACK, Vec::new());
    assert_eq!(world.transfers(), 1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_charter_holder_zones_locally_but_keeps_its_shard_until_the_realm_reconciles() {
    let world = world_shard("reconcile-with-charter");
    world.shard.assert_sql(&format!(
        "UPDATE game_item_template SET entry = {GUILD_CHARTER} WHERE entry = {FIXTURE_BLADE}"
    ));
    let mut wire = world.log_in();
    world.shard.assert_call(
        "debug_grant_item",
        &[
            &world.character.to_string(),
            &GUILD_CHARTER.to_string(),
            "1",
        ],
    );
    let home = world.entity();
    world.port_to_kalimdor(&mut wire);
    let replies = enter_trigger(&mut wire, TO_DEEPRUN_TRAM);
    assert!(
        replies.contains(&SMSG_NOTIFICATION) && !replies.contains(&SMSG_NEW_WORLD),
        "the portal to another World Shard must be refused with a notice: {replies:x?}"
    );
    assert_eq!(world.transfers(), 0);
    world.portal(
        TO_EASTERN_KINGDOMS,
        0,
        ["x", "y", "z"].map(|axis| number(&home, axis)),
    );
    let replies = enter_trigger(&mut wire, TO_EASTERN_KINGDOMS);
    assert!(
        replies.contains(&SMSG_NEW_WORLD),
        "a pending reconciliation must not hold a portal within the Shard: {replies:x?}"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_flying_character_ignores_area_triggers() {
    let world = world_shard("reconcile-taxi-flight");
    let mut wire = world.log_in();
    world.shard.insert_row(
        "game_active_taxi_flight",
        &[
            ("character_guid", &world.character),
            ("path_id", &0),
            ("source_node_id", &0),
            ("destination_node_id", &0),
            ("mount_display_id", &1147),
        ],
    );
    let [x, y, z] = VALLEY_OF_TRIALS;
    world.portal(TO_KALIMDOR, KALIMDOR, [x, y, z]);
    world.shard.assert_call(
        "debug_enter_areatrigger",
        &[&world.character.to_string(), &TO_KALIMDOR.to_string()],
    );
    let replies = send(&mut wire, CMSG_PING, vec![0; 8]);
    assert!(
        replies.contains(&SMSG_PONG)
            && !replies.contains(&SMSG_NOTIFICATION)
            && !replies.contains(&SMSG_NEW_WORLD),
        "a flying Character's AreaTrigger must send nothing: {replies:x?}"
    );
    assert_eq!(number::<u32>(&world.entity(), "map_id"), 0);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn logging_out_after_selling_a_charter_closes_its_petition() {
    let realm = realm_and_world("petition-sold-charter");
    let mut wire = realm.log_in();
    let vendor = realm.charter_vendor();
    realm.buy_charter(&mut wire, vendor, "Sold Founders");
    let charter = realm
        .petition_charter()
        .expect("the first charter purchase opens a Petition");
    let mut body = vendor.to_le_bytes().to_vec();
    body.extend(charter.to_le_bytes());
    body.push(0);
    send(&mut wire, CMSG_SELL_ITEM, body);
    send(&mut wire, CMSG_LOGOUT_REQUEST, Vec::new());
    realm.settle();
    assert_eq!(realm.petition_charter(), None);
    drop(wire);
    let mut wire = realm.log_in();
    realm.buy_charter(&mut wire, vendor, "Second Founders");
    assert!(realm
        .petition_charter()
        .is_some_and(|second| second != charter));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_purchase_refused_for_an_orphaned_petition_closes_it_for_the_next_purchase() {
    let realm = realm_and_world("petition-orphan-recovery");
    let mut wire = realm.log_in();
    let vendor = realm.charter_vendor();
    realm.buy_charter(&mut wire, vendor, "Lost Founders");
    let charter = realm
        .petition_charter()
        .expect("the first charter purchase opens a Petition");
    realm.world.shard.assert_sql(&format!(
        "DELETE FROM game_item_instance WHERE guid = {charter}"
    ));
    let money = realm.money();
    realm.buy_charter(&mut wire, vendor, "Found Founders");
    assert_eq!(realm.petition_charter(), None);
    assert_eq!(realm.money(), money);
    realm.buy_charter(&mut wire, vendor, "Found Founders");
    assert!(realm
        .petition_charter()
        .is_some_and(|second| second != charter));
    assert!(realm.money() < money);
}
