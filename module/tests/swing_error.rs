mod support;

use support::{event_rows, number, poll_until, Row, Standalone, TableSubscription, POLL_TIMEOUT};

const PLAYER: u64 = 1;
const TEST_WOLF_ENTRY: u32 = 51000;
const NONE: u8 = 0;
const NOT_IN_RANGE: u8 = 1;
const BAD_FACING: u8 = 2;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_due_swing_records_out_of_range_and_bad_facing_and_a_landed_swing_clears_it() {
    let mut standalone = Standalone::start("swing-error");
    standalone.publish_module();
    standalone.assert_call("debug_spawn_player_entity", &[&PLAYER.to_string()]);
    let wolf = spawn_wolf(&standalone, 20.0);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = true WHERE guid = {PLAYER} OR guid = {wolf}"
    ));
    let player = entity(&standalone, PLAYER);
    let (x, y): (f32, f32) = (number(&player, "x"), number(&player, "y"));
    // Face +x, toward the wolf `debug_spawn_at_feet` placed ahead on x.
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET orientation = 0.0 WHERE guid = {PLAYER}"
    ));

    let events = standalone.subscribe(&[
        "SELECT * FROM game_combat_event_live WHERE attacker_guid = 1",
        "SELECT * FROM game_melee_attack WHERE attacker_guid = 1",
    ]);
    standalone.assert_call("debug_engage", &[&PLAYER.to_string(), &wolf.to_string()]);
    wait_for_swing_error(&events, NOT_IN_RANGE);
    assert!(
        own_swings(&events).is_empty(),
        "an out-of-range swing must not land"
    );

    place(&standalone, wolf, x + 1.0, y);
    make_swing_due(&standalone);
    assert!(
        poll_until(POLL_TIMEOUT, || !own_swings(&events).is_empty()),
        "the swing in reach never landed"
    );
    wait_for_swing_error(&events, NONE);

    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET orientation = 3.14159 WHERE guid = {PLAYER}"
    ));
    drop(events);
    let events = standalone.subscribe(&[
        "SELECT * FROM game_combat_event_live WHERE attacker_guid = 1",
        "SELECT * FROM game_melee_attack WHERE attacker_guid = 1",
    ]);
    // Restart after subscribing so an earlier tick cannot consume the error transition.
    standalone.assert_call("debug_engage", &[&PLAYER.to_string(), &wolf.to_string()]);
    wait_for_swing_error(&events, BAD_FACING);
    assert!(
        own_swings(&events).is_empty(),
        "a swing at a target behind the player must not land"
    );
}

fn spawn_wolf(standalone: &Standalone, offset: f32) -> u64 {
    standalone.assert_call(
        "debug_spawn_at_feet",
        &[
            &PLAYER.to_string(),
            &TEST_WOLF_ENTRY.to_string(),
            &offset.to_string(),
        ],
    );
    standalone
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
        ))
        .iter()
        .map(|row| number::<u64>(row, "guid"))
        .max()
        .expect("the spawned wolf must exist")
}

fn entity(standalone: &Standalone, guid: u64) -> Row {
    standalone
        .query_rows(&format!(
            "SELECT x, y FROM game_world_entity WHERE guid = {guid}"
        ))
        .pop()
        .expect("the entity must be live")
}

fn place(standalone: &Standalone, guid: u64, x: f32, y: f32) {
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET x = {x}, y = {y} WHERE guid = {guid}"
    ));
}

/// Stamping the swing clock at 1 makes the next swing tick due at once.
fn make_swing_due(standalone: &Standalone) {
    standalone.assert_sql(&format!(
        "UPDATE game_melee_attack SET last_swing_ms = 1 WHERE attacker_guid = {PLAYER}"
    ));
}

fn swing_error(events: &TableSubscription) -> Option<u8> {
    event_rows(events, "game_melee_attack")
        .pop()
        .map(|row| number(&row, "swing_error"))
}

fn wait_for_swing_error(events: &TableSubscription, expected: u8) {
    assert!(
        poll_until(POLL_TIMEOUT, || swing_error(events) == Some(expected)),
        "the Swing Error must become {expected}, got {:?}",
        swing_error(events)
    );
}

fn own_swings(events: &TableSubscription) -> Vec<Row> {
    event_rows(events, "game_combat_event_live")
}
