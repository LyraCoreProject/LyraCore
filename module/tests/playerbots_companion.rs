//! Durable Priest companion behavior against private, seeded Module databases.

mod support;
use std::collections::BTreeMap;
use support::{poll_until, Standalone, POLL_TIMEOUT};

const HEAL: &str = "5090100";
const CHANNEL_HEAL: &str = "5090104";
const PB002_CORE: &str = "e6a755db0a150bbf73ad97b972fe829f20f6816c";
const PB002_CORE_TREE: &str = "0769d6b7cd96d7e23a7ad16528399544ece0e2ec";
const PB002_COLLECTION: &str = "155c9e401afb06d5731acedf8fc35a81dbe4aaa6";
const PB002_COLLECTION_TREE: &str = "e2558a9cf421a74f79497ef361dbbfceb911a0d1";
const PB002_PACKAGE_IDENTITY: &str =
    "33fcb8a217aad84f46f9ad65efdad23be24bad8e29828777de7c7e6469417161";

fn git(path: &std::path::Path, args: &[&str]) -> String {
    let result = std::process::Command::new("git")
        .current_dir(path)
        .args(args)
        .output()
        .unwrap();
    assert!(result.status.success());
    String::from_utf8(result.stdout).unwrap().trim().to_string()
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
            let contents = std::fs::read(&child).unwrap();
            digest.update(&(contents.len() as u64).to_le_bytes());
            digest.update(&contents);
        }
    }
}

struct PrecedingPb002 {
    wasm: Vec<u8>,
    manifest: serde_json::Value,
}

fn preceding_pb002() -> PrecedingPb002 {
    let wasm_path = std::env::var_os("PLAYERBOTS_COMPANION_PRECEDING_WASM")
        .expect("PLAYERBOTS_COMPANION_PRECEDING_WASM must name the merged PB-002 Wasm");
    let manifest_path = std::env::var_os("PLAYERBOTS_COMPANION_PRECEDING_MANIFEST")
        .expect("PLAYERBOTS_COMPANION_PRECEDING_MANIFEST must describe that Wasm build");
    let core_path = std::env::var_os("PLAYERBOTS_COMPANION_PRECEDING_CORE")
        .expect("PLAYERBOTS_COMPANION_PRECEDING_CORE must name the clean PB-002 checkout");
    let collection_path = std::env::var_os("PLAYERBOTS_COMPANION_PRECEDING_COLLECTION")
        .expect("PLAYERBOTS_COMPANION_PRECEDING_COLLECTION must name the clean PB-002 checkout");
    let core_path = std::path::Path::new(&core_path);
    let collection_path = std::path::Path::new(&collection_path);
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(manifest_path).unwrap()).unwrap();
    let wasm = std::fs::read(&wasm_path).unwrap();

    assert_eq!(manifest["core"], PB002_CORE);
    assert_eq!(manifest["collection"], PB002_COLLECTION);
    assert_eq!(manifest["core_tree"], PB002_CORE_TREE);
    assert_eq!(manifest["collection_tree"], PB002_COLLECTION_TREE);
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
    assert_eq!(manifest["package_content_identity"], PB002_PACKAGE_IDENTITY);
    assert_eq!(manifest["wasm_bytes"].as_u64(), Some(wasm.len() as u64));

    assert_eq!(git(core_path, &["rev-parse", "HEAD"]), PB002_CORE);
    assert_eq!(
        git(core_path, &["rev-parse", "HEAD^{tree}"]),
        PB002_CORE_TREE
    );
    assert!(git(core_path, &["status", "--porcelain"]).is_empty());
    assert_eq!(
        git(collection_path, &["rev-parse", "HEAD"]),
        PB002_COLLECTION
    );
    assert_eq!(
        git(collection_path, &["rev-parse", "HEAD^{tree}"]),
        PB002_COLLECTION_TREE
    );
    if let Some(playerbots_tree) = manifest["playerbots_tree"].as_str() {
        assert_eq!(
            git(collection_path, &["rev-parse", "HEAD:playerbots"]),
            playerbots_tree
        );
    }
    assert!(git(collection_path, &["status", "--porcelain"]).is_empty());
    let mut package_digest = blake3::Hasher::new();
    digest_files(&collection_path.join("playerbots"), &mut package_digest);
    assert_eq!(
        package_digest.finalize().to_hex().as_str(),
        PB002_PACKAGE_IDENTITY
    );

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

    PrecedingPb002 { wasm, manifest }
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

fn select(node: &Standalone, bot: &str, mode: &str) {
    node.assert_call(
        "playerbots_select_controller",
        &[bot, &format!("{{\"{mode}\":[]}}")],
    );
}

fn runner(node: &Standalone, bot: &str) -> BTreeMap<String, String> {
    node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_runner WHERE character_guid = {bot}"
    ))
    .into_iter()
    .next()
    .expect("runner explanation missing")
}

