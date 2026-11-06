mod support;

use support::{event_rows, fence_subscription, subscribe_with_fence, Standalone};

fn stage(name: &str) -> (Standalone, String) {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard.assert_sql("UPDATE game_world_entity SET unit_bytes_0 = 2049, level = 60, intellect = 125, godmode = true WHERE guid = 1");
    shard.assert_call("debug_spawn_at_feet", &["1", "51000", "2"]);
    let target = shard
        .query_rows("SELECT guid FROM game_world_entity WHERE entry = 51000")
        .into_iter()
        .map(|row| row["guid"].parse::<u64>().unwrap())
        .max()
        .unwrap()
        .to_string();
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET level = 63, simulation_indexed = false WHERE guid = {target}"
    ));
    (shard, target)
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn mage_spell_crit_uses_its_class_and_level() {
    let (shard, target) = stage("mage-spell-crit");
    shard.assert_call("debug_compute_spell", &["1", &target, "133"]);
    assert_eq!(
        shard.query_rows("SELECT spell_crit_bp FROM game_debug_readout WHERE key = 'spell'")[0]
            ["spell_crit_bp"],
        "602"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn creature_levels_add_partial_resistance_without_gear() {
    let (shard, target) = stage("innate-spell-resistance");
    shard.assert_sql("UPDATE game_world_entity SET intellect = 0 WHERE guid = 1");
    shard.assert_sql("UPDATE game_spell_effect SET base_points = 400 WHERE spell_id = 133");
    shard.assert_sql(&format!("INSERT INTO game_conformance_spell_random (character_guid,target_guid,state) VALUES (1,{target},6)"));
    shard.assert_call("debug_compute_spell", &["1", &target, "133"]);
    assert_eq!(
        shard.query_rows("SELECT spell_hit_normal FROM game_debug_readout WHERE key = 'spell'")[0]
            ["spell_hit_normal"],
        "100"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn binary_damage_skips_the_innate_partial_resistance_roll() {
    use lyracore_shared::spell::taxonomy::SPELL_ATTR_BINARY;

    let (shard, target) = stage("binary-spell-resistance");
    shard.assert_sql("UPDATE game_world_entity SET intellect = 0 WHERE guid = 1");
    shard.assert_sql("UPDATE game_spell_effect SET base_points = 400 WHERE spell_id = 133");
    shard.assert_sql(&format!("INSERT INTO game_conformance_spell_random (character_guid,target_guid,state) VALUES (1,{target},6)"));
    shard.assert_sql(&format!(
        "UPDATE game_spell SET cast_flags = {SPELL_ATTR_BINARY} WHERE spell_id = 133"
    ));
    shard.assert_call("debug_compute_spell", &["1", &target, "133"]);
    assert_eq!(
        shard.query_rows("SELECT spell_hit_normal FROM game_debug_readout WHERE key = 'spell'")[0]
            ["spell_hit_normal"],
        "400"
    );
    shard.assert_sql("UPDATE game_spell SET cast_flags = 0 WHERE spell_id = 133");
    shard.assert_sql("UPDATE game_conformance_spell_random SET state = 6 WHERE character_guid = 1");
    shard.assert_call("debug_compute_spell", &["1", &target, "133"]);
    assert_eq!(
        shard.query_rows("SELECT spell_hit_normal FROM game_debug_readout WHERE key = 'spell'")[0]
            ["spell_hit_normal"],
        "100"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_partial_resist_that_removes_a_small_area_tick_still_reports_its_log() {
    let (shard, target) = stage("resisted-area-log");
    shard.assert_sql("INSERT INTO game_faction_template (id,faction,faction_group,friend_group,enemy_group,enemy_0,enemy_1,enemy_2,enemy_3,friend_0,friend_1,friend_2,friend_3) VALUES (5090300,5090300,1,1,2,0,0,0,0,0,0,0,0),(5090301,5090301,2,2,1,0,0,0,0,0,0,0,0)");
    shard.assert_sql("UPDATE game_world_entity SET faction_template = 5090300 WHERE guid = 1");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET faction_template = 5090301 WHERE guid = {target}"
    ));
    shard.assert_sql("UPDATE game_spell SET duration_ms = 2000 WHERE spell_id = 133");
    shard.assert_sql("UPDATE game_spell_effect SET base_points = 1, period_ms = 500, radius_yd = 20.0 WHERE spell_id = 133");
    shard.assert_sql(&format!("INSERT INTO game_conformance_spell_random (character_guid,target_guid,state) VALUES (1,{target},149)"));
    let events = subscribe_with_fence(&shard, &["SELECT * FROM game_spell_cast_event_live"]);
    shard.assert_call("debug_spawn_ground_area", &["1", "133"]);
    shard.assert_call("debug_advance_roll_batch", &["10"]);
    fence_subscription(&shard, &events);
    assert_ne!(
        shard
            .query_rows("SELECT state FROM game_conformance_spell_random WHERE character_guid = 1")
            [0]["state"],
        "149"
    );
    let logs: Vec<_> = event_rows(&events, "game_spell_cast_event_live")
        .into_iter()
        .filter(|row| row["spell_id"] == "133" && row["target_guid"] == target)
        .collect();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0]["damage"], "0");
    assert_eq!(logs[0]["resisted"], "1");
}
