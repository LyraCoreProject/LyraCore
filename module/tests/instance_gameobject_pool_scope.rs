mod support;

use support::Standalone;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn imported_instance_maps_exclude_world_gameobject_pools_without_terrain() {
    let mut standalone = Standalone::start("instance-gameobject-pool-scope");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_repair_after_publish", &[]);
    assert!(!standalone
        .query_rows("SELECT point_id FROM game_gameobject_pool_member WHERE map_id = 0")
        .is_empty());
    assert!(standalone
        .query_rows("SELECT map_id FROM game_terrain_chunk")
        .is_empty());
    assert!(standalone
        .query_rows("SELECT map_id FROM game_nav_chunk")
        .is_empty());

    standalone.assert_sql("INSERT INTO game_creature_spawn_map (map_id) VALUES (36)");
    standalone.assert_sql("DELETE FROM game_gameobject WHERE map_id = 0");
    standalone.assert_call("arm_all_pools", &[]);

    assert!(
        standalone
            .query_rows("SELECT guid FROM game_gameobject WHERE map_id = 0")
            .is_empty(),
        "world pool members must remain dormant on an imported instance map"
    );
}
