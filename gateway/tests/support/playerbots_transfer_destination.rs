//! Retained Quest checks used by the shared cross-Shard Transfer fixture.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use crate::{TransferTopology, TransferredBot};

const SOURCE_QUEST: u32 = 7;
const REPLACEMENT_QUEST: u32 = 5_261;
const SOURCE_TARGET_GUID: u64 = (0xF130u64 << 48) | (6u64 << 24) | 1;
const DESTINATION_TARGET_GUID: u64 = (0xF130u64 << 48) | (6u64 << 24) | 10_002;
const DESTINATION_REPLACEMENT_GUID: u64 = (0xF130u64 << 48) | (823u64 << 24) | 10_003;
const REBUILT_CONTENT: &str = "playerbots-transfer-destination-q7-v1";
const REPLACEMENT_CONTENT: &str = "playerbots-transfer-destination-q5261-v1";
const ROLES_GROUP: u64 = 5_098_000;
const PRIEST: u8 = 5;
const MAGE: u8 = 8;
const HEALER: u8 = 1;
const DAMAGE: u8 = 2;

type Row = BTreeMap<String, String>;
type EvidenceRow = serde_json::Map<String, serde_json::Value>;

fn query_one<'a>(rows: &'a [Row], table: &str) -> &'a Row {
    assert_eq!(rows.len(), 1, "expected one {table} row, got {rows:?}");
    &rows[0]
}

fn number<T: std::str::FromStr>(row: &Row, field: &str) -> T {
    row.get(field)
        .unwrap_or_else(|| panic!("missing {field} in {row:?}"))
        .parse()
        .unwrap_or_else(|_| panic!("invalid {field} in {row:?}"))
}

fn stage_navigation(
    topology: &TransferTopology,
    database: &str,
    points: &[(u32, f32, f32, f32)],
) -> BTreeSet<u64> {
    topology.call(database, "debug_set_nav_enabled", &["true"]);
    let mut cells = BTreeSet::new();
    for &(map_id, x, y, z) in points {
        let cell_x = lyracore_shared::terrain::cell_index(x).expect("fixture x is on the map");
        let cell_y = lyracore_shared::terrain::cell_index(y).expect("fixture y is on the map");
        if cells.insert((map_id, cell_x, cell_y)) {
            let packed = format!("{map_id},{cell_x},{cell_y},{z},,");
            topology.call(database, "import_nav_chunks_append", &[&packed]);
        }
    }
    cells
        .into_iter()
        .map(|(map_id, cell_x, cell_y)| lyracore_shared::terrain::cell_key(map_id, cell_x, cell_y))
        .collect()
}

fn navigation_keys(evidence: &serde_json::Value) -> BTreeSet<u64> {
    evidence["navigation_chunks"]
        .as_array()
        .expect("Navigation Inputs rows are not an array")
        .iter()
        .map(|row| {
            row["key"]
                .as_str()
                .expect("Navigation Inputs key is not text")
                .parse()
                .expect("Navigation Inputs key is not a u64")
        })
        .collect()
}

fn recorded_keys(evidence: &serde_json::Value, name: &str) -> BTreeSet<u64> {
    evidence[name]
        .as_array()
        .unwrap_or_else(|| panic!("{name} is not an array: {evidence}"))
        .iter()
        .map(|key| {
            key.as_u64()
                .unwrap_or_else(|| panic!("{name} contains a non-u64 key: {evidence}"))
        })
        .collect()
}

fn exactly_one<'a>(rows: &'a [serde_json::Value], table: &str) -> &'a EvidenceRow {
    assert_eq!(rows.len(), 1, "expected one {table} row, got {rows:?}");
    rows[0]
        .as_object()
        .unwrap_or_else(|| panic!("{table} row is not an object: {:?}", rows[0]))
}

fn runner(topology: &TransferTopology, guid: u64) -> Vec<Row> {
    topology.query(
        &topology.destination_db,
        &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
    )
}

fn retained(topology: &TransferTopology, guid: u64) -> Vec<Row> {
    topology.query(
        &topology.destination_db,
        &format!("SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"),
    )
}

fn role_member(topology: &TransferTopology, class: u8, role: u8) -> u64 {
    let rows = topology.query(
        &topology.source_db,
        &format!(
            "SELECT character_guid FROM pkg_playerbots_bot WHERE class = {class} AND role = {role}"
        ),
    );
    number(
        query_one(&rows, "private role party member"),
        "character_guid",
    )
}

