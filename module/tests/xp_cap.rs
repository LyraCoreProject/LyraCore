//! Character XP awards through the Runtime Script Host and creature death.

mod support;

use support::Standalone;

/// Builds and publishes only to the test's private Standalone process.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn the_level_cap_discards_excess_xp_and_preserves_rested_xp_on_later_kills() {
    let mut standalone = Standalone::start("xp-cap");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("debug_seed_scenario_fixtures", &[]);
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_call("debug_set_level", &["1", "59"]);
    standalone.assert_sql("UPDATE game_world_entity SET xp = 209799 WHERE guid = 1");

    grant_xp(&standalone, 1_000_000);
    assert_capped(&standalone);
    let levels = standalone.query_rows("SELECT new_level FROM game_levelup_event");
    assert_eq!(levels.len(), 1);
    assert_eq!(levels[0]["new_level"], "60");

    grant_xp(&standalone, 25);
    assert_capped(&standalone);
    assert_eq!(
        standalone
            .query_rows("SELECT new_level FROM game_levelup_event")
            .len(),
        1,
        "a later award must not produce another level-up"
    );

    standalone.assert_sql("UPDATE game_character SET rested_xp = 1000 WHERE guid = 1");
    standalone.assert_call("debug_spawn_at_feet", &["1", "51000", "1.0"]);
    let creature = standalone
        .query_rows("SELECT guid FROM game_world_entity WHERE entry = 51000")
        .pop()
        .expect("the fixture creature exists")["guid"]
        .clone();
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET level = 60 WHERE guid = {creature}"
    ));
    standalone.assert_sql(&format!(
        "INSERT INTO game_creature_quest_tap (creature_guid, character_guid) \
         VALUES ({creature}, 1)"
    ));
    standalone.assert_call("debug_kill_nearest", &["1", "51000"]);

    assert_capped(&standalone);
    let character = standalone.query_rows("SELECT rested_xp FROM game_character WHERE guid = 1");
    assert_eq!(character[0]["rested_xp"], "1000");
    assert!(standalone
        .query_rows("SELECT id FROM game_xp_event")
        .is_empty());
    let corpse = standalone.query_rows(&format!(
        "SELECT dead FROM game_world_entity WHERE guid = {creature}"
    ));
    assert_eq!(corpse[0]["dead"], "true");
}

fn grant_xp(standalone: &Standalone, amount: u32) {
    let source = serde_json::to_string(&format!("grant_xp(event.actor, {amount})")).unwrap();
    standalone.assert_call(
        "debug_run_runtime_script",
        &["\"fixture.xp-cap\"", "\"on_kill\"", &source, "1", "0"],
    );
}

fn assert_capped(standalone: &Standalone) {
    let entity = standalone
        .query_rows("SELECT level, xp, next_level_xp FROM game_world_entity WHERE guid = 1");
    assert_eq!(entity[0]["level"], "60");
    assert_eq!(entity[0]["xp"], "0");
    assert_eq!(entity[0]["next_level_xp"], "0");
}
