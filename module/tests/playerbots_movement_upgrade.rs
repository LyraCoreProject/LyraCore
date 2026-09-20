//! A populated Package upgrade must preserve retained movement before continuation starts.

mod support;
use std::collections::BTreeMap;
use std::time::Duration;
use support::{poll_until, Standalone, POLL_TIMEOUT};

fn runner(node: &Standalone, bot: &str) -> BTreeMap<String, String> {
    node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_runner WHERE character_guid = {bot}"
    ))
    .into_iter()
    .next()
    .expect("runner missing")
}

fn position(node: &Standalone, bot: &str) -> f32 {
    node.query_rows(&format!(
        "SELECT x FROM game_world_entity WHERE guid = {bot}"
    ))[0]["x"]
        .parse()
        .unwrap()
}

#[test]
#[ignore = "requires preceding movement Wasm, SpacetimeDB, and the playerbots Package"]
fn playerbots_movement_upgrade_preserves_the_retained_destination() {
    let previous = std::env::var_os("PLAYERBOTS_MOVEMENT_PRECEDING_WASM")
        .expect("PLAYERBOTS_MOVEMENT_PRECEDING_WASM must name the preceding Module");
    let mut node = Standalone::start("playerbots-movement-upgrade");
    node.publish_module_bytes(&std::fs::read(previous).unwrap());
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("playerbots_spawn_role", &["1", "1200", "1200", "50", "1"]);
    node.assert_call("playerbots_fixture_prepare", &[]);
    let bot = node.query_rows("SELECT character_guid FROM pkg_playerbots_bot")[0]["character_guid"]
        .clone();
    node.assert_call("playerbots_fixture_runner_stage", &[&bot, "false"]);
    for controller in [r#"{"frozen":[]}"#, r#"{"cohort":[]}"#] {
        node.assert_call("playerbots_select_controller", &[&bot, controller]);
    }
    assert!(poll_until(POLL_TIMEOUT, || runner(&node, &bot)
        ["foreground"]
        .contains("movement")));
    node.assert_call("playerbots_fixture_runner_stage", &[&bot, "false"]);
    let before = runner(&node, &bot);
    assert!(!before.contains_key("movement_due_micros"));
    node.publish_module();
    let migrated = runner(&node, &bot);
    for (field, value) in &before {
        assert_eq!(migrated[field], *value, "migration changed {field}");
    }
    assert_eq!(migrated["movement_due_micros"], i64::MAX.to_string());
    std::thread::sleep(Duration::from_secs(1));
    node.assert_call("playerbots_fixture_runner_pass_once", &[&bot]);
    assert!(
        runner(&node, &bot)["movement_due_micros"]
            .parse::<i64>()
            .unwrap()
            < i64::MAX
    );
    assert!(poll_until(Duration::from_secs(6), || (position(
        &node, &bot
    ) - 1238.0)
        .abs()
        < 0.1));
    std::fs::write(
        support::log_dir().join(format!("{}-migration.json", node.shard_name())),
        serde_json::to_vec_pretty(&serde_json::json!({"before": before, "migrated": migrated,
            "arrived": runner(&node, &bot)}))
        .unwrap(),
    )
    .unwrap();
}

#[test]
#[ignore = "requires preceding movement Wasm, SpacetimeDB, and the playerbots Package"]
fn playerbots_waypoint_upgrade_preserves_an_existing_single_leg() {
    let previous = std::env::var_os("PLAYERBOTS_MOVEMENT_PRECEDING_WASM")
        .expect("PLAYERBOTS_MOVEMENT_PRECEDING_WASM must name the preceding Module");
    let mut node = Standalone::start("playerbots-waypoint-upgrade");
    node.publish_module_bytes(&std::fs::read(previous).unwrap());
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("playerbots_spawn_role", &["1", "1200", "1200", "50", "1"]);
    node.assert_call("playerbots_fixture_prepare", &[]);
    let bot = node.query_rows("SELECT character_guid FROM pkg_playerbots_bot")[0]["character_guid"]
        .clone();
    node.assert_sql("DELETE FROM game_creature_move_schedule");
    node.assert_call("playerbots_fixture_runner_stage", &[&bot, "false"]);
    node.assert_call("playerbots_fixture_move", &[&bot, "1240"]);
    let before = node.query_rows(&format!(
        "SELECT * FROM game_creature_spline WHERE guid = {bot}"
    ));
    assert_eq!(before.len(), 1);
    assert!(!before[0].contains_key("path"));
    let characters = node.query_rows("SELECT guid, name FROM game_character");
    node.publish_module();
    let migrated = node.query_rows(&format!(
        "SELECT * FROM game_creature_spline WHERE guid = {bot}"
    ));
    assert_eq!(migrated.len(), 1);
    for (field, value) in &before[0] {
        assert_eq!(migrated[0][field], *value, "migration changed {field}");
    }
    assert!(migrated[0]["path"].contains("none"));
    assert_eq!(
        node.query_rows("SELECT guid, name FROM game_character"),
        characters
    );
    std::fs::write(
        support::log_dir().join(format!("{}-migration.json", node.shard_name())),
        serde_json::to_vec_pretty(&serde_json::json!({"before": before, "migrated": migrated}))
            .unwrap(),
    )
    .unwrap();
}
