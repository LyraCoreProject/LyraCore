mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_shared::guid;
use support::{
    event_rows, fence_subscription, poll_until, subscribe_with_fence, Row, Standalone,
    TableSubscription, POLL_TIMEOUT,
};

const PLAYER: u64 = 1;
const WOLF: u64 = guid::creature(TEST_WOLF_ENTRY, 1);
const ACTOR: &str = r#"{"guid":1,"ownership":null}"#;
const FIREBALL: &str = "133";
const RANGE_YD: f32 = 30.0;
// Start leeway: 2 yd combat reach plus 8/3 yd moving-target tolerance.
const START_REACH_YD: f32 = RANGE_YD + 2.0 + 8.0 / 3.0;
// `CastRefusalReason` failure codes in `game_spell_cast_event_live.failure_reason`.
const NO_POWER: &str = "1";
const OUT_OF_RANGE: &str = "7";

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_target_that_leaves_range_during_the_cast_is_refused_out_of_range() {
    let shard = fixture("completion-out-of-range");
    let casts = subscribe_casts(&shard);
    cast_fireball(&shard);
    place_wolf(&shard, 80.0);
    assert_eq!(refusal(&shard, &casts)["failure_reason"], OUT_OF_RANGE);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_completion_checks_power_before_range() {
    let shard = fixture("completion-power-first");
    let casts = subscribe_casts(&shard);
    cast_fireball(&shard);
    shard.assert_call("debug_set_power", &[&PLAYER.to_string(), "0"]);
    place_wolf(&shard, 80.0);
    assert_eq!(refusal(&shard, &casts)["failure_reason"], NO_POWER);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_completion_reaches_5_yd_further_than_its_start() {
    let shard = fixture("completion-range-tolerance");
    let drifted = START_REACH_YD + 3.0;
    place_wolf(&shard, drifted);
    let result = shard.call("gw_cast_spell", &[ACTOR, FIREBALL, &WOLF.to_string()]);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        !result.status.success() && stderr.contains("out of range"),
        "{stderr}"
    );

    place_wolf(&shard, 10.0);
    let casts = subscribe_casts(&shard);
    cast_fireball(&shard);
    place_wolf(&shard, drifted);
    assert!(
        poll_until(POLL_TIMEOUT, || cast_rows(&casts)
            .iter()
            .any(|row| row["is_completion"] == "true")),
        "the drifted target was not hit at completion: {:?}",
        cast_rows(&casts)
    );
    assert!(!cast_rows(&casts).iter().any(is_interrupt));
    assert_wolf_at(&shard, drifted);
}

fn fixture(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.clear_schedule("game_creature_move_schedule");
    shard.clear_schedule("game_melee_schedule");
    shard.assert_call("claim_operator", &[]);
    shard.assert_sql("UPDATE game_character SET class = 8 WHERE guid = 1");
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call("debug_learn_spell", &["1", FIREBALL]);
    shard.assert_sql(&format!(
        "UPDATE game_spell SET cast_time_ms = 5000, gcd_ms = 0, cooldown_ms = 0, \
         range_yd = {RANGE_YD} WHERE spell_id = {FIREBALL}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET x = 0, y = 0, z = 0, orientation = 0 WHERE guid = {PLAYER}"
    ));
    place_wolf(&shard, 10.0);
    shard
}

fn place_wolf(shard: &Standalone, x: f32) {
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, x = {x}, y = 0, z = 0 \
         WHERE guid = {WOLF}"
    ));
}

fn assert_wolf_at(shard: &Standalone, x: f32) {
    let rows = shard.query_rows(&format!(
        "SELECT x, y, z FROM game_world_entity WHERE guid = {WOLF}"
    ));
    let at: Vec<f32> = ["x", "y", "z"]
        .iter()
        .map(|axis| rows[0][*axis].parse().unwrap())
        .collect();
    assert!(
        (at[0] - x).abs() < 0.01 && at[1] == 0.0 && at[2] == 0.0,
        "the wolf moved during the cast: {at:?}"
    );
}

fn cast_fireball(shard: &Standalone) {
    shard.assert_call("gw_cast_spell", &[ACTOR, FIREBALL, &WOLF.to_string()]);
}

fn subscribe_casts(shard: &Standalone) -> TableSubscription {
    subscribe_with_fence(
        shard,
        &["SELECT * FROM game_spell_cast_event_live WHERE caster_guid = 1 AND spell_id = 133"],
    )
}

fn cast_rows(casts: &TableSubscription) -> Vec<Row> {
    event_rows(casts, "game_spell_cast_event_live")
}

fn is_interrupt(row: &Row) -> bool {
    row["is_interrupted"] == "true"
}

fn refusal(shard: &Standalone, casts: &TableSubscription) -> Row {
    assert!(
        poll_until(POLL_TIMEOUT, || cast_rows(casts).iter().any(is_interrupt)),
        "the timed cast was not refused at completion: {:?}",
        cast_rows(casts)
    );
    fence_subscription(shard, casts);
    let rows = cast_rows(casts);
    assert!(
        !rows.iter().any(|row| row["is_completion"] == "true"),
        "{rows:?}"
    );
    let mut refusals: Vec<_> = rows.into_iter().filter(is_interrupt).collect();
    assert_eq!(refusals.len(), 1, "{refusals:?}");
    refusals.remove(0)
}
