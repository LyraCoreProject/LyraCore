mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use support::{
    event_rows, fence_subscription, number, poll_until, subscribe_with_fence, Standalone,
    POLL_TIMEOUT,
};

const PLAYER: u64 = 1;
const DUEL: u64 = 509_0001;
/// `state::ACTIVE` in `module/src/duel.rs`.
const ACTIVE: u8 = 2;

/// An active Duel between `PLAYER` and a second Character, whose guid it returns.
fn stage_active_duel(name: &str) -> (Standalone, u64) {
    let mut standalone = Standalone::start(name);
    standalone.publish_module();
    standalone.clear_schedule("game_creature_move_schedule");
    standalone.clear_schedule("game_event_reaper_schedule");
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_seed_scenario_fixtures", &[]);
    standalone.assert_call("debug_spawn_player_entity", &[&PLAYER.to_string()]);
    standalone.assert_call(
        "debug_create_package_character",
        &[r#""duel-test""#, r#""Rival""#, "1", "1"],
    );
    let rival: u64 = number(
        &standalone
            .query_rows("SELECT guid FROM game_character WHERE name = 'Rival'")
            .remove(0),
        "guid",
    );
    standalone.assert_call("debug_spawn_player_entity", &[&rival.to_string()]);

    let player = standalone
        .query_rows(&format!(
            "SELECT map_id, instance_id, x, y, z FROM game_world_entity WHERE guid = {PLAYER}"
        ))
        .remove(0);
    let (x, y, z): (f32, f32, f32) = (
        number(&player, "x"),
        number(&player, "y"),
        number(&player, "z"),
    );
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, x = {}, y = {y}, z = {z} WHERE guid = {rival}",
        x + 1.0
    ));
    standalone.insert_row(
        "game_duel",
        &[
            ("id", &DUEL),
            ("flag_entry", &21680),
            ("initiator_guid", &PLAYER),
            ("challenged_guid", &rival),
            ("map_id", &player["map_id"]),
            ("instance_id", &player["instance_id"]),
            ("flag_x", &x),
            ("flag_y", &y),
            ("flag_z", &z),
            ("state", &ACTIVE),
        ],
    );
    (standalone, rival)
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_duel_ending_swing_is_no_killing_blow() {
    let (standalone, rival) = stage_active_duel("duel-killing-blow");
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET orientation = 0.0, godmode = true WHERE guid = {PLAYER}"
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET health = 1 WHERE guid = {rival}"
    ));

    let events = subscribe_with_fence(
        &standalone,
        &[
            &format!("SELECT * FROM game_combat_event_live WHERE attacker_guid = {PLAYER}"),
            &format!("SELECT * FROM game_duel WHERE id = {DUEL}"),
        ],
    );
    standalone.assert_call("debug_engage", &[&PLAYER.to_string(), &rival.to_string()]);
    // A swing may miss or be avoided, so keep making the next one due.
    assert!(
        poll_until(POLL_TIMEOUT, || {
            standalone.assert_sql(&format!(
                "UPDATE game_melee_attack SET last_swing_ms = 1 WHERE attacker_guid = {PLAYER}"
            ));
            events
                .deletes("game_duel")
                .iter()
                .any(|row| row["id"] == DUEL)
        }),
        "no swing ended the Duel"
    );

    let rival_row = standalone
        .query_rows(&format!(
            "SELECT health, dead FROM game_world_entity WHERE guid = {rival}"
        ))
        .remove(0);
    assert_eq!(rival_row["dead"], "false", "the Duel loser must survive");
    assert_eq!(rival_row["health"], "1");
    fence_subscription(&standalone, &events);
    let swings = event_rows(&events, "game_combat_event_live");
    let damaging: Vec<_> = swings
        .iter()
        .filter(|swing| number::<u32>(swing, "damage") > 0)
        .collect();
    assert_eq!(damaging.len(), 1, "one swing must end the Duel: {swings:?}");
    assert_eq!(
        damaging[0]["killing_blow"], "false",
        "a Duel-ending swing is no killing blow: {swings:?}"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_creature_hit_that_leaves_a_duelist_at_one_health_ends_the_duel() {
    let (standalone, rival) = stage_active_duel("duel-third-party-hit");
    standalone.assert_sql(&format!(
        "DELETE FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
    ));
    standalone.assert_call(
        "debug_spawn_at_feet",
        &[&PLAYER.to_string(), &TEST_WOLF_ENTRY.to_string(), "3.0"],
    );
    let wolf: u64 = number(
        &standalone
            .query_rows(&format!(
                "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
            ))
            .remove(0),
        "guid",
    );
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false WHERE guid = {wolf}"
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET health = 40, max_health = 40 WHERE guid = {rival}"
    ));

    // debug_apply_damage caps the hit at the target's health minus 1.
    standalone.assert_call(
        "debug_apply_damage",
        &[&rival.to_string(), "1000", &wolf.to_string()],
    );

    let rival_row = standalone
        .query_rows(&format!(
            "SELECT health, dead FROM game_world_entity WHERE guid = {rival}"
        ))
        .remove(0);
    assert_eq!(rival_row["dead"], "false");
    assert_eq!(rival_row["health"], "1");
    assert!(
        standalone
            .query_rows(&format!("SELECT id FROM game_duel WHERE id = {DUEL}"))
            .is_empty(),
        "a hit that leaves a duelist at 1 health ends the Duel"
    );
}
