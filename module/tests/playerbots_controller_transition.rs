//! Populated Legacy controller cutover before the Legacy executor is removed.

mod support;

use serde_json::{json, Value};
use std::collections::BTreeMap;
use support::Standalone;

const MAX_BATCH: usize = 16;

fn one(node: &Standalone, sql: &str) -> BTreeMap<String, String> {
    let rows = node.query_rows(sql);
    assert_eq!(rows.len(), 1, "expected one row for {sql}, got {rows:?}");
    rows.into_iter().next().unwrap()
}

fn bot(node: &Standalone, guid: &str) -> BTreeMap<String, String> {
    one(
        node,
        &format!("SELECT * FROM pkg_playerbots_bot WHERE character_guid = {guid}"),
    )
}

fn runner(node: &Standalone, guid: &str) -> Vec<BTreeMap<String, String>> {
    node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}"
    ))
}

fn core_escrow_snapshot(node: &Standalone, guid: &str) -> Value {
    json!({
        "bot": bot(node, guid),
        "runner": runner(node, guid),
        "intent": node.query_rows(&format!("SELECT * FROM game_bot_transfer_intent WHERE bot_guid = {guid}")),
        "legacy_goal": node.query_rows(&format!("SELECT kind FROM pkg_playerbots_goal WHERE character_guid = {guid}")),
        "character": node.query_rows(&format!("SELECT * FROM game_character WHERE guid = {guid}")),
        "body": node.query_rows(&format!("SELECT * FROM game_world_entity WHERE guid = {guid}")),
        "escrow": node.query_rows(&format!("SELECT transfer_id, character_guid, dest_map_id, dest_instance_id FROM game_transfer_out WHERE character_guid = {guid}")),
    })
}

fn legacy_batch_state(node: &Standalone, guid: &str) -> Value {
    json!({
        "bots": node.query_rows("SELECT * FROM pkg_playerbots_bot"),
        "runner": runner(node, guid),
        "goals": node.query_rows("SELECT * FROM pkg_playerbots_goal"),
        "actions": node.query_rows("SELECT * FROM pkg_playerbots_action"),
    })
}

fn batch(guids: &[String]) -> String {
    format!("[{}]", guids.join(","))
}

fn output(result: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    )
}

fn sha256(path: &std::path::Path) -> String {
    for (program, args) in [("sha256sum", vec![]), ("shasum", vec!["-a", "256"])] {
        let result = std::process::Command::new(program)
            .args(args)
            .arg(path)
            .output();
        if let Ok(result) = result {
            assert!(
                result.status.success(),
                "{program} failed for {}",
                path.display()
            );
            return String::from_utf8(result.stdout)
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap()
                .to_string();
        }
    }
    panic!("neither sha256sum nor shasum is installed");
}

fn save(node: &Standalone, label: &str, evidence: &Value) {
    let path = support::log_dir().join(format!("{}-{label}.json", node.shard_name()));
    std::fs::write(path, serde_json::to_vec_pretty(evidence).unwrap()).unwrap();
}

