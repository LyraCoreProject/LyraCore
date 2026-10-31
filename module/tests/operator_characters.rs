mod support;

use lyracore_shared::terrain::cell_index;
use support::{number, position, Standalone};

fn realm(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_start_virtual_clock", &["1700000000000000"]);
    for map in [0, 1, 36, 489] {
        shard.assert_sql(&format!(
            "INSERT INTO game_map (id, internal_name, name, map_type, linked_zone, parent, player_limit, reset_delay, ghost_entrance_map, ghost_entrance_x, ghost_entrance_y) \
             VALUES ({map}, 'Test', 'Test', {}, 0, 0, 0, 0, -1, 0, 0)",
            if map < 2 { 0 } else { 1 }
        ));
        let cell = cell_index(100.0).unwrap();
        let terrain = format!(
            "{map},{cell},{cell},0,0,0,{},{}",
            5000 + map,
            ["10"; 145].join(":")
        );
        shard.assert_call(
            "import_terrain_chunks_append",
            &[&serde_json::to_string(&terrain).unwrap()],
        );
        shard.assert_sql(&format!(
            "INSERT INTO game_area (id, map_id, parent_area_id, area_bit, flags, exploration_level, faction_group, name) \
             VALUES ({}, {map}, {}, 0, 0, 0, 0, 'Destination')",
            5000 + map, 6000 + map
        ));
    }
    shard
}

fn teleport(shard: &Standalone, map: &str) {
    shard.assert_call(
        "operator_teleport_character",
        &["\"tEsTeR\"", map, "100", "100", "10.5", "1.25"],
    );
}

fn destination(shard: &Standalone, table: &str, map: u32) {
    let rows = shard.query_rows(&format!("SELECT * FROM {table} WHERE guid = 1"));
    assert_eq!(rows.len(), 1);
    assert_eq!(number::<u32>(&rows[0], "map_id"), map);
    assert_eq!(number::<u32>(&rows[0], "zone_id"), 6000 + map);
    assert_eq!(position(&rows[0]), (100.0, 100.0, 10.5));
    assert_eq!(number::<f32>(&rows[0], "orientation"), 1.25);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn offline_teleport_saves_the_destination_for_the_next_login() {
    let shard = realm("operator-character-offline");
    shard.assert_sql("UPDATE game_character SET pending_instance_id = 42 WHERE guid = 1");
    teleport(&shard, "1");
    destination(&shard, "game_character", 1);
    assert_eq!(
        shard.query_rows("SELECT pending_instance_id FROM game_character WHERE guid = 1")[0]
            ["pending_instance_id"],
        "0"
    );
    assert!(shard
        .query_rows("SELECT * FROM game_world_entity WHERE guid = 1")
        .is_empty());
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    destination(&shard, "game_world_entity", 1);
}

fn snapshot(shard: &Standalone) -> Vec<Vec<support::Row>> {
    [
        "game_character",
        "game_world_entity",
        "game_active_taxi_flight",
        "game_transport_passenger",
        "game_native_player",
        "game_teleport_event",
    ]
    .map(|table| shard.query_rows(&format!("SELECT * FROM {table}")))
    .to_vec()
}

fn refused(shard: &Standalone, args: &[&str], message: &str) {
    let before = snapshot(shard);
    let output = shard.call("operator_teleport_character", args);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success(), "{text}");
    assert!(text.contains(message), "expected {message:?}: {text}");
    assert_eq!(snapshot(shard), before);
    let logs = shard.module_logs(100);
    assert!(!String::from_utf8_lossy(&logs.stdout).contains("teleport character"));
}

