//! Autonomous supported quest-loop evidence on private seeded Standalone databases.

mod support;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use support::Standalone;

const PASS_INTERVAL: Duration = Duration::from_millis(1_250);
const LOOP_TIMEOUT: Duration = Duration::from_secs(180);
const CREATURE_6: u64 = (0xF130u64 << 48) | (6u64 << 24) | 1;
const GRIND_CREATURE_ENTRY: u32 = 51_000;
const SIMPLE_QUEST: u32 = 50_970;
const SIMPLE_GAMEOBJECT: u64 = (0xF110u64 << 48) | 5_090_970u64;
const SIMPLE_GAMEOBJECT_ALTERNATIVE: u64 = SIMPLE_GAMEOBJECT + 1;
const CHEST_ENTRY: u32 = 161_557;
const CHEST_GAMEOBJECT: u64 = (0xF110u64 << 48) | CHEST_ENTRY as u64;
const CHEST_GAMEOBJECT_ALTERNATIVE: u64 = CHEST_GAMEOBJECT + 1;

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

fn remove_builtin_weather_import_stamp(node: &Standalone) {
    let rows =
        node.query_rows("SELECT family, source_sha, file_hash, row_count FROM game_import_meta");
    assert_eq!(rows.len(), 1, "unexpected initial Import Catalogue");
    assert_eq!(rows[0]["family"], "weather_seed");
    assert_eq!(rows[0]["source_sha"], "");
    assert_eq!(rows[0]["file_hash"], "");
    assert_eq!(rows[0]["row_count"], "2");
    node.assert_sql(
        "DELETE FROM game_import_meta WHERE family = 'weather_seed' AND source_sha = '' AND file_hash = '' AND row_count = 2",
    );
}

fn fixture(name: &str, class: u8, role: u8, named: bool) -> (Standalone, String) {
    let mut node = Standalone::start(name);
    node.publish_module();
    remove_builtin_weather_import_stamp(&node);
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    stage_quest_geometry(&node);
    node.assert_call(
        "playerbots_spawn_class_role",
        &[
            "1",
            "1200",
            "1200",
            "50",
            &class.to_string(),
            &role.to_string(),
        ],
    );
    let guid = node.query_rows("SELECT character_guid FROM pkg_playerbots_bot")[0]
        ["character_guid"]
        .clone();
    node.assert_call(
        if named {
            "playerbots_quest_loop_fixture_stage_named"
        } else {
            "playerbots_quest_loop_fixture_stage_simple_gameobject"
        },
        &[&guid],
    );
    node.assert_call("playerbots_fixture_runner_select_cohort", &[&guid]);
    (node, guid)
}

fn spawn_frozen_foreign_bot(node: &Standalone, subject: &str, x: &str, y: &str) -> String {
    node.assert_call("playerbots_spawn_class_role", &["1", x, y, "50", "1", "0"]);
    let foreign = node
        .query_rows("SELECT character_guid FROM pkg_playerbots_bot")
        .into_iter()
        .map(|row| row["character_guid"].clone())
        .find(|guid| guid != subject)
        .expect("foreign bot is absent");
    node.assert_call("playerbots_fixture_freeze", &[&foreign]);
    foreign
}

fn assert_solo_loot_tag(node: &Standalone, creature_guid: u64, character_guid: &str) {
    let tag = query_one(
        node,
        &format!(
            "SELECT creature_guid, character_guid FROM game_creature_quest_tap WHERE creature_guid = {creature_guid}"
        ),
    );
    assert_eq!(tag["character_guid"], character_guid);
    let members = node.query_rows(&format!(
        "SELECT character_guid FROM game_creature_quest_tap_member WHERE creature_guid = {creature_guid}"
    ));
    assert_eq!(members.len(), 1, "{members:?}");
    assert_eq!(members[0]["character_guid"], character_guid);
    assert!(node
        .query_rows(&format!(
            "SELECT group_id FROM game_creature_loot_tag_group WHERE creature_guid = {creature_guid}"
        ))
        .is_empty());
}

fn assert_no_loot_tag(node: &Standalone, creature_guid: u64) {
    for table in [
        "game_creature_quest_tap",
        "game_creature_quest_tap_member",
        "game_creature_loot_tag_group",
    ] {
        assert!(node
            .query_rows(&format!(
                "SELECT creature_guid FROM {table} WHERE creature_guid = {creature_guid}"
            ))
            .is_empty());
    }
}

fn runner_target(
    node: &Standalone,
    guid: &str,
    reason: &str,
    targets: &[u64],
) -> BTreeMap<String, String> {
    drive_until(node, guid, Duration::from_secs(45), |node| {
        let runner = query_one(
            node,
            &format!(
                "SELECT character_guid, chosen, recovery FROM pkg_playerbots_runner WHERE character_guid = {guid}"
            ),
        );
        runner["chosen"].contains(reason)
            && targets
                .iter()
                .any(|target| runner["chosen"].contains(&target.to_string()))
    });
    query_one(
        node,
        &format!(
            "SELECT character_guid, chosen, recovery, objective_sequence FROM pkg_playerbots_runner WHERE character_guid = {guid}"
        ),
    )
}

fn query_one(node: &Standalone, sql: &str) -> BTreeMap<String, String> {
    node.query_rows(sql)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("query returned no row: {sql}"))
}

fn quest_read_limit_micros(failures: &str) -> Vec<i64> {
    const MARKER: &str = "questReadLimit = ()), at_micros = ";
    failures
        .match_indices(MARKER)
        .map(|(index, _)| {
            let value = &failures[index + MARKER.len()..];
            let end = value
                .find(|character: char| !character.is_ascii_digit())
                .unwrap_or(value.len());
            value[..end].parse().unwrap()
        })
        .collect()
}

fn quest(node: &Standalone, guid: &str, entry: u32) -> Option<BTreeMap<String, String>> {
    node.query_rows(&format!(
        "SELECT * FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = {entry}"
    ))
    .into_iter()
    .next()
}

fn first_quest_count(row: &BTreeMap<String, String>) -> u32 {
    row["counts"]
        .trim_matches(['[', ']'])
        .split(',')
        .next()
        .expect("quest count is missing")
        .trim()
        .parse()
        .expect("quest count is not numeric")
}

fn rewarded(node: &Standalone, guid: &str, entry: u32) -> bool {
    quest(node, guid, entry).is_some_and(|row| row["rewarded"] == "true")
}

fn item_count(node: &Standalone, guid: &str, entry: u32) -> u32 {
    node.query_rows(&format!(
        "SELECT stack_count FROM game_item_instance WHERE owner_guid = {guid} AND entry = {entry}"
    ))
    .iter()
    .map(|row| row["stack_count"].parse::<u32>().unwrap())
    .sum()
}

fn retained_quest_purpose(node: &Standalone, guid: &str) -> BTreeMap<String, String> {
    query_one(
        node,
        &format!(
            "SELECT quest_entry, target, work_area, actual_ender, actual_ender_entry, actual_ender_kind, catalog_revision, content_revision FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"
        ),
    )
}

fn drive_until(
    node: &Standalone,
    guid: &str,
    timeout: Duration,
    mut done: impl FnMut(&Standalone) -> bool,
) -> f32 {
    let deadline = Instant::now() + timeout;
    let mut max_x = f32::NEG_INFINITY;
    loop {
        node.assert_call("playerbots_fixture_runner_pass_once", &[guid]);
        let entity = query_one(
            node,
            &format!("SELECT x FROM game_world_entity WHERE guid = {guid}"),
        );
        max_x = max_x.max(entity["x"].parse::<f32>().unwrap());
        if done(node) {
            return max_x;
        }
        if Instant::now() >= deadline {
            record(node, "timeout");
            panic!("quest loop timed out");
        }
        std::thread::sleep(PASS_INTERVAL);
    }
}

fn actions(node: &Standalone, guid: &str) -> Vec<BTreeMap<String, String>> {
    node.query_rows(&format!(
        "SELECT kind, target_guid, spell_id, quest_entry, outcome FROM pkg_playerbots_action WHERE character_guid = {guid}"
    ))
}

fn action_present(rows: &[BTreeMap<String, String>], kind: &str, quest: u32) -> bool {
    rows.iter()
        .any(|row| row["kind"].contains(kind) && row["quest_entry"] == quest.to_string())
}

fn turnin_count(node: &Standalone, guid: &str, quest: u32) -> u16 {
    node.query_rows(&format!(
        "SELECT turnin_count FROM pkg_playerbots_quest_turnin_fixture WHERE character_guid = {guid} AND quest_entry = {quest}"
    ))
    .first()
    .map_or(0, |row| row["turnin_count"].parse().unwrap())
}

fn loot_receipt(node: &Standalone, guid: &str) -> Option<BTreeMap<String, String>> {
    node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_quest_loot_receipt_fixture WHERE character_guid = {guid}"
    ))
    .into_iter()
    .next()
}

fn structured_number(value: &str, field: &str) -> String {
    let marker = format!("{field} = ");
    value
        .split(&marker)
        .nth(1)
        .and_then(|tail| tail.split([',', ')']).next())
        .unwrap_or_else(|| panic!("{field} missing from {value}"))
        .trim()
        .to_string()
}

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

struct PrecedingQuestLoops {
    wasm: Vec<u8>,
    manifest: serde_json::Value,
}

