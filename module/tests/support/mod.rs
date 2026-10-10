// Every test crate compiles this module and uses a different part of it.
#![allow(dead_code)]

use std::collections::BTreeMap;

use lyracore_shared::spatial::{grid_cell, GRID_CELL_SIZE, MAP_COORD_MAX};
pub use lyracore_test_support::*;

/// Long enough to span several creature firings, short enough to stay inside one 50 yd cell.
pub const LEG_YD: f32 = 20.0;

pub type Row = BTreeMap<String, String>;

pub fn number<T: std::str::FromStr>(row: &Row, column: &str) -> T {
    row[column]
        .parse()
        .unwrap_or_else(|_| panic!("{column} is not a number in {row:?}"))
}

/// Publish the module and leave one wolf (entry 51000) per offset, in yards ahead of a Character,
/// out of combat: with the Character gone no cell is awake, so only a leg advance touches a wolf's
/// row. Returns their `game_world_entity` rows in spawn order.
pub fn wolves(shard: &mut Standalone, offsets: &[f32]) -> Vec<Row> {
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_sql("DELETE FROM game_world_entity WHERE entry = 51000");
    for offset in offsets {
        shard.assert_call("debug_spawn_at_feet", &["1", "51000", &offset.to_string()]);
    }
    shard.assert_sql("DELETE FROM game_world_entity WHERE guid = 1");
    let mut wolves = shard.query_rows("SELECT * FROM game_world_entity WHERE entry = 51000");
    assert_eq!(wolves.len(), offsets.len(), "{wolves:?}");
    wolves.sort_by_key(|wolf| number::<u64>(wolf, "guid"));
    wolves
}

pub fn lone_wolf(shard: &mut Standalone) -> Row {
    wolves(shard, &[5.0]).remove(0)
}

/// The point `LEG_YD` from `at` toward the centre of its cell, so no step of the leg crosses a
/// cell edge.
pub fn leg_destination(at: &Row) -> (f32, f32, f32) {
    let (x, y, z): (f32, f32, f32) = (number(at, "x"), number(at, "y"), number(at, "z"));
    let (grid_x, _) = grid_cell(x, y);
    let centre_x = MAP_COORD_MAX - (grid_x as f32 + 0.5) * GRID_CELL_SIZE;
    let destination = (x + LEG_YD.copysign(centre_x - x), y, z);
    assert_eq!(grid_cell(destination.0, y), grid_cell(x, y));
    destination
}

/// The creature's leg row. Panics unless it has exactly one.
pub fn leg(shard: &Standalone, guid: &str) -> Row {
    let mut legs = shard.query_rows(&format!(
        "SELECT sx, sy, sz, dx, dy, dz, start_micros, dur_ms, spline_id FROM game_creature_spline \
         WHERE guid = {guid}"
    ));
    assert_eq!(legs.len(), 1, "{legs:?}");
    legs.remove(0)
}

pub fn position(row: &Row) -> (f32, f32, f32) {
    (number(row, "x"), number(row, "y"), number(row, "z"))
}

pub fn distance(a: (f32, f32, f32), b: (f32, f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)).sqrt()
}

pub fn assert_near(actual: (f32, f32, f32), expected: (f32, f32, f32), what: &str) {
    assert!(
        (actual.0 - expected.0).abs() < 0.01
            && (actual.1 - expected.1).abs() < 0.01
            && (actual.2 - expected.2).abs() < 0.01,
        "{what}: expected {expected:?}, got {actual:?}"
    );
}
