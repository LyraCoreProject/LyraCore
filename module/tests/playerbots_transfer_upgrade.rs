//! Populated immediate-predecessor upgrade before enabling durable bot Transfer.

mod support;

use std::collections::BTreeMap;
use std::time::Duration;
use support::{poll_until, Standalone};

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

struct PrecedingTransfer {
    wasm: Vec<u8>,
    manifest: serde_json::Value,
}

fn preceding_transfer() -> PrecedingTransfer {
    let wasm_path = std::env::var_os("PLAYERBOTS_TRANSFER_PRECEDING_WASM")
        .expect("PLAYERBOTS_TRANSFER_PRECEDING_WASM must name the merged PB-009 Wasm");
    let manifest_path = std::env::var_os("PLAYERBOTS_TRANSFER_PRECEDING_MANIFEST")
        .expect("PLAYERBOTS_TRANSFER_PRECEDING_MANIFEST must describe that Wasm build");
    let core_path = std::env::var_os("PLAYERBOTS_TRANSFER_PRECEDING_CORE")
        .expect("PLAYERBOTS_TRANSFER_PRECEDING_CORE must name the clean merged Core checkout");
    let collection_path = std::env::var_os("PLAYERBOTS_TRANSFER_PRECEDING_COLLECTION").expect(
        "PLAYERBOTS_TRANSFER_PRECEDING_COLLECTION must name the clean merged Package checkout",
    );
    let core_path = std::path::Path::new(&core_path);
    let collection_path = std::path::Path::new(&collection_path);
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(manifest_path).unwrap()).unwrap();
    let wasm = std::fs::read(&wasm_path).unwrap();
    let expected_core = git(core_path, &["rev-parse", "HEAD"]);
    let expected_core_tree = git(core_path, &["rev-parse", "HEAD^{tree}"]);
    let expected_collection = git(collection_path, &["rev-parse", "HEAD"]);
    let expected_collection_tree = git(collection_path, &["rev-parse", "HEAD^{tree}"]);
    let expected_playerbots_tree = git(collection_path, &["rev-parse", "HEAD:playerbots"]);
    let mut package_digest = blake3::Hasher::new();
    digest_files(&collection_path.join("playerbots"), &mut package_digest);
    let expected_package_identity = package_digest.finalize().to_hex().to_string();
    for (field, expected) in [
        ("core", expected_core.as_str()),
        ("collection", expected_collection.as_str()),
        ("core_tree", expected_core_tree.as_str()),
        ("collection_tree", expected_collection_tree.as_str()),
        ("playerbots_tree", expected_playerbots_tree.as_str()),
        (
            "package_content_identity",
            expected_package_identity.as_str(),
        ),
    ] {
        assert_eq!(manifest[field], expected, "preceding manifest {field}");
    }
    assert_eq!(manifest["core_dirty"], false);
    assert_eq!(manifest["collection_dirty"], false);
    assert_eq!(manifest["rust"], "1.93.0");
    assert_eq!(manifest["spacetimedb"], "2.7.1");
    assert_eq!(manifest["target"], "wasm32-unknown-unknown");
    assert_eq!(manifest["profile"], "release");
    assert_eq!(manifest["features"], serde_json::json!(["debug_reducers"]));
    assert_eq!(
        manifest["installed_packages"],
        serde_json::json!(["dungeons", "example", "fire_nova", "playerbots"])
    );
    assert_eq!(manifest["wasm_bytes"].as_u64(), Some(wasm.len() as u64));

    assert!(git(core_path, &["status", "--porcelain"]).is_empty());
    assert!(git(collection_path, &["status", "--porcelain"]).is_empty());
    let sha256 = std::process::Command::new("sha256sum")
        .arg(&wasm_path)
        .output()
        .unwrap();
    assert!(sha256.status.success());
    let sha256 = String::from_utf8(sha256.stdout).unwrap();
    assert_eq!(
        sha256.split_whitespace().next().unwrap(),
        manifest["wasm_sha256"].as_str().unwrap()
    );
    if let Some(expected) = manifest["wasm_blake3"].as_str() {
        assert_eq!(blake3::hash(&wasm).to_hex().as_str(), expected);
    }
    PrecedingTransfer { wasm, manifest }
}

