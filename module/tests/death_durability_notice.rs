mod support;

use support::Standalone;

const BLADE: &str = "5090050";
const NOTICE_KIND: u8 = lyracore_shared::group::event_kind::DEATH_DURABILITY;

fn fixture(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_sql("DELETE FROM game_item_instance");
    // The reaper would delete the events under test.
    shard.assert_sql("DELETE FROM game_event_reaper_schedule");
    shard
}

fn notices(shard: &Standalone) -> Vec<String> {
    shard
        .query_rows(&format!(
            "SELECT recipient_guid FROM game_group_event WHERE kind = {NOTICE_KIND}"
        ))
        .into_iter()
        .map(|row| row["recipient_guid"].clone())
        .collect()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_death_that_damages_worn_gear_tells_the_victim_once() {
    let shard = fixture("death-durability-notice");
    shard.assert_call("debug_grant_item", &["1", BLADE, "1"]);
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET max_durability = 100 WHERE entry = {BLADE}"
    ));
    shard.assert_sql(
        "UPDATE game_item_instance SET slot = 15, durability = 100 WHERE owner_guid = 1",
    );

    shard.assert_call("debug_set_health", &["1", "0"]);

    let worn = shard.query_rows("SELECT durability FROM game_item_instance WHERE owner_guid = 1");
    assert_eq!(worn[0]["durability"], "90", "ten percent of the maximum");
    assert_eq!(notices(&shard), ["1"]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_death_with_nothing_to_damage_sends_no_notice() {
    let shard = fixture("death-durability-bare");

    shard.assert_call("debug_set_health", &["1", "0"]);

    assert!(notices(&shard).is_empty());
}
