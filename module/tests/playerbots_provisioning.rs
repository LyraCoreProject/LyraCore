//! Automatic provisioning against private, seeded Standalone databases.

mod support;

use std::collections::BTreeMap;

use support::Standalone;

const PB002_CORE: &str = "e6a755db0a150bbf73ad97b972fe829f20f6816c";
const PB002_PACKAGE: &str = "155c9e401afb06d5731acedf8fc35a81dbe4aaa6";

fn fixture(name: &str, class: &str, role: &str) -> (Standalone, String) {
    let mut node = Standalone::start(name);
    node.publish_module();
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call(
        "playerbots_spawn_class_role",
        &["1", "1200", "1200", "50", class, role],
    );
    let guid = node.query_rows("SELECT character_guid FROM pkg_playerbots_bot")[0]
        ["character_guid"]
        .clone();
    (node, guid)
}

fn select(node: &Standalone, guid: &str, controller: &str) {
    node.assert_call(
        "playerbots_select_controller",
        &[guid, &format!("{{\"{controller}\":[]}}")],
    );
}

fn one(node: &Standalone, query: &str) -> BTreeMap<String, String> {
    node.query_rows(query)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("query returned no row: {query}"))
}

fn item_count(node: &Standalone, guid: &str, entry: u32) -> u32 {
    node.query_rows(&format!(
        "SELECT stack_count FROM game_item_instance WHERE owner_guid = {guid} AND entry = {entry}"
    ))
    .iter()
    .map(|row| row["stack_count"].parse::<u32>().unwrap())
    .sum()
}

fn health(node: &Standalone, guid: &str) -> u32 {
    one(
        node,
        &format!("SELECT health FROM game_world_entity WHERE guid = {guid}"),
    )["health"]
        .parse()
        .unwrap()
}

fn gameplay(node: &Standalone, guid: &str) -> serde_json::Value {
    serde_json::json!({
        "character": node.query_rows(&format!("SELECT level, xp, money FROM game_character WHERE guid = {guid}")),
        "items": node.query_rows(&format!("SELECT entry, slot, stack_count FROM game_item_instance WHERE owner_guid = {guid}")),
        "skills": node.query_rows(&format!("SELECT skill_line, current, max_rank FROM game_player_skill WHERE character_guid = {guid}")),
        "spells": node.query_rows(&format!("SELECT spell_id FROM game_player_spell WHERE character_guid = {guid}")),
        "talents": node.query_rows(&format!("SELECT talent_id, rank FROM game_character_talent WHERE character_guid = {guid}")),
        "quests": node.query_rows(&format!("SELECT quest_entry, counts, rewarded FROM game_character_quest WHERE character_guid = {guid}")),
    })
}

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

