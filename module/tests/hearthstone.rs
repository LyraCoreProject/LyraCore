mod support;

use spacetimedb::Timestamp;
use std::time::Duration;
use support::{actor, poll_until, Row, Standalone, POLL_TIMEOUT};

fn fixture(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call("debug_grant_item", &["1", "6948", "1"]);
    shard.assert_sql("UPDATE game_character SET home_map = 0, home_x = -9000, home_y = -100, home_z = 80, orientation = 1 WHERE guid = 1");
    shard
}

fn items(shard: &Standalone) -> Vec<Row> {
    shard.query_rows("SELECT guid, slot, stack_count FROM game_item_instance WHERE owner_guid = 1 AND entry = 6948")
}

fn position(shard: &Standalone) -> Vec<Row> {
    shard.query_rows(
        "SELECT map_id, instance_id, x, y, z, orientation FROM game_world_entity WHERE guid = 1",
    )
}

fn use_hearthstone(shard: &Standalone) {
    shard.assert_call("gw_use_item", &[&actor("1"), &items(shard)[0]["slot"]]);
}

fn timestamp(row: &Row) -> i64 {
    Timestamp::parse_from_rfc3339(&row["created_at"])
        .unwrap()
        .to_micros_since_unix_epoch()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn hearthstone_recalls_to_the_bound_home_only_after_ten_seconds() {
    let shard = fixture("hearthstone-completion");
    let before = position(&shard);
    let inventory = items(&shard);
    use_hearthstone(&shard);
    assert_eq!(position(&shard), before);
    assert_eq!(items(&shard), inventory);
    let started = shard.query_rows(
        "SELECT created_at, cast_time_ms FROM game_spell_cast_event WHERE spell_id = 8690",
    );
    assert_eq!(started.len(), 1);
    assert_eq!(started[0]["cast_time_ms"], "10000");
    assert_eq!(
        shard.query_rows("SELECT spell_id FROM game_pending_cast WHERE caster_guid = 1")[0]
            ["spell_id"],
        "8690"
    );
    let mut completed = Vec::new();
    assert!(poll_until(POLL_TIMEOUT, || {
        completed = shard.query_rows("SELECT created_at FROM game_spell_cast_event WHERE spell_id = 8690 AND is_completion = true");
        !completed.is_empty()
    }));
    assert!(timestamp(&completed[0]) - timestamp(&started[0]) >= 10_000_000);
    let home = position(&shard);
    for (column, expected) in [
        ("map_id", "0"),
        ("instance_id", "0"),
        ("x", "-9000"),
        ("y", "-100"),
        ("z", "80"),
        ("orientation", "1"),
    ] {
        assert_eq!(
            home[0][column].parse::<f32>().unwrap(),
            expected.parse::<f32>().unwrap()
        );
    }
    assert_eq!(items(&shard), inventory);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn cancelling_hearthstone_prevents_recall_and_keeps_it_usable() {
    let shard = fixture("hearthstone-cancel");
    let before = position(&shard);
    let inventory = items(&shard);
    use_hearthstone(&shard);
    shard.assert_call("gw_cancel_cast", &[&actor("1")]);
    assert!(shard
        .query_rows("SELECT * FROM game_pending_cast WHERE caster_guid = 1")
        .is_empty());
    std::thread::sleep(Duration::from_secs(11));
    assert_eq!(position(&shard), before);
    assert_eq!(items(&shard), inventory);
    use_hearthstone(&shard);
    shard.assert_call("gw_cancel_cast", &[&actor("1")]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn movement_cancels_hearthstone_before_recall() {
    let shard = fixture("hearthstone-movement");
    let inventory = items(&shard);
    let before = position(&shard);
    use_hearthstone(&shard);
    let x = (before[0]["x"].parse::<f32>().unwrap() + 1.0).to_string();
    shard.assert_call(
        "gw_movement_update",
        &[
            &actor("1"),
            &lyracore_shared::opcodes::movement::MSG_MOVE_STOP.to_string(),
            "[]",
            &x,
            &before[0]["y"],
            &before[0]["z"],
            "0",
            "1",
        ],
    );
    let moved = position(&shard);
    assert_eq!(
        moved[0]["x"].parse::<f32>().unwrap(),
        x.parse::<f32>().unwrap()
    );
    assert!(shard
        .query_rows("SELECT * FROM game_pending_cast WHERE caster_guid = 1")
        .is_empty());
    std::thread::sleep(Duration::from_secs(11));
    assert_eq!(position(&shard), moved);
    assert_eq!(items(&shard), inventory);
    use_hearthstone(&shard);
    shard.assert_call("gw_cancel_cast", &[&actor("1")]);
}
