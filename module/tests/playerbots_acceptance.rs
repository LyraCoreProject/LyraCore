//! Release acceptance journeys on private Standalone databases.

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};
use support::Standalone;

const CREATURE_6: u64 = (0xF130u64 << 48) | (6u64 << 24) | 1;
const OBSERVATION_INTERVAL: Duration = Duration::from_millis(250);
const JOURNEY_TIMEOUT: Duration = Duration::from_secs(600);
const CATALOG_QUESTS: [u32; 13] = [
    783, 7, 5261, 33, 18, 3903, 3904, 3905, 40, 35, 37, 45, 50_970,
];
const CARRIED_ITEMS: [u32; 4] = [750, 752, 11_119, 11_125];
const CHEST_GAMEOBJECT: u64 = (0xF110u64 << 48) | 161_557;
const SIMPLE_GAMEOBJECT: u64 = (0xF110u64 << 48) | 5_090_970;
const INVENTORY_FAULT_FILLER: u32 = 5_099_400;
const LEVEL_GAP_QUEST: u32 = 40;
const LEVEL_GAP_END_ONLY_ENTRY: u32 = 952;
const LEVEL_GAP_SOURCE_ENTRY: u32 = 5_099_410;
const LEVEL_GAP_SOURCE_COUNT: u64 = 66;

#[derive(Clone, Copy)]
struct JourneyCase {
    seed: u64,
    class: u8,
    role: u8,
    suffix: u8,
    start_offset_x: f32,
    start_offset_y: f32,
    initial_due_phase_micros: i64,
    source_insertion_rotation: u8,
}

fn query_one(node: &Standalone, sql: &str) -> BTreeMap<String, String> {
    node.query_rows(sql)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("query returned no row: {sql}"))
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

fn save(node: &Standalone, suffix: &str, state: serde_json::Value) {
    let core = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let package = core.join("packages/playerbots");
    let mut package_digest = blake3::Hasher::new();
    digest_files(&package, &mut package_digest);
    let record = serde_json::json!({
        "tested_core": git(core, &["rev-parse", "HEAD"]),
        "tested_collection": git(&package, &["rev-parse", "HEAD"]),
        "core_tree": git(core, &["rev-parse", "HEAD^{tree}"]),
        "collection_tree": git(&package, &["rev-parse", "HEAD^{tree}"]),
        "playerbots_tree": git(&package, &["rev-parse", "HEAD:playerbots"]),
        "core_dirty": !git(core, &["status", "--porcelain"]).is_empty(),
        "collection_dirty": !git(&package, &["status", "--porcelain"]).is_empty(),
        "package_content_identity": package_digest.finalize().to_hex().to_string(),
        "module_wasm_blake3": blake3::hash(support::module_bytes()).to_hex().to_string(),
        "state": state,
    });
    let path = support::log_dir().join(format!("{}-{suffix}.json", node.shard_name()));
    std::fs::write(&path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    eprintln!("acceptance evidence: {}", path.display());
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

fn structured_number(value: &str, field: &str) -> i64 {
    let marker = format!("{field} = ");
    value
        .split(&marker)
        .nth(1)
        .and_then(|tail| tail.split([',', ')']).next())
        .unwrap_or_else(|| panic!("{field} missing from {value}"))
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("{field} is not numeric in {value}"))
}

fn structured_u64(value: &str, field: &str) -> u64 {
    let marker = format!("{field} = ");
    value
        .split(&marker)
        .nth(1)
        .and_then(|tail| tail.split([',', ')']).next())
        .unwrap_or_else(|| panic!("{field} missing from {value}"))
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("{field} is not an unsigned integer in {value}"))
}

fn structured_f32(value: &str, field: &str) -> f32 {
    let marker = format!("{field} = ");
    value
        .split(&marker)
        .nth(1)
        .and_then(|tail| tail.split([',', ')']).next())
        .unwrap_or_else(|| panic!("{field} missing from {value}"))
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("{field} is not a float in {value}"))
}

fn item_count(node: &Standalone, guid: &str, entry: u32) -> u32 {
    node.query_rows(&format!(
        "SELECT stack_count FROM game_item_instance WHERE owner_guid = {guid} AND entry = {entry}"
    ))
    .iter()
    .map(|row| row["stack_count"].parse::<u32>().unwrap())
    .sum()
}

fn rewarded(node: &Standalone, guid: &str, quest: u32) -> bool {
    node.query_rows(&format!(
        "SELECT rewarded FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = {quest}"
    ))
    .first()
    .is_some_and(|row| row["rewarded"] == "true")
}

fn turnin_count(node: &Standalone, guid: &str, quest: u32) -> u16 {
    node.query_rows(&format!(
        "SELECT turnin_count FROM pkg_playerbots_quest_turnin_fixture WHERE character_guid = {guid} AND quest_entry = {quest}"
    ))
    .first()
    .map_or(0, |row| row["turnin_count"].parse().unwrap())
}

fn creature_guid(entry: u32) -> u64 {
    (0xF130u64 << 48) | (u64::from(entry) << 24) | 1
}

fn json_f32(value: &serde_json::Value) -> f32 {
    value.as_str().unwrap().parse().unwrap()
}

fn stage_journey(node: &Standalone, case: JourneyCase) -> (String, serde_json::Value) {
    node.assert_call(
        "playerbots_acceptance_stage_journey",
        &[&case.seed.to_string()],
    );
    let guid = query_one(
        node,
        &format!(
            "SELECT character_guid FROM pkg_playerbots_bot WHERE class = {}",
            case.class
        ),
    )["character_guid"]
        .clone();
    let admission_fault_query = format!(
        "SELECT prerequisite_refused_micros, prerequisite_refusal_detail FROM \
         pkg_playerbots_acceptance_admission_fault WHERE character_guid = {guid}"
    );
    let prerequisite_refusal_observed = support::poll_until(support::POLL_TIMEOUT, || {
        let admission = query_one(node, &admission_fault_query);
        admission["prerequisite_refused_micros"].starts_with("(some = ")
            && admission["prerequisite_refusal_detail"]
                == "(some = \"must complete the prerequisite quest first\")"
    });
    let mut sources = node.query_rows(&format!(
        "SELECT guid, x, y, z FROM game_world_entity WHERE guid >= {CREATURE_6} AND guid < {}",
        CREATURE_6 + 10
    ));
    sources.sort_by_key(|row| row["guid"].parse::<u64>().unwrap());
    let staged = serde_json::json!({
        "plan": query_one(node, &format!("SELECT * FROM pkg_playerbots_acceptance_seed_plan WHERE seed = {}", case.seed)),
        "journey": query_one(node, &format!("SELECT * FROM pkg_playerbots_acceptance_journey WHERE character_guid = {guid}")),
        "combat_fault": query_one(node, &format!("SELECT * FROM pkg_playerbots_acceptance_combat_fault WHERE character_guid = {guid}")),
        "inventory_fault": query_one(node, &format!("SELECT * FROM pkg_playerbots_acceptance_inventory_fault WHERE character_guid = {guid}")),
        "missing_target_fault": query_one(node, &format!("SELECT * FROM pkg_playerbots_acceptance_missing_target_fault WHERE character_guid = {guid}")),
        "unreachable_giver_fault": query_one(node, &format!("SELECT * FROM pkg_playerbots_acceptance_unreachable_giver_fault WHERE character_guid = {guid}")),
        "admission_fault": query_one(node, &format!("SELECT * FROM pkg_playerbots_acceptance_admission_fault WHERE character_guid = {guid}")),
        "admission_fault_objective": query_one(node, "SELECT quest_entry, objective_index, kind FROM pkg_playerbots_catalog_objective WHERE quest_entry = 7 AND objective_index = 0"),
        "inventory_fault_template": node.query_rows(&format!("SELECT entry, name, max_stack FROM game_item_template WHERE entry = {INVENTORY_FAULT_FILLER}")),
        "inventory_fillers": node.query_rows(&format!("SELECT guid, entry, owner_guid FROM game_item_instance WHERE owner_guid = {guid} AND entry = {INVENTORY_FAULT_FILLER}")),
        "bot": query_one(node, &format!("SELECT character_guid, class, role, controller, next_think_micros FROM pkg_playerbots_bot WHERE character_guid = {guid}")),
        "character": query_one(node, &format!("SELECT guid, map_id, instance_id, x, y, z, grid_x, grid_y, cell FROM game_world_entity WHERE guid = {guid}")),
        "graveyard": query_one(node, "SELECT id, map_id, x, y, z, name FROM game_graveyard WHERE id = 105"),
        "graveyard_links": node.query_rows("SELECT safe_loc_id, zone_id, faction FROM game_graveyard_zone WHERE safe_loc_id = 105"),
        "sources": sources,
        "catalog": node.query_rows("SELECT * FROM pkg_playerbots_quest_catalog"),
        "named_content": node.query_rows(&format!("SELECT * FROM pkg_playerbots_quest_loop_fixture WHERE character_guid = {guid}")),
        "simple_content": node.query_rows("SELECT * FROM pkg_playerbots_seeded_quest_fixture"),
        "fixture_ownership": node.query_rows("SELECT * FROM pkg_playerbots_quest_fixture_ownership"),
        "navigation": node.query_rows("SELECT nav_enabled, nav_coverage_enabled FROM game_config WHERE id = 0"),
        "nav_chunks": node.query_rows("SELECT key, map_id, cell_x, cell_y, base_z FROM game_nav_chunk"),
        "provisioning": node.query_rows(&format!("SELECT * FROM pkg_playerbots_provisioning WHERE character_guid = {guid}")),
        "runner": node.query_rows(&format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}")),
        "actions": node.query_rows(&format!("SELECT * FROM pkg_playerbots_action WHERE character_guid = {guid}")),
    });
    save(node, "journey-staged", staged.clone());
    assert!(
        prerequisite_refusal_observed,
        "scheduled prerequisite Refusal was not observed before the staged boundary: {staged}"
    );
    (guid, staged)
}

