mod support;

use support::{number, Row, Standalone};

fn fixture(name: &str) -> (Standalone, String) {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    assert!(shard
        .query_rows("SELECT guid FROM game_aoi_viewer")
        .is_empty());
    assert!(shard.query_rows("SELECT id FROM game_aoi_grant").is_empty());
    shard.assert_call("debug_register_aoi_viewer", &["1"]);
    teleport(&shard, "0");
    shard.assert_call("debug_spawn_at_feet", &["1", "51000", "0"]);
    let wolf = shard
        .query_rows("SELECT guid FROM game_world_entity WHERE entry = 51000 AND x = 0")
        .remove(0)["guid"]
        .clone();
    (shard, wolf)
}

fn teleport(shard: &Standalone, x: &str) {
    shard.assert_call("debug_teleport", &["1", "0", x, "0", "0", "0"]);
}

fn grants(shard: &Standalone, guid: &str, layer: u8) -> Vec<Row> {
    shard.query_rows(&format!(
        "SELECT * FROM game_aoi_grant WHERE viewer_guid = 1 AND layer = {layer} AND entity_guid = {guid}"
    ))
}

#[test]
#[ignore = "requires the SpacetimeDB CLI and Wasm toolchain"]
fn cell_crossings_keep_surviving_grant_ids_and_remove_departures() {
    let (shard, wolf) = fixture("aoi-crossing");
    let original = number::<u64>(&grants(&shard, &wolf, 0)[0], "id");
    teleport(&shard, "10");
    assert_eq!(number::<u64>(&grants(&shard, &wolf, 0)[0], "id"), original);
    teleport(&shard, "50");
    assert_eq!(number::<u64>(&grants(&shard, &wolf, 0)[0], "id"), original);
    teleport(&shard, "150");
    assert!(grants(&shard, &wolf, 0).is_empty());
    teleport(&shard, "0");
    assert_eq!(grants(&shard, &wolf, 0).len(), 1);
}

#[test]
#[ignore = "requires the SpacetimeDB CLI and Wasm toolchain"]
fn despawn_and_logout_remove_grants_in_the_same_transaction() {
    let (shard, wolf) = fixture("aoi-removal");
    assert_eq!(grants(&shard, &wolf, 0).len(), 1);
    shard.assert_call("debug_clear_creatures", &["0"]);
    assert!(grants(&shard, &wolf, 0).is_empty());
    assert_eq!(grants(&shard, "1", 0).len(), 1);
    shard.assert_call("debug_logout_character", &["1"]);
    assert!(shard.query_rows("SELECT * FROM game_aoi_grant").is_empty());
    assert!(shard.query_rows("SELECT * FROM game_aoi_viewer").is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB CLI and Wasm toolchain"]
fn stealth_revokes_a_grant_and_damage_restores_it() {
    let (shard, wolf) = fixture("aoi-stealth");
    assert_eq!(grants(&shard, &wolf, 0).len(), 1);
    shard.assert_call("debug_force_cast", &[&wolf, "1784"]);
    assert!(grants(&shard, &wolf, 0).is_empty());
    shard.assert_call("debug_apply_damage", &[&wolf, "1", "0"]);
    assert_eq!(grants(&shard, &wolf, 0).len(), 1);
}

#[test]
#[ignore = "requires the SpacetimeDB CLI and Wasm toolchain"]
fn gameobjects_enter_and_leave_with_the_viewer() {
    let (shard, _) = fixture("aoi-gameobject");
    shard.assert_call(
        "debug_spawn_gameobject",
        &[
            "509001", "0", "0", "0", "0", "0", "0", "0", "0", "0", "0", "0",
        ],
    );
    let object = shard
        .query_rows("SELECT guid FROM game_gameobject WHERE template_entry = 509001")
        .remove(0)["guid"]
        .clone();
    let original = number::<u64>(&grants(&shard, &object, 1)[0], "id");
    teleport(&shard, "50");
    assert_eq!(
        number::<u64>(&grants(&shard, &object, 1)[0], "id"),
        original
    );
    teleport(&shard, "150");
    assert!(grants(&shard, &object, 1).is_empty());
    teleport(&shard, "0");
    assert_eq!(grants(&shard, &object, 1).len(), 1);
}
