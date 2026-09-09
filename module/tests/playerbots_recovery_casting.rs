//! Companion casting-position progress through the production recovery path.

mod support;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use support::{poll_until, Standalone, POLL_TIMEOUT};

const HEAL: u32 = 5_090_100;
const ROOT: u32 = 50_021;
const COMPANION_CREATURE_ENTRY: u32 = 5_090_302;

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

fn row(node: &Standalone, sql: &str) -> BTreeMap<String, String> {
    node.query_rows(sql)
        .into_iter()
        .next()
        .expect("fixture row missing")
}

fn position(node: &Standalone, guid: &str) -> (f32, f32) {
    let entity = row(
        node,
        &format!("SELECT x, y FROM game_world_entity WHERE guid = {guid}"),
    );
    (entity["x"].parse().unwrap(), entity["y"].parse().unwrap())
}

fn spline(node: &Standalone, guid: &str) -> Option<BTreeMap<String, String>> {
    node.query_rows(&format!(
        "SELECT spline_id, start_micros, dur_ms, sx, sy, dx, dy FROM game_creature_spline WHERE guid = {guid}"
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

fn snapshot(
    node: &Standalone,
    priest: &str,
    ally: &str,
    blocker: &str,
    elapsed: Duration,
) -> serde_json::Value {
    serde_json::json!({
        "elapsed_seconds": elapsed.as_secs_f64(),
        "runner": row(node, &format!(
            "SELECT chosen, foreground, failures, deferred_destinations, recovery, companion_heal_target_guid FROM pkg_playerbots_runner WHERE character_guid = {priest}"
        )),
        "priest": row(node, &format!(
            "SELECT guid, x, y, health FROM game_world_entity WHERE guid = {priest}"
        )),
        "ally": row(node, &format!(
            "SELECT guid, x, y, health FROM game_world_entity WHERE guid = {ally}"
        )),
        "blocker": row(node, &format!(
            "SELECT guid, x, y, health FROM game_world_entity WHERE guid = {blocker}"
        )),
        "engagement": node.query_rows(&format!(
            "SELECT attacker_guid, target_guid, ranged_spell_id FROM game_melee_attack WHERE attacker_guid = {blocker}"
        )),
        "threat": node.query_rows(&format!(
            "SELECT source_guid, threat FROM game_threat WHERE creature_guid = {blocker}"
        )),
        "root": node.query_rows(&format!(
            "SELECT target_guid, caster_guid, spell_id, eff_kind, eff_p0 FROM game_aura WHERE target_guid = {blocker} AND spell_id = {ROOT}"
        )),
        "spline": node.query_rows(&format!(
            "SELECT spline_id, start_micros, dur_ms, sx, sy, dx, dy FROM game_creature_spline WHERE guid = {priest}"
        )),
        "movement": node.query_rows(&format!(
            "SELECT kind, observed_micros, target_guid, outcome FROM pkg_playerbots_action WHERE character_guid = {priest}"
        )),
        "pending_cast": node.query_rows(&format!(
            "SELECT scheduled_id, spell_id, target_guid FROM game_pending_cast WHERE caster_guid = {priest}"
        )),
    })
}

fn assert_casting_sample(
    sample: &serde_json::Value,
    ally: &str,
    blocker: &str,
    ally_start_health: &str,
    blocker_x: f32,
) {
    let runner = &sample["runner"];
    assert!(
        runner["chosen"]
            .as_str()
            .unwrap()
            .contains("castingPosition"),
        "{sample}"
    );
    assert!(
        runner["chosen"].as_str().unwrap().contains(ally),
        "{sample}"
    );
    assert!(
        runner["foreground"]
            .as_str()
            .unwrap()
            .contains("castingPosition"),
        "{sample}"
    );
    assert!(
        runner["companion_heal_target_guid"]
            .as_str()
            .unwrap()
            .contains(ally),
        "{sample}"
    );
    assert!(
        runner["recovery"]
            .as_str()
            .unwrap()
            .contains(&format!("heal = {ally}")),
        "{sample}"
    );
    assert!(
        runner["recovery"]
            .as_str()
            .unwrap()
            .contains("stalled_micros = 0"),
        "{sample}"
    );
    assert!(
        !runner["chosen"]
            .as_str()
            .unwrap()
            .contains("recoveryPosition"),
        "{sample}"
    );
    assert!(
        !runner["failures"].as_str().unwrap().contains("noMovement"),
        "{sample}"
    );
    assert!(
        runner["deferred_destinations"]
            .as_str()
            .unwrap()
            .trim_matches(['[', ']', ' '])
            .is_empty(),
        "{sample}"
    );
    assert!(
        sample["pending_cast"].as_array().unwrap().is_empty(),
        "{sample}"
    );
    assert!(
        sample["movement"].as_array().unwrap().iter().any(|action| {
            action["kind"].as_str().unwrap().contains("move")
                && action["outcome"].as_str().unwrap().contains("movement")
        }),
        "{sample}"
    );
    assert_eq!(
        sample["ally"]["health"].as_str(),
        Some(ally_start_health),
        "{sample}"
    );
    assert_eq!(
        sample["blocker"]["x"]
            .as_str()
            .unwrap()
            .parse::<f32>()
            .unwrap(),
        blocker_x,
        "{sample}"
    );
    assert!(
        sample["root"].as_array().unwrap().iter().any(|aura| {
            aura["target_guid"].as_str() == Some(blocker)
                && aura["caster_guid"].as_str() == Some(ally)
                && aura["spell_id"].as_str() == Some("50021")
                && aura["eff_p0"].as_str() == Some("2")
        }),
        "{sample}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_recovery_counts_owned_casting_position_progress_for_the_same_heal() {
    let mut node = Standalone::start("playerbots-recovery-casting-position");
    node.publish_module();
    record_inputs(&node);
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("playerbots_spawn_role", &["3", "1200", "1200", "50", "1"]);
    node.assert_call("playerbots_fixture_prepare", &[]);
    let mut bots: Vec<_> = node
        .query_rows("SELECT character_guid FROM pkg_playerbots_bot")
        .into_iter()
        .map(|row| row["character_guid"].clone())
        .collect();
    bots.sort_by_key(|guid| guid.parse::<u64>().unwrap());
    assert_eq!(bots.len(), 3, "fixture must spawn three bots");
    let (priest, leader, ally) = (&bots[0], &bots[1], &bots[2]);
    node.assert_call(
        "playerbots_fixture_companion_stage",
        &[priest, leader, ally],
    );
    node.assert_call("playerbots_fixture_companion_move", &[ally, "1400", "1200"]);
    node.assert_call("playerbots_fixture_companion_health", &[ally, "25"]);
    let blocker = row(
        &node,
        &format!("SELECT guid FROM game_world_entity WHERE entry = {COMPANION_CREATURE_ENTRY}"),
    )["guid"]
        .clone();
    node.assert_call(
        "playerbots_fixture_companion_move",
        &[&blocker, "1392", "1200"],
    );
    node.assert_call(
        "playerbots_fixture_roles_control",
        &[ally, &blocker, &ROOT.to_string()],
    );
    node.assert_call("playerbots_fixture_roles_enemy_engage", &[&blocker, ally]);
    let initial_engagement = node.query_rows(&format!(
        "SELECT attacker_guid, target_guid, ranged_spell_id FROM game_melee_attack WHERE attacker_guid = {blocker}"
    ));
    node.assert_call("playerbots_fixture_runner_select_cohort", &[priest]);

    let start_position = position(&node, priest);
    let ally_position = position(&node, ally);
    let blocker_position = position(&node, &blocker);
    let ally_start_health = row(
        &node,
        &format!("SELECT health FROM game_world_entity WHERE guid = {ally}"),
    )["health"]
        .clone();
    let spell = row(
        &node,
        &format!("SELECT spell_id, range_yd, cast_time_ms FROM game_spell WHERE spell_id = {HEAL}"),
    );
    let started = Instant::now();
    let mut samples = Vec::new();
    let mut completed_legs = 0usize;
    let mut movement_failed = false;
    node.assert_call("playerbots_fixture_runner_pass_once", &[priest]);
    loop {
        samples.push(snapshot(&node, priest, ally, &blocker, started.elapsed()));
        if started.elapsed() >= Duration::from_secs(12) {
            break;
        }
        let Some(leg) = spline(&node, priest) else {
            movement_failed = true;
            break;
        };
        if !poll_until(POLL_TIMEOUT, || spline_finished(&node, priest, &leg)) {
            movement_failed = true;
            break;
        }
        completed_legs += 1;
        node.assert_call("playerbots_fixture_runner_pass_once", &[priest]);
    }
    let final_position = position(&node, priest);
    let evidence = serde_json::json!({
        "case": "casting-position-recovery-progress",
        "spell_id": HEAL,
        "priest_guid": priest,
        "ally_guid": ally,
        "blocker_guid": blocker,
        "spell": &spell,
        "start_position": start_position,
        "ally_position": ally_position,
        "blocker_position": blocker_position,
        "ally_start_health": ally_start_health,
        "initial_engagement": initial_engagement,
        "final_position": final_position,
        "completed_legs": completed_legs,
        "movement_failed": movement_failed,
        "samples": samples,
    });
    let path = support::log_dir().join(format!(
        "{}-casting-position-recovery-progress.json",
        node.shard_name()
    ));
    std::fs::write(&path, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
    eprintln!("fixture evidence: {}", path.display());

    let samples = evidence["samples"].as_array().unwrap();
    assert!(!movement_failed, "{evidence}");
    assert!(
        samples.last().unwrap()["elapsed_seconds"].as_f64().unwrap() >= 10.0,
        "{evidence}"
    );
    let initial_distance = ((ally_position.0 - start_position.0).powi(2)
        + (ally_position.1 - start_position.1).powi(2))
    .sqrt();
    let blocker_distance = ((ally_position.0 - blocker_position.0).powi(2)
        + (ally_position.1 - blocker_position.1).powi(2))
    .sqrt();
    assert!(
        initial_distance > spell["range_yd"].parse::<f32>().unwrap(),
        "{evidence}"
    );
    assert!(completed_legs >= 2, "{evidence}");
    assert!(final_position.0 > start_position.0 + 20.0, "{evidence}");
    assert!(blocker_distance >= 8.0, "{evidence}");
    assert!(
        initial_engagement.iter().any(|attack| {
            attack["attacker_guid"] == blocker
                && attack["target_guid"] == ally.as_str()
                && attack["ranged_spell_id"] == "0"
        }),
        "{evidence}"
    );
    for sample in samples {
        assert_casting_sample(
            sample,
            ally,
            &blocker,
            &ally_start_health,
            blocker_position.0,
        );
    }
    assert!(
        samples.iter().skip(1).all(|sample| {
            let recovery = sample["runner"]["recovery"].as_str().unwrap();
            recovery.contains("stalled_micros = 0") && recovery.contains("last_movement = (some")
        }),
        "{evidence}"
    );
}
