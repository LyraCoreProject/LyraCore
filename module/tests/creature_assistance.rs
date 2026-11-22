mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use support::Standalone;

// vmangos DBCEnums.h FactionTemplateFlags.
const RESPONDS: u32 = 0x1;
const FLEES: u32 = 0x400;
// vmangos World.cpp CreatureFamilyAssistanceDelay default.
const ASSISTANCE_DELAY_MICROS: i64 = 1_500_000;
const IMPACT_MICROS: i64 = 50_000;
// `debug_advance_virtual_clock` stays inside one 50 ms scheduler window.
const WINDOW_MICROS: i64 = 50_000;
const LATER_COMBAT_UNTIL_MS: u64 = 1_800_000_100_000;
// vmangos `Creature::Update` calls for help from more than 10 yd away from the respawn point.
const CHASE_SECONDS: i64 = 5;
const VICTIM_CLOSE_X: f32 = 34.0;
const VICTIM_FAR_X: f32 = 70.0;
const CALLER_HOME_X: f32 = 30.0;
const FRIEND_X: f32 = 45.0;
const CALL_RANGE_YD: f32 = 20.0;
const CALLER_GROUP: u32 = 0x10;
const FRIEND_GROUP: u32 = 0x20;

fn stage(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard.assert_sql("UPDATE game_world_entity SET godmode = true WHERE guid = 1");
    set_call_for_help_range(&shard, 5.0);
    shard
}

fn set_call_for_help_range(shard: &Standalone, range: f32) {
    shard.assert_sql(&format!(
        "UPDATE game_creature_template SET call_for_help_range = {range} WHERE entry = {TEST_WOLF_ENTRY}"
    ));
}

fn faction(shard: &Standalone, id: u32, flags: u32) {
    shard.insert_row("game_faction_template", &[("id", &id), ("flags", &flags)]);
}

fn wolf(shard: &Standalone, offset: f32, faction_template: u32) -> String {
    shard.assert_call(
        "debug_spawn_at_feet",
        &["1", &TEST_WOLF_ENTRY.to_string(), &offset.to_string()],
    );
    let guid = shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
        ))
        .into_iter()
        .map(|row| row["guid"].parse::<u64>().unwrap())
        .max()
        .unwrap()
        .to_string();
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = true, health = 1000, \
         max_health = 1000, faction_template = {faction_template} WHERE guid = {guid}"
    ));
    guid
}

fn stage_hit(shard: &Standalone, creature: &str) {
    shard.assert_call(
        "debug_stage_virtual_spell_impact",
        &["1", creature, "10", &IMPACT_MICROS.to_string()],
    );
}

struct Clock<'a> {
    shard: &'a Standalone,
    elapsed: i64,
}

impl<'a> Clock<'a> {
    fn new(shard: &'a Standalone) -> Self {
        Self { shard, elapsed: 0 }
    }

    fn advance(&mut self, micros: i64) {
        let mut remaining = micros;
        while remaining > 0 {
            let into_window = self.elapsed % WINDOW_MICROS;
            let step = if into_window == 0 && remaining >= WINDOW_MICROS {
                let windows = remaining / WINDOW_MICROS;
                self.shard
                    .assert_call("debug_advance_roll_batch", &[&windows.to_string()]);
                windows * WINDOW_MICROS
            } else {
                let step = remaining.min(WINDOW_MICROS - into_window);
                self.shard
                    .assert_call("debug_advance_virtual_clock", &[&step.to_string()]);
                step
            };
            self.elapsed += step;
            remaining -= step;
        }
    }
}

