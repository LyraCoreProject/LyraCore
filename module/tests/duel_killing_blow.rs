mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::constants::unit_flags::IN_COMBAT;
use lyracore_shared::spell::taxonomy::{
    A_MOD_SPEED, A_PERIODIC_DAMAGE, CAST_FLAG_IGNORE_RESISTANCES, T_TARGET_ANY, T_TARGET_ENEMY,
};
use serde_json::json;
use std::time::{Duration, Instant};
use support::{
    event_rows, fence_subscription, number, poll_until, subscribe_with_fence, Field, NativePacket,
    ReducerConnection, Standalone, POLL_TIMEOUT,
};

const PLAYER: u64 = 1;
const DUEL: u64 = 509_0001;
/// `state::COUNTDOWN` and `state::ACTIVE` in `module/src/duel.rs`.
const COUNTDOWN: u8 = 1;
const ACTIVE: u8 = 2;
const SMSG_CANCEL_COMBAT: u16 = 0x14e;
const SMSG_DUEL_COMPLETE: u16 = 0x16a;
const SMSG_DUEL_WINNER: u16 = 0x16b;
/// `DuelWinnerReason::Won` and `DuelWinnerReason::Fled`.
const WON: u8 = 0;
const FLED: u8 = 1;
const PACKET_WAIT: Duration = Duration::from_secs(10);
const CLOCK_START: i64 = 1_800_000_000_000_000;
const DUEL_START: i64 = CLOCK_START + 100_000;
// 50 ms scheduler ticks that pass `DUEL_START`.
const PAST_DUEL_START_TICKS: &str = "4";
const PERIOD_MS: u32 = 30_000;
const CAST_ATTEMPTS: usize = 20;
const SPELL_BEFORE_START: u32 = 5_136_601;
const SPELL_AFTER_START: u32 = 5_136_602;
const SPELL_BUFF: u32 = 5_136_603;
const NATURE: u32 = 0x08;

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

/// `PLAYER` receives one SMSG_DUEL_WINNER of `reason` that names it the winner and `Rival` the loser.
fn assert_player_beat_rival(staged: &mut StagedDuel, reason: u8) {
    let expected = duel_winner_body(
        reason,
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
        "the duelist that lost to its opponent is named in one SMSG_DUEL_WINNER"
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
    assert_player_beat_rival(&mut staged, WON);
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
    assert_player_beat_rival(&mut staged, WON);
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
    assert_player_beat_rival(&mut staged, WON);
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

/// An ACTIVE Duel that started at `DUEL_START`, 100 ms after the virtual clock's first instant.
fn stage_started_duel(name: &str) -> StagedDuel {
    let staged = stage_duel(name, ACTIVE);
    let standalone = &staged.standalone;
    standalone.assert_call("debug_start_virtual_clock", &[&CLOCK_START.to_string()]);
    standalone.assert_sql(&format!(
        "UPDATE game_duel SET countdown_deadline_micros = {DUEL_START} WHERE id = {DUEL}"
    ));
    for spell_id in [SPELL_BEFORE_START, SPELL_AFTER_START] {
        insert_spell(
            standalone,
            spell_id,
            true,
            &[
                ("kind", &A_PERIODIC_DAMAGE),
                ("base_points", &1),
                ("period_ms", &PERIOD_MS),
            ],
        );
    }
    insert_spell(
        standalone,
        SPELL_BUFF,
        false,
        &[("kind", &A_MOD_SPEED), ("base_points", &50)],
    );
    staged
}

fn insert_spell(standalone: &Standalone, spell_id: u32, negative: bool, effect: &[Field]) {
    let name = format!("Duel aura fixture {spell_id}");
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &spell_id),
            ("name", &name),
            ("school_mask", &NATURE),
            ("range_yd", &0),
            ("is_negative", &negative),
            ("gcd_ms", &0),
            ("cooldown_ms", &0),
            ("duration_ms", &60_000),
            ("cast_flags", &CAST_FLAG_IGNORE_RESISTANCES),
        ],
    );
    let effect_id = packed_spell_effect_id(spell_id, 0);
    let target = if negative {
        T_TARGET_ENEMY
    } else {
        T_TARGET_ANY
    };
    let mut row: Vec<Field> = vec![
        ("id", &effect_id),
        ("spell_id", &spell_id),
        ("target", &target),
    ];
    row.extend_from_slice(effect);
    standalone.insert_row("game_spell_effect", &row);
}

fn auras_from(standalone: &Standalone, caster: u64, target: u64) -> Vec<u32> {
    let mut spells: Vec<u32> = standalone
        .query_rows(&format!(
            "SELECT spell_id FROM game_aura WHERE target_guid = {target} AND caster_guid = {caster}"
        ))
        .iter()
        .map(|row| number(row, "spell_id"))
        .collect();
    spells.sort_unstable();
    spells
}

