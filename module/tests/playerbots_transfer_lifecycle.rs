//! Retained Package work across actual Module replacement and owned process restart.

mod support;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use support::{poll_until, Standalone};

const HEAL: u32 = 5_090_100;

fn git(path: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(path)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn digest_files(path: &Path, digest: &mut blake3::Hasher) {
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
            let bytes = std::fs::read(child).unwrap();
            digest.update(&(bytes.len() as u64).to_le_bytes());
            digest.update(&bytes);
        }
    }
}

fn capture(node: &Standalone, guid: &str, case: &str, wasm: &[u8]) -> serde_json::Value {
    let captured_micros = u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_micros(),
    )
    .unwrap();
    let core = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let package = core.join("packages/playerbots");
    let mut content = blake3::Hasher::new();
    digest_files(&package, &mut content);
    let evidence = serde_json::json!({
        "case": case,
        "captured_micros": captured_micros,
        "tested_core": git(core, &["rev-parse", "HEAD"]),
        "tested_collection": git(&package, &["rev-parse", "HEAD"]),
        "core_dirty": !git(core, &["status", "--porcelain"]).is_empty(),
        "collection_dirty": !git(&package, &["status", "--porcelain"]).is_empty(),
        "package_content_identity": content.finalize().to_hex().to_string(),
        "module_wasm_identity": blake3::hash(wasm).to_hex().to_string(),
        "published_wasm_bytes": wasm.len(),
        "compiled_wasm_identity": blake3::hash(support::module_bytes()).to_hex().to_string(),
        "program": node.query_rows("SELECT program_hash FROM st_module"),
        "process_id": node.process_id(),
        "content": "declared private lifecycle fixture; no imported world or client geometry",
        "runner": node.query_rows(&format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}")),
        "entity": node.query_rows(&format!("SELECT guid, health, max_health, x, y, z, map_id, instance_id FROM game_world_entity WHERE guid = {guid}")),
        "pending_cast": node.query_rows(&format!("SELECT * FROM game_pending_cast WHERE caster_guid = {guid}")),
        "spell_events": node.query_rows(&format!("SELECT kind, spell_id FROM game_spell_cast_event WHERE caster_guid = {guid} AND spell_id = {HEAL}")),
        "actions": node.query_rows(&format!("SELECT * FROM pkg_playerbots_action WHERE character_guid = {guid}")),
        "splines": node.query_rows(&format!("SELECT * FROM game_creature_spline WHERE guid = {guid}")),
        "movement_schedule": node.query_rows("SELECT * FROM game_creature_move_schedule"),
        "provisioning": node.query_rows(&format!("SELECT * FROM pkg_playerbots_provisioning WHERE character_guid = {guid}")),
        "character": node.query_rows(&format!("SELECT level, xp, money FROM game_character WHERE guid = {guid}")),
        "items": sorted(node, &format!("SELECT guid, entry, stack_count FROM game_item_instance WHERE owner_guid = {guid}")),
        "skills": sorted(node, &format!("SELECT skill_line, current, max_rank FROM game_player_skill WHERE character_guid = {guid}")),
        "spells": sorted(node, &format!("SELECT spell_id FROM game_player_spell WHERE character_guid = {guid}")),
        "talents": sorted(node, &format!("SELECT talent_id, rank FROM game_character_talent WHERE character_guid = {guid}")),
    });
    let path = support::log_dir().join(format!("{}-{case}.json", node.shard_name()));
    std::fs::write(path, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
    evidence
}

fn sorted(node: &Standalone, query: &str) -> Vec<BTreeMap<String, String>> {
    let mut rows = node.query_rows(query);
    rows.sort();
    rows
}