fn victim_of(shard: &Standalone, creature: &str) -> Option<String> {
    shard
        .query_rows(&format!(
            "SELECT target_guid FROM game_melee_attack WHERE attacker_guid = {creature}"
        ))
        .first()
        .map(|row| row["target_guid"].clone())
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_spell_pull_brings_a_same_template_neighbour_within_ten_yards_after_one_and_a_half_seconds() {
    let shard = stage("creature-assistance-delay");
    let mut clock = Clock::new(&shard);
    faction(&shard, 9001, RESPONDS);
    let beyond_radius = wolf(&shard, 15.0, 9001);
    let caller = wolf(&shard, 30.0, 9001);
    let neighbour = wolf(&shard, 38.0, 9001);
    let neighbour_of_neighbour = wolf(&shard, 46.0, 9001);

    stage_hit(&shard, &caller);
    clock.advance(IMPACT_MICROS);
    assert_eq!(victim_of(&shard, &caller).as_deref(), Some("1"));
    clock.advance(ASSISTANCE_DELAY_MICROS - WINDOW_MICROS);
    clock.advance(WINDOW_MICROS - 1);
    assert_eq!(
        victim_of(&shard, &neighbour),
        None,
        "help waits the full delay"
    );
    clock.advance(1);
    assert_eq!(
        victim_of(&shard, &neighbour).as_deref(),
        Some("1"),
        "the neighbour 8 yards from the caller attacks its victim"
    );
    clock.advance(ASSISTANCE_DELAY_MICROS);
    assert_eq!(
        victim_of(&shard, &beyond_radius),
        None,
        "a creature 15 yards from the caller is not called"
    );
    assert_eq!(
        victim_of(&shard, &neighbour_of_neighbour),
        None,
        "a creature that answers does not call in turn"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn only_a_responding_idle_non_passive_neighbour_whose_template_does_not_flee_answers() {
    let shard = stage("creature-assistance-flags");
    let mut clock = Clock::new(&shard);
    let mut groups = Vec::new();
    for (template, flags, offset) in [
        (9002, 0, 20.0),
        (9003, RESPONDS | FLEES, 35.0),
        (9004, RESPONDS, 50.0),
        (9006, RESPONDS, 65.0),
    ] {
        faction(&shard, template, flags);
        let caller = wolf(&shard, offset, template);
        let neighbour = wolf(&shard, offset + 8.0, template);
        groups.push((template, caller, neighbour));
    }
    let passive = &groups[2].2;
    shard.assert_sql(&format!(
        "DELETE FROM game_creature_ai_state WHERE creature_guid = {passive}"
    ));
    shard.insert_row(
        "game_creature_ai_state",
        &[
            ("creature_guid", passive),
            ("engagement_id", &1),
            ("react_state", &0),
        ],
    );
    let in_combat = &groups[3].2;
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_flags = {}, combat_until_ms = {LATER_COMBAT_UNTIL_MS} \
         WHERE guid = {in_combat}",
        lyracore_shared::constants::unit_flags::IN_COMBAT
    ));
    for (_, caller, _) in &groups {
        stage_hit(&shard, caller);
    }

    clock.advance(IMPACT_MICROS);
    for (template, caller, _) in &groups {
        assert_eq!(
            victim_of(&shard, caller).as_deref(),
            Some("1"),
            "the caller of faction template {template} is pulled"
        );
    }
    clock.advance(ASSISTANCE_DELAY_MICROS + WINDOW_MICROS);
    for (template, _, neighbour) in &groups {
        assert_eq!(
            victim_of(&shard, neighbour),
            None,
            "the neighbour of faction template {template} stays out of the fight"
        );
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_creature_calls_for_assistance_once_a_fight() {
    let shard = stage("creature-assistance-once");
    let mut clock = Clock::new(&shard);
    faction(&shard, 9005, 0);
    let caller = wolf(&shard, 30.0, 9005);
    let neighbour = wolf(&shard, 38.0, 9005);
    stage_hit(&shard, &caller);
    clock.advance(IMPACT_MICROS);
    assert_eq!(victim_of(&shard, &caller).as_deref(), Some("1"));
    clock.advance(ASSISTANCE_DELAY_MICROS + WINDOW_MICROS);
    assert_eq!(victim_of(&shard, &neighbour), None);

    shard.assert_sql("UPDATE game_faction_template SET flags = 1 WHERE id = 9005");
    stage_hit(&shard, &caller);
    clock.advance(IMPACT_MICROS + ASSISTANCE_DELAY_MICROS + WINDOW_MICROS);
    assert_eq!(victim_of(&shard, &caller).as_deref(), Some("1"));
    assert_eq!(
        victim_of(&shard, &neighbour),
        None,
        "a second hit in the same fight does not call again"
    );
}

fn friendly_faction(shard: &Standalone, id: u32, flags: u32) {
    shard.insert_row(
        "game_faction_template",
        &[
            ("id", &id),
            ("faction_group", &FRIEND_GROUP),
            ("friend_group", &CALLER_GROUP),
            ("flags", &flags),
        ],
    );
}

fn x_of(shard: &Standalone, guid: &str) -> f32 {
    shard.query_rows(&format!(
        "SELECT x FROM game_world_entity WHERE guid = {guid}"
    ))[0]["x"]
        .parse()
        .unwrap()
}

// A caller pulled onto a victim standing at `victim_x`, with a friend of another faction template
// 15 yd from the caller's respawn point and out of its Call for Assistance radius.
fn pull_caller_toward(
    victim_x: f32,
    friend_flags: u32,
    name: &str,
) -> (Standalone, String, String) {
    let shard = stage(name);
    set_call_for_help_range(&shard, CALL_RANGE_YD);
    shard.insert_row(
        "game_faction_template",
        &[("id", &9010u32), ("faction_group", &CALLER_GROUP)],
    );
    friendly_faction(&shard, 9011, friend_flags);
    let caller = wolf(&shard, CALLER_HOME_X, 9010);
    let friend = wolf(&shard, FRIEND_X, 9011);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET x = {victim_x} WHERE guid = 1"
    ));
    stage_hit(&shard, &caller);
    (shard, caller, friend)
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_creature_chased_more_than_ten_yards_from_home_calls_a_friendly_creature_in_range_for_help() {
    let (shard, caller, friend) = pull_caller_toward(VICTIM_FAR_X, RESPONDS, "creature-help-far");
    let mut clock = Clock::new(&shard);
    clock.advance(IMPACT_MICROS);
    assert_eq!(victim_of(&shard, &caller).as_deref(), Some("1"));
    assert_eq!(
        victim_of(&shard, &friend),
        None,
        "a friend 15 yards from the caller's home is out of its Call for Assistance"
    );
    clock.advance(CHASE_SECONDS * 1_000_000);
    assert!(
        x_of(&shard, &caller) - CALLER_HOME_X > 10.0,
        "the caller ran more than 10 yards from home"
    );
    assert_eq!(
        victim_of(&shard, &friend).as_deref(),
        Some("1"),
        "a friendly creature of another faction template joins the chase"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_creature_fighting_within_ten_yards_of_home_calls_no_friend_for_help() {
    let (shard, caller, friend) =
        pull_caller_toward(VICTIM_CLOSE_X, RESPONDS, "creature-help-near");
    let mut clock = Clock::new(&shard);
    clock.advance(IMPACT_MICROS + CHASE_SECONDS * 1_000_000);
    assert_eq!(victim_of(&shard, &caller).as_deref(), Some("1"));
    assert!(x_of(&shard, &caller) - CALLER_HOME_X <= 10.0);
    assert_eq!(victim_of(&shard, &friend), None);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_creature_with_call_for_help_range_zero_calls_no_friend_for_help() {
    let (shard, caller, friend) = pull_caller_toward(VICTIM_FAR_X, RESPONDS, "creature-help-zero");
    set_call_for_help_range(&shard, 0.0);
    let mut clock = Clock::new(&shard);
    clock.advance(IMPACT_MICROS + CHASE_SECONDS * 1_000_000);
    assert_eq!(victim_of(&shard, &caller).as_deref(), Some("1"));
    assert_eq!(victim_of(&shard, &friend), None);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_friend_that_flees_from_calls_for_help_runs_ten_yards_from_a_nearby_victim() {
    let (shard, caller, fleer) = pull_caller_toward(VICTIM_FAR_X, FLEES, "creature-help-flee");
    let fleer_x = VICTIM_FAR_X - 8.0;
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET x = {fleer_x} WHERE guid = {fleer}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_creature_spawn SET x = {fleer_x}, movement_type = 1 WHERE guid = {fleer}"
    ));
    set_call_for_help_range(&shard, 40.0);
    let mut clock = Clock::new(&shard);
    clock.advance(IMPACT_MICROS);
    let mut farthest = 0.0f32;
    for _ in 0..(CHASE_SECONDS * 10) {
        clock.advance(100_000);
        let course_end: f32 = shard
            .query_rows(&format!(
                "SELECT dx FROM game_creature_spline WHERE guid = {fleer}"
            ))
            .first()
            .map_or(fleer_x, |row| row["dx"].parse().unwrap());
        farthest = farthest
            .max(VICTIM_FAR_X - course_end)
            .max(VICTIM_FAR_X - x_of(&shard, &fleer));
    }
    assert_eq!(victim_of(&shard, &caller).as_deref(), Some("1"));
    assert_eq!(
        victim_of(&shard, &fleer),
        None,
        "a fleeing creature never attacks"
    );
    assert!(
        farthest >= 15.0,
        "the creature on random movement ran 10 yards from the victim, not {farthest} away"
    );
}
