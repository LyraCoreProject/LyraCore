mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_shared::constants::unit_flags::IN_COMBAT;
use serde_json::json;
use std::time::{Duration, Instant};
use support::{
    event_rows, fence_subscription, number, poll_until, subscribe_with_fence, NativePacket,
    ReducerConnection, Standalone, POLL_TIMEOUT,
};

const PLAYER: u64 = 1;
const DUEL: u64 = 509_0001;
/// `state::COUNTDOWN` and `state::ACTIVE` in `module/src/duel.rs`.
const COUNTDOWN: u8 = 1;
const ACTIVE: u8 = 2;
const SMSG_CANCEL_COMBAT: u16 = 0x14e;
const SMSG_DUEL_WINNER: u16 = 0x16b;
/// `DuelWinnerReason::Won`; 1 is `Fled`.
const WON: u8 = 0;
const PACKET_WAIT: Duration = Duration::from_secs(10);

struct StagedDuel {
    standalone: Standalone,
    rival: u64,
    /// The World Session of `PLAYER`, the initiator.
    wire: ReducerConnection,
}

/// A Duel in `state` between `PLAYER` and a second Character, `Rival`.
fn stage_duel(name: &str, state: u8) -> StagedDuel {
    let mut standalone = Standalone::start(name);
    standalone.publish_module();
    standalone.clear_schedule("game_creature_move_schedule");
    standalone.clear_schedule("game_event_reaper_schedule");
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_seed_scenario_fixtures", &[]);
    standalone.assert_call("debug_spawn_player_entity", &[&PLAYER.to_string()]);
    let mut wire = standalone.connect_reducers(&["SELECT * FROM game_native_packet"]);
    wire.call("native_world_open", json!([42])).unwrap();
    wire.call("debug_native_enter_world", json!([PLAYER]))
        .unwrap();
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
            ("state", &state),
            ("countdown_deadline_micros", &i64::MAX),
        ],
    );
    wire.drain_packets();
    StagedDuel {
        standalone,
        rival,
        wire,
    }
}

fn spawn_wolf(standalone: &Standalone) -> u64 {
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
    wolf
}

fn character_name(standalone: &Standalone, guid: u64) -> String {
    standalone
        .query_rows(&format!(
            "SELECT name FROM game_character WHERE guid = {guid}"
        ))
        .remove(0)
        .remove("name")
        .expect("a Character name")
}

/// The SMSG_DUEL_WINNER body: the reason, then the winner's and the loser's names as CStrings.
fn duel_winner_body(reason: u8, winner: &str, loser: &str) -> Vec<u8> {
    let mut body = vec![reason];
    for name in [winner, loser] {
        body.extend_from_slice(name.as_bytes());
        body.push(0);
    }
    body
}

fn wait_for_packet(
    wire: &mut ReducerConnection,
    wanted: impl Fn(&NativePacket) -> bool,
) -> Vec<NativePacket> {
    let deadline = Instant::now() + PACKET_WAIT;
    let mut packets = wire.wait_for_packets(Duration::from_secs(1)).unwrap();
    while !packets.iter().any(&wanted) && Instant::now() < deadline {
        packets.extend(wire.wait_for_packets(Duration::from_secs(1)).unwrap());
    }
    packets
}

/// `PLAYER` receives one SMSG_DUEL_WINNER that names it the winner and `Rival` the loser.
fn assert_player_beat_rival(staged: &mut StagedDuel) {
    let expected = duel_winner_body(
        WON,
        &character_name(&staged.standalone, PLAYER),
        &character_name(&staged.standalone, staged.rival),
    );
    let packets = wait_for_packet(&mut staged.wire, |packet| {
        packet.opcode == Some(SMSG_DUEL_WINNER)
    });
    let winners: Vec<&[u8]> = packets
        .iter()
        .filter(|packet| packet.opcode == Some(SMSG_DUEL_WINNER))
        .map(|packet| packet.body.as_slice())
        .collect();
    assert_eq!(
        winners,
        [expected.as_slice()],
        "the duelist left at 1 health loses to its opponent"
    );
}