fn position(node: &Standalone, guid: &str) -> (f32, f32) {
    let row = &node.query_rows(&format!(
        "SELECT x, y FROM game_world_entity WHERE guid = {guid}"
    ))[0];
    (row["x"].parse().unwrap(), row["y"].parse().unwrap())
}

fn health(node: &Standalone, guid: &str) -> u32 {
    node.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid = {guid}"
    ))[0]["health"]
        .parse()
        .unwrap()
}

fn spline(node: &Standalone, guid: &str) -> Option<BTreeMap<String, String>> {
    node.query_rows(&format!(
        "SELECT start_micros, dur_ms, sx, sy, dx, dy, spline_id FROM game_creature_spline WHERE guid = {guid}"
    ))
    .into_iter()
    .next()
}

fn spline_finished(node: &Standalone, guid: &str, leg: &BTreeMap<String, String>) -> bool {
    let (x, y) = position(node, guid);
    let at_destination = (x - leg["dx"].parse::<f32>().unwrap()).abs() < 0.01
        && (y - leg["dy"].parse::<f32>().unwrap()).abs() < 0.01;
    at_destination
        && spline(node, guid).is_none_or(|current| current["spline_id"] != leg["spline_id"])
}

fn fixture(name: &str) -> (Standalone, Vec<String>) {
    fixture_role(name, "1")
}

fn fixture_role(name: &str, role: &str) -> (Standalone, Vec<String>) {
    let mut node = Standalone::start(name);
    node.publish_module();
    record_inputs(&node);
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("playerbots_spawn_role", &["3", "1200", "1200", "50", role]);
    node.assert_call("playerbots_fixture_prepare", &[]);
    let bots: Vec<_> = node
        .query_rows("SELECT character_guid FROM pkg_playerbots_bot")
        .into_iter()
        .map(|row| row["character_guid"].clone())
        .collect();
    node.assert_call(
        "playerbots_fixture_companion_stage",
        &[&bots[0], &bots[1], &bots[2]],
    );
    (node, bots)
}

fn due(node: &Standalone, guid: &str) {
    node.assert_call("playerbots_fixture_companion_due", &[guid]);
    node.assert_call("playerbots_fixture_runner_pass", &[]);
}

fn pass_once(node: &Standalone, guid: &str) {
    node.assert_call("playerbots_fixture_runner_pass_once", &[guid]);
}

/// The largest supported Provisioning Profile has at most 30 actions. Two extra passes cover the
/// cycle boundary and the Follow selection. A later Follow would exceed the profile's bound.
fn resume_follow_after_provisioning(
    node: &Standalone,
    guid: &str,
    objective_sequence: &str,
    case: &str,
) -> BTreeMap<String, String> {
    const PASS_LIMIT: usize = 32;

    let starting_position = position(node, guid);
    let role = node.query_rows(&format!(
        "SELECT role FROM pkg_playerbots_bot WHERE character_guid = {guid}"
    ))[0]["role"]
        .clone();
    node.assert_call("playerbots_fixture_provision_due", &[guid]);
    let mut saw_provisioning = false;
    for pass in 1..=PASS_LIMIT {
        pass_once(node, guid);
        let state = runner(node, guid);
        evidence(node, &format!("{case}-pass-{pass}"));
        assert_eq!(state["objective_sequence"], objective_sequence);
        assert!(state["objective"].contains("companion"), "{state:?}");
        assert_eq!(
            node.query_rows(&format!(
                "SELECT role FROM pkg_playerbots_bot WHERE character_guid = {guid}"
            ))[0]["role"],
            role
        );
        if state["chosen"].contains("follow") {
            assert!(
                saw_provisioning,
                "forced Provisioning never consumed a pass"
            );
            assert!(state["foreground"].contains("movement"), "{state:?}");
            assert!(poll_until(POLL_TIMEOUT, || {
                let current = position(node, guid);
                (current.0 - starting_position.0).abs() > 0.01
                    || (current.1 - starting_position.1).abs() > 0.01
            }));
            return state;
        }
        assert!(state["chosen"].contains("provisioning"), "{state:?}");
        assert!(state["last_outcome"].contains("provisioning"), "{state:?}");
        assert!(state["foreground"].contains("none"), "{state:?}");
        saw_provisioning = true;
    }
    panic!("Follow did not resume within {PASS_LIMIT} Provisioning Profile passes")
}

