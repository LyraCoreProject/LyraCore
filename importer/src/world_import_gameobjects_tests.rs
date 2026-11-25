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
            name: "pool_template",
            columns: &[
                ("entry", "INTEGER"),
                ("max_limit", "INTEGER"),
                ("flags", "INTEGER"),
                ("patch_min", "INTEGER"),
                ("patch_max", "INTEGER"),
            ],
            key: &["entry", "patch_min", "patch_max"],
            rows: &[],
        },
        FixtureTable {
            name: "pool_pool",
            columns: &[
                ("pool_id", "INTEGER"),
                ("mother_pool", "INTEGER"),
                ("chance", "REAL"),
                ("flags", "INTEGER"),
            ],
            key: &["pool_id"],
            rows: &[],
        },
        FixtureTable {
            name: "pool_gameobject",
            columns: &[
                ("guid", "INTEGER"),
                ("pool_entry", "INTEGER"),
                ("chance", "REAL"),
                ("flags", "INTEGER"),
                ("patch_min", "INTEGER"),
                ("patch_max", "INTEGER"),
            ],
            key: &["guid"],
            rows: &[],
        },
        FixtureTable {
            name: "pool_gameobject_template",
            columns: &[
                ("id", "INTEGER"),
                ("pool_entry", "INTEGER"),
                ("chance", "REAL"),
                ("flags", "INTEGER"),
                ("patch_min", "INTEGER"),
                ("patch_max", "INTEGER"),
            ],
            key: &["id"],
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
    assert_eq!(
        rows[0],
        format!("{},7,0,0,0,0,0,0,0,0,0,0,300,300", go_guid(1))
    );
    assert_eq!(
        rows[1],
        format!("{},7,0,0,0,0,0,1,0,0,0,0,300,300", go_guid(2))
    );
    assert_eq!(
        rows[2],
        format!("{},1731,0,0,0,0,0,0,0,0,0,0,300,300", go_guid(6))
    );
    assert_eq!(
        rows[3],
        format!("{},7,0,0,0,0,0,2,0,0,0,0,300,300", go_guid(7))
    );
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

fn packed_spawn_row(source: &WorldSource, db_guid: u32) -> String {
    let scope =
        WorldImportScope::canonical(WorldImportProfile::parse("full-world").unwrap()).unwrap();
    let prefix = format!("{},", go_guid(db_guid));
    collect_gameobject_spawns(source, &scope)
        .unwrap()
        .placements
        .iter()
        .map(GoPlacement::packed_row)
        .find(|row| row.starts_with(&prefix))
        .unwrap()
}

#[test]
fn each_chest_spawn_carries_its_own_respawn_range() {
    let source = gameobject_source();
    edit_gameobject_source(
        &source,
        "INSERT INTO gameobject (guid,id,map,position_x,position_y,position_z,orientation,rotation0,rotation1,rotation2,rotation3,state,spawn_flags,patch_min,patch_max,visibility_mod,spawntimesecsmin,spawntimesecsmax,animprogress)
         SELECT 5090041,id,map,position_x,position_y,position_z,orientation,rotation0,rotation1,rotation2,rotation3,state,spawn_flags,patch_min,patch_max,visibility_mod,120,600,animprogress FROM gameobject WHERE guid = 5090040;
         INSERT INTO gameobject (guid,id,map,position_x,position_y,position_z,orientation,rotation0,rotation1,rotation2,rotation3,state,spawn_flags,patch_min,patch_max,visibility_mod,spawntimesecsmin,spawntimesecsmax,animprogress)
         SELECT 5090039,id,map,position_x,position_y,position_z,orientation,rotation0,rotation1,rotation2,rotation3,state,spawn_flags,patch_min,patch_max,visibility_mod,-1,-1,animprogress FROM gameobject WHERE guid = 5090040",
    );

    assert!(packed_spawn_row(&source, 5090040).ends_with(",300,300"));
    assert!(packed_spawn_row(&source, 5090041).ends_with(",120,600"));
    assert!(
        packed_spawn_row(&source, 5090039).ends_with(",0,0"),
        "a summoned spawn has no range of its own"
    );
}

#[test]
fn a_spawn_respawn_range_keeps_its_maximum_at_or_above_its_minimum() {
    assert_eq!(spawn_respawn_range(300, 300), (300, 300));
    assert_eq!(spawn_respawn_range(600, 120), (600, 600));
    assert_eq!(spawn_respawn_range(0, 900), (0, 900));
    assert_eq!(spawn_respawn_range(0, 0), (0, 0));
    assert_eq!(spawn_respawn_range(-300, 300), (0, 0));
    assert_eq!(
        spawn_respawn_range(1, i64::from(u32::MAX) + 1),
        (1, u32::MAX)
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

const POOLED_CHESTS: [u32; 6] = [
    5_090_050, 5_090_051, 5_090_052, 5_090_053, 5_090_054, 5_090_055,
];

/// Copies of the fixture chest at x = its guid, with a respawn range and a rotation of their own.
fn pooled_chest_source(pool_sql: &str) -> WorldSource {
    let source = gameobject_source();
    let copies = POOLED_CHESTS
        .iter()
        .map(|guid| {
            format!(
                "INSERT INTO gameobject (guid,id,map,position_x,position_y,position_z,orientation,rotation0,rotation1,rotation2,rotation3,state,spawn_flags,patch_min,patch_max,visibility_mod,spawntimesecsmin,spawntimesecsmax,animprogress)
                 SELECT {guid},id,map,{guid},position_y,position_z,orientation,rotation0,rotation1,rotation2,0.5,state,spawn_flags,patch_min,patch_max,visibility_mod,120,600,animprogress FROM gameobject WHERE guid = 5090040"
            )
        })
        .collect::<Vec<_>>()
        .join(";");
    edit_gameobject_source(&source, &format!("{copies};{pool_sql}"));
    source
}

fn pool_member_row(guid: u32, pool: u32, weight: u32) -> String {
    format!(
        "({},{pool},5090042,0,{guid},0,0,0,{weight},0,0,0,0.5,120,600)",
        (1u64 << 40) | u64::from(guid)
    )
}

fn inserted_rows(plan: &DumpPlan, table: &str) -> String {
    plan.stmts
        .iter()
        .filter(|sql| sql.starts_with(&format!("INSERT INTO {table} (")))
        .map(|sql| sql.split_once(" VALUES ").unwrap().1)
        .collect::<Vec<_>>()
        .join(",")
}

fn static_guids(plan: &DumpPlan) -> Vec<u64> {
    plan.go_batches
        .iter()
        .flat_map(|batch| batch.split(';'))
        .map(|row| row.split(',').next().unwrap().parse().unwrap())
        .collect()
}

#[test]
fn a_limit_one_pool_gives_chanced_members_their_share_and_splits_the_rest_equally() {
    assert_eq!(
        gameobject_pools::pool_weights(1, &[6_000, 0, 0]),
        [12_000, 4_000, 4_000],
        "60 percent, then 20 and 20"
    );
    assert_eq!(gameobject_pools::pool_weights(1, &[10_000, 0]), [10_000, 0]);
    assert_eq!(gameobject_pools::pool_weights(1, &[0, 0, 0]), [1, 1, 1]);
}

#[test]
fn chances_past_a_whole_roll_leave_the_later_member_only_what_remains() {
    assert_eq!(
        gameobject_pools::pool_weights(1, &[9_500, 9_500]),
        [9_500, 500]
    );
}

#[test]
fn a_pool_with_a_limit_above_one_ignores_chances() {
    assert_eq!(gameobject_pools::pool_weights(2, &[6_000, 0]), [1, 1]);
    assert_eq!(
        gameobject_pools::pool_weights(3, &[2_500, 2_500, 0]),
        [1, 1, 1]
    );
}

#[test]
fn pooled_placements_import_as_pool_members_instead_of_static_spawns() {
    let source = pooled_chest_source(
        "INSERT INTO pool_template VALUES (900,1,0,0,10);
         INSERT INTO pool_gameobject VALUES (5090050,900,60,0,0,10),(5090051,900,0,0,0,10),(5090052,900,0,0,0,10)",
    );
    let plan = gameobject_plan(&source);

    assert_eq!(
        inserted_rows(&plan, "game_gameobject_pool"),
        "(900,1,false)"
    );
    assert_eq!(
        inserted_rows(&plan, "game_gameobject_pool_member"),
        [
            pool_member_row(5_090_050, 900, 12_000),
            pool_member_row(5_090_051, 900, 4_000),
            pool_member_row(5_090_052, 900, 4_000),
        ]
        .join(",")
    );
    assert_eq!(
        inserted_rows(&plan, "game_gameobject_pool_link"),
        "(900,0,0,0)"
    );
    let statics = static_guids(&plan);
    assert!(statics.contains(&go_guid(5_090_040)));
    for guid in [5_090_050, 5_090_051, 5_090_052] {
        assert!(
            !statics.contains(&go_guid(guid)),
            "pooled {guid} is no static spawn"
        );
    }
    assert!(
        statics.contains(&go_guid(5_090_053)),
        "an unpooled copy stays static"
    );
    assert_eq!(plan.pool_roll_batches, ["900"]);
}

#[test]
fn a_top_level_pool_whose_chances_miss_a_whole_roll_never_spawns() {
    let source = pooled_chest_source(
        "INSERT INTO pool_template VALUES (940,1,0,0,10);
         INSERT INTO pool_gameobject VALUES (5090050,940,60,0,0,10),(5090051,940,30,0,0,10)",
    );
    let plan = gameobject_plan(&source);

    assert_eq!(inserted_rows(&plan, "game_gameobject_pool_member"), "");
    assert!(plan.pool_roll_batches.is_empty());
    let statics = static_guids(&plan);
    assert!(!statics.contains(&go_guid(5_090_050)));
    assert!(!statics.contains(&go_guid(5_090_051)));
}

#[test]
fn a_slice_of_only_pooled_placements_still_sends_the_batch_that_clears_old_objects() {
    let source = pooled_chest_source(
        "DELETE FROM gameobject WHERE id != 5090042;
         INSERT INTO pool_template VALUES (930,2,0,0,10);
         INSERT INTO pool_gameobject_template VALUES (5090042,930,0,0,0,10)",
    );
    let plan = gameobject_plan(&source);

    assert_eq!(plan.go_row_count, 0);
    assert_eq!(plan.go_batches, [String::new()]);
}

#[test]
fn a_pooled_placement_that_does_not_start_ready_is_refused() {
    let source = gameobject_source();
    edit_gameobject_source(
        &source,
        "INSERT INTO pool_template VALUES (950,1,0,0,10);
         INSERT INTO pool_gameobject VALUES (2,950,0,0,0,10)",
    );
    let args = crate::cli::world_import_args([
        "--dump",
        "fixture.sqlite",
        "--family",
        "gameobjects",
        "--world-profile",
        "full-world",
    ]);

    let error = build_vmangos_plan(&source, &args).err().unwrap();

    assert!(error.to_string().contains("gameobject 2"), "{error}");
}

#[test]
fn pool_rolls_are_grouped_so_one_call_brings_up_at_most_250_objects() {
    assert_eq!(
        gameobject_pools::group_rolls([(1, 100), (2, 100), (3, 100), (4, 300), (5, 10)]),
        ["1;2", "3", "4", "5"]
    );
}

#[test]
fn a_nested_pool_imports_its_child_pools_with_their_weights_and_the_roots_map() {
    let source = pooled_chest_source(
        "INSERT INTO pool_template VALUES (910,1,0,0,10),(911,1,0,0,10),(912,2,0,0,10);
         INSERT INTO pool_pool VALUES (911,910,30,0),(912,910,0,0);
         INSERT INTO pool_gameobject VALUES (5090053,911,0,0,0,10),(5090054,912,0,0,0,10),(5090055,912,0,0,0,10)",
    );
    let plan = gameobject_plan(&source);

    assert_eq!(
        inserted_rows(&plan, "game_gameobject_pool"),
        "(910,1,false),(911,1,false),(912,2,false)"
    );
    assert_eq!(
        inserted_rows(&plan, "game_gameobject_pool_link"),
        "(910,0,0,0),(911,910,3000,0),(912,910,7000,0)"
    );
    assert_eq!(
        inserted_rows(&plan, "game_gameobject_pool_member"),
        [
            pool_member_row(5_090_053, 911, 1),
            pool_member_row(5_090_054, 912, 1),
            pool_member_row(5_090_055, 912, 1),
        ]
        .join(",")
    );
}

#[test]
fn a_placement_under_a_mother_pool_without_a_template_never_spawns() {
    let source = pooled_chest_source(
        "INSERT INTO pool_template VALUES (920,1,0,0,10);
         INSERT INTO pool_pool VALUES (920,921,0,0);
         INSERT INTO pool_gameobject VALUES (5090050,920,0,0,0,10)",
    );
    let plan = gameobject_plan(&source);

    assert_eq!(inserted_rows(&plan, "game_gameobject_pool_member"), "");
    assert!(!static_guids(&plan).contains(&go_guid(5_090_050)));
}

#[test]
fn a_template_pool_takes_every_placement_of_its_entry() {
    let source = pooled_chest_source(
        "INSERT INTO pool_template VALUES (930,2,0,0,10);
         INSERT INTO pool_gameobject_template VALUES (5090042,930,0,0,0,10)",
    );
    let plan = gameobject_plan(&source);

    let members = inserted_rows(&plan, "game_gameobject_pool_member");
    assert_eq!(members.matches("),(").count() + 1, 1 + POOLED_CHESTS.len());
    let statics = static_guids(&plan);
    assert!(!statics.contains(&go_guid(5_090_040)));
    assert!(statics.contains(&go_guid(1)), "the door is in no pool");
}

#[test]
fn a_gameobject_reimport_replaces_every_pool_row_before_it_writes_any() {
    let source = pooled_chest_source(
        "INSERT INTO pool_template VALUES (900,1,0,0,10);
         INSERT INTO pool_gameobject VALUES (5090050,900,0,0,0,10)",
    );
    let plan = gameobject_plan(&source);

    let position = |prefix: &str| {
        plan.stmts
            .iter()
            .position(|sql| sql.starts_with(prefix))
            .unwrap_or_else(|| panic!("no statement starts with {prefix}"))
    };
    let first_insert = [
        "game_gameobject_pool",
        "game_gameobject_pool_member",
        "game_gameobject_pool_link",
    ]
    .map(|table| position(&format!("INSERT INTO {table} (")))
    .into_iter()
    .min()
    .unwrap();
    for table in [
        "game_gameobject_pool",
        "game_gameobject_pool_member",
        "game_gameobject_pool_link",
    ] {
        assert!(
            position(&format!("DELETE FROM {table} WHERE")) < first_insert,
            "{table}"
        );
    }
    assert_eq!(
        gameobject_plan(&source).stmts,
        plan.stmts,
        "a re-run plans the same rows"
    );
}
