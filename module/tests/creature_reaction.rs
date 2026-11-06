mod support;

use serde_json::json;
use support::{actor, number, Standalone};

fn stage(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard.assert_sql("UPDATE game_world_entity SET godmode = true WHERE guid = 1");

    shard
}

fn creature(shard: &Standalone, reaction: u8) -> String {
    shard.assert_call("debug_spawn_at_feet", &["1", "51000", "2"]);
    let target = shard
        .query_rows("SELECT guid FROM game_world_entity WHERE entry = 51000")
        .into_iter()
        .map(|row| row["guid"].parse::<u64>().unwrap())
        .max()
        .unwrap()
        .to_string();
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = true, health = 1000, \
         max_health = 1000 WHERE guid = {target}"
    ));
    shard.assert_sql(&format!(
        "DELETE FROM game_creature_ai_state WHERE creature_guid = {target}"
    ));
    shard.assert_sql(&format!(
        "INSERT INTO game_creature_ai_state \
         (creature_guid,phase,lifecycle_id,engagement_id,ranged_distance,ranged_angle,\
          ranged_posture_active,definition_revision,active_object,react_state,melee_enabled) \
         VALUES ({target},0,0,1,0,0,false,0,false,{reaction},true)"
    ));
    target
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn damage_adds_threat_but_only_nonpassive_creatures_retaliate() {
    let shard = stage("creature-reaction");
    for reaction in [0, 1, 2] {
        let target = creature(&shard, reaction);
        shard.assert_call(
            "debug_stage_virtual_spell_impact",
            &["1", &target, "10", "50000"],
        );
        shard.assert_call("debug_advance_virtual_clock", &["50000"]);
        assert_eq!(
            shard.query_rows(&format!(
                "SELECT health FROM game_world_entity WHERE guid = {target}"
            ))[0]["health"],
            "990"
        );
        assert_eq!(
            shard.query_rows(&format!(
                "SELECT threat FROM game_threat WHERE creature_guid = {target} AND source_guid = 1"
            ))[0]["threat"],
            "10"
        );
        assert_eq!(
            shard
                .query_rows(&format!(
                    "SELECT attacker_guid FROM game_melee_attack WHERE attacker_guid = {target}"
                ))
                .is_empty(),
            reaction == 0,
            "reaction state {reaction} after spell damage"
        );
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn passive_threat_keeps_both_units_in_combat_without_regeneration() {
    let shard = stage("passive-combat-threat");
    let target = creature(&shard, 0);
    shard.assert_call(
        "debug_stage_virtual_spell_impact",
        &["1", &target, "10", "50000"],
    );
    shard.assert_call("debug_advance_roll_batch", &["200"]);
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT health FROM game_world_entity WHERE guid = {target}"
        ))[0]["health"],
        "990"
    );
    for guid in ["1", &target] {
        let flags = shard.query_rows(&format!(
            "SELECT unit_flags FROM game_world_entity WHERE guid = {guid}"
        ))[0]["unit_flags"]
            .parse::<u32>()
            .unwrap();
        assert_ne!(flags & lyracore_shared::constants::unit_flags::IN_COMBAT, 0);
    }
    assert!(shard
        .query_rows(&format!(
            "SELECT attacker_guid FROM game_melee_attack WHERE attacker_guid = {target}"
        ))
        .is_empty());
    move_by(&shard, 100.0);
    shard.assert_call("debug_advance_roll_batch", &["700"]);
    assert!(
        !shard
            .query_rows(&format!(
                "SELECT id FROM game_threat WHERE creature_guid = {target}"
            ))
            .is_empty(),
        "passive combat must survive the ordinary melee leash while its grid stays active"
    );
    escape(&shard, 1000.0);
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT health FROM game_world_entity WHERE guid = {target}"
        ))[0]["health"],
        "1000",
        "evade restores the passive creature's health"
    );
    for guid in ["1", &target] {
        let flags = shard.query_rows(&format!(
            "SELECT unit_flags FROM game_world_entity WHERE guid = {guid}"
        ))[0]["unit_flags"]
            .parse::<u32>()
            .unwrap();
        assert_eq!(flags & lyracore_shared::constants::unit_flags::IN_COMBAT, 0);
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn healing_adds_threat_to_a_passive_creatures_combat() {
    let shard = stage("passive-healing-threat");
    let target = creature(&shard, 0);
    shard.assert_call(
        "debug_stage_virtual_spell_impact",
        &["1", &target, "10", "50000"],
    );
    shard.assert_call("debug_advance_virtual_clock", &["50000"]);
    shard.assert_sql("UPDATE game_world_entity SET health = 90, max_health = 100 WHERE guid = 1");
    shard.assert_call("debug_cast_at", &["1", "50110", "1"]);
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT threat FROM game_threat WHERE creature_guid = {target} AND source_guid = 1"
        ))[0]["threat"],
        "15"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn natural_escape_from_passive_combat_restores_mounting_and_logout() {
    let shard = stage("passive-mount-gate");
    let mut wire = shard.connect_reducers(&["SELECT * FROM game_native_packet"]);
    wire.call("native_world_open", json!([42])).unwrap();
    wire.call("debug_native_enter_world", json!([1])).unwrap();
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_learn_riding", &["1", "75"]);
    let target = creature(&shard, 0);
    shard.assert_call(
        "debug_stage_virtual_spell_impact",
        &["1", &target, "10", "50000"],
    );
    shard.assert_call("debug_advance_virtual_clock", &["50000"]);
    let refused = shard.call("debug_cast_at", &["1", "50310", "1"]);
    assert!(!refused.status.success());
    assert!(format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    )
    .contains("combat"));
    let refused = wire
        .call("native_world_receive", json!([0x4bu32, Vec::<u8>::new()]))
        .unwrap();
    assert!(
        refused.inserts("game_native_packet").iter().any(|row| {
            let packet: serde_json::Value = serde_json::from_str(row.get()).unwrap();
            packet[2][1] == 0x4c && packet[3] == "0100000001"
        }),
        "the wire refuses logout while the passive creature retains threat"
    );
    escape(&shard, 1000.0);
    shard.assert_call("debug_cast_at", &["1", "50310", "1"]);
    let accepted = wire
        .call("native_world_receive", json!([0x4bu32, Vec::<u8>::new()]))
        .unwrap();
    assert!(
        accepted.inserts("game_native_packet").iter().any(|row| {
            let packet: serde_json::Value = serde_json::from_str(row.get()).unwrap();
            packet[2][1] == 0x4d
        }),
        "the wire completes logout after natural evade"
    );
    assert!(shard
        .query_rows("SELECT guid FROM game_world_entity WHERE guid = 1")
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_resisted_hostile_spell_keeps_initial_threat_without_a_loot_tag() {
    let shard = stage("passive-initial-threat");
    let target = creature(&shard, 0);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET level = 60 WHERE guid = {target}"
    ));
    shard.assert_sql(&format!("INSERT INTO game_conformance_spell_random (character_guid,target_guid,state) VALUES (1,{target},1)"));
    shard.assert_call("debug_cast_at", &["1", "133", &target]);
    let threats = shard.query_rows(&format!(
        "SELECT threat FROM game_threat WHERE creature_guid = {target} AND source_guid = 1"
    ));
    assert_eq!(threats.len(), 1);
    assert_eq!(threats[0]["threat"], "0");
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT health FROM game_world_entity WHERE guid = {target}"
        ))[0]["health"],
        "1000"
    );
    assert!(shard
        .query_rows(&format!(
            "SELECT creature_guid FROM game_creature_quest_tap WHERE creature_guid = {target}"
        ))
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn taxi_takeoff_clears_a_passive_creatures_threat_only_combat() {
    use lyracore_shared::constants::{taxi_fixture as taxi, unit_flags};

    let shard = stage("passive-taxi-threat");
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    let target = creature(&shard, 0);
    shard.assert_call(
        "debug_stage_virtual_spell_impact",
        &["1", &target, "10", "50000"],
    );
    shard.assert_call("debug_advance_virtual_clock", &["50000"]);
    shard.assert_sql(&format!(
        "UPDATE game_threat SET threat = 0 WHERE creature_guid = {target} AND source_guid = 1"
    ));
    assert!(shard
        .query_rows("SELECT * FROM game_melee_attack")
        .is_empty());
    let in_combat = |guid: &str| {
        shard.query_rows(&format!(
            "SELECT unit_flags FROM game_world_entity WHERE guid = {guid}"
        ))[0]["unit_flags"]
            .parse::<u32>()
            .unwrap()
            & unit_flags::IN_COMBAT
            != 0
    };
    assert!(in_combat(&target));
    shard.assert_sql(&format!(
        "INSERT INTO game_active_taxi_flight \
         (character_guid,path_id,source_node_id,destination_node_id,mount_display_id,fare,\
          current_node_index,started_micros,origin_x,origin_y,origin_z,origin_recorded,first_destination_node_index) \
         VALUES (1,{},{},{},1147,25,0,0,0,0,0,false,1)",
        taxi::PATH_ID,
        taxi::SOURCE_NODE_STORAGE_ID,
        taxi::DESTINATION_NODE_STORAGE_ID
    ));
    shard.assert_call("gw_arm_taxi_flight", &[r#"{"guid":1,"ownership":null}"#]);
    assert!(shard
        .query_rows(&format!(
            "SELECT id FROM game_threat WHERE creature_guid = {target} AND source_guid = 1"
        ))
        .is_empty());
    assert!(
        !in_combat(&target),
        "a passive creature must leave combat after its last hostile reference goes offline"
    );
    assert!(in_combat("1"), "the passenger waits for its combat check");
    for _ in 0..24 {
        shard.assert_call("debug_advance_virtual_clock", &["50000"]);
    }
    assert!(!in_combat("1"));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_passive_creature_runs_its_script_once_on_each_combat_entry() {
    let shard = stage("passive-combat-script");
    let rules = "900001,aggro,100,4294967295,once,all,ordinary,any-posture,phase:1+help:20";
    let subject = "entry:51000";
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"lyracore-eventai-definition-v1");
    hasher.update(format!("{subject}@{rules}").as_bytes());
    let revision = u64::from_le_bytes(hasher.finalize().as_bytes()[..8].try_into().unwrap());
    shard.assert_call(
        "import_creature_ai_definitions",
        &[&format!("{subject}@{revision}@{rules}")],
    );
    let target = creature(&shard, 0);
    let ally = creature(&shard, 2);
    let phase = || {
        shard.query_rows(&format!(
            "SELECT phase FROM game_creature_ai_state WHERE creature_guid = {target}"
        ))[0]["phase"]
            .clone()
    };
    shard.assert_call(
        "debug_stage_virtual_spell_impact",
        &["1", &target, "10", "50000"],
    );
    shard.assert_call("debug_advance_virtual_clock", &["50000"]);
    assert_eq!(phase(), "1");
    assert!(shard
        .query_rows(&format!(
            "SELECT attacker_guid FROM game_melee_attack WHERE attacker_guid = {ally}"
        ))
        .is_empty());
    shard.assert_sql(&format!(
        "UPDATE game_creature_ai_state SET phase = 2 WHERE creature_guid = {target}"
    ));
    shard.assert_call(
        "debug_stage_virtual_spell_impact",
        &["1", &target, "10", "50000"],
    );
    shard.assert_call("debug_advance_virtual_clock", &["50000"]);
    assert_eq!(phase(), "2");
    escape(&shard, 1000.0);
    move_by(&shard, -1000.0);
    shard.assert_call(
        "debug_stage_virtual_spell_impact",
        &["1", &target, "10", "50000"],
    );
    shard.assert_call("debug_advance_virtual_clock", &["50000"]);
    assert_eq!(phase(), "1");
}

fn move_by(shard: &Standalone, dx: f32) {
    let pose = shard.query_rows("SELECT x,y,z,last_move_ms FROM game_world_entity WHERE guid = 1");
    let time_ms = number::<u32>(&pose[0], "last_move_ms").wrapping_add(1000);
    let [x, y, z] = ["x", "y", "z"].map(|axis| number::<f32>(&pose[0], axis));
    shard.assert_call(
        "gw_movement_update",
        &[
            &actor("1"),
            &lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT.to_string(),
            "[0,0,0,0]",
            &(x + dx).to_string(),
            &y.to_string(),
            &z.to_string(),
            "0",
            &time_ms.to_string(),
        ],
    );
}

fn escape(shard: &Standalone, distance: f32) {
    move_by(shard, distance);
    shard.assert_call("debug_advance_roll_batch", &["700"]);
    assert!(
        shard
            .query_rows("SELECT id FROM game_threat WHERE source_guid = 1")
            .is_empty(),
        "an inactive creature grid must evade and discard threat"
    );
}

fn script(shard: &Standalone, rules: &str) {
    let subject = "entry:51000";
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"lyracore-eventai-definition-v1");
    hasher.update(format!("{subject}@{rules}").as_bytes());
    let revision = u64::from_le_bytes(hasher.finalize().as_bytes()[..8].try_into().unwrap());
    shard.assert_call(
        "import_creature_ai_definitions",
        &[&format!("{subject}@{revision}@{rules}")],
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn passive_combat_suppresses_out_of_combat_timers_without_running_victim_events() {
    let shard = stage("passive-eventai-timers");
    script(&shard, "900001,aggro,100,4294967295,once,all,ordinary,any-posture,phase:1~900002,timer-ooc:0:0,100,4294967295,once,all,ordinary,any-posture,phase:2~900003,timer-combat:0:0,100,4294967295,once,all,ordinary,any-posture,phase:3~900004,health:0:100,100,4294967295,once,all,ordinary,any-posture,phase:4");
    let target = creature(&shard, 0);
    shard.assert_call(
        "debug_stage_virtual_spell_impact",
        &["1", &target, "10", "50000"],
    );
    shard.assert_call("debug_advance_roll_batch", &["20"]);
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT phase FROM game_creature_ai_state WHERE creature_guid = {target}"
        ))[0]["phase"],
        "1"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn retained_passive_threat_retries_attack_start_after_becoming_defensive() {
    let shard = stage("passive-becomes-defensive");
    let target = creature(&shard, 0);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET level = 60 WHERE guid = {target}"
    ));
    shard.assert_sql(&format!("INSERT INTO game_conformance_spell_random (character_guid,target_guid,state) VALUES (1,{target},1)"));
    shard.assert_call("debug_cast_at", &["1", "133", &target]);
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT threat FROM game_threat WHERE creature_guid = {target}"
        ))[0]["threat"],
        "0"
    );
    shard.assert_call("debug_advance_roll_batch", &["80"]);
    assert!(shard
        .query_rows(&format!(
            "SELECT attacker_guid FROM game_melee_attack WHERE attacker_guid = {target}"
        ))
        .is_empty());
    shard.assert_sql(&format!(
        "UPDATE game_creature_ai_state SET react_state = 1 WHERE creature_guid = {target}"
    ));
    shard.assert_call("debug_advance_roll_batch", &["80"]);
    let attacks = shard.query_rows(&format!(
        "SELECT target_guid FROM game_melee_attack WHERE attacker_guid = {target}"
    ));
    assert_eq!(
        attacks.len(),
        1,
        "defensive reaction retries AttackStart for retained zero threat"
    );
    assert_eq!(attacks[0]["target_guid"], "1");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_active_object_keeps_passive_combat_until_its_grid_can_deactivate() {
    let shard = stage("passive-active-grid");
    let target = creature(&shard, 0);
    shard.assert_sql(&format!(
        "UPDATE game_creature_ai_state SET active_object = true WHERE creature_guid = {target}"
    ));
    shard.assert_call(
        "debug_stage_virtual_spell_impact",
        &["1", &target, "10", "50000"],
    );
    shard.assert_call("debug_advance_virtual_clock", &["50000"]);
    move_by(&shard, 1000.0);
    shard.assert_call("debug_advance_roll_batch", &["700"]);
    assert!(
        !shard
            .query_rows(&format!(
                "SELECT id FROM game_threat WHERE creature_guid = {target}"
            ))
            .is_empty(),
        "an active object prevents grid deactivation even after the Character escapes"
    );
    shard.assert_sql(&format!(
        "UPDATE game_creature_ai_state SET active_object = false WHERE creature_guid = {target}"
    ));
    shard.assert_call("debug_advance_roll_batch", &["700"]);
    assert!(shard
        .query_rows(&format!(
            "SELECT id FROM game_threat WHERE creature_guid = {target}"
        ))
        .is_empty());
}