fn assert_staged(node: &Standalone, guid: &str, case: JourneyCase, staged: &serde_json::Value) {
    let plan = &staged["plan"];
    assert_eq!(plan["class"], case.class.to_string(), "{staged}");
    assert_eq!(plan["role"], case.role.to_string(), "{staged}");
    assert_eq!(plan["suffix"], case.suffix.to_string(), "{staged}");
    assert_eq!(
        json_f32(&plan["start_offset_x"]),
        case.start_offset_x,
        "{staged}"
    );
    assert_eq!(
        json_f32(&plan["start_offset_y"]),
        case.start_offset_y,
        "{staged}"
    );
    assert_eq!(
        plan["initial_due_phase_micros"],
        case.initial_due_phase_micros.to_string(),
        "{staged}"
    );
    assert_eq!(
        plan["source_insertion_rotation"],
        case.source_insertion_rotation.to_string(),
        "{staged}"
    );

    let journey = &staged["journey"];
    assert_eq!(journey["seed"], case.seed.to_string(), "{staged}");
    assert!(
        (json_f32(&journey["start_x"]) - (1_200.0 + case.start_offset_x)).abs() < 0.01,
        "{staged}"
    );
    assert!(
        (json_f32(&journey["start_y"]) - (1_200.0 + case.start_offset_y)).abs() < 0.01,
        "{staged}"
    );
    assert_eq!(journey["start_z"], "50", "{staged}");
    assert_eq!(journey["start_instance"], "0", "{staged}");
    assert_eq!(journey["graveyard_id"], "105", "{staged}");
    assert_eq!(
        staged["graveyard"]["id"], journey["graveyard_id"],
        "{staged}"
    );
    assert_eq!(
        staged["graveyard"]["map_id"], journey["start_map"],
        "{staged}"
    );
    for (coordinate, graveyard_field, start_field) in [
        ("x", "graveyard_x", "start_x"),
        ("y", "graveyard_y", "start_y"),
        ("z", "graveyard_z", "start_z"),
    ] {
        assert!(
            (json_f32(&staged["graveyard"][coordinate]) - json_f32(&journey[graveyard_field]))
                .abs()
                < 0.01,
            "{coordinate}: {staged}"
        );
        assert!(
            (json_f32(&staged["graveyard"][coordinate]) - json_f32(&journey[start_field])).abs()
                < 0.01,
            "{coordinate}: {staged}"
        );
    }
    assert_eq!(
        staged["graveyard"]["name"], "Acceptance journey graveyard",
        "{staged}"
    );
    assert!(
        staged["graveyard_links"]
            .as_array()
            .unwrap()
            .iter()
            .any(|link| {
                link["safe_loc_id"] == "105" && link["zone_id"] == "12" && link["faction"] == "469"
            }),
        "{staged}"
    );
    assert_eq!(
        journey["initial_due_phase_micros"],
        case.initial_due_phase_micros.to_string(),
        "{staged}"
    );
    assert_eq!(
        journey["source_insertion_rotation"],
        case.source_insertion_rotation.to_string(),
        "{staged}"
    );
    assert!(journey["journey_started_micros"]
        .as_str()
        .unwrap()
        .contains("none"));
    assert!(journey["first_due_micros"]
        .as_str()
        .unwrap()
        .contains("none"));
    assert!(
        staged["bot"]["controller"]
            .as_str()
            .unwrap()
            .contains("legacy"),
        "{staged}"
    );
    assert_eq!(staged["bot"]["next_think_micros"], i64::MAX.to_string());
    assert_eq!(journey["named_quest_entry"], "7", "{staged}");
    assert_eq!(journey["named_target_entry"], "6", "{staged}");
    assert_eq!(journey["named_target_count"], "10", "{staged}");
    assert_eq!(journey["simple_quest_entry"], "50970", "{staged}");
    assert_eq!(journey["simple_gameobject_entry"], "5090970", "{staged}");

    let expected_insertion: String = (0..10u64)
        .map(|index| CREATURE_6 + (index + u64::from(case.source_insertion_rotation)) % 10)
        .map(|source_guid| source_guid.to_string())
        .collect::<Vec<_>>()
        .concat();
    assert_eq!(
        journey["source_insertion_guids"], expected_insertion,
        "{staged}"
    );
    assert_eq!(staged["sources"].as_array().unwrap().len(), 10, "{staged}");
    for (offset, source) in staged["sources"].as_array().unwrap().iter().enumerate() {
        assert_eq!(
            source["guid"],
            (CREATURE_6 + offset as u64).to_string(),
            "{staged}"
        );
        let x = json_f32(&source["x"]);
        let y = json_f32(&source["y"]);
        assert!(
            (x - (1_360.0 + offset as f32 * 0.4)).abs() < 0.01,
            "{staged}"
        );
        assert!(
            (y - (1_200.0 + (offset % 2) as f32 * 0.4)).abs() < 0.01,
            "{staged}"
        );
    }
    assert!(staged["runner"].as_array().unwrap().is_empty(), "{staged}");
    assert!(staged["actions"].as_array().unwrap().is_empty(), "{staged}");
    assert_staged_faults(staged);

    node.assert_call(
        "playerbots_acceptance_stage_journey",
        &[&case.seed.to_string()],
    );
    let manifest_after_retry = serde_json::to_value(query_one(
        node,
        &format!("SELECT * FROM pkg_playerbots_acceptance_journey WHERE character_guid = {guid}"),
    ))
    .unwrap();
    assert_eq!(
        manifest_after_retry, staged["journey"],
        "{manifest_after_retry:?}"
    );
}

fn assert_staged_faults(staged: &serde_json::Value) {
    let combat_fault = &staged["combat_fault"];
    assert_eq!(combat_fault["quest_entry"], "7", "{staged}");
    assert_eq!(combat_fault["lethal_delay_micros"], "1000000", "{staged}");
    for field in [
        "incoming_attempted_micros",
        "incoming_applied_micros",
        "lethal_attempted_micros",
        "lethal_applied_micros",
        "resurrected_micros",
        "incoming_purpose",
        "incoming_destination",
        "lethal_purpose",
        "lethal_destination",
        "resurrected_purpose",
        "resurrected_destination",
    ] {
        assert!(
            combat_fault[field].as_str().unwrap().contains("none"),
            "{field}: {staged}"
        );
    }
    let unreachable = &staged["unreachable_giver_fault"];
    assert_eq!(unreachable["quest_entry"], "783", "{staged}");
    assert_eq!(unreachable["giver_entry"], "823", "{staged}");
    assert_eq!(
        unreachable["giver_guid"],
        creature_guid(823).to_string(),
        "{staged}"
    );
    assert_eq!(unreachable["alternative_quest_entry"], "40", "{staged}");
    assert_eq!(unreachable["maximum_approach"], "0", "{staged}");
    assert!(
        !unreachable["blocked_navigation_keys"]
            .as_str()
            .unwrap()
            .is_empty(),
        "{staged}"
    );
    for field in [
        "deferred_micros",
        "alternative_micros",
        "alternative_action_observed_micros",
        "restore_due_micros",
        "restore_attempted_micros",
        "restored_micros",
        "resumed_micros",
    ] {
        assert!(
            unreachable[field].as_str().unwrap().contains("none"),
            "{field}: {staged}"
        );
    }
    let admission = &staged["admission_fault"];
    assert_eq!(admission["quest_entry"], "7", "{staged}");
    assert_eq!(admission["prerequisite_quest_entry"], "783", "{staged}");
    assert_eq!(
        admission["supported_alternative_quest_entry"], "5261",
        "{staged}"
    );
    assert!(
        staged["admission_fault_objective"]["kind"]
            .as_str()
            .unwrap()
            .contains("escort"),
        "{staged}"
    );
    assert!(
        admission["prerequisite_refused_micros"]
            .as_str()
            .unwrap()
            .contains("some"),
        "{staged}"
    );
    assert_eq!(
        admission["prerequisite_refusal_detail"],
        "(some = \"must complete the prerequisite quest first\")",
        "{staged}"
    );
    for field in [
        "prerequisite_rewarded_micros",
        "unsupported_observed_micros",
        "alternative_accept_observed_micros",
        "restore_due_micros",
        "restore_attempted_micros",
        "restored_micros",
        "resumed_micros",
    ] {
        assert!(
            admission[field].as_str().unwrap().contains("none"),
            "{field}: {staged}"
        );
    }
    let inventory_fault = &staged["inventory_fault"];
    assert_eq!(inventory_fault["quest_entry"], "33", "{staged}");
    assert_eq!(inventory_fault["item_entry"], "750", "{staged}");
    assert_eq!(
        inventory_fault["release_delay_micros"], "500000",
        "{staged}"
    );
    for field in [
        "fill_attempted_micros",
        "fill_applied_micros",
        "refused_micros",
        "refused_action_observed_micros",
        "space_release_due_micros",
        "space_release_attempted_micros",
        "space_released_micros",
        "resumed_micros",
        "resumed_action_observed_micros",
        "purpose_before_fill",
        "destination_before_fill",
        "purpose_at_refusal",
        "destination_at_refusal",
        "resumed_purpose",
        "resumed_destination",
    ] {
        assert!(
            inventory_fault[field].as_str().unwrap().contains("none"),
            "{field}: {staged}"
        );
    }
    let fault_templates = staged["inventory_fault_template"].as_array().unwrap();
    assert_eq!(fault_templates.len(), 1, "{staged}");
    assert_eq!(
        fault_templates[0]["entry"],
        INVENTORY_FAULT_FILLER.to_string()
    );
    assert_eq!(fault_templates[0]["max_stack"], "1", "{staged}");
    assert!(
        staged["inventory_fillers"].as_array().unwrap().is_empty(),
        "{staged}"
    );
    let missing_target = &staged["missing_target_fault"];
    assert_eq!(missing_target["quest_entry"], "18", "{staged}");
    assert_eq!(missing_target["target_entry"], "38", "{staged}");
    assert_eq!(
        missing_target["target_guid"],
        creature_guid(38).to_string(),
        "{staged}"
    );
    assert_eq!(
        missing_target["respawn_delay_micros"], "60000000",
        "{staged}"
    );
    for field in [
        "removal_attempted_micros",
        "removal_applied_micros",
        "respawn_due_micros",
        "missing_observed_micros",
        "respawned_micros",
        "resumed_micros",
        "removal_character_x",
        "removal_character_y",
        "removal_character_z",
        "removal_target_x",
        "removal_target_y",
        "removal_target_z",
        "removal_target_distance",
        "purpose_before_removal",
        "destination_before_removal",
        "purpose_at_missing",
        "destination_at_missing",
        "resumed_purpose",
        "resumed_destination",
    ] {
        assert!(
            missing_target[field].as_str().unwrap().contains("none"),
            "{field}: {staged}"
        );
    }
}

