mod support;

use serde_json::json;
use support::{number, Standalone};

const FLIGHT_MASTER_ENTRY: u32 = 51_004;
const SOURCE_CLIENT_NODE: u32 = 253;
const DESTINATION_CLIENT_NODE: u32 = 254;
const FARE: u32 = 25;
// A flight of 200 s at 32 yd/s outlasts a slow test.
const FLIGHT_YARDS: f32 = 6400.0;
const MISSING_MASTER: u64 = 0xf130_00ff_ffff_ffff;
const ACTIVATE_TAXI: u32 = 0x1ad;
const ACTIVATE_TAXI_REPLY: u64 = 0x1ae;

fn stage(name: &str, money: u32) -> (Standalone, support::ReducerConnection, String) {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call("debug_set_money", &["1", &money.to_string()]);
    shard.assert_sql(
        "UPDATE game_world_entity SET movement_flags = 0, unit_flags = 0 WHERE guid = 1",
    );
    shard.assert_call(
        "debug_spawn_at_feet",
        &["1", &FLIGHT_MASTER_ENTRY.to_string(), "2"],
    );
    let master = shard.query_rows(&format!(
        "SELECT guid FROM game_world_entity WHERE entry = {FLIGHT_MASTER_ENTRY}"
    ))[0]["guid"]
        .clone();
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET npc_flags = 9 WHERE guid = {master}"
    ));
    let player =
        &shard.query_rows("SELECT map_id, x, y, z FROM game_world_entity WHERE guid = 1")[0];
    let (map_id, x, y, z): (u32, f32, f32, f32) = (
        number(player, "map_id"),
        number(player, "x"),
        number(player, "y"),
        number(player, "z"),
    );
    for (id, client_node, node_x) in [
        (5_090_301u32, SOURCE_CLIENT_NODE, x),
        (5_090_302, DESTINATION_CLIENT_NODE, x + FLIGHT_YARDS),
    ] {
        shard.insert_row(
            "game_taxi_node",
            &[
                ("id", &id),
                ("client_node_id", &client_node),
                ("map_id", &map_id),
                ("x", &node_x),
                ("y", &y),
                ("z", &z),
                ("name", &"Activation fixture"),
                ("mount_display_horde", &295),
                ("mount_display_alliance", &1147),
            ],
        );
    }
    shard.insert_row(
        "game_taxi_path",
        &[
            ("id", &5_090_303),
            ("source_node_id", &5_090_301),
            ("destination_node_id", &5_090_302),
            ("fare", &FARE),
        ],
    );
    for (id, index, node_x) in [(5_090_304u32, 0u32, x), (5_090_305, 1, x + FLIGHT_YARDS)] {
        shard.insert_row(
            "game_taxi_path_node",
            &[
                ("id", &id),
                ("path_id", &5_090_303),
                ("node_index", &index),
                ("map_id", &map_id),
                ("x", &node_x),
                ("y", &y),
                ("z", &z),
                ("flags", &0),
                ("delay_ms", &0),
            ],
        );
    }
    shard.insert_row(
        "game_character_taxi_node",
        &[("id", &0), ("character_guid", &1), ("node_id", &5_090_302)],
    );
    let mut wire = shard.connect_reducers(&["SELECT * FROM game_native_packet"]);
    wire.call("native_world_open", json!([42])).unwrap();
    wire.call("debug_native_enter_world", json!([1])).unwrap();
    wire.drain_packets();
    (shard, wire, master)
}

fn activate(wire: &mut support::ReducerConnection, master: &str) -> Vec<serde_json::Value> {
    activate_guid(wire, master.parse().unwrap())
}

fn activate_guid(wire: &mut support::ReducerConnection, master: u64) -> Vec<serde_json::Value> {
    let mut body = master.to_le_bytes().to_vec();
    body.extend(SOURCE_CLIENT_NODE.to_le_bytes());
    body.extend(DESTINATION_CLIENT_NODE.to_le_bytes());
    wire.call("native_world_receive", json!([ACTIVATE_TAXI, body]))
        .unwrap()
        .inserts("game_native_packet")
        .iter()
        .map(|row| serde_json::from_str(row.get()).unwrap())
        .collect()
}

fn replies(packets: &[serde_json::Value]) -> Vec<String> {
    packets
        .iter()
        .filter(|packet| packet[2][1] == ACTIVATE_TAXI_REPLY)
        .map(|packet| packet[3].as_str().unwrap().to_owned())
        .collect()
}

fn money(shard: &Standalone) -> u32 {
    number(
        &shard.query_rows("SELECT money FROM game_world_entity WHERE guid = 1")[0],
        "money",
    )
}

fn flights(shard: &Standalone) -> usize {
    shard
        .query_rows("SELECT character_guid FROM game_active_taxi_flight")
        .len()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_flying_character_gets_no_reply_and_no_second_charge() {
    let (shard, mut wire, master) = stage("taxi-activation-flying", 1000);
    assert_eq!(replies(&activate(&mut wire, &master)), ["00000000"]);
    assert_eq!((flights(&shard), money(&shard)), (1, 1000 - FARE));

    // A flight master that does not exist is answered with 5 unless the flight check comes first.
    assert!(replies(&activate_guid(&mut wire, MISSING_MASTER)).is_empty());
    assert!(replies(&activate(&mut wire, &master)).is_empty());
    assert_eq!((flights(&shard), money(&shard)), (1, 1000 - FARE));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_missing_flight_master_still_gets_its_reply_when_not_flying() {
    let (_shard, mut wire, _master) = stage("taxi-activation-missing", 1000);
    assert_eq!(
        replies(&activate_guid(&mut wire, MISSING_MASTER)),
        ["05000000"]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_request_out_of_range_of_the_flight_master_gets_no_reply() {
    let (shard, mut wire, master) = stage("taxi-activation-range", 1000);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET x = 500 WHERE guid = {master}"
    ));
    assert!(replies(&activate(&mut wire, &master)).is_empty());
    assert_eq!((flights(&shard), money(&shard)), (0, 1000));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_real_failure_still_gets_its_reply() {
    let (shard, mut wire, master) = stage("taxi-activation-fare", FARE - 1);
    assert_eq!(replies(&activate(&mut wire, &master)), ["03000000"]);
    assert_eq!((flights(&shard), money(&shard)), (0, FARE - 1));
}
