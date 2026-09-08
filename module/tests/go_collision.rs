//! Real Module registration, instance lifecycle and gameplay ray queries on a private fixture.
mod support;

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn closed_doors_block_only_their_own_instance() {
    let mut fixture = support::Standalone::start("go-collision");
    fixture.publish_module();
    fixture.assert_call("claim_operator", &[]);
    fixture.assert_call("debug_spawn_player_entity", &["1"]);
    fixture.assert_call("debug_assert_unreachable_goal_stops_at_wall", &["1"]);
    fixture.assert_call("debug_assert_go_collision", &[]);
    fixture.assert_call("debug_vmap_ray", &["36", "0", "0.5", "0", "4", "0.5", "0"]);
    fixture.assert_call(
        "debug_vmap_ray_instance",
        &["36", "0", "0.5", "0", "4", "0.5", "0", "0"],
    );
    assert!(fixture
        .query_rows("SELECT * FROM game_go_collider")
        .is_empty());
}