fn preceding_quest_loops() -> PrecedingQuestLoops {
    let wasm_path = std::env::var_os("PLAYERBOTS_QUEST_PRECEDING_WASM")
        .expect("PLAYERBOTS_QUEST_PRECEDING_WASM must name the merged PB-004 Wasm");
    let manifest_path = std::env::var_os("PLAYERBOTS_QUEST_PRECEDING_MANIFEST")
        .expect("PLAYERBOTS_QUEST_PRECEDING_MANIFEST must describe that Wasm build");
    let core_path = std::env::var_os("PLAYERBOTS_QUEST_PRECEDING_CORE")
        .expect("PLAYERBOTS_QUEST_PRECEDING_CORE must name the clean merged Core checkout");
    let collection_path = std::env::var_os("PLAYERBOTS_QUEST_PRECEDING_COLLECTION").expect(
        "PLAYERBOTS_QUEST_PRECEDING_COLLECTION must name the clean merged Package checkout",
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
    PrecedingQuestLoops { wasm, manifest }
}

fn record(node: &Standalone, suffix: &str) {
    let core = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let package = core.join("packages/playerbots");
    let core_status = git(core, &["status", "--porcelain"]);
    let package_status = git(&package, &["status", "--porcelain"]);
    assert!(
        core_status.is_empty(),
        "Core evidence source is dirty: {core_status}"
    );
    assert!(
        package_status.is_empty(),
        "Package evidence source is dirty: {package_status}"
    );
    let mut package_digest = blake3::Hasher::new();
    digest_files(&package, &mut package_digest);
    let fixture_ownership = node.query_rows("SELECT * FROM pkg_playerbots_quest_fixture_ownership");
    let loop_fixture = node.query_rows("SELECT * FROM pkg_playerbots_quest_loop_fixture");
    let simple_fixture = node.query_rows("SELECT * FROM pkg_playerbots_seeded_quest_fixture");
    let mut seeded_content_identity: Vec<_> = fixture_ownership
        .iter()
        .filter_map(|row| row.get("revision").cloned())
        .chain(
            loop_fixture
                .iter()
                .chain(&simple_fixture)
                .filter_map(|row| row.get("content_revision").cloned()),
        )
        .collect();
    seeded_content_identity.sort();
    seeded_content_identity.dedup();
    let named_loop = seeded_content_identity
        .iter()
        .any(|identity| identity == "playerbots-loopback-named-q7-ten-targets-v2");
    let smite_header = node.query_rows("SELECT * FROM game_spell WHERE spell_id = 585");
    let smite_effect = node.query_rows("SELECT * FROM game_spell_effect WHERE spell_id = 585");
    let nav_chunks = node
        .query_rows("SELECT key, map_id, cell_x, cell_y, base_z, walk, obs FROM game_nav_chunk");
    let seeded_geometry_identity =
        (!nav_chunks.is_empty()).then_some("playerbots-synthetic-nav-v1");
    let smite_fixture = named_loop.then(|| {
        serde_json::json!({
            "classic_db_spell_template_sha256": "d2083bcd2670451279cbf93af138eadae04c6d183a4cd0ff0357047e4a565de6",
            "source_fields": "spell 585 header and effect normalized from ClassicDB spell_template",
            "auxiliary_fixture_choices": {
                "cast_time_ms": 1500,
                "range_yd": 30,
                "duration_ms": 0,
                "initial_priest_mana": 400,
            },
        })
    });
    let evidence = serde_json::json!({
        "fixture_scope": "private-loopback-seeded-content",
        "tested_core": git(core, &["rev-parse", "HEAD"]),
        "tested_collection": null,
        "core_dirty": false,
        "collection_dirty": false,
        "package_content_identity": package_digest.finalize().to_hex().to_string(),
        "module_wasm_blake3": blake3::hash(support::module_bytes()).to_hex().to_string(),
        "seeded_content_identity": seeded_content_identity,
        "seeded_geometry_identity": seeded_geometry_identity,
        "content": {
            "provenance": "private quest-loop fixture rows listed in this record",
            "imported_content": null,
            "smite_fixture": smite_fixture,
        },
        "geometry": {
            "revision": seeded_geometry_identity,
            "provenance": "private navigation rows listed in this record",
            "imported_geometry": null,
            "client_geometry": null,
            "coverage_expectation": "unknown; synthetic cells have no finalized derived manifest",
        },
        "fixture_ownership": fixture_ownership,
        "loop_fixture": loop_fixture,
        "simple_fixture": simple_fixture,
        "bots": node.query_rows("SELECT * FROM pkg_playerbots_bot"),
        "characters": node.query_rows("SELECT * FROM game_world_entity WHERE entry = 0"),
        "character_resources": node.query_rows("SELECT guid, power, max_power FROM game_world_entity WHERE entry = 0"),
        "runner": node.query_rows("SELECT * FROM pkg_playerbots_runner"),
        "retained": node.query_rows("SELECT * FROM pkg_playerbots_quest_objective"),
        "admission": node.query_rows("SELECT * FROM pkg_playerbots_quest_admission"),
        "actions": node.query_rows("SELECT * FROM pkg_playerbots_action"),
        "turnins": node.query_rows("SELECT * FROM pkg_playerbots_quest_turnin_fixture"),
        "loot_receipts": node.query_rows("SELECT * FROM pkg_playerbots_quest_loot_receipt_fixture"),
        "quests": node.query_rows("SELECT * FROM game_character_quest"),
        "quest_objectives": node.query_rows("SELECT * FROM game_quest_objective"),
        "entities": node.query_rows(&format!("SELECT * FROM game_world_entity WHERE entry = 6 OR entry = 69 OR entry = 299 OR entry = {GRIND_CREATURE_ENTRY}")),
        "items": node.query_rows("SELECT * FROM game_item_instance"),
        "loot": node.query_rows("SELECT * FROM game_corpse_loot"),
        "loot_entitlement": node.query_rows("SELECT * FROM game_corpse_loot_eligible"),
        "loot_tags": node.query_rows("SELECT * FROM game_creature_quest_tap"),
        "loot_tag_members": node.query_rows("SELECT * FROM game_creature_quest_tap_member"),
        "loot_tag_groups": node.query_rows("SELECT * FROM game_creature_loot_tag_group"),
        "gameobjects": node.query_rows(&format!("SELECT * FROM game_gameobject WHERE template_entry = 5090970 OR template_entry = {CHEST_ENTRY}")),
        "smite_header": smite_header,
        "smite_effect": smite_effect,
        "nav_chunks": nav_chunks,
        "navigation_config": node.query_rows("SELECT nav_enabled, nav_coverage_enabled FROM game_config WHERE id = 0"),
        "coverage_manifests": node.query_rows("SELECT * FROM game_vmap_nav_coverage_manifest"),
        "import_catalogue": node.query_rows("SELECT * FROM game_import_meta"),
    });
    std::fs::write(
        support::log_dir().join(format!("{}-{suffix}.json", node.shard_name())),
        serde_json::to_vec_pretty(&evidence).unwrap(),
    )
    .unwrap();
}

fn record_declared_targets(node: &Standalone, suffix: &str, targets: &[u64]) {
    let entities: Vec<_> = targets
        .iter()
        .flat_map(|target| {
            node.query_rows(&format!(
                "SELECT * FROM game_world_entity WHERE guid = {target}"
            ))
        })
        .collect();
    let evidence = serde_json::json!({
        "declared_target_guids": targets,
        "entities": entities,
        "runner": node.query_rows("SELECT * FROM pkg_playerbots_runner"),
        "loot_tags": node.query_rows("SELECT * FROM game_creature_quest_tap"),
        "loot_tag_members": node.query_rows("SELECT * FROM game_creature_quest_tap_member"),
        "loot_tag_groups": node.query_rows("SELECT * FROM game_creature_loot_tag_group"),
    });
    std::fs::write(
        support::log_dir().join(format!("{}-{suffix}.json", node.shard_name())),
        serde_json::to_vec_pretty(&evidence).unwrap(),
    )
    .unwrap();
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_autonomous_talk_kill_and_collect_loops_run_for_all_starter_classes() {
    for (class, role, label) in [(1, 0, "warrior"), (5, 1, "priest"), (8, 2, "mage")] {
        let (node, guid) = fixture(&format!("playerbots-quest-loop-{label}"), class, role, true);
        assert!(node
            .query_rows(&format!(
                "SELECT * FROM game_character_quest WHERE character_guid = {guid}"
            ))
            .is_empty());
        let before = query_one(
            &node,
            &format!(
                "SELECT x, xp, money, power, max_power FROM game_world_entity WHERE guid = {guid}"
            ),
        );
        if class == 5 {
            assert_eq!(before["power"], "400");
            assert_eq!(before["max_power"], "400");
            record(&node, "priest-initial-resource");
        }

        drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
            rewarded(node, &guid, 783)
        });
        assert_eq!(turnin_count(&node, &guid, 783), 1);

        let mut max_x = drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
            quest(node, &guid, 7).is_some_and(|quest| first_quest_count(&quest) > 0)
        });
        assert!(!node
            .query_rows("SELECT guid FROM game_world_entity WHERE entry = 6 AND dead = true")
            .is_empty());
        max_x = max_x.max(drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
            rewarded(node, &guid, 7)
        }));
        assert!(
            max_x - before["x"].parse::<f32>().unwrap() > 150.0,
            "{label} did not reach the work area beyond the former home leash"
        );
        let kill = quest(&node, &guid, 7).unwrap();
        assert_eq!(first_quest_count(&kill), 10, "{kill:?}");
        let combat = actions(&node, &guid);
        assert!(combat.iter().any(|row| {
            row["kind"].contains("move")
                && row["outcome"].contains("complete")
                && !row["outcome"].contains("direct")
        }));
        assert_eq!(turnin_count(&node, &guid, 7), 1);
        assert!(combat.iter().any(|row| {
            row["quest_entry"] == "0"
                && (row["kind"].contains("attack") || row["kind"].contains("cast"))
                && (row["outcome"].contains("attackAccepted")
                    || row["outcome"].contains("castResolved"))
        }));
        if class == 5 {
            assert!(combat.iter().any(|row| {
                row["kind"].contains("cast")
                    && row["spell_id"] == "585"
                    && row["outcome"].contains("castResolved")
            }));
        }
        let after_kill = query_one(
            &node,
            &format!("SELECT xp, money FROM game_world_entity WHERE guid = {guid}"),
        );
        assert!(after_kill["xp"].parse::<u32>().unwrap() > before["xp"].parse::<u32>().unwrap());
        assert!(
            after_kill["money"].parse::<u32>().unwrap() > before["money"].parse::<u32>().unwrap()
        );

        drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
            quest(node, &guid, 5261).is_some()
        });

        if class == 8 {
            drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
                rewarded(node, &guid, 5261)
            });
            drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
                turnin_count(node, &guid, 33) == 1
            });
            record(&node, "mage-collect-reward");
            let receipt = loot_receipt(&node, &guid).expect("Mage loot receipt is absent");
            let objective = query_one(
                &node,
                "SELECT kind, target_entry, required_count FROM game_quest_objective WHERE quest_entry = 33 AND obj_index = 0",
            );
            assert_eq!(objective["kind"], "1");
            assert_eq!(objective["target_entry"], "750");
            assert_eq!(objective["required_count"], "8");
            assert_eq!(receipt["item_entry"], "750");
            assert_eq!(receipt["received_count"], "8");
            assert_eq!(receipt["peak_carried_count"], "8");
            assert_ne!(receipt["last_source_guid"], "0");
            assert!(rewarded(&node, &guid, 33));
            assert_eq!(item_count(&node, &guid, 750), 0);
            assert_eq!(turnin_count(&node, &guid, 5261), 1);
            assert_eq!(turnin_count(&node, &guid, 33), 1);
        }

        for _ in 0..4 {
            node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
            std::thread::sleep(PASS_INTERVAL);
        }
        assert_eq!(turnin_count(&node, &guid, 783), 1);
        assert_eq!(turnin_count(&node, &guid, 7), 1);
        if class == 8 {
            assert_eq!(turnin_count(&node, &guid, 5261), 1);
            assert_eq!(turnin_count(&node, &guid, 33), 1);
        }
        record(&node, label);
    }
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_quest_purpose_survives_interruption_death_and_retreats_to_an_observed_position() {
    let (node, guid) = fixture("playerbots-quest-loop-survival", 1, 0, true);
    drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
        rewarded(node, &guid, 783)
            && node
                .query_rows(&format!(
                    "SELECT safe_position FROM pkg_playerbots_quest_objective WHERE character_guid = {guid} AND quest_entry = 7"
                ))
                .first()
                .is_some_and(|row| !row["safe_position"].contains("none"))
    });
    let movement_deadline = Instant::now() + Duration::from_secs(30);
    let retained = loop {
        let retained = query_one(
            &node,
            &format!("SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"),
        );
        let safe_x = structured_number(&retained["safe_position"], "x")
            .parse::<f32>()
            .unwrap();
        let safe_y = structured_number(&retained["safe_position"], "y")
            .parse::<f32>()
            .unwrap();
        let entity = query_one(
            &node,
            &format!("SELECT x, y FROM game_world_entity WHERE guid = {guid}"),
        );
        let x = entity["x"].parse::<f32>().unwrap();
        let y = entity["y"].parse::<f32>().unwrap();
        if (x - safe_x).powi(2) + (y - safe_y).powi(2) > 3.0f32.powi(2) {
            break retained;
        }
        if Instant::now() >= movement_deadline {
            record(&node, "survival-interruption-movement-timeout");
            panic!("bot did not move away from its retained safe position");
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    let objective = query_one(
        &node,
        &format!("SELECT objective, objective_sequence FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
    );
    let safe = retained["safe_position"].clone();
    let safe_x = structured_number(&safe, "x");
    let safe_y = structured_number(&safe, "y");
    assert!(!safe.contains("none"));
    assert!(!safe.contains("guid"));

    let before_hit = query_one(
        &node,
        &format!("SELECT health, max_health FROM game_world_entity WHERE guid = {guid}"),
    );
    let max_health = before_hit["max_health"].parse::<u32>().unwrap();
    assert!(
        max_health >= 20,
        "starter health is too small for survival evidence"
    );
    let incoming_damage = max_health / 2;
    node.assert_call(
        "playerbots_fixture_runner_survival_hit",
        &[&guid, &CREATURE_6.to_string(), &incoming_damage.to_string()],
    );
    let after_hit = query_one(
        &node,
        &format!("SELECT health, dead FROM game_world_entity WHERE guid = {guid}"),
    );
    let interrupted = query_one(
        &node,
        &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
    );
    let retained_after_interruption = query_one(
        &node,
        &format!("SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"),
    );
    let character_after_interruption = query_one(
        &node,
        &format!("SELECT * FROM game_world_entity WHERE guid = {guid}"),
    );
    std::fs::write(
        support::log_dir().join(format!("{}-survival-interruption.json", node.shard_name())),
        serde_json::to_vec_pretty(&serde_json::json!({
            "retained_before_interruption": &retained,
            "retained_after_interruption": &retained_after_interruption,
            "before_hit": &before_hit,
            "after_hit": &after_hit,
            "character_after_interruption": &character_after_interruption,
            "runner_after_interruption": &interrupted,
        }))
        .unwrap(),
    )
    .unwrap();
    record(&node, "survival-interruption");

    assert!(
        after_hit["health"].parse::<u32>().unwrap() < before_hit["health"].parse::<u32>().unwrap(),
        "incoming combat did not change authoritative health"
    );
    assert_eq!(after_hit["dead"], "false");
    assert!(
        interrupted["chosen"].contains("survival"),
        "{interrupted:?}"
    );
    assert!(
        interrupted["foreground"].contains(&safe_x),
        "{interrupted:?}"
    );
    assert!(
        interrupted["foreground"].contains(&safe_y),
        "{interrupted:?}"
    );
    assert!(!interrupted["foreground"].contains("x = 1200"));
    assert_eq!(
        interrupted["objective_sequence"],
        objective["objective_sequence"]
    );
    assert_eq!(retained_after_interruption["safe_position"], safe);

    node.assert_call(
        "playerbots_fixture_runner_survival_hit",
        &[&guid, &CREATURE_6.to_string(), "10000"],
    );
    let lethal_character = query_one(
        &node,
        &format!("SELECT * FROM game_world_entity WHERE guid = {guid}"),
    );
    let lethal_runner = query_one(
        &node,
        &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
    );
    let lethal_retained = query_one(
        &node,
        &format!("SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"),
    );
    std::fs::write(
        support::log_dir().join(format!("{}-survival-death-hit.json", node.shard_name())),
        serde_json::to_vec_pretty(&serde_json::json!({
            "character": &lethal_character,
            "runner": &lethal_runner,
            "retained_quest": &lethal_retained,
        }))
        .unwrap(),
    )
    .unwrap();
    record(&node, "survival-death-hit");
    assert_eq!(lethal_character["dead"], "true");
    drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
        query_one(
            node,
            &format!("SELECT dead FROM game_world_entity WHERE guid = {guid}"),
        )["dead"]
            == "false"
    });
    drive_until(&node, &guid, Duration::from_secs(20), |node| {
        let state = query_one(
            node,
            &format!(
                "SELECT chosen, foreground FROM pkg_playerbots_runner WHERE character_guid = {guid}"
            ),
        );
        state["chosen"].contains("survival") && state["foreground"].contains(&safe_x)
    });
    let resumed = query_one(
        &node,
        &format!("SELECT objective_sequence, objective FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
    );
    assert_eq!(
        resumed["objective_sequence"],
        objective["objective_sequence"]
    );
    assert!(resumed["objective"].contains("quest"));
    assert_eq!(
        query_one(
            &node,
            &format!("SELECT safe_position FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"),
        )["safe_position"],
        safe
    );
    record(&node, "survival-death");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_observed_safe_position_is_invalidated_after_a_partition_change() {
    let (node, guid) = fixture("playerbots-quest-loop-safe-partition", 1, 0, true);
    node.assert_call("playerbots_quest_fixture_admit_accept", &[&guid, "7"]);
    node.assert_call("playerbots_fixture_position", &[&guid, "1328"]);
    drive_until(&node, &guid, Duration::from_secs(10), |node| {
        node.query_rows(&format!(
            "SELECT safe_position FROM pkg_playerbots_quest_objective WHERE character_guid = {guid} AND quest_entry = 7"
        ))
        .first()
        .is_some_and(|row| !row["safe_position"].contains("none"))
    });
    let held = quest(&node, &guid, 7).expect("accepted Quest missing");
    record(&node, "safe-before-partition-change");

    node.assert_call("playerbots_quest_loop_fixture_set_partition", &[&guid, "1"]);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    record(&node, "safe-partition-change");
    let retained = node.query_rows(&format!(
        "SELECT quest_entry, runner_objective_identity, safe_position FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"
    ));
    let admission = query_one(
        &node,
        &format!("SELECT considered_quest, selected_quest, missing_capability FROM pkg_playerbots_quest_admission WHERE character_guid = {guid}"),
    );
    let after_quest = quest(&node, &guid, 7);
    assert!(retained.is_empty(), "{retained:?}");
    assert_eq!(admission["considered_quest"], "7", "{admission:?}");
    assert_eq!(admission["selected_quest"], "(none = ())", "{admission:?}");
    assert_eq!(
        admission["missing_capability"], "(some = (missingActualEndDestination = ()))",
        "{admission:?}"
    );
    assert_eq!(after_quest, Some(held));
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_survival_preempts_an_unavailable_quest_rotation() {
    let (node, guid) = fixture("playerbots-quest-loop-survival-read-limit", 1, 0, true);
    drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
        rewarded(node, &guid, 783)
            && node
                .query_rows(&format!(
                    "SELECT safe_position FROM pkg_playerbots_quest_objective WHERE character_guid = {guid} AND quest_entry = 7"
                ))
                .first()
                .is_some_and(|row| !row["safe_position"].contains("none"))
    });
    let movement_deadline = Instant::now() + Duration::from_secs(30);
    let retained = loop {
        let retained = query_one(
            &node,
            &format!("SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"),
        );
        let safe_x = structured_number(&retained["safe_position"], "x")
            .parse::<f32>()
            .unwrap();
        let safe_y = structured_number(&retained["safe_position"], "y")
            .parse::<f32>()
            .unwrap();
        let entity = query_one(
            &node,
            &format!("SELECT x, y FROM game_world_entity WHERE guid = {guid}"),
        );
        let x = entity["x"].parse::<f32>().unwrap();
        let y = entity["y"].parse::<f32>().unwrap();
        if (x - safe_x).powi(2) + (y - safe_y).powi(2) > 3.0f32.powi(2) {
            break retained;
        }
        if Instant::now() >= movement_deadline {
            record(&node, "survival-read-limit-movement-timeout");
            panic!("bot did not move away from its retained safe position");
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    node.assert_call("playerbots_fixture_roles_overflow", &[&guid, "0"]);
    let before_hit = query_one(
        &node,
        &format!("SELECT health, max_health FROM game_world_entity WHERE guid = {guid}"),
    );
    let incoming_damage = before_hit["max_health"].parse::<u32>().unwrap() / 2;
    node.assert_call(
        "playerbots_fixture_runner_survival_hit",
        &[&guid, &CREATURE_6.to_string(), &incoming_damage.to_string()],
    );
    let after_hit = query_one(
        &node,
        &format!("SELECT health, dead FROM game_world_entity WHERE guid = {guid}"),
    );
    let runner = query_one(
        &node,
        &format!(
            "SELECT chosen, failures FROM pkg_playerbots_runner WHERE character_guid = {guid}"
        ),
    );
    record(&node, "survival-read-limit");
    std::fs::write(
        support::log_dir().join(format!(
            "{}-survival-read-limit-hit.json",
            node.shard_name()
        )),
        serde_json::to_vec_pretty(&serde_json::json!({
            "retained_quest": retained,
            "before_hit": before_hit,
            "after_hit": after_hit,
            "runner_after_hit": runner,
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(
        after_hit["health"].parse::<u32>().unwrap() < before_hit["health"].parse::<u32>().unwrap(),
        "incoming combat did not change authoritative health"
    );
    assert_eq!(after_hit["dead"], "false");
    assert!(runner["chosen"].contains("survival"), "{runner:?}");
    assert!(runner["failures"].contains("questReadLimit"), "{runner:?}");
    assert_eq!(
        query_one(
            &node,
            &format!("SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"),
        ),
        retained
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_quest_purpose_survives_recovery_and_provisioning() {
    let (node, guid) = fixture("playerbots-quest-loop-upkeep", 1, 0, true);
    drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
        rewarded(node, &guid, 783)
            && quest(node, &guid, 7).is_some()
            && node
                .query_rows(&format!(
                    "SELECT destination FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"
                ))
                .first()
                .is_some_and(|row| row["destination"].contains("entry = 6"))
    });
    let retained = retained_quest_purpose(&node, &guid);
    let objective = query_one(
        &node,
        &format!(
            "SELECT objective_sequence FROM pkg_playerbots_runner WHERE character_guid = {guid}"
        ),
    );

    node.assert_call("playerbots_fixture_runner_stage", &[&guid, "true"]);
    let before_recovery = query_one(
        &node,
        &format!("SELECT health, max_health FROM game_world_entity WHERE guid = {guid}"),
    );
    drive_until(&node, &guid, Duration::from_secs(60), |node| {
        actions(node, &guid).iter().any(|row| {
            row["kind"].contains("cast")
                && row["spell_id"] == "5090100"
                && row["outcome"].contains("castResolved")
        })
    });
    let after_recovery = query_one(
        &node,
        &format!("SELECT health, max_health FROM game_world_entity WHERE guid = {guid}"),
    );
    let retained_after_recovery = retained_quest_purpose(&node, &guid);

    node.assert_call("playerbots_fixture_provision_catalog", &[]);
    node.assert_call("playerbots_fixture_provision_complete_profile", &[&guid]);
    node.assert_call("playerbots_fixture_provision_reset", &[&guid]);
    drive_until(&node, &guid, Duration::from_secs(60), |node| {
        query_one(
            node,
            &format!(
                "SELECT last_outcome FROM pkg_playerbots_runner WHERE character_guid = {guid}"
            ),
        )["last_outcome"]
            .contains("provisioning")
    });
    let final_runner = query_one(
        &node,
        &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
    );
    let final_retained = retained_quest_purpose(&node, &guid);
    let final_character = query_one(
        &node,
        &format!("SELECT * FROM game_world_entity WHERE guid = {guid}"),
    );
    std::fs::write(
        support::log_dir().join(format!(
            "{}-recovery-provisioning-effect.json",
            node.shard_name()
        )),
        serde_json::to_vec_pretty(&serde_json::json!({
            "before_recovery": &before_recovery,
            "after_recovery": &after_recovery,
            "character": &final_character,
            "runner": &final_runner,
            "retained_after_recovery": &retained_after_recovery,
            "retained_after_provisioning": &final_retained,
            "actions": actions(&node, &guid),
            "quest": quest(&node, &guid, 7),
        }))
        .unwrap(),
    )
    .unwrap();
    record(&node, "recovery-provisioning");

    let before_health = before_recovery["health"].parse::<u32>().unwrap();
    let after_health = after_recovery["health"].parse::<u32>().unwrap();
    assert!(after_health > before_health);
    assert_eq!(retained_after_recovery, retained);
    assert_eq!(
        query_one(
            &node,
            &format!(
                "SELECT objective_sequence FROM pkg_playerbots_runner WHERE character_guid = {guid}"
            ),
        ),
        objective
    );
    assert_eq!(final_retained, retained);
    assert_eq!(first_quest_count(&quest(&node, &guid, 7).unwrap()), 0);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_raw_entity_read_limit_keeps_travelling_to_retained_quest_work() {
    let (node, guid) = fixture("playerbots-quest-loop-target-change", 1, 0, true);
    drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
        rewarded(node, &guid, 783)
    });
    drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
        quest(node, &guid, 7).is_some()
    });
    drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
        let objective = query_one(
            node,
            &format!(
                "SELECT quest_entry, destination, target FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"
            ),
        );
        let character = query_one(
            node,
            &format!("SELECT x, y, z FROM game_world_entity WHERE guid = {guid}"),
        );
        let target = query_one(
            node,
            &format!("SELECT x, y, z FROM game_world_entity WHERE guid = {CREATURE_6}"),
        );
        let distance_sq = ["x", "y", "z"].into_iter().fold(0.0, |sum, field| {
            let difference =
                character[field].parse::<f32>().unwrap() - target[field].parse::<f32>().unwrap();
            sum + difference * difference
        });
        objective["quest_entry"] == "7"
            && objective["destination"].contains(&format!("guid = {CREATURE_6}"))
            && objective["target"].contains("target_entry = 6")
            && objective["target"].contains(&format!("guid = {CREATURE_6}"))
            && distance_sq > 100.0 * 100.0
            && first_quest_count(&quest(node, &guid, 7).unwrap()) == 0
    });
    let retained = query_one(
        &node,
        &format!("SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"),
    );
    let attacks_before = node.query_rows(&format!(
        "SELECT * FROM game_melee_attack WHERE attacker_guid = {guid}"
    ));
    node.assert_call("playerbots_quest_loop_fixture_stage_search_limit", &[&guid]);
    drive_until(&node, &guid, Duration::from_secs(10), |node| {
        query_one(
            node,
            &format!("SELECT character_guid, failures FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
        )["failures"]
            .contains("questReadLimit")
    });
    let limited = query_one(
        &node,
        &format!(
            "SELECT failures, chosen, objective_sequence FROM pkg_playerbots_runner WHERE character_guid = {guid}"
        ),
    );
    assert!(
        limited["failures"].contains("questReadLimit"),
        "{limited:?}"
    );
    assert!(
        limited["chosen"].contains("move = (home = ())"),
        "{limited:?}"
    );
    assert!(
        limited["chosen"].contains("reason = (returnHome = ())"),
        "{limited:?}"
    );
    assert!(limited["chosen"].contains(&format!("objective = {}", limited["objective_sequence"])));
    assert_eq!(
        query_one(
            &node,
            &format!("SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"),
        ),
        retained
    );
    assert_eq!(
        node.query_rows(&format!(
            "SELECT * FROM game_melee_attack WHERE attacker_guid = {guid}"
        )),
        attacks_before
    );
    assert_eq!(first_quest_count(&quest(&node, &guid, 7).unwrap()), 0);
    let working = drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
        let working = query_one(
            node,
            &format!(
                "SELECT failures, objective_sequence, chosen FROM pkg_playerbots_runner WHERE character_guid = {guid}"
            ),
        );
        working["objective_sequence"] == limited["objective_sequence"]
            && working["chosen"].contains(&format!("attack = {CREATURE_6}"))
            && working["chosen"].contains("reason = (quest = ())")
            && working["chosen"].contains(&format!("objective = {}", limited["objective_sequence"]))
    });
    assert!(
        working >= 1_250.0,
        "retained destination travel did not advance: {working}"
    );
    let working_runner = query_one(
        &node,
        &format!(
            "SELECT character_guid, failures FROM pkg_playerbots_runner WHERE character_guid = {guid}"
        ),
    );
    let read_limits = quest_read_limit_micros(&working_runner["failures"]);
    assert!(!read_limits.is_empty(), "{working_runner:?}");
    assert!(
        read_limits
            .windows(2)
            .all(|pair| pair[1].saturating_sub(pair[0]) >= 30_000_000),
        "{working_runner:?}"
    );
    let mut working_objective = query_one(
        &node,
        &format!("SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"),
    );
    let mut retained_purpose = retained.clone();
    working_objective.remove("safe_position");
    retained_purpose.remove("safe_position");
    assert_eq!(working_objective, retained_purpose);
    let resumed = drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
        first_quest_count(&quest(node, &guid, 7).unwrap()) > 0
    });
    assert!(
        resumed >= 1_350.0,
        "retained destination was not reached: {resumed}"
    );
    record(&node, "read-limit");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_in_progress_quest_target_survives_an_unrelated_raw_read_limit() {
    let (node, guid) = fixture("playerbots-quest-loop-active-target-read-limit", 1, 0, true);
    drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
        rewarded(node, &guid, 783)
    });
    node.assert_call(
        "playerbots_select_controller",
        &[&guid, "{\"recordOnly\":[]}"],
    );
    node.assert_call("playerbots_quest_fixture_admit_accept", &[&guid, "7"]);
    let alternative = CREATURE_6 + 1;
    node.assert_call("playerbots_fixture_runner_select_cohort", &[&guid]);
    drive_until(&node, &guid, Duration::from_secs(30), |node| {
        let runner = query_one(
            node,
            &format!(
                "SELECT character_guid, chosen FROM pkg_playerbots_runner WHERE character_guid = {guid}"
            ),
        );
        let x = query_one(
            node,
            &format!("SELECT x FROM game_world_entity WHERE guid = {guid}"),
        )["x"]
            .parse::<f32>()
            .unwrap();
        runner["chosen"].contains(&format!("move = (entity = {CREATURE_6})")) && x > 1_210.0
    });
    node.assert_call(
        "playerbots_quest_loop_fixture_make_target_friendly",
        &[&guid, &CREATURE_6.to_string()],
    );
    drive_until(&node, &guid, Duration::from_secs(30), |node| {
        let runner = query_one(
            node,
            &format!(
                "SELECT chosen, objective_sequence, recovery FROM pkg_playerbots_runner WHERE character_guid = {guid}"
            ),
        );
        let x = query_one(
            node,
            &format!("SELECT x FROM game_world_entity WHERE guid = {guid}"),
        )["x"]
            .parse::<f32>()
            .unwrap();
        runner["chosen"].contains(&format!("move = (entity = {alternative})"))
            && runner["recovery"].contains(&format!("active = (some = (fight = {alternative}))"))
            && x > 1_210.0
    });
    let retained = query_one(
        &node,
        &format!(
            "SELECT quest_entry, runner_objective_identity, target FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"
        ),
    );
    let before = query_one(
        &node,
        &format!(
            "SELECT objective_sequence, chosen, failures, recovery FROM pkg_playerbots_runner WHERE character_guid = {guid}"
        ),
    );
    assert_eq!(retained["quest_entry"], "7");
    assert!(retained["target"].contains(&format!("guid = {CREATURE_6}")));
    assert_eq!(
        retained["runner_objective_identity"],
        before["objective_sequence"]
    );
    assert!(before["chosen"].contains(&alternative.to_string()));
    assert!(
        before["recovery"].contains(&format!("active = (some = (fight = {alternative}))")),
        "{before:?}"
    );
    assert_eq!(first_quest_count(&quest(&node, &guid, 7).unwrap()), 0);

    node.assert_call("playerbots_quest_loop_fixture_stage_search_limit", &[&guid]);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let continued = query_one(
        &node,
        &format!(
            "SELECT objective_sequence, chosen, failures FROM pkg_playerbots_runner WHERE character_guid = {guid}"
        ),
    );
    record(&node, "active-target-read-limit-boundary");
    assert_eq!(
        continued["objective_sequence"],
        before["objective_sequence"]
    );
    assert!(
        continued["chosen"].contains(&alternative.to_string()),
        "{continued:?}"
    );
    assert!(continued["chosen"].contains("reason = (quest = ())"));
    assert!(!continued["chosen"].contains("returnHome"));
    assert_eq!(continued["failures"], before["failures"]);

    drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
        first_quest_count(&quest(node, &guid, 7).unwrap()) > 0
    });
    assert!(actions(&node, &guid).iter().any(|action| {
        action["target_guid"] == alternative.to_string()
            && (action["kind"].contains("attack") || action["kind"].contains("cast"))
    }));
    assert_eq!(first_quest_count(&quest(&node, &guid, 7).unwrap()), 1);
    record(&node, "active-target-read-limit");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_quest_uses_a_reward_eligible_target_and_retains_productive_work() {
    let (node, guid) = fixture("playerbots-quest-loop-productive-target", 1, 0, true);
    let foreign = spawn_frozen_foreign_bot(&node, &guid, "1360", "1200");
    drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
        rewarded(node, &guid, 783)
    });
    node.assert_call(
        "playerbots_select_controller",
        &[&guid, "{\"recordOnly\":[]}"],
    );
    node.assert_call(
        "debug_add_threat",
        &[&CREATURE_6.to_string(), &foreign, "1"],
    );
    assert_solo_loot_tag(&node, CREATURE_6, &foreign);
    node.assert_call("playerbots_quest_fixture_admit_accept", &[&guid, "7"]);
    node.assert_call("playerbots_fixture_runner_select_cohort", &[&guid]);

    let alternative = CREATURE_6 + 1;
    let selected = runner_target(
        &node,
        &guid,
        "reason = (quest = ())",
        &[CREATURE_6, alternative],
    );
    assert_solo_loot_tag(&node, CREATURE_6, &foreign);
    node.assert_call(
        "playerbots_select_controller",
        &[&guid, "{\"recordOnly\":[]}"],
    );
    record(&node, "productive-target-selected");
    assert!(
        selected["chosen"].contains(&alternative.to_string()),
        "{selected:?}"
    );
    assert!(selected["recovery"].contains(&format!("active = (some = (fight = {alternative}))")));
    let retained = retained_quest_purpose(&node, &guid);
    assert_eq!(retained["quest_entry"], "7");
    assert_eq!(first_quest_count(&quest(&node, &guid, 7).unwrap()), 0);

    node.assert_call(
        "playerbots_quest_loop_fixture_respawn_target",
        &[&guid, &CREATURE_6.to_string()],
    );
    assert_no_loot_tag(&node, CREATURE_6);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let respawned = query_one(
        &node,
        &format!(
            "SELECT chosen, recovery FROM pkg_playerbots_runner WHERE character_guid = {guid}"
        ),
    );
    record(&node, "productive-target-respawned-primary");
    assert!(
        respawned["chosen"].contains(&alternative.to_string()),
        "{respawned:?}"
    );
    assert!(respawned["recovery"].contains(&format!("active = (some = (fight = {alternative}))")));

    node.assert_call(
        "debug_add_threat",
        &[&alternative.to_string(), &foreign, "1"],
    );
    assert_solo_loot_tag(&node, alternative, &foreign);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let switched = query_one(
        &node,
        &format!(
            "SELECT chosen, recovery FROM pkg_playerbots_runner WHERE character_guid = {guid}"
        ),
    );
    record(&node, "productive-target-retagged");
    assert!(
        switched["chosen"].contains(&CREATURE_6.to_string()),
        "{switched:?}"
    );
    assert!(!switched["chosen"].contains(&alternative.to_string()));
    assert_eq!(retained_quest_purpose(&node, &guid), retained);

    node.assert_call("playerbots_fixture_runner_select_cohort", &[&guid]);
    drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
        first_quest_count(&quest(node, &guid, 7).unwrap()) == 1
    });
    assert!(actions(&node, &guid).iter().any(|action| {
        action["target_guid"] == CREATURE_6.to_string()
            && (action["kind"].contains("attack") || action["kind"].contains("cast"))
    }));
    assert_no_loot_tag(&node, CREATURE_6);
    let entitlement = node.query_rows(&format!(
        "SELECT eligible_guid FROM game_corpse_loot_eligible WHERE corpse_guid = {CREATURE_6}"
    ));
    assert_eq!(entitlement.len(), 1, "{entitlement:?}");
    assert_eq!(entitlement[0]["eligible_guid"], guid);
    record(&node, "productive-target-credit");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_grind_reuses_only_a_reward_eligible_target() {
    let mut node = Standalone::start("playerbots-grind-productive-target");
    node.publish_module();
    remove_builtin_weather_import_stamp(&node);
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    stage_quest_geometry(&node);
    node.assert_call(
        "playerbots_spawn_class_role",
        &["1", "1200", "1200", "50", "1", "0"],
    );
    let guid = node.query_rows("SELECT character_guid FROM pkg_playerbots_bot")[0]
        ["character_guid"]
        .clone();
    node.assert_call(
        "playerbots_select_controller",
        &[&guid, "{\"recordOnly\":[]}"],
    );
    let subject = query_one(
        &node,
        &format!("SELECT x, y, orientation FROM game_world_entity WHERE guid = {guid}"),
    );
    let subject_x = subject["x"].parse::<f32>().unwrap();
    let subject_y = subject["y"].parse::<f32>().unwrap();
    assert_eq!(subject["orientation"].parse::<f32>().unwrap(), 0.0);
    let foreign_x = (subject_x + 50.0).to_string();
    let foreign_y = subject_y.to_string();
    let foreign = spawn_frozen_foreign_bot(&node, &guid, &foreign_x, &foreign_y);
    node.assert_call(
        "debug_spawn_at_feet",
        &[&guid, &GRIND_CREATURE_ENTRY.to_string(), "50"],
    );
    node.assert_call(
        "debug_spawn_at_feet",
        &[&guid, &GRIND_CREATURE_ENTRY.to_string(), "55"],
    );
    let targets: Vec<u64> = node
        .query_rows(&format!(
            "SELECT guid, x, y FROM game_world_entity WHERE entry = {GRIND_CREATURE_ENTRY} AND x >= {} AND x <= {} AND y >= {} AND y <= {}",
            subject_x + 49.0,
            subject_x + 56.0,
            subject_y - 1.0,
            subject_y + 1.0,
        ))
        .into_iter()
        .map(|row| row["guid"].parse().unwrap())
        .collect();
    assert_eq!(targets.len(), 2, "{targets:?}");
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let selected = runner_target(&node, &guid, "reason = (grind = ())", &targets);
    let target = *targets
        .iter()
        .find(|target| selected["chosen"].contains(&target.to_string()))
        .expect("Grind target is absent from the candidate");
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let retained = query_one(
        &node,
        &format!("SELECT chosen FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
    );
    assert!(
        retained["chosen"].contains(&target.to_string()),
        "{retained:?}"
    );

    node.assert_call("playerbots_fixture_runner_select_cohort", &[&guid]);
    drive_until(&node, &guid, Duration::from_secs(10), |node| {
        let runner = query_one(
            node,
            &format!(
                "SELECT chosen, foreground FROM pkg_playerbots_runner WHERE character_guid = {guid}"
            ),
        );
        runner["chosen"].contains(&target.to_string())
            && runner["chosen"].contains("reason = (grind = ())")
            && runner["foreground"].contains(&target.to_string())
            && runner["foreground"].contains("reason = (grind = ())")
    });
    record_declared_targets(&node, "grind-productive-target-active", &targets);
    node.assert_call("debug_add_threat", &[&target.to_string(), &foreign, "1"]);
    assert_solo_loot_tag(&node, target, &foreign);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let switched = query_one(
        &node,
        &format!(
            "SELECT chosen, foreground FROM pkg_playerbots_runner WHERE character_guid = {guid}"
        ),
    );
    let alternative = *targets
        .iter()
        .find(|candidate| **candidate != target)
        .unwrap();
    record_declared_targets(&node, "grind-productive-target-retagged", &targets);
    node.assert_call(
        "playerbots_select_controller",
        &[&guid, "{\"recordOnly\":[]}"],
    );
    record(&node, "grind-productive-target");
    assert!(
        switched["chosen"].contains("reason = (grind = ())"),
        "{switched:?}"
    );
    assert!(
        switched["chosen"].contains(&alternative.to_string()),
        "{switched:?}"
    );
    assert!(!switched["chosen"].contains(&target.to_string()));
    assert!(
        switched["foreground"].contains(&alternative.to_string()),
        "{switched:?}"
    );
    assert!(!switched["foreground"].contains(&target.to_string()));
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_ineligible_target_reselects_without_changing_quest_purpose() {
    let (node, guid) = fixture("playerbots-quest-loop-ineligible-target", 1, 0, true);
    drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
        rewarded(node, &guid, 783)
            && query_one(
                node,
                &format!("SELECT chosen FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
            )["chosen"]
                .contains(&CREATURE_6.to_string())
    });
    let retained = query_one(
        &node,
        &format!("SELECT quest_entry, runner_objective_identity FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"),
    );
    node.assert_call(
        "playerbots_quest_loop_fixture_make_target_friendly",
        &[&guid, &CREATURE_6.to_string()],
    );
    drive_until(&node, &guid, Duration::from_secs(10), |node| {
        let chosen = query_one(
            node,
            &format!("SELECT chosen FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
        )["chosen"]
            .clone();
        chosen.contains(&(CREATURE_6 + 1).to_string())
            && !chosen.contains(&format!("target = {CREATURE_6}"))
    });
    assert_eq!(
        query_one(
            &node,
            &format!("SELECT quest_entry, runner_objective_identity FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"),
        ),
        retained
    );
    assert_eq!(first_quest_count(&quest(&node, &guid, 7).unwrap()), 0);
    record(&node, "target-ineligible");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_disappeared_target_reselects_without_changing_quest_purpose() {
    let (node, guid) = fixture("playerbots-quest-loop-disappearance", 1, 0, true);
    drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
        rewarded(node, &guid, 783)
            && query_one(
                node,
                &format!("SELECT chosen FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
            )["chosen"]
                .contains(&CREATURE_6.to_string())
    });
    let retained = query_one(
        &node,
        &format!("SELECT quest_entry, runner_objective_identity FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"),
    );
    node.assert_call("playerbots_quest_fixture_hide_live_target", &["6"]);
    drive_until(&node, &guid, Duration::from_secs(10), |node| {
        let chosen = query_one(
            node,
            &format!("SELECT chosen FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
        )["chosen"]
            .clone();
        chosen.contains(&(CREATURE_6 + 1).to_string())
            && !chosen.contains(&format!("target = {CREATURE_6}"))
    });
    let after = query_one(
        &node,
        &format!("SELECT quest_entry, runner_objective_identity FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"),
    );
    assert_eq!(after, retained);
    assert_eq!(first_quest_count(&quest(&node, &guid, 7).unwrap()), 0);
    record(&node, "target-disappearance");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_ninth_inaccessible_corpse_reports_an_inconclusive_read() {
    let (node, guid) = fixture("playerbots-quest-loop-corpse-limit", 8, 2, true);
    node.assert_call(
        "playerbots_select_controller",
        &[&guid, "{\"recordOnly\":[]}"],
    );
    node.assert_call("playerbots_quest_fixture_admit_accept", &[&guid, "33"]);
    drive_until(&node, &guid, Duration::from_secs(10), |node| {
        let retained = query_one(
            node,
            &format!("SELECT quest_entry, target FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"),
        );
        let runner = query_one(
            node,
            &format!("SELECT character_guid, chosen FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
        );
        let source = structured_number(&retained["target"], "guid");
        retained["quest_entry"] == "33"
            && runner["chosen"].contains(&source)
            && runner["chosen"].contains("reason = (quest = ())")
    });
    let retained = retained_quest_purpose(&node, &guid);
    let source = structured_number(&retained["target"], "guid");
    assert!(!rewarded(&node, &guid, 33));
    assert_eq!(first_quest_count(&quest(&node, &guid, 33).unwrap()), 0);
    assert!(loot_receipt(&node, &guid).is_none());
    assert_eq!(item_count(&node, &guid, 750), 0);
    assert_eq!(turnin_count(&node, &guid, 33), 0);
    node.assert_call("playerbots_quest_loop_fixture_stage_corpse_limit", &[&guid]);
    node.assert_call("playerbots_quest_fixture_hide_live_target", &["69"]);
    let inaccessible = node
        .query_rows("SELECT guid, dead FROM game_world_entity WHERE entry = 69 AND dead = true");
    assert_eq!(inaccessible.len(), 9, "{inaccessible:?}");
    assert!(inaccessible.iter().all(|corpse| corpse["guid"] != source));
    assert!(node
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE guid = {source}"
        ))
        .is_empty());
    node.assert_call("playerbots_fixture_runner_select_cohort", &[&guid]);
    drive_until(&node, &guid, Duration::from_secs(10), |node| {
        query_one(
            node,
            &format!("SELECT character_guid, failures FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
        )["failures"]
            .contains("questReadLimit")
    });
    let limited = query_one(
        &node,
        &format!(
            "SELECT failures, chosen FROM pkg_playerbots_runner WHERE character_guid = {guid}"
        ),
    );
    assert!(
        limited["failures"].contains("questReadLimit"),
        "{limited:?}"
    );
    assert!(
        limited["chosen"].contains("move = (home = ())"),
        "{limited:?}"
    );
    assert!(limited["chosen"].contains("reason = (returnHome = ())"));
    drive_until(&node, &guid, Duration::from_secs(10), |node| {
        let waiting = query_one(
            node,
            &format!("SELECT chosen FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
        );
        waiting["chosen"].contains("hold") && waiting["chosen"].contains("reason = (quest = ())")
    });
    let first_wait = query_one(
        &node,
        &format!(
            "SELECT character_guid, failures, observed_micros, next_eligible_micros, objective FROM pkg_playerbots_runner WHERE character_guid = {guid}"
        ),
    );
    let first_failures = quest_read_limit_micros(&first_wait["failures"]);
    assert!(!first_failures.is_empty(), "{first_wait:?}");
    let retry_at = first_wait["next_eligible_micros"].parse::<i64>().unwrap();
    let observed = first_wait["observed_micros"].parse::<i64>().unwrap();
    assert!(retry_at > observed, "{first_wait:?}");
    assert_eq!(
        retry_at,
        first_failures.last().unwrap().saturating_add(30_000_000),
        "{first_wait:?}"
    );
    let actions_before_retries = actions(&node, &guid);
    for _ in 0..8 {
        node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    }
    let repeated_wait = query_one(
        &node,
        &format!(
            "SELECT character_guid, failures, next_eligible_micros, last_outcome, objective FROM pkg_playerbots_runner WHERE character_guid = {guid}"
        ),
    );
    record(&node, "corpse-limit-repeated-wait");
    assert_eq!(
        quest_read_limit_micros(&repeated_wait["failures"]),
        first_failures,
        "{repeated_wait:?}"
    );
    assert_eq!(
        repeated_wait["next_eligible_micros"]
            .parse::<i64>()
            .unwrap(),
        retry_at,
        "{repeated_wait:?}"
    );
    assert!(repeated_wait["last_outcome"].contains("waiting"));
    assert_eq!(repeated_wait["objective"], first_wait["objective"]);
    assert_eq!(actions(&node, &guid), actions_before_retries);
    let attempted = actions(&node, &guid);
    for corpse in &inaccessible {
        assert!(attempted.iter().all(|action| {
            action["target_guid"] != corpse["guid"]
                || !["attack", "cast", "openLoot", "takeLoot"]
                    .iter()
                    .any(|kind| action["kind"].contains(kind))
        }));
    }
    assert_eq!(first_quest_count(&quest(&node, &guid, 33).unwrap()), 0);
    assert!(!rewarded(&node, &guid, 33));
    assert!(loot_receipt(&node, &guid).is_none());
    assert_eq!(item_count(&node, &guid, 750), 0);
    assert_eq!(turnin_count(&node, &guid, 33), 0);
    record(&node, "corpse-limit");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_retained_creature_source_survives_a_raw_read_limit_through_loot() {
    let (node, guid) = fixture("playerbots-quest-loop-corpse-raw-limit", 8, 2, true);
    node.assert_call("playerbots_quest_fixture_admit_accept", &[&guid, "33"]);
    node.assert_call(
        "playerbots_select_controller",
        &[&guid, "{\"recordOnly\":[]}"],
    );
    drive_until(&node, &guid, Duration::from_secs(10), |node| {
        let retained = query_one(
            node,
            &format!("SELECT quest_entry, target FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"),
        );
        let runner = query_one(
            node,
            &format!("SELECT chosen FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
        );
        let source = structured_number(&retained["target"], "guid");
        retained["quest_entry"] == "33"
            && runner["chosen"].contains(&source)
            && (runner["chosen"].contains("attack") || runner["chosen"].contains("cast"))
            && runner["chosen"].contains("reason = (quest = ())")
            && first_quest_count(&quest(node, &guid, 33).unwrap()) == 0
    });
    let retained = query_one(
        &node,
        &format!("SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"),
    );
    let source = structured_number(&retained["target"], "guid");
    let runner = query_one(
        &node,
        &format!(
            "SELECT chosen, failures, objective_sequence FROM pkg_playerbots_runner WHERE character_guid = {guid}"
        ),
    );
    assert!(runner["chosen"].contains(&source), "{runner:?}");
    assert_eq!(
        retained["runner_objective_identity"],
        runner["objective_sequence"]
    );
    assert!(runner["chosen"].contains(&format!("objective = {}", runner["objective_sequence"])));
    let read_limits = quest_read_limit_micros(&runner["failures"]);
    assert!(!rewarded(&node, &guid, 33));
    assert!(loot_receipt(&node, &guid).is_none());
    assert_eq!(item_count(&node, &guid, 750), 0);
    assert_eq!(turnin_count(&node, &guid, 33), 0);
    let live_source = query_one(
        &node,
        &format!("SELECT health, dead FROM game_world_entity WHERE guid = {source}"),
    );
    assert_eq!(live_source["health"], "1");
    assert_eq!(live_source["dead"], "false");
    assert!(actions(&node, &guid)
        .iter()
        .all(|action| action["target_guid"] != source));

    node.assert_call("playerbots_quest_loop_fixture_stage_search_limit", &[&guid]);
    node.assert_call("playerbots_select_controller", &[&guid, "{\"cohort\":[]}"]);
    drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
        let source_actions: Vec<_> = actions(node, &guid)
            .into_iter()
            .filter(|action| action["target_guid"] == source)
            .collect();
        let resolved = source_actions.iter().any(|action| {
            (action["kind"].contains("attack") && action["outcome"].contains("attackAccepted"))
                || (action["kind"].contains("cast") && action["outcome"].contains("castResolved"))
        });
        let looted = ["openLoot", "takeLoot"].into_iter().all(|kind| {
            source_actions.iter().any(|action| {
                action["kind"].contains(kind) && action["outcome"].contains("completed")
            })
        });
        let terminal = item_count(node, &guid, 750) == 8
            || (rewarded(node, &guid, 33) && turnin_count(node, &guid, 33) == 1);
        loot_receipt(node, &guid).is_some_and(|receipt| {
            receipt["last_source_guid"] == source
                && receipt["item_entry"] == "750"
                && receipt["received_count"] == "8"
                && receipt["peak_carried_count"] == "8"
                && resolved
                && looted
                && terminal
        })
    });
    let receipt = loot_receipt(&node, &guid).expect("loot receipt is absent");
    let source_actions: Vec<_> = actions(&node, &guid)
        .into_iter()
        .filter(|action| action["target_guid"] == source)
        .collect();
    assert!(source_actions.iter().any(|action| {
        (action["kind"].contains("attack") && action["outcome"].contains("attackAccepted"))
            || (action["kind"].contains("cast") && action["outcome"].contains("castResolved"))
    }));
    for kind in ["openLoot", "takeLoot"] {
        assert!(source_actions.iter().any(|action| {
            action["kind"].contains(kind) && action["outcome"].contains("completed")
        }));
    }
    assert_eq!(receipt["last_source_guid"], source);
    assert_eq!(receipt["item_entry"], "750");
    assert_eq!(receipt["received_count"], "8");
    assert_eq!(receipt["peak_carried_count"], "8");
    assert!(
        item_count(&node, &guid, 750) == 8
            || (rewarded(&node, &guid, 33) && turnin_count(&node, &guid, 33) == 1)
    );
    assert_eq!(first_quest_count(&quest(&node, &guid, 33).unwrap()), 0);
    let final_runner = query_one(
        &node,
        &format!(
            "SELECT character_guid, failures FROM pkg_playerbots_runner WHERE character_guid = {guid}"
        ),
    );
    assert_eq!(
        quest_read_limit_micros(&final_runner["failures"]),
        read_limits
    );
    record(&node, "retained-source-loot");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_simple_gameobject_use_is_autonomous_stateful_and_once_only() {
    let (node, guid) = fixture("playerbots-quest-loop-simple-gameobject", 8, 2, false);
    node.assert_call("playerbots_quest_loop_fixture_refresh", &[]);
    drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
        quest(node, &guid, SIMPLE_QUEST).is_some()
    });
    node.assert_call(
        "playerbots_quest_loop_fixture_set_simple_gameobject_state",
        &["1"],
    );
    drive_until(&node, &guid, Duration::from_secs(10), |node| {
        node.query_rows(&format!(
            "SELECT character_guid, failures FROM pkg_playerbots_runner WHERE character_guid = {guid}"
        ))
        .first()
        .is_some_and(|row| row["failures"].contains("questRespawn"))
    });
    let waiting = query_one(
        &node,
        &format!(
            "SELECT failures, chosen FROM pkg_playerbots_runner WHERE character_guid = {guid}"
        ),
    );
    assert!(waiting["failures"].contains("questRespawn"), "{waiting:?}");
    assert!(waiting["chosen"].contains("hold"), "{waiting:?}");
    assert_eq!(
        first_quest_count(&quest(&node, &guid, SIMPLE_QUEST).unwrap()),
        0
    );
    record(&node, "simple-gameobject-busy");
    node.assert_call(
        "playerbots_quest_loop_fixture_add_gameobject_alternative",
        &["5090970"],
    );
    drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
        quest(node, &guid, SIMPLE_QUEST).is_some_and(|quest| first_quest_count(&quest) == 1)
    });
    let selected = query_one(
        &node,
        &format!("SELECT chosen FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
    );
    assert!(selected["chosen"].contains("useGameObject"), "{selected:?}");
    assert!(
        selected["chosen"].contains(&SIMPLE_GAMEOBJECT_ALTERNATIVE.to_string()),
        "{selected:?}"
    );
    assert!(selected["chosen"].contains(&SIMPLE_QUEST.to_string()));
    drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
        rewarded(node, &guid, SIMPLE_QUEST)
    });
    let completed = quest(&node, &guid, SIMPLE_QUEST).unwrap();
    assert_eq!(first_quest_count(&completed), 1);
    let rows = actions(&node, &guid);
    assert!(action_present(&rows, "acceptQuest", SIMPLE_QUEST));
    assert!(action_present(&rows, "useGameObject", SIMPLE_QUEST));
    assert!(action_present(&rows, "turnInQuest", SIMPLE_QUEST));
    assert_eq!(turnin_count(&node, &guid, SIMPLE_QUEST), 1);
    let reward = query_one(
        &node,
        &format!("SELECT xp, money FROM game_world_entity WHERE guid = {guid}"),
    );
    for _ in 0..4 {
        node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
        std::thread::sleep(PASS_INTERVAL);
    }
    assert_eq!(
        query_one(
            &node,
            &format!("SELECT xp, money FROM game_world_entity WHERE guid = {guid}"),
        ),
        reward
    );
    assert!(action_present(
        &actions(&node, &guid),
        "useGameObject",
        SIMPLE_QUEST
    ));
    assert_eq!(turnin_count(&node, &guid, SIMPLE_QUEST), 1);
    record(&node, "simple-gameobject");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_gameobject_collect_skips_a_depleted_nearer_chest() {
    let (node, guid) = fixture("playerbots-quest-loop-gameobject-alternative", 8, 2, true);
    node.assert_call("playerbots_quest_fixture_admit_accept", &[&guid, "3904"]);
    node.assert_call(
        "playerbots_quest_fixture_deplete_gameobject",
        &[&CHEST_ENTRY.to_string()],
    );
    node.assert_call(
        "playerbots_quest_loop_fixture_add_gameobject_alternative",
        &[&CHEST_ENTRY.to_string()],
    );
    drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
        item_count(node, &guid, 11119) == 8
    });
    let rows = actions(&node, &guid);
    assert!(rows.iter().any(|row| {
        row["kind"].contains("useGameObject")
            && row["quest_entry"] == "3904"
            && row["target_guid"] == CHEST_GAMEOBJECT_ALTERNATIVE.to_string()
    }));
    assert!(!rows.iter().any(|row| {
        row["kind"].contains("useGameObject")
            && row["quest_entry"] == "3904"
            && row["target_guid"] == CHEST_GAMEOBJECT.to_string()
    }));
    record(&node, "gameobject-alternative-before-turnin");
    drive_until(&node, &guid, LOOP_TIMEOUT, |node| {
        rewarded(node, &guid, 3904)
    });
    assert_eq!(item_count(&node, &guid, 11119), 0);
    assert_eq!(turnin_count(&node, &guid, 3904), 1);
    record(&node, "gameobject-alternative");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_record_only_quest_decisions_do_not_change_core_gameplay() {
    let (node, guid) = fixture("playerbots-quest-loop-record-only", 8, 2, false);
    node.assert_call(
        "playerbots_select_controller",
        &[&guid, "{\"recordOnly\":[]}"],
    );
    let before_actions = actions(&node, &guid);
    let before_quest = quest(&node, &guid, SIMPLE_QUEST);
    let before_entity = query_one(
        &node,
        &format!("SELECT * FROM game_world_entity WHERE guid = {guid}"),
    );
    let before_go = query_one(
        &node,
        &format!("SELECT * FROM game_gameobject WHERE guid = {SIMPLE_GAMEOBJECT}"),
    );
    let before_cast = node.query_rows(&format!(
        "SELECT * FROM game_pending_cast WHERE caster_guid = {guid}"
    ));
    let before_spline = node.query_rows(&format!(
        "SELECT * FROM game_creature_spline WHERE guid = {guid}"
    ));
    for _ in 0..4 {
        node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    }
    record(&node, "record-only");
    assert_eq!(quest(&node, &guid, SIMPLE_QUEST), before_quest);
    assert_eq!(actions(&node, &guid), before_actions);
    assert_eq!(
        query_one(
            &node,
            &format!("SELECT * FROM game_world_entity WHERE guid = {guid}"),
        ),
        before_entity
    );
    assert_eq!(
        query_one(
            &node,
            &format!("SELECT * FROM game_gameobject WHERE guid = {SIMPLE_GAMEOBJECT}"),
        ),
        before_go
    );
    assert_eq!(
        node.query_rows(&format!(
            "SELECT * FROM game_pending_cast WHERE caster_guid = {guid}"
        )),
        before_cast
    );
    assert_eq!(
        node.query_rows(&format!(
            "SELECT * FROM game_creature_spline WHERE guid = {guid}"
        )),
        before_spline
    );
    let runner = query_one(
        &node,
        &format!(
            "SELECT last_outcome, chosen FROM pkg_playerbots_runner WHERE character_guid = {guid}"
        ),
    );
    assert!(runner["last_outcome"].contains("recorded"));
    assert!(runner["chosen"].contains("quest"));
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_changed_quest_interaction_starts_a_fresh_retry() {
    let (node, guid) = fixture("playerbots-quest-retry-identity", 1, 0, true);
    node.assert_call("playerbots_quest_fixture_admit_accept", &[&guid, "7"]);
    node.assert_call("playerbots_fixture_runner_stage", &[&guid, "false"]);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    node.assert_call(
        "playerbots_fixture_runner_stage_quest_retry",
        &[&guid, "7", &CREATURE_6.to_string()],
    );
    let prior = query_one(
        &node,
        &format!(
            "SELECT retry_count, retry_candidate FROM pkg_playerbots_runner WHERE character_guid = {guid}"
        ),
    );

    node.assert_call(
        "playerbots_fixture_runner_refuse_quest_candidate",
        &[&guid, "7", &u64::MAX.to_string()],
    );
    let refused = query_one(
        &node,
        &format!(
            "SELECT retry_count, retry_candidate, next_eligible_micros, observed_micros, last_outcome, chosen, failures FROM pkg_playerbots_runner WHERE character_guid = {guid}"
        ),
    );
    record(&node, "changed-quest-retry");

    assert_eq!(prior["retry_count"], "3");
    assert!(prior["retry_candidate"].contains(&CREATURE_6.to_string()));
    assert_eq!(refused["retry_count"], "1");
    assert!(refused["retry_candidate"].contains(&u64::MAX.to_string()));
    assert!(refused["chosen"].contains(&u64::MAX.to_string()));
    assert!(refused["last_outcome"].contains("actionRefused"));
    assert_eq!(
        refused["next_eligible_micros"].parse::<i64>().unwrap()
            - refused["observed_micros"].parse::<i64>().unwrap(),
        1_000_000
    );
    assert_eq!(quest(&node, &guid, 7).unwrap()["rewarded"], "false");
}

#[test]
#[ignore = "requires the merged PB-004 Wasm, SpacetimeDB, and the playerbots Package"]
fn playerbots_populated_pb004_quest_objective_and_runner_upgrade_in_place() {
    let preceding = preceding_quest_loops();
    assert_ne!(
        blake3::hash(&preceding.wasm),
        blake3::hash(support::module_bytes())
    );
    let mut node = Standalone::start("playerbots-quest-loop-pb004-migration");
    node.publish_module_bytes(&preceding.wasm);
    remove_builtin_weather_import_stamp(&node);
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    stage_quest_geometry(&node);
    node.assert_call("playerbots_spawn_role", &["1", "1200", "1200", "50", "0"]);
    let guid = node.query_rows("SELECT character_guid FROM pkg_playerbots_bot")[0]
        ["character_guid"]
        .clone();
    node.assert_call("playerbots_select_controller", &[&guid, "{\"cohort\":[]}"]);
    node.assert_call("debug_learn_spell", &[&guid, "355"]);
    node.assert_call("playerbots_fixture_provision_steps", &[&guid, "1"]);
    node.assert_sql(&format!(
        "UPDATE pkg_playerbots_provisioning SET next_repair_micros = 9223372036854775807 WHERE character_guid = {guid}"
    ));
    node.assert_call("playerbots_quest_fixture_stage", &[&guid]);
    node.assert_call(
        "playerbots_quest_fixture_move_creature_spawn",
        &["6", "1230"],
    );
    node.assert_call("playerbots_quest_fixture_refresh", &[]);
    node.assert_call("playerbots_quest_fixture_admit_accept", &[&guid, "7"]);
    node.assert_call("playerbots_fixture_runner_stage", &[&guid, "true"]);
    node.assert_sql("UPDATE game_spell SET cast_time_ms = 60000 WHERE spell_id = 5090100");
    let deadline = Instant::now() + Duration::from_secs(30);
    let (preceding_runner, preceding_objective) = loop {
        node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
        let runner = query_one(
            &node,
            &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
        );
        let objective = query_one(
            &node,
            &format!("SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"),
        );
        if runner["objective"].contains("quest") && runner["foreground"].contains("cast") {
            break (runner, objective);
        }
        if Instant::now() >= deadline {
            std::fs::write(
                support::log_dir().join(format!(
                    "{}-pb004-populated-predecessor-timeout.json",
                    node.shard_name()
                )),
                serde_json::to_vec_pretty(&serde_json::json!({
                    "preceding_build": preceding.manifest.clone(),
                    "preceding_runner": runner,
                    "preceding_quest_objective": objective,
                    "preceding_quest": quest(&node, &guid, 7),
                    "preceding_cast": node.query_rows(&format!(
                        "SELECT * FROM game_pending_cast WHERE caster_guid = {guid}"
                    )),
                }))
                .unwrap(),
            )
            .unwrap();
            panic!("preceding recovery cast did not start under the retained quest");
        }
        std::thread::sleep(PASS_INTERVAL);
    };
    node.assert_call("playerbots_fixture_freeze", &[&guid]);
    let preceding_cast = query_one(
        &node,
        &format!("SELECT * FROM game_pending_cast WHERE caster_guid = {guid}"),
    );
    let preceding_spell = query_one(
        &node,
        "SELECT spell_id, cast_time_ms FROM game_spell WHERE spell_id = 5090100",
    );
    std::fs::write(
        support::log_dir().join(format!(
            "{}-pb004-populated-predecessor.json",
            node.shard_name()
        )),
        serde_json::to_vec_pretty(&serde_json::json!({
            "preceding_build": preceding.manifest.clone(),
            "preceding_runner": preceding_runner,
            "preceding_quest_objective": preceding_objective,
            "preceding_quest": quest(&node, &guid, 7),
            "preceding_cast": preceding_cast,
            "synthetic_recovery_spell": preceding_spell,
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(preceding_objective["quest_entry"], "7");
    assert!(preceding_runner["objective"].contains("quest"));
    assert!(preceding_runner["foreground"].contains("cast"));
    assert!(preceding_runner["foreground"].contains("spell = 5090100"));
    assert_eq!(first_quest_count(&quest(&node, &guid, 7).unwrap()), 0);
    assert!(!rewarded(&node, &guid, 7));

    node.publish_module();
    let upgraded_runner = query_one(
        &node,
        &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
    );
    let upgraded_objective = query_one(
        &node,
        &format!("SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"),
    );
    let upgraded_cast = query_one(
        &node,
        &format!("SELECT * FROM game_pending_cast WHERE caster_guid = {guid}"),
    );
    let upgraded_spell = query_one(
        &node,
        "SELECT spell_id, cast_time_ms FROM game_spell WHERE spell_id = 5090100",
    );
    record(&node, "pb004-migration-current");
    std::fs::write(
        support::log_dir().join(format!(
            "{}-pb004-populated-migration.json",
            node.shard_name()
        )),
        serde_json::to_vec_pretty(&serde_json::json!({
            "preceding_build": preceding.manifest,
            "preceding_runner": preceding_runner,
            "upgraded_runner": upgraded_runner,
            "preceding_quest_objective": preceding_objective,
            "upgraded_quest_objective": upgraded_objective,
            "preceding_cast": preceding_cast,
            "upgraded_cast": upgraded_cast,
            "synthetic_recovery_spell": preceding_spell,
            "current_wasm_blake3": blake3::hash(support::module_bytes()).to_hex().to_string(),
        }))
        .unwrap(),
    )
    .unwrap();

    for (field, value) in &preceding_runner {
        assert_eq!(&upgraded_runner[field], value, "runner field {field}");
    }
    for (field, value) in &preceding_objective {
        assert_eq!(&upgraded_objective[field], value, "quest field {field}");
    }
    assert_eq!(upgraded_cast, preceding_cast);
    assert_eq!(upgraded_spell, preceding_spell);
    assert!(upgraded_objective["work_area"].contains("none"));
    assert!(upgraded_objective["safe_position"].contains("none"));
    assert_eq!(
        quest(&node, &guid, 7).unwrap()["rewarded"],
        "false",
        "upgrade changed authoritative quest progress"
    );
}