fn write_evidence(node: &Standalone, name: &str, mut evidence: serde_json::Value) {
    let core = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let package = core.join("packages/playerbots");
    let mut digest = blake3::Hasher::new();
    digest_files(&package, &mut digest);
    let fields = evidence.as_object_mut().expect("evidence object");
    fields.insert("rust".to_string(), "1.93.0".into());
    fields.insert("spacetimedb".to_string(), "2.7.1".into());
    fields.insert(
        "tested_core".to_string(),
        git(core, &["rev-parse", "HEAD"]).into(),
    );
    fields.insert(
        "tested_collection".to_string(),
        git(&package, &["rev-parse", "HEAD"]).into(),
    );
    fields.insert(
        "core_dirty".to_string(),
        (!git(core, &["status", "--porcelain"]).is_empty()).into(),
    );
    fields.insert(
        "collection_dirty".to_string(),
        (!git(&package, &["status", "--porcelain"]).is_empty()).into(),
    );
    fields.insert(
        "module_wasm_identity".to_string(),
        blake3::hash(support::module_bytes())
            .to_hex()
            .to_string()
            .into(),
    );
    fields.insert(
        "package_content_identity".to_string(),
        digest.finalize().to_hex().to_string().into(),
    );
    fields.insert(
        "seeded_content_identity".to_string(),
        "playerbots-provisioning-v1".into(),
    );
    fields.insert(
        "seeded_geometry_identity".to_string(),
        "playerbots-position-v1".into(),
    );
    fields.insert(
        "content".to_string(),
        serde_json::json!({
            "revision": "playerbots-provisioning-v1",
            "profile_catalogue": "seeded class/role kit plus private profile-item fixture rows",
            "imported_content": null,
        }),
    );
    fields.insert(
        "geometry".to_string(),
        serde_json::json!({
            "revision": "playerbots-seeded-position-v1",
            "map": 0,
            "instance": 0,
            "position": [1200, 1200, 50],
            "client_geometry": null,
        }),
    );
    let path = support::log_dir().join(format!("{}-{name}.json", node.shard_name()));
    std::fs::write(path, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_provisioning_arms_then_reconciles_and_repairs_without_cost() {
    let (node, guid) = fixture("playerbots-provisioning-profile", "1", "0");
    let armed = one(
        &node,
        &format!("SELECT * FROM pkg_playerbots_provisioning WHERE character_guid = {guid}"),
    );
    assert!(armed["cause"].to_ascii_lowercase().contains("spawn"));
    assert!(armed["history"].is_empty());
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM game_player_spell WHERE character_guid = {guid} AND spell_id = 355"
        ))
        .is_empty());

    node.assert_call("debug_set_level", &[&guid, "20"]);
    node.assert_sql(&format!(
        "DELETE FROM game_player_skill WHERE character_guid = {guid} AND skill_line = 43"
    ));
    node.assert_sql(&format!(
        "DELETE FROM game_item_instance WHERE owner_guid = {guid} AND slot = 15"
    ));
    let existing_template = one(&node, "SELECT * FROM game_item_template WHERE entry = 52");
    node.assert_call("playerbots_fixture_provision_catalog", &[]);
    assert_eq!(
        one(&node, "SELECT * FROM game_item_template WHERE entry = 52"),
        existing_template,
        "fixture catalogue must preserve an existing low-ID template"
    );
    node.assert_call("playerbots_fixture_provision_complete_profile", &[&guid]);
    select(&node, &guid, "cohort");
    node.assert_call("playerbots_fixture_runner_due", &[]);
    node.assert_call("playerbots_fixture_runner_pass", &[]);
    let runner = one(
        &node,
        &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
    );
    assert!(runner["last_outcome"]
        .to_ascii_lowercase()
        .contains("provisioning"));
    assert!(runner["chosen"]
        .to_ascii_lowercase()
        .contains("provisioning"));

    let before = one(
        &node,
        &format!("SELECT level, xp, money FROM game_character WHERE guid = {guid}"),
    );
    node.assert_call("playerbots_fixture_provision_steps", &[&guid, "64"]);
    let completed = one(
        &node,
        &format!("SELECT * FROM pkg_playerbots_provisioning WHERE character_guid = {guid}"),
    );
    let history = completed["history"].to_ascii_lowercase();
    let provisioned = gameplay(&node, &guid);
    write_evidence(
        &node,
        "profile",
        serde_json::json!({
            "status": "captured before completion assertions",
            "armed": armed,
            "runner": runner,
            "provisioning": completed,
            "gameplay": provisioned,
            "fixture_profile_spells": node.query_rows("SELECT spell_id FROM pkg_playerbots_kit WHERE class = 1 AND role = 0"),
        }),
    );
    assert_eq!(item_count(&node, &guid, 4496), 4);
    assert_eq!(item_count(&node, &guid, 117), 10);
    assert_eq!(item_count(&node, &guid, 118), 5);
    assert_eq!(item_count(&node, &guid, 1251), 5);
    assert_eq!(item_count(&node, &guid, 2512), 200);
    assert_eq!(item_count(&node, &guid, 6948), 1);
    assert_eq!(
        one(
            &node,
            &format!(
                "SELECT entry FROM game_item_instance WHERE owner_guid = {guid} AND slot = 15"
            ),
        )["entry"],
        "25"
    );
    assert_eq!(
        node.query_rows(&format!(
            "SELECT * FROM game_player_skill WHERE character_guid = {guid} AND skill_line = 43"
        ))
        .len(),
        1
    );
    assert!(!node
        .query_rows(&format!(
            "SELECT * FROM game_character_talent WHERE character_guid = {guid}"
        ))
        .is_empty());
    for kit in
        node.query_rows("SELECT spell_id FROM pkg_playerbots_kit WHERE class = 1 AND role = 0")
    {
        assert_eq!(
            node.query_rows(&format!(
                "SELECT * FROM game_player_spell WHERE character_guid = {guid} AND spell_id = {}",
                kit["spell_id"]
            ))
            .len(),
            1,
            "profile spell {}",
            kit["spell_id"]
        );
    }
    assert!(history.contains("warrior-tank-free"));
    assert_eq!(completed["revision"], "1");
    assert!(history.contains("applied"));
    for action in ["skill", "spell", "talent", "item", "equip"] {
        assert!(
            history.contains(action),
            "missing {action} decision: {history}"
        );
    }
    assert!(completed["cause"].to_ascii_lowercase().contains("periodic"));

    node.assert_call("playerbots_fixture_provision_steps", &[&guid, "64"]);
    assert_eq!(gameplay(&node, &guid), provisioned);
    let after = one(
        &node,
        &format!("SELECT level, xp, money FROM game_character WHERE guid = {guid}"),
    );
    assert_eq!(after, before);

    let repair_spell = node
        .query_rows("SELECT spell_id FROM pkg_playerbots_kit WHERE class = 1 AND role = 0")[0]
        ["spell_id"]
        .clone();
    node.assert_sql(&format!(
        "DELETE FROM game_player_spell WHERE character_guid = {guid} AND spell_id = {repair_spell}"
    ));
    node.assert_call("playerbots_fixture_provision_steps", &[&guid, "64"]);
    assert_eq!(
        node.query_rows(&format!(
            "SELECT * FROM game_player_spell WHERE character_guid = {guid} AND spell_id = {repair_spell}"
        ))
        .len(),
        1
    );
    write_evidence(
        &node,
        "profile",
        serde_json::json!({
            "armed": armed,
            "runner": runner,
            "provisioning": one(&node, &format!("SELECT * FROM pkg_playerbots_provisioning WHERE character_guid = {guid}")),
            "gameplay": gameplay(&node, &guid),
            "fixture_profile_spells": node.query_rows("SELECT spell_id FROM pkg_playerbots_kit WHERE class = 1 AND role = 0"),
        }),
    );
}

