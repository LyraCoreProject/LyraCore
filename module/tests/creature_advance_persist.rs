//! A creature's leg advance reaches its stored row only past 4 yd, on a cell change, or on arrival.
//! `CtxWorld::settle_advances` puts the stored row back after each firing, which no in-memory
//! scenario can observe, so this drives real `tick_creatures` firings and reads the row back.

mod support;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use lyracore_shared::spatial::{grid_cell, GRID_CELL_SIZE, MAP_COORD_MAX};
use support::Standalone;

/// The drift a stored creature row may lag behind its leg, as `world::PERSIST_MAX_DRIFT_YD`.
const PERSIST_MAX_DRIFT_YD: f32 = 4.0;
/// Long enough for several persisted steps, short enough to stay inside one 50 yd cell.
const LEG_YD: f32 = 20.0;

type Row = BTreeMap<String, String>;

fn yd(row: &Row, column: &str) -> f32 {
    row[column]
        .parse()
        .unwrap_or_else(|_| panic!("{column} is not a number in {row:?}"))
}

fn drift(from: &Row, to: &Row) -> f32 {
    ["x", "y", "z"]
        .iter()
        .map(|axis| (yd(to, axis) - yd(from, axis)).powi(2))
        .sum::<f32>()
        .sqrt()
}

/// The columns that differ between two stored rows.
fn changed_columns(from: &Row, to: &Row) -> Vec<String> {
    from.keys()
        .filter(|column| from[*column] != to[*column])
        .cloned()
        .collect()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_walking_creature_stores_its_position_past_four_yards_and_on_arrival() {
    let mut shard = Standalone::start("creature-advance-persist");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_sql("DELETE FROM game_world_entity WHERE entry = 51000");
    shard.assert_call("debug_spawn_at_feet", &["1", "51000", "5"]);
    // With no Character left, no cell is awake: only the leg advance touches the wolf's row.
    shard.assert_sql("DELETE FROM game_world_entity WHERE guid = 1");

    let wolf = shard.query_rows("SELECT * FROM game_world_entity WHERE entry = 51000");
    assert_eq!(wolf.len(), 1, "{wolf:?}");
    let wolf = &wolf[0];
    let guid = wolf["guid"].clone();
    let (x, y, z) = (yd(wolf, "x"), yd(wolf, "y"), yd(wolf, "z"));
    // Walk toward the cell centre, so no step of the leg crosses a cell edge.
    let (grid_x, _) = grid_cell(x, y);
    let centre_x = MAP_COORD_MAX - (grid_x as f32 + 0.5) * GRID_CELL_SIZE;
    let dest_x = x + LEG_YD.copysign(centre_x - x);
    assert_eq!(grid_cell(dest_x, y), grid_cell(x, y));

    shard.assert_call(
        "debug_encounter_move",
        &[
            &guid,
            &dest_x.to_string(),
            &y.to_string(),
            &z.to_string(),
            "false",
        ],
    );
    let leg = shard.query_rows(&format!(
        "SELECT dur_ms FROM game_creature_spline WHERE guid = {guid}"
    ));
    assert_eq!(leg.len(), 1, "{leg:?}");
    let dur_ms: u32 = leg[0]["dur_ms"].parse().unwrap();
    assert!(
        dur_ms >= 4_000,
        "a walk must advance under 4 yd per 0.5 s firing, and {LEG_YD} yd took {dur_ms} ms"
    );

    // Record every distinct stored row until the leg lands.
    let row_query = format!("SELECT * FROM game_world_entity WHERE guid = {guid}");
    let leg_query = format!("SELECT guid FROM game_creature_spline WHERE guid = {guid}");
    let deadline = Instant::now() + Duration::from_millis(u64::from(dur_ms)) * 3;
    let mut stored: Vec<Row> = Vec::new();
    loop {
        let in_flight = !shard.query_rows(&leg_query).is_empty();
        let row = shard.query_rows(&row_query).remove(0);
        if stored.last() != Some(&row) {
            stored.push(row);
        }
        if !in_flight {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the leg did not land: {stored:?}"
        );
    }

    let (arrival, walk) = stored.split_last().unwrap();
    assert!(
        (yd(arrival, "x") - dest_x).abs() < 0.01 && (yd(arrival, "y") - y).abs() < 0.01,
        "arrival must store the destination ({dest_x}, {y}): {arrival:?}"
    );
    for pair in stored.windows(2) {
        let moved = changed_columns(&pair[0], &pair[1]);
        assert!(
            moved
                .iter()
                .all(|column| matches!(column.as_str(), "x" | "y" | "z" | "last_move_ms")),
            "only the walk may change the stored row, but {moved:?} changed"
        );
    }
    for pair in walk.windows(2) {
        let step = drift(&pair[0], &pair[1]);
        assert!(
            step > PERSIST_MAX_DRIFT_YD,
            "a mid-leg write moved the stored row only {step} yd: {:?} -> {:?}",
            pair[0],
            pair[1]
        );
    }
    let between = walk
        .iter()
        .filter(|row| (yd(row, "x") - dest_x).abs() >= 0.01)
        .count();
    assert!(
        between >= 2,
        "the walk must persist positions short of the destination: {stored:?}"
    );
}
