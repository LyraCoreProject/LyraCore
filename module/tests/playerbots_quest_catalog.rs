//! Supported quest admission and executor evidence on private Standalone databases.

mod support;

use std::collections::BTreeMap;
use support::Standalone;

const CREATURE_197: &str = "17379390965327855617";
const CREATURE_6: &str = "17379390962123407361";
const CREATURE_952: &str = "17379390977994653697";
const GAMEOBJECT_55: &str = "17370383762768003127";
const GAMEOBJECT_56: &str = "17370383762768003128";
const GAMEOBJECT_161557: &str = "17370383762768164629";

fn remove_builtin_weather_import_stamp(node: &Standalone) {
    let imports =
        node.query_rows("SELECT family, source_sha, file_hash, row_count FROM game_import_meta");
    assert_eq!(imports.len(), 1, "unexpected init Import Catalogue");
    assert_eq!(imports[0]["family"], "weather_seed");
    assert_eq!(imports[0]["source_sha"], "");
    assert_eq!(imports[0]["file_hash"], "");
    assert_eq!(imports[0]["row_count"], "2");

    // This disposable Standalone removes only its verified synthetic init stamp. Any later import
    // remains visible to the Package Gate and must make the quest fixture refuse.
    node.assert_sql(
        "DELETE FROM game_import_meta WHERE family = 'weather_seed' AND source_sha = '' AND file_hash = '' AND row_count = 2",
    );
    assert!(node.query_rows("SELECT * FROM game_import_meta").is_empty());
}

fn unstaged_fixture(name: &str) -> (Standalone, Vec<BTreeMap<String, String>>) {
    let mut node = Standalone::start(name);
    node.publish_module();
    remove_builtin_weather_import_stamp(&node);
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    for (class, role) in [("1", "0"), ("5", "1"), ("8", "2")] {
        node.assert_call(
            "playerbots_spawn_class_role",
            &["1", "1200", "1200", "50", class, role],
        );
    }
    let mut bots = node.query_rows("SELECT character_guid, class FROM pkg_playerbots_bot");
    bots.sort_by_key(|bot| bot["class"].parse::<u8>().unwrap());
    for bot in &bots {
        node.assert_call(
            "playerbots_select_controller",
            &[&bot["character_guid"], "{\"frozen\":[]}"],
        );
    }
    (node, bots)
}

fn fixture(name: &str) -> (Standalone, Vec<BTreeMap<String, String>>) {
    let (node, bots) = unstaged_fixture(name);
    node.assert_call(
        "playerbots_quest_fixture_stage",
        &[&bots[0]["character_guid"]],
    );
    record(&node, "inputs");
    (node, bots)
}

fn record(node: &Standalone, suffix: &str) {
    let core = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let package = core.join("packages/playerbots");
    let git = |path: &std::path::Path, args: &[&str]| {
        let output = std::process::Command::new("git")
            .current_dir(path)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_string()
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
    let mut package_digest = blake3::Hasher::new();
    digest_files(&package, &mut package_digest);
    let fixture: serde_json::Value = serde_json::from_slice(
        &std::fs::read(package.join("fixtures/quest-catalog.json")).unwrap(),
    )
    .unwrap();
    let result = serde_json::json!({
        "fixture": fixture,
        "tested_core": git(core, &["rev-parse", "HEAD"]),
        "tested_collection": git(&package, &["rev-parse", "HEAD"]),
        "core_dirty": !git(core, &["status", "--porcelain"]).is_empty(),
        "collection_dirty": !git(&package, &["status", "--porcelain"]).is_empty(),
        "package_content_identity": package_digest.finalize().to_hex().to_string(),
        "module_wasm_blake3": blake3::hash(support::module_bytes()).to_hex().to_string(),
        "catalog": node.query_rows("SELECT * FROM pkg_playerbots_quest_catalog"),
        "starter_seeds": node.query_rows("SELECT * FROM pkg_playerbots_catalog_seed"),
        "quests": node.query_rows("SELECT * FROM pkg_playerbots_catalog_quest"),
        "objectives": node.query_rows("SELECT * FROM pkg_playerbots_catalog_objective"),
        "admission": node.query_rows("SELECT * FROM pkg_playerbots_quest_admission"),
        "retained": node.query_rows("SELECT * FROM pkg_playerbots_quest_objective"),
        "provisioning": node.query_rows("SELECT * FROM pkg_playerbots_provisioning"),
        "actions": node.query_rows("SELECT * FROM pkg_playerbots_action"),
        "character_quests": node.query_rows("SELECT * FROM game_character_quest"),
        "items": node.query_rows("SELECT * FROM game_item_instance"),
        "loot": node.query_rows("SELECT * FROM game_corpse_loot"),
        "import_catalogue": node.query_rows("SELECT * FROM game_import_meta"),
        "fixture_ownership": node.query_rows("SELECT * FROM pkg_playerbots_quest_fixture_ownership"),
    });
    let path = support::log_dir().join(format!("{}-{suffix}.json", node.shard_name()));
    std::fs::write(path, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
}

fn bot_for_class<'a>(bots: &'a [BTreeMap<String, String>], class: &str) -> &'a str {
    &bots.iter().find(|bot| bot["class"] == class).unwrap()["character_guid"]
}

fn quest(node: &Standalone, bot: &str, entry: u32) -> BTreeMap<String, String> {
    node.query_rows(&format!(
        "SELECT * FROM game_character_quest WHERE character_guid = {bot} AND quest_entry = {entry}"
    ))
    .into_iter()
    .next()
    .expect("quest row missing")
}

