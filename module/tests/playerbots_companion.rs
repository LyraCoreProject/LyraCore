//! Durable Priest companion behavior against private, seeded Module databases.

mod support;
use std::collections::BTreeMap;
use support::{poll_until, Standalone, POLL_TIMEOUT};

const HEAL: &str = "5090100";

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

fn fixture(name: &str) -> (Standalone, Vec<String>) {
    let mut node = Standalone::start(name);
    node.publish_module();
    record_inputs(&node);
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("playerbots_spawn_role", &["3", "1200", "1200", "50", "1"]);
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

fn evidence(node: &Standalone, case: &str) {
    let path = support::log_dir().join(format!("{}-{case}.json", node.shard_name()));
    let record = serde_json::json!({
        "case": case,
        "spacetimedb": "2.7.1",
        "runner": node.query_rows("SELECT * FROM pkg_playerbots_runner"),
        "actions": node.query_rows("SELECT * FROM pkg_playerbots_action"),
        "entities": node.query_rows("SELECT guid, map_id, instance_id, x, y, z, health, max_health, dead FROM game_world_entity"),
        "party": node.query_rows("SELECT * FROM game_group_member"),
        "pending_casts": node.query_rows("SELECT * FROM game_pending_cast"),
        "cast_events": node.query_rows("SELECT * FROM game_spell_cast_event"),
        "melee": node.query_rows("SELECT * FROM game_melee_attack"),
    });
    std::fs::write(path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn priest_follows_a_moving_human_leader_without_pulling() {
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
fn priest_retains_one_ally_cast_while_the_leader_moves_then_resumes_follow() {
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
fn low_health_at_the_reached_leader_uses_recovery_instead_of_holding() {
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
fn targeted_cast_gates_match_and_preserve_explicit_exemptions() {
    let (node, bots) = fixture("playerbots-companion-gates");
    let (priest, ally) = (&bots[0], &bots[2]);
    node.assert_call("playerbots_fixture_companion_move", &[ally, "1400", "1200"]);
    node.assert_call("playerbots_fixture_companion_health", &[ally, "25"]);
    select(&node, priest, "cohort");
    due(&node, priest);
    let range = runner(&node, priest);
    assert!(range["chosen"].contains("castingPosition"), "{range:?}");
    assert!(range["chosen"].contains(ally));
    evidence(&node, "range-prerequisite");
    select(&node, priest, "frozen");

    node.assert_call("debug_set_nav_enabled", &["true"]);
    node.assert_call("playerbots_fixture_companion_wall", &[priest, ally]);
    select(&node, priest, "cohort");
    due(&node, priest);
    let state = runner(&node, priest);
    assert!(state["chosen"].contains("castingPosition"), "{state:?}");
    assert!(state["chosen"].contains(ally));
    let movement = node.query_rows(&format!(
        "SELECT outcome FROM pkg_playerbots_action WHERE character_guid = {priest}"
    ));
    assert!(
        movement.iter().any(|row| {
            row["outcome"].contains("blocked") || row["outcome"].contains("partial")
        }),
        "{movement:?}"
    );
    evidence(&node, "los-prerequisite");
    select(&node, priest, "frozen");
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

    let before = node.query_rows(&format!(
        "SELECT power FROM game_world_entity WHERE guid = {priest}"
    ))[0]["power"]
        .clone();
    node.assert_call("playerbots_fixture_companion_forget_heal", &[priest]);
    node.assert_call("playerbots_fixture_cast", &[priest, ally]);
    let unlearned = node.query_rows(&format!(
        "SELECT outcome FROM pkg_playerbots_action WHERE character_guid = {priest}"
    ));
    assert!(
        unlearned
            .iter()
            .any(|row| row["outcome"].contains("unlearnedSpell")),
        "{unlearned:?}"
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
        before
    );

    node.assert_call("playerbots_fixture_companion_learn_heal", &[priest]);
    node.assert_call("playerbots_fixture_companion_health", &[priest, "25"]);
    let self_before = node.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid = {priest}"
    ))[0]["health"]
        .parse::<u32>()
        .unwrap();
    node.assert_call(
        "playerbots_fixture_companion_client_cast",
        &[priest, priest],
    );
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
    assert!(
        node.query_rows(&format!(
            "SELECT health FROM game_world_entity WHERE guid = {priest}"
        ))[0]["health"]
            .parse::<u32>()
            .unwrap()
            > self_before
    );
    evidence(&node, "client-self-exemption-complete");
    std::thread::sleep(std::time::Duration::from_secs(2));
    node.assert_call("playerbots_fixture_companion_client_cast", &[priest, "0"]);
    assert!(!node
        .query_rows(&format!(
            "SELECT * FROM game_pending_cast WHERE caster_guid = {priest}"
        ))
        .is_empty());
    node.assert_call("playerbots_fixture_cancel", &[priest, "false"]);
    let triggered_before = node.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid = {ally}"
    ))[0]["health"]
        .parse::<u32>()
        .unwrap();
    node.assert_call(
        "playerbots_fixture_companion_triggered_cast",
        &[priest, ally],
    );
    let triggered_after = node.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid = {ally}"
    ))[0]["health"]
        .parse::<u32>()
        .unwrap();
    assert!(triggered_after > triggered_before);
    node.assert_call("playerbots_fixture_companion_health", &[ally, "25"]);
    node.assert_call("playerbots_fixture_companion_creature_cast", &[ally]);
    let creature = ((0xF130u64 << 48) | (5_090_301u64 << 24) | 1).to_string();
    assert!(poll_until(POLL_TIMEOUT, || node
        .query_rows(&format!(
            "SELECT * FROM game_pending_cast WHERE caster_guid = {creature}"
        ))
        .is_empty()));
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM game_spell_cast_event WHERE caster_guid = {creature} AND spell_id = {HEAL}"
        ))
        .iter()
        .any(|event| event["kind"] == "2"));
    node.assert_call("playerbots_fixture_cast_mode", &[priest, ally, "true"]);
    let channel = node.query_rows(&format!(
        "SELECT outcome FROM pkg_playerbots_action WHERE character_guid = {priest}"
    ));
    assert!(
        channel
            .iter()
            .any(|row| row["outcome"].contains("unsupportedChannel")),
        "{channel:?}"
    );
    evidence(&node, "cast-gates");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn cancellation_and_death_preserve_the_companion_role_and_objective() {
    let (node, bots) = fixture("playerbots-companion-recovery");
    let (priest, ally) = (&bots[0], &bots[2]);
    select(&node, priest, "cohort");
    node.assert_call("playerbots_fixture_companion_health", &[ally, "25"]);
    due(&node, priest);
    assert!(poll_until(POLL_TIMEOUT, || !node
        .query_rows(&format!(
            "SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = {priest}"
        ))
        .is_empty()));
    let objective = runner(&node, priest)["objective_sequence"].clone();
    node.assert_call("playerbots_fixture_cancel", &[priest, "false"]);
    node.assert_call("playerbots_fixture_companion_health", &[ally, "100"]);
    due(&node, priest);
    assert!(runner(&node, priest)["chosen"].contains("follow"));
    evidence(&node, "explicit-cancel-resume");
    node.assert_call("debug_set_nav_enabled", &["true"]);
    node.assert_call("playerbots_fixture_companion_health", &[ally, "25"]);
    due(&node, priest);
    assert!(poll_until(POLL_TIMEOUT, || !node
        .query_rows(&format!(
            "SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = {priest}"
        ))
        .is_empty()));
    node.assert_call("playerbots_fixture_companion_wall", &[priest, ally]);
    assert!(poll_until(POLL_TIMEOUT, || node
        .query_rows(&format!(
            "SELECT outcome FROM pkg_playerbots_action WHERE character_guid = {priest}"
        ))
        .iter()
        .any(|row| row["outcome"].contains("noLineOfSight"))));
    assert!(runner(&node, priest)["last_outcome"].contains("refused"));
    node.assert_call("playerbots_fixture_companion_health", &[ally, "100"]);
    due(&node, priest);
    assert!(runner(&node, priest)["chosen"].contains("follow"));
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
    evidence(&node, "cancel-death-regroup");
}

