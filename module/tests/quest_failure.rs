mod support;

use support::Standalone;

const CHARACTER: &str = "1";
const REPEATABLE: u32 = 5_115_800;
const PLAIN: u32 = 5_115_801;
const TIMED_REPEATABLE: u32 = 5_115_802;

fn setup(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &[CHARACTER]);
    for (entry, repeatable, limit_time) in [
        (REPEATABLE, true, 0u32),
        (PLAIN, false, 0),
        (TIMED_REPEATABLE, true, 600),
    ] {
        shard.insert_row(
            "game_quest_template",
            &[
                ("entry", &entry),
                ("quest_level", &2),
                ("title", &"Failing quest"),
                ("zone_or_sort", &12),
                ("repeatable", &repeatable),
                ("limit_time", &limit_time),
            ],
        );
    }
    shard
}

fn grant(shard: &Standalone, quest: u32) {
    shard.assert_call("debug_grant_quest", &[CHARACTER, &quest.to_string()]);
}

fn fail(shard: &Standalone, quest: u32) {
    shard.assert_call("debug_expire_quest", &[CHARACTER, &quest.to_string()]);
}

fn attempts(shard: &Standalone, quest: u32) -> Vec<support::Row> {
    shard.query_rows(&format!(
        "SELECT failed, rewarded FROM game_character_quest \
         WHERE character_guid = {CHARACTER} AND quest_entry = {quest}"
    ))
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_failed_repeatable_quest_without_a_timer_leaves_the_log_and_can_be_taken_again() {
    let shard = setup("quest-failure-repeatable");
    grant(&shard, REPEATABLE);
    fail(&shard, REPEATABLE);
    assert!(attempts(&shard, REPEATABLE).is_empty());

    grant(&shard, REPEATABLE);
    let again = attempts(&shard, REPEATABLE);
    assert_eq!(again.len(), 1, "{again:?}");
    assert_eq!(again[0]["failed"], "false", "{again:?}");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_failed_retake_returns_to_its_reward_record() {
    let shard = setup("quest-failure-retake");
    grant(&shard, REPEATABLE);
    shard.assert_sql(&format!(
        "UPDATE game_character_quest SET rewarded = true WHERE quest_entry = {REPEATABLE}"
    ));
    grant(&shard, REPEATABLE);
    fail(&shard, REPEATABLE);

    let rows = attempts(&shard, REPEATABLE);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["rewarded"], "true", "{rows:?}");
    assert_eq!(rows[0]["failed"], "false", "{rows:?}");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_failed_quest_that_is_not_repeatable_or_is_timed_stays_in_the_log() {
    let shard = setup("quest-failure-kept");
    for quest in [PLAIN, TIMED_REPEATABLE] {
        grant(&shard, quest);
        fail(&shard, quest);
        let rows = attempts(&shard, quest);
        assert_eq!(rows.len(), 1, "{quest}: {rows:?}");
        assert_eq!(rows[0]["failed"], "true", "{quest}: {rows:?}");
    }
}
