mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use support::Standalone;

fn chances(shard: &Standalone, attacker: &str, target: &str) -> [u32; 5] {
    shard.assert_call("debug_compute_swing", &[attacker, target]);
    let row = &shard.query_rows("SELECT * FROM game_debug_readout WHERE key = 'swing'")[0];
    ["hit_miss_bp", "dodge_bp", "parry_bp", "block_bp", "crit_bp"]
        .map(|field| row[field].parse().unwrap())
}

fn sheet_crit(shard: &Standalone) -> String {
    shard.query_rows("SELECT sheet_crit_bp FROM game_world_entity WHERE guid = 1")[0]
        ["sheet_crit_bp"]
        .clone()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn white_melee_chances_follow_catalogue_skills_and_defender_eligibility() {
    let mut shard = Standalone::start("white-melee-inputs");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_sql("DELETE FROM game_item_instance WHERE owner_guid = 1");
    shard.assert_sql("UPDATE game_character SET class = 1, level = 60 WHERE guid = 1");
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_sql("UPDATE game_world_entity SET simulation_indexed = false, level = 60, agility = 100, x = 0, y = 0, orientation = 0 WHERE guid = 1");
    shard.assert_call("debug_set_skill", &["1", "162", "300"]);
    shard.assert_call("debug_set_skill", &["1", "95", "300"]);
    shard.assert_call(
        "debug_spawn_at_feet",
        &["1", &TEST_WOLF_ENTRY.to_string(), "2"],
    );
    let target = shard.query_rows(&format!(
        "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
    ))[0]["guid"]
        .clone();
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, level = 60, agility = 0, x = 2, y = 0, orientation = 3.1415927 WHERE guid = {target}"
    ));
    assert_eq!(chances(&shard, "1", &target), [500, 500, 500, 500, 500]);
    assert_eq!(sheet_crit(&shard), "500");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET level = 63 WHERE guid = {target}"
    ));
    assert_eq!(chances(&shard, "1", &target), [800, 650, 1400, 500, 200]);
    assert_eq!(sheet_crit(&shard), "500");
    shard.assert_call("debug_set_skill", &["1", "162", "295"]);
    assert_eq!(sheet_crit(&shard), "480");
    assert_eq!(chances(&shard, "1", &target), [900, 700, 1700, 500, 80]);
    shard.assert_call("debug_set_skill", &["1", "162", "300"]);
    assert_eq!(sheet_crit(&shard), "500");
    shard.insert_row(
        "game_creature_defense",
        &[
            ("entry", &TEST_WOLF_ENTRY),
            ("no_parry", &true),
            ("no_block", &true),
        ],
    );
    assert_eq!(chances(&shard, "1", &target), [800, 650, 0, 0, 200]);
    shard.assert_sql(
        &format!("UPDATE game_creature_defense SET no_parry = false, no_block = false WHERE entry = {TEST_WOLF_ENTRY}"),
    );
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET orientation = 0 WHERE guid = {target}"
    ));
    assert_eq!(chances(&shard, "1", &target), [800, 650, 0, 0, 200]);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET orientation = 3.1415927 WHERE guid = {target}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_creature_template SET creature_type = 11 WHERE entry = {TEST_WOLF_ENTRY}"
    ));
    assert_eq!(chances(&shard, "1", &target)[3], 0);
    shard.assert_sql(&format!(
        "UPDATE game_creature_template SET creature_type = 1 WHERE entry = {TEST_WOLF_ENTRY}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_creature_defense SET no_defense = true WHERE entry = {TEST_WOLF_ENTRY}"
    ));
    assert_eq!(chances(&shard, "1", &target)[0], 0);
    shard.assert_sql(&format!(
        "UPDATE game_creature_defense SET no_defense = false WHERE entry = {TEST_WOLF_ENTRY}"
    ));
    shard.insert_row(
        "game_crit_per_agility",
        &[
            ("class_level", &2108),
            ("class", &8),
            ("level", &60),
            ("rate", &19.4553),
        ],
    );
    shard.assert_sql("UPDATE game_world_entity SET unit_bytes_0 = 2049 WHERE guid = 1");
    shard.assert_call("debug_set_skill", &["1", "162", "300"]);
    assert_eq!(sheet_crit(&shard), "833");
    assert_eq!(chances(&shard, "1", &target)[4], 533);
    shard.assert_sql("UPDATE game_world_entity SET unit_bytes_0 = 16777473 WHERE guid = 1");
    shard.assert_call("debug_set_skill", &["1", "162", "300"]);
    let incoming = chances(&shard, &target, "1");
    assert_eq!([incoming[0], incoming[4]], [440, 560]);
    let pvp = chances(&shard, "1", "1");
    assert_eq!([pvp[0], pvp[4]], [500, 500]);
}
