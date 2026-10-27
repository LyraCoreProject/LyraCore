use super::*;
use crate::standalone_support;
use crate::world_source::{sqlite_fixture, FixtureTable};
use rusqlite::types::Value::{Integer as N, Text};

fn gameobject_source() -> WorldSource {
    let template_columns = &[
        ("entry", "INTEGER"),
        ("patch", "INTEGER"),
        ("type", "INTEGER"),
        ("displayId", "INTEGER"),
        ("name", "TEXT"),
        ("data0", "INTEGER"),
        ("data1", "INTEGER"),
        ("data2", "INTEGER"),
        ("data3", "INTEGER"),
        ("data5", "INTEGER"),
        ("size", "REAL"),
        ("script_name", "TEXT"),
    ];
    let template = |entry, patch, kind, name: &str| {
        [
            ("entry", N(entry)),
            ("patch", N(patch)),
            ("type", N(kind)),
            ("displayId", N(1)),
            ("name", Text(name.into())),
            ("data0", N(0)),
            ("data1", N(if entry == 5090042 { 5090042 } else { 0 })),
            ("data2", N(0)),
            ("data3", N(0)),
            ("data5", N(0)),
            ("size", N(1)),
            ("script_name", Text(String::new())),
        ]
    };
    let spawn_columns = &[
        ("guid", "INTEGER"),
        ("id", "INTEGER"),
        ("map", "INTEGER"),
        ("position_x", "REAL"),
        ("position_y", "REAL"),
        ("position_z", "REAL"),
        ("orientation", "REAL"),
        ("rotation0", "REAL"),
        ("rotation1", "REAL"),
        ("rotation2", "REAL"),
        ("rotation3", "REAL"),
        ("state", "INTEGER"),
        ("spawn_flags", "INTEGER"),
        ("patch_min", "INTEGER"),
        ("patch_max", "INTEGER"),
        ("visibility_mod", "REAL"),
        ("spawntimesecsmin", "INTEGER"),
        ("spawntimesecsmax", "INTEGER"),
        ("animprogress", "INTEGER"),
    ];
    let spawn = |guid, entry, state, flags, patch_max| {
        [
            ("guid", N(guid)),
            ("id", N(entry)),
            ("map", N(0)),
            ("position_x", N(0)),
            ("position_y", N(0)),
            ("position_z", N(0)),
            ("orientation", N(0)),
            ("rotation0", N(0)),
            ("rotation1", N(0)),
            ("rotation2", N(0)),
            ("rotation3", N(0)),
            ("state", N(state)),
            ("spawn_flags", N(flags)),
            ("patch_min", N(0)),
            ("patch_max", N(patch_max)),
            ("visibility_mod", N(0)),
            ("spawntimesecsmin", N(300)),
            ("spawntimesecsmax", N(300)),
            ("animprogress", N(100)),
        ]
    };
    sqlite_fixture(&[
        FixtureTable {
            name: "game_event_gameobject",
            columns: &[("guid", "INTEGER"), ("event", "INTEGER")],
            key: &["guid", "event"],
            rows: &[&[("guid", N(1)), ("event", N(1))]],
        },
        FixtureTable {
            name: "pool_gameobject",
            columns: &[
                ("guid", "INTEGER"),
                ("patch_min", "INTEGER"),
                ("patch_max", "INTEGER"),
            ],
            key: &["guid"],
            rows: &[],
        },
        FixtureTable {
            name: "gameobject_template",
            columns: template_columns,
            key: &["entry", "patch"],
            rows: &[
                &template(7, 0, 0, "Old door"),
                &template(7, 10, 0, "Door"),
                &template(8, 10, 25, "Fishing hole"),
                &template(1731, 10, 3, "Copper node"),
                &template(5090042, 10, 3, "Fixture mug"),
            ],
        },
        FixtureTable {
            name: "gameobject",
            columns: spawn_columns,
            key: &["guid"],
            rows: &[
                &spawn(1, 7, 1, 0, 10),
                &spawn(2, 7, 0, 0, 10),
                &spawn(3, 7, 1, 2, 10),
                &spawn(4, 7, 1, 0, 9),
                &spawn(5, 8, 1, 0, 10),
                &spawn(6, 1731, 1, 0, 10),
                &spawn(7, 7, 2, 0, 10),
                &spawn(5090040, 5090042, 0, 0, 10),
            ],
        },
    ])
    .unwrap()
}

