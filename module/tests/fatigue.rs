mod support;

use lyracore_shared::constants::player_flags;
use lyracore_shared::terrain::cell_index;
use support::{actor, number, poll_until, Standalone, POLL_TIMEOUT};

const PLAYER: &str = "1";
const MAP_ID: u32 = 0;
const HEARTBEAT: u32 = lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT;

const DEEP_WATER: (f32, f32, f32) = (-8900.0, -132.0, -1.0);
const SAFE_WATER: (f32, f32, f32) = (-9000.0, -132.0, -1.0);

const FATIGUE_DRAIN: u8 = 3;
const FATIGUE_STOP: u8 = 5;
const EXHAUSTION_DAMAGE: u8 = 6;

fn fixture(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    // The reaper would delete the relay events under test.
    shard.assert_sql("DELETE FROM game_event_reaper_schedule");
    let heights = vec!["-50"; 145].join(":");
    let packed = [(DEEP_WATER, u64::MAX), (SAFE_WATER, 0)]
        .iter()
        .map(|((x, y, _), deep_water_tiles)| {
            let (cell_x, cell_y) = (cell_index(*x).unwrap(), cell_index(*y).unwrap());
            format!("{MAP_ID},{cell_x},{cell_y},0,1,0,0,{heights},{deep_water_tiles}")
        })
        .collect::<Vec<_>>()
        .join(";");
    shard.assert_call("import_terrain_chunks", &[&packed]);
    shard.assert_call("debug_spawn_player_entity", &[PLAYER]);
    shard
}

fn move_to(shard: &Standalone, (x, y, z): (f32, f32, f32), move_time_ms: u32) {
    shard.assert_call(
        "gw_movement_update",
        &[
            &actor(PLAYER),
            &HEARTBEAT.to_string(),
            "[]",
            &x.to_string(),
            &y.to_string(),
            &z.to_string(),
            "0.0",
            &move_time_ms.to_string(),
        ],
    );
}

fn fatigue_rows(shard: &Standalone) -> Vec<support::Row> {
    shard.query_rows(&format!(
        "SELECT * FROM game_fatigue_state WHERE entity_guid = {PLAYER}"
    ))
}

fn relayed(shard: &Standalone, kind: u8) -> bool {
    !shard
        .query_rows(&format!(
            "SELECT id FROM game_breath_relay_event WHERE character_guid = {PLAYER} AND kind = {kind}"
        ))
        .is_empty()
}

fn exhaust_now(shard: &Standalone) {
    shard.assert_sql(&format!(
        "UPDATE game_fatigue_state SET remaining_micros = 0 WHERE entity_guid = {PLAYER}"
    ));
}

fn entity(shard: &Standalone) -> support::Row {
    shard
        .query_rows(&format!(
            "SELECT * FROM game_world_entity WHERE guid = {PLAYER}"
        ))
        .remove(0)
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn deep_water_exhausts_a_motionless_character_and_safe_water_recovers_it() {
    let shard = fixture("fatigue-exhaustion");
    move_to(&shard, SAFE_WATER, 1000);
    assert!(
        fatigue_rows(&shard).is_empty(),
        "safe water starts no timer"
    );

    move_to(&shard, DEEP_WATER, 2000);
    assert_eq!(fatigue_rows(&shard)[0]["in_deep_water"], "true");
    assert!(relayed(&shard, FATIGUE_DRAIN));

    let max_health: u32 = number(&entity(&shard), "max_health");
    exhaust_now(&shard);
    assert!(
        poll_until(POLL_TIMEOUT, || number::<u32>(&entity(&shard), "health")
            < max_health),
        "the tick exhausts the Character without movement"
    );
    assert!(relayed(&shard, EXHAUSTION_DAMAGE));

    move_to(&shard, SAFE_WATER, 3000);
    assert!(
        poll_until(POLL_TIMEOUT, || fatigue_rows(&shard).is_empty()),
        "safe water recovers the timer"
    );
    assert!(relayed(&shard, FATIGUE_STOP));
    assert_eq!(entity(&shard)["dead"], "false");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_exhausted_ghost_returns_to_a_graveyard() {
    let shard = fixture("fatigue-ghost");
    shard.assert_call("debug_set_health", &[PLAYER, "0"]);
    shard.assert_call("debug_repop", &[PLAYER]);
    assert_ne!(
        number::<u32>(&entity(&shard), "player_flags") & player_flags::GHOST,
        0
    );

    move_to(&shard, DEEP_WATER, 1000);
    assert_eq!(fatigue_rows(&shard).len(), 1, "a ghost feels fatigue");
    exhaust_now(&shard);

    assert!(
        poll_until(POLL_TIMEOUT, || fatigue_rows(&shard).is_empty()),
        "exhaustion ends the ghost's timer"
    );
    let ghost = entity(&shard);
    assert_ne!(
        number::<f32>(&ghost, "x"),
        DEEP_WATER.0,
        "moved to a graveyard"
    );
    assert_eq!(ghost["dead"], "true");
}