const ARGS: &[&str] = &["\"Tester\"", "0", "100", "100", "10.5", "1.25"];

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn online_same_map_and_far_teleports_use_the_world_path() {
    let shard = realm("operator-character-online");
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_sql("UPDATE game_character SET online = true WHERE guid = 1");
    teleport(&shard, "0");
    destination(&shard, "game_world_entity", 0);
    destination(&shard, "game_character", 0);
    let events = shard.query_rows("SELECT cross_map FROM game_teleport_event WHERE mover_guid = 1");
    assert_eq!(events[0]["cross_map"], "false");
    teleport(&shard, "1");
    destination(&shard, "game_character", 1);
    assert!(shard
        .query_rows("SELECT * FROM game_world_entity WHERE guid = 1")
        .is_empty());
    let events = shard.query_rows("SELECT cross_map FROM game_teleport_event WHERE mover_guid = 1");
    assert!(events.iter().any(|row| row["cross_map"] == "true"));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn non_operator_and_unclaimed_operator_cannot_teleport() {
    let shard = realm("operator-character-authority");
    let before = snapshot(&shard);
    let output = shard.call_anonymous("operator_teleport_character", ARGS);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("operator only"));
    assert_eq!(snapshot(&shard), before);
    shard.assert_sql("DELETE FROM game_operator");
    refused(&shard, ARGS, "operator not claimed");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn unknown_names_instance_maps_unloaded_maps_and_outside_coordinates_refuse() {
    let shard = realm("operator-character-destination-refusal");
    for (index, value, message) in [
        (0, "\"Nobody\"", "does not exist or is in Transfer"),
        (1, "36", "map 36 requires an Instance"),
        (1, "489", "map 489 requires an Instance"),
        (1, "9999", "map 9999 is not in the Realm's map catalogue"),
        (2, "17100", "(17100, 100, 10.5) is outside the map bounds"),
        (3, "-17100", "(100, -17100, 10.5) is outside the map bounds"),
        (4, "20000", "(100, 100, 20000) is outside the map bounds"),
    ] {
        let mut args = ARGS.to_vec();
        args[index] = value;
        refused(&shard, &args, message);
    }
    shard.assert_sql("DELETE FROM game_terrain_chunk WHERE map_id = 1");
    let mut args = ARGS.to_vec();
    args[1] = "1";
    refused(&shard, &args, "map 1 is not loaded at (100, 100)");
    shard.assert_sql("UPDATE game_map SET map_type = 0 WHERE id = 36");
    args[1] = "36";
    refused(&shard, &args, "map 36 requires an Instance");
    shard.assert_sql("UPDATE game_map SET map_type = 2 WHERE id = 0");
    refused(&shard, ARGS, "map 0 requires an Instance");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn ambiguous_case_insensitive_names_refuse_even_an_exact_spelling() {
    let shard = realm("operator-character-ambiguous");
    let account = shard.query_rows("SELECT account_id FROM game_character WHERE guid = 1")[0]
        ["account_id"]
        .clone();
    shard.assert_call(
        "create_character",
        &[
            &account,
            "\"tester\"",
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
    refused(&shard, ARGS, "is ambiguous ignoring ASCII case");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn dead_ghost_combat_taxi_and_transport_states_refuse_without_changes() {
    let shard = realm("operator-character-state-refusal");
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    for (state, reset, message) in [
        ("dead = true", "dead = false", "character is dead"),
        (
            "dead = true, player_flags = 16",
            "dead = false, player_flags = 0",
            "character is a ghost",
        ),
        (
            "combat_active = true",
            "combat_active = false",
            "character is in combat",
        ),
        (
            "combat_until_ms = 1800000000000",
            "combat_until_ms = 0",
            "character is in combat",
        ),
    ] {
        shard.assert_sql(&format!(
            "UPDATE game_world_entity SET {state} WHERE guid = 1"
        ));
        refused(&shard, ARGS, message);
        shard.assert_sql(&format!(
            "UPDATE game_world_entity SET {reset} WHERE guid = 1"
        ));
    }
    shard.assert_sql("INSERT INTO game_active_taxi_flight (character_guid,path_id,source_node_id,destination_node_id,mount_display_id,fare,current_node_index,started_micros,origin_x,origin_y,origin_z,origin_recorded) VALUES (1,1,1,2,1,0,0,1700000000000000,0,0,0,true)");
    refused(&shard, ARGS, "character is on a taxi flight");
    shard.assert_sql("DELETE FROM game_active_taxi_flight");
    shard.assert_sql("INSERT INTO game_transport_passenger (character_guid,transport_entry,x,y,z,orientation) VALUES (1,1,0,0,0,0)");
    refused(&shard, ARGS, "character is aboard a transport");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn offline_ghost_and_online_character_without_entity_refuse() {
    let shard = realm("operator-character-offline-refusal");
    shard.assert_sql("UPDATE game_character SET pending_ghost = true WHERE guid = 1");
    refused(&shard, ARGS, "character is a ghost");
    shard.assert_sql(
        "UPDATE game_character SET pending_ghost = false, online = true WHERE guid = 1",
    );
    refused(
        &shard,
        ARGS,
        "character is online without a world entity; retry after logout",
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn offline_destination_without_area_data_clears_the_old_zone() {
    let shard = realm("operator-character-no-area");
    shard.assert_sql("DELETE FROM game_area");
    teleport(&shard, "1");
    assert_eq!(
        shard.query_rows("SELECT zone_id FROM game_character WHERE guid = 1")[0]["zone_id"],
        "0"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn transfer_fenced_character_cannot_be_relocated() {
    let shard = realm("operator-character-transfer");
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call(
        "begin_transfer",
        &[
            "509675",
            &support::actor("1"),
            "0",
            "0",
            "100",
            "100",
            "10",
            "0",
            "true",
        ],
    );
    refused(&shard, ARGS, "does not exist or is in Transfer");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_live_sessionless_character_uses_the_world_path() {
    let shard = realm("operator-character-sessionless");
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    teleport(&shard, "1");
    destination(&shard, "game_character", 1);
    destination(&shard, "game_world_entity", 1);
    assert_eq!(
        shard.query_rows("SELECT cross_map FROM game_teleport_event WHERE mover_guid = 1")[0]
            ["cross_map"],
        "true"
    );
}
