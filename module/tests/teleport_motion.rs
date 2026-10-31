mod support;

use std::time::Duration;

use support::{poll_until, Standalone};

const CHARACTER_GUID: u64 = 1;
const HEARTBEAT: u32 = lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn connected_character_motion_is_removed_until_world_port_acknowledgement() {
    let mut standalone = Standalone::start("teleport-motion");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_spawn_player_entity", &[&CHARACTER_GUID.to_string()]);
    standalone.assert_sql(&format!(
        "UPDATE game_character SET online = true WHERE guid = {CHARACTER_GUID}"
    ));

    move_to(&standalone, 100.0, 100.0, 1);
    wait_for_public_motion(&standalone);

    standalone.assert_call(
        "debug_teleport",
        &[
            &CHARACTER_GUID.to_string(),
            "0",
            "200.0",
            "200.0",
            "20.0",
            "0.0",
        ],
    );
    assert_eq!(row_count(&standalone, "game_world_entity"), 1);
    assert_eq!(
        row_count(&standalone, "game_entity_motion"),
        1,
        "a same-map teleport keeps the live entity's public motion row"
    );

    standalone.assert_sql("DELETE FROM game_motion_publish_schedule");
    move_to(&standalone, 201.0, 200.0, 2);
    assert_eq!(row_count(&standalone, "game_entity_motion_pending"), 1);

    standalone.assert_call(
        "debug_teleport",
        &[
            &CHARACTER_GUID.to_string(),
            "1",
            "300.0",
            "300.0",
            "30.0",
            "0.0",
        ],
    );
    assert_eq!(row_count(&standalone, "game_world_entity"), 0);
    assert_eq!(
        row_count(&standalone, "game_entity_motion"),
        0,
        "a map change must remove public motion until world entry"
    );
    assert_eq!(
        row_count(&standalone, "game_entity_motion_pending"),
        0,
        "a map change must discard staged motion from the previous map"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn sessionless_map_change_keeps_the_character_live_and_discards_previous_motion() {
    let mut standalone = Standalone::start("sessionless-teleport-motion");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_spawn_player_entity", &[&CHARACTER_GUID.to_string()]);

    move_to(&standalone, 100.0, 100.0, 1);
    wait_for_public_motion(&standalone);
    standalone.assert_sql("DELETE FROM game_motion_publish_schedule");
    move_to(&standalone, 101.0, 100.0, 2);
    assert_eq!(row_count(&standalone, "game_entity_motion_pending"), 1);

    standalone.assert_call(
        "debug_teleport",
        &[
            &CHARACTER_GUID.to_string(),
            "1",
            "300.0",
            "300.0",
            "30.0",
            "0.0",
        ],
    );
    for table in ["game_world_entity", "game_character"] {
        let rows = standalone.query_rows(&format!(
            "SELECT map_id, x, y, z FROM {table} WHERE guid = {CHARACTER_GUID}"
        ));
        assert_eq!(rows.len(), 1, "{table}: {rows:?}");
        assert_eq!(support::number::<u32>(&rows[0], "map_id"), 1);
        assert_eq!(support::position(&rows[0]), (300.0, 300.0, 30.0));
    }
    assert_eq!(row_count(&standalone, "game_entity_motion"), 0);
    assert_eq!(row_count(&standalone, "game_entity_motion_pending"), 0);
}

fn move_to(standalone: &Standalone, x: f32, y: f32, move_time_ms: u32) {
    let args = [
        support::actor(&CHARACTER_GUID.to_string()),
        HEARTBEAT.to_string(),
        "[]".to_string(),
        x.to_string(),
        y.to_string(),
        "20.0".to_string(),
        "0.0".to_string(),
        move_time_ms.to_string(),
    ];
    standalone.assert_call(
        "gw_movement_update",
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
    );
}

fn wait_for_public_motion(standalone: &Standalone) {
    assert!(
        poll_until(Duration::from_secs(5), || {
            row_count(standalone, "game_entity_motion") == 1
        }),
        "scheduled publisher did not create the public motion row"
    );
}

fn row_count(standalone: &Standalone, table: &str) -> usize {
    standalone
        .query_rows(&format!(
            "SELECT * FROM {table} WHERE guid = {CHARACTER_GUID}"
        ))
        .len()
}