fn item_count(node: &Standalone, bot: &str, entry: u32) -> u32 {
    node.query_rows(&format!(
        "SELECT stack_count FROM game_item_instance WHERE owner_guid = {bot} AND entry = {entry}"
    ))
    .iter()
    .map(|row| row["stack_count"].parse::<u32>().unwrap())
    .sum()
}

fn sorted_rows(node: &Standalone, query: &str, key: &str) -> Vec<BTreeMap<String, String>> {
    let mut rows = node.query_rows(query);
    rows.sort_by_key(|row| row[key].parse::<u64>().unwrap());
    rows
}

fn catalog_definition_snapshot(node: &Standalone) -> serde_json::Value {
    serde_json::json!({
        "item": node.query_rows("SELECT * FROM game_item_template WHERE entry = 750"),
        "creature": node.query_rows("SELECT * FROM game_creature_template WHERE entry = 823"),
        "creature_spawn": node.query_rows("SELECT * FROM game_creature_spawn WHERE entry = 823"),
        "gameobject": node.query_rows("SELECT * FROM game_gameobject_template WHERE entry = 55"),
        "gameobject_spawn": node.query_rows("SELECT * FROM game_gameobject WHERE template_entry = 161557"),
        "quest": node.query_rows("SELECT * FROM game_quest_template WHERE entry = 783"),
        "creature_relations": sorted_rows(node, "SELECT * FROM game_creature_quest WHERE creature_entry = 823", "id"),
        "gameobject_relations": sorted_rows(node, "SELECT * FROM game_gameobject_quest WHERE go_entry = 55", "id"),
        "objectives": sorted_rows(node, "SELECT * FROM game_quest_objective WHERE quest_entry = 7", "id"),
        "creature_loot": sorted_rows(node, "SELECT * FROM game_creature_loot WHERE creature_entry = 299", "id"),
        "gameobject_loot": sorted_rows(node, "SELECT * FROM game_gameobject_loot WHERE loot_id = 10119", "id"),
    })
}

fn select_cohort(node: &Standalone, bot: &str) {
    node.assert_call("playerbots_select_controller", &[bot, "{\"cohort\":[]}"]);
}

fn run_once(node: &Standalone) {
    node.assert_call("playerbots_fixture_runner_due", &[]);
    node.assert_call("playerbots_fixture_runner_pass", &[]);
}

