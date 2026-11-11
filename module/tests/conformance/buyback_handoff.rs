use super::*;
use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, FIXTURE_JERKY, SCENARIO_VENDOR_ENTRY};
use std::collections::BTreeMap;
use std::io::Cursor;
use wow_world_messages::vanilla::{ClientMessage, CMSG_AUTH_SESSION};

// SHA-1 for CONFORMANCE, client seed 0, server seed 42 and key [0x5a; 40].
const PROOF: [u8; 20] = [
    0, 219, 190, 39, 108, 63, 12, 6, 223, 186, 9, 97, 43, 173, 114, 133, 133, 42, 164, 178,
];

fn session(shard: &Standalone) {
    let account = shard.query_rows("SELECT id FROM game_account WHERE username = 'CONFORMANCE'")[0]
        ["id"]
        .clone();
    shard.assert_call(
        "establish_session",
        &[
            &account,
            &serde_json::to_string(&vec![0x5au8; 40]).unwrap(),
            r#"{"__identity__":"0x1234"}"#,
        ],
    );
}

fn authenticate(shard: &Standalone, guid: u64) -> ReducerConnection {
    session(shard);
    let mut connection = shard.connect_reducers(&["SELECT * FROM game_native_packet"]);
    connection.call("native_world_open", json!([42])).unwrap();
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
    connection
        .call("native_world_receive", json!([0x1ed, &frame[6..]]))
        .unwrap();
    connection
        .call("native_world_receive", json!([0x3d, guid.to_le_bytes()]))
        .unwrap();
    connection.drain_packets();
    connection
}

fn database_identity(shard: &Standalone) -> String {
    let decimal =
        &shard.query_rows("SELECT database_identity FROM st_module")[0]["database_identity"];
    let mut limbs = [0u64; 4];
    for digit in decimal.bytes() {
        let mut carry = u128::from(digit - b'0');
        for limb in limbs.iter_mut().rev() {
            let wide = u128::from(*limb) * 10 + carry;
            *limb = wide as u64;
            carry = wide >> 64;
        }
    }
    format!(
        r#"{{"__identity__":"0x{}"}}"#,
        limbs
            .iter()
            .map(|limb| format!("{limb:016x}"))
            .collect::<String>()
    )
}

