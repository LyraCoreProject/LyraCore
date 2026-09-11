//! Finite recovery through the ordinary Package scheduler and owning gameplay Gates.

mod support;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use support::{poll_until, Standalone, POLL_TIMEOUT};

const TARGET: u64 = (0xF130u64 << 48) | (6u64 << 24) | 1;
const ALTERNATIVE_TARGET: u64 = (0xF130u64 << 48) | (823u64 << 24) | 1;

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

struct PrecedingRecovery {
    wasm: Vec<u8>,
    manifest: serde_json::Value,
}

fn preceding_recovery() -> PrecedingRecovery {
    let wasm_path = std::env::var_os("PLAYERBOTS_RECOVERY_PRECEDING_WASM")
        .expect("PLAYERBOTS_RECOVERY_PRECEDING_WASM must name the merged PB-007 Wasm");
    let manifest_path = std::env::var_os("PLAYERBOTS_RECOVERY_PRECEDING_MANIFEST")
        .expect("PLAYERBOTS_RECOVERY_PRECEDING_MANIFEST must describe that Wasm build");
    let core_path = std::env::var_os("PLAYERBOTS_RECOVERY_PRECEDING_CORE")
        .expect("PLAYERBOTS_RECOVERY_PRECEDING_CORE must name the clean merged Core checkout");
    let collection_path = std::env::var_os("PLAYERBOTS_RECOVERY_PRECEDING_COLLECTION").expect(
        "PLAYERBOTS_RECOVERY_PRECEDING_COLLECTION must name the clean merged Package checkout",
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
    PrecedingRecovery { wasm, manifest }
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

#[track_caller]
fn row(node: &Standalone, sql: &str) -> BTreeMap<String, String> {
    let rows = node.query_rows(sql);
    let Some(row) = rows.into_iter().next() else {
        let path = support::log_dir().join(format!("{}-missing-row.json", node.shard_name()));
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "query": sql, "caller": std::panic::Location::caller().to_string(),
            }))
            .unwrap(),
        )
        .unwrap();
        panic!("fixture row missing for {sql}");
    };
    row
}

fn route_endpoint(outcome: &str) -> Option<(f32, f32)> {
    let (_, endpoint) = outcome.split_once("endpoint = (x = ")?;
    let (x, endpoint) = endpoint.split_once(", y = ")?;
    let (y, _) = endpoint.split_once(')')?;
    Some((x.parse().ok()?, y.parse().ok()?))
}

fn retained_route_from(recovery: &str) -> Option<(f32, f32)> {
    let (_, from) = recovery.split_once("route = (some = (from = (x = ")?;
    let (x, from) = from.split_once(", y = ")?;
    let (y, _) = from.split_once(')')?;
    Some((x.parse().ok()?, y.parse().ok()?))
}

fn tuple_field<'a>(value: &'a str, name: &str) -> Option<&'a str> {
    let tuple = &value[value.find(name)? + name.len()..];
    if !tuple.starts_with('(') {
        return None;
    }
    let mut depth = 0;
    for (index, byte) in tuple.bytes().enumerate() {
        match byte {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&tuple[..=index]);
                }
            }
            _ => {}
        }
    }
    None
}

fn prepare_open(node: &Standalone) -> String {
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
    guid
}

