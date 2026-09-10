//! Durable companion AreaTrigger decisions on private Module databases.

mod support;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;
use support::Standalone;

struct TransferFixture {
    node: Standalone,
    companion: String,
    leader: String,
    generation: u64,
    mode: u8,
}

fn row(node: &Standalone, sql: &str) -> BTreeMap<String, String> {
    node.query_rows(sql)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("fixture row missing for {sql}"))
}

fn runner(node: &Standalone, guid: &str) -> BTreeMap<String, String> {
    row(
        node,
        &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
    )
}

fn git(path: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(path)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn fixture(name: &str, mode: u8) -> TransferFixture {
    let mut node = Standalone::start(name);
    node.publish_module();
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("playerbots_spawn", &["4", "1200", "1200", "50"]);
    let mut bots = node.query_rows("SELECT character_guid, class, role FROM pkg_playerbots_bot");
    bots.sort_by_key(|bot| bot["character_guid"].parse::<u64>().unwrap());
    let companion = bots
        .iter()
        .find(|bot| bot["class"] == "1" && bot["role"] == "0")
        .unwrap()["character_guid"]
        .clone();
    let leader = bots
        .iter()
        .rev()
        .find(|bot| bot["class"] == "1" && bot["role"] == "0")
        .unwrap()["character_guid"]
        .clone();
    let priest = bots
        .iter()
        .find(|bot| bot["class"] == "5" && bot["role"] == "1")
        .unwrap()["character_guid"]
        .clone();
    let mage = bots
        .iter()
        .find(|bot| bot["class"] == "8" && bot["role"] == "2")
        .unwrap()["character_guid"]
        .clone();
    node.assert_call("playerbots_fixture_prepare", &[]);
    node.assert_call(
        "playerbots_fixture_roles_stage",
        &[&companion, &priest, &mage, &leader],
    );
    for guid in [&companion, &priest, &mage] {
        node.assert_call("playerbots_fixture_provision_steps", &[guid, "32"]);
    }
    let generation = runner(&node, &companion)["generation"]
        .parse::<u64>()
        .unwrap();
    node.assert_call(
        "playerbots_transfer_fixture_stage",
        &[&companion, &leader, &mode.to_string()],
    );
    TransferFixture {
        node,
        companion,
        leader,
        generation,
        mode,
    }
}

fn capture(fixture: &TransferFixture, case: &str) -> serde_json::Value {
    let node = &fixture.node;
    let companion = &fixture.companion;
    let leader = &fixture.leader;
    let core = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let collection = core.join("packages/playerbots");
    let (trigger, source_volume, landing) = if fixture.mode == 3 {
        (
            119,
            "client AreaTrigger 119 sphere centered at (-14.3628,-393.38,64.5605), radius 6",
            "ClassicDB game_areatrigger_teleport 119 to map 0 at (-11208.7,1675.9,24.5733,4.71239)",
        )
    } else {
        (
            78,
            "private AreaTrigger 78 sphere centered at (1208,1200,50), radius 2",
            "ClassicDB game_areatrigger_teleport 78 to map 36 at (-14.5732,-385.475,62.4561,1.5708)",
        )
    };
    let evidence = serde_json::json!({
        "case": case,
        "tested_core": git(core, &["rev-parse", "HEAD"]),
        "tested_collection": git(&collection, &["rev-parse", "HEAD"]),
        "core_dirty": !git(core, &["status", "--porcelain"]).is_empty(),
        "collection_dirty": !git(&collection, &["status", "--porcelain"]).is_empty(),
        "module_wasm_identity": blake3::hash(support::module_bytes()).to_hex().to_string(),
        "content": {
            "fixture_mode": fixture.mode,
            "source_volume": source_volume,
            "landing": landing,
            "supported_trigger": trigger,
            "source_audit": "pb010-imported-portal-source-14cda70a; importer sha256 23ff3ec6e391deb6bdec584a5dc63553c356a3e5069792714446191ce6f7a28f",
        },
        "runner": node.query_rows(&format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {companion}")),
        "bot": node.query_rows(&format!("SELECT guid, map_id, instance_id, x, y, z FROM game_world_entity WHERE guid = {companion}")),
        "character": node.query_rows(&format!("SELECT guid, map_id, pending_instance_id, x, y, z, health FROM game_character WHERE guid = {companion}")),
        "leader_character": node.query_rows(&format!("SELECT guid, map_id, pending_instance_id, x, y, z FROM game_character WHERE guid = {leader}")),
        "leader_partition": node.query_rows(&format!("SELECT character_guid, map_id, instance_id, locator_revision, state FROM game_group_member_partition WHERE character_guid = {leader}")),
        "source_instance": node.query_rows("SELECT instance_id, map_id, party_id FROM game_instance WHERE instance_id = 5098078"),
        "companion_binding": node.query_rows(&format!("SELECT character_guid, instance_id, map_id FROM game_instance_binding WHERE character_guid = {companion} AND map_id = 36")),
        "source_volume": node.query_rows("SELECT id, map_id, x, y, z, radius, box_length, box_width, box_height, box_yaw FROM game_area_trigger WHERE id = 78"),
        "landing": node.query_rows("SELECT trigger_id, target_map, x, y, z, o, name FROM game_areatrigger_teleport WHERE trigger_id = 78"),
        "exit_source_volume": node.query_rows("SELECT id, map_id, x, y, z, radius, box_length, box_width, box_height, box_yaw FROM game_area_trigger WHERE id = 119"),
        "exit_landing": node.query_rows("SELECT trigger_id, target_map, x, y, z, o, name FROM game_areatrigger_teleport WHERE trigger_id = 119"),
        "intent": node.query_rows(&format!("SELECT id, bot_guid, destination_map, destination_instance, controller_generation FROM game_bot_transfer_intent WHERE bot_guid = {companion}")),
        "action": node.query_rows(&format!("SELECT kind, target_guid, spell_id, quest_entry, cast_id, outcome FROM pkg_playerbots_action WHERE character_guid = {companion}")),
        "movement": node.query_rows(&format!("SELECT guid, sx, sy, dx, dy, start_micros, dur_ms FROM game_creature_spline WHERE guid = {companion}")),
        "pending_cast": node.query_rows(&format!("SELECT scheduled_id, spell_id, target_guid FROM game_pending_cast WHERE caster_guid = {companion}")),
        "cast_spell": node.query_rows("SELECT spell_id, cast_time_ms FROM game_spell WHERE spell_id = 5090100"),
        "known_cast_spell": node.query_rows(&format!("SELECT spell_id FROM game_player_spell WHERE character_guid = {companion} AND spell_id = 5090100")),
        "attack": node.query_rows(&format!("SELECT attacker_guid, target_guid FROM game_melee_attack WHERE attacker_guid = {companion}")),
    });
    let path = support::log_dir().join(format!("{}-{case}.json", node.shard_name()));
    std::fs::write(path, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
    evidence
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_companion_approaches_the_exact_supported_areatrigger() {
    let fixture = fixture("playerbots-transfer-approach", 1);
    fixture
        .node
        .assert_call("playerbots_fixture_runner_pass_once", &[&fixture.companion]);
    let evidence = capture(&fixture, "approach");

    let chosen = evidence["runner"][0]["chosen"].as_str().unwrap();
    assert!(
        chosen.contains("areaTrigger") && chosen.contains("transferPosition"),
        "{evidence}"
    );
    assert_eq!(
        evidence["intent"].as_array().unwrap().len(),
        0,
        "{evidence}"
    );
    assert_eq!(
        evidence["movement"].as_array().unwrap().len(),
        1,
        "{evidence}"
    );
    assert_eq!(
        evidence["leader_partition"][0]["map_id"], "36",
        "{evidence}"
    );
    assert_eq!(
        evidence["leader_partition"][0]["instance_id"], "5098078",
        "{evidence}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_companion_enters_the_areatrigger_with_normalized_transfer_state() {
    let fixture = fixture("playerbots-transfer-entry", 2);
    fixture
        .node
        .assert_call("playerbots_fixture_runner_pass_once", &[&fixture.companion]);
    let evidence = capture(&fixture, "entered");

    assert_eq!(
        evidence["source_volume"].as_array().unwrap().len(),
        1,
        "{evidence}"
    );
    assert_eq!(
        evidence["landing"].as_array().unwrap().len(),
        1,
        "{evidence}"
    );
    assert_eq!(
        evidence["intent"].as_array().unwrap().len(),
        1,
        "{evidence}"
    );
    assert_eq!(evidence["intent"][0]["destination_map"], "36", "{evidence}");
    assert_eq!(
        evidence["intent"][0]["destination_instance"], "5098078",
        "{evidence}"
    );
    assert_eq!(
        evidence["source_instance"][0]["party_id"], "5098000",
        "{evidence}"
    );
    assert_eq!(
        evidence["companion_binding"][0]["instance_id"], "5098078",
        "the normal instance admission must bind the companion before Transfer: {evidence}"
    );
    assert_eq!(evidence["character"][0]["map_id"], "36", "{evidence}");
    assert_eq!(
        evidence["character"][0]["pending_instance_id"], "5098078",
        "{evidence}"
    );
    assert_eq!(evidence["character"][0]["x"], "-14.5732", "{evidence}");
    assert_eq!(evidence["character"][0]["y"], "-385.475", "{evidence}");
    assert_eq!(evidence["bot"].as_array().unwrap().len(), 0, "{evidence}");
    assert_eq!(
        evidence["pending_cast"].as_array().unwrap().len(),
        0,
        "{evidence}"
    );
    assert_eq!(
        evidence["attack"].as_array().unwrap().len(),
        0,
        "{evidence}"
    );
    assert_eq!(
        evidence["movement"].as_array().unwrap().len(),
        0,
        "{evidence}"
    );
    let runner = evidence["runner"][0].as_object().unwrap();
    assert_eq!(
        runner["generation"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap(),
        fixture.generation + 1,
        "{evidence}"
    );
    assert_eq!(runner["foreground"], "(none = ())", "{evidence}");
    assert_eq!(runner["chosen"], "(none = ())", "{evidence}");
    assert_eq!(runner["candidate_order"], "", "{evidence}");
    assert_eq!(runner["movement_progress"], "(none = ())", "{evidence}");
    assert_eq!(runner["combat_progress"], "(none = ())", "{evidence}");
    assert_eq!(runner["cast_progress"], "(none = ())", "{evidence}");
    assert_eq!(runner["quest_progress"], "", "{evidence}");
    assert_eq!(runner["progress_age_micros"], "(none = ())", "{evidence}");
    assert_eq!(runner["last_target_health"], "(none = ())", "{evidence}");
    assert_eq!(runner["defense_target"], "(none = ())", "{evidence}");
    assert_eq!(
        runner["companion_heal_target_guid"], "(none = ())",
        "{evidence}"
    );
    assert_eq!(
        runner["companion_fight_target_guid"], "(none = ())",
        "{evidence}"
    );
    assert_eq!(
        runner["companion_buff_target_guid"], "(none = ())",
        "{evidence}"
    );
    assert_eq!(runner["deferred_destinations"], "", "{evidence}");
    assert_eq!(runner["recovery"], "(none = ())", "{evidence}");
    assert!(
        runner["transfer_checkpoint"]
            .as_str()
            .unwrap()
            .contains("5098078"),
        "{evidence}"
    );
    assert!(
        evidence["action"].as_array().unwrap().iter().any(|row| {
            row["kind"] == "(transfer = ())"
                && row["outcome"]
                    .as_str()
                    .unwrap()
                    .contains("transferAccepted")
        }),
        "{evidence}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_sessionless_areatrigger_replay_keeps_its_exact_intent_authority() {
    let fixture = fixture("playerbots-transfer-entry-replay", 2);
    fixture
        .node
        .assert_call("playerbots_fixture_runner_pass_once", &[&fixture.companion]);
    let first = capture(&fixture, "entry-replay-first");
    let intent = first["intent"]
        .as_array()
        .and_then(|intents| intents.first())
        .unwrap_or_else(|| panic!("initial Transfer Intent absent: {first}"));
    let intent_id = intent["id"].as_str().unwrap().to_string();
    let generation = intent["controller_generation"]
        .as_str()
        .unwrap()
        .to_string();
    let exact = fixture.node.call(
        "debug_replay_sessionless_areatrigger",
        &[
            &fixture.companion,
            "78",
            "36",
            "5098078",
            &generation,
            &intent_id,
        ],
    );
    let replayed = capture(&fixture, "entry-replay-exact");

    fixture.node.assert_call(
        "playerbots_select_controller",
        &[&fixture.companion, "{\"frozen\":[]}"],
    );
    let disabled = fixture.node.call(
        "debug_replay_sessionless_areatrigger",
        &[
            &fixture.companion,
            "78",
            "36",
            "5098078",
            &generation,
            &intent_id,
        ],
    );
    fixture.node.assert_call(
        "playerbots_select_controller",
        &[&fixture.companion, "{\"recordOnly\":[]}"],
    );
    let record_only = fixture.node.call(
        "debug_replay_sessionless_areatrigger",
        &[
            &fixture.companion,
            "78",
            "36",
            "5098078",
            &generation,
            &intent_id,
        ],
    );
    fixture.node.assert_call(
        "playerbots_select_controller",
        &[&fixture.companion, "{\"cohort\":[]}"],
    );
    let changed_generation = runner(&fixture.node, &fixture.companion)["generation"]
        .as_str()
        .unwrap()
        .to_string();
    let changed = fixture.node.call(
        "debug_replay_sessionless_areatrigger",
        &[
            &fixture.companion,
            "78",
            "36",
            "5098078",
            &changed_generation,
            &intent_id,
        ],
    );
    let final_state = capture(&fixture, "entry-replay-refusals");
    let exact_output = format!(
        "{}{}",
        String::from_utf8_lossy(&exact.stdout),
        String::from_utf8_lossy(&exact.stderr)
    );
    let disabled_output = format!(
        "{}{}",
        String::from_utf8_lossy(&disabled.stdout),
        String::from_utf8_lossy(&disabled.stderr)
    );
    let record_only_output = format!(
        "{}{}",
        String::from_utf8_lossy(&record_only.stdout),
        String::from_utf8_lossy(&record_only.stderr)
    );
    let changed_output = format!(
        "{}{}",
        String::from_utf8_lossy(&changed.stdout),
        String::from_utf8_lossy(&changed.stderr)
    );
    let evidence = serde_json::json!({
        "first": first,
        "exact_replay": {
            "success": exact.status.success(),
            "output": exact_output,
            "state": replayed,
        },
        "disabled_replay": {
            "success": disabled.status.success(),
            "output": disabled_output,
        },
        "record_only_replay": {
            "success": record_only.status.success(),
            "output": record_only_output,
        },
        "changed_generation_replay": {
            "generation": changed_generation,
            "success": changed.status.success(),
            "output": changed_output,
        },
        "final": final_state,
    });
    let path = support::log_dir().join(format!(
        "{}-entry-replay-authority.json",
        fixture.node.shard_name()
    ));
    std::fs::write(&path, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
    eprintln!("fixture evidence: {}", path.display());

    assert!(exact.status.success(), "{evidence}");
    assert_eq!(
        evidence["exact_replay"]["state"]["intent"], evidence["first"]["intent"],
        "an exact replay must retain the sole durable Transfer Intent: {evidence}"
    );
    assert!(!disabled.status.success(), "{evidence}");
    assert!(
        evidence["disabled_replay"]["output"]
            .as_str()
            .unwrap()
            .contains("session-less Transfer is disabled"),
        "{evidence}"
    );
    assert!(!record_only.status.success(), "{evidence}");
    assert!(
        evidence["record_only_replay"]["output"]
            .as_str()
            .unwrap()
            .contains("session-less Transfer is disabled"),
        "{evidence}"
    );
    assert!(!changed.status.success(), "{evidence}");
    assert!(
        evidence["changed_generation_replay"]["output"]
            .as_str()
            .unwrap()
            .contains("TransferPending"),
        "{evidence}"
    );
    assert_eq!(
        evidence["final"]["intent"], evidence["first"]["intent"],
        "refused replays must not replace the admitted Transfer Intent: {evidence}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_companion_uses_the_audited_deadmines_exit_route() {
    let fixture = fixture("playerbots-transfer-deadmines-exit", 3);
    let staged = capture(&fixture, "deadmines-exit-staged");
    assert_eq!(staged["bot"][0]["map_id"], "36", "{staged}");
    assert_eq!(staged["bot"][0]["instance_id"], "5098078", "{staged}");
    assert_eq!(staged["content"]["fixture_mode"], 3, "{staged}");
    assert_eq!(staged["content"]["supported_trigger"], 119, "{staged}");
    assert_eq!(staged["leader_partition"][0]["map_id"], "0", "{staged}");
    assert_eq!(staged["exit_source_volume"][0]["id"], "119", "{staged}");
    assert_eq!(staged["exit_source_volume"][0]["map_id"], "36", "{staged}");
    assert_eq!(staged["exit_source_volume"][0]["x"], "-14.3628", "{staged}");
    assert_eq!(staged["exit_source_volume"][0]["y"], "-393.38", "{staged}");
    assert_eq!(staged["exit_source_volume"][0]["z"], "64.5605", "{staged}");
    assert_eq!(staged["exit_source_volume"][0]["radius"], "6", "{staged}");
    assert_eq!(
        staged["exit_source_volume"][0]["box_length"], "0",
        "{staged}"
    );
    assert_eq!(
        staged["exit_source_volume"][0]["box_width"], "0",
        "{staged}"
    );
    assert_eq!(
        staged["exit_source_volume"][0]["box_height"], "0",
        "{staged}"
    );
    assert_eq!(staged["exit_source_volume"][0]["box_yaw"], "0", "{staged}");
    assert_eq!(staged["source_instance"][0]["map_id"], "36", "{staged}");
    assert_eq!(staged["companion_binding"][0]["map_id"], "36", "{staged}");
    assert_eq!(
        staged["companion_binding"][0]["instance_id"], "5098078",
        "{staged}"
    );
    assert_eq!(staged["exit_landing"][0]["target_map"], "0", "{staged}");
    assert_eq!(staged["exit_landing"][0]["x"], "-11208.7", "{staged}");
    assert_eq!(staged["exit_landing"][0]["y"], "1675.9", "{staged}");
    assert_eq!(staged["exit_landing"][0]["z"], "24.5733", "{staged}");
    assert_eq!(staged["exit_landing"][0]["o"], "4.71239", "{staged}");

    fixture
        .node
        .assert_call("playerbots_fixture_runner_pass_once", &[&fixture.companion]);
    let exited = capture(&fixture, "deadmines-exit-entered");
    assert_eq!(exited["intent"].as_array().unwrap().len(), 1, "{exited}");
    assert_eq!(exited["intent"][0]["destination_map"], "0", "{exited}");
    assert_eq!(exited["intent"][0]["destination_instance"], "0", "{exited}");
    assert_eq!(exited["character"][0]["map_id"], "0", "{exited}");
    assert_eq!(
        exited["character"][0]["pending_instance_id"], "0",
        "{exited}"
    );
    assert!(exited["bot"].as_array().unwrap().is_empty(), "{exited}");
    assert_eq!(
        exited["runner"][0]["generation"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap(),
        fixture.generation + 1,
        "{exited}"
    );
    let checkpoint = exited["runner"][0]["transfer_checkpoint"].as_str().unwrap();
    assert!(
        checkpoint.contains("source_map = 36")
            && checkpoint.contains("source_instance = 5098078")
            && checkpoint.contains("destination_map = 0")
            && checkpoint.contains("destination_instance = 0"),
        "{exited}"
    );
    assert!(
        exited["action"].as_array().unwrap().iter().any(|row| {
            row["kind"] == "(transfer = ())"
                && row["outcome"]
                    .as_str()
                    .unwrap()
                    .contains("transferAccepted")
        }),
        "{exited}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_transfer_advances_generation_and_rejects_the_source_cast_completion() {
    let fixture = fixture("playerbots-transfer-source-cast", 2);
    fixture.node.assert_call(
        "playerbots_fixture_companion_health",
        &[&fixture.companion, "50"],
    );
    fixture.node.assert_call(
        "playerbots_fixture_cast",
        &[&fixture.companion, &fixture.companion],
    );
    let started = capture(&fixture, "source-cast-started");
    let pending = started["pending_cast"].as_array().unwrap();
    assert_eq!(pending.len(), 1, "{started}");
    assert_eq!(
        started["cast_spell"][0]["cast_time_ms"], "5000",
        "{started}"
    );
    assert_eq!(
        started["known_cast_spell"].as_array().unwrap().len(),
        1,
        "{started}"
    );
    let scheduled_id = pending[0]["scheduled_id"].clone();

    fixture
        .node
        .assert_call("playerbots_fixture_runner_pass_once", &[&fixture.companion]);
    let entered = capture(&fixture, "source-cast-transfer-entered");
    assert!(
        entered["pending_cast"].as_array().unwrap().is_empty(),
        "{entered}"
    );
    assert_eq!(
        entered["runner"][0]["generation"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap(),
        fixture.generation + 1,
        "{entered}"
    );
    assert!(
        entered["action"].as_array().unwrap().iter().any(|row| {
            row["kind"] == "(cast = ())"
                && row["cast_id"] == scheduled_id
                && row["outcome"] == "(cancelled = ())"
        }),
        "{entered}"
    );
    assert!(
        entered["action"].as_array().unwrap().iter().any(|row| {
            row["kind"] == "(transfer = ())"
                && row["outcome"]
                    .as_str()
                    .unwrap()
                    .contains("transferAccepted")
        }),
        "{entered}"
    );

    std::thread::sleep(Duration::from_millis(5_200));
    let after_deadline = capture(&fixture, "source-cast-after-deadline");
    assert!(
        after_deadline["pending_cast"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{after_deadline}"
    );
    assert!(
        after_deadline["action"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| {
                row["kind"] == "(cast = ())"
                    && row["cast_id"] == scheduled_id
                    && row["outcome"] == "(cancelled = ())"
            }),
        "{after_deadline}"
    );
    assert_eq!(
        after_deadline["character"][0]["health"], entered["character"][0]["health"],
        "the rejected source completion cannot apply its heal after Transfer: {after_deadline}"
    );
    assert_eq!(
        after_deadline["runner"][0]["cast_progress"], "(none = ())",
        "{after_deadline}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_pending_transfer_intent_keeps_the_admitted_instance_lease() {
    let fixture = fixture("playerbots-transfer-intent-holds-instance", 2);
    fixture
        .node
        .assert_call("playerbots_fixture_runner_pass_once", &[&fixture.companion]);
    let refusal = fixture.node.call("debug_reap_instance", &["5098078"]);
    let refusal_text = format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&refusal.stdout),
        String::from_utf8_lossy(&refusal.stderr)
    );
    std::fs::write(
        support::log_dir().join(format!(
            "{}-intent-holds-instance-refusal.txt",
            fixture.node.shard_name()
        )),
        &refusal_text,
    )
    .unwrap();
    let evidence = capture(&fixture, "intent-holds-instance");

    assert!(!refusal.status.success(), "{evidence}");
    assert!(refusal_text.contains("occupied or claimed"), "{evidence}");
    assert_eq!(
        evidence["source_instance"].as_array().unwrap().len(),
        1,
        "{evidence}"
    );
    assert_eq!(
        evidence["intent"].as_array().unwrap().len(),
        1,
        "{evidence}"
    );
    assert_eq!(evidence["bot"].as_array().unwrap().len(), 0, "{evidence}");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_legacy_crossing_uses_the_runner_transfer_checkpoint() {
    let fixture = fixture("playerbots-transfer-legacy-checkpoint", 2);
    fixture.node.assert_call(
        "playerbots_select_controller",
        &[&fixture.companion, "{\"legacy\":[]}"],
    );
    fixture
        .node
        .assert_call("playerbots_fixture_runner_pass_once", &[&fixture.companion]);
    let evidence = capture(&fixture, "legacy-checkpoint");

    assert_eq!(
        evidence["intent"].as_array().unwrap().len(),
        1,
        "{evidence}"
    );
    assert!(
        evidence["intent"][0]["controller_generation"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > 0,
        "{evidence}"
    );
    assert!(
        evidence["runner"][0]["transfer_checkpoint"]
            .as_str()
            .unwrap()
            .contains("5098078"),
        "{evidence}"
    );
    assert!(
        evidence["action"].as_array().unwrap().iter().any(|row| {
            row["kind"] == "(transfer = ())"
                && row["outcome"]
                    .as_str()
                    .unwrap()
                    .contains("transferAccepted")
        }),
        "{evidence}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_missing_supported_route_records_a_finite_transfer_stop() {
    let fixture = fixture("playerbots-transfer-route-missing", 0);
    fixture
        .node
        .assert_call("playerbots_fixture_runner_pass_once", &[&fixture.companion]);
    let evidence = capture(&fixture, "route-missing");

    let runner = &evidence["runner"][0];
    assert!(
        runner["chosen"].as_str().unwrap().contains("transfer"),
        "{evidence}"
    );
    assert!(
        runner["failures"]
            .as_str()
            .unwrap()
            .contains("transferRouteUnavailable"),
        "{evidence}"
    );
    assert_eq!(
        evidence["intent"].as_array().unwrap().len(),
        0,
        "{evidence}"
    );
    assert_eq!(evidence["bot"].as_array().unwrap().len(), 1, "{evidence}");
}
