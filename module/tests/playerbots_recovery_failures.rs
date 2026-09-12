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
const ALTERNATIVE_GIVER: u64 = (0xF130u64 << 48) | (241u64 << 24) | 1;

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
        "SELECT character_guid, objective_sequence, objective, foreground, chosen, last_outcome, failures, recovery, deferred_destinations, retry_count, observed_micros, next_eligible_micros FROM pkg_playerbots_runner WHERE character_guid = {guid}"
    ))
}

fn integer_after(value: &str, marker: &str) -> i64 {
    let tail = value
        .split_once(marker)
        .unwrap_or_else(|| panic!("{marker} missing from {value}"))
        .1;
    let number: String = tail
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '-')
        .collect();
    number
        .parse()
        .unwrap_or_else(|_| panic!("invalid number after {marker} in {value}"))
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
fn playerbots_recovery_capacity_is_recorded_once_while_heal_and_expiry_remain_live() {
    let mut node = Standalone::start("playerbots-recovery-capacity-reporting");
    node.publish_module();
    record_inputs(&node);
    remove_builtin_weather_import_stamp(&node);
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("debug_set_nav_enabled", &["true"]);
    node.assert_call(
        "playerbots_spawn_class_role",
        &["1", "1200", "1200", "50", "5", "1"],
    );
    let guid = node.query_rows("SELECT character_guid FROM pkg_playerbots_bot")[0]
        ["character_guid"]
        .clone();
    node.assert_call("playerbots_fixture_runner_stage", &[&guid, "false"]);
    node.assert_call("playerbots_fixture_runner_select_cohort", &[&guid]);
    node.assert_call("playerbots_fixture_provision_steps", &[&guid, "64"]);
    node.assert_call(
        "playerbots_fixture_runner_stage_recovery_capacity",
        &[&guid],
    );

    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let first = runner(&node, &guid).remove(0);
    save(
        &node,
        "capacity-reporting-first",
        serde_json::json!({ "first": first.clone() }),
    );
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let repeated = runner(&node, &guid).remove(0);
    save(
        &node,
        "capacity-reporting-repeated",
        serde_json::json!({ "repeated": repeated.clone() }),
    );
    assert!(first["chosen"].contains("hold = ()"), "{first:?}");
    assert!(first["chosen"].contains("returnHome = ()"), "{first:?}");
    assert_eq!(first["failures"].matches("recoveryCapacity").count(), 1);
    assert!(first["last_outcome"].contains("waiting"), "{first:?}");
    assert_eq!(first["retry_count"], "1");
    assert_eq!(
        first["next_eligible_micros"].parse::<i64>().unwrap()
            - first["observed_micros"].parse::<i64>().unwrap(),
        1_000_000
    );
    assert_eq!(repeated["failures"], first["failures"]);
    assert!(repeated["last_outcome"].contains("waiting"), "{repeated:?}");
    assert_eq!(repeated["retry_count"], first["retry_count"]);
    assert_eq!(
        repeated["next_eligible_micros"].parse::<i64>().unwrap()
            - repeated["observed_micros"].parse::<i64>().unwrap(),
        1_000_000
    );

    node.assert_call("playerbots_fixture_companion_health", &[&guid, "25"]);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let healing = runner(&node, &guid).remove(0);
    assert!(healing["chosen"].contains("cast = ("), "{healing:?}");
    assert!(healing["chosen"].contains("recovery = ()"), "{healing:?}");
    assert!(healing["chosen"].contains(&format!("target = {guid}")));
    assert_eq!(healing["failures"], first["failures"]);

    node.assert_call("playerbots_fixture_companion_health", &[&guid, "100"]);
    node.assert_call(
        "playerbots_fixture_runner_expire_recovery_capacity",
        &[&guid],
    );
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let released = runner(&node, &guid).remove(0);
    assert!(
        released["chosen"].contains("move = (home = ())"),
        "{released:?}"
    );
    assert!(released["recovery"].contains("destination"), "{released:?}");
    assert_eq!(released["failures"], first["failures"]);

    save(
        &node,
        "capacity-reporting",
        serde_json::json!({
            "first": first,
            "repeated": repeated,
            "healing": healing,
            "released": released,
        }),
    );
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
    let before_retry = bounded["before"]["runner"][0]["retry_count"]
        .as_str()
        .unwrap()
        .parse::<u8>()
        .unwrap();
    let after_retry = bounded["runner"][0]["retry_count"]
        .as_str()
        .unwrap()
        .parse::<u8>()
        .unwrap();
    assert_eq!(before_retry, 1, "{bounded}");
    // The first deadline may pass while the caller captures evidence. Only that first pass may retry.
    assert!((1..=2).contains(&after_retry), "{bounded}");
    let next = bounded["runner"][0]["next_eligible_micros"]
        .as_str()
        .unwrap()
        .parse::<i64>()
        .unwrap();
    let observed = bounded["runner"][0]["observed_micros"]
        .as_str()
        .unwrap()
        .parse::<i64>()
        .unwrap();
    assert!(next > observed, "{bounded}");
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
                    && row["last_source_guid"]
                        .as_str()
                        .and_then(|guid| guid.parse::<u64>().ok())
                        == Some(LOOT_SOURCE)
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
    node.assert_call("playerbots_fixture_position", &[&guid, "1204"]);
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
        "character": node.query_rows(&format!("SELECT guid, x, y, z, map_id, instance_id FROM game_world_entity WHERE guid = {guid}")),
        "gameobject": node.query_rows(&format!("SELECT guid, state, respawn_at_micros FROM game_gameobject WHERE guid = {RESPAWNING_GAMEOBJECT}")),
    });
    save(&node, "respawn-wait", waiting.clone());
    assert!(accepted, "{waiting}");
    assert_eq!(waiting["character"][0]["x"], "1204", "{waiting}");
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
    assert_eq!(waiting["quest"]["counts"], "0", "{waiting}");

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
    assert_eq!(final_state["quest"]["counts"], "1", "{final_state}");
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
fn playerbots_recovery_cancels_an_owned_gameobject_approach_when_the_target_disappears() {
    let (node, guid) = fixture(
        "playerbots-recovery-gameobject-disappears",
        "playerbots_quest_loop_fixture_stage_simple_gameobject",
    );
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let accepted = serde_json::json!({
        "retained": node.query_rows(&format!("SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}")),
        "runner": node.query_rows(&format!("SELECT character_guid, objective_sequence, objective, foreground, chosen, recovery, history FROM pkg_playerbots_runner WHERE character_guid = {guid}")),
        "quest": quest(&node, &guid, RESPAWN_QUEST),
        "gameobject": node.query_rows(&format!("SELECT guid, x, y, z, state FROM game_gameobject WHERE guid = {RESPAWNING_GAMEOBJECT}")),
        "actions": actions(&node, &guid),
    });
    save(&node, "gameobject-quest-accepted", accepted.clone());
    assert_eq!(accepted["quest"]["counts"], "0", "{accepted}");
    assert_eq!(accepted["quest"]["rewarded"], "false", "{accepted}");

    node.assert_call(
        "playerbots_recovery_fixture_position_simple_gameobject",
        &[&guid],
    );
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let retained = node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"
    ));
    let moving = node.query_rows(&format!(
        "SELECT character_guid, objective_sequence, objective, foreground, chosen, recovery, history FROM pkg_playerbots_runner WHERE character_guid = {guid}"
    ));
    let started = serde_json::json!({
        "retained": retained,
        "runner": moving,
        "quest": quest(&node, &guid, RESPAWN_QUEST),
        "gameobject": node.query_rows(&format!("SELECT guid, x, y, z, state FROM game_gameobject WHERE guid = {RESPAWNING_GAMEOBJECT}")),
        "movement": node.query_rows(&format!("SELECT guid, sx, sy, dx, dy, start_micros, dur_ms FROM game_creature_spline WHERE guid = {guid}")),
        "actions": actions(&node, &guid),
    });
    save(&node, "gameobject-approach-started", started.clone());
    assert_eq!(
        started["retained"].as_array().unwrap().len(),
        1,
        "{started}"
    );
    assert!(
        started["runner"][0]["foreground"]
            .as_str()
            .unwrap()
            .contains(&format!("gameObject = {RESPAWNING_GAMEOBJECT}")),
        "{started}"
    );
    assert!(
        started["runner"][0]["chosen"]
            .as_str()
            .unwrap()
            .contains("reason = (quest"),
        "{started}"
    );
    assert!(
        started["movement"][0]["dur_ms"]
            .as_str()
            .unwrap()
            .parse::<u32>()
            .unwrap()
            > 0,
        "{started}"
    );
    assert_eq!(started["quest"], accepted["quest"], "{started}");

    node.assert_call("playerbots_recovery_fixture_remove_simple_gameobject", &[]);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let stopped = serde_json::json!({
        "retained": node.query_rows(&format!("SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}")),
        "runner": node.query_rows(&format!("SELECT character_guid, observed_micros, next_eligible_micros, objective_sequence, objective, foreground, chosen, failures, recovery, history, last_outcome FROM pkg_playerbots_runner WHERE character_guid = {guid}")),
        "quest": quest(&node, &guid, RESPAWN_QUEST),
        "gameobject": node.query_rows(&format!("SELECT guid, x, y, z, state FROM game_gameobject WHERE guid = {RESPAWNING_GAMEOBJECT}")),
        "movement": node.query_rows(&format!("SELECT guid, sx, sy, dx, dy, start_micros, dur_ms FROM game_creature_spline WHERE guid = {guid}")),
        "actions": actions(&node, &guid),
    });
    save(&node, "gameobject-approach-cancelled", stopped.clone());
    assert!(
        stopped["gameobject"].as_array().unwrap().is_empty(),
        "{stopped}"
    );
    assert_eq!(stopped["retained"], started["retained"], "{stopped}");
    assert_eq!(stopped["quest"], accepted["quest"], "{stopped}");
    assert_eq!(
        stopped["runner"][0]["objective_sequence"], started["runner"][0]["objective_sequence"],
        "{stopped}"
    );
    assert!(
        stopped["runner"][0]["objective"]
            .as_str()
            .unwrap()
            .contains("quest"),
        "{stopped}"
    );
    assert!(
        stopped["runner"][0]["next_eligible_micros"]
            .as_str()
            .unwrap()
            .parse::<i64>()
            .unwrap()
            > stopped["runner"][0]["observed_micros"]
                .as_str()
                .unwrap()
                .parse::<i64>()
                .unwrap(),
        "{stopped}"
    );
    assert!(
        stopped["runner"][0]["last_outcome"]
            .as_str()
            .unwrap()
            .contains("waiting"),
        "{stopped}"
    );
    assert!(
        !stopped["runner"][0]["foreground"]
            .as_str()
            .unwrap()
            .contains(&format!("gameObject = {RESPAWNING_GAMEOBJECT}")),
        "{stopped}"
    );
    let history = stopped["runner"][0]["history"].as_str().unwrap();
    assert!(
        history.split("(at_micros =").any(|transition| {
            transition.contains(&format!("gameObject = {RESPAWNING_GAMEOBJECT}"))
                && transition.contains("outcome = (cancelled")
        }),
        "{stopped}"
    );

    node.assert_call(
        "playerbots_recovery_fixture_restore_simple_gameobject",
        &[&guid],
    );
    node.assert_call(
        "playerbots_recovery_fixture_position_simple_gameobject",
        &[&guid],
    );
    std::thread::sleep(PASS_INTERVAL);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let resumed = serde_json::json!({
        "retained": node.query_rows(&format!("SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}")),
        "runner": node.query_rows(&format!("SELECT character_guid, objective_sequence, objective, foreground, chosen, failures, recovery, history FROM pkg_playerbots_runner WHERE character_guid = {guid}")),
        "quest": quest(&node, &guid, RESPAWN_QUEST),
        "gameobject": node.query_rows(&format!("SELECT guid, x, y, z, state FROM game_gameobject WHERE guid = {RESPAWNING_GAMEOBJECT}")),
        "movement": node.query_rows(&format!("SELECT guid, sx, sy, dx, dy, start_micros, dur_ms FROM game_creature_spline WHERE guid = {guid}")),
        "actions": actions(&node, &guid),
    });
    save(&node, "gameobject-approach-resumed", resumed.clone());
    assert_eq!(resumed["retained"], started["retained"], "{resumed}");
    assert_eq!(resumed["quest"], accepted["quest"], "{resumed}");
    assert_eq!(
        resumed["runner"][0]["objective_sequence"], started["runner"][0]["objective_sequence"],
        "{resumed}"
    );
    assert_eq!(
        resumed["gameobject"].as_array().unwrap().len(),
        1,
        "{resumed}"
    );
    assert!(
        resumed["runner"][0]["chosen"]
            .as_str()
            .unwrap()
            .contains(&format!("gameObject = {RESPAWNING_GAMEOBJECT}")),
        "{resumed}"
    );
    assert!(
        resumed["runner"][0]["chosen"]
            .as_str()
            .unwrap()
            .contains("reason = (quest"),
        "{resumed}"
    );
    assert!(
        resumed["runner"][0]["foreground"]
            .as_str()
            .unwrap()
            .contains(&format!("gameObject = {RESPAWNING_GAMEOBJECT}")),
        "{resumed}"
    );
    assert_eq!(
        resumed["movement"].as_array().unwrap().len(),
        1,
        "{resumed}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_recovery_does_not_retain_a_missing_gameobject_with_a_changed_objective() {
    let (node, guid) = fixture(
        "playerbots-recovery-gameobject-capability-changed",
        "playerbots_quest_loop_fixture_stage_simple_gameobject",
    );
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let accepted = serde_json::json!({
        "retained": node.query_rows(&format!("SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}")),
        "runner": runner(&node, &guid),
        "quest": quest(&node, &guid, RESPAWN_QUEST),
        "gameobject": node.query_rows(&format!("SELECT guid FROM game_gameobject WHERE guid = {RESPAWNING_GAMEOBJECT}")),
    });
    save(&node, "gameobject-capability-accepted", accepted.clone());

    node.assert_call(
        "playerbots_recovery_fixture_remove_simple_gameobject_and_objective",
        &[],
    );
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let refused = serde_json::json!({
        "admission": node.query_rows(&format!("SELECT considered_quest, selected_quest, state, missing_capability, detail FROM pkg_playerbots_quest_admission WHERE character_guid = {guid}")),
        "retained": node.query_rows(&format!("SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}")),
        "runner": runner(&node, &guid),
        "quest": quest(&node, &guid, RESPAWN_QUEST),
        "gameobject": node.query_rows(&format!("SELECT guid FROM game_gameobject WHERE guid = {RESPAWNING_GAMEOBJECT}")),
        "objective": node.query_rows(&format!("SELECT id FROM game_quest_objective WHERE quest_entry = {RESPAWN_QUEST}")),
        "actions": actions(&node, &guid),
    });
    save(&node, "gameobject-capability-refused", refused.clone());
    assert_eq!(
        accepted["retained"].as_array().unwrap().len(),
        1,
        "{accepted}"
    );
    assert_eq!(accepted["quest"]["counts"], "0", "{accepted}");
    assert_eq!(refused["quest"], accepted["quest"], "{refused}");
    assert!(
        refused["gameobject"].as_array().unwrap().is_empty(),
        "{refused}"
    );
    assert!(
        refused["objective"].as_array().unwrap().is_empty(),
        "{refused}"
    );
    assert!(
        refused["retained"].as_array().unwrap().is_empty(),
        "{refused}"
    );
    assert!(
        refused["admission"][0]["missing_capability"]
            .as_str()
            .unwrap()
            .contains("objectiveMismatch"),
        "{refused}"
    );
    assert!(
        refused["runner"][0]["objective"]
            .as_str()
            .unwrap()
            .contains("returnHome"),
        "{refused}"
    );
    assert!(
        !refused["runner"][0]["foreground"]
            .as_str()
            .unwrap()
            .contains(&RESPAWNING_GAMEOBJECT.to_string()),
        "{refused}"
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
                && action["outcome"]
                    .as_str()
                    .unwrap()
                    .contains("status = (blocked = ())")
                && action["outcome"]
                    .as_str()
                    .unwrap()
                    .contains("coverage = (unknown = ())")
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
    let initial_identity = evidence["initial"]["runner"][0]["objective_sequence"]
        .as_str()
        .unwrap();
    let runner = &deferred["runner"][0];
    assert!(
        deferred["elapsed_seconds"].as_f64().unwrap() <= 32.0,
        "{deferred}"
    );
    let failures = runner["failures"].as_str().unwrap();
    assert!(
        failures.contains("noMovement")
            && failures.contains("missingImportedCoverage")
            && !failures.contains("deadline"),
        "{deferred}"
    );
    let recovery = runner["recovery"].as_str().unwrap();
    let current_identity = runner["objective_sequence"].as_str().unwrap();
    assert!(
        recovery.contains(&format!(
            "work = (quest = (step = (target = {ALTERNATIVE_GIVER}, quest = 40), operation = (accept = ())))"
        )) && recovery.contains(&format!("objective = {current_identity},")),
        "{deferred}"
    );
    assert!(
        !recovery.contains(&format!(
            "target = {UNREACHABLE_ENDER}, quest = {UNREACHABLE_ENDER_QUEST}"
        )),
        "{deferred}"
    );
    assert!(
        runner["chosen"].as_str().unwrap().contains(&format!(
            "acceptQuest = (target = {ALTERNATIVE_GIVER}, quest = 40)"
        )) && runner["chosen"]
            .as_str()
            .unwrap()
            .contains(&format!("objective = {current_identity}")),
        "{deferred}"
    );
    let deferred_until = integer_after(
        runner["deferred_destinations"].as_str().unwrap(),
        "until_micros = ",
    );
    let observed_micros = runner["observed_micros"]
        .as_str()
        .unwrap()
        .parse::<i64>()
        .unwrap();
    let remaining_micros = deferred_until.saturating_sub(observed_micros);
    assert!(
        remaining_micros > 0 && remaining_micros <= 30_000_000,
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
    let retained_quest = UNREACHABLE_ENDER_QUEST.to_string();
    let useful_alternative = samples.iter().find(|sample| {
        sample["elapsed_seconds"].as_f64().unwrap() >= deferred["elapsed_seconds"].as_f64().unwrap()
            && sample["actions"].as_array().unwrap().iter().any(|action| {
                action["quest_entry"]
                    .as_str()
                    .is_some_and(|entry| entry != "0" && entry != retained_quest.as_str())
                    && action["observed_micros"]
                        .as_str()
                        .and_then(|value| value.parse::<i64>().ok())
                        .is_some_and(|observed| {
                            observed > observed_micros && observed <= deferred_until
                        })
                    && action["outcome"].as_str().unwrap().contains("completed")
            })
    });
    let useful_alternative = useful_alternative
        .unwrap_or_else(|| panic!("no alternative Quest completed during deferral: {evidence}"));
    assert!(
        useful_alternative["runner"][0]["objective_sequence"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > initial_identity.parse::<u64>().unwrap(),
        "{useful_alternative}"
    );
}
