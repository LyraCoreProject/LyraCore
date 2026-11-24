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
        ("data6", "INTEGER"),
        ("size", "REAL"),
        ("script_name", "TEXT"),
        ("mingold", "INTEGER"),
        ("maxgold", "INTEGER"),
    ];
    let template = |entry, patch, kind, name: &str| {
        let ritual = kind == 18;
        [
            ("entry", N(entry)),
            ("patch", N(patch)),
            ("type", N(kind)),
            ("displayId", N(1)),
            ("name", Text(name.into())),
            ("data0", N(if ritual { 3 } else { 0 })),
            (
                "data1",
                N(if entry == 5090042 {
                    5090042
                } else if kind == 10 {
                    -1
                } else if ritual {
                    7720
                } else {
                    0
                }),
            ),
            ("data2", N(if ritual { 698 } else { 0 })),
            ("data3", N(0)),
            ("data5", N(0)),
            ("data6", N(i64::from(ritual))),
            ("size", N(1)),
            ("script_name", Text(String::new())),
            ("mingold", N(if entry == 5090042 { 40 } else { 0 })),
            ("maxgold", N(if entry == 5090042 { 40 } else { 0 })),
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
                &template(5090043, 10, 10, "Fixture goober"),
                &template(36727, 0, 18, "Summoning Portal"),
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
    let args = crate::cli::world_import_args([
        "--dump",
        "fixture.sqlite",
        "--family",
        "gameobjects",
        "--world-profile",
        "full-world",
    ]);
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
    assert!(sql.contains("(5090043,10,1,'Fixture goober',0,0,0,0,0,0,1,0,0)"));
    assert!(sql.contains("(1731,25,1,'Copper node',2770,1,186,300,100,0,1,0,0)"));
    assert!(sql.contains("(5090042,3,1,'Fixture mug',0,5090042,0,300,0,0,1,40,40)"));
}

fn chest_template_row(source: &WorldSource) -> String {
    let scope =
        WorldImportScope::canonical(WorldImportProfile::parse("full-world").unwrap()).unwrap();
    let spawns = collect_gameobject_spawns(source, &scope).unwrap();
    gameobject_template_rows(&spawns, &Default::default())
        .unwrap()
        .templates
        .into_iter()
        .find(|row| row.starts_with("(5090042,"))
        .unwrap()
}

fn edit_gameobject_source(source: &WorldSource, sql: &str) {
    let connection = source.fixture_connection();
    connection.pragma_update(None, "query_only", false).unwrap();
    connection.execute_batch(sql).unwrap();
    connection.pragma_update(None, "query_only", true).unwrap();
}

#[test]
fn a_chest_template_takes_the_smallest_positive_spawn_time_of_its_entry() {
    let source = gameobject_source();
    edit_gameobject_source(
        &source,
        "INSERT INTO gameobject (guid,id,map,position_x,position_y,position_z,orientation,rotation0,rotation1,rotation2,rotation3,state,spawn_flags,patch_min,patch_max,visibility_mod,spawntimesecsmin,spawntimesecsmax,animprogress)
         SELECT 5090041,id,map,position_x,position_y,position_z,orientation,rotation0,rotation1,rotation2,rotation3,state,spawn_flags,patch_min,patch_max,visibility_mod,120,120,animprogress FROM gameobject WHERE guid = 5090040;
         INSERT INTO gameobject (guid,id,map,position_x,position_y,position_z,orientation,rotation0,rotation1,rotation2,rotation3,state,spawn_flags,patch_min,patch_max,visibility_mod,spawntimesecsmin,spawntimesecsmax,animprogress)
         SELECT 5090039,id,map,position_x,position_y,position_z,orientation,rotation0,rotation1,rotation2,rotation3,state,spawn_flags,patch_min,patch_max,visibility_mod,-1,-1,animprogress FROM gameobject WHERE guid = 5090040",
    );

    assert_eq!(
        chest_template_row(&source),
        "(5090042,3,1,'Fixture mug',0,5090042,0,120,0,0,1,40,40)"
    );
}

#[test]
fn a_chest_template_with_no_positive_spawn_time_never_respawns() {
    let source = gameobject_source();
    edit_gameobject_source(
        &source,
        "UPDATE gameobject SET spawntimesecsmin = -1, spawntimesecsmax = -1 WHERE guid = 5090040",
    );

    assert_eq!(
        chest_template_row(&source),
        "(5090042,3,1,'Fixture mug',0,5090042,0,0,0,0,1,40,40)"
    );
}

#[test]
fn a_summoning_ritual_template_imports_its_participants_spells_and_grouping() {
    let sql = gameobject_plan(&gameobject_source()).stmts.join(";");
    assert!(sql.contains("(36727,18,1,'Summoning Portal',0,0,0,0,0,0,1,0,0)"));
    assert!(sql.contains(
        "INSERT INTO game_gameobject_ritual (entry,required_participants,spell_id,anim_spell_id,persistent,casters_grouped) VALUES (36727,3,7720,698,false,true)"
    ));
}

#[test]
#[ignore = "requires the local SpacetimeDB 2.14.0 CLI and debug Module"]
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
    for (index, batch) in plan.go_batches.into_iter().enumerate() {
        shard.assert_call(
            if index == 0 {
                "import_gameobjects"
            } else {
                "import_gameobjects_append"
            },
            &[&serde_json::to_string(&batch).unwrap()],
        );
    }
    shard.assert_sql("INSERT INTO game_gameobject_loot (id,loot_id,item_entry,chance_bp,count,group_id,quest_only,min_count) VALUES (5090040,5090042,25,10000,1,0,false,0)");
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
        let connection = source.fixture_connection();
        connection.pragma_update(None, "query_only", false).unwrap();
        connection.execute_batch(sql).unwrap();
        connection.pragma_update(None, "query_only", true).unwrap();
        let scope =
            WorldImportScope::canonical(WorldImportProfile::parse("full-world").unwrap()).unwrap();
        let error = collect_gameobject_spawns(&source, &scope).err().unwrap();
        assert!(error.to_string().contains(column), "{error}");
    }
}