fn gameobject_plan(source: &WorldSource) -> DumpPlan {
    let args = crate::parse_args_from([
        "--source",
        "vmangos",
        "--dump",
        "fixture.sqlite",
        "--family",
        "gameobjects",
        "--world-profile",
        "full-world",
    ])
    .unwrap()
    .world_import_args()
    .unwrap();
    build_vmangos_plan(source, &args).unwrap()
}

#[test]
fn vmangos_gameobjects_select_versions_and_ranges_skip_disabled_and_translate_states() {
    let source = gameobject_source();
    let plan = gameobject_plan(&source);
    assert_eq!(plan.go_row_count, 5);
    let manifest: serde_json::Value =
        serde_json::from_str(plan.world_source_manifest.as_ref().unwrap()).unwrap();
    assert!(manifest["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|finding| finding["code"] == "gameobjects.game-event" && finding["occurrences"] == 1));
    let rows = plan.go_batches[0].split(';').collect::<Vec<_>>();
    assert_eq!(rows[0], format!("{},7,0,0,0,0,0,0,0,0,0,0", go_guid(1)));
    assert_eq!(rows[1], format!("{},7,0,0,0,0,0,1,0,0,0,0", go_guid(2)));
    assert_eq!(rows[2], format!("{},1731,0,0,0,0,0,0,0,0,0,0", go_guid(6)));
    assert_eq!(rows[3], format!("{},7,0,0,0,0,0,2,0,0,0,0", go_guid(7)));
    let sql = plan.stmts.join(";");
    assert!(!sql.contains("Old door"));
    assert!(!sql.contains("Fishing hole"));
    assert!(sql.contains("(1731,25,1,'Copper node',2770,1,186,300,100,0,1)"));
}

#[test]
#[ignore = "requires the local SpacetimeDB 2.7.1 CLI and debug Module"]
fn imported_open_chest_can_be_looted() {
    let plan = gameobject_plan(&gameobject_source());
    let mut shard = standalone_support::Standalone::start("vmangos-open-chest");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call("debug_teleport", &["1", "0", "0", "0", "0", "0"]);
    for statement in plan.stmts {
        shard.assert_sql(&statement);
    }
    for batch in plan.go_batches {
        shard.assert_call(
            "import_gameobjects_append",
            &[&serde_json::to_string(&batch).unwrap()],
        );
    }
    shard.assert_sql("INSERT INTO game_gameobject_loot (id,loot_id,item_entry,chance_bp,count,group_id,quest_only) VALUES (5090040,5090042,25,10000,1,0,false)");
    let guid = go_guid(5090040).to_string();
    shard.assert_call(
        "gw_use_gameobject",
        &[&standalone_support::actor("1"), &guid],
    );
    let loot = shard.query_rows(&format!(
        "SELECT item_entry,count FROM game_corpse_loot WHERE corpse_guid = {guid}"
    ));
    assert_eq!(loot.len(), 1);
    assert_eq!(loot[0]["item_entry"], "25");
    assert_eq!(loot[0]["count"], "1");
}

#[test]
fn malformed_gameobject_numbers_do_not_create_an_unusable_spawn() {
    for (sql, column) in [
        (
            "UPDATE gameobject_template SET displayId = 4294967296 WHERE entry = 5090042",
            "displayId",
        ),
        (
            "UPDATE gameobject SET guid = -1 WHERE guid = 5090040",
            "guid",
        ),
    ] {
        let source = gameobject_source();
        let WorldSource::Vmangos(connection) = &source else {
            panic!("SQLite fixture")
        };
        connection.pragma_update(None, "query_only", false).unwrap();
        connection.execute_batch(sql).unwrap();
        connection.pragma_update(None, "query_only", true).unwrap();
        let scope = WorldImportScope::canonical(WorldImportProfile::FullWorld).unwrap();
        let error = collect_gameobject_spawns(&source, &scope).err().unwrap();
        assert!(error.to_string().contains(column), "{error}");
    }
}
