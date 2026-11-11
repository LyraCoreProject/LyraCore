#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_shared::spatial::{grid_cell, GRID_CELL_SIZE, MAP_COORD_MAX};
pub use lyracore_test_support::*;
use serde_json::json;

pub const LEG_YD: f32 = 20.0;

pub type Row = BTreeMap<String, String>;

const SUBSCRIPTION_MARKER: u64 = 509_9999;

pub fn subscribe_with_fence(shard: &Standalone, queries: &[&str]) -> TableSubscription {
    shard.assert_sql(&format!(
        "DELETE FROM game_package_config WHERE id = {SUBSCRIPTION_MARKER}"
    ));
    shard.assert_sql(&format!(
        "INSERT INTO game_package_config (id, package_name, key, value) \
         VALUES ({SUBSCRIPTION_MARKER}, 'fixture.subscription', 'transaction', 'initial')"
    ));
    let marker_query =
        format!("SELECT * FROM game_package_config WHERE id = {SUBSCRIPTION_MARKER}");
    let mut queries = queries.to_vec();
    queries.push(&marker_query);
    shard.subscribe(&queries)
}

// A marker on the same stream proves all earlier committed updates reached the recorder.
pub fn fence_subscription(shard: &Standalone, subscription: &TableSubscription) {
    static NEXT_MARKER: AtomicU64 = AtomicU64::new(1);
    let marker = NEXT_MARKER.fetch_add(1, Ordering::Relaxed).to_string();
    shard.assert_sql(&format!(
        "UPDATE game_package_config SET value = '{marker}' WHERE id = {SUBSCRIPTION_MARKER}"
    ));
    assert!(
        poll_until(POLL_TIMEOUT, || {
            subscription
                .inserts("game_package_config")
                .iter()
                .any(|row| row["id"] == SUBSCRIPTION_MARKER && row["value"] == marker)
        }),
        "the subscription did not receive transaction marker {marker}"
    );
}

pub fn event_rows(subscription: &TableSubscription, table: &str) -> Vec<Row> {
    subscription
        .inserts(table)
        .into_iter()
        .map(|row| {
            row.as_object()
                .expect("event row is an object")
                .iter()
                .map(|(column, value)| {
                    let value = if let Some(micros) =
                        value["__timestamp_micros_since_unix_epoch__"].as_i64()
                    {
                        spacetimedb::Timestamp::from_micros_since_unix_epoch(micros)
                            .to_rfc3339()
                            .expect("event timestamp is representable")
                    } else {
                        value
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| value.to_string())
                    };
                    (column.clone(), value)
                })
                .collect()
        })
        .collect()
}

pub fn wait_for_event_rows(subscription: &TableSubscription, table: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    assert!(
        poll_until(POLL_TIMEOUT, || {
            rows = event_rows(subscription, table);
            !rows.is_empty()
        }),
        "no inserts arrived for {table}"
    );
    rows
}

pub fn number<T: std::str::FromStr>(row: &Row, column: &str) -> T {
    row[column]
        .parse()
        .unwrap_or_else(|_| panic!("{column} is not a number in {row:?}"))
}

pub fn refused(shard: &Standalone, reducer: &str, args: &[&str], tag: &str) {
    let output = shard.call(reducer, args);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success() && text.contains(tag),
        "{reducer} should refuse with {tag}: {text}"
    );
}

pub fn guild_op_refused(realm: &Standalone, args: &[&str], tag: &str) {
    refused(realm, "realm_guild_op", args, tag);
}

pub fn roster_revision(node: &Standalone) -> u64 {
    node.query_rows("SELECT revision FROM game_group_roster_revision")[0]["revision"]
        .parse()
        .unwrap()
}

pub fn group_op_args(op: &str, actor_guid: u64, target_guid: u64, arg_a: u8) -> Vec<String> {
    vec![
        op.to_string(),
        actor(&actor_guid.to_string()),
        target_guid.to_string(),
        arg_a.to_string(),
        "0".to_string(),
        "0".to_string(),
    ]
}

pub fn group_op(node: &Standalone, op: &str, actor_guid: u64, target_guid: u64, arg_a: u8) {
    let args = group_op_args(op, actor_guid, target_guid, arg_a);
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    node.assert_call("realm_group_op", &args);
}

pub fn stage_group(realm: &Standalone, leader: u64, members: &[u64]) {
    realm.assert_call(
        "debug_stage_meeting_stone_group",
        &[&leader.to_string(), &json!(members).to_string(), "false"],
    );
}

pub fn wolves(shard: &mut Standalone, offsets: &[f32]) -> Vec<Row> {
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_sql(&format!(
        "DELETE FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
    ));
    for offset in offsets {
        shard.assert_call(
            "debug_spawn_at_feet",
            &["1", &TEST_WOLF_ENTRY.to_string(), &offset.to_string()],
        );
    }
    shard.assert_sql("DELETE FROM game_world_entity WHERE guid = 1");
    let mut wolves = shard.query_rows(&format!(
        "SELECT * FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
    ));
    assert_eq!(wolves.len(), offsets.len(), "{wolves:?}");
    wolves.sort_by_key(|wolf| number::<u64>(wolf, "guid"));
    wolves
}

pub fn wolves_beside(shard: &Standalone, character: &str, offsets: &[f32]) -> Vec<String> {
    shard.assert_sql(&format!(
        "DELETE FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
    ));
    for offset in offsets {
        shard.assert_call(
            "debug_spawn_at_feet",
            &[character, &TEST_WOLF_ENTRY.to_string(), &offset.to_string()],
        );
    }
    let mut guids: Vec<u64> = shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
        ))
        .iter()
        .map(|wolf| number(wolf, "guid"))
        .collect();
    assert_eq!(guids.len(), offsets.len(), "{guids:?}");
    guids.sort_unstable();
    guids.iter().map(u64::to_string).collect()
}

pub fn lone_wolf(shard: &mut Standalone) -> Row {
    wolves(shard, &[5.0]).remove(0)
}

pub fn leg_destination(at: &Row) -> (f32, f32, f32) {
    let (x, y, z): (f32, f32, f32) = (number(at, "x"), number(at, "y"), number(at, "z"));
    let (grid_x, _) = grid_cell(x, y);
    let centre_x = MAP_COORD_MAX - (grid_x as f32 + 0.5) * GRID_CELL_SIZE;
    let destination = (x + LEG_YD.copysign(centre_x - x), y, z);
    assert_eq!(grid_cell(destination.0, y), grid_cell(x, y));
    destination
}

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
