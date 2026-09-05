mod support;

use std::thread;
use std::time::{Duration, Instant};

use support::Standalone;

const CHARACTER_GUID: u64 = 1;
const HEARTBEAT: u32 = lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT;

/// Runs only when requested because it builds and publishes the Wasm module to its own standalone.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn teleport_motion_rows_follow_the_live_entity_boundary() {
    let standalone = Standalone::start("teleport-motion");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("debug_spawn_player_entity", &[&CHARACTER_GUID.to_string()]);

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

    // Stop the isolated node's publisher after it has produced the public row. The next movement
    // therefore leaves a known pending row for the teleport boundary to clean up as well.
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
        "a cross-map teleport must not leave public motion on the source Shard"
    );
    assert_eq!(
        row_count(&standalone, "game_entity_motion_pending"),
        0,
        "a cross-map teleport must not leave staged motion on the source Shard"
    );
}

fn move_to(standalone: &Standalone, x: f32, y: f32, move_time_ms: u32) {
    let args = [
        CHARACTER_GUID.to_string(),
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
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if row_count(standalone, "game_entity_motion") == 1 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "scheduled publisher did not create the public motion row"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn row_count(standalone: &Standalone, table: &str) -> usize {
    standalone
        .query_rows(&format!(
            "SELECT * FROM {table} WHERE guid = {CHARACTER_GUID}"
        ))
        .len()
}
