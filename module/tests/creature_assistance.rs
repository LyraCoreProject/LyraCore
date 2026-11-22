mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use support::Standalone;

// vmangos DBCEnums.h FactionTemplateFlags.
const RESPONDS: u32 = 0x1;
const FLEES: u32 = 0x400;
// vmangos World.cpp CreatureFamilyAssistanceDelay default.
const ASSISTANCE_DELAY_MICROS: i64 = 1_500_000;
const IMPACT_MICROS: i64 = 50_000;

fn stage(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard.assert_sql("UPDATE game_world_entity SET godmode = true WHERE guid = 1");
    shard
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

fn advance(shard: &Standalone, micros: i64) {
    shard.assert_call("debug_advance_virtual_clock", &[&micros.to_string()]);
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
    faction(&shard, 9001, RESPONDS);
    let beyond_radius = wolf(&shard, 15.0, 9001);
    let caller = wolf(&shard, 30.0, 9001);
    let neighbour = wolf(&shard, 38.0, 9001);
    let neighbour_of_neighbour = wolf(&shard, 46.0, 9001);

    stage_hit(&shard, &caller);
    advance(&shard, IMPACT_MICROS);
    assert_eq!(victim_of(&shard, &caller).as_deref(), Some("1"));
    advance(&shard, ASSISTANCE_DELAY_MICROS - 1);
    assert_eq!(
        victim_of(&shard, &neighbour),
        None,
        "help waits the full delay"
    );
    advance(&shard, 1);
    assert_eq!(
        victim_of(&shard, &neighbour).as_deref(),
        Some("1"),
        "the neighbour 8 yards from the caller attacks its victim"
    );
    advance(&shard, ASSISTANCE_DELAY_MICROS);
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
fn only_a_responding_non_passive_neighbour_whose_template_does_not_flee_answers() {
    let shard = stage("creature-assistance-flags");
    let mut neighbours = Vec::new();
    for (template, flags, offset) in [
        (9002, 0, 20.0),
        (9003, RESPONDS | FLEES, 40.0),
        (9004, RESPONDS, 60.0),
    ] {
        faction(&shard, template, flags);
        let caller = wolf(&shard, offset, template);
        neighbours.push((template, wolf(&shard, offset + 8.0, template)));
        stage_hit(&shard, &caller);
    }
    let passive = &neighbours[2].1;
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

    advance(&shard, IMPACT_MICROS + ASSISTANCE_DELAY_MICROS);
    for (template, neighbour) in &neighbours {
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
    faction(&shard, 9005, 0);
    let caller = wolf(&shard, 30.0, 9005);
    let neighbour = wolf(&shard, 38.0, 9005);
    stage_hit(&shard, &caller);
    advance(&shard, IMPACT_MICROS + ASSISTANCE_DELAY_MICROS);
    assert_eq!(victim_of(&shard, &neighbour), None);

    shard.assert_sql("UPDATE game_faction_template SET flags = 1 WHERE id = 9005");
    stage_hit(&shard, &caller);
    advance(&shard, IMPACT_MICROS + ASSISTANCE_DELAY_MICROS);
    assert_eq!(victim_of(&shard, &caller).as_deref(), Some("1"));
    assert_eq!(
        victim_of(&shard, &neighbour),
        None,
        "a second hit in the same fight does not call again"
    );
}