fn provision_spell_outcome(node: &Standalone, guid: &str, spell: &str) -> String {
    node.assert_call("playerbots_fixture_provision_reset", &[guid]);
    node.assert_call("playerbots_fixture_provision_spell_action", &[guid, spell]);
    node.assert_call("playerbots_fixture_provision_steps", &[guid, "1"]);
    one(
        node,
        &format!("SELECT history FROM pkg_playerbots_provisioning WHERE character_guid = {guid}"),
    )["history"]
        .to_ascii_lowercase()
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_provisioning_bounds_imported_style_trainer_facts() {
    let (node, guid) = fixture("playerbots-provisioning-trainers", "1", "0");
    node.assert_call("debug_set_level", &[&guid, "20"]);
    node.assert_call("playerbots_fixture_provision_trainer_catalog", &[&guid]);
    select(&node, &guid, "cohort");
    let unrelated_offerings = node
        .query_rows("SELECT spell_id FROM game_trainer_spell")
        .into_iter()
        .filter(|row| {
            let spell_id = row["spell_id"].parse::<u32>().unwrap();
            (5_099_000..5_099_040).contains(&spell_id)
        })
        .count();
    assert_eq!(unrelated_offerings, 40);
    let duplicate_offerings = node
        .query_rows("SELECT id FROM game_trainer_spell WHERE spell_id = 5090210")
        .len();
    let overflow_offerings = node
        .query_rows("SELECT id FROM game_trainer_spell WHERE spell_id = 5090208")
        .len();
    assert_eq!(duplicate_offerings, 17);
    assert_eq!(overflow_offerings, 17);

    let direct = provision_spell_outcome(&node, &guid, "355");
    assert!(direct.contains("applied"), "{direct}");
    assert_eq!(
        node.query_rows(&format!(
            "SELECT * FROM game_player_spell WHERE character_guid = {guid} AND spell_id = 355"
        ))
        .len(),
        1
    );

    let wrapper = provision_spell_outcome(&node, &guid, "2050");
    assert!(wrapper.contains("applied"), "{wrapper}");
    assert_eq!(
        node.query_rows(&format!(
            "SELECT * FROM game_player_spell WHERE character_guid = {guid} AND spell_id = 2050"
        ))
        .len(),
        1
    );

    let duplicate_success = provision_spell_outcome(&node, &guid, "5090210");
    assert!(duplicate_success.contains("applied"), "{duplicate_success}");
    assert_eq!(
        node.query_rows(&format!(
            "SELECT * FROM game_player_spell WHERE character_guid = {guid} AND spell_id = 5090210"
        ))
        .len(),
        1
    );

    let wrong_wrapper = provision_spell_outcome(&node, &guid, "5090211");
    assert!(wrong_wrapper.contains("class"), "{wrong_wrapper}");

    let channel_payload = provision_spell_outcome(&node, &guid, "139");
    assert!(channel_payload.contains("class"), "{channel_payload}");
    let proc_payload = provision_spell_outcome(&node, &guid, "133");
    assert!(proc_payload.contains("class"), "{proc_payload}");
    let wrong_class = provision_spell_outcome(&node, &guid, "5090205");
    assert!(wrong_class.contains("class"), "{wrong_class}");
    let low_level = provision_spell_outcome(&node, &guid, "5090206");
    assert!(low_level.contains("level"), "{low_level}");
    let previous_rank = provision_spell_outcome(&node, &guid, "5090207");
    assert!(previous_rank.contains("prerequisite"), "{previous_rank}");
    let overflow = provision_spell_outcome(&node, &guid, "5090208");
    assert!(overflow.contains("profilelimit"), "{overflow}");
    assert!(overflow.contains("stopped"), "{overflow}");

    for spell in [
        139, 133, 5_090_205, 5_090_206, 5_090_207, 5_090_208, 5_090_211, 5_090_212,
    ] {
        assert!(
            node.query_rows(&format!(
                "SELECT * FROM game_player_spell WHERE character_guid = {guid} AND spell_id = {spell}"
            ))
            .is_empty(),
            "refused spell {spell} changed the spellbook"
        );
    }
    write_evidence(
        &node,
        "trainer-bounds",
        serde_json::json!({
            "fixture_kind": "source-derived imported-style rows; not imported-world proof",
            "unrelated_offerings": unrelated_offerings,
            "duplicate_offerings": duplicate_offerings,
            "overflow_offerings": overflow_offerings,
            "direct": direct,
            "wrapper": wrapper,
            "duplicate_success": duplicate_success,
            "wrong_wrapper": wrong_wrapper,
            "channel_payload": channel_payload,
            "proc_payload": proc_payload,
            "wrong_class": wrong_class,
            "low_level": low_level,
            "previous_rank": previous_rank,
            "overflow": overflow,
            "spellbook": node.query_rows(&format!("SELECT spell_id FROM game_player_spell WHERE character_guid = {guid}")),
        }),
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_provisioning_yields_to_recovery_then_supplies_missing_recovery() {
    let (node, guid) = fixture("playerbots-provisioning-priority", "5", "1");
    node.assert_call("playerbots_fixture_provision_catalog", &[]);
    select(&node, &guid, "cohort");
    node.assert_call("playerbots_fixture_provision_steps", &[&guid, "64"]);
    assert_eq!(item_count(&node, &guid, 159), 10);
    assert_eq!(item_count(&node, &guid, 17029), 5);
    node.assert_call("playerbots_fixture_provision_reset", &[&guid]);
    node.assert_call("playerbots_fixture_prepare", &[]);
    node.assert_call("playerbots_fixture_runner_stage", &[&guid, "true"]);
    node.assert_call("playerbots_fixture_position", &[&guid, "1240"]);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let urgent = one(
        &node,
        &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
    );
    assert!(urgent["chosen"].to_ascii_lowercase().contains("recovery"));
    assert!(!urgent["last_outcome"]
        .to_ascii_lowercase()
        .contains("provisioning"));
    assert!(one(
        &node,
        &format!("SELECT * FROM pkg_playerbots_provisioning WHERE character_guid = {guid}")
    )["history"]
        .is_empty());

    node.assert_call("playerbots_fixture_cancel", &[&guid, "false"]);
    node.assert_call("playerbots_fixture_provision_remove_recovery", &[&guid]);
    let staged_food = item_count(&node, &guid, 117);
    let wounded_health = health(&node, &guid);
    write_evidence(
        &node,
        "priority-staged",
        serde_json::json!({
            "food": staged_food,
            "health": wounded_health,
            "bot": one(&node, &format!("SELECT next_think_micros FROM pkg_playerbots_bot WHERE character_guid = {guid}")),
            "provisioning": one(&node, &format!("SELECT * FROM pkg_playerbots_provisioning WHERE character_guid = {guid}")),
        }),
    );
    assert_eq!(staged_food, 9);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    assert_eq!(item_count(&node, &guid, 117), 10);
    let supplied = one(
        &node,
        &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
    );
    assert!(supplied["last_outcome"]
        .to_ascii_lowercase()
        .contains("provisioning"));
    node.assert_call("playerbots_fixture_provision_due", &[&guid]);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    assert_eq!(item_count(&node, &guid, 117), 9);
    assert!(!node
        .query_rows(&format!(
            "SELECT * FROM game_aura WHERE target_guid = {guid} AND spell_id = 50115"
        ))
        .is_empty());
    assert!(support::poll_until(support::POLL_TIMEOUT, || {
        health(&node, &guid) > wounded_health
    }));
    let history = one(
        &node,
        &format!("SELECT history FROM pkg_playerbots_provisioning WHERE character_guid = {guid}"),
    )["history"]
        .to_ascii_lowercase();
    assert!(history.matches("recoverhealth").count() >= 2);
    assert!(history.matches("applied").count() >= 2);
    write_evidence(
        &node,
        "priority",
        serde_json::json!({"urgent_runner": urgent, "supplied_runner": supplied, "history": history}),
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_provisioning_stops_cleanly_and_preserves_owned_items_and_gear() {
    let (node, guid) = fixture("playerbots-provisioning-stops", "1", "0");
    select(&node, &guid, "cohort");
    node.assert_call("playerbots_fixture_provision_full_bag", &[&guid]);
    let before = gameplay(&node, &guid);
    node.assert_call("playerbots_fixture_provision_steps", &[&guid, "1"]);
    assert_eq!(item_count(&node, &guid, 5_090_150), 1);
    assert_eq!(item_count(&node, &guid, 117), 0);
    let stopped = one(
        &node,
        &format!("SELECT history FROM pkg_playerbots_provisioning WHERE character_guid = {guid}"),
    )["history"]
        .to_ascii_lowercase();
    assert!(stopped.contains("stopped"));
    assert!(stopped.contains("inventoryfull"));
    let history_before = stopped.clone();
    node.assert_call("playerbots_fixture_runner_due", &[]);
    node.assert_call("playerbots_fixture_runner_pass", &[]);
    assert_eq!(
        one(
            &node,
            &format!(
                "SELECT history FROM pkg_playerbots_provisioning WHERE character_guid = {guid}"
            )
        )["history"]
            .to_ascii_lowercase(),
        history_before
    );
    assert!(!one(
        &node,
        &format!("SELECT last_outcome FROM pkg_playerbots_runner WHERE character_guid = {guid}")
    )["last_outcome"]
        .to_ascii_lowercase()
        .contains("provisioning"));
    let after_useful_tick = gameplay(&node, &guid);
    for field in ["character", "items", "quests", "spells", "talents"] {
        assert_eq!(
            after_useful_tick[field], before[field],
            "gameplay field {field}"
        );
    }

    node.assert_call("playerbots_fixture_provision_missing_resource", &[&guid]);
    node.assert_call("playerbots_fixture_provision_steps", &[&guid, "1"]);
    let missing = one(
        &node,
        &format!("SELECT history FROM pkg_playerbots_provisioning WHERE character_guid = {guid}"),
    )["history"]
        .to_ascii_lowercase();
    assert!(missing.contains("refused"));
    assert!(missing.contains("missingresource"));
    node.assert_call("playerbots_fixture_provision_missing_spell", &[&guid]);
    node.assert_call("playerbots_fixture_provision_steps", &[&guid, "1"]);
    let missing_spell = one(
        &node,
        &format!("SELECT history FROM pkg_playerbots_provisioning WHERE character_guid = {guid}"),
    )["history"]
        .to_ascii_lowercase();
    assert!(missing_spell.contains("spell"));
    assert!(missing_spell.contains("7386"));
    assert!(missing_spell.contains("refused"));
    assert!(missing_spell.contains("missingresource"));
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM game_player_spell WHERE character_guid = {guid} AND spell_id = 7386"
        ))
        .is_empty());
    node.assert_call("playerbots_fixture_provision_wrong_class_spell", &[&guid]);
    node.assert_call("playerbots_fixture_provision_steps", &[&guid, "1"]);
    let wrong_class = one(
        &node,
        &format!("SELECT history FROM pkg_playerbots_provisioning WHERE character_guid = {guid}"),
    )["history"]
        .to_ascii_lowercase();
    assert!(wrong_class.contains("refused"));
    assert!(wrong_class.contains("class"));
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM game_player_spell WHERE character_guid = {guid} AND spell_id = 133"
        ))
        .is_empty());
    node.assert_call("playerbots_fixture_provision_profile_overflow", &[&guid]);
    node.assert_call("playerbots_fixture_provision_steps", &[&guid, "1"]);
    let overflow = one(
        &node,
        &format!("SELECT * FROM pkg_playerbots_provisioning WHERE character_guid = {guid}"),
    )["history"]
        .to_ascii_lowercase();
    assert!(overflow.contains("stopped"));
    assert!(overflow.contains("profilelimit"));
    node.assert_call("playerbots_fixture_provision_dead", &[&guid]);
    node.assert_call("playerbots_fixture_provision_steps", &[&guid, "1"]);
    let dead = one(
        &node,
        &format!("SELECT history FROM pkg_playerbots_provisioning WHERE character_guid = {guid}"),
    )["history"]
        .to_ascii_lowercase();
    assert!(dead.contains("deferred"));
    assert!(dead.contains("dead"));

    let (gear_node, gear_guid) = fixture("playerbots-provisioning-gear", "1", "0");
    select(&gear_node, &gear_guid, "cohort");
    gear_node.assert_call(
        "playerbots_fixture_provision_stronger_weapon",
        &[&gear_guid],
    );
    gear_node.assert_call("playerbots_fixture_provision_steps", &[&gear_guid, "1"]);
    let mainhand = one(
        &gear_node,
        &format!(
            "SELECT entry FROM game_item_instance WHERE owner_guid = {gear_guid} AND slot = 15"
        ),
    );
    assert_eq!(mainhand["entry"], "50");
    write_evidence(
        &node,
        "stops",
        serde_json::json!({"stopped": stopped, "missing_item": missing, "missing_spell": missing_spell, "wrong_class": wrong_class, "overflow": overflow, "dead": dead, "gameplay": gameplay(&node, &guid)}),
    );
    write_evidence(
        &gear_node,
        "gear",
        serde_json::json!({"mainhand": mainhand, "gameplay": gameplay(&gear_node, &gear_guid)}),
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_provisioning_arms_levelup_but_control_modes_block_gameplay() {
    let (node, guid) = fixture("playerbots-provisioning-control", "1", "0");
    let initial = gameplay(&node, &guid);
    for controller in ["legacy", "recordOnly", "frozen"] {
        select(&node, &guid, controller);
        node.assert_call("playerbots_fixture_provision_steps", &[&guid, "4"]);
        assert_eq!(gameplay(&node, &guid), initial, "{controller}");
    }
    select(&node, &guid, "recordOnly");
    node.assert_call("playerbots_fixture_provision_levelup", &[&guid]);
    let armed = one(
        &node,
        &format!("SELECT * FROM pkg_playerbots_provisioning WHERE character_guid = {guid}"),
    );
    assert!(armed["cause"].to_ascii_lowercase().contains("levelup"));
    assert!(armed["armed_level"].parse::<u32>().unwrap() > 9);
    assert!(armed["history"].is_empty());
    let levelled = gameplay(&node, &guid);
    node.assert_call("playerbots_fixture_provision_steps", &[&guid, "4"]);
    assert_eq!(gameplay(&node, &guid), levelled);
    write_evidence(
        &node,
        "control",
        serde_json::json!({"armed": armed, "gameplay": levelled}),
    );
}

fn talent_state(node: &Standalone, guid: &str) -> serde_json::Value {
    serde_json::json!({
        "talents": node.query_rows(&format!("SELECT talent_id, rank FROM game_character_talent WHERE character_guid = {guid}")),
        "spells": node.query_rows(&format!("SELECT spell_id FROM game_player_spell WHERE character_guid = {guid}")),
        "auras": node.query_rows(&format!("SELECT spell_id, effect_id FROM game_aura WHERE target_guid = {guid}")),
    })
}

fn assert_talent_refused_unchanged(node: &Standalone, guid: &str, talent: &str) {
    let before = talent_state(node, guid);
    let output = node.call("debug_learn_talent", &[guid, talent]);
    assert!(
        !output.status.success(),
        "talent {talent} unexpectedly learned"
    );
    assert_eq!(talent_state(node, guid), before, "talent {talent}");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_talent_gates_are_atomic_and_use_global_tab_identity() {
    let (node, mage) = fixture("playerbots-provisioning-talents", "8", "2");
    node.assert_call("debug_set_level", &[&mage, "20"]);
    assert_talent_refused_unchanged(&node, &mage, "1");

    node.assert_call(
        "playerbots_spawn_class_role",
        &["1", "1200", "1200", "50", "1", "0"],
    );
    let warrior = node
        .query_rows("SELECT character_guid FROM pkg_playerbots_bot")
        .into_iter()
        .map(|row| row["character_guid"].clone())
        .find(|guid| guid != &mage)
        .unwrap();
    node.assert_call("debug_set_level", &[&warrior, "20"]);
    for (tab, class_mask, race_mask) in [(9001, 128, 0), (9002, 1, 0), (9003, 1, 0), (9004, 1, 2)] {
        node.assert_sql(&format!(
            "INSERT INTO game_talent_tab (tab_id,name,class_mask,race_mask,order_index) VALUES ({tab},'PB005 tab',{class_mask},{race_mask},0)"
        ));
    }
    assert_talent_refused_unchanged(&node, &warrior, "1");

    let insert_talent = |id: u32,
                         tab: u32,
                         max_rank: u8,
                         required_points: u32,
                         required_spell: u32| {
        node.assert_sql(&format!(
            "INSERT INTO game_talent (talent_id,name,tree_id,tier,column,max_rank,spell_id,required_talent_id,required_points_in_tree,grant_spell_id,tab_id,rank_spell_2,rank_spell_3,rank_spell_4,rank_spell_5,required_talent_rank,required_spell_id) VALUES ({id},'PB005 talent',0,0,0,{max_rank},0,0,{required_points},0,{tab},0,0,0,0,0,{required_spell})"
        ));
    };
    insert_talent(9001, 9001, 1, 0, 0);
    assert_talent_refused_unchanged(&node, &warrior, "9001");
    insert_talent(9004, 9004, 1, 0, 0);
    assert_talent_refused_unchanged(&node, &warrior, "9004");
    insert_talent(9002, 9002, 1, 0, 999_999);
    assert_talent_refused_unchanged(&node, &warrior, "9002");

    insert_talent(9010, 9002, 5, 0, 0);
    insert_talent(9020, 9003, 1, 5, 0);
    for _ in 0..5 {
        node.assert_call("debug_learn_talent", &[&warrior, "9010"]);
    }
    assert_talent_refused_unchanged(&node, &warrior, "9020");
    write_evidence(
        &node,
        "talent-gates",
        serde_json::json!({"mage": talent_state(&node, &mage), "warrior": talent_state(&node, &warrior)}),
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_provisioning_stops_on_partial_talent_catalog_without_character_changes() {
    let (node, guid) = fixture("playerbots-provisioning-partial-talents", "1", "0");
    node.assert_call("debug_set_level", &[&guid, "20"]);
    assert!(node.query_rows("SELECT * FROM game_talent_tab").is_empty());
    node.assert_call("playerbots_fixture_provision_partial_talent_catalog", &[]);
    let raw_tree_rows = node
        .query_rows("SELECT talent_id FROM game_talent WHERE tree_id = 2")
        .len();
    assert!(raw_tree_rows > 64);
    select(&node, &guid, "cohort");
    let before = talent_state(&node, &guid);

    node.assert_call("playerbots_fixture_provision_steps", &[&guid, "1"]);
    let provisioning = one(
        &node,
        &format!("SELECT * FROM pkg_playerbots_provisioning WHERE character_guid = {guid}"),
    );
    let history = provisioning["history"].to_ascii_lowercase();
    assert!(history.contains("stopped"), "{history}");
    assert!(history.contains("profilelimit"), "{history}");
    assert!(history.contains("raw rows"), "{history}");
    assert_eq!(talent_state(&node, &guid), before);
    write_evidence(
        &node,
        "partial-talent-catalog",
        serde_json::json!({
            "fixture_kind": "source-derived partial Talent import",
            "raw_tree_rows": raw_tree_rows,
            "provisioning": provisioning,
            "character_talent_state": before,
        }),
    );
}

#[test]
#[ignore = "requires the integrated PB-002 Wasm, SpacetimeDB, and the playerbots Package"]
fn playerbots_provisioning_migrates_populated_pb002_runner_state() {
    let old_path = std::env::var_os("PLAYERBOTS_PB002_WASM")
        .expect("PLAYERBOTS_PB002_WASM must name the integrated PB-002 Wasm");
    let old_wasm = std::fs::read(old_path).unwrap();
    assert_ne!(
        blake3::hash(&old_wasm),
        blake3::hash(support::module_bytes())
    );
    let mut node = Standalone::start("playerbots-provisioning-pb002-upgrade");
    node.publish_module_bytes(&old_wasm);
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("playerbots_spawn_role", &["1", "1200", "1200", "50", "1"]);
    let guid = node.query_rows("SELECT character_guid FROM pkg_playerbots_bot")[0]
        ["character_guid"]
        .clone();
    node.assert_call("playerbots_fixture_runner_stage", &[&guid, "false"]);
    select(&node, &guid, "cohort");
    node.assert_call("playerbots_fixture_runner_due", &[]);
    node.assert_call("playerbots_fixture_runner_pass", &[]);
    assert!(support::poll_until(support::POLL_TIMEOUT, || {
        one(
            &node,
            &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
        )["foreground"]
            .to_ascii_lowercase()
            .contains("movement")
    }));
    node.assert_call("playerbots_fixture_freeze", &[&guid]);
    let preceding = one(
        &node,
        &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
    );
    assert!(preceding["objective"]
        .to_ascii_lowercase()
        .contains("returnhome"));
    assert!(preceding["foreground"]
        .to_ascii_lowercase()
        .contains("movement"));

    node.publish_module();
    let upgraded = one(
        &node,
        &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
    );
    for field in ["objective_sequence", "objective", "foreground", "chosen"] {
        assert_eq!(upgraded[field], preceding[field], "runner field {field}");
    }
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM pkg_playerbots_provisioning WHERE character_guid = {guid}"
        ))
        .is_empty());
    node.assert_call("playerbots_fixture_runner_due", &[]);
    node.assert_call("playerbots_fixture_runner_pass", &[]);
    write_evidence(
        &node,
        "pb002-upgrade",
        serde_json::json!({
            "preceding_core": PB002_CORE,
            "preceding_package": PB002_PACKAGE,
            "preceding_wasm_identity": blake3::hash(&old_wasm).to_hex().to_string(),
            "current_wasm_identity": blake3::hash(support::module_bytes()).to_hex().to_string(),
            "preceding_runner": preceding,
            "upgraded_runner": upgraded,
            "provisioning": node.query_rows(&format!("SELECT * FROM pkg_playerbots_provisioning WHERE character_guid = {guid}")),
        }),
    );
}
