//! Verified geometry coverage in finite playerbot recovery.

mod support;

use lyracore_shared::spatial::MAP_COORD_MAX;
use lyracore_shared::terrain::{cell_index, cell_key, CELL_SIZE};
use lyracore_shared::vmap::{encode, TriClass, VmapTri};
use std::collections::BTreeMap;
use std::time::Duration;
use support::{poll_until, Standalone, POLL_TIMEOUT};

const TARGET: u64 = (0xF130u64 << 48) | (6u64 << 24) | 1;
const GENERATION: u64 = 8_007_001;

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
    std::fs::write(path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
}

#[track_caller]
fn row(node: &Standalone, sql: &str) -> BTreeMap<String, String> {
    node.query_rows(sql)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("fixture row missing for {sql}"))
}

fn prepare(node: &Standalone) -> String {
    node.assert_sql("DELETE FROM game_import_meta WHERE family = 'weather_seed' AND source_sha = '' AND file_hash = '' AND row_count = 2");
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("debug_set_nav_enabled", &["true"]);
    node.assert_call(
        "playerbots_spawn_class_role",
        &["1", "1200", "1200", "50", "1", "0"],
    );
    let guid = row(node, "SELECT character_guid FROM pkg_playerbots_bot")["character_guid"].clone();
    node.assert_call("playerbots_quest_loop_fixture_stage_named", &[&guid]);
    node.assert_call("playerbots_fixture_runner_select_cohort", &[&guid]);
    node.assert_call("playerbots_fixture_provision_steps", &[&guid, "32"]);
    let accepted = poll_until(POLL_TIMEOUT, || {
        node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
        let held = node.query_rows(&format!(
            "SELECT rewarded FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = 7"
        ));
        if held.is_empty() {
            std::thread::sleep(Duration::from_millis(1_100));
        }
        !held.is_empty()
    });
    assert!(accepted, "ordinary runner did not accept Quest 7");
    node.assert_call("playerbots_recovery_fixture_block_quest_target", &[&guid]);
    guid
}

fn wall_blob(cell_x: u16, cell_y: u16) -> Vec<u8> {
    let x_high = MAP_COORD_MAX - f32::from(cell_x) * CELL_SIZE;
    let x_low = x_high - CELL_SIZE;
    let y = MAP_COORD_MAX - (f32::from(cell_y) * CELL_SIZE + CELL_SIZE / 2.0);
    encode(&[
        VmapTri {
            verts: [[x_low, y, 40.0], [x_high, y, 40.0], [x_low, y, 80.0]],
            class: TriClass::M2,
        },
        VmapTri {
            verts: [[x_high, y, 40.0], [x_high, y, 80.0], [x_low, y, 80.0]],
            class: TriClass::M2,
        },
    ])
}

struct CoverageFixture {
    generation: u64,
    retrofit_cell: (u16, u16),
}

