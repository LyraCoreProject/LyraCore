use super::*;
use crate::standalone_support;

#[test]
fn tavern_import_selects_patch_and_replaces_rows() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    connection.execute_batch("CREATE TABLE areatrigger_tavern (id INTEGER, patch_min INTEGER, PRIMARY KEY (id, patch_min)); INSERT INTO areatrigger_tavern VALUES (71,0),(71,10),(72,10),(73,11)").unwrap();
    let source = WorldSource::Vmangos(connection);
    let statements = build_tavern_area_trigger_sql(&source).unwrap();
    assert_eq!(
        statements,
        vec![
            "DELETE FROM game_tavern_area_trigger WHERE trigger_id >= 0".to_string(),
            "INSERT INTO game_tavern_area_trigger (trigger_id) VALUES (71),(72)".to_string(),
        ]
    );
}

fn source_args(profile: &str) -> (WorldSource, WorldImportArgs) {
    let path = std::env::var("LYRACORE_VMANGOS_SQLITE").unwrap();
    let client = std::env::var("LYRACORE_TEST_DBC").unwrap();
    let source = WorldSource::open(std::path::Path::new(&path)).unwrap();
    let args = crate::parse_args_from([
        "--dump",
        &path,
        "--dbc",
        &client,
        "--world-profile",
        profile,
    ])
    .unwrap()
    .world_import_args()
    .unwrap();
    (source, args)
}

#[test]
#[ignore = "requires LYRACORE_VMANGOS_SQLITE and LYRACORE_TEST_DBC"]
fn creature_ai_alone_owns_event_requirements() {
    for profile in ["full-world", "alliance-single"] {
        let (source, mut args) = source_args(profile);
        args.family = Some("quests".into());
        let quests = build_vmangos_plan(&source, &args).unwrap();
        assert!(!quests
            .stmts
            .iter()
            .any(|sql| sql.contains("game_quest_event_requirement")));
        args.family = Some("creature-ai".into());
        let ai = build_vmangos_plan(&source, &args).unwrap();
        assert!(ai
            .stmts
            .iter()
            .any(|sql| sql.starts_with("DELETE FROM game_quest_event_requirement")));
        let inserted: Vec<_> = ai
            .stmts
            .iter()
            .filter(|sql| sql.starts_with("INSERT INTO game_quest_event_requirement"))
            .collect();
        assert_eq!(inserted.len(), 1);
        assert_eq!(inserted[0].contains(",2460)"), profile == "full-world");
        assert!(inserted[0].contains(",2608)"));
    }
}

#[test]
#[ignore = "requires LYRACORE_VMANGOS_SQLITE, LYRACORE_TEST_DBC, SpacetimeDB and Wasm"]
fn full_world_source_loads_every_definition_and_spawn_locally() {
    let (source, args) = source_args("full-world");
    let plan = build_vmangos_plan(&source, &args).unwrap();
    assert_eq!(plan.spawn_row_count, 65_969);
    assert_eq!(
        plan.stmts
            .iter()
            .filter(|sql| sql.starts_with("DELETE FROM game_quest_event_requirement"))
            .count(),
        1
    );
    let mut node = standalone_support::Standalone::start("full-world-source");
    node.publish_module();
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["0"]);
    for (index, statement) in plan.stmts.iter().enumerate() {
        node.assert_sql(statement);
        if index % 100 == 0 {
            eprintln!(
                "full-world catalogue: {index}/{} statements",
                plan.stmts.len()
            );
        }
    }
    for (index, (reducer, batch)) in plan.reducer_batches(&args).unwrap().iter().enumerate() {
        if index % 50 == 0 {
            eprintln!("full-world reducer batch {index}: {reducer}");
        }
        node.assert_call(reducer, &[&serde_json::to_string(batch).unwrap()]);
    }
    assert_eq!(
        node.query_rows("SELECT entry FROM game_creature_template")
            .len(),
        10_376
    );
    let sources = node.query_rows(
        "SELECT guid, map_id FROM game_creature_spawn WHERE source_guid = 0 AND instance_id = 0",
    );
    assert_eq!(sources.len(), 65_969);
    let maps: std::collections::BTreeSet<_> = sources
        .iter()
        .map(|row| row["map_id"].parse::<u32>().unwrap())
        .collect();
    assert_eq!(maps.len(), 36);
    for map in lyracore_shared::instance::DUNGEON_MAPS
        .iter()
        .chain(lyracore_shared::instance::BATTLEGROUND_MAPS)
    {
        assert!(node
            .query_rows(&format!(
                "SELECT guid FROM game_world_entity WHERE map_id = {map} AND instance_id = 0"
            ))
            .is_empty());
    }
    assert!(
        standalone_support::poll_until(std::time::Duration::from_secs(180), || {
            std::thread::sleep(std::time::Duration::from_secs(2));
            [0, 1].into_iter().all(|map| {
                node.query_rows(&format!(
                    "SELECT guid FROM game_world_entity WHERE map_id = {map} AND instance_id = 0"
                ))
                .len()
                    > 1_500
            })
        }),
        "continent populations did not materialize within three minutes"
    );
    for (index, batch) in plan.spawn_batches.iter().enumerate() {
        let reducer = if index == 0 {
            "import_creature_spawns"
        } else {
            "import_creature_spawns_append"
        };
        node.assert_call(reducer, &[&serde_json::to_string(batch).unwrap()]);
    }
    assert_eq!(
        node.query_rows(
            "SELECT guid FROM game_creature_spawn WHERE source_guid = 0 AND instance_id = 0"
        )
        .len(),
        65_969
    );
}
