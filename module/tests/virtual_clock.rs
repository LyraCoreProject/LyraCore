use std::time::{SystemTime, UNIX_EPOCH};

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_test_support::{ReducerConnection, Standalone};
use serde_json::json;

fn fixture(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard
}

fn start_clock(shard: &Standalone) {
    let start = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros();
    shard.assert_call("debug_start_virtual_clock", &[&start.to_string()]);
}

fn advance(connection: &mut ReducerConnection, ticks: usize) {
    for _ in 0..ticks {
        connection
            .call("debug_advance_virtual_clock", json!([50_000]))
            .unwrap();
    }
}

fn rows(shard: &Standalone, query: &str) -> usize {
    shard.query_rows(query).len()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_five_second_aura_expires_at_the_same_virtual_time_on_repeated_casts() {
    let shard = fixture("virtual-aura-expiry");
    shard.assert_sql("UPDATE game_spell SET duration_ms = 5000 WHERE spell_id = 168");
    start_clock(&shard);
    let mut connection = shard.connect_reducers(&[]);
    let aura = "SELECT id FROM game_aura WHERE target_guid = 1 AND spell_id = 168";
    for _ in 0..2 {
        shard.assert_call("debug_force_cast_at", &["1", "168", "1"]);
        assert!(rows(&shard, aura) > 0);
        advance(&mut connection, 99);
        assert!(rows(&shard, aura) > 0);
        advance(&mut connection, 1);
        assert_eq!(rows(&shard, aura), 0);
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_pending_cast_resolves_at_its_virtual_due_time() {
    let shard = fixture("virtual-pending-cast");
    shard.assert_sql("UPDATE game_spell SET cast_time_ms = 1000 WHERE spell_id = 168");
    start_clock(&shard);
    shard.assert_call("debug_begin_cast", &["1", "168", "1"]);
    let pending = "SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = 1";
    let aura = "SELECT id FROM game_aura WHERE target_guid = 1 AND spell_id = 168";
    assert_eq!(rows(&shard, pending), 1);
    let mut connection = shard.connect_reducers(&[]);
    advance(&mut connection, 19);
    assert_eq!(rows(&shard, pending), 1);
    assert_eq!(rows(&shard, aura), 0);
    advance(&mut connection, 1);
    assert_eq!(rows(&shard, pending), 0);
    assert!(rows(&shard, aura) > 0);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_spell_projectile_impacts_at_its_virtual_due_time() {
    let shard = fixture("virtual-spell-impact");
    shard.assert_call(
        "debug_spawn_at_feet",
        &["1", &TEST_WOLF_ENTRY.to_string(), "2"],
    );
    let target = shard.query_rows(&format!(
        "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
    ))[0]["guid"]
        .clone();
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET health = 100, max_health = 100 WHERE guid = {target}"
    ));
    start_clock(&shard);
    shard.assert_call(
        "debug_stage_virtual_spell_impact",
        &["1", &target, "5", "500000"],
    );
    let health = || {
        shard.query_rows(&format!(
            "SELECT health FROM game_world_entity WHERE guid = {target}"
        ))[0]["health"]
            .clone()
    };
    let mut connection = shard.connect_reducers(&[]);
    advance(&mut connection, 9);
    assert_eq!(health(), "100");
    assert_eq!(
        rows(&shard, "SELECT scheduled_id FROM game_pending_spell_impact"),
        1
    );
    advance(&mut connection, 1);
    assert!(health().parse::<u32>().unwrap() < 100);
    assert_eq!(
        rows(&shard, "SELECT scheduled_id FROM game_pending_spell_impact"),
        0
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_completed_spell_impact_releases_its_arming_row() {
    let shard = fixture("virtual-spell-impact-arming");
    start_clock(&shard);
    let now_micros = shard.query_rows("SELECT now_micros FROM game_conformance_clock")[0]
        ["now_micros"]
        .parse::<i64>()
        .unwrap();
    shard.assert_call(
        "debug_stage_virtual_spell_impact",
        &["1", "1", "5", "10000"],
    );
    let impact_arming = shard.query_rows(&format!(
        "SELECT parked_micros FROM game_conformance_arming WHERE due_micros = {}",
        now_micros + 10_000
    ));
    assert_eq!(impact_arming.len(), 1);
    let parked_micros = &impact_arming[0]["parked_micros"];
    let mut connection = shard.connect_reducers(&[]);
    advance(&mut connection, 1);
    assert_eq!(
        rows(&shard, "SELECT scheduled_id FROM game_pending_spell_impact"),
        0
    );
    assert_eq!(
        rows(
            &shard,
            &format!(
                "SELECT parked_micros FROM game_conformance_arming WHERE parked_micros = {parked_micros}"
            )
        ),
        0
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_taxi_started_off_phase_waits_for_its_own_first_tick() {
    let shard = fixture("virtual-taxi-phase");
    start_clock(&shard);
    let mut connection = shard.connect_reducers(&[]);
    advance(&mut connection, 1);
    shard.insert_rows(
        "game_taxi_node",
        &[
            &[
                ("id", &5090990),
                ("client_node_id", &100),
                ("name", &"Start"),
                ("mount_display_alliance", &1147),
            ],
            &[
                ("id", &5090991),
                ("client_node_id", &101),
                ("x", &200),
                ("name", &"End"),
                ("mount_display_alliance", &1147),
            ],
        ],
    );
    shard.insert_row(
        "game_taxi_path",
        &[
            ("id", &5090992),
            ("source_node_id", &5090990),
            ("destination_node_id", &5090991),
        ],
    );
    shard.insert_rows(
        "game_taxi_path_node",
        &[
            &[("id", &5090993), ("path_id", &5090992)],
            &[
                ("id", &5090994),
                ("path_id", &5090992),
                ("node_index", &1),
                ("x", &200),
            ],
        ],
    );
    shard.insert_row(
        "game_active_taxi_flight",
        &[
            ("character_guid", &1),
            ("path_id", &5090992),
            ("source_node_id", &5090990),
            ("destination_node_id", &5090991),
            ("mount_display_id", &1147),
        ],
    );
    shard.assert_call("gw_arm_taxi_flight", &[r#"{"guid":1,"ownership":null}"#]);
    let position = || {
        shard.query_rows("SELECT x FROM game_world_entity WHERE guid = 1")[0]["x"]
            .parse::<f32>()
            .unwrap()
    };
    let start = position();
    advance(&mut connection, 1);
    assert_eq!(position(), start);
    advance(&mut connection, 1);
    assert!(position() > start);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_missing_near_teleport_ack_expires_after_twenty_virtual_seconds() {
    let shard = fixture("virtual-near-teleport-expiry");
    let mut connection = shard.connect_reducers(&[]);
    connection.call("native_world_open", json!([42])).unwrap();
    connection
        .call("debug_native_enter_world", json!([1]))
        .unwrap();
    start_clock(&shard);
    shard.assert_call("debug_teleport", &["1", "0", "20", "0", "0", "0"]);
    let teleport =
        || shard.query_rows("SELECT teleport FROM game_native_player")[0]["teleport"].clone();
    advance(&mut connection, 399);
    connection
        .call("native_world_receive", json!([0x1ceu32, Vec::<u8>::new()]))
        .unwrap();
    assert_ne!(teleport(), "(none = ())");
    advance(&mut connection, 1);
    connection
        .call("native_world_receive", json!([0x1ceu32, Vec::<u8>::new()]))
        .unwrap();
    assert_eq!(teleport(), "(none = ())");
}