fn assert_rival_lives_at_one_health(staged: &StagedDuel) {
    let rival_row = staged
        .standalone
        .query_rows(&format!(
            "SELECT health, dead FROM game_world_entity WHERE guid = {}",
            staged.rival
        ))
        .remove(0);
    assert_eq!(rival_row["dead"], "false", "the Duel loser must survive");
    assert_eq!(rival_row["health"], "1");
    assert!(
        staged
            .standalone
            .query_rows(&format!("SELECT id FROM game_duel WHERE id = {DUEL}"))
            .is_empty(),
        "a hit that leaves a duelist at 1 health ends the Duel"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_duel_ending_swing_is_no_killing_blow_and_wins_the_duel() {
    let mut staged = stage_duel("duel-killing-blow", ACTIVE);
    let rival = staged.rival;
    let standalone = &staged.standalone;
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET orientation = 0.0, godmode = true WHERE guid = {PLAYER}"
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET health = 1 WHERE guid = {rival}"
    ));

    let events = subscribe_with_fence(
        standalone,
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

    assert_rival_lives_at_one_health(&staged);
    fence_subscription(&staged.standalone, &events);
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
    assert_player_beat_rival(&mut staged);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_creature_hit_that_leaves_a_duelist_at_one_health_ends_the_duel_for_its_opponent() {
    let mut staged = stage_duel("duel-third-party-hit", ACTIVE);
    let rival = staged.rival;
    let wolf = spawn_wolf(&staged.standalone);
    staged.standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET health = 40, max_health = 40 WHERE guid = {rival}"
    ));

    // debug_apply_damage caps the hit at the target's health minus 1.
    staged.standalone.assert_call(
        "debug_apply_damage",
        &[&rival.to_string(), "1000", &wolf.to_string()],
    );

    assert_rival_lives_at_one_health(&staged);
    assert_player_beat_rival(&mut staged);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_creature_hit_during_the_countdown_ends_the_duel_for_the_other_duelist() {
    let mut staged = stage_duel("duel-countdown-hit", COUNTDOWN);
    let rival = staged.rival;
    let wolf = spawn_wolf(&staged.standalone);
    staged.standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET health = 40, max_health = 40 WHERE guid = {rival}"
    ));

    staged.standalone.assert_call(
        "debug_apply_damage",
        &[&rival.to_string(), "1000", &wolf.to_string()],
    );

    assert_rival_lives_at_one_health(&staged);
    assert_player_beat_rival(&mut staged);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_creature_fighting_the_winner_stops_its_attack_when_the_duel_ends() {
    let mut staged = stage_duel("duel-end-stops-combat", ACTIVE);
    let rival = staged.rival;
    let wolf = spawn_wolf(&staged.standalone);
    let standalone = &staged.standalone;
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET health = 40, max_health = 40 WHERE guid = {rival}"
    ));
    standalone.assert_call("debug_engage", &[&wolf.to_string(), &PLAYER.to_string()]);
    let flags: u32 = number(
        &standalone
            .query_rows(&format!(
                "SELECT unit_flags FROM game_world_entity WHERE guid = {PLAYER}"
            ))
            .remove(0),
        "unit_flags",
    );
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_flags = {} WHERE guid = {PLAYER}",
        flags | IN_COMBAT
    ));
    staged.wire.drain_packets();

    standalone.assert_call(
        "debug_apply_damage",
        &[&rival.to_string(), "1000", &PLAYER.to_string()],
    );

    assert!(
        standalone
            .query_rows(&format!(
                "SELECT attacker_guid FROM game_melee_attack WHERE attacker_guid = {wolf}"
            ))
            .is_empty(),
        "the Duel's end stops every attack on the winner"
    );
    let flags: u32 = number(
        &standalone
            .query_rows(&format!(
                "SELECT unit_flags FROM game_world_entity WHERE guid = {PLAYER}"
            ))
            .remove(0),
        "unit_flags",
    );
    assert_eq!(flags & IN_COMBAT, 0, "the winner leaves combat");
    let packets = wait_for_packet(&mut staged.wire, |packet| {
        packet.opcode == Some(SMSG_CANCEL_COMBAT)
    });
    assert!(
        packets
            .iter()
            .any(|packet| packet.opcode == Some(SMSG_CANCEL_COMBAT)),
        "the winner's client is told to cancel its attack"
    );
}