fn set_companion_party_membership(
    topology: &TransferTopology,
    database: &str,
    transferred: &TransferredBot,
    mode: u8,
) {
    let priest = role_member(topology, PRIEST, HEALER);
    let mage = role_member(topology, MAGE, DAMAGE);
    let actor = format!(r#"{{"guid":{},"ownership":null}}"#, transferred.leader_guid);
    topology.call(
        database,
        "playerbots_fixture_orders_party_as",
        &[
            &transferred.guid.to_string(),
            &priest.to_string(),
            &mage.to_string(),
            &transferred.leader_guid.to_string(),
            &mode.to_string(),
            &actor,
        ],
    );
}

fn destination_snapshot(topology: &TransferTopology, guid: u64) -> serde_json::Value {
    serde_json::json!({
        "runner": runner(topology, guid),
        "retained_quest": retained(topology, guid),
        "quest_admission": topology.query(
            &topology.destination_db,
            &format!("SELECT * FROM pkg_playerbots_quest_admission WHERE character_guid = {guid}"),
        ),
        "actions": topology.query(
            &topology.destination_db,
            &format!("SELECT * FROM pkg_playerbots_action WHERE character_guid = {guid}"),
        ),
        "movement": topology.query(
            &topology.destination_db,
            &format!("SELECT * FROM game_creature_spline WHERE guid = {guid}"),
        ),
        "attack": topology.query(
            &topology.destination_db,
            &format!("SELECT * FROM game_melee_attack WHERE attacker_guid = {guid}"),
        ),
        "pending_cast": topology.query(
            &topology.destination_db,
            &format!("SELECT * FROM game_pending_cast WHERE caster_guid = {guid}"),
        ),
        "recovery_scan": topology.query(
            &topology.destination_db,
            &format!("SELECT * FROM pkg_playerbots_recovery_scan WHERE character_guid = {guid}"),
        ),
        "group_membership": topology.query(
            &topology.destination_db,
            &format!("SELECT * FROM game_group_member WHERE group_id = {ROLES_GROUP} AND character_guid = {guid}"),
        ),
        "catalog": topology.query(
            &topology.destination_db,
            "SELECT * FROM pkg_playerbots_quest_catalog WHERE revision = 1",
        ),
        "source_catalog_quest": topology.query(
            &topology.destination_db,
            "SELECT * FROM pkg_playerbots_catalog_quest WHERE quest_entry = 7",
        ),
        "replacement_catalog_quest": topology.query(
            &topology.destination_db,
            "SELECT * FROM pkg_playerbots_catalog_quest WHERE quest_entry = 5261",
        ),
        "catalog_objectives": topology.query(
            &topology.destination_db,
            "SELECT * FROM pkg_playerbots_catalog_objective WHERE id = 5100120",
        ),
        "navigation_revision": topology.query(
            &topology.destination_db,
            "SELECT * FROM game_navigation_revision",
        ),
        "navigation_config": topology.query(
            &topology.destination_db,
            "SELECT nav_enabled FROM game_config WHERE id = 0",
        ),
        "navigation_chunks": topology.query(
            &topology.destination_db,
            "SELECT key, map_id, cell_x, cell_y, base_z, walk, obs FROM game_nav_chunk WHERE map_id = 36",
        ),
        "source_target_entity": topology.query(
            &topology.destination_db,
            &format!("SELECT guid FROM game_world_entity WHERE guid = {SOURCE_TARGET_GUID}"),
        ),
        "source_target_spawn": topology.query(
            &topology.destination_db,
            &format!("SELECT guid FROM game_creature_spawn WHERE guid = {SOURCE_TARGET_GUID}"),
        ),
        "source_quest_log": topology.query(
            &topology.destination_db,
            &format!("SELECT * FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = 7"),
        ),
        "replacement_quest_log": topology.query(
            &topology.destination_db,
            &format!("SELECT * FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = 5261"),
        ),
    })
}

fn source_snapshot(topology: &TransferTopology, guid: u64) -> serde_json::Value {
    let entity = topology.query(
        &topology.source_db,
        &format!("SELECT map_id, x, y, z FROM game_world_entity WHERE guid = {guid}"),
    );
    let map_id = number::<u32>(query_one(&entity, "source Character entity"), "map_id");
    serde_json::json!({
        "entity": entity,
        "runner": topology.query(
            &topology.source_db,
            &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}"),
        ),
        "retained_quest": topology.query(
            &topology.source_db,
            &format!(
                "SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"
            ),
        ),
        "actions": topology.query(
            &topology.source_db,
            &format!("SELECT * FROM pkg_playerbots_action WHERE character_guid = {guid}"),
        ),
        "transfer_intent": topology.query(
            &topology.source_db,
            &format!("SELECT * FROM game_bot_transfer_intent WHERE bot_guid = {guid}"),
        ),
        "navigation_revision": topology.query(
            &topology.source_db,
            "SELECT * FROM game_navigation_revision",
        ),
        "navigation_config": topology.query(
            &topology.source_db,
            "SELECT nav_enabled FROM game_config WHERE id = 0",
        ),
        "navigation_chunks": topology.query(
            &topology.source_db,
            &format!("SELECT key, map_id, cell_x, cell_y, base_z, walk, obs FROM game_nav_chunk WHERE map_id = {map_id}"),
        ),
        "source_target": topology.query(
            &topology.source_db,
            &format!("SELECT guid, entry, map_id, instance_id, x, y, z FROM game_world_entity WHERE guid = {SOURCE_TARGET_GUID}"),
        ),
        "quest_log": topology.query(
            &topology.source_db,
            &format!("SELECT * FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = 7"),
        ),
        "group_membership": topology.query(
            &topology.source_db,
            &format!("SELECT * FROM game_group_member WHERE group_id = {ROLES_GROUP} AND character_guid = {guid}"),
        ),
    })
}

