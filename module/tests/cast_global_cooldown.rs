mod support;

use spacetimedb::Timestamp;
use support::{poll_until, Row, Standalone, POLL_TIMEOUT};

const WOLF: u64 = (0xF130_u64 << 48) | (51000_u64 << 24) | 1;
const ACTOR: &str = r#"{"guid":1,"ownership":null}"#;

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn fireball_can_be_cast_again_when_its_cast_bar_finishes() {
    let shard = fixture("fireball-global-cooldown", 2000, 1500, 0);
    cast_fireball(&shard);
    let completed_at = timestamp(&completion(&shard), "created_at");
    let ready_at = timestamp(&global_cooldown(&shard), "ready_at");
    assert!(
        ready_at <= completed_at,
        "Fireball started another global cooldown at launch: ready {ready_at}, launched {completed_at}"
    );
    cast_fireball(&shard);
    assert_eq!(
        shard
            .query_rows("SELECT * FROM game_pending_cast WHERE caster_guid = 1")
            .len(),
        1
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_short_cast_finishes_during_its_original_global_cooldown() {
    let shard = fixture("short-cast-global-cooldown", 500, 5000, 7000);
    cast_fireball(&shard);
    let started_at = timestamp(
        &shard.query_rows("SELECT * FROM game_spell_cast_event WHERE cast_time_ms = 500")[0],
        "created_at",
    );
    let original_cooldown = global_cooldown(&shard);
    assert_eq!(
        timestamp(&original_cooldown, "ready_at"),
        started_at + 5_000_000
    );
    assert!(shard.query_rows("SELECT * FROM game_spell_cd").is_empty());

    let completed_at = timestamp(&completion(&shard), "created_at");
    assert!(completed_at < timestamp(&original_cooldown, "ready_at"));
    assert_eq!(global_cooldown(&shard), original_cooldown);
    let spell_cooldown = shard.query_rows("SELECT * FROM game_spell_cd WHERE caster_guid = 1");
    assert_eq!(
        timestamp(&spell_cooldown[0], "ready_at"),
        completed_at + 7_000_000
    );
    assert_global_cooldown_refusal(&shard);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn an_instant_cast_still_starts_a_global_cooldown() {
    let shard = fixture("instant-cast-global-cooldown", 0, 5000, 0);
    cast_fireball(&shard);
    let launched_at = timestamp(
        &shard.query_rows("SELECT * FROM game_spell_cast_event WHERE caster_guid = 1")[0],
        "created_at",
    );
    assert_eq!(
        timestamp(&global_cooldown(&shard), "ready_at"),
        launched_at + 5_000_000
    );
    assert_global_cooldown_refusal(&shard);
    shard.assert_call("gw_cancel_cast", &[ACTOR]);
    assert_global_cooldown_refusal(&shard);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn cancelling_a_timed_cast_releases_its_global_cooldown() {
    let shard = fixture("cancelled-cast-global-cooldown", 60000, 60000, 0);
    cast_fireball(&shard);
    assert_global_cooldown_refusal(&shard);
    shard.assert_call("gw_cancel_cast", &[ACTOR]);
    assert!(shard
        .query_rows("SELECT * FROM game_pending_cast")
        .is_empty());
    assert!(shard
        .query_rows("SELECT * FROM game_spell_cooldown")
        .is_empty());
    cast_fireball(&shard);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_refused_cast_does_not_start_a_global_cooldown() {
    let shard = fixture("refused-cast-global-cooldown", 2000, 1500, 0);
    shard.assert_sql("UPDATE game_spell SET cost = 1000 WHERE spell_id = 133");
    shard.assert_call("debug_set_power", &["1", "0"]);
    let result = shard.call("gw_cast_spell", &[ACTOR, "133", &WOLF.to_string()]);
    assert!(!result.status.success());
    assert!(shard
        .query_rows("SELECT * FROM game_spell_cooldown")
        .is_empty());
    assert!(shard
        .query_rows("SELECT * FROM game_pending_cast")
        .is_empty());
    shard.assert_call("debug_set_power", &["1", "1000"]);
    cast_fireball(&shard);
}

fn fixture(name: &str, cast_ms: u32, gcd_ms: u32, cooldown_ms: u32) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_sql("UPDATE game_character SET class = 8 WHERE guid = 1");
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call("debug_learn_spell", &["1", "133"]);
    shard.assert_sql(&format!(
        "UPDATE game_spell SET cast_time_ms = {cast_ms}, gcd_ms = {gcd_ms}, \
         cooldown_ms = {cooldown_ms} WHERE spell_id = 133"
    ));
    shard
}

fn cast_fireball(shard: &Standalone) {
    shard.assert_call("gw_cast_spell", &[ACTOR, "133", &WOLF.to_string()]);
}

fn completion(shard: &Standalone) -> Row {
    let mut rows = Vec::new();
    assert!(
        poll_until(POLL_TIMEOUT, || {
            rows =
                shard.query_rows("SELECT * FROM game_spell_cast_event WHERE is_completion = true");
            !rows.is_empty()
        }),
        "the timed cast did not launch"
    );
    assert_eq!(rows.len(), 1);
    rows.remove(0)
}

fn global_cooldown(shard: &Standalone) -> Row {
    let mut rows = shard.query_rows("SELECT * FROM game_spell_cooldown WHERE caster_guid = 1");
    assert_eq!(rows.len(), 1, "an accepted cast starts the global cooldown");
    rows.remove(0)
}

fn timestamp(row: &Row, column: &str) -> i64 {
    Timestamp::parse_from_rfc3339(&row[column])
        .unwrap()
        .to_micros_since_unix_epoch()
}

fn assert_global_cooldown_refusal(shard: &Standalone) {
    let result = shard.call("gw_cast_spell", &[ACTOR, "133", &WOLF.to_string()]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("global cooldown"));
}