fn install_verified_coverage(node: &Standalone, guid: &str) -> CoverageFixture {
    let character = row(
        node,
        &format!("SELECT map_id, x, y FROM game_world_entity WHERE guid = {guid}"),
    );
    let map_id: u32 = character["map_id"].parse().unwrap();
    let center_x = cell_index(character["x"].parse().unwrap()).unwrap();
    let center_y = cell_index(character["y"].parse().unwrap()).unwrap();
    let mut prepared = Vec::new();
    for x in center_x.saturating_sub(1)..=center_x.saturating_add(1).min(1023) {
        for y in center_y.saturating_sub(1)..=center_y.saturating_add(1).min(1023) {
            prepared.push((x, y));
        }
    }
    let retrofit_cell = (center_x.saturating_add(4).min(1023), center_y);
    assert!(!prepared.contains(&retrofit_cell));
    let mut chunks: Vec<_> = prepared
        .iter()
        .copied()
        .chain(std::iter::once(retrofit_cell))
        .map(|(x, y)| (cell_key(map_id, x, y), x, y, wall_blob(x, y)))
        .collect();
    chunks.sort_by_key(|chunk| chunk.0);
    let mut manifest = blake3::Hasher::new();
    manifest.update(b"lyracore-vmap-manifest-v1");
    let mut expected_bytes = 0u64;
    let mut packed = String::new();
    for (key, x, y, blob) in &chunks {
        manifest.update(&key.to_le_bytes());
        manifest.update(&0u32.to_le_bytes());
        manifest.update(&(blob.len() as u32).to_le_bytes());
        manifest.update(blob);
        expected_bytes += blob.len() as u64;
        if !packed.is_empty() {
            packed.push(';');
        }
        let hex: String = blob.iter().map(|byte| format!("{byte:02x}")).collect();
        packed.push_str(&format!("0,{map_id},{x},{y},{hex}"));
    }
    let manifest = manifest.finalize().to_hex().to_string();
    let chunks_len = chunks.len().to_string();
    let expected_bytes = expected_bytes.to_string();
    let manifest = serde_json::to_string(&manifest).unwrap();
    let source = serde_json::to_string("synthetic-verified-coverage-v1").unwrap();
    let selection = serde_json::to_string("quest-target-neighborhood-plus-retrofit-v1").unwrap();
    node.assert_call(
        "stage_vmap_generation",
        &[
            &GENERATION.to_string(),
            &map_id.to_string(),
            &chunks_len,
            &expected_bytes,
            &manifest,
            &source,
            &selection,
        ],
    );
    node.assert_call(
        "append_vmap_generation_chunks",
        &[
            &GENERATION.to_string(),
            &serde_json::to_string(&packed).unwrap(),
        ],
    );
    node.assert_call("verify_vmap_generation", &[&GENERATION.to_string()]);
    let prepared = serde_json::Value::Array(
        prepared
            .iter()
            .map(|(x, y)| serde_json::json!({"cell_x": x, "cell_y": y}))
            .collect(),
    )
    .to_string();
    node.assert_call(
        "prepare_vmap_nav_coverage",
        &[&GENERATION.to_string(), &prepared],
    );
    node.assert_call("finalize_vmap_nav_coverage", &[&GENERATION.to_string()]);
    node.assert_call("activate_vmap_generation", &[&GENERATION.to_string()]);
    node.assert_call("debug_set_nav_coverage_enabled", &["true"]);
    CoverageFixture {
        generation: GENERATION,
        retrofit_cell,
    }
}

fn snapshot(node: &Standalone, guid: &str) -> serde_json::Value {
    serde_json::json!({
        "runner": row(node, &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}")),
        "character": row(node, &format!("SELECT guid, map_id, instance_id, x, y, z FROM game_world_entity WHERE guid = {guid}")),
        "target": row(node, &format!("SELECT guid, map_id, instance_id, x, y, health, dead FROM game_world_entity WHERE guid = {TARGET}")),
        "quest": row(node, &format!("SELECT counts, rewarded FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = 7")),
        "generation": row(node, &format!("SELECT id, map_id, state FROM game_vmap_generation WHERE id = {GENERATION}")),
        "coverage_manifest": row(node, &format!("SELECT generation_id, map_id, cell_count, complete FROM game_vmap_nav_coverage_manifest WHERE generation_id = {GENERATION}")),
        "navigation_revision": node.query_rows("SELECT * FROM game_navigation_revision"),
        "actions": node.query_rows(&format!("SELECT * FROM pkg_playerbots_action WHERE character_guid = {guid}")),
    })
}

