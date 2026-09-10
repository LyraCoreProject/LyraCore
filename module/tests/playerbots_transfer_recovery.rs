//! Destination Runner settlement for compact Recovery state carried by Transfer.

mod support;

use serde_json::{json, Value};
use std::collections::BTreeMap;
use support::Standalone;

const CREATURE_PREFIX: u64 = 0xF130u64 << 48;
const ENEMY: u64 = CREATURE_PREFIX | (5_098_001u64 << 24) | 1;

struct Fixture {
    node: Standalone,
    companion: String,
    leader: String,
}

fn fixture(name: &str, leader_x: &str) -> Fixture {
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
    node.assert_call("playerbots_fixture_provision_steps", &[&companion, "32"]);
    node.assert_call(
        "playerbots_fixture_roles_move",
        &[&leader, leader_x, "1200"],
    );
    node.assert_call("playerbots_fixture_runner_pass_once", &[&companion]);
    Fixture {
        node,
        companion,
        leader,
    }
}

fn sorted(mut rows: Vec<BTreeMap<String, String>>, key: &str) -> Vec<BTreeMap<String, String>> {
    rows.sort_by(|left, right| left[key].cmp(&right[key]));
    rows
}

fn snapshot(fixture: &Fixture) -> Value {
    json!({
        "bot": fixture.node.query_rows(&format!(
            "SELECT character_guid, controller FROM pkg_playerbots_bot WHERE character_guid = {}",
            fixture.companion
        )),
        "runner": fixture.node.query_rows(&format!(
            "SELECT character_guid, objective_sequence, objective, recovery, transfer_checkpoint, chosen, candidate_order, last_outcome, failures, history FROM pkg_playerbots_runner WHERE character_guid = {}",
            fixture.companion
        )),
        "entities": sorted(fixture.node.query_rows(&format!(
            "SELECT guid, map_id, instance_id, x, y, z, health, dead FROM game_world_entity WHERE guid = {} OR guid = {}",
            fixture.companion, fixture.leader
        )), "guid"),
        "actions": fixture.node.query_rows(&format!(
            "SELECT character_guid, kind, target_guid, outcome FROM pkg_playerbots_action WHERE character_guid = {}",
            fixture.companion
        )),
    })
}

fn stage_arrival(fixture: &Fixture) -> Value {
    fixture.node.assert_call(
        "playerbots_transfer_recovery_fixture_stage_arrival",
        &[&fixture.companion, &fixture.leader],
    );
    let staged = snapshot(fixture);
    save(fixture, "staged", &staged);
    let checkpoint = staged["runner"][0]["transfer_checkpoint"].as_str().unwrap();
    assert!(checkpoint.contains("stalled_micros = 30000000"), "{staged}");
    assert!(checkpoint.contains("approach = 2"), "{staged}");
    assert!(checkpoint.contains("deferred_micros = 5000000"), "{staged}");
    staged
}

fn save(fixture: &Fixture, label: &str, evidence: &Value) {
    let path = support::log_dir().join(format!("{}-{label}.json", fixture.node.shard_name()));
    std::fs::write(path, serde_json::to_vec_pretty(evidence).unwrap()).unwrap();
}

