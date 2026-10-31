mod support;

use serde_json::{json, Value};
use support::{number, poll_until, Standalone, POLL_TIMEOUT};

const CHICKEN_ENTRY: u32 = 620;
const CHICKEN: u64 = (0xF130_u64 << 48) | ((CHICKEN_ENTRY as u64) << 24) | 1;
const CHEST: u64 = (0xF110_u64 << 48) | 1;
const FIRST_OPERATOR_CHICKEN: u64 =
    (0xF130_u64 << 48) | ((CHICKEN_ENTRY as u64) << 24) | 14_000_000;
const FIRST_OPERATOR_GAMEOBJECT: u64 = (0xF110_u64 << 48) | 14_000_000;

fn realm(test_name: &str) -> Standalone {
    let mut shard = Standalone::start(test_name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard
}

fn spawn_row(shard: &Standalone, guid: u64) -> Option<support::Row> {
    shard
        .query_rows(&format!(
            "SELECT * FROM game_creature_spawn WHERE guid = {guid}"
        ))
        .pop()
}

fn entity_row(shard: &Standalone, guid: u64) -> Option<support::Row> {
    shard
        .query_rows(&format!(
            "SELECT * FROM game_world_entity WHERE guid = {guid}"
        ))
        .pop()
}

fn gameobject_row(shard: &Standalone, guid: u64) -> Option<support::Row> {
    shard
        .query_rows(&format!(
            "SELECT * FROM game_gameobject WHERE guid = {guid}"
        ))
        .pop()
}

fn waypoint_count(shard: &Standalone, guid: u64) -> usize {
    shard
        .query_rows(&format!(
            "SELECT id FROM game_creature_waypoint WHERE creature_guid = {guid}"
        ))
        .len()
}

fn chicken_home(shard: &Standalone) -> (u32, f32, f32, f32) {
    let row = spawn_row(shard, CHICKEN).expect("the seeded chicken spawn");
    (
        number(&row, "map_id"),
        number(&row, "x"),
        number(&row, "y"),
        number(&row, "z"),
    )
}

fn placement(map_id: u32, x: f32, y: f32, z: f32) -> Value {
    json!({
        "map_id": map_id,
        "x": x,
        "y": y,
        "z": z,
        "orientation": 1.5,
        "movement_type": 0,
        "respawn_secs": 120,
    })
}

fn chest_fields(map_id: u32, x: f32, y: f32, z: f32) -> Value {
    json!({
        "template_entry": 50100,
        "map_id": map_id,
        "x": x,
        "y": y,
        "z": z,
        "orientation": 0.5,
        "state": 0,
        "rotation_0": 0.0,
        "rotation_1": 0.0,
        "rotation_2": 0.25,
        "rotation_3": 0.97,
    })
}

fn refusal(shard: &Standalone, reducer: &str, args: &[&str]) -> String {
    let output = shard.call(reducer, args);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success(), "{reducer} succeeded: {text}");
    text
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_updated_creature_spawn_respawns_at_its_new_place_in_a_new_life() {
    let shard = realm("operator-content-update");
    let (map_id, x, y, z) = chicken_home(&shard);
    let life_before: u64 = number(&spawn_row(&shard, CHICKEN).unwrap(), "life_seq");
    assert!(entity_row(&shard, CHICKEN).is_some());

    let moved = placement(map_id, x + 20.0, y - 5.0, z);
    shard.assert_call(
        "operator_update_creature_spawn",
        &[&CHICKEN.to_string(), &moved.to_string()],
    );

    let spawn = spawn_row(&shard, CHICKEN).unwrap();
    assert_eq!(number::<f32>(&spawn, "x"), x + 20.0);
    assert_eq!(number::<u32>(&spawn, "respawn_secs"), 120);
    assert_eq!(number::<u8>(&spawn, "movement_type"), 0);
    assert!(poll_until(POLL_TIMEOUT, || {
        entity_row(&shard, CHICKEN)
            .is_some_and(|entity| (number::<f32>(&entity, "x") - (x + 20.0)).abs() < 0.01)
    }));
    let life_after: u64 = number(&spawn_row(&shard, CHICKEN).unwrap(), "life_seq");
    assert_eq!(life_after, life_before + 1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_created_creature_spawn_takes_the_operator_range_and_appears() {
    let shard = realm("operator-content-create");
    let (map_id, x, y, z) = chicken_home(&shard);

    shard.assert_call(
        "operator_create_creature_spawn",
        &[
            &CHICKEN_ENTRY.to_string(),
            &placement(map_id, x - 10.0, y, z).to_string(),
        ],
    );
    let spawn = spawn_row(&shard, FIRST_OPERATOR_CHICKEN).expect("the first operator spawn");
    assert_eq!(number::<u64>(&spawn, "instance_id"), 0);
    assert_eq!(number::<u64>(&spawn, "source_guid"), 0);
    assert!(poll_until(POLL_TIMEOUT, || entity_row(
        &shard,
        FIRST_OPERATOR_CHICKEN
    )
    .is_some()));

    let points = json!([
        {"x": x - 10.0, "y": y, "z": z},
        {"x": x - 10.0, "y": y + 6.0, "z": z},
        {"x": x - 4.0, "y": y + 6.0, "z": z},
    ]);
    shard.assert_call(
        "operator_set_creature_waypoints",
        &[&FIRST_OPERATOR_CHICKEN.to_string(), &points.to_string()],
    );
    assert_eq!(waypoint_count(&shard, FIRST_OPERATOR_CHICKEN), 3);
    assert!(poll_until(POLL_TIMEOUT, || entity_row(
        &shard,
        FIRST_OPERATOR_CHICKEN
    )
    .is_some()));

    shard.assert_call(
        "operator_create_creature_spawn",
        &[
            &CHICKEN_ENTRY.to_string(),
            &placement(map_id, x - 12.0, y, z).to_string(),
        ],
    );
    let newest = shard.query_rows(&format!(
        "SELECT guid FROM game_creature_spawn WHERE guid >= {FIRST_OPERATOR_CHICKEN} \
         AND guid <= {}",
        FIRST_OPERATOR_CHICKEN + 999_999
    ));
    let mut guids: Vec<u64> = newest.iter().map(|row| number(row, "guid")).collect();
    guids.sort_unstable();
    assert_eq!(
        guids,
        [FIRST_OPERATOR_CHICKEN, FIRST_OPERATOR_CHICKEN + 1],
        "the newest spawn has the highest guid in the range"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_deleted_creature_spawn_leaves_no_entity_route_or_row() {
    let shard = realm("operator-content-delete");
    assert!(entity_row(&shard, CHICKEN).is_some());
    assert!(waypoint_count(&shard, CHICKEN) > 0);

    shard.assert_call("operator_delete_creature_spawn", &[&CHICKEN.to_string()]);

    assert!(spawn_row(&shard, CHICKEN).is_none());
    assert!(entity_row(&shard, CHICKEN).is_none());
    assert_eq!(waypoint_count(&shard, CHICKEN), 0);
    let text = refusal(
        &shard,
        "operator_delete_creature_spawn",
        &[&CHICKEN.to_string()],
    );
    assert!(text.contains("does not exist"), "{text}");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn spawn_edits_refuse_bad_targets_and_bad_input() {
    let shard = realm("operator-content-refusals");
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    let (map_id, x, y, z) = chicken_home(&shard);
    let good = placement(map_id, x, y, z).to_string();

    let cases: [(&str, Vec<String>, &str); 9] = [
        (
            "operator_delete_creature_spawn",
            vec!["1".into()],
            "not a creature spawn guid",
        ),
        (
            "operator_update_creature_spawn",
            vec![(CHICKEN + 1).to_string(), good.clone()],
            "does not exist",
        ),
        (
            "operator_update_creature_spawn",
            vec![
                CHICKEN.to_string(),
                placement(map_id, 20_000.0, y, z).to_string(),
            ],
            "outside the map bounds",
        ),
        (
            "operator_update_creature_spawn",
            vec![CHICKEN.to_string(), placement(4_242, x, y, z).to_string()],
            "map 4242 is not loaded",
        ),
        (
            "operator_update_creature_spawn",
            vec![CHICKEN.to_string(), {
                let mut patrol = placement(map_id, x, y, z);
                patrol["movement_type"] = json!(3);
                patrol.to_string()
            }],
            "movement type 3",
        ),
        (
            "operator_create_creature_spawn",
            vec!["999999".into(), good.clone()],
            "creature template 999999 does not exist",
        ),
        (
            "operator_delete_gameobject",
            vec![CHICKEN.to_string()],
            "not a gameobject spawn guid",
        ),
        (
            "operator_create_gameobject",
            vec![{
                let mut fields = chest_fields(map_id, x, y, z);
                fields["state"] = json!(2);
                fields.to_string()
            }],
            "gameobject state 2",
        ),
        (
            "operator_update_creature_spawn",
            vec![
                (((0xF130_u64 << 48) | ((CHICKEN_ENTRY as u64) << 24)) | 15_000_000).to_string(),
                good.clone(),
            ],
            "Package Creature Range",
        ),
    ];
    let spawns_before = shard
        .query_rows("SELECT guid FROM game_creature_spawn")
        .len();
    for (reducer, args, expected) in &cases {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let text = refusal(&shard, reducer, &args);
        assert!(text.contains(expected), "{reducer}: {text}");
    }
    assert_eq!(
        shard
            .query_rows("SELECT guid FROM game_creature_spawn")
            .len(),
        spawns_before
    );
    assert!(
        entity_row(&shard, 1).is_some(),
        "the player stays in the world"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_population_copy_is_not_an_authored_spawn() {
    let shard = realm("operator-content-population-copy");
    shard.assert_sql(&format!(
        "UPDATE game_creature_spawn SET source_guid = 1 WHERE guid = {CHICKEN}"
    ));
    let text = refusal(
        &shard,
        "operator_delete_creature_spawn",
        &[&CHICKEN.to_string()],
    );
    assert!(text.contains("not an authored creature spawn"), "{text}");
    assert!(entity_row(&shard, CHICKEN).is_some());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn gameobject_spawn_edits_move_create_and_remove_the_object() {
    let shard = realm("operator-content-gameobject");
    let chest = gameobject_row(&shard, CHEST).expect("the seeded chest");
    let (map_id, x, y, z): (u32, f32, f32, f32) = (
        number(&chest, "map_id"),
        number(&chest, "x"),
        number(&chest, "y"),
        number(&chest, "z"),
    );
    let far_x = x + 120.0;

    shard.assert_call(
        "operator_update_gameobject",
        &[
            &CHEST.to_string(),
            &chest_fields(map_id, far_x, y, z).to_string(),
        ],
    );
    let moved = gameobject_row(&shard, CHEST).unwrap();
    assert_eq!(number::<f32>(&moved, "x"), far_x);
    assert_eq!(number::<i32>(&moved, "grid_x"), 517);
    assert_eq!(number::<i32>(&moved, "grid_y"), 343);
    assert_eq!(number::<i64>(&moved, "cell"), (517 << 32) | 343);
    assert_eq!(number::<f32>(&moved, "rotation_2"), 0.25);

    shard.assert_call(
        "operator_create_gameobject",
        &[&chest_fields(map_id, x, y + 3.0, z).to_string()],
    );
    assert!(gameobject_row(&shard, FIRST_OPERATOR_GAMEOBJECT).is_some());

    shard.assert_call("operator_delete_gameobject", &[&CHEST.to_string()]);
    assert!(gameobject_row(&shard, CHEST).is_none());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_spawn_edit_batch_applies_whole_or_not_at_all() {
    let shard = realm("operator-content-batch");
    let (map_id, x, y, z) = chicken_home(&shard);
    let create = json!({"createCreature": {
        "entry": CHICKEN_ENTRY,
        "placement": placement(map_id, x - 10.0, y, z),
    }});
    let create_chest = json!({"createGameObject": chest_fields(map_id, x, y + 3.0, z)});

    let refused = json!([create.clone(), create_chest.clone(), {"deleteCreature": CHICKEN + 1}]);
    let text = refusal(
        &shard,
        "operator_apply_spawn_edits",
        &[&refused.to_string()],
    );
    assert!(text.contains("spawn edit 2"), "{text}");
    assert!(spawn_row(&shard, FIRST_OPERATOR_CHICKEN).is_none());
    assert!(gameobject_row(&shard, FIRST_OPERATOR_GAMEOBJECT).is_none());

    let accepted = json!([
        create.clone(),
        create.clone(),
        create_chest,
        {"updateCreature": {"guid": CHICKEN, "placement": placement(map_id, x + 1.0, y, z)}},
        {"deleteGameObject": CHEST},
    ]);
    shard.assert_call("operator_apply_spawn_edits", &[&accepted.to_string()]);
    assert!(spawn_row(&shard, FIRST_OPERATOR_CHICKEN).is_some());
    assert!(spawn_row(&shard, FIRST_OPERATOR_CHICKEN + 1).is_some());
    assert!(gameobject_row(&shard, FIRST_OPERATOR_GAMEOBJECT).is_some());
    assert!(gameobject_row(&shard, CHEST).is_none());
    assert_eq!(
        number::<f32>(&spawn_row(&shard, CHICKEN).unwrap(), "x"),
        x + 1.0
    );

    let too_many = Value::Array(vec![create; 257]);
    let text = refusal(
        &shard,
        "operator_apply_spawn_edits",
        &[&too_many.to_string()],
    );
    assert!(text.contains("at most 256 spawn edits"), "{text}");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_instance_copy_skips_the_guid_of_a_dormant_operator_spawn() {
    let shard = realm("operator-content-instance-copy");
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    let (map_id, x, y, z) = chicken_home(&shard);
    shard.assert_call(
        "operator_create_creature_spawn",
        &[
            &CHICKEN_ENTRY.to_string(),
            &placement(map_id, x - 10.0, y, z).to_string(),
        ],
    );
    assert!(poll_until(POLL_TIMEOUT, || entity_row(
        &shard,
        FIRST_OPERATOR_CHICKEN
    )
    .is_some()));
    shard.assert_sql("INSERT INTO game_creature_spawn_map (map_id) VALUES (36)");
    shard.assert_call(
        "operator_create_creature_spawn",
        &[
            &CHICKEN_ENTRY.to_string(),
            &placement(36, -16.0, -383.0, 62.0).to_string(),
        ],
    );
    let dormant = FIRST_OPERATOR_CHICKEN + 1;
    assert!(spawn_row(&shard, dormant).is_some());
    assert!(entity_row(&shard, dormant).is_none());

    shard.assert_call("debug_create_fixture_instance", &["1"]);

    let copies = shard.query_rows(&format!(
        "SELECT guid FROM game_creature_spawn WHERE source_guid = {FIRST_OPERATOR_CHICKEN}"
    ));
    assert_eq!(copies.len(), 1, "{copies:?}");
    assert_ne!(number::<u64>(&copies[0], "guid"), dormant);
    assert_eq!(
        number::<u32>(&spawn_row(&shard, dormant).unwrap(), "map_id"),
        36
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_population_member_can_move_but_not_be_deleted() {
    let shard = realm("operator-content-population-member");
    shard.assert_sql(&format!(
        "INSERT INTO game_creature_population_member (source_guid, population_id, entry, \
         choices, chance_bp, event_ids) VALUES ({CHICKEN}, 7, 0, '', 0, '')"
    ));
    let text = refusal(
        &shard,
        "operator_delete_creature_spawn",
        &[&CHICKEN.to_string()],
    );
    assert!(text.contains("member of population 7"), "{text}");
    assert!(spawn_row(&shard, CHICKEN).is_some());

    let (map_id, x, y, z) = chicken_home(&shard);
    shard.assert_call(
        "operator_update_creature_spawn",
        &[
            &CHICKEN.to_string(),
            &placement(map_id, x + 2.0, y, z).to_string(),
        ],
    );
    assert_eq!(
        number::<f32>(&spawn_row(&shard, CHICKEN).unwrap(), "x"),
        x + 2.0
    );
}
