mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use support::{stage_group, Standalone};

const HUMAN: u8 = 1;
const WARRIOR: u8 = 1;
const STONE_AT: (f32, f32, f32) = (-8949.95, -132.493, 83.5312);

fn start(name: &str) -> Standalone {
    let mut realm = Standalone::start(name);
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    realm.assert_call("install_guid_range", &["0"]);
    realm.assert_call("debug_seed_scenario_fixtures", &[]);
    realm.assert_sql(&format!(
        "DELETE FROM game_creature_spawn WHERE entry = {TEST_WOLF_ENTRY}"
    ));
    realm.assert_sql(&format!(
        "DELETE FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
    ));
    realm
}

fn stage(realm: &Standalone, guid: u64, level: u32) {
    let (x, y, z) = STONE_AT;
    realm.assert_call(
        "debug_stage_meeting_stone_character",
        &[
            &guid.to_string(),
            &guid.to_string(),
            &HUMAN.to_string(),
            &WARRIOR.to_string(),
            &level.to_string(),
            "0",
            &x.to_string(),
            &y.to_string(),
            &z.to_string(),
        ],
    );
    realm.assert_call("debug_set_level", &[&guid.to_string(), &level.to_string()]);
    realm.assert_sql(&format!(
        "UPDATE game_character SET rested_xp = 0 WHERE guid = {guid}"
    ));
}

fn spawn_wolf(realm: &Standalone, beside: u64, level: u32) -> String {
    realm.assert_call(
        "debug_spawn_at_feet",
        &[&beside.to_string(), &TEST_WOLF_ENTRY.to_string(), "1.0"],
    );
    let wolves = realm.query_rows(&format!(
        "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
    ));
    assert_eq!(wolves.len(), 1, "the kill target must be unique");
    let wolf = wolves[0]["guid"].clone();
    realm.assert_sql(&format!(
        "UPDATE game_world_entity SET level = {level} WHERE guid = {wolf}"
    ));
    wolf
}

fn xp_of(realm: &Standalone, guid: u64) -> u32 {
    realm.query_rows(&format!(
        "SELECT xp FROM game_world_entity WHERE guid = {guid}"
    ))[0]["xp"]
        .parse()
        .unwrap()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_solo_kill_pays_the_1_12_figure_for_the_level_difference() {
    let realm = start("kill-xp-solo");
    const KILLER: u64 = 509_7001;
    stage(&realm, KILLER, 30);
    let wolf = spawn_wolf(&realm, KILLER, 27);
    realm.insert_row(
        "game_creature_quest_tap",
        &[("creature_guid", &wolf), ("character_guid", &KILLER)],
    );
    realm.insert_row(
        "game_creature_quest_tap_member",
        &[("creature_guid", &wolf), ("character_guid", &KILLER)],
    );

    realm.assert_call(
        "debug_kill_nearest",
        &[&KILLER.to_string(), &TEST_WOLF_ENTRY.to_string()],
    );

    assert_eq!(xp_of(&realm, KILLER), 146);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_two_member_party_splits_the_kill_by_level() {
    let realm = start("kill-xp-party");
    const HIGH: u64 = 509_7011;
    const LOW: u64 = 509_7012;
    stage(&realm, HIGH, 20);
    stage(&realm, LOW, 18);
    stage_group(&realm, HIGH, &[LOW]);
    let group = realm.query_rows("SELECT group_id FROM game_group")[0]["group_id"].clone();
    let wolf = spawn_wolf(&realm, HIGH, 19);
    realm.insert_row(
        "game_creature_quest_tap",
        &[("creature_guid", &wolf), ("character_guid", &HIGH)],
    );
    realm.insert_row(
        "game_creature_loot_tag_group",
        &[("creature_guid", &wolf), ("group_id", &group)],
    );
    for member in [HIGH, LOW] {
        realm.insert_row(
            "game_creature_quest_tap_member",
            &[("creature_guid", &wolf), ("character_guid", &member)],
        );
    }

    realm.assert_call(
        "debug_kill_nearest",
        &[&HIGH.to_string(), &TEST_WOLF_ENTRY.to_string()],
    );

    assert_eq!(xp_of(&realm, HIGH), 69);
    assert_eq!(xp_of(&realm, LOW), 62);
}
