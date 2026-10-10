//! A creature's leg advance reaches its stored row only past 4 yd, on a cell change, or on arrival.
//! `CtxWorld::settle_advances` puts the stored row back after each firing, which no in-memory
//! scenario can observe, so this drives real `tick_creatures` firings and reads the row back.

mod support;

use std::time::{Duration, Instant};

use support::{
    distance, leg, leg_destination, lone_wolf, number, position, Row, Standalone, LEG_YD,
};

/// The drift a stored creature row may lag behind its leg, as `world::PERSIST_MAX_DRIFT_YD`.
const PERSIST_MAX_DRIFT_YD: f32 = 4.0;

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
    let wolf = lone_wolf(&mut shard);
    let guid = wolf["guid"].clone();
    let (dest_x, y, z) = leg_destination(&wolf);

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
    let dur_ms: u32 = number(&leg(&shard, &guid), "dur_ms");
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
        (number::<f32>(arrival, "x") - dest_x).abs() < 0.01
            && (number::<f32>(arrival, "y") - y).abs() < 0.01,
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
        let step = distance(position(&pair[0]), position(&pair[1]));
        assert!(
            step > PERSIST_MAX_DRIFT_YD,
            "a mid-leg write moved the stored row only {step} yd: {:?} -> {:?}",
            pair[0],
            pair[1]
        );
    }
    let between = walk
        .iter()
        .filter(|row| (number::<f32>(row, "x") - dest_x).abs() >= 0.01)
        .count();
    assert!(
        between >= 2,
        "the walk must persist positions short of the destination: {stored:?}"
    );
}