fn runner(node: &Standalone, bot: &str) -> BTreeMap<String, String> {
    node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_runner WHERE character_guid = {bot}"
    ))
    .into_iter()
    .next()
    .expect("runner row missing")
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_catalog_is_named_versioned_and_shared_by_starter_classes() {
    let (node, bots) = unstaged_fixture("playerbots-quest-catalog");
    node.assert_sql(
        "INSERT INTO game_creature_quest (id,creature_entry,quest_entry,role) VALUES (1,5099997,5099997,0)",
    );
    node.assert_call(
        "playerbots_quest_fixture_stage",
        &[&bots[0]["character_guid"]],
    );
    record(&node, "inputs");
    let explicit_relation = node.query_rows("SELECT * FROM game_creature_quest WHERE id = 1");
    assert_eq!(explicit_relation.len(), 1);
    assert_eq!(
        bots.iter()
            .map(|bot| bot["class"].as_str())
            .collect::<Vec<_>>(),
        ["1", "5", "8"]
    );
    let header = node.query_rows("SELECT * FROM pkg_playerbots_quest_catalog");
    assert_eq!(header.len(), 1);
    assert_eq!(header[0]["name"], "northshire-elwynn-supported-v1");
    assert_eq!(header[0]["revision"], "1");
    assert_eq!(header[0]["quest_count"], "12");
    assert_eq!(header[0]["reference_source_revision"], "unknown");
    assert!(header[0]["blueprint_revision"].contains("d2083bcd"));
    let mut seeds = node.query_rows(
        "SELECT class, fixture_seed, catalog_revision, quest_order FROM pkg_playerbots_catalog_seed",
    );
    seeds.sort_by_key(|row| row["class"].parse::<u8>().unwrap());
    assert_eq!(
        seeds
            .iter()
            .map(|row| (row["class"].as_str(), row["fixture_seed"].as_str()))
            .collect::<Vec<_>>(),
        [("1", "783001"), ("5", "783005"), ("8", "783008")]
    );
    assert!(seeds
        .iter()
        .all(|row| row["catalog_revision"] == "1" && row["quest_order"].contains("3904")));
    assert_eq!(
        node.query_rows("SELECT quest_entry FROM pkg_playerbots_catalog_quest")
            .len(),
        12
    );
    let harvest = node.query_rows(
        "SELECT kind, target_entry, executor, source_entries, source_destinations, work_area, destination_evidence_revision FROM pkg_playerbots_catalog_objective WHERE quest_entry = 3904",
    );
    assert_eq!(harvest.len(), 1);
    assert!(harvest[0]["kind"].contains("collectItem"));
    assert!(harvest[0]["executor"].contains("gameObjectLoot"));
    assert_eq!(harvest[0]["target_entry"], "11119");
    assert!(harvest[0]["source_entries"].contains("161557"));
    assert!(harvest[0]["source_destinations"].contains(GAMEOBJECT_161557));
    assert!(!harvest[0]["work_area"].contains("none"));
    assert!(harvest[0]["destination_evidence_revision"].starts_with("observed-catalog-v1:"));
    let provided = node.query_rows(
        "SELECT kind, target_entry, executor, source_entries FROM pkg_playerbots_catalog_objective WHERE quest_entry = 3905",
    );
    assert!(provided[0]["kind"].contains("collectItem"));
    assert_eq!(provided[0]["target_entry"], "11125");
    assert!(provided[0]["executor"].contains("providedItem"));
    assert!(provided[0]["source_entries"].is_empty());
    let before = harvest[0]["destination_evidence_revision"].clone();
    node.assert_call("playerbots_quest_fixture_recheck_unchanged", &[]);
    let unchanged = node.query_rows(
        "SELECT content_revision, refresh_after_micros FROM pkg_playerbots_quest_catalog",
    );
    assert_eq!(unchanged[0]["content_revision"], before);
    assert!(unchanged[0]["refresh_after_micros"].parse::<i64>().unwrap() > 0);
    node.assert_call(
        "playerbots_quest_fixture_move_gameobject",
        &["161557", "1210"],
    );
    node.assert_call("playerbots_quest_fixture_refresh", &[]);
    let after = node.query_rows(
        "SELECT destination_evidence_revision FROM pkg_playerbots_catalog_objective WHERE quest_entry = 3904",
    );
    assert_ne!(after[0]["destination_evidence_revision"], before);
    for query in [
        "SELECT id FROM game_creature_quest WHERE creature_entry = 823",
        "SELECT id FROM game_gameobject_quest WHERE go_entry = 55",
        "SELECT id FROM game_quest_objective WHERE quest_entry = 7",
        "SELECT id FROM game_creature_loot WHERE creature_entry = 299",
        "SELECT id FROM game_gameobject_loot WHERE loot_id = 10119",
    ] {
        assert!(node.query_rows(query).iter().all(|row| {
            let id = row["id"].parse::<u64>().unwrap();
            (5_099_000..=5_099_999).contains(&id)
        }));
    }
    let staged = catalog_definition_snapshot(&node);
    node.assert_call(
        "playerbots_quest_fixture_stage",
        &[&bots[0]["character_guid"]],
    );
    assert_eq!(catalog_definition_snapshot(&node), staged);
    assert_eq!(
        node.query_rows("SELECT * FROM game_creature_quest WHERE id = 1"),
        explicit_relation
    );
    node.assert_call(
        "stamp_import_meta",
        &[
            "unrelated-fixture-import",
            "fixture-source",
            "fixture-hash",
            "1",
        ],
    );
    let refused = node.call(
        "playerbots_quest_fixture_stage",
        &[&bots[0]["character_guid"]],
    );
    assert!(
        !refused.status.success(),
        "imported stage unexpectedly succeeded"
    );
    assert_eq!(catalog_definition_snapshot(&node), staged);
    assert_eq!(
        node.query_rows("SELECT * FROM game_creature_quest WHERE id = 1"),
        explicit_relation
    );
    let refused = node.call(
        "playerbots_quest_fixture_move_gameobject",
        &["161557", "1220"],
    );
    assert!(
        !refused.status.success(),
        "fixture operation accepted imported content"
    );
    assert_eq!(catalog_definition_snapshot(&node), staged);
    record(&node, "catalog");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_catalog_fixture_refuses_preexisting_semantic_rows_without_mutation() {
    let (conflict, bots) = unstaged_fixture("playerbots-quest-catalog-conflict");
    conflict.assert_sql(
        "INSERT INTO game_creature_quest (id,creature_entry,quest_entry,role) VALUES (1,823,783,0)",
    );
    let before = conflict.query_rows("SELECT * FROM game_creature_quest WHERE id = 1");
    let character = conflict.query_rows(&format!(
        "SELECT * FROM game_world_entity WHERE guid = {}",
        bots[0]["character_guid"]
    ));
    let roster = conflict.query_rows("SELECT * FROM pkg_playerbots_bot");
    let refused = conflict.call(
        "playerbots_quest_fixture_stage",
        &[&bots[0]["character_guid"]],
    );
    assert!(
        !refused.status.success(),
        "pre-existing semantic relation was replaced"
    );
    assert_eq!(
        conflict.query_rows("SELECT * FROM game_creature_quest WHERE id = 1"),
        before
    );
    assert_eq!(
        conflict.query_rows(&format!(
            "SELECT * FROM game_world_entity WHERE guid = {}",
            bots[0]["character_guid"]
        )),
        character
    );
    assert_eq!(
        conflict.query_rows("SELECT * FROM pkg_playerbots_bot"),
        roster
    );
    assert!(conflict
        .query_rows("SELECT * FROM pkg_playerbots_quest_fixture_ownership")
        .is_empty());
    assert!(conflict
        .query_rows("SELECT * FROM game_item_template WHERE entry = 750")
        .is_empty());
    assert!(conflict
        .query_rows("SELECT * FROM game_creature_template WHERE entry = 823")
        .is_empty());
    assert!(conflict
        .query_rows("SELECT * FROM game_gameobject_template WHERE entry = 55")
        .is_empty());
    assert!(conflict
        .query_rows("SELECT * FROM game_quest_template WHERE entry = 783")
        .is_empty());
    assert!(conflict
        .query_rows("SELECT * FROM game_spell WHERE spell_id = 5090100")
        .is_empty());
    record(&conflict, "preexisting-refusal");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_talk_kill_creature_drop_and_provided_item_paths_change_core_state() {
    let (node, bots) = fixture("playerbots-quest-creature-paths");
    let warrior = bot_for_class(&bots, "1");
    node.assert_call("playerbots_quest_fixture_admit_accept", &[warrior, "783"]);
    let admitted = node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_quest_admission WHERE character_guid = {warrior}"
    ));
    assert!(admitted[0]["state"].contains("admitted"));
    node.assert_call("playerbots_quest_fixture_turn_in", &[warrior, "783"]);
    assert_eq!(quest(&node, warrior, 783)["rewarded"], "true");
    let turn_in = node.query_rows(&format!(
        "SELECT target_guid, outcome FROM pkg_playerbots_action WHERE character_guid = {warrior} AND quest_entry = 783"
    ));
    assert!(turn_in.iter().any(|row| row["target_guid"] == CREATURE_197));

    node.assert_call("playerbots_quest_fixture_admit_accept", &[warrior, "37"]);
    node.assert_call("playerbots_quest_fixture_turn_in", &[warrior, "37"]);
    assert!(node
        .query_rows(&format!(
            "SELECT target_guid FROM pkg_playerbots_action WHERE character_guid = {warrior} AND quest_entry = 37"
        ))
        .iter()
        .any(|row| row["target_guid"] == GAMEOBJECT_55));
    node.assert_call("playerbots_quest_fixture_admit_accept", &[warrior, "45"]);
    assert!(node
        .query_rows(&format!(
            "SELECT target_guid FROM pkg_playerbots_action WHERE character_guid = {warrior} AND quest_entry = 45"
        ))
        .iter()
        .any(|row| row["target_guid"] == GAMEOBJECT_55));
    node.assert_call("playerbots_quest_fixture_turn_in", &[warrior, "45"]);
    assert!(node
        .query_rows(&format!(
            "SELECT target_guid FROM pkg_playerbots_action WHERE character_guid = {warrior} AND quest_entry = 45"
        ))
        .iter()
        .any(|row| row["target_guid"] == GAMEOBJECT_56));

    node.assert_call("playerbots_quest_fixture_admit_accept", &[warrior, "7"]);
    node.assert_call("playerbots_quest_fixture_kill", &[warrior, "6"]);
    assert!(quest(&node, warrior, 7)["counts"].contains('1'));
    assert!(node
        .query_rows(&format!(
            "SELECT outcome FROM pkg_playerbots_action WHERE character_guid = {warrior}"
        ))
        .iter()
        .any(|row| row["outcome"].contains("attackAccepted")));

    let priest = bot_for_class(&bots, "5");
    node.assert_call("playerbots_quest_fixture_admit_accept", &[priest, "33"]);
    node.assert_call("playerbots_quest_fixture_kill", &[priest, "299"]);
    node.assert_call(
        "playerbots_quest_fixture_take_creature_loot",
        &[priest, "299"],
    );
    assert_eq!(item_count(&node, priest, 750), 8);
    let loot_actions = node.query_rows(&format!(
        "SELECT kind, outcome FROM pkg_playerbots_action WHERE character_guid = {priest}"
    ));
    assert!(loot_actions
        .iter()
        .any(|row| row["kind"].contains("openLoot") && row["outcome"].contains("completed")));
    assert!(loot_actions
        .iter()
        .any(|row| row["kind"].contains("takeLoot") && row["outcome"].contains("completed")));
    node.assert_call("playerbots_quest_fixture_turn_in", &[priest, "33"]);
    assert_eq!(quest(&node, priest, 33)["rewarded"], "true");
    assert_eq!(item_count(&node, priest, 750), 0);

    node.assert_call("playerbots_quest_fixture_admit_accept", &[warrior, "3905"]);
    assert_eq!(item_count(&node, warrior, 11125), 1);
    node.assert_call("playerbots_quest_fixture_turn_in", &[warrior, "3905"]);
    assert_eq!(quest(&node, warrior, 3905)["rewarded"], "true");
    assert_eq!(item_count(&node, warrior, 11125), 0);
    assert!(node
        .query_rows(&format!(
            "SELECT target_guid FROM pkg_playerbots_action WHERE character_guid = {warrior} AND quest_entry = 3905"
        ))
        .iter()
        .any(|row| row["target_guid"] == CREATURE_952));
    record(&node, "creature-paths");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_gameobject_loot_and_simple_use_paths_change_core_state() {
    let (node, bots) = fixture("playerbots-quest-gameobject-loot");
    let mage = bot_for_class(&bots, "8");
    node.assert_call("playerbots_quest_fixture_admit_accept", &[mage, "3904"]);
    node.assert_call(
        "playerbots_quest_fixture_move_gameobject",
        &["161557", "1300"],
    );
    node.assert_call(
        "playerbots_quest_fixture_try_use_gameobject",
        &[mage, "161557"],
    );
    let refusal = node.query_rows(&format!(
        "SELECT outcome FROM pkg_playerbots_action WHERE character_guid = {mage}"
    ));
    assert!(refusal
        .iter()
        .any(|row| row["outcome"].contains("outOfRange")));
    assert_eq!(
        node.query_rows(&format!(
            "SELECT state FROM game_gameobject WHERE guid = {GAMEOBJECT_161557}"
        ))[0]["state"],
        "0"
    );
    record(&node, "gameobject-refusal");
    node.assert_call(
        "playerbots_quest_fixture_move_gameobject",
        &["161557", "1202"],
    );
    node.assert_call(
        "playerbots_quest_fixture_use_gameobject",
        &[mage, "161557", "false"],
    );
    node.assert_call("playerbots_quest_fixture_fill_inventory", &[mage]);
    let inventory_query = format!("SELECT * FROM game_item_instance WHERE owner_guid = {mage}");
    let loot_query =
        format!("SELECT * FROM game_corpse_loot WHERE corpse_guid = {GAMEOBJECT_161557}");
    let inventory_before = sorted_rows(&node, &inventory_query, "guid");
    let loot_before = sorted_rows(&node, &loot_query, "id");
    assert!(!loot_before.is_empty());
    node.assert_call("playerbots_quest_fixture_try_take_gameobject_loot", &[mage]);
    assert_eq!(
        sorted_rows(&node, &inventory_query, "guid"),
        inventory_before
    );
    assert_eq!(sorted_rows(&node, &loot_query, "id"), loot_before);
    let take_refusal = node.query_rows(&format!(
        "SELECT outcome FROM pkg_playerbots_action WHERE character_guid = {mage}"
    ));
    assert!(take_refusal
        .iter()
        .any(|row| row["outcome"].contains("inventoryFull")));
    record(&node, "gameobject-inventory-refusal");
    node.assert_call(
        "playerbots_quest_fixture_clear_filler_and_take_gameobject_loot",
        &[mage],
    );
    assert_eq!(item_count(&node, mage, 11119), 8);
    assert_eq!(
        node.query_rows(&format!(
            "SELECT state FROM game_gameobject WHERE guid = {GAMEOBJECT_161557}"
        ))[0]["state"],
        "1"
    );
    node.assert_call("playerbots_quest_fixture_turn_in", &[mage, "3904"]);
    assert_eq!(quest(&node, mage, 3904)["rewarded"], "true");
    record(&node, "gameobject-loot");

    let (node, bots) = fixture("playerbots-quest-gameobject-use");
    let mage = bot_for_class(&bots, "8");
    node.assert_call("playerbots_quest_fixture_direct_gameobject", &[mage]);
    node.assert_call(
        "playerbots_quest_fixture_use_gameobject",
        &[mage, "161557", "false"],
    );
    assert!(quest(&node, mage, 3904)["counts"].contains('1'));
    assert!(node
        .query_rows(&format!(
            "SELECT outcome FROM pkg_playerbots_action WHERE character_guid = {mage}"
        ))
        .iter()
        .any(|row| row["outcome"].contains("completed")));
    record(&node, "gameobject-use");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_admission_refuses_each_unsupported_requirement_and_mixed_work() {
    let expected = [
        "exploration",
        "escort",
        "scriptedEvent",
        "transport",
        "complexGameObject",
    ];
    let (node, bots) = fixture("playerbots-quest-unsupported");
    let bot = bot_for_class(&bots, "1");
    node.assert_call(
        "playerbots_quest_fixture_level_admission",
        &[bot, "1", "18"],
    );
    let ineligible = node.query_rows(&format!(
        "SELECT state, detail FROM pkg_playerbots_quest_admission WHERE character_guid = {bot}"
    ));
    assert!(ineligible[0]["state"].contains("ineligible"));
    assert!(ineligible[0]["detail"].contains("requires level 2"));
    node.assert_call(
        "playerbots_quest_fixture_level_admission",
        &[bot, "2", "783"],
    );
    for (kind, expected) in expected.into_iter().enumerate() {
        node.assert_call(
            "playerbots_quest_fixture_unsupported",
            &[bot, &kind.to_string()],
        );
        let admission = node.query_rows(&format!(
            "SELECT state, missing_capability, observed_micros, wait_until_micros FROM pkg_playerbots_quest_admission WHERE character_guid = {bot}"
        ));
        assert!(admission[0]["state"].contains("waiting"));
        assert!(admission[0]["missing_capability"].contains(expected));
        assert!(
            admission[0]["wait_until_micros"].parse::<i64>().unwrap()
                > admission[0]["observed_micros"].parse::<i64>().unwrap()
        );
        assert!(node
            .query_rows(&format!(
            "SELECT id FROM game_character_quest WHERE character_guid = {bot} AND quest_entry = 7"
            ))
            .is_empty());
        node.assert_call("playerbots_quest_fixture_refresh", &[]);
    }
    node.assert_call("playerbots_quest_fixture_mixed_unsupported", &[bot]);
    let admission = node.query_rows(&format!(
        "SELECT missing_capability FROM pkg_playerbots_quest_admission WHERE character_guid = {bot}"
    ));
    assert!(admission[0]["missing_capability"].contains("escort"));

    node.assert_call("playerbots_quest_fixture_mixed_progress", &[bot]);
    node.assert_call("playerbots_fixture_runner_stage", &[bot, "false"]);
    select_cohort(&node, bot);
    run_once(&node);
    let retained = node.query_rows(&format!(
        "SELECT target FROM pkg_playerbots_quest_objective WHERE character_guid = {bot}"
    ));
    assert!(retained[0]["target"].contains("752"));
    record(&node, "unsupported");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_imported_destination_and_live_target_are_separate_facts() {
    let (node, bots) = fixture("playerbots-quest-destination-facts");
    let bot = bot_for_class(&bots, "1");
    node.assert_call("playerbots_quest_fixture_hide_live_target", &["6"]);
    node.assert_call(
        "playerbots_quest_fixture_assert_no_live_target",
        &[bot, "6"],
    );
    node.assert_call("playerbots_quest_fixture_admit_accept", &[bot, "7"]);
    assert_eq!(quest(&node, bot, 7)["rewarded"], "false");
    let objective = node.query_rows(
        "SELECT source_destinations FROM pkg_playerbots_catalog_objective WHERE quest_entry = 7",
    );
    assert!(!objective[0]["source_destinations"].is_empty());
    record(&node, "destination-facts");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_quest_objective_survives_combat_and_refreshes_changed_evidence() {
    let (node, bots) = fixture("playerbots-quest-retention");
    let bot = bot_for_class(&bots, "1");
    node.assert_call("playerbots_quest_fixture_admit_accept", &[bot, "7"]);
    node.assert_call("playerbots_fixture_runner_stage", &[bot, "false"]);
    select_cohort(&node, bot);
    run_once(&node);
    let before_runner = runner(&node, bot);
    assert!(before_runner["objective"].contains("quest"));
    let before_detail = node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {bot}"
    ))[0]
        .clone();
    assert_eq!(before_detail["quest_entry"], "7");
    assert_eq!(before_detail["actual_ender_entry"], "197");
    assert_eq!(before_detail["catalog_revision"], "1");
    assert!(before_detail["destination_evidence_revision"].starts_with("observed-catalog-v1:"));

    node.assert_call("playerbots_fixture_runner_damage", &[bot, CREATURE_6, "1"]);
    run_once(&node);
    let interrupted = runner(&node, bot);
    assert!(interrupted["chosen"].contains("defense"));
    assert!(interrupted["chosen"].contains(CREATURE_6));
    assert_eq!(interrupted["objective"], before_runner["objective"]);
    assert_eq!(
        node.query_rows(&format!(
            "SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {bot}"
        ))[0],
        before_detail
    );

    node.assert_call(
        "playerbots_quest_fixture_move_creature_spawn",
        &["6", "1230"],
    );
    node.assert_call("playerbots_quest_fixture_refresh", &[]);
    run_once(&node);
    let refreshed_runner = runner(&node, bot);
    let refreshed_detail = node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {bot}"
    ))[0]
        .clone();
    assert_ne!(
        refreshed_runner["objective_sequence"],
        before_runner["objective_sequence"]
    );
    assert_ne!(
        refreshed_detail["destination_evidence_revision"],
        before_detail["destination_evidence_revision"]
    );
    assert!(refreshed_detail["destination"].contains("1230"));
    record(&node, "retention");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_companion_control_precedes_held_quest_work() {
    let (node, bots) = fixture("playerbots-quest-companion-control");
    let bot = bot_for_class(&bots, "1");
    let leader = bot_for_class(&bots, "5");
    let ally = bot_for_class(&bots, "8");
    node.assert_call("playerbots_quest_fixture_admit_accept", &[bot, "7"]);
    node.assert_call("playerbots_fixture_runner_select_cohort", &[bot]);
    node.assert_call("playerbots_fixture_runner_pass_once", &[bot]);
    let solo = runner(&node, bot);
    assert!(solo["objective"].contains("quest"), "{solo:?}");
    let held_quest = quest(&node, bot, 7);

    node.assert_call("playerbots_fixture_companion_stage", &[bot, leader, ally]);
    let mut following = None;
    let mut companion_sequence = None;
    for pass in 0..32 {
        node.assert_call("playerbots_fixture_runner_pass_once", &[bot]);
        let state = runner(&node, bot);
        record(&node, &format!("companion-pass-{pass}"));
        assert!(state["objective"].contains("companion"), "{state:?}");
        let sequence =
            companion_sequence.get_or_insert_with(|| state["objective_sequence"].clone());
        assert_eq!(state["objective_sequence"].as_str(), sequence.as_str());
        assert_eq!(quest(&node, bot, 7), held_quest);
        if state["chosen"].contains("follow") {
            following = Some(state);
            break;
        }
        assert!(state["chosen"].contains("provisioning"), "{state:?}");
        assert!(state["last_outcome"].contains("provisioning"), "{state:?}");
        assert!(state["foreground"].contains("none"), "{state:?}");
    }
    let following = following.expect("companion did not follow after bounded upkeep");
    record(&node, "companion-follow");
    assert!(
        following["objective"].contains("companion"),
        "{following:?}"
    );
    assert!(following["chosen"].contains("follow"), "{following:?}");
    assert!(
        following["foreground"].contains("movement"),
        "{following:?}"
    );
    assert!(support::poll_until(support::POLL_TIMEOUT, || {
        let position = node.query_rows(&format!(
            "SELECT x, y FROM game_world_entity WHERE guid = {bot}"
        ));
        (position[0]["x"].parse::<f32>().unwrap() - 1200.0).abs() > 0.01
            || (position[0]["y"].parse::<f32>().unwrap() - 1200.0).abs() > 0.01
    }));
    assert_ne!(following["objective_sequence"], solo["objective_sequence"]);
    assert_eq!(quest(&node, bot, 7), held_quest);
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM game_melee_attack WHERE attacker_guid = {bot}"
        ))
        .is_empty());

    node.assert_call("playerbots_fixture_companion_remove_group", &[]);
    node.assert_call("playerbots_fixture_runner_pass_once", &[bot]);
    let unavailable = runner(&node, bot);
    record(&node, "companion-unavailable");
    let path = support::log_dir().join(format!("{}-runner.json", node.shard_name()));
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "solo": solo, "following": following, "unavailable": unavailable
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(unavailable["objective"], following["objective"]);
    assert_eq!(
        unavailable["objective_sequence"],
        following["objective_sequence"]
    );
    assert!(
        unavailable["chosen"].contains("partyUnavailable"),
        "{unavailable:?}"
    );
    assert!(unavailable["foreground"].contains("none"));
    assert_eq!(quest(&node, bot, 7), held_quest);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_quest_retries_after_deferral_without_replacing_its_purpose() {
    let (node, bots) = fixture("playerbots-quest-deferred-retry");
    let bot = bot_for_class(&bots, "1");
    node.assert_call("playerbots_quest_fixture_admit_accept", &[bot, "7"]);
    node.assert_call(
        "playerbots_quest_fixture_move_creature_spawn",
        &["6", "1230"],
    );
    node.assert_call("playerbots_quest_fixture_refresh", &[]);
    node.assert_call("playerbots_fixture_runner_stage", &[bot, "false"]);
    select_cohort(&node, bot);
    run_once(&node);
    let initial = runner(&node, bot);
    let retained = node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {bot}"
    ));
    assert!(initial["objective"].contains("quest"));
    assert!(initial["objective"].contains("travelling"), "{initial:?}");

    node.assert_call("playerbots_fixture_runner_expire_objective", &[bot]);
    run_once(&node);
    let deferred = runner(&node, bot);
    record(&node, "deferral-start");
    assert!(deferred["objective"].contains("deferred"), "{deferred:?}");
    assert!(!deferred["deferred_destinations"].is_empty());
    run_once(&node);
    let waiting = runner(&node, bot);
    assert_eq!(waiting["objective"], deferred["objective"]);
    assert_eq!(
        waiting["deferred_destinations"],
        deferred["deferred_destinations"]
    );

    let retried = support::poll_until(std::time::Duration::from_secs(45), || {
        runner(&node, bot)["objective"].contains("travelling")
    });
    let resumed = runner(&node, bot);
    record(&node, "deferred-retry");
    let path = support::log_dir().join(format!("{}-runner.json", node.shard_name()));
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "initial": initial, "deferred": deferred, "waiting": waiting, "resumed": resumed
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(
        retried,
        "quest did not retry after its deferral expired: {resumed:?}"
    );
    assert_eq!(resumed["objective_sequence"], initial["objective_sequence"]);
    assert!(resumed["deferred_destinations"].is_empty());
    assert_eq!(resumed["retry_count"], "0");
    assert_eq!(
        node.query_rows(&format!(
            "SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {bot}"
        )),
        retained
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_held_unsupported_quest_selects_supported_work_without_reaccepting() {
    let (node, bots) = fixture("playerbots-quest-reconcile");
    let bot = bot_for_class(&bots, "1");
    node.assert_call("playerbots_quest_fixture_admit_accept", &[bot, "7"]);
    node.assert_call("playerbots_fixture_runner_stage", &[bot, "false"]);
    select_cohort(&node, bot);
    run_once(&node);
    let objective_before = runner(&node, bot)["objective_sequence"].clone();
    let before = quest(&node, bot, 7);
    node.assert_call("playerbots_quest_fixture_held_becomes_unsupported", &[bot]);
    run_once(&node);
    let after = quest(&node, bot, 7);
    assert_eq!(after, before);
    assert_eq!(quest(&node, bot, 5261)["rewarded"], "false");
    let admission = node.query_rows(&format!(
        "SELECT selected_quest, state, missing_capability FROM pkg_playerbots_quest_admission WHERE character_guid = {bot}"
    ));
    assert_eq!(admission[0]["selected_quest"], "(some = 5261)");
    assert!(admission[0]["state"].contains("unsupported"));
    assert!(admission[0]["missing_capability"].contains("escort"));
    let selected = node.query_rows(&format!(
        "SELECT quest_entry, actual_ender_entry FROM pkg_playerbots_quest_objective WHERE character_guid = {bot}"
    ));
    assert_eq!(selected[0]["quest_entry"], "5261");
    assert_eq!(selected[0]["actual_ender_entry"], "196");
    let reconciled = runner(&node, bot);
    assert_ne!(reconciled["objective_sequence"], objective_before);
    assert!(reconciled["chosen"].contains("quest"));
    assert_eq!(
        node.query_rows(&format!(
            "SELECT id FROM game_character_quest WHERE character_guid = {bot} AND rewarded = false"
        ))
        .len(),
        2
    );
    let stable_seven = quest(&node, bot, 7);
    let stable_alternative = quest(&node, bot, 5261);
    run_once(&node);
    assert_eq!(quest(&node, bot, 7), stable_seven);
    assert_eq!(quest(&node, bot, 5261), stable_alternative);
    assert_eq!(
        runner(&node, bot)["objective_sequence"],
        reconciled["objective_sequence"]
    );
    let persistent = node.query_rows(&format!(
        "SELECT selected_quest, missing_capability FROM pkg_playerbots_quest_admission WHERE character_guid = {bot}"
    ));
    assert_eq!(persistent[0]["selected_quest"], "(some = 5261)");
    assert!(persistent[0]["missing_capability"].contains("escort"));
    record(&node, "reconcile");

    let (node, bots) = fixture("playerbots-quest-provided-item-loss");
    for (class, banked) in [("1", "false"), ("5", "true")] {
        let bot = bot_for_class(&bots, class);
        node.assert_call("playerbots_quest_fixture_admit_accept", &[bot, "3905"]);
        node.assert_call("playerbots_fixture_runner_stage", &[bot, "false"]);
        select_cohort(&node, bot);
        run_once(&node);
        assert_eq!(
            node.query_rows(&format!(
                "SELECT quest_entry FROM pkg_playerbots_quest_objective WHERE character_guid = {bot}"
            ))[0]["quest_entry"],
            "3905"
        );
        let held_before = quest(&node, bot, 3905);
        node.assert_call(
            "playerbots_quest_fixture_lose_provided_item",
            &[bot, banked],
        );
        run_once(&node);
        record(&node, &format!("provided-item-loss-class-{class}"));
        assert_eq!(quest(&node, bot, 3905), held_before);
        let admission = node.query_rows(&format!(
            "SELECT selected_quest, state, missing_capability, detail, observed_micros, wait_until_micros FROM pkg_playerbots_quest_admission WHERE character_guid = {bot}"
        ));
        assert_eq!(admission[0]["selected_quest"], "(none = ())");
        assert!(admission[0]["state"].contains("waiting"));
        assert!(admission[0]["missing_capability"].contains("missingProvidedItem"));
        assert!(admission[0]["detail"].contains("no longer carried"));
        assert!(
            admission[0]["wait_until_micros"].parse::<i64>().unwrap()
                > admission[0]["observed_micros"].parse::<i64>().unwrap()
        );
        assert!(node
            .query_rows(&format!(
                "SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {bot}"
            ))
            .is_empty());
        let held = runner(&node, bot);
        assert!(held["chosen"].contains("hold"));
        assert!(held["chosen"].contains("quest"));
        let provided = node.query_rows(&format!(
            "SELECT slot FROM game_item_instance WHERE owner_guid = {bot} AND entry = 11125"
        ));
        if banked == "true" {
            assert_eq!(provided.len(), 1);
            assert_eq!(provided[0]["slot"], "39");
        } else {
            assert!(provided.is_empty());
        }
    }
    record(&node, "provided-item-loss");
}

#[test]
#[ignore = "requires pinned PB-002 Wasm, SpacetimeDB, and the playerbots Package"]
fn playerbots_quest_catalog_upgrades_populated_pb002_runner_state() {
    let old_path = std::env::var_os("PLAYERBOTS_CATALOG_PRECEDING_WASM")
        .expect("PLAYERBOTS_CATALOG_PRECEDING_WASM must name the PB-002 Wasm");
    let old_wasm = std::fs::read(old_path).unwrap();
    let mut node = Standalone::start("playerbots-quest-pb002-migration");
    node.publish_module_bytes(&old_wasm);
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("playerbots_spawn_role", &["1", "1200", "1200", "50", "1"]);
    node.assert_call("playerbots_fixture_prepare", &[]);
    let bot = node.query_rows("SELECT character_guid FROM pkg_playerbots_bot")[0]["character_guid"]
        .clone();
    node.assert_sql("UPDATE game_spell SET cast_time_ms = 60000 WHERE spell_id = 5090100");
    node.assert_call("playerbots_fixture_runner_stage", &[&bot, "true"]);
    node.assert_call("playerbots_select_controller", &[&bot, "{\"cohort\":[]}"]);
    node.assert_call("playerbots_fixture_runner_due", &[]);
    node.assert_call("playerbots_fixture_runner_pass", &[]);
    node.assert_call("playerbots_fixture_freeze", &[&bot]);
    let preceding_rows = node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_runner WHERE character_guid = {bot}"
    ));
    assert_eq!(preceding_rows.len(), 1);
    let preceding = preceding_rows[0].clone();
    assert!(preceding["objective"].contains("returnHome"));
    assert!(preceding["foreground"].contains("cast"));
    let preceding_cast = node.query_rows("SELECT * FROM game_pending_cast");
    assert_eq!(preceding_cast.len(), 1);
    node.publish_module();
    let upgraded = node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_runner WHERE character_guid = {bot}"
    ));
    let upgraded_cast = node.query_rows("SELECT * FROM game_pending_cast");
    record(&node, "pb002-migration-current");
    let path = support::log_dir().join(format!("{}-pb002-migration.json", node.shard_name()));
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "preceding_core": "e6a755db0a150bbf73ad97b972fe829f20f6816c",
            "preceding_collection": "155c9e401afb06d5731acedf8fc35a81dbe4aaa6",
            "local_shared_reference_sha256": "9a041750c7e67f61b0d015504c7f40187c167254a377559f22a474e477297c32",
            "preceding_wasm_bytes": old_wasm.len(),
            "preceding_wasm_blake3": blake3::hash(&old_wasm).to_hex().to_string(),
            "current_wasm_blake3": blake3::hash(support::module_bytes()).to_hex().to_string(),
            "preceding_runner": preceding,
            "upgraded_runner": upgraded,
            "fixture_cast_time_ms": 60000,
            "preceding_pending_cast": preceding_cast,
            "upgraded_pending_cast": upgraded_cast,
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(upgraded.len(), 1);
    assert_eq!(upgraded[0]["objective"], preceding["objective"]);
    assert_eq!(upgraded[0]["foreground"], preceding["foreground"]);
    assert_eq!(
        upgraded[0]["objective_sequence"],
        preceding["objective_sequence"]
    );
    assert_eq!(upgraded_cast, preceding_cast);
    assert!(node
        .query_rows("SELECT * FROM pkg_playerbots_quest_objective")
        .is_empty());
}
