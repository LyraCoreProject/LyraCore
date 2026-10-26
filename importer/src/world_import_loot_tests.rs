use super::*;
use crate::world_source::{sqlite_fixture, FixtureTable};
use rusqlite::types::Value::{Integer as N, Real as F};

#[test]
fn vmangos_loot_selects_patch_rows_expands_nested_references_and_excludes_forbidden_items() {
    let columns = &[
        ("entry", "INTEGER"),
        ("item", "INTEGER"),
        ("ChanceOrQuestChance", "REAL"),
        ("groupid", "INTEGER"),
        ("mincountOrRef", "INTEGER"),
        ("maxcount", "INTEGER"),
        ("condition_id", "INTEGER"),
        ("patch_min", "INTEGER"),
        ("patch_max", "INTEGER"),
    ];
    let drop = |entry, item, chance, reference, maximum_patch| {
        [
            ("entry", N(entry)),
            ("item", N(item)),
            ("ChanceOrQuestChance", F(chance)),
            ("groupid", N(0)),
            ("mincountOrRef", N(reference)),
            ("maxcount", N(1)),
            ("condition_id", N(0)),
            ("patch_min", N(0)),
            ("patch_max", N(maximum_patch)),
        ]
    };
    let leaf = drop(20, 100, 100.0, 1, 10);
    let forbidden_leaf = drop(20, 101, 100.0, 1, 10);
    let nested = drop(30, 0, 100.0, -20, 10);
    let referenced = drop(10, 0, 40.0, -30, 10);
    let quest = drop(10, 103, -5.0, 1, 10);
    let old = drop(10, 102, 100.0, 1, 9);
    let direct_forbidden = drop(10, 101, 100.0, 1, 10);
    let drop_rows = [&referenced[..], &quest[..], &old[..], &direct_forbidden[..]];
    let ref_rows = [&leaf[..], &forbidden_leaf[..], &nested[..]];
    let mut tables = vec![
        FixtureTable {
            name: "forbidden_items",
            columns: &[
                ("entry", "INTEGER"),
                ("patch", "INTEGER"),
                ("after_or_before", "INTEGER"),
            ],
            key: &["entry"],
            rows: &[&[
                ("entry", N(101)),
                ("patch", N(10)),
                ("after_or_before", N(0)),
            ]],
        },
        FixtureTable {
            name: "reference_loot_template",
            columns,
            key: &[],
            rows: &ref_rows,
        },
    ];
    for name in [
        "creature_loot_template",
        "pickpocketing_loot_template",
        "skinning_loot_template",
        "gameobject_loot_template",
        "fishing_loot_template",
    ] {
        tables.push(FixtureTable {
            name,
            columns,
            key: &[],
            rows: &drop_rows,
        });
    }
    let source = sqlite_fixture(&tables).unwrap();
    let creatures = HashMap::from([(7, 10)]);
    let loot = build_loot_family_rows(&source, &creatures, &creatures, &[10]).unwrap();
    assert_eq!(
        loot.pickpocket_rows,
        ["(1,7,100,4000,1,1000000,false)", "(2,7,103,500,1,0,true)"]
    );
    assert_eq!(
        loot.skinning_rows,
        ["(1,10,100,4000,1,1000000)", "(2,10,103,500,1,0)"]
    );
    assert_eq!(
        loot.gameobject_loot_rows,
        ["(1,10,100,4000,1,1000000,false)", "(2,10,103,500,1,0,true)"]
    );
    assert_eq!(
        loot.fishing_rows,
        ["(1,10,100,4000,1,1000000)", "(2,10,103,500,1,0)"]
    );
    let creature_loot = parse_creature_drops(&source).unwrap();
    assert_eq!(
        creature_loot[&10],
        [(100, 4000, 1, 1000000, false), (103, 500, 1, 0, true)]
    );
}

#[test]
fn vmangos_equal_chance_group_shares_the_remainder_after_forbidden_items() {
    let columns = &[
        ("entry", "INTEGER"),
        ("item", "INTEGER"),
        ("ChanceOrQuestChance", "REAL"),
        ("groupid", "INTEGER"),
        ("mincountOrRef", "INTEGER"),
        ("maxcount", "INTEGER"),
        ("patch_min", "INTEGER"),
        ("patch_max", "INTEGER"),
    ];
    let member = |item, chance| {
        [
            ("entry", N(7)),
            ("item", N(item)),
            ("ChanceOrQuestChance", F(chance)),
            ("groupid", N(1)),
            ("mincountOrRef", N(1)),
            ("maxcount", N(1)),
            ("patch_min", N(0)),
            ("patch_max", N(10)),
        ]
    };
    let source = sqlite_fixture(&[
        FixtureTable {
            name: "forbidden_items",
            columns: &[
                ("entry", "INTEGER"),
                ("patch", "INTEGER"),
                ("after_or_before", "INTEGER"),
            ],
            key: &[],
            rows: &[&[
                ("entry", N(103)),
                ("patch", N(10)),
                ("after_or_before", N(0)),
            ]],
        },
        FixtureTable {
            name: "creature_loot_template",
            columns,
            key: &[],
            rows: &[
                &member(100, 25.0),
                &member(101, 0.0),
                &member(102, 0.0),
                &member(103, 0.0),
            ],
        },
        FixtureTable {
            name: "reference_loot_template",
            columns,
            key: &[],
            rows: &[],
        },
    ])
    .unwrap();
    assert_eq!(
        parse_creature_drops(&source).unwrap()[&7],
        [
            (100, 2500, 1, 1, false),
            (101, 3750, 1, 1, false),
            (102, 3750, 1, 1, false)
        ]
    );
}
