mod support;

use support::{
    event_rows, fence_subscription, number, poll_until, subscribe_with_fence, Standalone,
    POLL_TIMEOUT,
};

const PLAYER: u64 = 1;
const DUEL: u64 = 509_0001;
/// `lyracore_shared::duel::state::ACTIVE`.
const ACTIVE: u8 = 2;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_duel_ending_swing_is_no_killing_blow() {
    let mut standalone = Standalone::start("duel-killing-blow");
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
        "UPDATE game_world_entity SET simulation_indexed = false, x = {}, y = {y}, z = {z}, health = 1 WHERE guid = {rival}",
        x + 1.0
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET orientation = 0.0, godmode = true WHERE guid = {PLAYER}"
    ));
    standalone.assert_sql(&format!(
        "INSERT INTO game_duel (id, flag_guid, flag_entry, initiator_guid, challenged_guid, \
         map_id, instance_id, flag_x, flag_y, flag_z, flag_orientation, state, \
         countdown_deadline_micros, initiator_out_of_bounds_deadline_micros, \
         challenged_out_of_bounds_deadline_micros, created_at_micros) \
         VALUES ({DUEL}, 0, 21680, {PLAYER}, {rival}, {}, {}, {x}, {y}, {z}, 0.0, {ACTIVE}, \
         0, 0, 0, 0)",
        player["map_id"], player["instance_id"]
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