#[test]
#[ignore = "requires the pinned PB-002 Wasm, SpacetimeDB, and the playerbots Package"]
fn populated_pb002_runner_state_upgrades_with_objective_and_foreground_intact() {
    let old_path = std::env::var_os("PLAYERBOTS_COMPANION_PRECEDING_WASM")
        .expect("PLAYERBOTS_COMPANION_PRECEDING_WASM must name the merged PB-002 Wasm");
    let old_wasm = std::fs::read(old_path).unwrap();
    assert_eq!(
        blake3::hash(&old_wasm).to_hex().to_string(),
        "e5bfa3e63cd2b03c782af829029030b9a6368b0e6552295958cc128613083fcb"
    );
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
    let bot = node.query_rows("SELECT character_guid FROM pkg_playerbots_bot")[0]["character_guid"]
        .clone();
    node.assert_call("playerbots_fixture_runner_stage", &[&bot, "true"]);
    select(&node, &bot, "cohort");
    assert!(poll_until(POLL_TIMEOUT, || runner(&node, &bot)
        ["foreground"]
        .contains("cast")));
    for _ in 0..4 {
        node.assert_call("playerbots_fixture_runner_damage", &[&bot, "0", "1"]);
    }
    node.assert_call("playerbots_fixture_freeze", &[&bot]);
    let before = runner(&node, &bot);
    assert!(before["objective"].contains("returnHome"));
    assert!(before["foreground"].contains("cast"));
    node.publish_module();
    record_inputs(&node);
    let after = runner(&node, &bot);
    assert_eq!(after["objective_sequence"], before["objective_sequence"]);
    assert_eq!(after["objective"], before["objective"]);
    assert_eq!(after["foreground"], before["foreground"]);
    assert!(after["companion_leader_guid"].contains("none"));
    let effect_target = || {
        node.query_rows("SELECT target FROM game_spell_effect WHERE id = 8200")[0]["target"].clone()
    };
    assert_eq!(effect_target(), "0");
    node.assert_call("debug_repair_after_publish", &[]);
    assert_eq!(effect_target(), "2");
    node.assert_call("debug_repair_after_publish", &[]);
    assert_eq!(effect_target(), "2");
    node.assert_call("playerbots_fixture_companion_lesser_heal_target", &["3"]);
    node.assert_call("debug_repair_after_publish", &[]);
    assert_eq!(effect_target(), "3");
    let path = support::log_dir().join(format!("{}-migration.json", node.shard_name()));
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "spacetimedb": "2.7.1",
            "preceding_core": "e6a755db0a150bbf73ad97b972fe829f20f6816c",
            "preceding_collection": "155c9e401afb06d5731acedf8fc35a81dbe4aaa6",
            "preceding_wasm_blake3": blake3::hash(&old_wasm).to_hex().to_string(),
            "current_wasm_blake3": blake3::hash(support::module_bytes()).to_hex().to_string(),
            "canonical_lesser_heal_target_after_repair": 2,
            "imported_like_lesser_heal_target_after_repair": effect_target(),
            "before": before,
            "after": after,
        }))
        .unwrap(),
    )
    .unwrap();
}