fn row(node: &Standalone, query: &str) -> BTreeMap<String, String> {
    let mut rows = node.query_rows(query);
    assert_eq!(rows.len(), 1, "{query}");
    rows.remove(0)
}

fn state(node: &Standalone, guid: &str) -> serde_json::Value {
    serde_json::json!({
        "program": node.query_rows("SELECT program_hash FROM st_module"),
        "runner": row(node, &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}")),
        "retained_quest": node.query_rows(&format!("SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}")),
        "quests": node.query_rows(&format!("SELECT * FROM game_character_quest WHERE character_guid = {guid}")),
        "pending_cast": node.query_rows(&format!("SELECT * FROM game_pending_cast WHERE caster_guid = {guid}")),
        "provisioning": node.query_rows(&format!("SELECT * FROM pkg_playerbots_provisioning WHERE character_guid = {guid}")),
        "character": node.query_rows(&format!("SELECT * FROM game_character WHERE guid = {guid}")),
        "bot_intents": node.query_rows("SELECT * FROM game_bot_transfer_intent"),
        "source_escrows": node.query_rows("SELECT * FROM game_transfer_out"),
        "arrivals": node.query_rows("SELECT * FROM game_transfer_in"),
    })
}

fn save(node: &Standalone, phase: &str, evidence: &serde_json::Value) {
    let path = support::log_dir().join(format!("{}-{phase}.json", node.shard_name()));
    std::fs::write(path, serde_json::to_vec_pretty(evidence).unwrap()).unwrap();
}