fn evidence(node: &Standalone, case: &str) {
    let path = support::log_dir().join(format!("{}-{case}.json", node.shard_name()));
    let record = serde_json::json!({
        "case": case,
        "spacetimedb": "2.7.1",
        "runner": node.query_rows("SELECT * FROM pkg_playerbots_runner"),
        "provisioning": node.query_rows("SELECT * FROM pkg_playerbots_provisioning"),
        "actions": node.query_rows("SELECT * FROM pkg_playerbots_action"),
        "entities": node.query_rows("SELECT guid, map_id, instance_id, x, y, z, health, max_health, dead FROM game_world_entity"),
        "party": node.query_rows("SELECT * FROM game_group_member"),
        "splines": node.query_rows("SELECT * FROM game_creature_spline"),
        "pending_casts": node.query_rows("SELECT * FROM game_pending_cast"),
        "cast_events": node.query_rows("SELECT * FROM game_spell_cast_event"),
        "melee": node.query_rows("SELECT * FROM game_melee_attack"),
    });
    std::fs::write(path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_priest_follows_a_moving_human_leader_without_pulling() {
    let (node, bots) = fixture("playerbots-companion-follow");
    let (priest, leader) = (&bots[0], &bots[1]);
    select(&node, priest, "cohort");
    assert!(poll_until(POLL_TIMEOUT, || runner(&node, priest)["chosen"]
        .contains("follow")));
    let before = runner(&node, priest);
    let objective = before["objective_sequence"].clone();
    assert!(before["objective"].contains("companion"));
    assert!(before["companion_leader_guid"].contains(leader));
    assert!(before["foreground"].contains("movement"));
    node.assert_call(
        "playerbots_fixture_companion_move",
        &[leader, "1240", "1200"],
    );
    due(&node, priest);
    let refreshed = runner(&node, priest);
    assert_eq!(refreshed["objective_sequence"], objective);
    assert!(refreshed["objective"].contains("x = 1240"));
    assert!(poll_until(POLL_TIMEOUT, || position(&node, priest).0 > 1201.0));
    assert_eq!(
        node.query_rows("SELECT guid FROM game_world_entity WHERE entry = 5090302")
            .len(),
        1,
        "the unrelated hostile fixture must be present"
    );
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM game_melee_attack WHERE attacker_guid = {priest}"
        ))
        .is_empty());
    evidence(&node, "follow");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_missing_group_parent_holds_the_companion_objective() {
    let (node, bots) = fixture("playerbots-companion-party-unavailable");
    let priest = &bots[0];
    select(&node, priest, "cohort");
    due(&node, priest);
    let before = runner(&node, priest);
    assert!(before["objective"].contains("companion"));
    node.assert_call("playerbots_fixture_companion_remove_group", &[]);
    due(&node, priest);
    let held = runner(&node, priest);
    evidence(&node, "party-unavailable-hold");
    assert_eq!(held["objective_sequence"], before["objective_sequence"]);
    assert_eq!(held["objective"], before["objective"]);
    assert!(held["chosen"].contains("partyUnavailable"), "{held:?}");
    assert!(held["last_outcome"].contains("partyReadUnavailable"));
    assert!(held["last_outcome"].contains("missingGroup"));
    assert!(held["foreground"].contains("none"));
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_priest_retains_one_ally_cast_while_the_leader_moves_then_resumes_follow() {
    let (node, bots) = fixture("playerbots-companion-heal");
    let (priest, leader, ally) = (&bots[0], &bots[1], &bots[2]);
    select(&node, priest, "cohort");
    node.assert_call("playerbots_fixture_companion_health", &[ally, "25"]);
    let ally_before = node.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid = {ally}"
    ))[0]["health"]
        .parse::<u32>()
        .unwrap();
    due(&node, priest);
    assert!(poll_until(POLL_TIMEOUT, || !node
        .query_rows(&format!(
            "SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = {priest}"
        ))
        .is_empty()));
    let pending = node.query_rows(&format!(
        "SELECT scheduled_id, target_guid FROM game_pending_cast WHERE caster_guid = {priest}"
    ))[0]
        .clone();
    assert_eq!(pending["target_guid"], *ally);
    let starts = node.query_rows(&format!(
        "SELECT * FROM game_spell_cast_event WHERE caster_guid = {priest} AND spell_id = {HEAL}"
    ));
    assert_eq!(
        starts.iter().filter(|event| event["kind"] == "1").count(),
        1,
        "{starts:?}"
    );
    evidence(&node, "heal-started");
    let state = runner(&node, priest);
    let objective = state["objective_sequence"].clone();
    assert!(state["chosen"].contains("heal"));
    node.assert_call(
        "playerbots_fixture_companion_move",
        &[leader, "1240", "1200"],
    );
    due(&node, priest);
    assert_eq!(runner(&node, priest)["objective_sequence"], objective);
    assert_eq!(
        node.query_rows(&format!(
            "SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = {priest}"
        ))[0]["scheduled_id"],
        pending["scheduled_id"]
    );
    evidence(&node, "heal-retained-before-completion");
    assert!(poll_until(POLL_TIMEOUT, || node
        .query_rows(&format!(
            "SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = {priest}"
        ))
        .is_empty()));
    let ally_after = node.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid = {ally}"
    ))[0]["health"]
        .parse::<u32>()
        .unwrap();
    assert!(ally_after > ally_before);
    due(&node, priest);
    assert!(poll_until(POLL_TIMEOUT, || runner(&node, priest)["chosen"]
        .contains("follow")));
    let events = node.query_rows(&format!(
        "SELECT * FROM game_spell_cast_event WHERE caster_guid = {priest} AND spell_id = {HEAL}"
    ));
    assert_eq!(
        events.iter().filter(|event| event["kind"] == "2").count(),
        1
    );
    assert_eq!(runner(&node, priest)["objective_sequence"], objective);
    evidence(&node, "heal-resume");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_low_health_at_the_reached_leader_uses_recovery_instead_of_holding() {
    let (node, bots) = fixture("playerbots-companion-safe-recovery");
    let (priest, leader) = (&bots[0], &bots[1]);
    node.assert_call(
        "playerbots_fixture_companion_move",
        &[leader, "1202", "1200"],
    );
    node.assert_call("playerbots_fixture_companion_health", &[priest, "25"]);
    let before = node.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid = {priest}"
    ))[0]["health"]
        .parse::<u32>()
        .unwrap();
    select(&node, priest, "cohort");
    due(&node, priest);
    assert!(poll_until(POLL_TIMEOUT, || !node
        .query_rows(&format!(
            "SELECT target_guid FROM game_pending_cast WHERE caster_guid = {priest}"
        ))
        .is_empty()));
    assert!(runner(&node, priest)["chosen"].contains("heal"));
    assert!(!runner(&node, priest)["chosen"].contains("survival"));
    assert_eq!(
        node.query_rows(&format!(
            "SELECT target_guid FROM game_pending_cast WHERE caster_guid = {priest}"
        ))[0]["target_guid"],
        *priest
    );
    assert!(poll_until(POLL_TIMEOUT, || node
        .query_rows(&format!(
            "SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = {priest}"
        ))
        .is_empty()));
    let after = node.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid = {priest}"
    ))[0]["health"]
        .parse::<u32>()
        .unwrap();
    assert!(after > before);
    evidence(&node, "safe-recovery");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_non_healer_companion_retains_self_recovery() {
    let (node, bots) = fixture_role("playerbots-companion-non-healer-recovery", "2");
    let (companion, leader) = (&bots[0], &bots[1]);
    node.assert_call(
        "playerbots_fixture_companion_move",
        &[leader, "1202", "1200"],
    );
    node.assert_call("playerbots_fixture_companion_health", &[companion, "25"]);
    select(&node, companion, "cohort");
    due(&node, companion);
    let state = runner(&node, companion);
    assert!(state["chosen"].contains("recovery"), "{state:?}");
    let pending = node.query_rows(&format!(
        "SELECT spell_id, target_guid FROM game_pending_cast WHERE caster_guid = {companion}"
    ));
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["spell_id"], HEAL);
    assert_eq!(pending[0]["target_guid"], *companion);
    evidence(&node, "non-healer-recovery");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_casting_position_retains_one_injured_ally_across_movement_legs() {
    let (node, bots) = fixture("playerbots-companion-target-retention");
    let (priest, leader, ally) = (&bots[0], &bots[1], &bots[2]);
    node.assert_call(
        "playerbots_fixture_companion_move",
        &[priest, "1340", "1200"],
    );
    node.assert_call("playerbots_fixture_companion_health", &[leader, "40"]);
    node.assert_call("playerbots_fixture_companion_move", &[ally, "1400", "1200"]);
    node.assert_call("playerbots_fixture_companion_health", &[ally, "30"]);
    node.assert_call("playerbots_fixture_runner_select_cohort", &[priest]);
    let mut movement_legs = 0;
    let mut observed_splines = Vec::new();
    let mut previous_endpoint: Option<(f32, f32)> = None;
    let mut previous_spline_id: Option<String> = None;
    let pending = loop {
        pass_once(&node, priest);
        let retained = runner(&node, priest);
        assert!(retained["chosen"].contains(ally), "{retained:?}");
        assert!(retained["companion_heal_target_guid"].contains(ally));
        let pending = node.query_rows(&format!(
            "SELECT scheduled_id, target_guid FROM game_pending_cast WHERE caster_guid = {priest}"
        ));
        if let Some(pending) = pending.first() {
            assert!(spline(&node, priest).is_none());
            assert_eq!(pending["target_guid"], *ally);
            break pending.clone();
        }
        assert!(
            retained["chosen"].contains("castingPosition"),
            "{retained:?}"
        );
        let leg = spline(&node, priest).expect("casting-position pass did not start a spline");
        assert_ne!(leg["dur_ms"], "0");
        if let Some(previous) = &previous_spline_id {
            assert_ne!(&leg["spline_id"], previous);
        }
        if let Some((x, y)) = previous_endpoint {
            assert!((leg["sx"].parse::<f32>().unwrap() - x).abs() < 0.01);
            assert!((leg["sy"].parse::<f32>().unwrap() - y).abs() < 0.01);
        }
        previous_endpoint = Some((
            leg["dx"].parse::<f32>().unwrap(),
            leg["dy"].parse::<f32>().unwrap(),
        ));
        previous_spline_id = Some(leg["spline_id"].clone());
        observed_splines.push(leg.clone());
        evidence(
            &node,
            &format!("target-retention-leg-{}", movement_legs + 1),
        );
        assert!(movement_legs < 6, "{retained:?}");
        assert!(poll_until(POLL_TIMEOUT, || spline_finished(
            &node, priest, &leg
        )));
        movement_legs += 1;
        if movement_legs == 1 {
            node.assert_call("playerbots_fixture_companion_health", &[leader, "10"]);
        }
    };
    assert!((2..=6).contains(&movement_legs));
    let path = support::log_dir().join(format!("{}-target-retention-legs.json", node.shard_name()));
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "completed_legs": movement_legs,
            "splines": &observed_splines,
            "pending_cast": &pending,
        }))
        .unwrap(),
    )
    .unwrap();

    assert!(poll_until(POLL_TIMEOUT, || node
        .query_rows(&format!(
            "SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = {priest}"
        ))
        .is_empty()));
    evidence(&node, "target-retention-cast-complete");

    node.assert_call("playerbots_fixture_companion_health", &[ally, "100"]);
    pass_once(&node, priest);
    let replaced = runner(&node, priest);
    evidence(&node, "target-retention");
    assert!(node
        .query_rows(&format!(
            "SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = {priest} AND scheduled_id = {}",
            pending["scheduled_id"]
        ))
        .is_empty());
    assert!(replaced["companion_heal_target_guid"].contains(leader));
    assert!(replaced["chosen"].contains(leader), "{replaced:?}");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_unsupported_channel_does_not_hide_a_supported_learned_heal() {
    let (node, bots) = fixture("playerbots-companion-mixed-heals");
    let (priest, ally) = (&bots[0], &bots[2]);
    node.assert_call("playerbots_fixture_companion_mixed_heals", &[priest]);
    let bot = &node.query_rows(&format!(
        "SELECT class, role FROM pkg_playerbots_bot WHERE character_guid = {priest}"
    ))[0];
    for spell in [HEAL, CHANNEL_HEAL] {
        let rotations = node.query_rows(&format!(
            "SELECT class, role FROM pkg_playerbots_rotation WHERE spell_id = {spell}"
        ));
        assert_eq!(rotations.len(), 1, "{spell}: {rotations:?}");
        assert_eq!(rotations[0]["class"], bot["class"]);
        assert_eq!(rotations[0]["role"], bot["role"]);
    }
    node.assert_call("playerbots_fixture_companion_health", &[ally, "25"]);
    select(&node, priest, "cohort");
    due(&node, priest);
    let state = runner(&node, priest);
    assert!(state["chosen"].contains(HEAL), "{state:?}");
    assert!(!state["chosen"].contains(CHANNEL_HEAL), "{state:?}");
    let pending = node.query_rows(&format!(
        "SELECT spell_id, target_guid FROM game_pending_cast WHERE caster_guid = {priest}"
    ));
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["spell_id"], HEAL);
    assert_eq!(pending[0]["target_guid"], *ally);
    evidence(&node, "mixed-heal-capabilities");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_poor_range_selects_the_injured_allys_casting_position() {
    let (node, bots) = fixture("playerbots-companion-range");
    let (priest, ally) = (&bots[0], &bots[2]);
    node.assert_call("playerbots_fixture_companion_move", &[ally, "1400", "1200"]);
    node.assert_call("playerbots_fixture_companion_health", &[ally, "25"]);
    select(&node, priest, "cohort");
    due(&node, priest);
    let state = runner(&node, priest);
    assert!(state["chosen"].contains("castingPosition"), "{state:?}");
    assert!(state["chosen"].contains(ally));
    evidence(&node, "range-prerequisite");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_targeted_los_gate_matches_client_and_bot_casts() {
    let (node, bots) = fixture("playerbots-companion-los-parity");
    let (priest, ally) = (&bots[0], &bots[2]);
    node.assert_call("debug_set_nav_enabled", &["true"]);
    node.assert_call("playerbots_fixture_companion_wall", &[priest, ally]);
    node.assert_call("playerbots_fixture_cast", &[priest, ally]);
    let bot_refusal = node.query_rows(&format!(
        "SELECT outcome FROM pkg_playerbots_action WHERE character_guid = {priest}"
    ));
    assert!(
        bot_refusal
            .iter()
            .any(|row| row["outcome"].contains("noLineOfSight")),
        "{bot_refusal:?}"
    );
    let client = node.call("playerbots_fixture_companion_client_cast", &[priest, ally]);
    assert!(!client.status.success());
    let client_text = format!(
        "{}{}",
        String::from_utf8_lossy(&client.stdout),
        String::from_utf8_lossy(&client.stderr)
    );
    assert!(client_text.contains("line of sight"), "{client_text}");
    evidence(&node, "los-client-bot-parity");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_unlearned_actor_heal_refuses_without_cast_power_or_cooldown_state() {
    let (node, bots) = fixture("playerbots-companion-unlearned");
    let (priest, ally) = (&bots[0], &bots[2]);
    let power = node.query_rows(&format!(
        "SELECT power FROM game_world_entity WHERE guid = {priest}"
    ))[0]["power"]
        .clone();
    node.assert_call("playerbots_fixture_companion_forget_heal", &[priest]);
    node.assert_call("playerbots_fixture_cast", &[priest, ally]);
    let action = node.query_rows(&format!(
        "SELECT outcome FROM pkg_playerbots_action WHERE character_guid = {priest}"
    ));
    assert!(
        action
            .iter()
            .any(|row| row["outcome"].contains("unlearnedSpell")),
        "{action:?}"
    );
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM game_pending_cast WHERE caster_guid = {priest}"
        ))
        .is_empty());
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM game_spell_cd WHERE caster_guid = {priest} AND spell_id = {HEAL}"
        ))
        .is_empty());
    assert_eq!(
        node.query_rows(&format!(
            "SELECT power FROM game_world_entity WHERE guid = {priest}"
        ))[0]["power"],
        power
    );
    evidence(&node, "unlearned-atomic-refusal");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_explicit_self_target_bypasses_targeted_los_and_completes() {
    let (node, bots) = fixture("playerbots-companion-self-cast");
    let priest = &bots[0];
    node.assert_call("playerbots_fixture_companion_health", &[priest, "25"]);
    let before = health(&node, priest);
    node.assert_call(
        "playerbots_fixture_companion_client_cast",
        &[priest, priest],
    );
    assert!(poll_until(POLL_TIMEOUT, || node
        .query_rows(&format!(
            "SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = {priest}"
        ))
        .is_empty()));
    assert!(health(&node, priest) > before);
    evidence(&node, "self-target-complete");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_untargeted_actor_cast_keeps_its_supported_lifecycle() {
    let (node, bots) = fixture("playerbots-companion-untargeted-cast");
    let priest = &bots[0];
    node.assert_call("playerbots_fixture_companion_client_cast", &[priest, "0"]);
    let pending = node.query_rows(&format!(
        "SELECT target_guid FROM game_pending_cast WHERE caster_guid = {priest}"
    ));
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["target_guid"], "0");
    evidence(&node, "untargeted-started");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_triggered_player_cast_bypasses_targeted_los() {
    let (node, bots) = fixture("playerbots-companion-triggered-cast");
    let (priest, ally) = (&bots[0], &bots[2]);
    node.assert_call("debug_set_nav_enabled", &["true"]);
    node.assert_call("playerbots_fixture_companion_wall", &[priest, ally]);
    let before = health(&node, ally);
    node.assert_call(
        "playerbots_fixture_companion_triggered_cast",
        &[priest, ally],
    );
    assert!(health(&node, ally) > before);
    evidence(&node, "triggered-los-exemption");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_creature_cast_bypasses_targeted_los_and_completes() {
    let (node, bots) = fixture("playerbots-companion-creature-cast");
    let (priest, ally) = (&bots[0], &bots[2]);
    node.assert_call("debug_set_nav_enabled", &["true"]);
    node.assert_call("playerbots_fixture_companion_wall", &[priest, ally]);
    let before = health(&node, ally);
    node.assert_call("playerbots_fixture_companion_creature_cast", &[ally]);
    let creature = ((0xF130u64 << 48) | (5_090_301u64 << 24) | 1).to_string();
    assert!(poll_until(POLL_TIMEOUT, || node
        .query_rows(&format!(
            "SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = {creature}"
        ))
        .is_empty()));
    assert!(health(&node, ally) > before);
    evidence(&node, "creature-los-exemption");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_actor_channel_request_has_a_typed_unsupported_refusal() {
    let (node, bots) = fixture("playerbots-companion-channel-refusal");
    let (priest, ally) = (&bots[0], &bots[2]);
    node.assert_call("playerbots_fixture_cast_mode", &[priest, ally, "true"]);
    let action = node.query_rows(&format!(
        "SELECT outcome FROM pkg_playerbots_action WHERE character_guid = {priest}"
    ));
    assert!(
        action
            .iter()
            .any(|row| row["outcome"].contains("unsupportedChannel")),
        "{action:?}"
    );
    evidence(&node, "channel-refusal");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_explicit_cancellation_releases_the_heal_and_resumes_follow() {
    let (node, bots) = fixture("playerbots-companion-cancel-resume");
    let (priest, ally) = (&bots[0], &bots[2]);
    node.assert_sql("UPDATE game_spell SET cast_time_ms = 60000 WHERE spell_id = 5090100");
    node.assert_call("playerbots_fixture_companion_health", &[ally, "25"]);
    node.assert_call("playerbots_fixture_runner_select_cohort", &[priest]);
    pass_once(&node, priest);
    let pending = node.query_rows(&format!(
        "SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = {priest}"
    ));
    assert_eq!(pending.len(), 1);
    let scheduled_id = pending[0]["scheduled_id"].clone();
    let objective = runner(&node, priest)["objective_sequence"].clone();
    node.assert_call("playerbots_fixture_cancel", &[priest, "false"]);
    evidence(&node, "cancelled-before-provisioning");
    assert!(node
        .query_rows(&format!(
            "SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = {priest}"
        ))
        .is_empty());
    let cancelled = node.query_rows(&format!(
        "SELECT cast_id, outcome FROM pkg_playerbots_action WHERE character_guid = {priest}"
    ));
    assert_eq!(cancelled.len(), 1);
    assert_eq!(cancelled[0]["cast_id"], scheduled_id);
    assert_eq!(cancelled[0]["outcome"], "(cancelled = ())");
    assert!(runner(&node, priest)["last_outcome"].contains("cancelled"));
    node.assert_call("playerbots_fixture_companion_health", &[ally, "100"]);
    let resumed = resume_follow_after_provisioning(&node, priest, &objective, "cancel-resume");
    assert!(resumed["companion_heal_target_guid"].contains("none"));
    evidence(&node, "cancel-resume");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_completion_time_los_refusal_releases_the_heal_and_resumes_follow() {
    let (node, bots) = fixture("playerbots-companion-los-resume");
    let (priest, ally) = (&bots[0], &bots[2]);
    node.assert_call("debug_set_nav_enabled", &["true"]);
    node.assert_call("playerbots_fixture_companion_health", &[ally, "25"]);
    node.assert_call("playerbots_fixture_runner_select_cohort", &[priest]);
    pass_once(&node, priest);
    assert!(poll_until(POLL_TIMEOUT, || !node
        .query_rows(&format!(
            "SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = {priest}"
        ))
        .is_empty()));
    let objective = runner(&node, priest)["objective_sequence"].clone();
    node.assert_call("playerbots_fixture_companion_wall", &[priest, ally]);
    assert!(poll_until(POLL_TIMEOUT, || node
        .query_rows(&format!(
            "SELECT outcome FROM pkg_playerbots_action WHERE character_guid = {priest}"
        ))
        .iter()
        .any(|row| row["outcome"].contains("noLineOfSight"))));
    assert!(runner(&node, priest)["last_outcome"].contains("refused"));
    assert!(runner(&node, priest)["companion_heal_target_guid"].contains("none"));
    node.assert_call("playerbots_fixture_companion_health", &[ally, "100"]);
    resume_follow_after_provisioning(&node, priest, &objective, "los-refusal-resume");
    evidence(&node, "los-refusal-resume");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_death_and_resurrection_preserve_role_then_regroup() {
    let (node, bots) = fixture("playerbots-companion-death-regroup");
    let priest = &bots[0];
    select(&node, priest, "cohort");
    due(&node, priest);
    let objective = runner(&node, priest)["objective_sequence"].clone();
    node.assert_call(
        "playerbots_fixture_runner_damage",
        &[priest, "0", "1000000"],
    );
    due(&node, priest);
    assert!(runner(&node, priest)["chosen"].contains("resurrection"));
    evidence(&node, "dead-objective-retained");
    for _ in 0..3 {
        due(&node, priest);
        if node.query_rows(&format!(
            "SELECT dead FROM game_world_entity WHERE guid = {priest}"
        ))[0]["dead"]
            == "false"
        {
            break;
        }
    }
    assert_eq!(
        node.query_rows(&format!(
            "SELECT dead FROM game_world_entity WHERE guid = {priest}"
        ))[0]["dead"],
        "false"
    );
    evidence(&node, "resurrected-before-regroup");
    for _ in 0..8 {
        due(&node, priest);
        let state = runner(&node, priest);
        assert_eq!(state["objective_sequence"], objective);
        if state["chosen"].contains("follow") {
            break;
        }
        if !node
            .query_rows(&format!(
                "SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = {priest}"
            ))
            .is_empty()
        {
            assert!(poll_until(POLL_TIMEOUT, || node
                .query_rows(&format!(
                    "SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = {priest}"
                ))
                .is_empty()));
        }
    }
    assert_eq!(runner(&node, priest)["objective_sequence"], objective);
    assert!(runner(&node, priest)["companion_leader_guid"].contains(&bots[1]));
    assert!(runner(&node, priest)["chosen"].contains("follow"));
    let bot = node.query_rows(&format!(
        "SELECT role FROM pkg_playerbots_bot WHERE character_guid = {priest}"
    ));
    assert_eq!(bot[0]["role"], "1");
    evidence(&node, "death-regroup");
}

#[test]
#[ignore = "requires the pinned PB-002 Wasm, SpacetimeDB, and the playerbots Package"]
fn playerbots_populated_pb002_runner_state_upgrades_with_objective_and_foreground_intact() {
    let preceding = preceding_pb002();
    let old_wasm = preceding.wasm;
    assert_ne!(
        blake3::hash(&old_wasm),
        blake3::hash(support::module_bytes())
    );
    let mut node = Standalone::start("playerbots-companion-pb002-migration");
    node.publish_module_bytes(&old_wasm);
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("playerbots_spawn_role", &["1", "1200", "1200", "50", "1"]);
    node.assert_call("playerbots_fixture_prepare", &[]);
    node.assert_sql("UPDATE game_spell SET cast_time_ms = 60000 WHERE spell_id = 5090100");
    let bot = node.query_rows("SELECT character_guid FROM pkg_playerbots_bot")[0]["character_guid"]
        .clone();
    node.assert_call("playerbots_fixture_runner_stage", &[&bot, "true"]);
    select(&node, &bot, "cohort");
    assert!(poll_until(POLL_TIMEOUT, || {
        runner(&node, &bot)["foreground"].contains("cast")
    }));
    for _ in 0..4 {
        node.assert_call("playerbots_fixture_runner_damage", &[&bot, "0", "1"]);
    }
    node.assert_call("playerbots_fixture_freeze", &[&bot]);
    let before = runner(&node, &bot);
    let pending_before = node.query_rows(&format!(
        "SELECT * FROM game_pending_cast WHERE caster_guid = {bot}"
    ));
    assert_eq!(pending_before.len(), 1);
    assert!(before["objective"].contains("returnHome"));
    assert!(before["foreground"].contains("cast"));
    node.publish_module();
    record_inputs(&node);
    let after = runner(&node, &bot);
    let pending_after = node.query_rows(&format!(
        "SELECT * FROM game_pending_cast WHERE caster_guid = {bot}"
    ));
    assert_eq!(after["objective_sequence"], before["objective_sequence"]);
    assert_eq!(after["objective"], before["objective"]);
    assert_eq!(after["foreground"], before["foreground"]);
    assert_eq!(pending_after, pending_before);
    assert!(after["companion_leader_guid"].contains("none"));
    assert!(after["companion_heal_target_guid"].contains("none"));
    let path = support::log_dir().join(format!("{}-migration.json", node.shard_name()));
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "spacetimedb": "2.7.1",
            "preceding_build": preceding.manifest,
            "preceding_wasm_blake3": blake3::hash(&old_wasm).to_hex().to_string(),
            "current_wasm_blake3": blake3::hash(support::module_bytes()).to_hex().to_string(),
            "before": before,
            "after": after,
            "pending_before": pending_before,
            "pending_after": pending_after,
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
#[ignore = "requires the pinned PB-002 Wasm, SpacetimeDB, and the playerbots Package"]
fn playerbots_canonical_lesser_heal_repair_preserves_changed_definitions() {
    let preceding = preceding_pb002();
    let old_wasm = preceding.wasm;
    let mut node = Standalone::start("playerbots-companion-seed-repair");
    node.publish_module_bytes(&old_wasm);
    node.assert_call("claim_operator", &[]);
    node.publish_module();
    record_inputs(&node);
    let effect_target = || {
        node.query_rows("SELECT target FROM game_spell_effect WHERE id = 8200")[0]["target"].clone()
    };
    assert_eq!(effect_target(), "0");
    node.assert_call(
        "playerbots_fixture_companion_extra_lesser_heal_effect",
        &["true"],
    );
    node.assert_call("debug_repair_after_publish", &[]);
    let target_with_extra_effect = effect_target();
    assert_eq!(target_with_extra_effect, "0");
    node.assert_call(
        "playerbots_fixture_companion_extra_lesser_heal_effect",
        &["false"],
    );
    node.assert_call("debug_repair_after_publish", &[]);
    let canonical_target = effect_target();
    assert_eq!(canonical_target, "2");
    node.assert_call("debug_repair_after_publish", &[]);
    let repeated_target = effect_target();
    assert_eq!(repeated_target, "2");
    node.assert_call("playerbots_fixture_companion_lesser_heal_target", &["3"]);
    node.assert_call("debug_repair_after_publish", &[]);
    let changed_target = effect_target();
    assert_eq!(changed_target, "3");
    let path = support::log_dir().join(format!("{}-seed-repair.json", node.shard_name()));
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "spacetimedb": "2.7.1",
            "preceding_build": preceding.manifest,
            "preceding_wasm_blake3": blake3::hash(&old_wasm).to_hex().to_string(),
            "current_wasm_blake3": blake3::hash(support::module_bytes()).to_hex().to_string(),
            "canonical_lesser_heal_target_after_repair": canonical_target,
            "repeated_repair_target": repeated_target,
            "additional_effect_preserved_legacy_target": target_with_extra_effect,
            "changed_lesser_heal_target_after_repair": changed_target,
        }))
        .unwrap(),
    )
    .unwrap();
}
