//! A scripted move keeps the mover's stored row on its leg, and the relay arrival that follows a
//! `move-dynamic` still fires when the leg lands. Both need real creature firings between the
//! move and the read.

mod support;

use std::time::Duration;

use support::{
    assert_near, distance, leg, leg_destination, lone_wolf, number, poll_until, position, wolves,
    Row, Standalone, LEG_YD, POLL_TIMEOUT,
};

fn stored(shard: &Standalone, guid: &str) -> Row {
    shard
        .query_rows(&format!(
            "SELECT * FROM game_world_entity WHERE guid = {guid}"
        ))
        .remove(0)
}

fn move_to(shard: &Standalone, guid: &str, to: (f32, f32, f32)) {
    shard.assert_call(
        "debug_encounter_move",
        &[
            guid,
            &to.0.to_string(),
            &to.1.to_string(),
            &to.2.to_string(),
            "false",
        ],
    );
}

fn equipment_rows(shard: &Standalone, guid: &str) -> usize {
    shard
        .query_rows(&format!(
            "SELECT creature_guid FROM game_encounter_equip WHERE creature_guid = {guid}"
        ))
        .len()
}

fn arrival_pending(shard: &Standalone, guid: &str) -> bool {
    !shard
        .query_rows(&format!(
            "SELECT scheduled_id FROM game_creature_ai_relay_arrival WHERE source_guid = {guid}"
        ))
        .is_empty()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_scripted_move_leaves_the_stored_row_on_the_leg_and_a_kill_stops_at_the_drawn_point() {
    let mut shard = Standalone::start("encounter-move-stored-row");
    let wolf = lone_wolf(&mut shard);
    let guid = wolf["guid"].clone();
    let start = position(&wolf);
    let destination = leg_destination(&wolf);

    move_to(&shard, &guid, destination);
    let leg = leg(&shard, &guid);
    let leg_start = (number(&leg, "sx"), number(&leg, "sy"), number(&leg, "sz"));
    assert_near(
        leg_start,
        start,
        "the leg must start at the stored position",
    );
    assert_near(
        position(&stored(&shard, &guid)),
        start,
        "the stored row right after the call",
    );

    // The walk covers 2.5 yd/s, so the wolf is about 3 yd along, however the call round trips go.
    std::thread::sleep(Duration::from_millis(900));
    let walking = position(&stored(&shard, &guid));
    assert!(
        distance(walking, destination) > LEG_YD / 2.0,
        "a reader mid-leg must not see the destination, but the wolf is stored at {walking:?}"
    );

    shard.assert_call("debug_kill_creature", &["1", &guid]);
    let corpse = position(&stored(&shard, &guid));
    let walked = distance(start, corpse);
    assert!(
        walked > 0.0 && walked < LEG_YD / 2.0,
        "the corpse must rest on the drawn point part way along the leg, not at {corpse:?}"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_move_dynamic_arrival_fires_when_the_leg_lands() {
    let mut shard = Standalone::start("encounter-move-arrival");
    let pair = wolves(&mut shard, &[5.0, 30.0]);
    let (mover, target) = (pair[0]["guid"].clone(), pair[1]["guid"].clone());
    let start = position(&pair[0]);

    shard.assert_call("debug_start_relay_move", &[&mover, &target]);
    assert_near(
        position(&stored(&shard, &mover)),
        start,
        "the stored row right after the move",
    );
    assert!(arrival_pending(&shard, &mover));
    assert_eq!(equipment_rows(&shard, &mover), 0);

    assert!(
        poll_until(POLL_TIMEOUT, || equipment_rows(&shard, &mover) == 1),
        "the arrival relay did not run after the leg landed"
    );
    assert_near(
        position(&stored(&shard, &mover)),
        position(&pair[1]),
        "the mover after its arrival relay ran",
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_move_dynamic_arrival_is_dropped_when_a_newer_leg_replaced_the_leg() {
    let mut shard = Standalone::start("encounter-move-arrival-superseded");
    let pair = wolves(&mut shard, &[5.0, 30.0]);
    let (mover, target) = (pair[0]["guid"].clone(), pair[1]["guid"].clone());
    let start = position(&pair[0]);

    shard.assert_call("debug_start_relay_move", &[&mover, &target]);
    // A walk of 20 yd outlasts the run to the target, so the newer leg is still in flight when the
    // first one lands.
    move_to(&shard, &mover, (start.0, start.1 + LEG_YD, start.2));
    assert!(arrival_pending(&shard, &mover));

    assert!(
        poll_until(POLL_TIMEOUT, || !arrival_pending(&shard, &mover)),
        "the arrival never came due"
    );
    let newer: u32 = number(&leg(&shard, &mover), "dur_ms");
    assert!(newer >= 7_000, "the newer leg must still be in flight");
    assert_eq!(
        equipment_rows(&shard, &mover),
        0,
        "the replaced leg's arrival must not run"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_move_dynamic_arrival_puts_the_mover_on_the_destination_and_reaps_the_leg() {
    let mut shard = Standalone::start("encounter-move-arrival-placement");
    let pair = wolves(&mut shard, &[5.0, 30.0]);
    let (mover, target) = (pair[0]["guid"].clone(), pair[1]["guid"].clone());

    shard.assert_call("debug_verify_relay_arrival_placement", &[&mover, &target]);
}