#[test]
#[ignore = "requires merged PB-009 Wasm, SpacetimeDB, and the playerbots Package"]
fn playerbots_transfer_upgrades_populated_predecessor_without_a_checkpoint() {
    let preceding = preceding_transfer();
    assert_eq!(
        preceding.manifest["core"],
        "2f2b1f35dde90e52dd3b0d3e16d44bb3ffe41a2d"
    );
    let current = support::module_bytes();
    assert_ne!(blake3::hash(&preceding.wasm), blake3::hash(current));
    let mut node = Standalone::start("playerbots-transfer-pb009-upgrade");
    node.publish_module_bytes(&preceding.wasm);
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    let imports =
        node.query_rows("SELECT family, source_sha, file_hash, row_count FROM game_import_meta");
    assert_eq!(imports.len(), 1);
    assert_eq!(imports[0]["family"], "weather_seed");
    assert_eq!(imports[0]["source_sha"], "");
    assert_eq!(imports[0]["file_hash"], "");
    assert_eq!(imports[0]["row_count"], "2");
    node.assert_sql("DELETE FROM game_import_meta WHERE family = 'weather_seed' AND source_sha = '' AND file_hash = '' AND row_count = 2");
    let x = lyracore_shared::terrain::cell_index(1200.0).unwrap();
    let y = lyracore_shared::terrain::cell_index(1200.0).unwrap();
    let navigation: Vec<_> = (x - 2..=x + 2)
        .flat_map(|cx| (y - 2..=y + 2).map(move |cy| format!("0,{cx},{cy},50,,")))
        .collect();
    node.assert_call("import_nav_chunks", &[&navigation.join(";")]);
    node.assert_call("debug_set_nav_enabled", &["true"]);
    node.assert_call("playerbots_spawn_role", &["1", "1200", "1200", "50", "0"]);
    let guid =
        row(&node, "SELECT character_guid FROM pkg_playerbots_bot")["character_guid"].clone();
    node.assert_call("playerbots_fixture_runner_select_cohort", &[&guid]);
    node.assert_call("debug_learn_spell", &[&guid, "355"]);
    node.assert_call("playerbots_fixture_provision_steps", &[&guid, "1"]);
    node.assert_sql(&format!("UPDATE pkg_playerbots_provisioning SET next_repair_micros = 9223372036854775807 WHERE character_guid = {guid}"));
    node.assert_call("playerbots_quest_fixture_stage", &[&guid]);
    node.assert_call(
        "playerbots_quest_fixture_move_creature_spawn",
        &["6", "1230"],
    );
    node.assert_call("playerbots_quest_fixture_refresh", &[]);
    node.assert_call("playerbots_quest_fixture_admit_accept", &[&guid, "7"]);
    node.assert_call("playerbots_fixture_runner_stage", &[&guid, "true"]);
    node.assert_sql("UPDATE game_spell SET cast_time_ms = 60000 WHERE spell_id = 5090100");
    let pending = poll_until(Duration::from_secs(30), || {
        node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
        let runner = row(&node, &format!("SELECT objective, foreground FROM pkg_playerbots_runner WHERE character_guid = {guid}"));
        let ready = runner["objective"].contains("quest") && runner["foreground"].contains("cast");
        if !ready {
            std::thread::sleep(Duration::from_millis(1100));
        }
        ready
    });
    node.assert_call("playerbots_fixture_freeze", &[&guid]);
    let before = state(&node, &guid);
    let before_pid = node.process_id();
    save(
        &node,
        "populated-predecessor",
        &serde_json::json!({
            "phase": "populated-before-upgrade",
            "preceding_build": preceding.manifest,
            "published_wasm_blake3": blake3::hash(&preceding.wasm).to_hex().to_string(),
            "process_id": before_pid,
            "state": before,
        }),
    );
    assert!(pending, "{before}");
    assert_eq!(before["pending_cast"].as_array().unwrap().len(), 1);
    assert!(before["runner"]["foreground"]
        .as_str()
        .unwrap()
        .contains("spell = 5090100"));
    assert!(before["runner"]["transfer_checkpoint"].is_null());
    assert!(before["runner"]["companion_order_revision"].is_string());
    assert_eq!(before["retained_quest"].as_array().unwrap().len(), 1);
    assert_eq!(before["retained_quest"][0]["quest_entry"], "7");
    assert_eq!(before["provisioning"].as_array().unwrap().len(), 1);
    for field in ["bot_intents", "source_escrows", "arrivals"] {
        assert!(
            before[field].as_array().unwrap().is_empty(),
            "predecessor must drain {field}"
        );
    }
    node.publish_module_bytes(current);
    let after = state(&node, &guid);
    let core = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let package = core.join("packages/playerbots");
    let mut content = blake3::Hasher::new();
    digest_files(&package, &mut content);
    save(
        &node,
        "populated-upgrade",
        &serde_json::json!({
            "phase": "after-upgrade",
            "tested_core": git(core, &["rev-parse", "HEAD"]),
            "tested_collection": git(&package, &["rev-parse", "HEAD"]),
            "core_dirty": !git(core, &["status", "--porcelain"]).is_empty(),
            "collection_dirty": !git(&package, &["status", "--porcelain"]).is_empty(),
            "package_content_identity": content.finalize().to_hex().to_string(),
            "published_wasm_blake3": blake3::hash(current).to_hex().to_string(),
            "process_id": node.process_id(),
            "state": after,
        }),
    );
    assert_eq!(node.process_id(), before_pid);
    assert_ne!(before["program"], after["program"]);
    for (field, value) in before["runner"].as_object().unwrap() {
        assert_eq!(
            &after["runner"][field], value,
            "retained runner field {field}"
        );
    }
    assert_eq!(
        after["runner"].as_object().unwrap().len(),
        before["runner"].as_object().unwrap().len() + 1
    );
    assert_eq!(after["runner"]["transfer_checkpoint"], "(none = ())");
    for field in [
        "retained_quest",
        "quests",
        "pending_cast",
        "provisioning",
        "character",
        "bot_intents",
        "source_escrows",
        "arrivals",
    ] {
        assert_eq!(before[field], after[field], "{field}");
    }
}