fn failure_snapshot(
    node: &Standalone,
    guid: &str,
    observation: Option<&JourneyObservation>,
) -> serde_json::Value {
    let observed = observation.map(|state| {
        serde_json::json!({
            "max_x": state.max_x,
            "peak_items": &state.peak_items,
            "dead_quest_targets": &state.dead_quest_targets,
            "loot_sources": &state.loot_sources,
        })
    });
    serde_json::json!({
        "journey": node.query_rows(&format!("SELECT * FROM pkg_playerbots_acceptance_journey WHERE character_guid = {guid}")),
        "combat_fault": node.query_rows(&format!("SELECT * FROM pkg_playerbots_acceptance_combat_fault WHERE character_guid = {guid}")),
        "inventory_fault": node.query_rows(&format!("SELECT * FROM pkg_playerbots_acceptance_inventory_fault WHERE character_guid = {guid}")),
        "missing_target_fault": node.query_rows(&format!("SELECT * FROM pkg_playerbots_acceptance_missing_target_fault WHERE character_guid = {guid}")),
        "unreachable_giver_fault": node.query_rows(&format!("SELECT * FROM pkg_playerbots_acceptance_unreachable_giver_fault WHERE character_guid = {guid}")),
        "admission_fault": node.query_rows(&format!("SELECT * FROM pkg_playerbots_acceptance_admission_fault WHERE character_guid = {guid}")),
        "seed_plan": node.query_rows("SELECT * FROM pkg_playerbots_acceptance_seed_plan"),
        "bot": node.query_rows(&format!("SELECT * FROM pkg_playerbots_bot WHERE character_guid = {guid}")),
        "world_entity": node.query_rows(&format!("SELECT * FROM game_world_entity WHERE guid = {guid}")),
        "graveyard": node.query_rows("SELECT * FROM game_graveyard WHERE id = 105"),
        "graveyard_links": node.query_rows("SELECT * FROM game_graveyard_zone WHERE safe_loc_id = 105"),
        "corpse": node.query_rows(&format!("SELECT * FROM game_corpse WHERE owner_guid = {guid}")),
        "runner": node.query_rows(&format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}")),
        "provisioning": node.query_rows(&format!("SELECT * FROM pkg_playerbots_provisioning WHERE character_guid = {guid}")),
        "retained": node.query_rows(&format!("SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}")),
        "quests": node.query_rows(&format!("SELECT * FROM game_character_quest WHERE character_guid = {guid}")),
        "actions": node.query_rows(&format!("SELECT * FROM pkg_playerbots_action WHERE character_guid = {guid}")),
        "turnins": node.query_rows(&format!("SELECT * FROM pkg_playerbots_quest_turnin_fixture WHERE character_guid = {guid}")),
        "items": node.query_rows(&format!("SELECT * FROM game_item_instance WHERE owner_guid = {guid}")),
        "named_targets": node.query_rows(&format!("SELECT * FROM game_world_entity WHERE guid >= {CREATURE_6} AND guid < {}", CREATURE_6 + 10)),
        "fixture_entities": node.query_rows("SELECT * FROM game_world_entity WHERE entry = 38 OR entry = 69 OR entry = 299 OR entry = 823 OR entry = 197 OR entry = 196 OR entry = 9296 OR entry = 952 OR entry = 241 OR entry = 240 OR entry = 261"),
        "fixture_gameobjects": node.query_rows("SELECT * FROM game_gameobject WHERE template_entry = 55 OR template_entry = 56 OR template_entry = 161557 OR template_entry = 5090970"),
        "movement": node.query_rows(&format!("SELECT * FROM game_creature_spline WHERE guid = {guid}")),
        "pending_cast": node.query_rows(&format!("SELECT * FROM game_pending_cast WHERE caster_guid = {guid}")),
        "melee": node.query_rows(&format!("SELECT * FROM game_melee_attack WHERE attacker_guid = {guid}")),
        "melee_schedule": node.query_rows("SELECT * FROM game_melee_schedule"),
        "scheduler": node.query_rows("SELECT * FROM pkg_playerbots_scheduler"),
        "navigation_config": node.query_rows("SELECT nav_enabled, nav_coverage_enabled FROM game_config WHERE id = 0"),
        "navigation_revision": node.query_rows("SELECT * FROM game_navigation_revision"),
        "nav_chunks": node.query_rows("SELECT key, map_id, cell_x, cell_y, base_z FROM game_nav_chunk WHERE map_id = 0"),
        "coverage_manifest": node.query_rows("SELECT * FROM game_vmap_nav_coverage_manifest WHERE map_id = 0"),
        "observed": observed,
    })
}