fn cast_until_applied(standalone: &Standalone, caster: u64, spell_id: u32, target: u64) {
    for _ in 0..CAST_ATTEMPTS {
        standalone.assert_call(
            "debug_force_cast_at",
            &[
                &caster.to_string(),
                &spell_id.to_string(),
                &target.to_string(),
            ],
        );
        if auras_from(standalone, caster, target).contains(&spell_id) {
            return;
        }
    }
    panic!("spell {spell_id} from {caster} did not land on {target} in {CAST_ATTEMPTS} casts");
}

fn duel_exists(standalone: &Standalone) -> bool {
    !standalone
        .query_rows(&format!("SELECT id FROM game_duel WHERE id = {DUEL}"))
        .is_empty()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_duel_ending_removes_the_negative_auras_each_duelist_cast_on_the_other_after_the_countdown() {
    let staged = stage_started_duel("duel-end-removes-auras");
    let (rival, standalone) = (staged.rival, &staged.standalone);
    cast_until_applied(standalone, rival, SPELL_BEFORE_START, PLAYER);
    standalone.assert_call("debug_advance_roll_batch", &[PAST_DUEL_START_TICKS]);
    cast_until_applied(standalone, rival, SPELL_AFTER_START, PLAYER);
    cast_until_applied(standalone, rival, SPELL_BUFF, PLAYER);
    cast_until_applied(standalone, PLAYER, SPELL_AFTER_START, rival);
    assert!(duel_exists(standalone), "the Duel must still run");

    standalone.assert_call(
        "debug_apply_damage",
        &[&rival.to_string(), "1000", &PLAYER.to_string()],
    );

    assert!(!duel_exists(standalone), "the hit must end the Duel");
    assert_eq!(
        auras_from(standalone, rival, PLAYER),
        [SPELL_BEFORE_START, SPELL_BUFF],
        "only the negative Aura cast after the start is removed"
    );
    assert!(
        auras_from(standalone, PLAYER, rival).is_empty(),
        "the winner's negative Aura leaves the loser too"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_duel_ending_before_its_start_removes_no_aura() {
    let staged = stage_started_duel("duel-end-before-start-keeps-auras");
    let (rival, standalone) = (staged.rival, &staged.standalone);
    standalone.assert_sql(&format!(
        "UPDATE game_duel SET countdown_deadline_micros = 0 WHERE id = {DUEL}"
    ));
    cast_until_applied(standalone, PLAYER, SPELL_AFTER_START, rival);
    standalone.assert_sql(&format!(
        "UPDATE game_duel SET state = {COUNTDOWN} WHERE id = {DUEL}"
    ));

    standalone.assert_call(
        "debug_apply_damage",
        &[&rival.to_string(), "1000", &PLAYER.to_string()],
    );

    assert!(!duel_exists(standalone), "the hit must end the Duel");
    assert_eq!(
        auras_from(standalone, PLAYER, rival),
        [SPELL_AFTER_START],
        "a Duel that never started removes nothing"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_duelist_teleporting_to_another_map_flees_and_the_opponent_wins() {
    let mut staged = stage_duel("duel-teleport-flees", ACTIVE);
    let rival = staged.rival.to_string();

    staged.standalone.assert_call(
        "debug_teleport",
        &[&rival, "1", "300.0", "300.0", "30.0", "0.0"],
    );

    assert!(
        !duel_exists(&staged.standalone),
        "the teleport must end the Duel"
    );
    assert_player_beat_rival(&mut staged, FLED);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_duelist_entering_another_instance_flees_and_the_opponent_wins() {
    let mut staged = stage_duel("duel-instance-move-flees", ACTIVE);

    staged.standalone.assert_call(
        "debug_create_fixture_instance",
        &[&staged.rival.to_string()],
    );

    assert!(
        !duel_exists(&staged.standalone),
        "the instance move must end the Duel"
    );
    assert_player_beat_rival(&mut staged, FLED);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_challenged_character_teleporting_away_interrupts_the_pending_challenge_without_a_winner() {
    let mut staged = stage_duel("duel-teleport-pending", 0);
    let rival = staged.rival.to_string();

    staged.standalone.assert_call(
        "debug_teleport",
        &[&rival, "1", "300.0", "300.0", "30.0", "0.0"],
    );

    let packets = wait_for_packet(&mut staged.wire, |packet| {
        packet.opcode == Some(SMSG_DUEL_COMPLETE)
    });
    let completions: Vec<&[u8]> = packets
        .iter()
        .filter(|packet| packet.opcode == Some(SMSG_DUEL_COMPLETE))
        .map(|packet| packet.body.as_slice())
        .collect();
    assert_eq!(
        completions,
        [[0u8].as_slice()],
        "the challenge is interrupted, not completed"
    );
    assert!(
        !packets
            .iter()
            .any(|packet| packet.opcode == Some(SMSG_DUEL_WINNER)),
        "a challenge nobody accepted has no winner"
    );
}