fn item_count(node: &Standalone, guid: &str, entry: u32) -> u32 {
    node.query_rows(&format!(
        "SELECT stack_count FROM game_item_instance WHERE owner_guid = {guid} AND entry = {entry}"
    ))
    .iter()
    .map(|row| row["stack_count"].parse::<u32>().unwrap())
    .sum()
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_process_restart_resumes_provisioning_without_duplicate_grants() {
    let mut node = Standalone::start_persistent("playerbots-transfer-provision-restart");
    node.publish_module();
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("playerbots_spawn_role", &["1", "1200", "1200", "50", "0"]);
    let guid = node.query_rows("SELECT character_guid FROM pkg_playerbots_bot")[0]
        ["character_guid"]
        .clone();
    node.assert_call("debug_set_level", &[&guid, "20"]);
    node.assert_call("playerbots_fixture_provision_catalog", &[]);
    node.assert_call("playerbots_fixture_provision_complete_profile", &[&guid]);
    node.assert_call("playerbots_fixture_runner_select_cohort", &[&guid]);
    assert_eq!(item_count(&node, &guid, 4496), 0);
    for _ in 0..32 {
        node.assert_call("playerbots_fixture_provision_steps", &[&guid, "1"]);
        if item_count(&node, &guid, 4496) == 4 {
            break;
        }
    }
    let before = capture(
        &node,
        &guid,
        "before-provision-restart",
        support::module_bytes(),
    );
    assert_eq!(item_count(&node, &guid, 4496), 4, "{before}");
    let cursor = before["provisioning"][0]["action_cursor"]
        .as_str()
        .unwrap()
        .parse::<u16>()
        .unwrap();
    assert!(cursor > 0);
    let bags: Vec<_> = before["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["entry"] == "4496")
        .cloned()
        .collect();
    node.restart_persistent();
    let after = capture(
        &node,
        &guid,
        "after-provision-restart",
        support::module_bytes(),
    );
    assert_ne!(before["process_id"], after["process_id"]);
    for field in [
        "program",
        "provisioning",
        "character",
        "items",
        "skills",
        "spells",
        "talents",
    ] {
        assert_eq!(before[field], after[field], "{field}");
    }
    node.assert_call("playerbots_fixture_provision_steps", &[&guid, "1"]);
    let advanced = capture(
        &node,
        &guid,
        "first-resumed-provision-step",
        support::module_bytes(),
    );
    assert!(
        advanced["provisioning"][0]["action_cursor"]
            .as_str()
            .unwrap()
            .parse::<u16>()
            .unwrap()
            > cursor
    );
    for _ in 0..32 {
        let state = node.query_rows(&format!(
            "SELECT cause FROM pkg_playerbots_provisioning WHERE character_guid = {guid}"
        ));
        if state[0]["cause"].to_ascii_lowercase().contains("periodic") {
            break;
        }
        node.assert_call("playerbots_fixture_provision_steps", &[&guid, "1"]);
    }
    let completed = capture(
        &node,
        &guid,
        "completed-resumed-provisioning",
        support::module_bytes(),
    );
    assert!(completed["provisioning"][0]["cause"]
        .as_str()
        .unwrap()
        .to_ascii_lowercase()
        .contains("periodic"));
    assert_eq!(completed["provisioning"][0]["action_cursor"], "0");
    for field in ["character", "skills", "spells", "talents"] {
        assert_eq!(
            before[field], completed[field],
            "completed grant changed: {field}"
        );
    }
    let after_bags: Vec<_> = completed["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["entry"] == "4496")
        .cloned()
        .collect();
    assert_eq!(after_bags, bags);
    for (entry, count) in [
        (4496, 4),
        (117, 10),
        (118, 5),
        (1251, 5),
        (2512, 200),
        (6948, 1),
    ] {
        assert_eq!(item_count(&node, &guid, entry), count, "item {entry}");
    }
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_process_restart_resumes_one_owned_movement_leg() {
    let mut node = Standalone::start_persistent("playerbots-transfer-movement-restart");
    node.publish_module();
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("playerbots_spawn_role", &["1", "1200", "1200", "50", "0"]);
    let guid = node.query_rows("SELECT character_guid FROM pkg_playerbots_bot")[0]
        ["character_guid"]
        .clone();
    node.assert_call("playerbots_fixture_runner_select_cohort", &[&guid]);
    node.assert_call("playerbots_fixture_provision_steps", &[&guid, "64"]);
    node.assert_call("playerbots_fixture_prepare", &[]);
    node.assert_call("playerbots_lifecycle_stage_movement", &[&guid]);
    let before = capture(
        &node,
        &guid,
        "before-movement-restart",
        support::module_bytes(),
    );
    assert_eq!(before["splines"].as_array().unwrap().len(), 1, "{before}");
    assert_eq!(before["runner"][0]["movement_progress"], "null");
    assert!(before["runner"][0]["foreground"]
        .as_str()
        .unwrap()
        .contains("movement"));
    let dx = before["splines"][0]["dx"]
        .as_str()
        .unwrap()
        .parse::<f32>()
        .unwrap();
    let dy = before["splines"][0]["dy"]
        .as_str()
        .unwrap()
        .parse::<f32>()
        .unwrap();
    node.restart_persistent();
    let after = capture(
        &node,
        &guid,
        "after-movement-restart",
        support::module_bytes(),
    );
    assert_ne!(before["process_id"], after["process_id"]);
    assert_eq!(before["program"], after["program"]);
    assert_eq!(before["splines"], after["splines"]);
    assert_eq!(before["movement_schedule"], after["movement_schedule"]);
    assert_eq!(before["entity"], after["entity"]);
    for field in [
        "generation",
        "objective_sequence",
        "objective",
        "foreground",
    ] {
        assert_eq!(
            before["runner"][0][field], after["runner"][0][field],
            "{field}"
        );
    }
    let arrived = poll_until(Duration::from_secs(45), || {
        let rows = node.query_rows(&format!(
            "SELECT x, y FROM game_world_entity WHERE guid = {guid}"
        ));
        let x = rows[0]["x"].parse::<f32>().unwrap();
        let y = rows[0]["y"].parse::<f32>().unwrap();
        (x - dx).abs() < 0.05 && (y - dy).abs() < 0.05
    });
    let arrival = capture(
        &node,
        &guid,
        "resumed-movement-leg",
        support::module_bytes(),
    );
    assert!(arrived, "{arrival}");
    assert!(arrival["splines"].as_array().unwrap().is_empty());
    assert_ne!(arrival["entity"][0]["x"], before["entity"][0]["x"]);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let observed = capture(
        &node,
        &guid,
        "observed-resumed-movement",
        support::module_bytes(),
    );
    assert_eq!(
        observed["runner"][0]["generation"],
        before["runner"][0]["generation"]
    );
    assert_eq!(
        observed["runner"][0]["objective_sequence"],
        before["runner"][0]["objective_sequence"]
    );
    let progress = observed["runner"][0]["movement_progress"].as_str().unwrap();
    assert!(
        (sats_number::<f32>(progress, "x") - dx).abs() < 0.05,
        "{observed}"
    );
    assert!(
        (sats_number::<f32>(progress, "y") - dy).abs() < 0.05,
        "{observed}"
    );
    assert!(
        sats_number::<u64>(progress, "observed_micros")
            >= arrival["captured_micros"].as_u64().unwrap(),
        "{observed}"
    );
}

fn sats_number<T: std::str::FromStr>(value: &str, field: &str) -> T
where
    T::Err: std::fmt::Debug,
{
    let key = format!("{field} = ");
    assert_eq!(value.matches(&key).count(), 1, "{value}");
    value
        .split_once(&key)
        .unwrap()
        .1
        .split([',', ')'])
        .next()
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

fn runner(node: &Standalone, guid: &str) -> BTreeMap<String, String> {
    let mut rows = node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}"
    ));
    assert_eq!(rows.len(), 1);
    rows.remove(0)
}

// A custom section changes the published program identity without changing executable code.
// Equal program hashes skip database update in SpacetimeDB 2.7.1.
fn replacement_bytes(node: &Standalone) -> Vec<u8> {
    let name = b"playerbots-lifecycle-replacement";
    let mut wasm = support::module_bytes().to_vec();
    assert_eq!(&wasm[..8], b"\0asm\x01\0\0\0");
    wasm.extend([
        0,
        u8::try_from(name.len() + 1).unwrap(),
        u8::try_from(name.len()).unwrap(),
    ]);
    wasm.extend(name);
    let path = support::log_dir().join(format!("{}-replacement.wasm", node.shard_name()));
    std::fs::write(path, &wasm).unwrap();
    wasm
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_module_replacement_retains_one_cast_and_controller_generation() {
    let mut node = Standalone::start("playerbots-transfer-cast-replacement");
    node.publish_module();
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("playerbots_spawn_role", &["1", "1200", "1200", "50", "1"]);
    let guid = node.query_rows("SELECT character_guid FROM pkg_playerbots_bot")[0]
        ["character_guid"]
        .clone();
    node.assert_call("playerbots_fixture_prepare", &[]);
    node.assert_sql("DELETE FROM game_event_reaper_schedule");
    node.assert_sql(&format!(
        "UPDATE game_spell SET cast_time_ms = 30000 WHERE spell_id = {HEAL}"
    ));
    node.assert_sql(&format!(
        "UPDATE game_spell_effect SET base_points = 1000000 WHERE spell_id = {HEAL}"
    ));
    node.assert_call("playerbots_fixture_runner_stage", &[&guid, "true"]);
    node.assert_call("playerbots_fixture_runner_select_cohort", &[&guid]);
    let started = poll_until(Duration::from_secs(20), || {
        node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
        runner(&node, &guid)["foreground"].contains("cast")
    });
    let before = capture(&node, &guid, "before-replacement", support::module_bytes());
    assert!(started, "{before}");
    assert_eq!(before["pending_cast"].as_array().unwrap().len(), 1);
    let scheduled_id = before["pending_cast"][0]["scheduled_id"].as_str().unwrap();
    let replacement = replacement_bytes(&node);
    node.publish_module_bytes(&replacement);
    let after = capture(&node, &guid, "after-replacement", &replacement);
    assert_ne!(before["program"], after["program"]);
    assert_eq!(before["process_id"], after["process_id"]);
    assert_eq!(before["pending_cast"], after["pending_cast"]);
    for field in [
        "generation",
        "objective_sequence",
        "objective",
        "foreground",
    ] {
        assert_eq!(
            before["runner"][0][field], after["runner"][0][field],
            "{field}"
        );
    }
    let resolved = poll_until(Duration::from_secs(45), || {
        let state = runner(&node, &guid);
        state["cast_progress"].contains(&format!("scheduled_id = {scheduled_id},"))
    });
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let completed = capture(&node, &guid, "completed-after-replacement", &replacement);
    assert!(resolved, "{completed}");
    assert!(completed["pending_cast"].as_array().unwrap().is_empty());
    assert_eq!(
        completed["entity"][0]["health"],
        completed["entity"][0]["max_health"]
    );
    assert_eq!(
        completed["runner"][0]["generation"],
        before["runner"][0]["generation"]
    );
    assert_eq!(
        completed["runner"][0]["objective_sequence"],
        before["runner"][0]["objective_sequence"]
    );
    let events = completed["spell_events"].as_array().unwrap();
    assert_eq!(events.iter().filter(|row| row["kind"] == "1").count(), 1);
    assert_eq!(events.iter().filter(|row| row["kind"] == "2").count(), 1);
}
