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

    let model_hex = lyracore_shared::vmap::encode(&[])
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    fixture.assert_call(
        "import_go_models_append",
        &[&serde_json::to_string(&format!("709113,1,0,{model_hex}")).unwrap()],
    );
    let templates = fixture.query_rows("SELECT entry, type_id FROM game_gameobject_template");
    let template = templates.first().expect("a seeded GameObject template");
    let package = serde_json::json!({
        "version": 1,
        "package": "fixture.preserved",
        "source_hash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "claims": [{
            "table": "game_gameobject_template",
            "key": {"entry": template["entry"].parse::<u32>().unwrap()},
            "operation": "update",
            "fields": {"type_id": {"type": "u8", "value": template["type_id"].parse::<u8>().unwrap()}}
        }]
    });
    fixture.assert_call(
        "apply_package_deltas",
        &[
            "\"gameobjects\"",
            &serde_json::to_string(&package.to_string()).unwrap(),
        ],
    );
    let preserved = [
        "SELECT * FROM game_gameobject",
        "SELECT * FROM game_gameobject_template",
        "SELECT * FROM game_go_model",
        "SELECT * FROM game_world_entity WHERE guid = 1",
        "SELECT * FROM game_character WHERE guid = 1",
        "SELECT * FROM game_config",
        "SELECT * FROM game_package_import",
    ]
    .map(|query| {
        let mut rows = fixture.query_rows(query);
        assert!(!rows.is_empty(), "seeded rows required for {query}");
        rows.sort();
        (query, rows)
    });

    let result = fixture.call("debug_assert_go_collision", &[]);
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!result.status.success(), "fixture must roll back");
    assert!(
        output.contains("GO_COLLISION_FIXTURE_PASSED_ROLLED_BACK"),
        "fixture did not reach its success marker: {output}"
    );
    for (query, expected) in preserved {
        let mut actual = fixture.query_rows(query);
        actual.sort();
        assert_eq!(actual, expected, "fixture changed rows in {query}");
    }

    fixture.assert_call("debug_vmap_ray", &["36", "0", "0.5", "0", "4", "0.5", "0"]);
    fixture.assert_call(
        "debug_vmap_ray_instance",
        &["36", "0", "0.5", "0", "4", "0.5", "0", "0"],
    );
    assert!(fixture
        .query_rows("SELECT * FROM game_go_collider")
        .is_empty());
}