fn current_fixture(name: &str, count: &str) -> (Standalone, Vec<String>) {
    let mut node = Standalone::start(name);
    node.publish_module();
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("playerbots_spawn", &[count, "1200", "1200", "50"]);
    node.assert_sql("DELETE FROM game_creature_move_schedule");
    let mut bots = node.query_rows("SELECT character_guid FROM pkg_playerbots_bot");
    bots.sort_by_key(|row| row["character_guid"].parse::<u64>().unwrap());
    (
        node,
        bots.into_iter()
            .map(|row| row["character_guid"].clone())
            .collect(),
    )
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_new_bots_use_cohort_and_legacy_selection_refuses_before_state_change() {
    let (node, guids) = current_fixture("playerbots-controller-new-default", "1");
    let guid = &guids[0];
    let before = json!({
        "bot": bot(&node, guid),
        "runner": runner(&node, guid),
        "consent": node.query_rows(&format!("SELECT * FROM game_sessionless_action_consent WHERE character_guid = {guid}")),
        "actions": node.query_rows(&format!("SELECT * FROM pkg_playerbots_action WHERE character_guid = {guid}")),
    });
    let refused = node.call("playerbots_select_controller", &[guid, "{\"legacy\":[]}"]);
    let after_refusal = json!({
        "bot": bot(&node, guid),
        "runner": runner(&node, guid),
        "consent": node.query_rows(&format!("SELECT * FROM game_sessionless_action_consent WHERE character_guid = {guid}")),
        "actions": node.query_rows(&format!("SELECT * FROM pkg_playerbots_action WHERE character_guid = {guid}")),
    });
    node.assert_call("playerbots_migrate_legacy_controllers", &[&batch(&guids)]);
    let after_replay = json!({
        "bot": bot(&node, guid),
        "runner": runner(&node, guid),
        "consent": node.query_rows(&format!("SELECT * FROM game_sessionless_action_consent WHERE character_guid = {guid}")),
        "actions": node.query_rows(&format!("SELECT * FROM pkg_playerbots_action WHERE character_guid = {guid}")),
    });
    let evidence = json!({
        "before": before,
        "legacy_selection": {"success": refused.status.success(), "output": output(&refused)},
        "after_refusal": after_refusal,
        "after_migration_replay": after_replay,
    });
    save(&node, "new-default", &evidence);
    assert!(
        evidence["before"]["bot"]["controller"]
            .as_str()
            .unwrap()
            .contains("cohort"),
        "{evidence}"
    );
    assert!(!refused.status.success(), "{evidence}");
    assert!(
        output(&refused).contains("Legacy controller selection is retired"),
        "{evidence}"
    );
    assert_eq!(evidence["after_refusal"], evidence["before"], "{evidence}");
    assert_eq!(
        evidence["after_migration_replay"], evidence["before"],
        "{evidence}"
    );
}

#[test]
#[ignore = "requires the actual preceding PB011 Wasm, SpacetimeDB, and the playerbots Package"]
fn playerbots_populated_legacy_batches_resume_after_restart_and_replay_idempotently() {
    let preceding_path = std::env::var_os("PLAYERBOTS_PB011_PRECEDING_WASM")
        .expect("PLAYERBOTS_PB011_PRECEDING_WASM must name the accepted PB011 Wasm");
    let manifest_path = std::env::var_os("PLAYERBOTS_PB011_PRECEDING_MANIFEST")
        .expect("PLAYERBOTS_PB011_PRECEDING_MANIFEST must describe the accepted PB011 build");
    let expected_manifest_sha = std::env::var("PLAYERBOTS_PB011_PRECEDING_MANIFEST_SHA256")
        .expect("PLAYERBOTS_PB011_PRECEDING_MANIFEST_SHA256 must bind the accepted PB011 build");
    assert!(
        expected_manifest_sha.len() == 64
            && expected_manifest_sha
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "accepted PB011 manifest SHA-256 must be 64 lowercase hexadecimal characters"
    );
    let preceding_path = std::path::Path::new(&preceding_path);
    let manifest_path = std::path::Path::new(&manifest_path);
    let observed_manifest_sha = sha256(manifest_path);
    assert_eq!(observed_manifest_sha, expected_manifest_sha);
    let manifest: Value = serde_json::from_slice(&std::fs::read(manifest_path).unwrap()).unwrap();
    let preceding = std::fs::read(preceding_path).unwrap();
    assert_eq!(manifest["schema"], "playerbots-build-v1");
    assert_eq!(manifest["core"]["dirty"], false);
    assert_eq!(manifest["collection"]["dirty"], false);
    assert_eq!(
        manifest["wasm_bytes"].as_u64(),
        Some(preceding.len() as u64)
    );
    assert_eq!(
        sha256(preceding_path),
        manifest["wasm_sha256"].as_str().unwrap()
    );
    assert_ne!(
        blake3::hash(&preceding),
        blake3::hash(support::module_bytes())
    );
    let mut node = Standalone::start_persistent("playerbots-controller-populated-cutover");
    node.publish_module_bytes(&preceding);
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("playerbots_spawn", &["17", "1200", "1200", "50"]);
    let mut rows = node.query_rows("SELECT character_guid FROM pkg_playerbots_bot");
    rows.sort_by_key(|row| row["character_guid"].parse::<u64>().unwrap());
    let guids: Vec<_> = rows
        .into_iter()
        .map(|row| row["character_guid"].clone())
        .collect();
    let retained = &guids[0];
    node.assert_call("playerbots_fixture_runner_stage", &[retained, "false"]);
    node.assert_call(
        "playerbots_select_controller",
        &[retained, "{\"cohort\":[]}"],
    );
    node.assert_call("playerbots_fixture_runner_pass_once", &[retained]);
    node.assert_call(
        "playerbots_select_controller",
        &[retained, "{\"legacy\":[]}"],
    );
    node.assert_sql("UPDATE pkg_playerbots_bot SET next_think_micros = 9223372036854775807");
    node.assert_sql("DELETE FROM game_creature_move_schedule");
    let preceding_state = legacy_batch_state(&node, retained);
    node.publish_module();
    let imported = legacy_batch_state(&node, retained);
    let oversized = node.call("playerbots_migrate_legacy_controllers", &[&batch(&guids)]);
    let after_oversized = legacy_batch_state(&node, retained);
    let unsorted_batch = batch(&[guids[1].clone(), guids[0].clone()]);
    let unsorted = node.call("playerbots_migrate_legacy_controllers", &[&unsorted_batch]);
    let after_unsorted = legacy_batch_state(&node, retained);
    let retained_before = imported["runner"][0].clone();
    node.assert_call(
        "playerbots_migrate_legacy_controllers",
        &[&batch(&guids[..MAX_BATCH])],
    );
    let first_batch = node.query_rows("SELECT character_guid, controller FROM pkg_playerbots_bot");
    node.restart_persistent();
    let scheduler_after_restart = node.query_rows("SELECT * FROM game_creature_move_schedule");
    node.assert_call(
        "playerbots_migrate_legacy_controllers",
        &[&batch(&guids[MAX_BATCH..])],
    );
    let after_resume = json!({
        "bots": node.query_rows("SELECT * FROM pkg_playerbots_bot"),
        "retained_runner": runner(&node, retained),
        "goals": node.query_rows("SELECT * FROM pkg_playerbots_goal"),
        "actions": node.query_rows("SELECT * FROM pkg_playerbots_action"),
    });
    node.assert_call(
        "playerbots_migrate_legacy_controllers",
        &[&batch(&guids[..MAX_BATCH])],
    );
    node.assert_call(
        "playerbots_migrate_legacy_controllers",
        &[&batch(&guids[MAX_BATCH..])],
    );
    let replayed = json!({
        "bots": node.query_rows("SELECT * FROM pkg_playerbots_bot"),
        "retained_runner": runner(&node, retained),
        "goals": node.query_rows("SELECT * FROM pkg_playerbots_goal"),
        "actions": node.query_rows("SELECT * FROM pkg_playerbots_action"),
    });
    let evidence = json!({
        "preceding_wasm_blake3": blake3::hash(&preceding).to_hex().to_string(),
        "expected_preceding_manifest_sha256": expected_manifest_sha,
        "observed_preceding_manifest_sha256": observed_manifest_sha,
        "preceding_build": manifest,
        "current_wasm_blake3": blake3::hash(support::module_bytes()).to_hex().to_string(),
        "preceding": preceding_state,
        "imported": imported,
        "oversized": {"success": oversized.status.success(), "output": output(&oversized)},
        "after_oversized": after_oversized,
        "unsorted": {"success": unsorted.status.success(), "output": output(&unsorted)},
        "after_unsorted": after_unsorted,
        "first_batch": first_batch,
        "scheduler_after_restart": scheduler_after_restart,
        "after_restart_and_resume": after_resume,
        "after_idempotent_replay": replayed,
    });
    save(&node, "populated-cutover", &evidence);

    assert_eq!(guids.len(), MAX_BATCH + 1, "{evidence}");
    assert_eq!(evidence["imported"], evidence["preceding"], "{evidence}");
    assert!(!oversized.status.success(), "{evidence}");
    assert!(
        output(&oversized).contains("migration batch exceeds"),
        "{evidence}"
    );
    assert_eq!(
        evidence["after_oversized"], evidence["imported"],
        "{evidence}"
    );
    assert!(!unsorted.status.success(), "{evidence}");
    assert!(
        output(&unsorted).contains("must be strictly increasing"),
        "{evidence}"
    );
    assert_eq!(
        evidence["after_unsorted"], evidence["imported"],
        "{evidence}"
    );
    assert_eq!(
        evidence["first_batch"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["controller"].as_str().unwrap().contains("cohort"))
            .count(),
        MAX_BATCH,
        "{evidence}"
    );
    assert!(
        evidence["scheduler_after_restart"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{evidence}"
    );
    assert!(
        evidence["after_restart_and_resume"]["bots"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["controller"].as_str().unwrap().contains("cohort")),
        "{evidence}"
    );
    let retained_after = &evidence["after_restart_and_resume"]["retained_runner"][0];
    assert_eq!(
        retained_after["objective"], retained_before["objective"],
        "{evidence}"
    );
    assert_eq!(
        retained_after["generation"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap(),
        retained_before["generation"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            + 1,
        "{evidence}"
    );
    assert_eq!(
        evidence["after_idempotent_replay"], evidence["after_restart_and_resume"],
        "{evidence}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_cutover_leaves_core_escrow_under_legacy_ownership_without_package_transfer_rows() {
    let (node, guids) = current_fixture("playerbots-controller-core-escrow-fence", "1");
    let guid = &guids[0];
    node.assert_call(
        "playerbots_controller_transition_fixture_stage_legacy",
        &[guid, "false"],
    );
    let before = core_escrow_snapshot(&node, guid);
    let actor = format!(r#"{{"guid":{guid},"ownership":null}}"#);
    node.assert_call(
        "begin_transfer",
        &[
            "5091201", &actor, "36", "5098078", "-14.5732", "-385.475", "62.4561", "1.5708", "true",
        ],
    );
    let escrowed = core_escrow_snapshot(&node, guid);
    node.assert_call("playerbots_migrate_legacy_controllers", &[&batch(&guids)]);
    let after_batch = core_escrow_snapshot(&node, guid);
    let evidence = json!({
        "before": before,
        "escrowed": escrowed,
        "after_migration_batch": after_batch,
    });
    save(&node, "core-escrow-fence", &evidence);

    assert!(
        evidence["before"]["bot"]["controller"]
            .as_str()
            .unwrap()
            .contains("legacy"),
        "{evidence}"
    );
    assert_eq!(
        evidence["before"]["runner"].as_array().unwrap().len(),
        1,
        "{evidence}"
    );
    assert!(
        evidence["before"]["runner"][0]["transfer_checkpoint"]
            .as_str()
            .unwrap()
            .contains("none"),
        "{evidence}"
    );
    assert!(
        evidence["before"]["intent"].as_array().unwrap().is_empty(),
        "{evidence}"
    );
    assert!(
        evidence["before"]["legacy_goal"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{evidence}"
    );
    assert!(
        evidence["before"]["escrow"].as_array().unwrap().is_empty(),
        "{evidence}"
    );
    assert_eq!(
        evidence["before"]["character"].as_array().unwrap().len(),
        1,
        "{evidence}"
    );
    assert_eq!(
        evidence["before"]["body"].as_array().unwrap().len(),
        1,
        "{evidence}"
    );
    assert_eq!(
        evidence["escrowed"]["runner"], evidence["before"]["runner"],
        "{evidence}"
    );
    assert!(
        evidence["escrowed"]["intent"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{evidence}"
    );
    assert!(
        evidence["escrowed"]["legacy_goal"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{evidence}"
    );
    assert_eq!(
        evidence["escrowed"]["escrow"].as_array().unwrap().len(),
        1,
        "{evidence}"
    );
    assert_eq!(
        evidence["escrowed"]["character"].as_array().unwrap().len(),
        1,
        "{evidence}"
    );
    assert!(
        evidence["escrowed"]["body"].as_array().unwrap().is_empty(),
        "{evidence}"
    );
    assert_eq!(
        evidence["after_migration_batch"], evidence["escrowed"],
        "{evidence}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_cutover_leaves_pending_intent_checkpoint_and_escrow_under_legacy_ownership() {
    let (node, guids) = current_fixture("playerbots-controller-transfer-fence", "4");
    let mut bots = node.query_rows("SELECT character_guid, class, role FROM pkg_playerbots_bot");
    bots.sort_by_key(|row| row["character_guid"].parse::<u64>().unwrap());
    let companion = bots
        .iter()
        .find(|row| row["class"] == "1" && row["role"] == "0")
        .unwrap()["character_guid"]
        .clone();
    let leader = bots
        .iter()
        .rev()
        .find(|row| row["class"] == "1" && row["role"] == "0")
        .unwrap()["character_guid"]
        .clone();
    let priest = bots.iter().find(|row| row["class"] == "5").unwrap()["character_guid"].clone();
    let mage = bots.iter().find(|row| row["class"] == "8").unwrap()["character_guid"].clone();
    node.assert_call("playerbots_fixture_prepare", &[]);
    node.assert_call(
        "playerbots_fixture_roles_stage",
        &[&companion, &priest, &mage, &leader],
    );
    node.assert_call(
        "playerbots_transfer_fixture_stage",
        &[&companion, &leader, "2"],
    );
    node.assert_sql("DELETE FROM game_creature_move_schedule");
    node.assert_call(
        "playerbots_controller_transition_fixture_stage_legacy",
        &[&companion, "false"],
    );
    node.assert_call("playerbots_fixture_runner_pass_once", &[&companion]);
    let pending = json!({
        "bot": bot(&node, &companion),
        "runner": runner(&node, &companion),
        "intent": node.query_rows(&format!("SELECT * FROM game_bot_transfer_intent WHERE bot_guid = {companion}")),
        "character": node.query_rows(&format!("SELECT * FROM game_character WHERE guid = {companion}")),
    });
    node.assert_call(
        "playerbots_migrate_legacy_controllers",
        &[&batch(std::slice::from_ref(&companion))],
    );
    let after_pending_batch = json!({
        "bot": bot(&node, &companion),
        "runner": runner(&node, &companion),
        "intent": node.query_rows(&format!("SELECT * FROM game_bot_transfer_intent WHERE bot_guid = {companion}")),
        "character": node.query_rows(&format!("SELECT * FROM game_character WHERE guid = {companion}")),
    });
    let intent_id = pending["intent"][0]["id"].as_str().unwrap();
    let actor = format!(r#"{{"guid":{companion},"ownership":null}}"#);
    node.assert_call(
        "begin_transfer",
        &[
            intent_id, &actor, "36", "5098078", "-14.5732", "-385.475", "62.4561", "1.5708", "true",
        ],
    );
    let escrowed = json!({
        "bot": bot(&node, &companion),
        "runner": runner(&node, &companion),
        "intent": node.query_rows(&format!("SELECT * FROM game_bot_transfer_intent WHERE bot_guid = {companion}")),
        "character": node.query_rows(&format!("SELECT * FROM game_character WHERE guid = {companion}")),
        "escrow": node.query_rows(&format!("SELECT transfer_id, character_guid, dest_map_id, dest_instance_id FROM game_transfer_out WHERE character_guid = {companion}")),
    });
    node.assert_call(
        "playerbots_migrate_legacy_controllers",
        &[&batch(std::slice::from_ref(&companion))],
    );
    let after_escrow_batch = json!({
        "bot": bot(&node, &companion),
        "runner": runner(&node, &companion),
        "intent": node.query_rows(&format!("SELECT * FROM game_bot_transfer_intent WHERE bot_guid = {companion}")),
        "character": node.query_rows(&format!("SELECT * FROM game_character WHERE guid = {companion}")),
        "escrow": node.query_rows(&format!("SELECT transfer_id, character_guid, dest_map_id, dest_instance_id FROM game_transfer_out WHERE character_guid = {companion}")),
    });
    let evidence = json!({
        "all_spawned_guids": guids,
        "pending": pending,
        "after_pending_batch": after_pending_batch,
        "escrowed": escrowed,
        "after_escrow_batch": after_escrow_batch,
    });
    save(&node, "transfer-fence", &evidence);
    assert_eq!(
        evidence["after_pending_batch"], evidence["pending"],
        "{evidence}"
    );
    assert_eq!(
        evidence["after_escrow_batch"], evidence["escrowed"],
        "{evidence}"
    );
    assert!(
        evidence["pending"]["bot"]["controller"]
            .as_str()
            .unwrap()
            .contains("legacy"),
        "{evidence}"
    );
    assert!(
        !evidence["pending"]["runner"][0]["transfer_checkpoint"]
            .as_str()
            .unwrap()
            .contains("none"),
        "{evidence}"
    );
    assert_eq!(
        evidence["pending"]["intent"].as_array().unwrap().len(),
        1,
        "{evidence}"
    );
    assert_eq!(
        evidence["escrowed"]["escrow"].as_array().unwrap().len(),
        1,
        "{evidence}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_cutover_waits_for_stale_in_transit_compatibility_then_migrates_once() {
    let (node, guids) = current_fixture("playerbots-controller-stale-transfer", "1");
    let guid = &guids[0];
    node.assert_call(
        "playerbots_controller_transition_fixture_stage_legacy",
        &[guid, "true"],
    );
    let before = json!({
        "bot": bot(&node, guid),
        "runner": runner(&node, guid),
        "goal": node.query_rows(&format!("SELECT * FROM pkg_playerbots_goal WHERE character_guid = {guid}")),
        "character": node.query_rows(&format!("SELECT * FROM game_character WHERE guid = {guid}")),
        "body": node.query_rows(&format!("SELECT * FROM game_world_entity WHERE guid = {guid}")),
    });
    node.assert_call("playerbots_migrate_legacy_controllers", &[&batch(&guids)]);
    let skipped = json!({
        "bot": bot(&node, guid),
        "runner": runner(&node, guid),
        "goal": node.query_rows(&format!("SELECT * FROM pkg_playerbots_goal WHERE character_guid = {guid}")),
        "character": node.query_rows(&format!("SELECT * FROM game_character WHERE guid = {guid}")),
        "body": node.query_rows(&format!("SELECT * FROM game_world_entity WHERE guid = {guid}")),
    });
    std::thread::sleep(std::time::Duration::from_millis(3_100));
    node.assert_call("playerbots_fixture_runner_pass_once", &[guid]);
    let settled = json!({
        "bot": bot(&node, guid),
        "runner": runner(&node, guid),
        "goal": node.query_rows(&format!("SELECT * FROM pkg_playerbots_goal WHERE character_guid = {guid}")),
        "character": node.query_rows(&format!("SELECT * FROM game_character WHERE guid = {guid}")),
        "body": node.query_rows(&format!("SELECT * FROM game_world_entity WHERE guid = {guid}")),
    });
    node.assert_call("playerbots_migrate_legacy_controllers", &[&batch(&guids)]);
    let migrated = json!({
        "bot": bot(&node, guid),
        "runner": runner(&node, guid),
        "goal": node.query_rows(&format!("SELECT * FROM pkg_playerbots_goal WHERE character_guid = {guid}")),
        "character": node.query_rows(&format!("SELECT * FROM game_character WHERE guid = {guid}")),
        "body": node.query_rows(&format!("SELECT * FROM game_world_entity WHERE guid = {guid}")),
    });
    node.assert_call("playerbots_migrate_legacy_controllers", &[&batch(&guids)]);
    let replayed = json!({
        "bot": bot(&node, guid),
        "runner": runner(&node, guid),
        "goal": node.query_rows(&format!("SELECT * FROM pkg_playerbots_goal WHERE character_guid = {guid}")),
        "character": node.query_rows(&format!("SELECT * FROM game_character WHERE guid = {guid}")),
        "body": node.query_rows(&format!("SELECT * FROM game_world_entity WHERE guid = {guid}")),
    });
    let evidence = json!({"before": before, "skipped": skipped, "after_legacy_pass": settled, "migrated": migrated, "replayed": replayed});
    save(&node, "stale-transfer", &evidence);
    assert_eq!(evidence["skipped"], evidence["before"], "{evidence}");
    assert!(
        evidence["before"]["body"].as_array().unwrap().is_empty(),
        "{evidence}"
    );
    assert_ne!(
        evidence["after_legacy_pass"]["goal"], evidence["before"]["goal"],
        "{evidence}"
    );
    assert_eq!(
        evidence["after_legacy_pass"]["body"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "{evidence}"
    );
    assert!(
        evidence["migrated"]["bot"]["controller"]
            .as_str()
            .unwrap()
            .contains("cohort"),
        "{evidence}"
    );
    assert_eq!(evidence["replayed"], evidence["migrated"], "{evidence}");
}