fn assert_restored(before: &Value, after: &Value, leader: &str) {
    assert_eq!(
        after["runner"][0]["transfer_checkpoint"], "(none = ())",
        "{after}"
    );
    let recovery = after["runner"][0]["recovery"].as_str().unwrap();
    assert!(recovery.contains(&format!("follow = {leader}")), "{after}");
    assert!(recovery.contains("stalled_micros = 30000000"), "{after}");
    assert!(
        recovery.contains("deferred_until_micros = (some"),
        "{after}"
    );
    assert!(
        after["runner"][0]["history"]
            .as_str()
            .unwrap()
            .contains("arrived"),
        "{after}"
    );
    assert_eq!(
        before["runner"][0]["objective_sequence"], after["runner"][0]["objective_sequence"],
        "{after}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_legacy_arrival_restores_its_exact_companion_recovery() {
    let fixture = fixture("playerbots-transfer-recovery-legacy", "1240");
    fixture.node.assert_call(
        "playerbots_select_controller",
        &[&fixture.companion, "{\"legacy\":[]}"],
    );
    let staged = stage_arrival(&fixture);
    fixture
        .node
        .assert_call("playerbots_fixture_runner_pass_once", &[&fixture.companion]);
    let after = snapshot(&fixture);
    let evidence = json!({"staged": staged, "after": after});
    save(&fixture, "legacy-restored", &evidence);
    assert_restored(&evidence["staged"], &evidence["after"], &fixture.leader);
    assert_eq!(
        evidence["after"]["bot"][0]["controller"], "(legacy = ())",
        "{evidence}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_tactical_action_does_not_hide_transfer_recovery_root() {
    let fixture = fixture("playerbots-transfer-recovery-tactical", "1240");
    let staged = stage_arrival(&fixture);
    fixture.node.assert_call(
        "playerbots_fixture_runner_damage",
        &[&fixture.companion, &ENEMY.to_string(), "1"],
    );
    fixture
        .node
        .assert_call("playerbots_fixture_runner_pass_once", &[&fixture.companion]);
    let after = snapshot(&fixture);
    let evidence = json!({"staged": staged, "after": after});
    save(&fixture, "tactical-restored", &evidence);
    assert_restored(&evidence["staged"], &evidence["after"], &fixture.leader);
    let chosen = evidence["after"]["runner"][0]["chosen"].as_str().unwrap();
    assert!(
        (chosen.contains("defense") || chosen.contains("tankFight"))
            && chosen.contains(&ENEMY.to_string()),
        "{evidence}"
    );
    assert!(
        evidence["after"]["runner"][0]["candidate_order"]
            .as_str()
            .unwrap()
            .contains(&format!("entity = {}", fixture.leader)),
        "{evidence}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_arrival_settles_recovery_only_after_exact_follow_completion() {
    let fixture = fixture("playerbots-transfer-recovery-complete", "1202");
    let staged = stage_arrival(&fixture);
    fixture
        .node
        .assert_call("playerbots_fixture_runner_pass_once", &[&fixture.companion]);
    let after = snapshot(&fixture);
    let evidence = json!({"staged": staged, "after": after});
    save(&fixture, "completed", &evidence);
    assert_eq!(
        evidence["after"]["runner"][0]["transfer_checkpoint"], "(none = ())",
        "{evidence}"
    );
    assert!(
        evidence["after"]["runner"][0]["recovery"]
            .as_str()
            .unwrap()
            .contains("attempts = []"),
        "{evidence}"
    );
    assert!(
        evidence["after"]["runner"][0]["history"]
            .as_str()
            .unwrap()
            .contains("arrived"),
        "{evidence}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_arrival_does_not_settle_recovery_for_a_dead_nearby_member() {
    let fixture = fixture("playerbots-transfer-recovery-dead-member", "1202");
    let staged = stage_arrival(&fixture);
    fixture
        .node
        .assert_call("playerbots_fixture_provision_dead", &[&fixture.leader]);
    fixture
        .node
        .assert_call("playerbots_fixture_runner_pass_once", &[&fixture.companion]);
    let after = snapshot(&fixture);
    let evidence = json!({"staged": staged, "after": after});
    save(&fixture, "dead-member-pending", &evidence);
    assert_eq!(
        evidence["after"]["runner"][0]["transfer_checkpoint"],
        evidence["staged"]["runner"][0]["transfer_checkpoint"],
        "{evidence}"
    );
    let leader = evidence["after"]["entities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entity| entity["guid"] == fixture.leader)
        .unwrap();
    assert_eq!(leader["dead"], "true", "{evidence}");
    assert_eq!(leader["health"], "0", "{evidence}");
    assert!(
        !evidence["after"]["runner"][0]["history"]
            .as_str()
            .unwrap()
            .contains("arrived"),
        "{evidence}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_missing_recovery_root_retains_its_exact_transfer_budget() {
    let fixture = fixture("playerbots-transfer-recovery-missing-root", "1240");
    let staged = stage_arrival(&fixture);
    fixture.node.assert_call(
        "playerbots_fixture_orders_remove_member_body",
        &[&fixture.leader],
    );
    fixture
        .node
        .assert_call("playerbots_fixture_runner_pass_once", &[&fixture.companion]);
    let after = snapshot(&fixture);
    let evidence = json!({"staged": staged, "after": after});
    save(&fixture, "missing-root-pending", &evidence);
    assert_eq!(
        evidence["after"]["runner"][0]["transfer_checkpoint"],
        evidence["staged"]["runner"][0]["transfer_checkpoint"],
        "{evidence}"
    );
    assert!(
        evidence["after"]["runner"][0]["recovery"]
            .as_str()
            .unwrap()
            .contains("attempts = []"),
        "{evidence}"
    );
    assert!(
        !evidence["after"]["runner"][0]["history"]
            .as_str()
            .unwrap()
            .contains("arrived"),
        "{evidence}"
    );
}
