mod support;

use support::{actor, number, Row, Standalone};

const PACKAGE: &str = "\"fixture.defense\"";
const SOURCE: u64 = (0xF130u64 << 48) | (51000u64 << 24) | 1_000_189;
const DOOR: u64 = (0xF110u64 << 48) | 509_9190;

fn start(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_stage_meeting_stone_group", &["1", "[]", "false"]);
    shard.assert_call("debug_stage_meeting_stone_group", &["2", "[]", "false"]);
    shard.assert_call(
        "import_creature_spawns",
        &[&serde_json::to_string(&format!("{SOURCE},51000,189,10,20,30,0,2,1")).unwrap()],
    );
    shard.assert_sql("INSERT INTO game_gameobject_template (entry,type_id,display_id,name,data0,data1,gather_skill_line,respawn_secs,gather_gray,lock_id,size) VALUES (5099190,0,0,'Fixture door',0,0,0,0,0,0,1)");
    shard.assert_call(
        "import_gameobjects_append",
        &[&serde_json::to_string(&format!("{DOOR},5099190,189,10,20,30,0,0,0,0,0,1")).unwrap()],
    );
    shard
}

fn create(shard: &Standalone, leader: u64) -> u64 {
    let party = &shard.query_rows(&format!(
        "SELECT group_id FROM game_group WHERE leader_guid = {leader}"
    ))[0]["group_id"];
    shard.assert_call("debug_defense_create_instance", &[PACKAGE, "189", party]);
    number(
        &shard.query_rows(&format!(
            "SELECT instance_id FROM game_instance WHERE party_id = {party}"
        ))[0],
        "instance_id",
    )
}

fn rows(shard: &Standalone, table: &str, instance: u64) -> Vec<Row> {
    shard.query_rows(&format!(
        "SELECT * FROM {table} WHERE instance_id = {instance}"
    ))
}