fn field<'a>(row: &'a EvidenceRow, name: &str) -> &'a str {
    row.get(name)
        .unwrap_or_else(|| panic!("missing {name} in {row:?}"))
        .as_str()
        .unwrap_or_else(|| panic!("{name} is not a string in {row:?}"))
}

fn embedded_u64(value: &str, name: &str) -> u64 {
    let prefix = format!("{name} = ");
    let rest = value
        .split(&prefix)
        .nth(1)
        .unwrap_or_else(|| panic!("missing {name} in {value}"));
    rest.split(|character: char| !character.is_ascii_digit())
        .next()
        .and_then(|number| number.parse().ok())
        .unwrap_or_else(|| panic!("invalid {name} in {value}"))
}

/// Stage the real Quest and execute its selected Transfer operation before Gateway crossing.
pub(crate) fn stage_retained_quest(
    topology: &TransferTopology,
    transferred: &TransferredBot,
) -> serde_json::Value {
    let character_guid = transferred.guid;
    set_companion_party_membership(topology, &topology.source_db, transferred, 1);
    let entities = topology.query(
        &topology.source_db,
        &format!("SELECT map_id, x, y, z FROM game_world_entity WHERE guid = {character_guid}"),
    );
    let entity = query_one(&entities, "source Character entity");
    let expected_navigation = stage_navigation(
        topology,
        &topology.source_db,
        &[(
            (number::<u32>(entity, "map_id")),
            number::<f32>(entity, "x"),
            number::<f32>(entity, "y"),
            number::<f32>(entity, "z"),
        )],
    );
    topology.call(
        &topology.source_db,
        "playerbots_transfer_quest_source_stage",
        &[&character_guid.to_string()],
    );
    let staged = source_snapshot(topology, character_guid);
    set_companion_party_membership(topology, &topology.source_db, transferred, 0);
    let suspended = source_snapshot(topology, character_guid);
    topology.call(
        &topology.source_db,
        "playerbots_transfer_quest_execute",
        &[&character_guid.to_string()],
    );
    let operation = source_snapshot(topology, character_guid);
    let evidence = serde_json::json!({
        "expected_navigation": expected_navigation,
        "staged": staged,
        "suspended": suspended,
        "operation": operation,
    });
    evidence
}

