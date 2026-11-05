mod support;

use lyracore_shared::constants::{taxi_fixture as taxi, unit_flags};
use support::Standalone;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_taxi_passenger_leaves_combat_and_cannot_be_targeted_until_landing() {
    let mut shard = Standalone::start("taxi-targeting");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard.assert_call("debug_spawn_at_feet", &["1", "51000", "2"]);
    let wolf = shard
        .query_rows("SELECT guid FROM game_world_entity WHERE entry = 51000")
        .pop()
        .unwrap()["guid"]
        .clone();
    shard.assert_sql(
        "UPDATE game_spell SET cast_time_ms = 0, gcd_ms = 0, cost = 0 WHERE spell_id = 116",
    );
    shard.assert_call("debug_engage", &[&wolf, "1"]);
    shard.assert_sql(&format!(
        "INSERT INTO game_active_taxi_flight \
         (character_guid,path_id,source_node_id,destination_node_id,mount_display_id,fare,\
          current_node_index,started_micros,origin_x,origin_y,origin_z,origin_recorded) \
         VALUES (1,{},{},{},1147,25,0,0,0,0,0,false)",
        taxi::PATH_ID,
        taxi::SOURCE_NODE_STORAGE_ID,
        taxi::DESTINATION_NODE_STORAGE_ID
    ));
    shard.assert_call("gw_arm_taxi_flight", &[r#"{"guid":1,"ownership":null}"#]);
    assert!(
        shard
            .query_rows("SELECT attacker_guid FROM game_melee_attack WHERE target_guid = 1")
            .is_empty(),
        "takeoff must stop attacks already aimed at the passenger"
    );
    let before = shard.query_rows("SELECT health FROM game_world_entity WHERE guid = 1");
    let refused = shard.call("debug_begin_cast", &[&wolf, "116", "1"]);
    assert!(
        !refused.status.success(),
        "a hostile spell must refuse a taxi passenger"
    );
    shard.assert_call("debug_advance_virtual_clock", &["1000000"]);
    assert!(
        shard
            .query_rows("SELECT attacker_guid FROM game_melee_attack WHERE target_guid = 1")
            .is_empty(),
        "nearby creatures must not acquire a taxi passenger"
    );
    assert_eq!(
        shard.query_rows("SELECT health FROM game_world_entity WHERE guid = 1"),
        before
    );
    assert!(shard
        .query_rows("SELECT id FROM game_aura WHERE target_guid = 1 AND spell_id = 116")
        .is_empty());

    shard.assert_sql("DELETE FROM game_active_taxi_flight WHERE character_guid = 1");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_flags = {} WHERE guid = 1",
        unit_flags::PLAYER_CONTROLLED
    ));
    shard.assert_call("debug_begin_cast", &[&wolf, "116", "1"]);
}