fn defer_verified_fight(node: &Standalone, guid: &str) -> serde_json::Value {
    let changed_approach = poll_until(Duration::from_secs(15), || {
        node.assert_call("playerbots_fixture_runner_pass_once", &[guid]);
        let chosen = row(
            node,
            &format!("SELECT chosen FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
        );
        let changed = chosen["chosen"].contains("recoveryPosition");
        if !changed {
            std::thread::sleep(Duration::from_millis(1_100));
        }
        changed
    });
    let changed_state = snapshot(node, guid);
    std::fs::write(
        support::log_dir().join(format!("{}-changed-approach.json", node.shard_name())),
        serde_json::to_vec_pretty(&changed_state).unwrap(),
    )
    .unwrap();
    assert!(
        changed_approach,
        "runner did not change its blocked approach: {changed_state}"
    );
    assert!(changed_state["runner"]["recovery"]
        .as_str()
        .unwrap()
        .contains(&format!("active = (some = (fight = {TARGET}))")));
    node.assert_call("playerbots_fixture_runner_pass_once", &[guid]);
    node.assert_call("playerbots_recovery_fixture_exhaust_attempt", &[guid]);
    let deferred = poll_until(POLL_TIMEOUT, || {
        node.assert_call("playerbots_fixture_runner_pass_once", &[guid]);
        !row(
            node,
            &format!("SELECT deferred_destinations FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
        )["deferred_destinations"]
            .trim_matches(['[', ']', ' '])
            .is_empty()
    });
    let state = snapshot(node, guid);
    std::fs::write(
        support::log_dir().join(format!("{}-deferred.json", node.shard_name())),
        serde_json::to_vec_pretty(&state).unwrap(),
    )
    .unwrap();
    assert!(deferred, "verified blocked fight was not deferred: {state}");
    state
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_recovery_classifies_a_verified_blocked_route_without_missing_coverage() {
    let mut node = Standalone::start("playerbots-recovery-verified-coverage");
    node.publish_module();
    record_inputs(&node);
    let guid = prepare(&node);
    let coverage = install_verified_coverage(&node, &guid);
    let deferred = defer_verified_fight(&node, &guid);
    let path =
        support::log_dir().join(format!("{}-verified-blocked-route.json", node.shard_name()));
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "coverage_generation": coverage.generation,
            "deferred": deferred,
        }))
        .unwrap(),
    )
    .unwrap();
    let runner = deferred["runner"].as_object().unwrap();
    assert!(runner["failures"].as_str().unwrap().contains("noMovement"));
    assert!(!runner["failures"]
        .as_str()
        .unwrap()
        .contains("missingImportedCoverage"));
    assert!(runner["recovery"].as_str().unwrap().contains(&format!(
        "verifiedCells = (generation_id = {}",
        coverage.generation
    )));
    assert!(runner["recovery"]
        .as_str()
        .unwrap()
        .contains(&format!("fight = {TARGET}")));
    assert!(runner["recovery"]
        .as_str()
        .unwrap()
        .contains("status = blocked"));
    assert_eq!(deferred["coverage_manifest"]["complete"], "true");
    assert_eq!(deferred["quest"]["rewarded"], "false");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_recovery_retries_exact_work_after_active_coverage_grows() {
    let mut node = Standalone::start("playerbots-recovery-coverage-retrofit");
    node.publish_module();
    record_inputs(&node);
    let guid = prepare(&node);
    let coverage = install_verified_coverage(&node, &guid);
    let before = defer_verified_fight(&node, &guid);
    let retrofit = serde_json::json!([{
        "cell_x": coverage.retrofit_cell.0,
        "cell_y": coverage.retrofit_cell.1,
    }])
    .to_string();
    node.assert_call(
        "prepare_vmap_nav_coverage",
        &[&coverage.generation.to_string(), &retrofit],
    );
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let after = snapshot(&node, &guid);
    let path = support::log_dir().join(format!(
        "{}-active-coverage-retrofit.json",
        node.shard_name()
    ));
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "retrofit_cell": {
                "cell_x": coverage.retrofit_cell.0,
                "cell_y": coverage.retrofit_cell.1,
            },
            "before": before,
            "after": after,
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(before["generation"]["id"], after["generation"]["id"]);
    assert_eq!(
        before["coverage_manifest"]["generation_id"],
        after["coverage_manifest"]["generation_id"]
    );
    assert!(before["navigation_revision"].as_array().unwrap().is_empty());
    assert_eq!(after["navigation_revision"][0]["revision"], "1");
    let failed_fight = before["runner"]["recovery"]
        .as_str()
        .unwrap()
        .split("work = ")
        .find(|attempt| attempt.starts_with(&format!("(fight = {TARGET})")))
        .expect("the original quest fight must have a Recovery Attempt");
    assert!(
        failed_fight.contains("deferred_until_micros = (some"),
        "{before}"
    );
    assert!(!after["runner"]["recovery"]
        .as_str()
        .unwrap()
        .contains("deferred_until_micros = (some"));
    assert!(after["runner"]["deferred_destinations"]
        .as_str()
        .unwrap()
        .is_empty());
    assert!(after["runner"]["chosen"]
        .as_str()
        .unwrap()
        .contains(&format!("attack = {TARGET}")));
    assert!(after["runner"]["recovery"]
        .as_str()
        .unwrap()
        .contains(&format!("active = (some = (fight = {TARGET}))")));
    assert_eq!(before["quest"], after["quest"]);
}