fn prepare(node: &Standalone) -> String {
    let guid = prepare_open(node);
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
    assert!(samples[0]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|action| {
            action["outcome"]
                .as_str()
                .unwrap()
                .contains("attackAccepted")
        }));
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
        .find(|sample| {
            !sample["runner"]["deferred_destinations"]
                .as_str()
                .unwrap()
                .trim_matches(['[', ']', ' '])
                .is_empty()
        })
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
    let alternative = samples
        .iter()
        .skip_while(|sample| {
            sample["elapsed_seconds"].as_f64().unwrap()
                < deferred["elapsed_seconds"].as_f64().unwrap()
        })
        .find(|sample| {
            let runner = &sample["runner"];
            let chosen = runner["chosen"].as_str().unwrap();
            chosen.contains(&format!("move = (entity = {ALTERNATIVE_TARGET})"))
                && chosen.contains("reason = (quest = ())")
                && runner["last_outcome"].as_str() == Some("(waiting = ())")
                && sample["actions"].as_array().unwrap().iter().any(|action| {
                    action["observed_micros"] == runner["observed_micros"]
                        && action["kind"].as_str() == Some("(move = ())")
                        && {
                            let outcome = action["outcome"].as_str().unwrap();
                            outcome.contains("destination = (x = 1202, y = 1200")
                                && outcome.contains(
                                    "route = (from = (x = 1357, y = 1200), endpoint = (x = 1357, y = 1200), first_waypoint = (none = ()), status = (blocked = ())",
                                )
                                && outcome.contains("coverage = (unknown = ())")
                                && outcome.contains("arrived = false")
                                && outcome.contains("last_advance_micros = (none = ())")
                        }
                })
        })
        .expect("failed quest target prevented the exact eligible Quest attempt");
    assert_ne!(
        alternative["runner"]["objective_sequence"],
        samples[0]["runner"]["objective_sequence"]
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_recovery_exhausts_quest_targets_then_earns_alternative_quest_credit() {
    let mut node = Standalone::start("playerbots-recovery-objective-fallback");
    node.publish_module();
    record_inputs(&node);
    let guid = prepare_open(&node);
    node.assert_call(
        "playerbots_recovery_fixture_keep_two_quest_targets",
        &[&guid],
    );
    let original = row(
        &node,
        &format!("SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"),
    );
    let original_runner = row(
        &node,
        &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
    );
    let original_destination = tuple_field(&original_runner["objective"], "destination = ")
        .expect("the original Quest Objective destination is absent")
        .to_string();
    let original_identity = original["runner_objective_identity"].clone();
    let xp = row(
        &node,
        &format!("SELECT xp FROM game_world_entity WHERE guid = {guid}"),
    )["xp"]
        .parse::<u32>()
        .unwrap();
    let started = Instant::now();
    let exhaustion_path = support::log_dir().join(format!(
        "{}-objective-fallback-exhaustion.json",
        node.shard_name()
    ));
    let mut exhaustion_samples = Vec::new();

    for target in [TARGET, TARGET + 1] {
        let selected = poll_until(POLL_TIMEOUT, || {
            node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
            let runner = row(
                &node,
                &format!("SELECT character_guid, recovery FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
            );
            if runner["recovery"].contains(&format!("fight = {target}")) {
                true
            } else {
                std::thread::sleep(Duration::from_millis(1_100));
                false
            }
        });
        assert!(selected, "Quest target {target} was not selected");
        if target == TARGET + 1 {
            node.assert_call(
                "playerbots_recovery_fixture_expire_quest_target",
                &[&guid, &TARGET.to_string()],
            );
        }
        node.assert_call("playerbots_recovery_fixture_exhaust_attempt", &[&guid]);
        node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
        let current_objective = row(
            &node,
            &format!("SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"),
        );
        exhaustion_samples.push(serde_json::json!({
            "target_guid": target,
            "target": row(
                &node,
                &format!("SELECT guid, x, y, z, health, dead FROM game_world_entity WHERE guid = {target}"),
            ),
            "sample": snapshot(&node, &guid, started.elapsed()),
            "objective": current_objective.clone(),
        }));
        std::fs::write(
            &exhaustion_path,
            serde_json::to_vec_pretty(&exhaustion_samples).unwrap(),
        )
        .unwrap();
        assert_eq!(
            current_objective["runner_objective_identity"],
            original_identity
        );
    }

    let fallback = row(
        &node,
        &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
    );
    assert_eq!(fallback["objective_sequence"], original_identity);
    assert!(fallback["objective"].contains("stage = (deferred = ())"));
    assert!(fallback["deferred_destinations"].contains("x = 1360"));
    assert!(fallback["chosen"].contains("hold"));
    assert!(fallback["chosen"].contains("reason = (quest = ())"));
    assert!(fallback["failures"].contains("noMovement"));
    assert_eq!(fallback["recovery"].matches("work = (fight = ").count(), 2);
    assert_eq!(
        fallback["recovery"]
            .matches("deferred_until_micros = (some =")
            .count(),
        2
    );
    for target in [TARGET, TARGET + 1] {
        assert!(
            fallback["recovery"].contains(&format!("work = (fight = {target})")),
            "{fallback:?}"
        );
    }
    let observed = fallback["observed_micros"].parse::<i64>().unwrap();
    let next_eligible = fallback["next_eligible_micros"].parse::<i64>().unwrap();
    assert!((1..=2_000_000).contains(&next_eligible.saturating_sub(observed)));

    std::thread::sleep(Duration::from_millis(1_100));
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let resumed = row(
        &node,
        &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
    );
    let retained_alternative = row(
        &node,
        &format!("SELECT quest_entry, runner_objective_identity FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"),
    );
    assert_ne!(resumed["objective_sequence"], original_identity);
    assert_eq!(retained_alternative["quest_entry"], "5261");
    assert_eq!(
        retained_alternative["runner_objective_identity"],
        resumed["objective_sequence"]
    );
    assert_eq!(
        resumed["deferred_destinations"]
            .matches("destination = ")
            .count(),
        1
    );
    assert!(resumed["deferred_destinations"].contains(&original_destination));
    assert!(resumed["chosen"].contains("acceptQuest"));
    assert!(resumed["chosen"].contains(&ALTERNATIVE_TARGET.to_string()));
    assert!(resumed["chosen"].contains("quest = 5261"));
    assert!(resumed["chosen"].contains("reason = (quest = ())"));
    for target in [TARGET, TARGET + 1] {
        assert!(!resumed["recovery"].contains(&format!("fight = {target}")));
    }

    let credited = poll_until(Duration::from_secs(15), || {
        node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
        let rewarded = node
            .query_rows(&format!(
                "SELECT rewarded FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = 5261"
            ))
            .first()
            .is_some_and(|quest| quest["rewarded"] == "true");
        if !rewarded {
            std::thread::sleep(Duration::from_millis(1_100));
        }
        rewarded
    });
    let runner = row(
        &node,
        &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
    );
    let alternative = row(
        &node,
        &format!("SELECT rewarded FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = 5261"),
    );
    let after_xp = row(
        &node,
        &format!("SELECT xp FROM game_world_entity WHERE guid = {guid}"),
    )["xp"]
        .parse::<u32>()
        .unwrap();
    let quest_seven = row(
        &node,
        &format!("SELECT counts, rewarded FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = 7"),
    );
    let turnin = row(
        &node,
        &format!("SELECT turnin_count FROM pkg_playerbots_quest_turnin_fixture WHERE character_guid = {guid} AND quest_entry = 5261"),
    );
    let actions = node.query_rows(&format!(
        "SELECT kind, quest_entry FROM pkg_playerbots_action WHERE character_guid = {guid}"
    ));
    let path = support::log_dir().join(format!("{}-objective-fallback.json", node.shard_name()));
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "original": original,
            "original_runner": original_runner,
            "runner": runner,
            "fallback": fallback,
            "resumed": resumed,
            "retained_alternative": retained_alternative,
            "alternative": alternative,
            "quest_seven": quest_seven,
            "turnin": turnin,
            "actions": actions,
            "xp_before": xp,
            "xp_after": after_xp,
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(
        credited,
        "failed Quest targets prevented ordinary alternative Quest credit"
    );
    assert_eq!(alternative["rewarded"], "true");
    assert_eq!(quest_seven["counts"], "[0]");
    assert_eq!(quest_seven["rewarded"], "false");
    assert_eq!(turnin["turnin_count"], "1");
    assert!(actions.iter().any(|action| {
        action["kind"].contains("turnInQuest") && action["quest_entry"] == "5261"
    }));
    assert!(after_xp > xp);
    assert_ne!(runner["objective_sequence"], original_identity);
    for target in [TARGET, TARGET + 1] {
        assert!(!runner["recovery"].contains(&format!("fight = {target}")));
    }
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_recovery_failure_memory_survives_a_persistent_process_restart() {
    let mut node = Standalone::start_persistent("playerbots-recovery-restart");
    node.publish_module();
    record_inputs(&node);
    let guid = prepare(&node);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    node.assert_call("playerbots_fixture_companion_due", &[&guid]);
    let start = Instant::now();
    let deferred = poll_until(Duration::from_secs(40), || {
        let state = row(&node, &format!("SELECT character_guid, deferred_destinations FROM pkg_playerbots_runner WHERE character_guid = {guid}"));
        !state["deferred_destinations"]
            .trim_matches(['[', ']', ' '])
            .is_empty()
    });
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let before = snapshot(&node, &guid, start.elapsed());
    let before_pid = node.process_id();
    let before_path = support::log_dir().join(format!("{}-before-restart.json", node.shard_name()));
    std::fs::write(before_path, serde_json::to_vec_pretty(&before).unwrap()).unwrap();
    assert!(deferred, "{before}");
    node.restart_persistent();
    let after_pid = node.process_id();
    let after = snapshot(&node, &guid, start.elapsed());
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let resumed = snapshot(&node, &guid, start.elapsed());
    let path = support::log_dir().join(format!("{}-process-restart.json", node.shard_name()));
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "before_pid": before_pid, "after_pid": after_pid, "before": before, "after": after,
            "resumed": resumed,
        }))
        .unwrap(),
    )
    .unwrap();
    assert_ne!(before_pid, after_pid);
    assert_eq!(before["runner"]["recovery"], after["runner"]["recovery"]);
    assert_eq!(
        before["runner"]["deferred_destinations"],
        after["runner"]["deferred_destinations"]
    );
    assert_eq!(
        before["runner"]["objective_sequence"],
        after["runner"]["objective_sequence"]
    );
    assert_eq!(before["quest"], after["quest"]);
    assert!(!resumed["runner"]["chosen"]
        .as_str()
        .unwrap()
        .contains(&format!("attack = {TARGET}")));
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_recovery_defers_a_moving_leader_and_allows_a_real_self_heal() {
    let mut node = Standalone::start("playerbots-recovery-companion");
    node.publish_module();
    record_inputs(&node);
    node.assert_sql("DELETE FROM game_import_meta WHERE family = 'weather_seed' AND source_sha = '' AND file_hash = '' AND row_count = 2");
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("debug_set_nav_enabled", &["true"]);
    node.assert_call(
        "playerbots_spawn_class_role",
        &["3", "1200", "1200", "50", "5", "1"],
    );
    let guids: Vec<_> = node
        .query_rows("SELECT character_guid FROM pkg_playerbots_bot")
        .into_iter()
        .map(|row| row["character_guid"].clone())
        .collect();
    let (priest, leader, ally) = (&guids[0], &guids[1], &guids[2]);
    node.assert_call("playerbots_quest_loop_fixture_stage_named", &[priest]);
    node.assert_call("playerbots_fixture_prepare", &[]);
    node.assert_call(
        "playerbots_fixture_companion_stage",
        &[priest, leader, ally],
    );
    node.assert_call("playerbots_fixture_runner_select_cohort", &[priest]);
    node.assert_call("playerbots_fixture_provision_steps", &[priest, "32"]);
    node.assert_call("playerbots_recovery_fixture_block_companion", &[priest]);
    node.assert_call("playerbots_fixture_runner_pass_once", &[priest]);
    let initial = row(
        &node,
        &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {priest}"),
    );
    let initial_path =
        support::log_dir().join(format!("{}-moving-leader-initial.json", node.shard_name()));
    std::fs::write(
        initial_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "runner": initial.clone(),
            "priest": row(&node, &format!("SELECT guid, x, y, health FROM game_world_entity WHERE guid = {priest}")),
            "leader": row(&node, &format!("SELECT guid, x, y FROM game_world_entity WHERE guid = {leader}")),
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(initial["chosen"].contains("follow"), "{initial:?}");
    node.assert_call("playerbots_fixture_companion_due", &[priest]);
    let start = Instant::now();
    let mut samples = vec![];
    let mut moved_leader = false;
    while start.elapsed() < Duration::from_secs(34) {
        if !moved_leader && start.elapsed() >= Duration::from_secs(15) {
            node.assert_call("playerbots_fixture_position", &[leader, "1240"]);
            moved_leader = true;
        }
        samples.push(serde_json::json!({
            "elapsed_seconds": start.elapsed().as_secs_f64(),
            "runner": row(&node, &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {priest}")),
            "priest": row(&node, &format!("SELECT guid, x, y, health FROM game_world_entity WHERE guid = {priest}")),
            "leader": row(&node, &format!("SELECT guid, x, y FROM game_world_entity WHERE guid = {leader}")),
            "actions": node.query_rows(&format!("SELECT * FROM pkg_playerbots_action WHERE character_guid = {priest}")),
        }));
        std::thread::sleep(Duration::from_millis(750));
    }
    let path = support::log_dir().join(format!("{}-moving-leader.json", node.shard_name()));
    std::fs::write(path, serde_json::to_vec_pretty(&samples).unwrap()).unwrap();
    let changed = samples
        .iter()
        .find(|sample| {
            sample["runner"]["chosen"]
                .as_str()
                .unwrap()
                .contains("recoveryPosition")
        })
        .expect("Follow never changed approach");
    assert!(
        changed["elapsed_seconds"].as_f64().unwrap() <= 12.0,
        "{changed}"
    );
    let deferred = samples
        .iter()
        .find(|sample| {
            !sample["runner"]["deferred_destinations"]
                .as_str()
                .unwrap()
                .trim_matches(['[', ']', ' '])
                .is_empty()
        })
        .expect("moving leader erased the failed Follow attempt");
    assert!(
        deferred["elapsed_seconds"].as_f64().unwrap() <= 32.0,
        "{deferred}"
    );
    assert!(samples
        .iter()
        .all(|sample| sample["runner"]["objective_sequence"] == initial["objective_sequence"]));
    node.assert_call("playerbots_fixture_runner_pass_once", &[priest]);
    node.assert_call("playerbots_fixture_companion_health", &[priest, "25"]);
    node.assert_call("playerbots_fixture_runner_pass_once", &[priest]);
    let pending_casts = node.query_rows(&format!(
        "SELECT * FROM game_pending_cast WHERE caster_guid = {priest}"
    ));
    let start_health_row = row(
        &node,
        &format!("SELECT health FROM game_world_entity WHERE guid = {priest}"),
    );
    let staged_path = support::log_dir().join(format!(
        "{}-heal-during-deferral-staged.json",
        node.shard_name()
    ));
    std::fs::write(
        staged_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "pending_casts": pending_casts.clone(),
            "runner": row(&node, &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {priest}")),
            "priest": row(&node, &format!("SELECT guid, x, y, health FROM game_world_entity WHERE guid = {priest}")),
            "leader": row(&node, &format!("SELECT guid, x, y FROM game_world_entity WHERE guid = {leader}")),
            "actions": node.query_rows(&format!("SELECT * FROM pkg_playerbots_action WHERE character_guid = {priest}")),
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(pending_casts.len(), 1, "self-heal did not start");
    let cast = pending_casts[0].clone();
    let start_health: u32 = start_health_row["health"].parse().unwrap();
    let healed = poll_until(POLL_TIMEOUT, || {
        node.query_rows(&format!(
            "SELECT outcome FROM pkg_playerbots_action WHERE character_guid = {priest}"
        ))
        .iter()
        .any(|row| row["outcome"].contains("castResolved"))
    });
    let final_state = row(
        &node,
        &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {priest}"),
    );
    let health = row(
        &node,
        &format!("SELECT health FROM game_world_entity WHERE guid = {priest}"),
    );
    let path = support::log_dir().join(format!(
        "{}-heal-during-follow-deferral.json",
        node.shard_name()
    ));
    std::fs::write(path, serde_json::to_vec_pretty(&serde_json::json!({
        "cast": cast, "before_health": start_health, "after_health": health,
        "runner": final_state,
        "actions": node.query_rows(&format!("SELECT * FROM pkg_playerbots_action WHERE character_guid = {priest}")),
    })).unwrap()).unwrap();
    assert!(healed);
    assert_eq!(cast["target_guid"], *priest);
    assert!(health["health"].parse::<u32>().unwrap() > start_health);
    assert!(!final_state["deferred_destinations"]
        .trim_matches(['[', ']', ' '])
        .is_empty());
    assert!(final_state["companion_leader_guid"].contains(leader));
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_recovery_invalidates_failed_work_after_an_actual_navigation_import() {
    let mut node = Standalone::start("playerbots-recovery-geometry");
    node.publish_module();
    record_inputs(&node);
    let guid = prepare(&node);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    node.assert_call("playerbots_fixture_companion_due", &[&guid]);
    let deferred = poll_until(Duration::from_secs(40), || {
        !row(&node, &format!("SELECT character_guid, deferred_destinations FROM pkg_playerbots_runner WHERE character_guid = {guid}"))["deferred_destinations"].trim_matches(['[', ']', ' ']).is_empty()
    });
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let before = snapshot(&node, &guid, Duration::ZERO);
    let path = support::log_dir().join(format!("{}-before-import.json", node.shard_name()));
    std::fs::write(path, serde_json::to_vec_pretty(&before).unwrap()).unwrap();
    assert!(deferred, "{before}");
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
    assert!(before["runner"]["failures"]
        .as_str()
        .unwrap()
        .contains("missingImportedCoverage"));
    node.assert_call("import_nav_chunks_append", &["\"0,999,999,0,,\""]);
    let imported = row(
        &node,
        "SELECT revision FROM game_navigation_revision WHERE id = 0",
    );
    let refused = node.call("import_nav_chunks_append", &["\"0,998,999,0,,;invalid\""]);
    let unchanged = row(
        &node,
        "SELECT revision FROM game_navigation_revision WHERE id = 0",
    );
    let refused_rows =
        node.query_rows("SELECT key FROM game_nav_chunk WHERE cell_x = 998 AND cell_y = 999");
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let after = snapshot(&node, &guid, Duration::ZERO);
    let path = support::log_dir().join(format!(
        "{}-changed-navigation-inputs.json",
        node.shard_name()
    ));
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "before": before, "after": after, "imported": imported, "after_refusal": unchanged,
            "import_refused": !refused.status.success(), "rolled_back_rows": refused_rows,
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(imported["revision"], "1");
    assert!(!refused.status.success());
    assert_eq!(imported, unchanged);
    assert!(refused_rows.is_empty());
    assert!(after["runner"]["recovery"]
        .as_str()
        .unwrap()
        .contains("imported_revision = (some = 1)"));
    assert!(after["runner"]["chosen"]
        .as_str()
        .unwrap()
        .contains(&format!("attack = {TARGET}")));
    assert!(after["runner"]["recovery"]
        .as_str()
        .unwrap()
        .contains(&format!("active = (some = (fight = {TARGET}))")));
    assert!(!after["runner"]["recovery"]
        .as_str()
        .unwrap()
        .contains("deferred_until_micros = (some"));
    assert!(after["runner"]["deferred_destinations"]
        .as_str()
        .unwrap()
        .is_empty());
    assert_eq!(before["quest"], after["quest"]);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_recovery_retains_a_partial_route_that_first_moves_away_from_the_leader() {
    let mut node = Standalone::start("playerbots-recovery-partial");
    node.publish_module();
    record_inputs(&node);
    node.assert_sql("DELETE FROM game_import_meta WHERE family = 'weather_seed' AND source_sha = '' AND file_hash = '' AND row_count = 2");
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("debug_set_nav_enabled", &["true"]);
    node.assert_call(
        "playerbots_spawn_class_role",
        &["3", "1200", "1200", "50", "5", "1"],
    );
    let guids: Vec<_> = node
        .query_rows("SELECT character_guid FROM pkg_playerbots_bot")
        .into_iter()
        .map(|r| r["character_guid"].clone())
        .collect();
    let (priest, leader, ally) = (&guids[0], &guids[1], &guids[2]);
    node.assert_call("playerbots_quest_loop_fixture_stage_named", &[priest]);
    node.assert_call("playerbots_fixture_prepare", &[]);
    node.assert_call(
        "playerbots_fixture_companion_stage",
        &[priest, leader, ally],
    );
    node.assert_call("playerbots_fixture_runner_select_cohort", &[priest]);
    node.assert_call("playerbots_fixture_provision_steps", &[priest, "32"]);
    node.assert_call("playerbots_recovery_fixture_partial_route", &[priest]);
    node.assert_call("playerbots_fixture_runner_pass_once", &[priest]);
    let initial = row(
        &node,
        &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {priest}"),
    );
    let initial_actions = node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_action WHERE character_guid = {priest}"
    ));
    let start = row(
        &node,
        &format!("SELECT x, y FROM game_world_entity WHERE guid = {priest}"),
    );
    let advanced = poll_until(POLL_TIMEOUT, || {
        row(
            &node,
            &format!("SELECT x FROM game_world_entity WHERE guid = {priest}"),
        )["x"]
            .parse::<f32>()
            .unwrap()
            < start["x"].parse::<f32>().unwrap() - 0.5
    });
    node.assert_call("playerbots_fixture_runner_pass_once", &[priest]);
    let final_state = row(
        &node,
        &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {priest}"),
    );
    let position = row(
        &node,
        &format!("SELECT x, y FROM game_world_entity WHERE guid = {priest}"),
    );
    let path = support::log_dir().join(format!("{}-partial-route-away.json", node.shard_name()));
    std::fs::write(path, serde_json::to_vec_pretty(&serde_json::json!({
        "initial": initial, "initial_actions": initial_actions, "start": start, "final": final_state, "position": position,
        "actions": node.query_rows(&format!("SELECT * FROM pkg_playerbots_action WHERE character_guid = {priest}")),
    })).unwrap()).unwrap();
    assert!(advanced, "{initial:?} {position:?}");
    assert!(
        initial_actions.iter().any(
            |action| action["outcome"].contains("status = (partial = ())")
                && action["outcome"].contains("expansions = 4096")
        ),
        "{initial_actions:?}"
    );
    assert_eq!(initial["route_expansions"], "4096");
    assert!(
        final_state["recovery"].contains("stalled_micros = 0"),
        "{final_state:?}"
    );
    assert_eq!(
        initial["objective_sequence"],
        final_state["objective_sequence"]
    );
    assert!(final_state["deferred_destinations"]
        .trim_matches(['[', ']', ' '])
        .is_empty());
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_recovery_defers_a_partial_endpoint_revisited_after_an_approach() {
    let mut node = Standalone::start("playerbots-recovery-revisited-endpoint");
    node.publish_module();
    record_inputs(&node);
    let guid = prepare(&node);
    node.assert_call("playerbots_fixture_position", &[&guid, "1340"]);
    node.assert_call("playerbots_recovery_fixture_partial_route", &[&guid]);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let first = snapshot(&node, &guid, Duration::ZERO);
    let first_move = first["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["kind"].as_str() == Some("(move = ())"))
        .expect("the Quest approach did not record a movement leg");
    let first_outcome = first_move["outcome"].as_str().unwrap();
    assert!(first_outcome.contains("status = (partial = ())"), "{first}");
    assert!(first_outcome.contains("expansions = 4096"), "{first}");
    let destination = tuple_field(
        first["runner"]["recovery"].as_str().unwrap(),
        "destination = ",
    )
    .expect("retained recovery destination missing")
    .to_string();
    let objective = first["runner"]["objective_sequence"].clone();
    let quest = first["quest"].clone();

    node.assert_call("playerbots_fixture_companion_due", &[&guid]);
    let started = Instant::now();
    let mut samples = vec![first];
    while started.elapsed() < Duration::from_secs(70) {
        std::thread::sleep(Duration::from_millis(750));
        samples.push(snapshot(&node, &guid, started.elapsed()));
    }
    let path = support::log_dir().join(format!(
        "{}-revisited-partial-endpoint.json",
        node.shard_name()
    ));
    std::fs::write(path, serde_json::to_vec_pretty(&samples).unwrap()).unwrap();

    let approach_index = samples
        .iter()
        .position(|sample| {
            sample["runner"]["chosen"]
                .as_str()
                .unwrap()
                .contains("recoveryPosition")
        })
        .expect("the stalled Quest route did not select a Recovery Position");
    let stalled_endpoint = route_endpoint(
        samples[approach_index]["runner"]["recovery"]
            .as_str()
            .unwrap(),
    )
    .expect("the Recovery Position did not retain the stalled Quest endpoint");
    let character_position = |sample: &serde_json::Value| {
        (
            sample["character"]["x"]
                .as_str()
                .unwrap()
                .parse::<f32>()
                .unwrap(),
            sample["character"]["y"]
                .as_str()
                .unwrap()
                .parse::<f32>()
                .unwrap(),
        )
    };
    let departed_index = samples
        .iter()
        .enumerate()
        .skip(approach_index)
        .find(|(_, sample)| {
            let position = character_position(sample);
            (position.0 - stalled_endpoint.0).hypot(position.1 - stalled_endpoint.1) > 1.0
        })
        .map(|(index, _)| index)
        .expect("the Recovery Position did not move away from the stalled Quest endpoint");
    let revisited = samples.iter().skip(departed_index + 1).find(|sample| {
        let position = character_position(sample);
        let recovery = sample["runner"]["recovery"].as_str().unwrap();
        let returned_from = retained_route_from(recovery);
        (position.0 - stalled_endpoint.0).abs() < 0.05
            && (position.1 - stalled_endpoint.1).abs() < 0.05
            && recovery.contains(&format!("work = (fight = {TARGET})"))
            && recovery.contains("status = (partial = ())")
            && route_endpoint(recovery).is_some_and(|endpoint| {
                (endpoint.0 - stalled_endpoint.0).abs() < 0.05
                    && (endpoint.1 - stalled_endpoint.1).abs() < 0.05
            })
            && returned_from.is_some_and(|from| {
                (from.0 - stalled_endpoint.0).hypot(from.1 - stalled_endpoint.1) > 1.0
            })
    });
    assert!(
        revisited.is_some(),
        "the Quest route did not revisit {stalled_endpoint:?}"
    );
    let (deferred_index, deferred) = samples
        .iter()
        .enumerate()
        .find(|(_, sample)| {
            !sample["runner"]["deferred_destinations"]
                .as_str()
                .unwrap()
                .trim_matches(['[', ']', ' '])
                .is_empty()
        })
        .expect("the revisited Quest destination was not deferred");
    assert!(deferred["runner"]["failures"]
        .as_str()
        .unwrap()
        .contains("noMovement"));
    assert!(deferred["runner"]["failures"]
        .as_str()
        .unwrap()
        .contains("missingImportedCoverage"));
    assert!(deferred["runner"]["deferred_destinations"]
        .as_str()
        .unwrap()
        .contains(&destination));
    assert_ne!(deferred["runner"]["objective_sequence"], objective);
    assert_eq!(deferred["quest"], quest);
    let alternative = samples[deferred_index..]
        .iter()
        .find(|sample| {
            let runner = &sample["runner"];
            let chosen = runner["chosen"].as_str().unwrap();
            let current_objective = runner["objective_sequence"].as_str().unwrap();
            !runner["deferred_destinations"]
                .as_str()
                .unwrap()
                .trim_matches(['[', ']', ' '])
                .is_empty()
                && runner["deferred_destinations"]
                    .as_str()
                    .unwrap()
                    .contains(&destination)
                && chosen.contains(&format!("entity = {ALTERNATIVE_TARGET}"))
                && chosen.contains("reason = (quest = ())")
                && chosen.contains(&format!("objective = {current_objective}"))
        })
        .expect("the active destination deferral prevented other useful Quest work");
    assert_eq!(
        alternative["runner"]["objective_sequence"],
        deferred["runner"]["objective_sequence"]
    );
    assert_ne!(alternative["runner"]["objective_sequence"], objective);
    assert_eq!(alternative["quest"], quest);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_recovery_holds_a_quest_when_its_remaining_targets_are_controlled() {
    let mut node = Standalone::start("playerbots-recovery-controlled");
    node.publish_module();
    record_inputs(&node);
    let guid = prepare(&node);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let before = snapshot(&node, &guid, Duration::ZERO);
    for offset in 0..10u64 {
        node.assert_call(
            "playerbots_fixture_roles_control",
            &[&guid, &(TARGET + offset).to_string(), "50020"],
        );
    }
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let after = snapshot(&node, &guid, Duration::ZERO);
    let attacks = node.query_rows(&format!(
        "SELECT * FROM game_melee_attack WHERE attacker_guid = {guid}"
    ));
    let auras = node.query_rows(&format!(
        "SELECT * FROM game_aura WHERE target_guid = {TARGET}"
    ));
    let path = support::log_dir().join(format!("{}-controlled-quest-wait.json", node.shard_name()));
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "before": before, "after": after, "attacks": attacks, "auras": auras,
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(after["runner"]["chosen"]
        .as_str()
        .unwrap()
        .contains("crowdControl"));
    assert!(after["runner"]["failures"]
        .as_str()
        .unwrap()
        .contains("questControlled"));
    assert!(attacks.is_empty());
    assert!(!auras.is_empty());
    assert_eq!(before["quest"], after["quest"]);
    assert_eq!(before["target"]["health"], after["target"]["health"]);
    assert_eq!(
        before["runner"]["objective_sequence"],
        after["runner"]["objective_sequence"]
    );
}

fn incomplete_quest_target_read(label: &str, stage: impl Fn(&Standalone, &str)) {
    let mut node = Standalone::start(label);
    node.publish_module();
    record_inputs(&node);
    let guid = prepare(&node);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let armed = node.query_rows(&format!(
        "SELECT * FROM game_melee_attack WHERE attacker_guid = {guid}"
    ));
    let before = snapshot(&node, &guid, Duration::ZERO);
    let path = support::log_dir().join(format!("{}-armed-before-read.json", node.shard_name()));
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "before": before, "armed": armed,
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(armed.len(), 1, "quest attack was not armed");
    stage(&node, &guid);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let after = snapshot(&node, &guid, Duration::ZERO);
    let attacks = node.query_rows(&format!(
        "SELECT * FROM game_melee_attack WHERE attacker_guid = {guid}"
    ));
    let chosen = after["runner"]["chosen"].as_str().unwrap();
    assert!(chosen.contains("hold = ()"), "{after}");
    assert!(chosen.contains("reason = (returnHome = ())"), "{after}");
    assert!(attacks.is_empty());
    assert!(after["runner"]["foreground"]
        .as_str()
        .unwrap()
        .contains("none"));
    assert_eq!(before["quest"], after["quest"]);
    assert_eq!(before["target"]["health"], after["target"]["health"]);
    assert_eq!(
        before["runner"]["objective_sequence"],
        after["runner"]["objective_sequence"]
    );

    let observed = after["runner"]["observed_micros"]
        .as_str()
        .unwrap()
        .parse::<i64>()
        .unwrap();
    let next_eligible = after["runner"]["next_eligible_micros"]
        .as_str()
        .unwrap()
        .parse::<i64>()
        .unwrap();
    let wait_micros = next_eligible.saturating_sub(observed);
    assert!(wait_micros > 0, "{after}");
    let wait = Duration::from_micros(wait_micros as u64);
    assert!(wait < POLL_TIMEOUT, "{after}");
    std::thread::sleep(wait);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let resumed = snapshot(&node, &guid, wait);
    let resumed_attacks = node.query_rows(&format!(
        "SELECT * FROM game_melee_attack WHERE attacker_guid = {guid}"
    ));
    let path =
        support::log_dir().join(format!("{}-incomplete-target-read.json", node.shard_name()));
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "before": before,
            "after": after,
            "resumed": resumed,
            "armed": armed,
            "attacks": attacks,
            "resumed_attacks": resumed_attacks,
        }))
        .unwrap(),
    )
    .unwrap();
    let resumed_chosen = resumed["runner"]["chosen"].as_str().unwrap();
    assert!(resumed_chosen.contains("move = (home = ())"), "{resumed}");
    assert!(
        resumed_chosen.contains("reason = (returnHome = ())"),
        "{resumed}"
    );
    assert!(
        resumed["runner"]["failures"]
            .as_str()
            .unwrap()
            .contains("questReadLimit"),
        "{resumed}"
    );
    assert!(resumed_attacks.is_empty());
    assert_eq!(before["quest"], resumed["quest"]);
    assert_eq!(before["target"]["health"], resumed["target"]["health"]);
    assert_eq!(
        before["runner"]["objective_sequence"],
        resumed["runner"]["objective_sequence"]
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_recovery_keeps_an_armed_known_quest_target_when_other_reads_are_incomplete() {
    let mut node = Standalone::start("playerbots-recovery-read-limit");
    node.publish_module();
    record_inputs(&node);
    let guid = prepare(&node);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let armed = node.query_rows(&format!(
        "SELECT * FROM game_melee_attack WHERE attacker_guid = {guid}"
    ));
    let before = snapshot(&node, &guid, Duration::ZERO);
    assert_eq!(armed.len(), 1, "quest attack was not armed");

    node.assert_call("playerbots_quest_loop_fixture_stage_search_limit", &[&guid]);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let after = snapshot(&node, &guid, Duration::ZERO);
    let attacks = node.query_rows(&format!(
        "SELECT * FROM game_melee_attack WHERE attacker_guid = {guid}"
    ));
    std::fs::write(
        support::log_dir().join(format!("{}-known-target-read.json", node.shard_name())),
        serde_json::to_vec_pretty(&serde_json::json!({
            "before": before,
            "after": after,
            "armed": armed,
            "attacks": attacks,
        }))
        .unwrap(),
    )
    .unwrap();

    let chosen = after["runner"]["chosen"].as_str().unwrap();
    assert!(chosen.contains(&format!("attack = {TARGET}")), "{after}");
    assert!(chosen.contains("reason = (quest = ())"), "{after}");
    assert_eq!(attacks, armed);
    assert_eq!(before["quest"], after["quest"]);
    assert_eq!(before["target"]["health"], after["target"]["health"]);
    assert_eq!(before["runner"]["failures"], after["runner"]["failures"]);
    assert_eq!(
        before["runner"]["objective_sequence"],
        after["runner"]["objective_sequence"]
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_recovery_stops_an_armed_quest_attack_when_target_control_is_unknown() {
    incomplete_quest_target_read("playerbots-recovery-control-read-limit", |node, guid| {
        node.assert_call("playerbots_fixture_roles_overflow", &[guid, "1"]);
        node.assert_sql(&format!(
            "UPDATE game_aura SET target_guid = {TARGET} WHERE spell_id >= 5098600 AND spell_id < 5098665"
        ));
    });
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_recovery_records_a_controlled_quest_while_survival_takes_priority() {
    let mut node = Standalone::start("playerbots-recovery-controlled-survival");
    node.publish_module();
    record_inputs(&node);
    let guid = prepare(&node);
    stage_quest_geometry(&node);
    node.assert_call("playerbots_fixture_position", &[&guid, "1340"]);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let approach = snapshot(&node, &guid, Duration::ZERO);
    let safe = row(
        &node,
        &format!("SELECT safe_position FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"),
    );
    let moved = poll_until(Duration::from_secs(5), || {
        row(
            &node,
            &format!("SELECT x FROM game_world_entity WHERE guid = {guid}"),
        )["x"]
            .parse::<f32>()
            .unwrap()
            > 1343.0
    });
    for offset in 0..10u64 {
        node.assert_call(
            "playerbots_fixture_roles_control",
            &[&guid, &(TARGET + offset).to_string(), "50020"],
        );
    }
    let before = snapshot(&node, &guid, Duration::ZERO);
    node.assert_call("playerbots_fixture_runner_survival_hit", &[&guid, "0", "1"]);
    let after = snapshot(&node, &guid, Duration::ZERO);
    let attacks = node.query_rows(&format!(
        "SELECT * FROM game_melee_attack WHERE attacker_guid = {guid}"
    ));
    let path = support::log_dir().join(format!("{}-controlled-survival.json", node.shard_name()));
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "approach": approach, "safe": safe, "moved": moved,
            "before": before, "after": after, "attacks": attacks,
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(moved, "{before}");
    assert!(safe["safe_position"].contains("x = 1340"), "{safe:?}");
    assert!(
        approach["runner"]["foreground"]
            .as_str()
            .unwrap()
            .contains("movement"),
        "{approach}"
    );
    assert!(
        after["character"]["health"]
            .as_str()
            .unwrap()
            .parse::<u32>()
            .unwrap()
            < before["character"]["health"]
                .as_str()
                .unwrap()
                .parse::<u32>()
                .unwrap()
    );
    assert!(
        after["runner"]["chosen"]
            .as_str()
            .unwrap()
            .contains("survival"),
        "{after}"
    );
    assert!(
        after["runner"]["failures"]
            .as_str()
            .unwrap()
            .contains("questControlled"),
        "{after}"
    );
    assert!(
        !after["runner"]["last_outcome"]
            .as_str()
            .unwrap()
            .contains("refused"),
        "{after}"
    );
    assert!(attacks.is_empty());
    assert_eq!(before["quest"], after["quest"]);
    assert_eq!(
        before["runner"]["objective_sequence"],
        after["runner"]["objective_sequence"]
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_recovery_replaces_a_recovery_leg_when_the_quest_fight_changes() {
    let mut node = Standalone::start("playerbots-recovery-replaced-fight");
    node.publish_module();
    record_inputs(&node);
    let guid = prepare(&node);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    node.assert_call("playerbots_fixture_companion_due", &[&guid]);
    let changed = poll_until(Duration::from_secs(15), || {
        row(
            &node,
            &format!("SELECT foreground FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
        )["foreground"]
            .contains("recoveryPosition")
    });
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let before = snapshot(&node, &guid, Duration::ZERO);
    let path = support::log_dir().join(format!("{}-before-fight-change.json", node.shard_name()));
    std::fs::write(path, serde_json::to_vec_pretty(&before).unwrap()).unwrap();
    assert!(changed, "{before}");
    node.assert_call(
        "playerbots_fixture_roles_control",
        &[&guid, &TARGET.to_string(), "50020"],
    );
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let after = snapshot(&node, &guid, Duration::ZERO);
    let attacks = node.query_rows(&format!(
        "SELECT * FROM game_melee_attack WHERE attacker_guid = {guid}"
    ));
    let path = support::log_dir().join(format!("{}-changed-fight.json", node.shard_name()));
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "before": before, "after": after, "attacks": attacks,
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(
        !after["runner"]["foreground"]
            .as_str()
            .unwrap()
            .contains("recoveryPosition"),
        "{after}"
    );
    assert!(
        after["runner"]["chosen"]
            .as_str()
            .unwrap()
            .contains(&format!("attack = {}", TARGET + 1)),
        "{after}"
    );
    assert_eq!(attacks.len(), 1);
    assert_eq!(attacks[0]["target_guid"], (TARGET + 1).to_string());
    assert_eq!(before["quest"], after["quest"]);
    assert_eq!(
        before["runner"]["objective_sequence"],
        after["runner"]["objective_sequence"]
    );
}

fn stage_quest_geometry(node: &Standalone) {
    let x0 = lyracore_shared::terrain::cell_index(1_150.0).unwrap();
    let x1 = lyracore_shared::terrain::cell_index(1_400.0).unwrap();
    let y0 = lyracore_shared::terrain::cell_index(1_150.0).unwrap();
    let y1 = lyracore_shared::terrain::cell_index(1_250.0).unwrap();
    let mut rows = Vec::new();
    for cell_x in x0.min(x1)..=x0.max(x1) {
        for cell_y in y0.min(y1)..=y0.max(y1) {
            rows.push(format!("0,{cell_x},{cell_y},50,,"));
        }
    }
    node.assert_call("import_nav_chunks", &[&rows.join(";")]);
    node.assert_call("debug_set_nav_enabled", &["true"]);
}

#[test]
#[ignore = "requires merged PB-007 Wasm, SpacetimeDB, and the playerbots Package"]
fn playerbots_recovery_upgrades_a_retained_quest_and_owned_cast_without_resetting_them() {
    let preceding = preceding_recovery();
    assert_ne!(
        blake3::hash(&preceding.wasm),
        blake3::hash(support::module_bytes())
    );
    let mut node = Standalone::start("playerbots-recovery-pb007-migration");
    node.publish_module_bytes(&preceding.wasm);
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
    stage_quest_geometry(&node);
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
        let runner = row(
            &node,
            &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
        );
        let ready = runner["objective"].contains("quest") && runner["foreground"].contains("cast");
        if !ready {
            std::thread::sleep(Duration::from_millis(1_100));
        }
        ready
    });
    node.assert_call("playerbots_fixture_freeze", &[&guid]);
    fn capture(node: &Standalone, guid: &str) -> serde_json::Value {
        serde_json::json!({
            "runner": row(node, &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}")),
            "retained_quest": node.query_rows(&format!("SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}")),
            "quest": node.query_rows(&format!("SELECT * FROM game_character_quest WHERE character_guid = {guid}")),
            "pending_cast": node.query_rows(&format!("SELECT * FROM game_pending_cast WHERE caster_guid = {guid}")),
            "provisioning": node.query_rows(&format!("SELECT * FROM pkg_playerbots_provisioning WHERE character_guid = {guid}")),
            "synthetic_recovery_spell": node.query_rows("SELECT spell_id, cast_time_ms FROM game_spell WHERE spell_id = 5090100"),
        })
    }
    let before = capture(&node, &guid);
    let before_path =
        support::log_dir().join(format!("{}-populated-predecessor.json", node.shard_name()));
    std::fs::write(
        before_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "evidence_phase": "populated-pre-upgrade",
            "tested_core": preceding.manifest["core"],
            "tested_collection": preceding.manifest["collection"],
            "core_dirty": false, "collection_dirty": false,
            "module_wasm_identity": blake3::hash(&preceding.wasm).to_hex().to_string(),
            "package_content_identity": preceding.manifest["package_content_identity"],
            "preceding_build": preceding.manifest,
            "before": before,
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(pending, "{before}");
    assert_eq!(before["pending_cast"].as_array().unwrap().len(), 1);
    let retained = before["retained_quest"].as_array().unwrap();
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0]["quest_entry"], "7");
    assert_eq!(before["provisioning"].as_array().unwrap().len(), 1);
    let quest = before["quest"]
        .as_array()
        .unwrap()
        .iter()
        .find(|quest| quest["quest_entry"] == "7")
        .expect("retained quest is in the Character quest log");
    assert_eq!(quest["rewarded"], "false");
    assert_eq!(
        quest["counts"]
            .as_str()
            .unwrap()
            .split(',')
            .next()
            .unwrap()
            .trim(),
        "0"
    );
    assert!(before["runner"]["foreground"]
        .as_str()
        .unwrap()
        .contains("spell = 5090100"));
    node.publish_module();
    record_inputs(&node);
    let after = capture(&node, &guid);
    let navigation = node.query_rows("SELECT * FROM game_navigation_revision");
    let path = support::log_dir().join(format!("{}-populated-upgrade.json", node.shard_name()));
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "before": before, "after": after, "navigation": navigation,
        }))
        .unwrap(),
    )
    .unwrap();
    for (field, value) in before["runner"].as_object().unwrap() {
        assert_eq!(&after["runner"][field], value, "runner field {field}");
    }
    for field in [
        "retained_quest",
        "quest",
        "pending_cast",
        "provisioning",
        "synthetic_recovery_spell",
    ] {
        assert_eq!(before[field], after[field], "{field}");
    }
    assert_eq!(after["runner"]["recovery"], "(none = ())");
    assert!(navigation.is_empty());
}