fn receive_handoff(
    shard: &Standalone,
    guid: u64,
    transfer: &str,
    blob: &str,
) -> (ReducerConnection, std::process::Output) {
    use spacetimedb::{Identity, Timestamp};
    let account_id = shard.query_rows("SELECT id FROM game_account WHERE username = 'CONFORMANCE'")
        [0]["id"]
        .parse()
        .unwrap();
    let identity = Some(Identity::from_hex(format!("{:064x}", 0x1234)).unwrap());
    let account = lyracore_module::Account {
        id: account_id,
        username: "CONFORMANCE".into(),
        salt: vec![0; 32],
        verifier: vec![0; 32],
        identity,
        banned: false,
        alpha_test_tools: true,
        gm_level: 0,
    };
    let session = lyracore_module::Session {
        account_id,
        session_key: vec![0x5a; 40],
        identity,
        created_at: Timestamp::UNIX_EPOCH,
        expires_at: Timestamp::from_micros_since_unix_epoch(i64::MAX),
    };
    let realm = format!(r#"{{"__identity__":"0x{}"}}"#, "11".repeat(32));
    let operator = shard.query_rows("SELECT identity FROM game_operator")[0]["identity"].clone();
    for peer in [&realm, &format!(r#"{{"__identity__":"{operator}"}}"#)] {
        shard.assert_call("operator_add_peer_shard", &[peer, "\"fixture-peer\""]);
    }
    shard.assert_call(
        "operator_set_shard_role",
        &[&format!(r#"{{"some":{{"world":{realm}}}}}"#)],
    );
    let characters = shard.query_rows(&format!(
        "SELECT map_id FROM game_character WHERE guid = {guid}"
    ));
    let map = characters.first().map_or("0", |row| row["map_id"].as_str());
    shard.assert_call("operator_set_map_owner", &[map, &database_identity(shard)]);
    let connection = shard.connect_reducers(&["SELECT * FROM game_native_packet"]);
    let args = vec![
        realm,
        transfer.into(),
        connection.connection_id().to_string(),
        "42".into(),
        serde_json::to_string(&spacetimedb::sats::bsatn::to_vec(&account).unwrap()).unwrap(),
        serde_json::to_string(&spacetimedb::sats::bsatn::to_vec(&session).unwrap()).unwrap(),
        "0".into(),
        serde_json::to_string(&PROOF).unwrap(),
        guid.to_string(),
        "1".into(),
        blob.into(),
        "0".into(),
        "[]".into(),
        "true".into(),
    ];
    let result = shard.call(
        "idc_native_transfer",
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    (connection, result)
}

fn adopt(
    shard: &Standalone,
    guid: u64,
    transfer: &str,
    blob: &str,
    keep_realm_role: bool,
) -> (ReducerConnection, Vec<(u16, Vec<u8>)>) {
    let (mut connection, result) = receive_handoff(shard, guid, transfer, blob);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    shard.assert_call(
        "release_transfer",
        &[transfer, r#"{"guid":0,"ownership":null}"#],
    );
    if !keep_realm_role {
        shard.assert_call("operator_set_shard_role", &[r#"{"none":[]}"#]);
    }
    connection.call("native_world_adopt", json!([])).unwrap();
    let packets = delivered(&mut connection);
    (connection, packets)
}

struct Crossing {
    source: Standalone,
    destination: Standalone,
    source_connection: ReducerConnection,
    connection: ReducerConnection,
    guid: u64,
    items: Vec<BTreeMap<String, String>>,
    state: Vec<BTreeMap<String, String>>,
    arrival: Vec<(u16, Vec<u8>)>,
    departure: Vec<(u16, Vec<u8>)>,
    escrow_items: Vec<BTreeMap<String, String>>,
    escrow_state: Vec<BTreeMap<String, String>>,
    transported_equip_auras: usize,
    cooldowns: Vec<BTreeMap<String, String>>,
}

fn crossing(name: &str) -> Crossing {
    crossing_with_equipment(name, false)
}

fn crossing_with_equipment(name: &str, equipment: bool) -> Crossing {
    let (source, old_connection, guid, vendor) = stage(&format!("{name}-source"));
    let mut source_connection = authenticate(&source, guid);
    drop(old_connection);
    source.assert_sql(&format!(
        "DELETE FROM game_item_instance WHERE owner_guid = {guid}"
    ));
    source
        .assert_sql("UPDATE game_conformance_clock SET now_micros = 1800000005000000 WHERE id = 0");
    for index in 0..3 {
        let item = grant(&source, guid, FIXTURE_BLADE, 1);
        if index == 0 {
            poison(&source, &mut source_connection, guid, item);
        }
        sell(&mut source_connection, vendor, item, 0);
    }
    buyback(&mut source_connection, vendor, 70);
    let items = sold_rows(&source, guid);
    let state = source.query_rows("SELECT * FROM game_buyback_state");
    if equipment {
        super::super::item_equip::configure(&source);
        source.assert_sql("UPDATE game_spell SET category = 30, category_cooldown_ms = 60000 WHERE spell_id = 5090901");
        let item = grant(&source, guid, FIXTURE_BLADE, 1);
        let slot: u8 = source.query_rows(&format!(
            "SELECT slot FROM game_item_instance WHERE guid = {item}"
        ))[0]["slot"]
            .parse()
            .unwrap();
        request(&mut source_connection, 0x10a, vec![255, slot]);
        super::super::item_equip::assert_effect(&source, guid, 1);
    }
    let realm = format!(r#"{{"__identity__":"0x{}"}}"#, "11".repeat(32));
    let target = format!(r#"{{"__identity__":"0x{}"}}"#, "22".repeat(32));
    for peer in [&realm, &target] {
        source.assert_call("operator_add_peer_shard", &[peer, "\"unavailable-peer\""]);
    }
    source.assert_call(
        "operator_set_shard_role",
        &[&format!(r#"{{"some":{{"world":{realm}}}}}"#)],
    );
    source.assert_call("operator_set_map_owner", &["1", &target]);
    source.assert_call(
        "debug_teleport",
        &[&guid.to_string(), "1", "0", "0", "0", "0"],
    );
    let departure = request(&mut source_connection, 0xdc, Vec::new());
    let escrow_items = sold_rows(&source, guid);
    let escrow_state = source.query_rows("SELECT * FROM game_buyback_state");
    let out = source
        .query_rows("SELECT transfer_id,blob FROM game_transfer_out")
        .remove(0);
    let cooldowns = source.query_rows(&format!(
        "SELECT spell_id,ready_at,category,category_ready_micros FROM game_spell_cd WHERE caster_guid = {guid}"
    ));
    let transfer = &out["transfer_id"];
    let blob = serde_json::to_string(out["blob"].strip_prefix("0x").unwrap()).unwrap();
    let transported_equip_auras = if equipment {
        count_transported_equip_auras(&out["blob"])
    } else {
        0
    };
    let (destination, _, _) = sessionless(&format!("{name}-destination"));
    if equipment {
        super::super::item_equip::configure(&destination);
        destination.assert_sql("UPDATE game_spell SET category = 30, category_cooldown_ms = 60000 WHERE spell_id = 5090901");
    }
    destination.assert_call(
        "debug_start_virtual_clock",
        &[if equipment {
            "1800000005000000"
        } else {
            "1800003605000000"
        }],
    );
    destination.assert_sql(&format!(
        "DELETE FROM game_world_entity WHERE guid = {guid}"
    ));
    destination.assert_call(
        "import_character_blob",
        &[transfer, &blob, r#"{"guid":0,"ownership":null}"#],
    );
    source.assert_call(
        "confirm_import",
        &[transfer, r#"{"guid":0,"ownership":null}"#],
    );
    source.assert_call(
        "finish_transfer",
        &[transfer, r#"{"guid":0,"ownership":null}"#],
    );
    let (connection, arrival) = adopt(&destination, guid, transfer, &blob, false);
    Crossing {
        source,
        destination,
        source_connection,
        connection,
        guid,
        items,
        state,
        arrival,
        departure,
        escrow_items,
        escrow_state,
        transported_equip_auras,
        cooldowns,
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn equip_auras_rebuild_once_after_native_handoff_and_world_port() {
    let c = crossing_with_equipment("equip-native-handoff", true);
    assert_eq!(c.transported_equip_auras, 0);
    assert!(!c.cooldowns.is_empty());
    assert_eq!(
        c.destination.query_rows(&format!(
            "SELECT spell_id,ready_at,category,category_ready_micros FROM game_spell_cd WHERE caster_guid = {}",
            c.guid
        )),
        c.cooldowns
    );
    super::super::item_equip::assert_effect(&c.destination, c.guid, 1);
    assert!(c
        .source
        .query_rows(&format!(
            "SELECT id FROM game_aura WHERE target_guid = {} AND spell_id = 5090900",
            c.guid
        ))
        .is_empty());
    assert!(!c.arrival.iter().any(|(opcode, _)| *opcode == 0xb0));
    let initial = &c
        .arrival
        .iter()
        .find(|(opcode, _)| *opcode == 0x12a)
        .unwrap()
        .1;
    let known = usize::from(u16::from_le_bytes(initial[1..3].try_into().unwrap()));
    let start = 3 + 4 * known;
    let count = usize::from(u16::from_le_bytes(
        initial[start..start + 2].try_into().unwrap(),
    ));
    let cooldown = initial[start + 2..]
        .chunks_exact(14)
        .take(count)
        .find(|row| row[0..2] == (5090901u32 as u16).to_le_bytes())
        .unwrap();
    assert_eq!(&cooldown[2..4], &[0, 0]);
    assert_eq!(&cooldown[4..6], &30u16.to_le_bytes());
    assert_eq!(&cooldown[6..10], &30000u32.to_le_bytes());
    assert_eq!(&cooldown[10..14], &60000u32.to_le_bytes());
    assert!(!c
        .destination
        .call(
            "gw_use_item",
            &[&format!(r#"{{"guid":{},"ownership":null}}"#, c.guid), "10"]
        )
        .status
        .success());
    c.destination
        .assert_call("debug_spawn_player_entity", &[&c.guid.to_string()]);
    super::super::item_equip::assert_effect(&c.destination, c.guid, 1);
}

fn count_transported_equip_auras(hex: &str) -> usize {
    #[derive(spacetimedb::SpacetimeType)]
    struct Manifest {
        table: String,
        hot: bool,
    }
    #[derive(spacetimedb::SpacetimeType)]
    struct Rows {
        table: String,
        rows: Vec<u8>,
    }
    #[derive(spacetimedb::SpacetimeType)]
    struct Blob {
        transfer_id: u64,
        character_guid: u64,
        money: u32,
        manifest: Vec<Manifest>,
        dest_map_id: u32,
        dest_instance_id: u64,
        dest_x: f32,
        dest_y: f32,
        dest_z: f32,
        dest_o: f32,
        character_row: Vec<u8>,
        payload: Vec<Rows>,
    }
    let hex = hex.strip_prefix("0x").unwrap();
    let bytes: Vec<_> = (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
        .collect();
    let blob: Blob = spacetimedb::sats::bsatn::from_slice(&bytes).unwrap();
    let rows = blob
        .payload
        .iter()
        .find(|rows| rows.table == "game_aura")
        .unwrap();
    let auras: Vec<lyracore_module::Aura> =
        spacetimedb::sats::bsatn::from_slice(&rows.rows).unwrap();
    auras.iter().filter(|aura| aura.spell_id == 5090900).count()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn handoff_keeps_buyback_ring_state_and_arrival_fields() {
    let mut c = crossing("buyback-handoff-ring");
    assert_eq!(
        c.escrow_items, c.items,
        "unattested departure keeps the source Items"
    );
    assert_eq!(c.escrow_state, c.state);
    assert_eq!(sold_rows(&c.destination, c.guid), c.items);
    assert_eq!(
        c.destination.query_rows("SELECT * FROM game_buyback_state"),
        c.state
    );
    let fields = values(&c.arrival, c.guid);
    for item in &c.items {
        let guid: u64 = item["item_guid"].parse().unwrap();
        let slot: u16 = item["slot"].parse().unwrap();
        assert_eq!(fields[&(624 + 2 * (slot - 69))], guid as u32);
        assert_eq!(fields[&(1226 + slot - 69)], 13);
        assert_eq!(fields[&(1238 + slot - 69)], 108005);
        assert_eq!(values(&c.arrival, guid)[&14], 1);
    }
    let first: u64 = c.items[0]["item_guid"].parse().unwrap();
    let enchantment = values(&c.arrival, first);
    assert_eq!(enchantment[&25], 323);
    assert_eq!(enchantment[&26], 60_000);
    assert_eq!(enchantment[&27], 40);
    c.source_connection
        .call("native_world_reply", json!([[]]))
        .unwrap();
    c.departure.extend(delivered(&mut c.source_connection));
    for row in &c.items {
        let guid: u64 = row["item_guid"].parse().unwrap();
        assert!(c
            .departure
            .iter()
            .all(|(opcode, body)| *opcode != 0xaa || body != &guid.to_le_bytes()));
    }
    assert!(sold_rows(&c.source, c.guid).is_empty());
    c.destination.assert_call(
        "debug_spawn_at_feet",
        &[
            &c.guid.to_string(),
            &SCENARIO_VENDOR_ENTRY.to_string(),
            "2.0",
        ],
    );
    let vendor = c
        .destination
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {SCENARIO_VENDOR_ENTRY}"
        ))
        .iter()
        .map(|row| row["guid"].parse::<u64>().unwrap())
        .max()
        .unwrap();
    let restored = buyback(&mut c.connection, vendor, 69);
    assert_eq!(values(&restored, first)[&25], 323);
    assert_eq!(values(&restored, first)[&26], 60_000);
    assert_eq!(values(&restored, first)[&27], 40);
    assert_eq!(
        c.destination.query_rows(&format!(
            "SELECT charges FROM game_temporary_enchantment WHERE item_guid = {first}"
        ))[0]["charges"],
        "40"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn repair_after_handoff_keeps_the_current_world_session() {
    let mut c = crossing("buyback-handoff-repair");
    c.connection
        .call("debug_repair_after_publish", json!([]))
        .unwrap();
    assert_eq!(
        c.destination
            .query_rows("SELECT * FROM game_native_player")
            .len(),
        1
    );
    assert_eq!(
        c.destination.query_rows("SELECT * FROM game_buyback_state"),
        c.state
    );
    assert!(c.connection.drain_packets().iter().all(|row| {
        let Packet(_, _, _, _, close, _, _) = serde_json::from_str(row.get()).unwrap();
        !close
    }));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn sale_after_handoff_uses_the_original_login_time_and_current_slot() {
    let mut c = crossing("buyback-handoff-timestamp");
    c.destination.assert_call(
        "debug_spawn_at_feet",
        &[
            &c.guid.to_string(),
            &SCENARIO_VENDOR_ENTRY.to_string(),
            "2.0",
        ],
    );
    let vendor = c
        .destination
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {SCENARIO_VENDOR_ENTRY}"
        ))
        .iter()
        .map(|row| row["guid"].parse::<u64>().unwrap())
        .max()
        .unwrap();
    let item = grant(&c.destination, c.guid, FIXTURE_BLADE, 1);
    let fields = values(&sell(&mut c.connection, vendor, item, 0), c.guid);
    let row = sold_rows(&c.destination, c.guid)
        .into_iter()
        .find(|row| row["item_guid"] == item.to_string())
        .unwrap();
    assert_eq!(row["timestamp"], "111605");
    assert_eq!(row["slot"], "72");
    assert_eq!(fields.get(&1241), Some(&111605));
    assert_eq!(fields.get(&630), Some(&(item as u32)));
}

fn refuse_conflicting_buyback(location: &str) {
    let (source, guid, vendor) = sessionless(&format!("buyback-guid-source-{location}"));
    let item = grant(&source, guid, FIXTURE_BLADE, 1);
    source.assert_call(
        "debug_sell_item",
        &[&guid.to_string(), &vendor.to_string(), "23"],
    );
    let source_items = sold_rows(&source, guid);
    let (destination, _, _) = sessionless(&format!("buyback-guid-target-{location}"));
    if location == "payload" {
        let duplicate = grant(&source, guid, FIXTURE_JERKY, 1);
        source.assert_sql(&format!(
            "UPDATE game_item_instance SET guid = {item} WHERE guid = {duplicate}"
        ));
    } else {
        destination.assert_call(
            "create_character",
            &["1", "\"Other\"", "1", "1", "0", "0", "0", "0", "0", "0"],
        );
        let other = destination.query_rows("SELECT guid FROM game_character WHERE name = 'Other'")
            [0]["guid"]
            .parse::<u64>()
            .unwrap();
        destination.assert_call("debug_spawn_player_entity", &[&other.to_string()]);
        let duplicate = grant(&destination, other, FIXTURE_BLADE, 1);
        destination.assert_sql(&format!(
            "UPDATE game_item_instance SET guid = {item} WHERE guid = {duplicate}"
        ));
        if location == "buyback" {
            destination.assert_call(
                "debug_spawn_at_feet",
                &[
                    &other.to_string(),
                    &SCENARIO_VENDOR_ENTRY.to_string(),
                    "2.0",
                ],
            );
            let vendor = destination
                .query_rows(&format!(
                    "SELECT guid FROM game_world_entity WHERE entry = {SCENARIO_VENDOR_ENTRY}"
                ))
                .iter()
                .map(|row| row["guid"].parse::<u64>().unwrap())
                .max()
                .unwrap();
            destination.assert_call(
                "debug_sell_item",
                &[&other.to_string(), &vendor.to_string(), "23"],
            );
        }
    }
    let blob = freeze(&source, guid, 5090081);
    let inventory = destination.query_rows("SELECT * FROM game_item_instance");
    let buybacks = destination.query_rows("SELECT * FROM game_buyback_item");
    destination.assert_sql(&format!(
        "DELETE FROM game_world_entity WHERE guid = {guid}"
    ));
    let result = destination.call(
        "import_character_blob",
        &["5090081", &blob, r#"{"guid":0,"ownership":null}"#],
    );
    assert!(
        !result.status.success(),
        "incoming buyback GUID overwrote {location}"
    );
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("ITEM_GUID_CONFLICT"),
        "{result:?}"
    );
    assert_eq!(
        destination.query_rows("SELECT * FROM game_item_instance"),
        inventory
    );
    assert_eq!(
        destination.query_rows("SELECT * FROM game_buyback_item"),
        buybacks
    );
    assert!(destination
        .query_rows("SELECT * FROM game_transfer_in")
        .is_empty());
    assert_eq!(sold_rows(&source, guid), source_items);
    assert!(!source
        .call(
            "finish_transfer",
            &["5090081", r#"{"guid":0,"ownership":null}"#]
        )
        .status
        .success());
    assert_eq!(sold_rows(&source, guid), source_items);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn incoming_buyback_guid_conflicts_with_inventory() {
    refuse_conflicting_buyback("inventory");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn incoming_buyback_guid_conflicts_with_buyback() {
    refuse_conflicting_buyback("buyback");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn incoming_buyback_guid_conflicts_with_payload() {
    refuse_conflicting_buyback("payload");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn transported_buyback_text_is_remapped_and_each_reference_is_released_once() {
    let (source, mut source_connection, guid, vendor) = stage("buyback-text-source");
    let item = grant(&source, guid, FIXTURE_BLADE, 1);
    source.assert_call(
        "create_character",
        &["1", "\"Other\"", "1", "1", "0", "0", "0", "0", "0", "0"],
    );
    let other = source.query_rows("SELECT guid FROM game_character WHERE name = 'Other'")[0]
        ["guid"]
        .parse::<u64>()
        .unwrap();
    source.assert_call("debug_spawn_player_entity", &[&other.to_string()]);
    let kept = grant(&source, other, FIXTURE_BLADE, 1);
    source.insert_row(
        "game_item_text",
        &[
            ("id", &5090001),
            ("text", &"A note in buyback"),
            ("letters", &2),
        ],
    );
    for guid in [item, kept] {
        source.assert_sql(&format!(
            "UPDATE game_item_instance SET item_text_id = 5090001 WHERE guid = {guid}"
        ));
    }
    sell(&mut source_connection, vendor, item, 0);
    let blob = freeze(&source, guid, 5090082);
    let (destination, _, _) = sessionless("buyback-text-target");
    destination.assert_sql(&format!(
        "DELETE FROM game_world_entity WHERE guid = {guid}"
    ));
    destination.assert_call(
        "import_character_blob",
        &["5090082", &blob, r#"{"guid":0,"ownership":null}"#],
    );
    assert_eq!(sold_rows(&destination, guid).len(), 1);
    source.assert_call(
        "confirm_import",
        &["5090082", r#"{"guid":0,"ownership":null}"#],
    );
    source.assert_call(
        "finish_transfer",
        &["5090082", r#"{"guid":0,"ownership":null}"#],
    );
    source_connection
        .call("debug_repair_after_publish", json!([]))
        .unwrap();
    assert!(
        delivered(&mut source_connection)
            .iter()
            .all(|(opcode, body)| *opcode != 0xaa || body != &item.to_le_bytes()),
        "transported buyback Item must not be destroyed on the source client"
    );
    assert_eq!(
        source.query_rows("SELECT letters FROM game_item_text WHERE id = 5090001")[0]["letters"],
        "1"
    );
    let (mut connection, _) = adopt(&destination, guid, "5090082", &blob, false);
    destination.assert_call(
        "debug_spawn_at_feet",
        &[&guid.to_string(), &SCENARIO_VENDOR_ENTRY.to_string(), "2.0"],
    );
    let vendor = destination
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {SCENARIO_VENDOR_ENTRY}"
        ))
        .iter()
        .map(|row| row["guid"].parse::<u64>().unwrap())
        .max()
        .unwrap();
    let returned = buyback(&mut connection, vendor, 69);
    assert!(
        returned
            .iter()
            .all(|(opcode, _)| !matches!(opcode, 0x1a1 | 0x1a5 | 0x112)),
        "{returned:?}"
    );
    let restored = destination
        .query_rows(&format!(
            "SELECT item_text_id,slot FROM game_item_instance WHERE guid = {item}"
        ))
        .remove(0);
    let text = &restored["item_text_id"];
    assert_ne!(text, "5090001");
    assert_eq!(
        destination.query_rows(&format!(
            "SELECT text FROM game_item_text WHERE id = {text}"
        ))[0]["text"],
        "A note in buyback"
    );
    destination.assert_call(
        "gw_destroy_item",
        &[
            &format!(r#"{{"guid":{guid},"ownership":null}}"#),
            &restored["slot"],
            "0",
        ],
    );
    assert!(destination
        .query_rows(&format!("SELECT * FROM game_item_text WHERE id = {text}"))
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn world_port_without_buyback_session_state_refuses_import_atomically() {
    let (source, guid, _) = sessionless("buyback-state-required-source");
    grant(&source, guid, FIXTURE_JERKY, 3);
    let inventory = source.query_rows("SELECT * FROM game_item_instance");
    let blob = freeze(&source, guid, 5090083);
    let (destination, _, _) = sessionless("buyback-state-required-target");
    destination.assert_call("debug_delete_character", &[&guid.to_string()]);
    let (_connection, result) = receive_handoff(&destination, guid, "5090083", &blob);
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("buyback World Session state"),
        "{result:?}"
    );
    assert!(destination
        .query_rows(&format!("SELECT * FROM game_character WHERE guid = {guid}"))
        .is_empty());
    assert!(destination
        .query_rows("SELECT * FROM game_transfer_in")
        .is_empty());
    assert!(destination
        .query_rows("SELECT * FROM game_native_handoff")
        .is_empty());
    assert_eq!(
        source.query_rows("SELECT * FROM game_item_instance"),
        inventory
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn handoff_reconciliation_keeps_a_petition_whose_charter_is_in_buyback() {
    let (source, mut connection, guid, vendor) = stage("buyback-charter-handoff-source");
    source.assert_sql(&format!(
        "UPDATE game_item_template SET entry = 5863 WHERE entry = {FIXTURE_BLADE}"
    ));
    let charter = grant(&source, guid, 5863, 1);
    sell(&mut connection, vendor, charter, 0);
    let blob = freeze(&source, guid, 5090084);
    let (destination, _, _) = sessionless("buyback-charter-handoff-target");
    destination.assert_sql(&format!(
        "DELETE FROM game_world_entity WHERE guid = {guid}"
    ));
    destination.assert_call(
        "import_character_blob",
        &["5090084", &blob, r#"{"guid":0,"ownership":null}"#],
    );
    source.assert_call(
        "confirm_import",
        &["5090084", r#"{"guid":0,"ownership":null}"#],
    );
    source.assert_call(
        "finish_transfer",
        &["5090084", r#"{"guid":0,"ownership":null}"#],
    );
    let (_connection, _) = adopt(&destination, guid, "5090084", &blob, true);
    let transaction = destination.query_rows(&format!(
        "SELECT operation FROM game_guild_transaction WHERE guid = {guid}"
    ));
    assert_eq!(transaction.len(), 1);
    assert!(
        transaction[0]["operation"].contains(&charter.to_string()),
        "Realm reconciliation must retain the sold Charter's Petition: {transaction:?}"
    );
}