/// Check the retained Quest and real source Transfer operation after the caller saves evidence.
pub(crate) fn assert_retained_quest_stage(evidence: &serde_json::Value) {
    let runner = exactly_one(
        evidence["staged"]["runner"].as_array().unwrap(),
        "source Runner",
    );
    let retained = exactly_one(
        evidence["staged"]["retained_quest"].as_array().unwrap(),
        "source retained Quest",
    );
    let navigation = exactly_one(
        evidence["staged"]["navigation_revision"]
            .as_array()
            .unwrap(),
        "source Navigation Inputs revision",
    );
    assert_eq!(field(retained, "quest_entry"), SOURCE_QUEST.to_string());
    assert!(
        field(retained, "destination").contains(&SOURCE_TARGET_GUID.to_string()),
        "{evidence}"
    );
    assert!(
        field(runner, "objective").contains("kind = (quest = ())"),
        "{evidence}"
    );
    assert!(
        field(runner, "recovery").contains("stalled_micros = 30000000"),
        "{evidence}"
    );
    assert!(
        field(navigation, "revision").parse::<u64>().unwrap() > 0,
        "{evidence}"
    );
    assert!(
        navigation_keys(&evidence["staged"]) == recorded_keys(evidence, "expected_navigation"),
        "{evidence}"
    );
    let navigation_config = exactly_one(
        evidence["staged"]["navigation_config"].as_array().unwrap(),
        "source Navigation Inputs config",
    );
    assert_eq!(
        field(navigation_config, "nav_enabled"),
        "true",
        "{evidence}"
    );
    assert_eq!(
        evidence["staged"]["source_target"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "{evidence}"
    );
    assert!(
        !evidence["staged"]["actions"].as_array().unwrap().is_empty(),
        "{evidence}"
    );
    assert!(
        evidence["staged"]["group_membership"]
            .as_array()
            .unwrap()
            .is_empty(),
        "the source Quest must be selected while the companion is outside the party: {evidence}"
    );
    let suspended_runner = exactly_one(
        evidence["suspended"]["runner"].as_array().unwrap(),
        "suspended source Runner",
    );
    assert_eq!(
        evidence["suspended"]["group_membership"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "{evidence}"
    );
    assert_eq!(
        field(suspended_runner, "objective"),
        field(runner, "objective"),
        "{evidence}"
    );
    assert_eq!(
        evidence["suspended"]["retained_quest"], evidence["staged"]["retained_quest"],
        "{evidence}"
    );
    let operation_runner = exactly_one(
        evidence["operation"]["runner"].as_array().unwrap(),
        "source Runner after Transfer operation",
    );
    assert!(
        field(operation_runner, "transfer_checkpoint").contains("quest = 7")
            && field(operation_runner, "transfer_checkpoint").contains("stalled_micros = 30000000"),
        "{evidence}"
    );
    assert_eq!(
        field(operation_runner, "objective"),
        field(runner, "objective"),
        "{evidence}"
    );
    assert_eq!(
        evidence["operation"]["retained_quest"], evidence["staged"]["retained_quest"],
        "{evidence}"
    );
    assert_eq!(
        evidence["operation"]["group_membership"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "{evidence}"
    );
    for field_name in [
        "foreground",
        "chosen",
        "movement_progress",
        "combat_progress",
        "cast_progress",
        "progress_age_micros",
        "last_target_health",
        "defense_target",
        "recovery",
        "retry_candidate",
        "companion_heal_target_guid",
        "companion_fight_target_guid",
        "companion_buff_target_guid",
    ] {
        assert_eq!(
            field(operation_runner, field_name),
            "(none = ())",
            "Transfer normalization retained {field_name}: {evidence}"
        );
    }
    for field_name in ["candidate_order", "quest_progress", "deferred_destinations"] {
        assert_eq!(
            field(operation_runner, field_name),
            "",
            "Transfer normalization retained {field_name}: {evidence}"
        );
    }
    assert_eq!(field(operation_runner, "transitions"), "0", "{evidence}");
    assert_eq!(
        field(operation_runner, "route_expansions"),
        "0",
        "{evidence}"
    );
    assert_eq!(
        evidence["operation"]["transfer_intent"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "{evidence}"
    );
    let intent = exactly_one(
        evidence["operation"]["transfer_intent"].as_array().unwrap(),
        "source Transfer Intent",
    );
    let transfer_actions: Vec<_> = evidence["operation"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(serde_json::Value::as_object)
        .filter(|action| field(action, "kind") == "(transfer = ())")
        .collect();
    assert_eq!(
        transfer_actions.len(),
        1,
        "expected one source Transfer action: {evidence}"
    );
    let action = transfer_actions[0];
    assert!(
        field(operation_runner, "transfer_checkpoint")
            .contains(&format!("intent_id = {}", field(intent, "id")))
            && field(action, "kind") == "(transfer = ())"
            && field(action, "outcome")
                .contains(&format!("transferAccepted = {}", field(intent, "id"))),
        "the real Transfer operation was not recorded exactly: {evidence}"
    );
}

/// Stage static destination content before the shared driver begins Transfer.
pub(crate) fn stage_destination_catalogue(
    topology: &TransferTopology,
    character_guid: u64,
    mode: u8,
) -> serde_json::Value {
    let expected_navigation = stage_navigation(
        topology,
        &topology.destination_db,
        &[
            (36, -14.5732, -385.475, 62.4561),
            (36, -12.5732, -385.475, 62.4561),
            (36, 3.4268, -382.475, 62.4561),
        ],
    );
    topology.call(
        &topology.destination_db,
        "playerbots_transfer_destination_catalogue_stage",
        &[&character_guid.to_string(), &mode.to_string()],
    );
    let snapshot = destination_snapshot(topology, character_guid);
    let source_navigation = topology.query(
        &topology.source_db,
        "SELECT revision FROM game_navigation_revision WHERE id = 0",
    );
    serde_json::json!({
        "mode": mode,
        "expected_navigation": expected_navigation,
        "source_navigation": source_navigation,
        "snapshot": snapshot,
    })
}

/// Check destination content and Navigation Inputs after the caller saves evidence.
pub(crate) fn assert_destination_catalogue(evidence: &serde_json::Value) {
    let snapshot = &evidence["snapshot"];
    assert!(
        snapshot["runner"].as_array().unwrap().is_empty(),
        "{evidence}"
    );
    assert!(
        snapshot["source_quest_log"].as_array().unwrap().is_empty()
            && snapshot["replacement_quest_log"]
                .as_array()
                .unwrap()
                .is_empty(),
        "{evidence}"
    );
    assert!(
        snapshot["actions"].as_array().unwrap().is_empty(),
        "{evidence}"
    );
    let navigation = exactly_one(
        snapshot["navigation_revision"].as_array().unwrap(),
        "destination Navigation Inputs revision",
    );
    let source_navigation = exactly_one(
        evidence["source_navigation"].as_array().unwrap(),
        "source Navigation Inputs revision",
    );
    assert_ne!(
        field(navigation, "revision"),
        field(source_navigation, "revision"),
        "source and destination Navigation Inputs must have different actual revisions: {evidence}"
    );
    assert!(
        recorded_keys(evidence, "expected_navigation").len() >= 2
            && navigation_keys(snapshot) == recorded_keys(evidence, "expected_navigation"),
        "destination navigation rows do not cover its landing and target cells: {evidence}"
    );
    let navigation_config = exactly_one(
        snapshot["navigation_config"].as_array().unwrap(),
        "destination Navigation Inputs config",
    );
    assert_eq!(
        field(navigation_config, "nav_enabled"),
        "true",
        "{evidence}"
    );
    let catalog = exactly_one(
        snapshot["catalog"].as_array().unwrap(),
        "destination Quest catalog",
    );
    let (selected, absent, content_revision) = if evidence["mode"].as_u64() == Some(1) {
        (
            "source_catalog_quest",
            "replacement_catalog_quest",
            REBUILT_CONTENT,
        )
    } else {
        (
            "replacement_catalog_quest",
            "source_catalog_quest",
            REPLACEMENT_CONTENT,
        )
    };
    assert_eq!(field(catalog, "content_revision"), content_revision);
    let quest = exactly_one(
        snapshot[selected].as_array().unwrap(),
        "destination catalog Quest",
    );
    assert_eq!(field(quest, "content_revision"), content_revision);
    assert!(
        snapshot[absent].as_array().unwrap().is_empty(),
        "{evidence}"
    );
}

fn pass_until(
    topology: &TransferTopology,
    guid: u64,
    ready: impl Fn(&serde_json::Value) -> bool,
) -> Vec<serde_json::Value> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut observations = Vec::new();
    loop {
        topology.call(
            &topology.destination_db,
            "playerbots_fixture_runner_pass_once",
            &[&guid.to_string()],
        );
        observations.push(destination_snapshot(topology, guid));
        if ready(observations.last().unwrap()) {
            return observations;
        }
        if Instant::now() >= deadline {
            return observations;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn assert_arrival_cleared(evidence: &serde_json::Value) {
    let runner = exactly_one(evidence["runner"].as_array().unwrap(), "arriving Runner");
    for field_name in [
        "foreground",
        "chosen",
        "movement_progress",
        "combat_progress",
        "cast_progress",
        "progress_age_micros",
        "last_target_health",
        "defense_target",
        "recovery",
        "retry_candidate",
        "companion_heal_target_guid",
        "companion_fight_target_guid",
        "companion_buff_target_guid",
    ] {
        assert_eq!(field(runner, field_name), "(none = ())", "{evidence}");
    }
    for field_name in ["candidate_order", "quest_progress", "deferred_destinations"] {
        assert_eq!(field(runner, field_name), "", "{evidence}");
    }
    assert_eq!(field(runner, "transitions"), "0", "{evidence}");
    assert_eq!(field(runner, "route_expansions"), "0", "{evidence}");
    assert!(
        evidence["actions"].as_array().unwrap().is_empty()
            && evidence["movement"].as_array().unwrap().is_empty()
            && evidence["attack"].as_array().unwrap().is_empty()
            && evidence["pending_cast"].as_array().unwrap().is_empty()
            && evidence["recovery_scan"].as_array().unwrap().is_empty(),
        "source-local work survived Transfer: {evidence}"
    );
}

fn assert_authenticated_leave(evidence: &serde_json::Value) {
    let attached_runner = exactly_one(
        evidence["attached"]["runner"].as_array().unwrap(),
        "attached destination Runner",
    );
    let left_runner = exactly_one(
        evidence["left"]["runner"].as_array().unwrap(),
        "destination Runner after Leave",
    );
    assert_eq!(
        evidence["attached"]["group_membership"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "{evidence}"
    );
    assert!(
        evidence["left"]["group_membership"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{evidence}"
    );
    for field_name in ["objective", "objective_sequence", "transfer_checkpoint"] {
        assert_eq!(
            field(left_runner, field_name),
            field(attached_runner, field_name),
            "authenticated Leave changed retained {field_name}: {evidence}"
        );
    }
}

/// Prove case 9 after the shared driver releases the destination Character.
pub(crate) fn case9_rebuilds_destination(
    topology: &TransferTopology,
    transferred: &TransferredBot,
) -> serde_json::Value {
    let attached = destination_snapshot(topology, transferred.guid);
    set_companion_party_membership(topology, &topology.destination_db, transferred, 1);
    let left = destination_snapshot(topology, transferred.guid);
    let attempts = pass_until(topology, transferred.guid, |snapshot| {
        let Some(runner) = snapshot["runner"].as_array().and_then(|rows| rows.first()) else {
            return false;
        };
        runner["chosen"]
            .as_str()
            .is_some_and(|chosen| chosen.contains("recoveryPosition"))
    });
    let after = attempts.last().unwrap().clone();
    let evidence = serde_json::json!({
        "case": "retained Quest rebuilds destination content and Navigation Inputs",
        "transferred": {
            "guid": transferred.guid,
            "intent_id": transferred.intent_id,
            "generation": transferred.generation,
            "objective_identity": transferred.objective_identity,
        },
        "attached": attached,
        "left": left,
        "attempts": attempts,
        "after": after,
    });
    evidence
}

/// Check compatible destination reconciliation after the caller saves evidence.
pub(crate) fn assert_case9_rebuilds_destination(evidence: &serde_json::Value) {
    let intent_id = evidence["transferred"]["intent_id"]
        .as_u64()
        .expect("transferred intent id is not a u64");
    let objective_identity = evidence["transferred"]["objective_identity"]
        .as_u64()
        .expect("transferred objective identity is not a u64");
    let before_runner = exactly_one(
        evidence["attached"]["runner"].as_array().unwrap(),
        "arriving Runner",
    );
    assert_arrival_cleared(&evidence["attached"]);
    assert_authenticated_leave(evidence);
    let after_runner = exactly_one(
        evidence["after"]["runner"].as_array().unwrap(),
        "reconciled Runner",
    );
    let retained = exactly_one(
        evidence["after"]["retained_quest"].as_array().unwrap(),
        "rebuilt retained Quest",
    );
    assert!(
        field(before_runner, "transfer_checkpoint").contains(&format!("intent_id = {intent_id}")),
        "{evidence}"
    );
    assert!(
        field(before_runner, "transfer_checkpoint").contains("quest = 7"),
        "{evidence}"
    );
    assert!(
        field(before_runner, "transfer_checkpoint").contains("stalled_micros = 30000000"),
        "{evidence}"
    );
    assert_eq!(
        embedded_u64(field(before_runner, "objective"), "identity"),
        objective_identity,
        "{evidence}"
    );
    assert_eq!(
        embedded_u64(field(after_runner, "objective"), "identity"),
        objective_identity,
        "{evidence}"
    );
    assert_eq!(
        embedded_u64(field(before_runner, "objective"), "deadline_micros"),
        embedded_u64(field(after_runner, "objective"), "deadline_micros"),
        "{evidence}"
    );
    assert_eq!(
        field(before_runner, "objective_sequence"),
        field(after_runner, "objective_sequence"),
        "{evidence}"
    );
    assert_eq!(
        field(before_runner, "failures"),
        field(after_runner, "failures"),
        "{evidence}"
    );
    assert_eq!(
        field(after_runner, "transfer_checkpoint"),
        "(none = ())",
        "{evidence}"
    );
    assert!(
        field(after_runner, "history").contains("arrived"),
        "{evidence}"
    );
    assert_eq!(field(retained, "quest_entry"), SOURCE_QUEST.to_string());
    assert_eq!(field(retained, "content_revision"), REBUILT_CONTENT);
    assert_eq!(
        field(retained, "destination_evidence_revision"),
        REBUILT_CONTENT
    );
    assert!(
        field(retained, "destination").contains(&DESTINATION_TARGET_GUID.to_string()),
        "{evidence}"
    );
    assert!(
        !field(retained, "destination").contains(&SOURCE_TARGET_GUID.to_string()),
        "{evidence}"
    );
    assert!(
        field(after_runner, "recovery").contains(&format!(
            "imported_revision = (some = {})",
            field(
                exactly_one(
                    evidence["after"]["navigation_revision"].as_array().unwrap(),
                    "destination Navigation Inputs revision",
                ),
                "revision",
            )
        )) && field(after_runner, "recovery").contains("stalled_micros = 30000000")
            && field(after_runner, "recovery").contains(&DESTINATION_TARGET_GUID.to_string())
            && !field(after_runner, "recovery").contains(&SOURCE_TARGET_GUID.to_string()),
        "{evidence}"
    );
    assert!(
        field(after_runner, "chosen").contains("recoveryPosition")
            && field(after_runner, "chosen").contains("reason = (quest = ())"),
        "{evidence}"
    );
    assert_eq!(
        evidence["after"]["source_quest_log"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "{evidence}"
    );
    assert!(
        evidence["attached"]["source_target_entity"]
            .as_array()
            .unwrap()
            .is_empty()
            && evidence["attached"]["source_target_spawn"]
                .as_array()
                .unwrap()
                .is_empty()
            && evidence["attached"]["actions"]
                .as_array()
                .unwrap()
                .is_empty()
            && evidence["attached"]["movement"]
                .as_array()
                .unwrap()
                .is_empty()
            && evidence["attached"]["attack"]
                .as_array()
                .unwrap()
                .is_empty()
            && evidence["attached"]["pending_cast"]
                .as_array()
                .unwrap()
                .is_empty()
            && evidence["attached"]["recovery_scan"]
                .as_array()
                .unwrap()
                .is_empty(),
        "source-local rows survived Transfer: {evidence}"
    );
}

/// Prove case 10 after the shared driver releases the destination Character.
pub(crate) fn case10_records_incompatible_destination(
    topology: &TransferTopology,
    transferred: &TransferredBot,
) -> serde_json::Value {
    let attached = destination_snapshot(topology, transferred.guid);
    set_companion_party_membership(topology, &topology.destination_db, transferred, 1);
    let left = destination_snapshot(topology, transferred.guid);
    topology.call(
        &topology.destination_db,
        "playerbots_fixture_runner_pass_once",
        &[&transferred.guid.to_string()],
    );
    let refused = destination_snapshot(topology, transferred.guid);
    let replacement_attempts = pass_until(topology, transferred.guid, |snapshot| {
        let chosen = snapshot["runner"]
            .as_array()
            .and_then(|rows| rows.first())
            .and_then(|runner| runner["chosen"].as_str());
        chosen
            .is_some_and(|chosen| chosen.contains("acceptQuest") && chosen.contains("quest = 5261"))
            && snapshot["replacement_quest_log"]
                .as_array()
                .is_some_and(|rows| rows.len() == 1)
    });
    let replacement = replacement_attempts.last().unwrap().clone();
    let evidence = serde_json::json!({
        "case": "incompatible destination records why retained Quest changed",
        "transferred": {
            "guid": transferred.guid,
            "intent_id": transferred.intent_id,
            "generation": transferred.generation,
            "objective_identity": transferred.objective_identity,
        },
        "attached": attached,
        "left": left,
        "refused": refused,
        "replacement_attempts": replacement_attempts,
        "replacement": replacement,
    });
    evidence
}

/// Check incompatible destination replacement after the caller saves evidence.
pub(crate) fn assert_case10_records_incompatible_destination(evidence: &serde_json::Value) {
    let objective_identity = evidence["transferred"]["objective_identity"]
        .as_u64()
        .expect("transferred objective identity is not a u64");
    let before_runner = exactly_one(
        evidence["attached"]["runner"].as_array().unwrap(),
        "arriving Runner",
    );
    assert_arrival_cleared(&evidence["attached"]);
    assert_authenticated_leave(evidence);
    let refused_runner = exactly_one(
        evidence["refused"]["runner"].as_array().unwrap(),
        "arrival-refused Runner",
    );
    let replacement_runner = exactly_one(
        evidence["replacement"]["runner"].as_array().unwrap(),
        "replacement Runner",
    );
    let retained = exactly_one(
        evidence["replacement"]["retained_quest"]
            .as_array()
            .unwrap(),
        "replacement retained Quest",
    );
    assert!(
        field(before_runner, "transfer_checkpoint").contains("quest = 7"),
        "{evidence}"
    );
    assert_eq!(
        field(refused_runner, "transfer_checkpoint"),
        "(none = ())",
        "{evidence}"
    );
    assert!(
        field(refused_runner, "last_outcome").contains("(refused = (transferPurposeChanged = ()))")
            && field(refused_runner, "failures").contains("reason = (transferPurposeChanged = ())"),
        "{evidence}"
    );
    assert_ne!(
        embedded_u64(field(refused_runner, "objective"), "identity"),
        objective_identity,
        "{evidence}"
    );
    assert_eq!(
        field(retained, "quest_entry"),
        REPLACEMENT_QUEST.to_string()
    );
    assert_eq!(field(retained, "content_revision"), REPLACEMENT_CONTENT);
    assert!(
        field(retained, "destination").contains(&DESTINATION_REPLACEMENT_GUID.to_string()),
        "{evidence}"
    );
    assert!(
        field(replacement_runner, "chosen").contains("acceptQuest")
            && field(replacement_runner, "chosen").contains("quest = 5261")
            && field(replacement_runner, "chosen")
                .contains(&DESTINATION_REPLACEMENT_GUID.to_string()),
        "{evidence}"
    );
    assert_eq!(
        evidence["replacement"]["replacement_quest_log"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "{evidence}"
    );
    assert!(
        evidence["attached"]["source_target_entity"]
            .as_array()
            .unwrap()
            .is_empty()
            && evidence["attached"]["source_target_spawn"]
                .as_array()
                .unwrap()
                .is_empty()
            && evidence["attached"]["actions"]
                .as_array()
                .unwrap()
                .is_empty()
            && evidence["attached"]["movement"]
                .as_array()
                .unwrap()
                .is_empty()
            && evidence["attached"]["attack"]
                .as_array()
                .unwrap()
                .is_empty()
            && evidence["attached"]["pending_cast"]
                .as_array()
                .unwrap()
                .is_empty()
            && evidence["attached"]["recovery_scan"]
                .as_array()
                .unwrap()
                .is_empty(),
        "source-local rows survived Transfer: {evidence}"
    );
}