fn refuse(shard: &Standalone, reducer: &str, args: &[&str], reason: &str) {
    let output = shard.call(reducer, args);
    assert!(!output.status.success());
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(text.contains(reason), "{text}");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn simultaneous_package_instances_are_empty_and_ordinary_instances_keep_authored_population() {
    let shard = start("defense-instance-isolation");
    let first = create(&shard, 1);
    let second = create(&shard, 2);
    assert_ne!(first, second);
    for instance in [first, second] {
        for table in [
            "game_creature_spawn",
            "game_world_entity",
            "game_gameobject",
        ] {
            assert!(
                rows(&shard, table, instance).is_empty(),
                "{table} in {instance}"
            );
        }
    }
    shard.assert_call("ensure_instance", &["5099191", "189", "0", &actor("1")]);
    assert_eq!(rows(&shard, "game_creature_spawn", 5099191).len(), 1);
    assert_eq!(rows(&shard, "game_gameobject", 5099191).len(), 1);
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT guid, instance_id FROM game_creature_spawn WHERE guid = {SOURCE}"
        ))[0]["instance_id"],
        "0"
    );
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT guid, instance_id FROM game_gameobject WHERE guid = {DOOR}"
        ))[0]["instance_id"],
        "0"
    );
    refuse(
        &shard,
        "ensure_instance",
        &[&first.to_string(), "189", "0", &actor("1")],
        "Package instance requires Package mirroring",
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn closing_waits_for_occupancy_then_removes_wave_state_without_touching_another_match() {
    let shard = start("defense-instance-cleanup");
    let first = create(&shard, 1);
    let second = create(&shard, 2);
    shard.assert_sql("UPDATE game_character SET online = true, map_id = 189 WHERE guid = 1");
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    refuse(
        &shard,
        "debug_defense_enter_instance",
        &[PACKAGE, "1", &second.to_string()],
        "belongs to another Party",
    );
    refuse(
        &shard,
        "debug_defense_enter_instance",
        &["\"another.package\"", "1", &first.to_string()],
        "another Package",
    );
    shard.assert_call(
        "debug_defense_enter_instance",
        &[PACKAGE, "1", &first.to_string()],
    );
    for instance in [first, second] {
        shard.assert_call("debug_defense_spawn_wave", &[&instance.to_string()]);
        assert_eq!(rows(&shard, "game_creature_spawn", instance).len(), 1);
    }
    shard.assert_call(
        "debug_defense_close_instance",
        &[PACKAGE, &first.to_string()],
    );
    assert_eq!(
        rows(&shard, "game_instance", first)[0]["reset_requested"],
        "true"
    );
    assert_eq!(rows(&shard, "game_creature_spawn", first).len(), 1);
    refuse(
        &shard,
        "debug_defense_enter_instance",
        &[PACKAGE, "1", &first.to_string()],
        "closing",
    );
    shard.assert_call("debug_teleport", &["1", "189", "10", "20", "30", "0"]);
    shard.assert_call(
        "debug_defense_close_instance",
        &[PACKAGE, &first.to_string()],
    );
    shard.assert_call(
        "debug_defense_close_instance",
        &[PACKAGE, &first.to_string()],
    );
    for table in [
        "game_instance",
        "game_package_instance",
        "game_creature_spawn",
        "game_world_entity",
        "game_encounter_spawn",
        "game_instance_binding",
    ] {
        assert!(rows(&shard, table, first).is_empty(), "{table}");
    }
    assert_eq!(rows(&shard, "game_instance", second).len(), 1);
    assert_eq!(rows(&shard, "game_creature_spawn", second).len(), 1);
    assert_eq!(rows(&shard, "game_world_entity", second).len(), 1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn closing_preserves_an_empty_instance_claimed_by_a_transfer_intent() {
    let shard = start("defense-instance-transfer-claim");
    let instance = create(&shard, 1);
    shard.assert_sql("UPDATE game_character SET online = false, map_id = 189 WHERE guid = 1");
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call(
        "debug_bot_transfer",
        &[
            "1",
            "189",
            &instance.to_string(),
            "10",
            "20",
            "30",
            "0",
            "\"fixture claim\"",
        ],
    );
    shard.assert_sql("DELETE FROM game_world_entity WHERE guid = 1");
    shard.assert_call(
        "debug_defense_close_instance",
        &[PACKAGE, &instance.to_string()],
    );
    assert_eq!(
        rows(&shard, "game_instance", instance)[0]["reset_requested"],
        "true"
    );
    shard.assert_sql("DELETE FROM game_bot_transfer_intent WHERE bot_guid = 1");
    shard.assert_call(
        "debug_defense_close_instance",
        &[PACKAGE, &instance.to_string()],
    );
    assert!(rows(&shard, "game_instance", instance).is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_shared_package_lobby_is_empty_reusable_ungrouped_and_capped_at_two_hundred() {
    let shard = start("defense-instance-shared-lobby");
    shard.assert_call("debug_defense_create_lobby", &[PACKAGE, "189"]);
    shard.assert_call("debug_defense_create_lobby", &[PACKAGE, "189"]);
    let lobbies = shard.query_rows("SELECT instance_id FROM game_instance WHERE party_id = 0");
    assert_eq!(lobbies.len(), 1);
    let lobby = &lobbies[0]["instance_id"];
    let instance: u64 = lobby.parse().unwrap();
    assert!(rows(&shard, "game_creature_spawn", instance).is_empty());
    assert!(rows(&shard, "game_gameobject", instance).is_empty());
    shard.assert_sql("DELETE FROM game_group_member WHERE character_guid = 1");
    shard.assert_sql("UPDATE game_character SET online = true, map_id = 189 WHERE guid = 1");
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call("debug_defense_fill_lobby", &[lobby]);
    refuse(
        &shard,
        "debug_defense_enter_instance",
        &[PACKAGE, "1", lobby],
        "200-Character capacity",
    );
    shard.assert_sql("DELETE FROM game_world_entity WHERE guid = 509929000");
    shard.assert_call("debug_defense_enter_instance", &[PACKAGE, "1", lobby]);
    shard.assert_call("debug_defense_enter_instance", &[PACKAGE, "1", lobby]);
    assert_eq!(
        shard.query_rows("SELECT instance_id FROM game_world_entity WHERE guid = 1")[0]
            ["instance_id"],
        *lobby
    );
}
