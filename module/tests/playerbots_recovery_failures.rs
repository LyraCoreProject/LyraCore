//! Finite quest failure outcomes through the ordinary Package runner and Core gameplay Gates.

mod support;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use support::{poll_until, Standalone};

const PASS_INTERVAL: Duration = Duration::from_millis(1_250);
const FULL_BAG_QUEST: u32 = 33;
const RESPAWN_QUEST: u32 = 50_970;
const UNREACHABLE_ENDER_QUEST: u32 = 783;
const QUEST_ITEM: u32 = 750;
const LOOT_SOURCE: u64 = (0xF130u64 << 48) | (69u64 << 24) | 1;
const RESPAWNING_GAMEOBJECT: u64 = (0xF110u64 << 48) | 5_090_970u64;
const UNREACHABLE_ENDER: u64 = (0xF130u64 << 48) | (197u64 << 24) | 1;

fn git(path: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(path)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn digest_files(path: &std::path::Path, digest: &mut blake3::Hasher) {
    let mut children: Vec<_> = std::fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    children.sort();
    digest.update(&(children.len() as u64).to_le_bytes());
    for child in children {
        let name = child.file_name().unwrap().as_encoded_bytes();
        digest.update(&(name.len() as u64).to_le_bytes());
        digest.update(name);
        digest.update(&[u8::from(child.is_dir())]);
        if child.is_dir() {
            digest_files(&child, digest);
        } else {
            let bytes = std::fs::read(&child).unwrap();
            digest.update(&(bytes.len() as u64).to_le_bytes());
            digest.update(&bytes);
        }
    }
}

fn record_inputs(node: &Standalone) {
    let core = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let package = core.join("packages/playerbots");
    let mut digest = blake3::Hasher::new();
    digest_files(&package, &mut digest);
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(package.join("fixtures/actions.json")).unwrap())
            .unwrap();
    record["tested_core"] = git(core, &["rev-parse", "HEAD"]).into();
    record["tested_collection"] = git(&package, &["rev-parse", "HEAD"]).into();
    record["core_dirty"] = (!git(core, &["status", "--porcelain"]).is_empty()).into();
    record["collection_dirty"] = (!git(&package, &["status", "--porcelain"]).is_empty()).into();
    record["package_content_identity"] = digest.finalize().to_hex().to_string().into();
    record["module_wasm_identity"] = blake3::hash(support::module_bytes())
        .to_hex()
        .to_string()
        .into();
    let path = support::log_dir().join(format!("{}-inputs.json", node.shard_name()));
    std::fs::write(&path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    eprintln!("fixture inputs: {}", path.display());
}

fn remove_builtin_weather_import_stamp(node: &Standalone) {
    let rows =
        node.query_rows("SELECT family, source_sha, file_hash, row_count FROM game_import_meta");
    assert_eq!(rows.len(), 1, "unexpected initial Import Catalogue");
    assert_eq!(rows[0]["family"], "weather_seed");
    assert_eq!(rows[0]["source_sha"], "");
    assert_eq!(rows[0]["file_hash"], "");
    assert_eq!(rows[0]["row_count"], "2");
    node.assert_sql(
        "DELETE FROM game_import_meta WHERE family = 'weather_seed' AND source_sha = '' AND file_hash = '' AND row_count = 2",
    );
}

fn fixture(name: &str, stage: &str) -> (Standalone, String) {
    let mut node = Standalone::start(name);
    node.publish_module();
    record_inputs(&node);
    remove_builtin_weather_import_stamp(&node);
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("debug_set_nav_enabled", &["true"]);
    node.assert_call(
        "playerbots_spawn_class_role",
        &["1", "1200", "1200", "50", "1", "0"],
    );
    let guid = node.query_rows("SELECT character_guid FROM pkg_playerbots_bot")[0]
        ["character_guid"]
        .clone();
    node.assert_call(stage, &[&guid]);
    node.assert_call("playerbots_fixture_runner_select_cohort", &[&guid]);
    node.assert_call("playerbots_fixture_provision_steps", &[&guid, "64"]);
    (node, guid)
}

fn quest(node: &Standalone, guid: &str, entry: u32) -> Option<BTreeMap<String, String>> {
    node.query_rows(&format!(
        "SELECT counts, rewarded, failed FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = {entry}"
    ))
    .into_iter()
    .next()
}

fn actions(node: &Standalone, guid: &str) -> Vec<BTreeMap<String, String>> {
    node.query_rows(&format!(
        "SELECT kind, target_guid, quest_entry, outcome, started_micros, observed_micros FROM pkg_playerbots_action WHERE character_guid = {guid}"
    ))
}

fn runner(node: &Standalone, guid: &str) -> Vec<BTreeMap<String, String>> {
    node.query_rows(&format!(
        "SELECT character_guid, objective_sequence, objective, chosen, failures, recovery, deferred_destinations, next_eligible_micros FROM pkg_playerbots_runner WHERE character_guid = {guid}"
    ))
}

fn catalog(node: &Standalone, entry: u32) -> serde_json::Value {
    serde_json::json!({
        "header": node.query_rows("SELECT revision, blueprint_revision, reference_source_revision, content_revision, quest_count FROM pkg_playerbots_quest_catalog"),
        "quest": node.query_rows(&format!("SELECT quest_entry, start_entry, actual_ender_entry, content_revision FROM pkg_playerbots_catalog_quest WHERE quest_entry = {entry}")),
        "objective": node.query_rows(&format!("SELECT quest_entry, kind, target_entry, required_count, executor, source_entries, source_destinations, destination_evidence_revision FROM pkg_playerbots_catalog_objective WHERE quest_entry = {entry}")),
    })
}

fn item_count(node: &Standalone, guid: &str, entry: u32) -> u32 {
    node.query_rows(&format!(
        "SELECT stack_count FROM game_item_instance WHERE owner_guid = {guid} AND entry = {entry}"
    ))
    .iter()
    .map(|row| row["stack_count"].parse::<u32>().unwrap())
    .sum()
}

fn drive_until(
    node: &Standalone,
    guid: &str,
    timeout: Duration,
    mut done: impl FnMut(&Standalone) -> bool,
) -> bool {
    poll_until(timeout, || {
        node.assert_call("playerbots_fixture_runner_pass_once", &[guid]);
        if done(node) {
            true
        } else {
            std::thread::sleep(PASS_INTERVAL);
            false
        }
    })
}

fn save(node: &Standalone, phase: &str, evidence: serde_json::Value) {
    let path = support::log_dir().join(format!("{}-{phase}.json", node.shard_name()));
    std::fs::write(&path, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
    eprintln!("fixture evidence: {}", path.display());
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_recovery_full_bag_refuses_boundedly_then_resumes_after_inventory_space() {
    let (node, guid) = fixture(
        "playerbots-recovery-full-bag",
        "playerbots_recovery_fixture_stage_full_bag",
    );
    let refused = drive_until(&node, &guid, Duration::from_secs(45), |node| {
        actions(node, &guid).iter().any(|action| {
            action["kind"].contains("takeLoot") && action["outcome"].contains("inventoryFull")
        })
    });
    let first = serde_json::json!({
        "catalog": catalog(&node, FULL_BAG_QUEST),
        "runner": runner(&node, &guid),
        "quest": quest(&node, &guid, FULL_BAG_QUEST),
        "actions": actions(&node, &guid),
        "inventory": node.query_rows(&format!("SELECT guid, entry, slot, stack_count FROM game_item_instance WHERE owner_guid = {guid}")),
        "corpse": node.query_rows(&format!("SELECT guid, health, dead FROM game_world_entity WHERE guid = {LOOT_SOURCE}")),
        "loot": node.query_rows(&format!("SELECT corpse_guid, slot, item_entry, count, withheld FROM game_corpse_loot WHERE corpse_guid = {LOOT_SOURCE}")),
        "loot_receipt": node.query_rows(&format!("SELECT item_entry, received_count, peak_carried_count, last_source_guid FROM pkg_playerbots_quest_loot_receipt_fixture WHERE character_guid = {guid}")),
    });
    save(&node, "inventory-full-refusal", first.clone());
    assert!(refused, "{first}");
    assert!(first["quest"]["rewarded"] == "false", "{first}");
    assert_eq!(item_count(&node, &guid, QUEST_ITEM), 0, "{first}");
    assert!(
        first["corpse"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["dead"] == "true")),
        "{first}"
    );
    assert!(
        first["loot"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty()),
        "{first}"
    );
    assert!(
        first["runner"][0]["failures"]
            .as_str()
            .unwrap()
            .contains("inventoryFull"),
        "{first}"
    );

    node.assert_call("playerbots_recovery_fixture_three_parked_passes", &[&guid]);
    let bounded = serde_json::json!({
        "before": first,
        "runner": runner(&node, &guid),
        "actions": actions(&node, &guid),
        "quest": quest(&node, &guid, FULL_BAG_QUEST),
    });
    save(&node, "inventory-full-bounded", bounded.clone());
    let before_take = bounded["before"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["kind"].as_str().unwrap().contains("takeLoot"))
        .unwrap();
    let after_take = bounded["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["kind"].as_str().unwrap().contains("takeLoot"))
        .unwrap();
    assert_eq!(before_take, after_take, "{bounded}");
    assert_eq!(bounded["before"]["quest"], bounded["quest"], "{bounded}");

    node.assert_call(
        "playerbots_recovery_fixture_clear_one_inventory_slot",
        &[&guid],
    );
    std::thread::sleep(PASS_INTERVAL);
    let completed = drive_until(&node, &guid, Duration::from_secs(45), |node| {
        quest(node, &guid, FULL_BAG_QUEST).is_some_and(|quest| quest["rewarded"] == "true")
    });
    let final_state = serde_json::json!({
        "runner": runner(&node, &guid),
        "quest": quest(&node, &guid, FULL_BAG_QUEST),
        "actions": actions(&node, &guid),
        "inventory": node.query_rows(&format!("SELECT guid, entry, slot, stack_count FROM game_item_instance WHERE owner_guid = {guid}")),
        "loot": node.query_rows(&format!("SELECT corpse_guid, slot, item_entry, count, withheld FROM game_corpse_loot WHERE corpse_guid = {LOOT_SOURCE}")),
        "loot_receipt": node.query_rows(&format!("SELECT item_entry, received_count, peak_carried_count, last_source_guid FROM pkg_playerbots_quest_loot_receipt_fixture WHERE character_guid = {guid}")),
        "turnin": node.query_rows(&format!("SELECT quest_entry, turnin_count FROM pkg_playerbots_quest_turnin_fixture WHERE character_guid = {guid} AND quest_entry = {FULL_BAG_QUEST}")),
    });
    save(&node, "inventory-space-recovery", final_state.clone());
    assert!(completed, "{final_state}");
    assert!(
        final_state["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|action| {
                action["kind"].as_str().unwrap().contains("takeLoot")
                    && action["outcome"].as_str().unwrap().contains("completed")
            }),
        "{final_state}"
    );
    assert_eq!(item_count(&node, &guid, QUEST_ITEM), 0, "{final_state}");
    assert!(
        final_state["loot_receipt"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| {
                row["received_count"] == "8"
                    && row["peak_carried_count"] == "8"
                    && row["last_source_guid"] == LOOT_SOURCE.to_string()
            })),
        "{final_state}"
    );
    assert!(
        final_state["turnin"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["turnin_count"] == "1")),
        "{final_state}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_recovery_depleted_target_waits_for_actual_respawn_then_resumes() {
    let (node, guid) = fixture(
        "playerbots-recovery-respawn",
        "playerbots_quest_loop_fixture_stage_simple_gameobject",
    );
    let accepted = drive_until(&node, &guid, Duration::from_secs(15), |node| {
        quest(node, &guid, RESPAWN_QUEST).is_some()
    });
    node.assert_call(
        "playerbots_recovery_fixture_arm_gameobject_respawn",
        &["10"],
    );
    std::thread::sleep(PASS_INTERVAL);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let waiting = serde_json::json!({
        "catalog": catalog(&node, RESPAWN_QUEST),
        "runner": runner(&node, &guid),
        "quest": quest(&node, &guid, RESPAWN_QUEST),
        "actions": actions(&node, &guid),
        "gameobject": node.query_rows(&format!("SELECT guid, state, respawn_at_micros FROM game_gameobject WHERE guid = {RESPAWNING_GAMEOBJECT}")),
    });
    save(&node, "respawn-wait", waiting.clone());
    assert!(accepted, "{waiting}");
    assert!(
        waiting["runner"][0]["failures"]
            .as_str()
            .unwrap()
            .contains("questRespawn"),
        "{waiting}"
    );
    assert!(
        waiting["runner"][0]["chosen"]
            .as_str()
            .unwrap()
            .contains("hold"),
        "{waiting}"
    );
    assert_eq!(waiting["gameobject"][0]["state"], "1", "{waiting}");
    assert_eq!(waiting["quest"]["counts"], "[0]", "{waiting}");

    let respawned = poll_until(Duration::from_secs(20), || {
        node.query_rows(&format!(
            "SELECT state FROM game_gameobject WHERE guid = {RESPAWNING_GAMEOBJECT}"
        ))
        .first()
        .is_some_and(|row| row["state"] == "0")
    });
    let respawn_state = serde_json::json!({
        "runner": runner(&node, &guid),
        "quest": quest(&node, &guid, RESPAWN_QUEST),
        "gameobject": node.query_rows(&format!("SELECT guid, state, respawn_at_micros FROM game_gameobject WHERE guid = {RESPAWNING_GAMEOBJECT}")),
    });
    save(&node, "actual-gameobject-respawn", respawn_state.clone());
    assert!(respawned, "{respawn_state}");
    assert_eq!(
        respawn_state["gameobject"][0]["respawn_at_micros"], "0",
        "{respawn_state}"
    );

    std::thread::sleep(Duration::from_secs(31));
    let completed = drive_until(&node, &guid, Duration::from_secs(30), |node| {
        quest(node, &guid, RESPAWN_QUEST).is_some_and(|quest| quest["rewarded"] == "true")
    });
    let final_state = serde_json::json!({
        "runner": runner(&node, &guid),
        "quest": quest(&node, &guid, RESPAWN_QUEST),
        "actions": actions(&node, &guid),
        "gameobject": node.query_rows(&format!("SELECT guid, state, respawn_at_micros FROM game_gameobject WHERE guid = {RESPAWNING_GAMEOBJECT}")),
        "turnin": node.query_rows(&format!("SELECT quest_entry, turnin_count FROM pkg_playerbots_quest_turnin_fixture WHERE character_guid = {guid} AND quest_entry = {RESPAWN_QUEST}")),
    });
    save(&node, "respawn-recovery", final_state.clone());
    assert!(completed, "{final_state}");
    assert_eq!(final_state["quest"]["counts"], "[1]", "{final_state}");
    assert!(
        final_state["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|action| {
                action["kind"].as_str().unwrap().contains("useGameObject")
                    && action["outcome"].as_str().unwrap().contains("completed")
            }),
        "{final_state}"
    );
    assert!(
        final_state["turnin"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["turnin_count"] == "1")),
        "{final_state}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_recovery_unreachable_quest_ender_defers_and_preserves_the_quest() {
    let (node, guid) = fixture(
        "playerbots-recovery-unreachable-ender",
        "playerbots_recovery_fixture_stage_unreachable_ender",
    );
    let accepted = drive_until(&node, &guid, Duration::from_secs(15), |node| {
        quest(node, &guid, UNREACHABLE_ENDER_QUEST).is_some()
    });
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let initial = serde_json::json!({
        "catalog": catalog(&node, UNREACHABLE_ENDER_QUEST),
        "runner": runner(&node, &guid),
        "quest": quest(&node, &guid, UNREACHABLE_ENDER_QUEST),
        "character": node.query_rows(&format!("SELECT guid, x, y, map_id, instance_id FROM game_world_entity WHERE guid = {guid}")),
        "ender": node.query_rows(&format!("SELECT guid, x, y, z, map_id, instance_id FROM game_world_entity WHERE guid = {UNREACHABLE_ENDER}")),
        "actions": actions(&node, &guid),
        "navigation": node.query_rows("SELECT key, map_id, cell_x, cell_y, base_z, walk, obs FROM game_nav_chunk WHERE map_id = 0"),
    });
    save(&node, "unreachable-ender-start", initial.clone());
    let ender = &initial["ender"][0];
    let movement_destination = format!(
        "map_id = {}, instance_id = {}, destination = (x = {}, y = {})",
        ender["map_id"].as_str().unwrap(),
        ender["instance_id"].as_str().unwrap(),
        ender["x"].as_str().unwrap(),
        ender["y"].as_str().unwrap(),
    );
    let deferred_destination = format!(
        "map_id = {}, instance_id = {}, x = {}, y = {}, z = {}",
        ender["map_id"].as_str().unwrap(),
        ender["instance_id"].as_str().unwrap(),
        ender["x"].as_str().unwrap(),
        ender["y"].as_str().unwrap(),
        ender["z"].as_str().unwrap(),
    );
    assert!(accepted, "{initial}");
    assert_eq!(initial["quest"]["rewarded"], "false", "{initial}");
    assert!(
        initial["actions"].as_array().unwrap().iter().any(|action| {
            action["kind"].as_str().unwrap().contains("move")
                && action["outcome"]
                    .as_str()
                    .unwrap()
                    .contains(&movement_destination)
        }),
        "{initial}"
    );

    node.assert_call("playerbots_fixture_companion_due", &[&guid]);
    let start = Instant::now();
    let mut samples = Vec::new();
    while start.elapsed() < Duration::from_secs(38) {
        samples.push(serde_json::json!({
            "elapsed_seconds": start.elapsed().as_secs_f64(),
            "runner": runner(&node, &guid),
            "quest": quest(&node, &guid, UNREACHABLE_ENDER_QUEST),
            "character": node.query_rows(&format!("SELECT guid, x, y FROM game_world_entity WHERE guid = {guid}")),
            "ender": node.query_rows(&format!("SELECT guid, x, y, z, map_id, instance_id FROM game_world_entity WHERE guid = {UNREACHABLE_ENDER}")),
            "actions": actions(&node, &guid),
            "turnin": node.query_rows(&format!("SELECT quest_entry, turnin_count FROM pkg_playerbots_quest_turnin_fixture WHERE character_guid = {guid} AND quest_entry = {UNREACHABLE_ENDER_QUEST}")),
        }));
        std::thread::sleep(Duration::from_millis(750));
    }
    let evidence = serde_json::json!({"initial": initial, "samples": samples});
    save(&node, "unreachable-ender-deferral", evidence.clone());
    let samples = evidence["samples"].as_array().unwrap();
    let deferred = samples.iter().find(|sample| {
        sample["runner"]
            .as_array()
            .and_then(|rows| rows.first())
            .and_then(|row| row["deferred_destinations"].as_str())
            .is_some_and(|value| value.contains(&deferred_destination))
    });
    let deferred =
        deferred.unwrap_or_else(|| panic!("actual ender was never deferred: {evidence}"));
    assert!(
        deferred["elapsed_seconds"].as_f64().unwrap() <= 32.0,
        "{deferred}"
    );
    assert!(
        samples
            .iter()
            .all(|sample| sample["quest"]["rewarded"] == "false"),
        "{evidence}"
    );
    assert!(
        samples
            .iter()
            .all(|sample| sample["quest"] == evidence["initial"]["quest"]),
        "{evidence}"
    );
    assert!(
        samples.iter().all(|sample| sample["turnin"]
            .as_array()
            .is_some_and(|rows| rows.is_empty())),
        "{evidence}"
    );
    assert!(
        samples.iter().any(|sample| {
            sample["runner"]
                .as_array()
                .and_then(|rows| rows.first())
                .and_then(|row| row["failures"].as_str())
                .is_some_and(|failures| failures.contains("noMovement"))
        }),
        "{evidence}"
    );
    assert!(
        samples.iter().all(|sample| {
            sample["runner"][0]["objective_sequence"]
                == evidence["initial"]["runner"][0]["objective_sequence"]
        }),
        "{evidence}"
    );
}
