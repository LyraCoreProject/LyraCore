use super::*;
use crate::world_source::{sqlite_fixture, FixtureTable};
use rusqlite::types::Value::{Integer as N, Text};

#[test]
fn vmangos_gameobjects_select_versions_and_ranges_skip_disabled_and_translate_states() {
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
            ("data1", N(0)),
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
    let source = sqlite_fixture(&[
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
            ],
        },
    ])
    .unwrap();
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
    let plan = build_vmangos_plan(&source, &args).unwrap();
    assert_eq!(plan.go_row_count, 4);
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
    assert_eq!(
        plan.reducer_batches(&args).unwrap()[0].0,
        "import_gameobjects"
    );
}