fn assert_combat_fault_complete(complete: &serde_json::Value) {
    let fault = &complete["combat_fault"];
    assert_eq!(fault["quest_entry"], "7", "{complete}");
    let incoming_attempted_at =
        structured_number(fault["incoming_attempted_micros"].as_str().unwrap(), "some");
    let incoming_at = structured_number(fault["incoming_applied_micros"].as_str().unwrap(), "some");
    let lethal_attempted_at =
        structured_number(fault["lethal_attempted_micros"].as_str().unwrap(), "some");
    let lethal_at = structured_number(fault["lethal_applied_micros"].as_str().unwrap(), "some");
    let resurrected_at = structured_number(fault["resurrected_micros"].as_str().unwrap(), "some");
    assert_eq!(incoming_attempted_at, incoming_at, "{complete}");
    assert_eq!(lethal_attempted_at, lethal_at, "{complete}");
    assert!(lethal_at >= incoming_attempted_at + 1_000_000, "{complete}");
    assert!(resurrected_at > lethal_at, "{complete}");

    let incoming_before =
        structured_number(fault["incoming_health_before"].as_str().unwrap(), "some");
    let incoming_after =
        structured_number(fault["incoming_health_after"].as_str().unwrap(), "some");
    assert!(
        incoming_before > incoming_after && incoming_after > 0,
        "{complete}"
    );
    assert!(
        structured_number(fault["lethal_health_before"].as_str().unwrap(), "some") > 0,
        "{complete}"
    );
    assert_eq!(
        structured_number(fault["lethal_health_after"].as_str().unwrap(), "some"),
        0,
        "{complete}"
    );
    assert!(
        structured_number(fault["resurrected_health"].as_str().unwrap(), "some") > 0,
        "{complete}"
    );
    assert_eq!(
        structured_number(fault["resurrected_map"].as_str().unwrap(), "some"),
        complete["journey"]["start_map"]
            .as_str()
            .unwrap()
            .parse::<i64>()
            .unwrap(),
        "{complete}"
    );
    assert_eq!(
        structured_number(fault["resurrected_instance"].as_str().unwrap(), "some"),
        complete["journey"]["start_instance"]
            .as_str()
            .unwrap()
            .parse::<i64>()
            .unwrap(),
        "{complete}"
    );
    for (coordinate, fault_field, graveyard_field) in [
        ("x", "resurrected_x", "graveyard_x"),
        ("y", "resurrected_y", "graveyard_y"),
        ("z", "resurrected_z", "graveyard_z"),
    ] {
        assert!(
            (structured_f32(fault[fault_field].as_str().unwrap(), "some")
                - json_f32(&complete["journey"][graveyard_field]))
            .abs()
                < 0.01,
            "{coordinate}: {complete}"
        );
    }
    let journey_sources: BTreeSet<u64> = complete["named_targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|source| source["guid"].as_str().unwrap().parse().unwrap())
        .collect();
    for attacker_field in ["incoming_attacker_guid", "lethal_attacker_guid"] {
        let attacker = structured_u64(fault[attacker_field].as_str().unwrap(), "some");
        assert!(
            (CREATURE_6..CREATURE_6 + 10).contains(&attacker),
            "{attacker_field}: {complete}"
        );
        assert!(
            journey_sources.contains(&attacker),
            "{attacker_field}: {complete}"
        );
    }
    let incoming_credit =
        structured_number(fault["incoming_credit_before"].as_str().unwrap(), "some");
    assert_eq!(
        structured_number(fault["incoming_credit_after"].as_str().unwrap(), "some"),
        incoming_credit,
        "{complete}"
    );
    let lethal_credit = structured_number(fault["lethal_credit_before"].as_str().unwrap(), "some");
    assert_eq!(
        structured_number(fault["lethal_credit_after"].as_str().unwrap(), "some"),
        lethal_credit,
        "{complete}"
    );
    assert_eq!(
        structured_number(fault["resurrected_credit"].as_str().unwrap(), "some"),
        lethal_credit,
        "{complete}"
    );
    assert!((0..10).contains(&incoming_credit), "{complete}");
    assert!((0..10).contains(&lethal_credit), "{complete}");
    let incoming_identity = structured_number(
        fault["incoming_objective_identity_before"]
            .as_str()
            .unwrap(),
        "some",
    );
    assert_eq!(
        structured_number(
            fault["incoming_objective_identity_after"].as_str().unwrap(),
            "some"
        ),
        incoming_identity,
        "{complete}"
    );
    assert_quest_destination(
        fault,
        "incoming_purpose",
        "incoming_destination",
        197,
        creature_guid(197),
        complete,
    );
    let lethal_identity = structured_number(
        fault["lethal_objective_identity_before"].as_str().unwrap(),
        "some",
    );
    assert_ne!(lethal_identity, incoming_identity, "{complete}");
    assert_eq!(
        structured_number(
            fault["lethal_objective_identity_after"].as_str().unwrap(),
            "some"
        ),
        lethal_identity,
        "{complete}"
    );
    assert_quest_destination(
        fault,
        "lethal_purpose",
        "lethal_destination",
        6,
        creature_guid(6),
        complete,
    );
    assert_eq!(
        structured_number(
            fault["resurrected_objective_identity"].as_str().unwrap(),
            "some"
        ),
        lethal_identity,
        "{complete}"
    );
    assert_quest_destination(
        fault,
        "resurrected_purpose",
        "resurrected_destination",
        6,
        creature_guid(6),
        complete,
    );
    assert_eq!(
        fault["lethal_destination"], fault["resurrected_destination"],
        "{complete}"
    );
}

fn assert_quest_destination(
    fault: &serde_json::Value,
    purpose_field: &str,
    destination_field: &str,
    entry: u32,
    guid: u64,
    complete: &serde_json::Value,
) {
    assert!(
        fault[purpose_field].as_str().unwrap().contains("quest"),
        "{purpose_field}: {complete}"
    );
    let destination = fault[destination_field].as_str().unwrap();
    assert!(
        destination.contains("creature"),
        "{destination_field}: {complete}"
    );
    assert_eq!(
        structured_number(destination, "entry"),
        i64::from(entry),
        "{destination_field}: {complete}"
    );
    assert_eq!(
        structured_u64(destination, "guid"),
        guid,
        "{destination_field}: {complete}"
    );
    assert_eq!(
        structured_number(destination, "map_id"),
        0,
        "{destination_field}: {complete}"
    );
    assert_eq!(
        structured_u64(destination, "instance_id"),
        0,
        "{destination_field}: {complete}"
    );
}

fn assert_inventory_fault_complete(complete: &serde_json::Value) {
    let fault = &complete["inventory_fault"];
    assert_eq!(fault["quest_entry"], "33", "{complete}");
    assert_eq!(fault["item_entry"], "750", "{complete}");
    for error_field in ["fill_error", "space_release_error"] {
        assert!(
            fault[error_field].as_str().unwrap().contains("none"),
            "{error_field}: {complete}"
        );
    }
    let filled_at = structured_number(fault["fill_applied_micros"].as_str().unwrap(), "some");
    assert_eq!(
        structured_number(fault["fill_attempted_micros"].as_str().unwrap(), "some"),
        filled_at,
        "{complete}"
    );
    let refused_at = structured_number(fault["refused_micros"].as_str().unwrap(), "some");
    let refused_action_at = structured_number(
        fault["refused_action_observed_micros"].as_str().unwrap(),
        "some",
    );
    assert!(refused_action_at <= refused_at, "{complete}");
    let released_at = structured_number(fault["space_released_micros"].as_str().unwrap(), "some");
    let release_due =
        structured_number(fault["space_release_due_micros"].as_str().unwrap(), "some");
    assert_eq!(
        release_due,
        refused_at
            + fault["release_delay_micros"]
                .as_str()
                .unwrap()
                .parse::<i64>()
                .unwrap(),
        "{complete}"
    );
    assert_eq!(
        structured_number(
            fault["space_release_attempted_micros"].as_str().unwrap(),
            "some"
        ),
        released_at,
        "{complete}"
    );
    let resumed_at = structured_number(fault["resumed_micros"].as_str().unwrap(), "some");
    let resumed_action_at = structured_number(
        fault["resumed_action_observed_micros"].as_str().unwrap(),
        "some",
    );
    assert!(resumed_action_at <= resumed_at, "{complete}");
    assert!(
        filled_at < refused_at
            && refused_at < release_due
            && release_due <= released_at
            && released_at < resumed_at
    );

    assert_eq!(
        structured_number(fault["item_count_before_fill"].as_str().unwrap(), "some"),
        0,
        "{complete}"
    );
    assert_eq!(
        structured_number(fault["item_count_at_refusal"].as_str().unwrap(), "some"),
        0,
        "{complete}"
    );
    assert!(
        structured_number(fault["resumed_item_count"].as_str().unwrap(), "some") > 0,
        "{complete}"
    );
    let credit = structured_number(fault["credit_before_fill"].as_str().unwrap(), "some");
    assert_eq!(
        structured_number(fault["credit_at_refusal"].as_str().unwrap(), "some"),
        credit,
        "{complete}"
    );
    assert_eq!(
        structured_number(fault["resumed_credit"].as_str().unwrap(), "some"),
        credit,
        "{complete}"
    );
    let available_identity = structured_number(
        fault["objective_identity_before_fill"].as_str().unwrap(),
        "some",
    );
    let held_identity = structured_number(
        fault["objective_identity_at_refusal"].as_str().unwrap(),
        "some",
    );
    assert_ne!(held_identity, available_identity, "{complete}");
    assert_eq!(
        structured_number(
            fault["resumed_objective_identity"].as_str().unwrap(),
            "some"
        ),
        held_identity,
        "{complete}"
    );
    let refused_source = structured_u64(fault["refused_source_guid"].as_str().unwrap(), "some");
    assert!(
        [creature_guid(299), creature_guid(69)].contains(&refused_source),
        "{complete}"
    );
    let refused_entry = if refused_source == creature_guid(299) {
        299
    } else {
        69
    };
    assert_quest_destination(
        fault,
        "purpose_before_fill",
        "destination_before_fill",
        196,
        creature_guid(196),
        complete,
    );
    assert_quest_destination(
        fault,
        "purpose_at_refusal",
        "destination_at_refusal",
        refused_entry,
        refused_source,
        complete,
    );
    assert_quest_destination(
        fault,
        "resumed_purpose",
        "resumed_destination",
        refused_entry,
        refused_source,
        complete,
    );
    assert_eq!(
        fault["destination_at_refusal"], fault["resumed_destination"],
        "{complete}"
    );
    assert_eq!(
        structured_u64(fault["resumed_source_guid"].as_str().unwrap(), "some"),
        refused_source,
        "{complete}"
    );
    assert_eq!(
        structured_number(fault["refused_loot_slot"].as_str().unwrap(), "some"),
        0,
        "{complete}"
    );
    let retry_observed =
        structured_number(fault["retry_observed_micros"].as_str().unwrap(), "some");
    assert!(
        structured_number(fault["retry_after_micros"].as_str().unwrap(), "some") > retry_observed,
        "{complete}"
    );
    assert!(
        resumed_action_at
            >= structured_number(fault["retry_after_micros"].as_str().unwrap(), "some"),
        "{complete}"
    );
    assert!((1..=3).contains(&structured_number(
        fault["retry_count_at_refusal"].as_str().unwrap(),
        "some"
    )));
}

fn assert_missing_target_fault_complete(complete: &serde_json::Value) {
    let fault = &complete["missing_target_fault"];
    assert_eq!(fault["quest_entry"], "18", "{complete}");
    assert_eq!(fault["target_entry"], "38", "{complete}");
    let target_guid = creature_guid(38);
    assert_eq!(fault["target_guid"], target_guid.to_string(), "{complete}");
    assert!(
        fault["removal_error"].as_str().unwrap().contains("none"),
        "{complete}"
    );
    let removed_at = structured_number(fault["removal_applied_micros"].as_str().unwrap(), "some");
    assert_eq!(
        structured_number(fault["removal_attempted_micros"].as_str().unwrap(), "some"),
        removed_at,
        "{complete}"
    );
    let missing_at = structured_number(fault["missing_observed_micros"].as_str().unwrap(), "some");
    let respawn_due = structured_number(fault["respawn_due_micros"].as_str().unwrap(), "some");
    let respawn_delay = fault["respawn_delay_micros"]
        .as_str()
        .unwrap()
        .parse::<i64>()
        .unwrap();
    assert_eq!(respawn_delay, 60_000_000, "{complete}");
    assert_eq!(respawn_due, removed_at + respawn_delay, "{complete}");
    let character_x = structured_f32(fault["removal_character_x"].as_str().unwrap(), "some");
    let character_y = structured_f32(fault["removal_character_y"].as_str().unwrap(), "some");
    let character_z = structured_f32(fault["removal_character_z"].as_str().unwrap(), "some");
    let target_x = structured_f32(fault["removal_target_x"].as_str().unwrap(), "some");
    let target_y = structured_f32(fault["removal_target_y"].as_str().unwrap(), "some");
    let target_z = structured_f32(fault["removal_target_z"].as_str().unwrap(), "some");
    let removal_distance =
        structured_f32(fault["removal_target_distance"].as_str().unwrap(), "some");
    let recorded_distance = ((character_x - target_x).powi(2)
        + (character_y - target_y).powi(2)
        + (character_z - target_z).powi(2))
    .sqrt();
    assert!(
        (removal_distance - recorded_distance).abs() < 0.01,
        "{complete}"
    );
    assert!(removal_distance > 2.05, "{complete}");
    let respawned_at = structured_number(fault["respawned_micros"].as_str().unwrap(), "some");
    let resumed_at = structured_number(fault["resumed_micros"].as_str().unwrap(), "some");
    assert!(
        removed_at < missing_at
            && missing_at < respawn_due
            && respawn_due <= respawned_at
            && respawned_at <= resumed_at,
        "{complete}"
    );
    assert!(
        fault["missing_failure"]
            .as_str()
            .unwrap()
            .contains("questTargetMissing"),
        "{complete}"
    );
    assert!((1..=3).contains(&structured_number(
        fault["missing_retry_count"].as_str().unwrap(),
        "some"
    )));
    assert!(
        structured_number(
            fault["missing_retry_after_micros"].as_str().unwrap(),
            "some"
        ) > missing_at,
        "{complete}"
    );
    if fault["attack_target_at_missing"]
        .as_str()
        .unwrap()
        .contains("some")
    {
        let stale_target =
            structured_u64(fault["attack_target_at_missing"].as_str().unwrap(), "some");
        let stale_observed = structured_number(
            fault["attack_observed_micros_at_missing"].as_str().unwrap(),
            "some",
        );
        assert!(
            stale_target != target_guid || stale_observed < removed_at,
            "{complete}"
        );
    }
    assert!(
        structured_number(fault["respawned_health"].as_str().unwrap(), "some") > 0,
        "{complete}"
    );
    assert!(
        structured_number(fault["respawned_life_seq"].as_str().unwrap(), "some")
            > structured_number(fault["removed_life_seq"].as_str().unwrap(), "some"),
        "{complete}"
    );
    assert_eq!(
        structured_u64(fault["resumed_attack_target"].as_str().unwrap(), "some"),
        target_guid,
        "{complete}"
    );
    let credit = structured_number(fault["credit_before_removal"].as_str().unwrap(), "some");
    for field in ["credit_at_missing", "resumed_credit"] {
        assert_eq!(
            structured_number(fault[field].as_str().unwrap(), "some"),
            credit,
            "{field}: {complete}"
        );
    }
    let available_identity = structured_number(
        fault["objective_identity_before_removal"].as_str().unwrap(),
        "some",
    );
    let held_identity = structured_number(
        fault["objective_identity_at_missing"].as_str().unwrap(),
        "some",
    );
    assert_ne!(held_identity, available_identity, "{complete}");
    assert_eq!(
        structured_number(
            fault["resumed_objective_identity"].as_str().unwrap(),
            "some"
        ),
        held_identity,
        "{complete}"
    );
    assert_quest_destination(
        fault,
        "purpose_before_removal",
        "destination_before_removal",
        823,
        creature_guid(823),
        complete,
    );
    assert_quest_destination(
        fault,
        "purpose_at_missing",
        "destination_at_missing",
        38,
        target_guid,
        complete,
    );
    assert_quest_destination(
        fault,
        "resumed_purpose",
        "resumed_destination",
        38,
        target_guid,
        complete,
    );
    assert_eq!(
        fault["destination_at_missing"], fault["resumed_destination"],
        "{complete}"
    );
}

fn assert_unreachable_giver_fault_complete(complete: &serde_json::Value) {
    let fault = &complete["unreachable_giver_fault"];
    assert_eq!(fault["quest_entry"], "783", "{complete}");
    assert_eq!(fault["giver_entry"], "823", "{complete}");
    assert_eq!(
        fault["giver_guid"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap(),
        creature_guid(823),
        "{complete}"
    );
    assert_eq!(fault["alternative_quest_entry"], "40", "{complete}");
    assert!(
        fault["maximum_approach"]
            .as_str()
            .unwrap()
            .parse::<u8>()
            .unwrap()
            >= 2,
        "{complete}"
    );
    let deferred = structured_number(fault["deferred_micros"].as_str().unwrap(), "some");
    let deferred_until =
        structured_number(fault["deferred_until_micros"].as_str().unwrap(), "some");
    let alternative = structured_number(fault["alternative_micros"].as_str().unwrap(), "some");
    let restore_due = structured_number(fault["restore_due_micros"].as_str().unwrap(), "some");
    let restore_attempted =
        structured_number(fault["restore_attempted_micros"].as_str().unwrap(), "some");
    let restored = structured_number(fault["restored_micros"].as_str().unwrap(), "some");
    let resumed = structured_number(fault["resumed_micros"].as_str().unwrap(), "some");
    assert_eq!(deferred_until - deferred, 30_000_000, "{complete}");
    assert!(
        alternative >= deferred && alternative < deferred_until,
        "{complete}"
    );
    let alternative_action = structured_number(
        fault["alternative_action_observed_micros"]
            .as_str()
            .unwrap(),
        "some",
    );
    assert!(
        alternative_action >= deferred && alternative_action < deferred_until,
        "{complete}"
    );
    assert_eq!(restore_due, deferred_until, "{complete}");
    assert!(restore_attempted >= restore_due, "{complete}");
    assert!(restored >= restore_attempted, "{complete}");
    assert!(
        resumed > restored && resumed >= deferred_until,
        "{complete}"
    );
    assert!(
        structured_number(
            fault["restored_navigation_revision"].as_str().unwrap(),
            "some"
        ) > fault["blocked_navigation_revision"]
            .as_str()
            .unwrap()
            .parse::<i64>()
            .unwrap(),
        "{complete}"
    );
    assert!(
        fault["restore_error"].as_str().unwrap().contains("none"),
        "{complete}"
    );
    assert_ne!(
        structured_number(
            fault["alternative_objective_identity"].as_str().unwrap(),
            "some"
        ),
        structured_number(
            fault["resumed_objective_identity"].as_str().unwrap(),
            "some"
        ),
        "{complete}"
    );
    assert_eq!(
        structured_u64(fault["alternative_target_guid"].as_str().unwrap(), "some"),
        creature_guid(241),
        "{complete}"
    );
    for field in [
        "quest_id_at_deferral",
        "credit_at_deferral",
        "quest_id_at_resume",
        "credit_at_resume",
    ] {
        assert!(
            fault[field].as_str().unwrap().contains("none"),
            "{field}: {complete}"
        );
    }
}

fn assert_admission_fault_complete(complete: &serde_json::Value) {
    let fault = &complete["admission_fault"];
    assert_eq!(fault["quest_entry"], "7", "{complete}");
    assert_eq!(fault["prerequisite_quest_entry"], "783", "{complete}");
    assert_eq!(
        fault["supported_alternative_quest_entry"], "5261",
        "{complete}"
    );
    let prerequisite_refused = structured_number(
        fault["prerequisite_refused_micros"].as_str().unwrap(),
        "some",
    );
    let prerequisite_rewarded = structured_number(
        fault["prerequisite_rewarded_micros"].as_str().unwrap(),
        "some",
    );
    let unsupported = structured_number(
        fault["unsupported_observed_micros"].as_str().unwrap(),
        "some",
    );
    let alternative = structured_number(
        fault["alternative_accept_observed_micros"]
            .as_str()
            .unwrap(),
        "some",
    );
    let restore_due = structured_number(fault["restore_due_micros"].as_str().unwrap(), "some");
    let restored = structured_number(fault["restored_micros"].as_str().unwrap(), "some");
    let resumed = structured_number(fault["resumed_micros"].as_str().unwrap(), "some");
    assert!(prerequisite_rewarded > prerequisite_refused, "{complete}");
    assert!(unsupported >= prerequisite_rewarded, "{complete}");
    assert!(alternative >= unsupported, "{complete}");
    assert!(restore_due >= alternative + 500_000, "{complete}");
    assert!(
        structured_number(fault["restore_attempted_micros"].as_str().unwrap(), "some")
            >= restore_due,
        "{complete}"
    );
    assert!(restored >= restore_due, "{complete}");
    assert!(resumed >= restored, "{complete}");
    assert_eq!(
        fault["prerequisite_refusal_detail"],
        "(some = \"must complete the prerequisite quest first\")",
        "{complete}"
    );
    assert_eq!(
        structured_number(fault["prerequisite_turnin_count"].as_str().unwrap(), "some"),
        1,
        "{complete}"
    );
    assert!(
        fault["unsupported_capability"]
            .as_str()
            .unwrap()
            .contains("escort"),
        "{complete}"
    );
    assert_eq!(
        structured_number(
            fault["selected_alternative_quest"].as_str().unwrap(),
            "some"
        ),
        5261,
        "{complete}"
    );
    assert_eq!(
        structured_u64(
            fault["alternative_accept_target_guid"].as_str().unwrap(),
            "some"
        ),
        creature_guid(823),
        "{complete}"
    );
    assert_eq!(
        structured_u64(
            fault["resumed_accept_target_guid"].as_str().unwrap(),
            "some"
        ),
        creature_guid(197),
        "{complete}"
    );
    assert_eq!(
        structured_number(fault["resumed_credit"].as_str().unwrap(), "some"),
        0,
        "{complete}"
    );
    for field in [
        "quest_id_at_prerequisite_refusal",
        "quest_id_at_unsupported_refusal",
    ] {
        assert!(
            fault[field].as_str().unwrap().contains("none"),
            "{field}: {complete}"
        );
    }
    assert!(
        fault["restore_error"].as_str().unwrap().contains("none"),
        "{complete}"
    );
}

fn begin_and_observe_first_decision(
    node: &Standalone,
    guid: &str,
    case: JourneyCase,
) -> serde_json::Value {
    node.assert_call("playerbots_acceptance_begin_journey", &[guid]);
    let begun = serde_json::json!({
        "journey": query_one(node, &format!("SELECT * FROM pkg_playerbots_acceptance_journey WHERE character_guid = {guid}")),
        "combat_fault": query_one(node, &format!("SELECT * FROM pkg_playerbots_acceptance_combat_fault WHERE character_guid = {guid}")),
        "inventory_fault": query_one(node, &format!("SELECT * FROM pkg_playerbots_acceptance_inventory_fault WHERE character_guid = {guid}")),
        "missing_target_fault": query_one(node, &format!("SELECT * FROM pkg_playerbots_acceptance_missing_target_fault WHERE character_guid = {guid}")),
        "unreachable_giver_fault": query_one(node, &format!("SELECT * FROM pkg_playerbots_acceptance_unreachable_giver_fault WHERE character_guid = {guid}")),
        "admission_fault": query_one(node, &format!("SELECT * FROM pkg_playerbots_acceptance_admission_fault WHERE character_guid = {guid}")),
        "bot": query_one(node, &format!("SELECT character_guid, class, role, controller, next_think_micros FROM pkg_playerbots_bot WHERE character_guid = {guid}")),
        "runner": query_one(node, &format!("SELECT character_guid, observed_micros, objective_sequence, objective, foreground, chosen FROM pkg_playerbots_runner WHERE character_guid = {guid}")),
    });
    save(node, "journey-begun", begun.clone());
    let started_micros = structured_number(
        begun["journey"]["journey_started_micros"].as_str().unwrap(),
        "some",
    );
    let first_due_micros = structured_number(
        begun["journey"]["first_due_micros"].as_str().unwrap(),
        "some",
    );
    assert_eq!(
        first_due_micros - started_micros,
        case.initial_due_phase_micros,
        "{begun}"
    );
    let begun_observed_micros = begun["runner"]["observed_micros"]
        .as_str()
        .unwrap()
        .parse::<i64>()
        .unwrap();
    if begun_observed_micros > started_micros && begun_observed_micros < first_due_micros {
        save(
            node,
            "early-first-decision",
            failure_snapshot(node, guid, None),
        );
        panic!("runner made a decision before the declared first due time");
    }
    if begun_observed_micros <= started_micros {
        assert_eq!(
            begun["bot"]["next_think_micros"],
            first_due_micros.to_string(),
            "{begun}"
        );
    }
    assert!(
        begun["bot"]["controller"]
            .as_str()
            .unwrap()
            .contains("cohort"),
        "{begun}"
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    let first_decision = loop {
        let runner = query_one(
            node,
            &format!(
                "SELECT observed_micros, objective_sequence, objective, chosen, last_outcome FROM pkg_playerbots_runner WHERE character_guid = {guid}"
            ),
        );
        let observed_micros = runner["observed_micros"].parse::<i64>().unwrap();
        if observed_micros > started_micros && observed_micros < first_due_micros {
            save(
                node,
                "early-first-decision",
                failure_snapshot(node, guid, None),
            );
            panic!("runner made a decision before the declared first due time");
        }
        if observed_micros >= first_due_micros && observed_micros > started_micros {
            break runner;
        }
        if Instant::now() >= deadline {
            save(
                node,
                "first-decision-timeout",
                failure_snapshot(node, guid, None),
            );
            panic!("declared first due time did not produce a scheduled decision");
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    save(
        node,
        "first-scheduled-decision",
        serde_json::to_value(&first_decision).unwrap(),
    );
    begun
}

struct JourneyObservation {
    max_x: f32,
    peak_items: BTreeMap<u32, u32>,
    dead_quest_targets: BTreeSet<u64>,
    loot_sources: BTreeMap<u32, u64>,
}

fn observe_until_complete(node: &Standalone, guid: &str) -> JourneyObservation {
    let mut observation = JourneyObservation {
        max_x: f32::NEG_INFINITY,
        peak_items: BTreeMap::new(),
        dead_quest_targets: BTreeSet::new(),
        loot_sources: BTreeMap::new(),
    };
    let deadline = Instant::now() + JOURNEY_TIMEOUT;
    loop {
        if let Some(character) = node
            .query_rows(&format!(
                "SELECT x FROM game_world_entity WHERE guid = {guid}"
            ))
            .into_iter()
            .next()
        {
            observation.max_x = observation
                .max_x
                .max(character["x"].parse::<f32>().unwrap());
        }
        for entry in CARRIED_ITEMS {
            let count = item_count(node, guid, entry);
            observation
                .peak_items
                .entry(entry)
                .and_modify(|peak| *peak = (*peak).max(count))
                .or_insert(count);
        }
        for target in node.query_rows(&format!(
            "SELECT guid, dead FROM game_world_entity WHERE guid >= {CREATURE_6} AND guid < {}",
            CREATURE_6 + 10
        )) {
            if target["dead"] == "true" {
                observation
                    .dead_quest_targets
                    .insert(target["guid"].parse().unwrap());
            }
        }
        for action in node.query_rows(&format!(
            "SELECT kind, target_guid, quest_entry, outcome FROM pkg_playerbots_action WHERE character_guid = {guid}"
        )) {
            if action["kind"].contains("takeLoot") && action["outcome"].contains("completed") {
                let quest = action["quest_entry"].parse::<u32>().unwrap();
                if matches!(quest, 33 | 18 | 3904) {
                    observation
                        .loot_sources
                        .entry(quest)
                        .or_insert_with(|| action["target_guid"].parse().unwrap());
                }
            }
        }
        if CATALOG_QUESTS
            .iter()
            .all(|quest| rewarded(node, guid, *quest))
        {
            return observation;
        }
        if Instant::now() >= deadline {
            save(
                node,
                "whole-journey-timeout",
                failure_snapshot(node, guid, Some(&observation)),
            );
            panic!("seeded acceptance journey timed out");
        }
        std::thread::sleep(OBSERVATION_INTERVAL);
    }
}

fn assert_complete(
    node: &Standalone,
    guid: &str,
    begun: &serde_json::Value,
    observation: &JourneyObservation,
) {
    let complete = serde_json::json!({
        "journey": query_one(node, &format!("SELECT * FROM pkg_playerbots_acceptance_journey WHERE character_guid = {guid}")),
        "combat_fault": query_one(node, &format!("SELECT * FROM pkg_playerbots_acceptance_combat_fault WHERE character_guid = {guid}")),
        "inventory_fault": query_one(node, &format!("SELECT * FROM pkg_playerbots_acceptance_inventory_fault WHERE character_guid = {guid}")),
        "missing_target_fault": query_one(node, &format!("SELECT * FROM pkg_playerbots_acceptance_missing_target_fault WHERE character_guid = {guid}")),
        "unreachable_giver_fault": query_one(node, &format!("SELECT * FROM pkg_playerbots_acceptance_unreachable_giver_fault WHERE character_guid = {guid}")),
        "admission_fault": query_one(node, &format!("SELECT * FROM pkg_playerbots_acceptance_admission_fault WHERE character_guid = {guid}")),
        "bot": query_one(node, &format!("SELECT * FROM pkg_playerbots_bot WHERE character_guid = {guid}")),
        "world_entity": node.query_rows(&format!("SELECT * FROM game_world_entity WHERE guid = {guid}")),
        "graveyard": node.query_rows("SELECT * FROM game_graveyard WHERE id = 105"),
        "graveyard_links": node.query_rows("SELECT * FROM game_graveyard_zone WHERE safe_loc_id = 105"),
        "corpse": node.query_rows(&format!("SELECT * FROM game_corpse WHERE owner_guid = {guid}")),
        "runner": query_one(node, &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}")),
        "provisioning": query_one(node, &format!("SELECT * FROM pkg_playerbots_provisioning WHERE character_guid = {guid}")),
        "quests": node.query_rows(&format!("SELECT * FROM game_character_quest WHERE character_guid = {guid}")),
        "actions": node.query_rows(&format!("SELECT * FROM pkg_playerbots_action WHERE character_guid = {guid}")),
        "turnins": node.query_rows(&format!("SELECT * FROM pkg_playerbots_quest_turnin_fixture WHERE character_guid = {guid}")),
        "loot_receipts": node.query_rows(&format!("SELECT * FROM pkg_playerbots_quest_loot_receipt_fixture WHERE character_guid = {guid}")),
        "simple_gameobject_use_receipts": node.query_rows(&format!("SELECT * FROM pkg_playerbots_acceptance_gameobject_use_receipt WHERE character_guid = {guid}")),
        "items": node.query_rows(&format!("SELECT * FROM game_item_instance WHERE owner_guid = {guid}")),
        "named_targets": node.query_rows(&format!("SELECT * FROM game_world_entity WHERE guid >= {CREATURE_6} AND guid < {}", CREATURE_6 + 10)),
        "fixture_gameobjects": node.query_rows("SELECT * FROM game_gameobject WHERE template_entry = 161557 OR template_entry = 5090970"),
        "peak_items": &observation.peak_items,
        "dead_quest_targets": &observation.dead_quest_targets,
        "loot_sources": &observation.loot_sources,
        "max_x": observation.max_x,
    });
    save(node, "whole-journey-complete", complete.clone());
    assert_eq!(complete["journey"], begun["journey"], "{complete}");
    assert_combat_fault_complete(&complete);
    assert_inventory_fault_complete(&complete);
    assert_missing_target_fault_complete(&complete);
    assert_unreachable_giver_fault_complete(&complete);
    assert_admission_fault_complete(&complete);
    assert!(observation.max_x > 1_350.0, "{complete}");
    assert!(
        complete["provisioning"]["history"]
            .as_str()
            .unwrap()
            .to_ascii_lowercase()
            .contains("applied"),
        "{complete}"
    );
    for quest in CATALOG_QUESTS {
        assert!(rewarded(node, guid, quest), "quest {quest}: {complete}");
        assert_eq!(
            turnin_count(node, guid, quest),
            1,
            "quest {quest}: {complete}"
        );
    }
    let expected_dead: BTreeSet<_> = (CREATURE_6..CREATURE_6 + 10).collect();
    assert_eq!(observation.dead_quest_targets, expected_dead, "{complete}");
    let loot_receipt = complete["loot_receipts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["item_entry"] == "750")
        .expect("creature loot receipt missing");
    let creature_loot_source = loot_receipt["last_source_guid"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert!(
        [creature_guid(299), creature_guid(69)].contains(&creature_loot_source),
        "{complete}"
    );
    assert_eq!(loot_receipt["received_count"], "8", "{complete}");
    assert_eq!(loot_receipt["peak_carried_count"], "8", "{complete}");
    assert_eq!(
        observation.loot_sources.get(&18),
        Some(&creature_guid(38)),
        "{complete}"
    );
    assert_eq!(
        observation.loot_sources.get(&3904),
        Some(&CHEST_GAMEOBJECT),
        "{complete}"
    );
    let simple_uses = complete["simple_gameobject_use_receipts"]
        .as_array()
        .unwrap();
    assert_eq!(simple_uses.len(), 1, "{complete}");
    let simple_use = &simple_uses[0];
    let simple_quest = complete["quests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|quest| quest["quest_entry"] == "50970")
        .expect("simple GameObject Quest is missing");
    assert_eq!(simple_use["character_guid"], guid, "{complete}");
    assert_eq!(
        simple_use["character_quest_id"], simple_quest["id"],
        "{complete}"
    );
    assert!(
        simple_use["runner_objective_identity"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > 0,
        "{complete}"
    );
    assert_eq!(simple_use["quest_entry"], "50970", "{complete}");
    assert_eq!(
        simple_use["gameobject_guid"],
        SIMPLE_GAMEOBJECT.to_string(),
        "{complete}"
    );
    assert_eq!(simple_use["gameobject_entry"], "5090970", "{complete}");
    assert_eq!(simple_use["instance_id"], "0", "{complete}");
    assert_eq!(simple_use["use_count"], "1", "{complete}");
    assert!(
        simple_use["first_observed_micros"]
            .as_str()
            .unwrap()
            .parse::<i64>()
            .unwrap()
            >= structured_number(
                complete["journey"]["journey_started_micros"]
                    .as_str()
                    .unwrap(),
                "some",
            ),
        "{complete}"
    );
    let kill = query_one(
        node,
        &format!("SELECT counts FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = 7"),
    );
    assert_eq!(kill["counts"].trim_matches(['[', ']']), "10", "{complete}");
    let simple = query_one(
        node,
        &format!("SELECT counts FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = 50970"),
    );
    assert_eq!(simple["counts"].trim_matches(['[', ']']), "1", "{complete}");
    for (item, required) in [(750, 8), (752, 12), (11_119, 8), (11_125, 1)] {
        assert!(
            observation.peak_items[&item] >= required,
            "item {item}: {complete}"
        );
        assert_eq!(item_count(node, guid, item), 0, "item {item}: {complete}");
    }
}

fn run_whole_journey(case: JourneyCase) {
    let mut node = Standalone::start(&format!("playerbots-acceptance-seed-{}", case.seed));
    node.publish_module();
    remove_builtin_weather_import_stamp(&node);
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    stage_quest_geometry(&node);
    let (guid, staged) = stage_journey(&node, case);
    assert_staged(&node, &guid, case, &staged);
    let begun = begin_and_observe_first_decision(&node, &guid, case);
    let observation = observe_until_complete(&node, &guid);
    assert_complete(&node, &guid, &begun, &observation);
}

fn active_fight(node: &Standalone, guid: &str, reason: &str) -> Option<(i64, u64)> {
    let runner = node
        .query_rows(&format!(
            "SELECT observed_micros, chosen FROM pkg_playerbots_runner WHERE character_guid = {guid}"
        ))
        .into_iter()
        .next()?;
    let chosen = &runner["chosen"];
    if !chosen.contains(reason) {
        return None;
    }
    let chosen_target = if chosen.contains("action = (attack = ") {
        structured_u64(chosen, "attack")
    } else if chosen.contains("action = (cast = ") {
        structured_u64(chosen, "target")
    } else {
        return None;
    };
    let melee = node
        .query_rows(&format!(
            "SELECT target_guid FROM game_melee_attack WHERE attacker_guid = {guid}"
        ))
        .into_iter()
        .next()?;
    let melee_target = melee["target_guid"].parse().ok()?;
    if chosen_target != melee_target {
        return None;
    }
    Some((runner["observed_micros"].parse().ok()?, melee_target))
}

fn level_gap_snapshot(node: &Standalone, guid: &str) -> serde_json::Value {
    serde_json::json!({
        "fixture": node.query_rows(&format!("SELECT * FROM pkg_playerbots_acceptance_level_gap WHERE character_guid = {guid}")),
        "bot": node.query_rows(&format!("SELECT * FROM pkg_playerbots_bot WHERE character_guid = {guid}")),
        "character": node.query_rows(&format!("SELECT guid, level, xp, next_level_xp, health, dead, map_id, instance_id, x, y, z FROM game_world_entity WHERE guid = {guid}")),
        "quest_template": node.query_rows(&format!("SELECT entry, min_level, prev_quest_id FROM game_quest_template WHERE entry = {LEVEL_GAP_QUEST}")),
        "catalog_quest": node.query_rows(&format!("SELECT quest_entry, min_level, content_revision FROM pkg_playerbots_catalog_quest WHERE quest_entry = {LEVEL_GAP_QUEST}")),
        "end_only_relation": node.query_rows(&format!("SELECT creature_entry, quest_entry, role FROM game_creature_quest WHERE creature_entry = {LEVEL_GAP_END_ONLY_ENTRY}")),
        "character_quest": node.query_rows(&format!("SELECT * FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = {LEVEL_GAP_QUEST}")),
        "runner": node.query_rows(&format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}")),
        "actions": node.query_rows(&format!("SELECT * FROM pkg_playerbots_action WHERE character_guid = {guid}")),
        "melee": node.query_rows(&format!("SELECT * FROM game_melee_attack WHERE attacker_guid = {guid}")),
        "source_template": node.query_rows(&format!("SELECT entry, level, health, damage_min, damage_max FROM game_creature_template WHERE entry = {LEVEL_GAP_SOURCE_ENTRY}")),
        "sources": node.query_rows(&format!("SELECT guid, entry, level, health, max_health, dead, map_id, instance_id, x, y, z FROM game_world_entity WHERE entry = {LEVEL_GAP_SOURCE_ENTRY}")),
        "spawns": node.query_rows(&format!("SELECT guid, entry, map_id, x, y, z, life_seq FROM game_creature_spawn WHERE entry = {LEVEL_GAP_SOURCE_ENTRY}")),
        "kills": node.query_rows(&format!("SELECT * FROM pkg_playerbots_acceptance_level_gap_kill WHERE character_guid = {guid}")),
        "xp_events": node.query_rows("SELECT killed_guid, total_exp, is_kill, created_at FROM game_xp_event"),
        "levelups": node.query_rows("SELECT new_level, created_at FROM game_levelup_event"),
    })
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_acceptance_level_gap_uses_ordinary_kill_xp() {
    let mut node = Standalone::start("playerbots-acceptance-level-gap");
    node.publish_module();
    remove_builtin_weather_import_stamp(&node);
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    stage_quest_geometry(&node);
    node.assert_call("playerbots_acceptance_stage_level_gap", &[]);
    let guid = query_one(
        &node,
        "SELECT character_guid FROM pkg_playerbots_acceptance_level_gap",
    )["character_guid"]
        .clone();
    let staged = level_gap_snapshot(&node, &guid);
    save(&node, "level-gap-staged", staged.clone());
    assert_eq!(staged["fixture"][0]["required_level"], "7", "{staged}");
    assert_eq!(staged["fixture"][0]["initial_level"], "5", "{staged}");
    assert_eq!(staged["fixture"][0]["initial_xp"], "0", "{staged}");
    assert!(staged["fixture"][0]["imported_content_revision"]
        .as_str()
        .unwrap()
        .contains("none"));
    assert_eq!(staged["quest_template"][0]["min_level"], "7", "{staged}");
    assert_eq!(staged["catalog_quest"][0]["min_level"], "7", "{staged}");
    assert_eq!(
        staged["end_only_relation"].as_array().unwrap().len(),
        1,
        "{staged}"
    );
    assert_eq!(
        staged["end_only_relation"][0]["quest_entry"], "3905",
        "{staged}"
    );
    assert_eq!(staged["end_only_relation"][0]["role"], "1", "{staged}");
    assert!(staged["character_quest"].as_array().unwrap().is_empty());
    assert_eq!(staged["sources"].as_array().unwrap().len(), 66, "{staged}");
    assert_eq!(staged["source_template"][0]["damage_min"], "1", "{staged}");
    assert_eq!(staged["source_template"][0]["damage_max"], "1", "{staged}");
    node.assert_call("playerbots_acceptance_begin_level_gap", &[&guid]);

    let source_floor = (0xF130u64 << 48) | (u64::from(LEVEL_GAP_SOURCE_ENTRY) << 24) | 1;
    let fight = std::cell::RefCell::new(None);
    let fight_began = support::poll_until(Duration::from_secs(180), || {
        let Some((observed, target)) = active_fight(&node, &guid, "reason = (grind = ())") else {
            return false;
        };
        let is_long_source = (source_floor..source_floor + 3).contains(&target);
        let live_melee = !node
            .query_rows(&format!(
                "SELECT target_guid FROM game_melee_attack WHERE attacker_guid = {guid} AND target_guid = {target}"
            ))
            .is_empty();
        let live_target = node
            .query_rows(&format!(
                "SELECT dead FROM game_world_entity WHERE guid = {target}"
            ))
            .first()
            .is_some_and(|row| row["dead"] == "false");
        if is_long_source && live_melee && live_target {
            fight.replace(Some((observed, target)));
            true
        } else {
            false
        }
    });
    if !fight_began {
        let failed = level_gap_snapshot(&node, &guid);
        save(&node, "level-gap-fight-timeout", failed.clone());
        panic!("no long Grind fight began: {failed}");
    }
    let (fight_started, fight_target) = fight.into_inner().unwrap();
    let next_salt_window = (fight_started / 8_000_000 + 1) * 8_000_000;
    let crossed_salt = support::poll_until(Duration::from_secs(30), || {
        active_fight(&node, &guid, "reason = (grind = ())")
            .or_else(|| active_fight(&node, &guid, "reason = (defense = ())"))
            .is_some_and(|(observed, target)| {
                observed >= next_salt_window
                    && target == fight_target
                    && node
                        .query_rows(&format!(
                            "SELECT dead FROM game_world_entity WHERE guid = {target}"
                        ))
                        .first()
                        .is_some_and(|row| row["dead"] == "false")
            })
    });
    if !crossed_salt {
        let failed = level_gap_snapshot(&node, &guid);
        save(&node, "level-gap-salt-timeout", failed.clone());
        panic!("Grind target changed at the salt boundary: {failed}");
    }
    let across_salt = level_gap_snapshot(&node, &guid);
    save(&node, "level-gap-across-salt", across_salt);

    let progressed = support::poll_until(Duration::from_secs(600), || {
        !node.query_rows(&format!(
            "SELECT level FROM game_world_entity WHERE guid = {guid} AND level >= 7"
        ))
        .is_empty()
            && !node
                .query_rows(&format!(
                    "SELECT quest_entry FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = {LEVEL_GAP_QUEST}"
                ))
                .is_empty()
    });
    if !progressed {
        let failed = level_gap_snapshot(&node, &guid);
        save(&node, "level-gap-progression-timeout", failed.clone());
        panic!("level-gap progression timed out: {failed}");
    }
    let complete = level_gap_snapshot(&node, &guid);
    save(&node, "level-gap-complete", complete.clone());
    assert_eq!(complete["character"][0]["level"], "7", "{complete}");
    let source_ceiling = source_floor + LEVEL_GAP_SOURCE_COUNT;
    let mut kills = complete["kills"].as_array().unwrap().clone();
    kills.sort_by_key(|kill| kill["ordinal"].as_str().unwrap().parse::<u32>().unwrap());
    assert!(!kills.is_empty(), "{complete}");
    assert!(
        kills.iter().all(|kill| kill["victim_entry"]
            .as_str()
            .unwrap()
            .parse::<u32>()
            .unwrap()
            != LEVEL_GAP_END_ONLY_ENTRY),
        "{complete}"
    );
    assert_eq!(kills[0]["level_before"], "5", "{complete}");
    assert_eq!(kills[0]["xp_before"], "0", "{complete}");
    for (index, kill) in kills.iter().enumerate() {
        assert_eq!(kill["ordinal"], (index + 1).to_string(), "{complete}");
        let killed = kill["victim_guid"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap();
        assert!(
            (source_floor..source_ceiling).contains(&killed)
                || [299, 69, 38]
                    .into_iter()
                    .map(creature_guid)
                    .any(|source| source == killed),
            "{complete}"
        );
        let before = (
            kill["level_before"]
                .as_str()
                .unwrap()
                .parse::<u32>()
                .unwrap(),
            kill["xp_before"].as_str().unwrap().parse::<u32>().unwrap(),
        );
        let after = (
            kill["level_after"]
                .as_str()
                .unwrap()
                .parse::<u32>()
                .unwrap(),
            kill["xp_after"].as_str().unwrap().parse::<u32>().unwrap(),
        );
        assert!(after > before, "{complete}");
        if let Some(next) = kills.get(index + 1) {
            assert_eq!(next["level_before"], kill["level_after"], "{complete}");
            assert_eq!(next["xp_before"], kill["xp_after"], "{complete}");
        }
    }
    let last = kills.last().unwrap();
    assert_eq!(last["level_after"], "7", "{complete}");
    assert_eq!(
        complete["fixture"][0]["kill_count"],
        kills.len().to_string(),
        "{complete}"
    );
    assert_eq!(complete["fixture"][0]["observed_level"], "7", "{complete}");
    assert_eq!(
        complete["fixture"][0]["accepted_level"], "(some = 7)",
        "{complete}"
    );
    assert_eq!(
        complete["fixture"][0]["accepted_xp"],
        format!("(some = {})", last["xp_after"].as_str().unwrap()),
        "{complete}"
    );
    assert!(
        complete["fixture"][0]["accepted_micros"]
            .as_str()
            .unwrap()
            .contains("some"),
        "{complete}"
    );
    assert_eq!(
        complete["quest_template"][0]["min_level"], "7",
        "{complete}"
    );
    assert_eq!(complete["catalog_quest"][0]["min_level"], "7", "{complete}");
}

macro_rules! journey_case {
    ($name:ident, $seed:literal, $class:literal, $role:literal, $suffix:literal,
        $x:literal, $y:literal, $phase:literal, $rotation:literal) => {
        #[test]
        #[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
        fn $name() {
            run_whole_journey(JourneyCase {
                seed: $seed,
                class: $class,
                role: $role,
                suffix: $suffix,
                start_offset_x: $x,
                start_offset_y: $y,
                initial_due_phase_micros: $phase,
                source_insertion_rotation: $rotation,
            });
        }
    };
}

journey_case!(
    playerbots_acceptance_warrior_seed_783001000_completes_the_whole_journey,
    783_001_000,
    1,
    0,
    0,
    -0.75,
    -0.75,
    0,
    0
);
journey_case!(
    playerbots_acceptance_warrior_seed_783001001_completes_the_whole_journey,
    783_001_001,
    1,
    0,
    1,
    -0.25,
    -0.75,
    100_000,
    1
);
journey_case!(
    playerbots_acceptance_warrior_seed_783001002_completes_the_whole_journey,
    783_001_002,
    1,
    0,
    2,
    0.25,
    -0.75,
    200_000,
    2
);
journey_case!(
    playerbots_acceptance_warrior_seed_783001003_completes_the_whole_journey,
    783_001_003,
    1,
    0,
    3,
    0.75,
    -0.75,
    300_000,
    3
);
journey_case!(
    playerbots_acceptance_warrior_seed_783001004_completes_the_whole_journey,
    783_001_004,
    1,
    0,
    4,
    -0.50,
    -0.25,
    400_000,
    4
);
journey_case!(
    playerbots_acceptance_warrior_seed_783001005_completes_the_whole_journey,
    783_001_005,
    1,
    0,
    5,
    0.00,
    -0.25,
    500_000,
    5
);
journey_case!(
    playerbots_acceptance_warrior_seed_783001006_completes_the_whole_journey,
    783_001_006,
    1,
    0,
    6,
    0.50,
    0.25,
    600_000,
    6
);
journey_case!(
    playerbots_acceptance_warrior_seed_783001007_completes_the_whole_journey,
    783_001_007,
    1,
    0,
    7,
    -0.75,
    0.75,
    700_000,
    7
);
journey_case!(
    playerbots_acceptance_warrior_seed_783001008_completes_the_whole_journey,
    783_001_008,
    1,
    0,
    8,
    0.25,
    0.75,
    800_000,
    8
);
journey_case!(
    playerbots_acceptance_warrior_seed_783001009_completes_the_whole_journey,
    783_001_009,
    1,
    0,
    9,
    0.75,
    0.75,
    900_000,
    9
);

journey_case!(
    playerbots_acceptance_priest_seed_783005000_completes_the_whole_journey,
    783_005_000,
    5,
    1,
    0,
    -0.75,
    -0.75,
    0,
    0
);
journey_case!(
    playerbots_acceptance_priest_seed_783005001_completes_the_whole_journey,
    783_005_001,
    5,
    1,
    1,
    -0.25,
    -0.75,
    100_000,
    1
);
journey_case!(
    playerbots_acceptance_priest_seed_783005002_completes_the_whole_journey,
    783_005_002,
    5,
    1,
    2,
    0.25,
    -0.75,
    200_000,
    2
);
journey_case!(
    playerbots_acceptance_priest_seed_783005003_completes_the_whole_journey,
    783_005_003,
    5,
    1,
    3,
    0.75,
    -0.75,
    300_000,
    3
);
journey_case!(
    playerbots_acceptance_priest_seed_783005004_completes_the_whole_journey,
    783_005_004,
    5,
    1,
    4,
    -0.50,
    -0.25,
    400_000,
    4
);
journey_case!(
    playerbots_acceptance_priest_seed_783005005_completes_the_whole_journey,
    783_005_005,
    5,
    1,
    5,
    0.00,
    -0.25,
    500_000,
    5
);
journey_case!(
    playerbots_acceptance_priest_seed_783005006_completes_the_whole_journey,
    783_005_006,
    5,
    1,
    6,
    0.50,
    0.25,
    600_000,
    6
);
journey_case!(
    playerbots_acceptance_priest_seed_783005007_completes_the_whole_journey,
    783_005_007,
    5,
    1,
    7,
    -0.75,
    0.75,
    700_000,
    7
);
journey_case!(
    playerbots_acceptance_priest_seed_783005008_completes_the_whole_journey,
    783_005_008,
    5,
    1,
    8,
    0.25,
    0.75,
    800_000,
    8
);
journey_case!(
    playerbots_acceptance_priest_seed_783005009_completes_the_whole_journey,
    783_005_009,
    5,
    1,
    9,
    0.75,
    0.75,
    900_000,
    9
);

journey_case!(
    playerbots_acceptance_mage_seed_783008000_completes_the_whole_journey,
    783_008_000,
    8,
    2,
    0,
    -0.75,
    -0.75,
    0,
    0
);
journey_case!(
    playerbots_acceptance_mage_seed_783008001_completes_the_whole_journey,
    783_008_001,
    8,
    2,
    1,
    -0.25,
    -0.75,
    100_000,
    1
);
journey_case!(
    playerbots_acceptance_mage_seed_783008002_completes_the_whole_journey,
    783_008_002,
    8,
    2,
    2,
    0.25,
    -0.75,
    200_000,
    2
);
journey_case!(
    playerbots_acceptance_mage_seed_783008003_completes_the_whole_journey,
    783_008_003,
    8,
    2,
    3,
    0.75,
    -0.75,
    300_000,
    3
);
journey_case!(
    playerbots_acceptance_mage_seed_783008004_completes_the_whole_journey,
    783_008_004,
    8,
    2,
    4,
    -0.50,
    -0.25,
    400_000,
    4
);
journey_case!(
    playerbots_acceptance_mage_seed_783008005_completes_the_whole_journey,
    783_008_005,
    8,
    2,
    5,
    0.00,
    -0.25,
    500_000,
    5
);
journey_case!(
    playerbots_acceptance_mage_seed_783008006_completes_the_whole_journey,
    783_008_006,
    8,
    2,
    6,
    0.50,
    0.25,
    600_000,
    6
);
journey_case!(
    playerbots_acceptance_mage_seed_783008007_completes_the_whole_journey,
    783_008_007,
    8,
    2,
    7,
    -0.75,
    0.75,
    700_000,
    7
);
journey_case!(
    playerbots_acceptance_mage_seed_783008008_completes_the_whole_journey,
    783_008_008,
    8,
    2,
    8,
    0.25,
    0.75,
    800_000,
    8
);
journey_case!(
    playerbots_acceptance_mage_seed_783008009_completes_the_whole_journey,
    783_008_009,
    8,
    2,
    9,
    0.75,
    0.75,
    900_000,
    9
);
