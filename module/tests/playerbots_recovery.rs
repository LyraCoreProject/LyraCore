//! Finite recovery through the ordinary Package scheduler and owning gameplay Gates.

mod support;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use support::{poll_until, Standalone, POLL_TIMEOUT};

const TARGET: u64 = (0xF130u64 << 48) | (6u64 << 24) | 1;

fn record_inputs(node: &Standalone) {
    let core = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let package = core.join("packages/playerbots");
    let git = |path: &std::path::Path, args: &[&str]| {
        let result = std::process::Command::new("git")
            .current_dir(path)
            .args(args)
            .output()
            .unwrap();
        assert!(result.status.success());
        String::from_utf8(result.stdout).unwrap().trim().to_string()
    };
    fn digest_files(path: &std::path::Path, digest: &mut blake3::Hasher) {
        let mut children: Vec<_> = std::fs::read_dir(path)
            .unwrap()
            .map(|e| e.unwrap().path())
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
                let contents = std::fs::read(&child).unwrap();
                digest.update(&(contents.len() as u64).to_le_bytes());
                digest.update(&contents);
            }
        }
    }
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

fn row(node: &Standalone, sql: &str) -> BTreeMap<String, String> {
    node.query_rows(sql)
        .into_iter()
        .next()
        .expect("fixture row missing")
}

fn prepare(node: &Standalone) -> String {
    let imports =
        node.query_rows("SELECT family, source_sha, file_hash, row_count FROM game_import_meta");
    assert_eq!(imports.len(), 1);
    assert_eq!(imports[0]["family"], "weather_seed");
    assert_eq!(imports[0]["source_sha"], "");
    assert_eq!(imports[0]["file_hash"], "");
    assert_eq!(imports[0]["row_count"], "2");
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
        let held = node.query_rows(&format!("SELECT rewarded FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = 7"));
        if held.is_empty() {
            std::thread::sleep(Duration::from_millis(1_100));
        }
        !held.is_empty()
    });
    assert!(accepted, "ordinary runner did not accept Quest 7");
    node.assert_call("playerbots_recovery_fixture_block_quest_target", &[&guid]);
    guid
}

fn snapshot(node: &Standalone, guid: &str, elapsed: Duration) -> serde_json::Value {
    serde_json::json!({
        "elapsed_seconds": elapsed.as_secs_f64(),
        "runner": row(node, &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}")),
        "character": row(node, &format!("SELECT guid, x, y, z, health, dead FROM game_world_entity WHERE guid = {guid}")),
        "target": row(node, &format!("SELECT guid, x, y, health, max_health, dead FROM game_world_entity WHERE guid = {TARGET}")),
        "quest": row(node, &format!("SELECT counts, rewarded FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = 7")),
        "actions": node.query_rows(&format!("SELECT * FROM pkg_playerbots_action WHERE character_guid = {guid}")),
    })
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_recovery_changes_a_stalled_attack_then_defers_without_false_progress() {
    let mut node = Standalone::start("playerbots-recovery-stalled-attack");
    node.publish_module();
    record_inputs(&node);
    let guid = prepare(&node);
    let armed = poll_until(POLL_TIMEOUT, || {
        node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
        let chosen = row(
            &node,
            &format!("SELECT chosen FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
        );
        if chosen["chosen"].contains("attack") {
            true
        } else {
            std::thread::sleep(Duration::from_millis(1_100));
            false
        }
    });
    let first = snapshot(&node, &guid, Duration::ZERO);
    let initial_path = support::log_dir().join(format!("{}-armed.json", node.shard_name()));
    std::fs::write(initial_path, serde_json::to_vec_pretty(&first).unwrap()).unwrap();
    assert!(
        armed,
        "ordinary runner did not choose the blocked quest attack"
    );
    let start = Instant::now();
    node.assert_call("playerbots_fixture_companion_due", &[&guid]);
    let mut samples = vec![first];
    while start.elapsed() < Duration::from_secs(34) {
        std::thread::sleep(Duration::from_millis(750));
        samples.push(snapshot(&node, &guid, start.elapsed()));
    }
    let path = support::log_dir().join(format!("{}-live-recovery.json", node.shard_name()));
    std::fs::write(path, serde_json::to_vec_pretty(&samples).unwrap()).unwrap();
    assert!(
        samples[0]["runner"]["chosen"]
            .as_str()
            .unwrap()
            .contains("attack"),
        "{:?}",
        samples[0]
    );
    let initial_health = samples[0]["target"]["health"].as_str().unwrap();
    for sample in &samples {
        assert_eq!(sample["target"]["health"].as_str().unwrap(), initial_health);
        assert_eq!(sample["quest"]["counts"], samples[0]["quest"]["counts"]);
        assert_eq!(sample["character"]["x"], samples[0]["character"]["x"]);
        assert_eq!(sample["character"]["y"], samples[0]["character"]["y"]);
    }
    let changed = samples
        .iter()
        .find(|sample| {
            sample["runner"]["chosen"]
                .as_str()
                .unwrap()
                .contains("move")
        })
        .expect("stalled attack never changed approach");
    assert!(
        changed["elapsed_seconds"].as_f64().unwrap() <= 12.0,
        "{changed}"
    );
    let deferred = samples
        .iter()
        .find(|sample| sample["runner"]["deferred_destinations"].as_str().unwrap() != "[]")
        .expect("blocked destination was never deferred");
    assert!(
        deferred["elapsed_seconds"].as_f64().unwrap() <= 32.0,
        "{deferred}"
    );
    assert!(
        deferred["runner"]["failures"]
            .as_str()
            .unwrap()
            .contains("noMovement"),
        "{deferred}"
    );
}
