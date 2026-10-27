use super::*;
use crate::world_source::{sqlite_fixture, FixtureTable};
use rusqlite::types::Value::{Integer as N, Real as F, Text};
use std::collections::HashSet;

fn item_source() -> WorldSource {
    let numeric = "entry patch class subclass display_id quality flags buy_count buy_price sell_price inventory_type allowable_class allowable_race item_level required_level required_skill required_skill_rank required_reputation_faction required_reputation_rank max_count stackable container_slots dmg_min1 dmg_max1 armor holy_res fire_res nature_res frost_res shadow_res arcane_res delay spellid_1 spelltrigger_1 spellid_2 spelltrigger_2 spellid_3 spelltrigger_3 spellid_4 spelltrigger_4 spellid_5 spelltrigger_5 bonding page_text start_quest sheath random_property block max_durability bag_family food_type";
    let mut names = numeric
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    for n in 1..=10 {
        names.extend([format!("stat_type{n}"), format!("stat_value{n}")]);
    }
    names.reverse();
    let mut columns = names
        .iter()
        .map(|name| (name.as_str(), "INTEGER"))
        .collect::<Vec<_>>();
    columns.push(("name", "TEXT"));
    let item = |entry, patch, name: &str, spell| {
        let mut row = columns
            .iter()
            .map(|(column, _)| (*column, N(0)))
            .collect::<Vec<_>>();
        for (column, value) in [
            ("entry", N(entry)),
            ("patch", N(patch)),
            ("name", Text(name.into())),
            ("class", N(0)),
            ("spellid_1", N(spell)),
            ("stackable", N(20)),
            ("allowable_class", N(-1)),
            ("allowable_race", N(-1)),
        ] {
            row.iter_mut().find(|(name, _)| *name == column).unwrap().1 = value;
        }
        row
    };
    let old = item(1000, 9, "Old drink", 0);
    let current = item(1000, 10, "Fixture broth", 500);
    let ordinary = item(1001, 10, "Ordinary Water", 0);
    let vendor_columns = &[
        ("entry", "INTEGER"),
        ("slot", "INTEGER"),
        ("item", "INTEGER"),
        ("maxcount", "INTEGER"),
        ("incrtime", "INTEGER"),
        ("condition_id", "INTEGER"),
        ("itemflags", "INTEGER"),
    ];
    let offer = |entry, item, slot| {
        [
            ("entry", N(entry)),
            ("item", N(item)),
            ("slot", N(slot)),
            ("maxcount", N(0)),
            ("incrtime", N(0)),
            ("condition_id", N(0)),
            ("itemflags", N(0)),
        ]
    };
    let direct = offer(100, 1001, 1);
    let shared = offer(7, 1000, 2);
    let forbidden = offer(7, 1002, 3);
    let loot_columns = &[
        ("entry", "INTEGER"),
        ("item", "INTEGER"),
        ("ChanceOrQuestChance", "REAL"),
        ("groupid", "INTEGER"),
        ("mincountOrRef", "INTEGER"),
        ("maxcount", "INTEGER"),
        ("condition_id", "INTEGER DEFAULT 0"),
        ("patch_min", "INTEGER"),
        ("patch_max", "INTEGER"),
    ];
    sqlite_fixture(&[
        FixtureTable {
            name: "spell_template",
            columns: &[
                ("entry", "INTEGER"),
                ("build", "INTEGER"),
                ("effect1", "INTEGER"),
                ("effect2", "INTEGER"),
                ("effect3", "INTEGER"),
                ("effectApplyAuraName1", "INTEGER"),
                ("effectMiscValue1", "INTEGER"),
                ("effectApplyAuraName2", "INTEGER"),
                ("effectApplyAuraName3", "INTEGER"),
                ("effectMiscValue2", "INTEGER"),
                ("effectMiscValue3", "INTEGER"),
            ],
            key: &["entry", "build"],
            rows: &[&[
                ("entry", N(500)),
                ("build", N(5875)),
                ("effect1", N(6)),
                ("effect2", N(0)),
                ("effect3", N(0)),
                ("effectApplyAuraName1", N(85)),
                ("effectMiscValue1", N(0)),
            ]],
        },
        FixtureTable {
            name: "item_template",
            columns: &columns,
            key: &["entry", "patch"],
            rows: &[&old, &current, &ordinary],
        },
        FixtureTable {
            name: "creature_template",
            columns: &[
                ("entry", "INTEGER"),
                ("patch", "INTEGER"),
                ("vendor_id", "INTEGER"),
                ("loot_id", "INTEGER"),
            ],
            key: &["entry", "patch"],
            rows: &[&[
                ("entry", N(100)),
                ("patch", N(10)),
                ("vendor_id", N(7)),
                ("loot_id", N(3)),
            ]],
        },
        FixtureTable {
            name: "npc_vendor",
            columns: vendor_columns,
            key: &["entry", "item"],
            rows: &[&direct],
        },
        FixtureTable {
            name: "npc_vendor_template",
            columns: vendor_columns,
            key: &["entry", "item"],
            rows: &[&shared, &forbidden],
        },
        FixtureTable {
            name: "forbidden_items",
            columns: &[
                ("entry", "INTEGER"),
                ("patch", "INTEGER"),
                ("after_or_before", "INTEGER"),
            ],
            key: &["entry"],
            rows: &[&[
                ("entry", N(1002)),
                ("patch", N(10)),
                ("after_or_before", N(0)),
            ]],
        },
        FixtureTable {
            name: "item_enchantment_template",
            columns: &[
                ("entry", "INTEGER"),
                ("ench", "INTEGER"),
                ("chance", "REAL"),
                ("patch_min", "INTEGER"),
                ("patch_max", "INTEGER"),
            ],
            key: &["entry", "ench"],
            rows: &[],
        },
        FixtureTable {
            name: "reference_loot_template",
            columns: loot_columns,
            key: &["entry", "item"],
            rows: &[],
        },
        FixtureTable {
            name: "creature_loot_template",
            columns: loot_columns,
            key: &["entry", "item"],
            rows: &[&[
                ("entry", N(3)),
                ("item", N(1002)),
                ("ChanceOrQuestChance", F(100.0)),
                ("groupid", N(0)),
                ("mincountOrRef", N(1)),
                ("maxcount", N(1)),
                ("patch_min", N(0)),
                ("patch_max", N(10)),
            ]],
        },
    ])
    .unwrap()
}

#[test]
fn vmangos_items_select_versions_join_vendor_templates_and_exclude_forbidden_offers() {
    let source = item_source();
    let args = crate::parse_args_from([
        "--source",
        "vmangos",
        "--dump",
        "fixture.sqlite",
        "--family",
        "items",
        "--world-profile",
        "full-world",
    ])
    .unwrap()
    .world_import_args()
    .unwrap();
    let plan = build_vmangos_plan(&source, &args).unwrap();
    let sql = plan.stmts.join(";\n");
    assert_eq!(plan.stamps, [("items", 2)]);
    assert!(sql.contains("'Fixture broth'"));
    assert!(!sql.contains("Old drink"));
    assert!(sql.contains(",false,500,0,"));
    assert!(sql.contains("(1,100,1001,1,0),(2,100,1000,2,0)"));
    assert!(!sql.contains("INSERT INTO game_creature_loot"));
    assert!(!sql.contains("1002"));
    assert!(plan.reducer_batches(&args).unwrap().is_empty());
}

fn edit_source(source: &WorldSource, sql: &str) {
    let WorldSource::Vmangos(connection) = source else {
        panic!("SQLite fixture")
    };
    connection.pragma_update(None, "query_only", false).unwrap();
    connection.execute_batch(sql).unwrap();
    connection.pragma_update(None, "query_only", true).unwrap();
}

#[test]
fn patch_conditions_remove_unavailable_vendor_offers_and_creature_drops() {
    let source = item_source();
    edit_source(
        &source,
        "
        CREATE TABLE conditions (condition_entry INTEGER PRIMARY KEY, type INTEGER,
            value1 INTEGER, value2 INTEGER, flags INTEGER,
            value3 INTEGER DEFAULT 0, value4 INTEGER DEFAULT 0);
        INSERT INTO conditions (condition_entry,type,value1,value2,flags) VALUES
            (1,24,9,2,0), (2,24,10,0,0), (3,-3,1,0,0),
            (4,-1,2,3,0), (5,-2,1,2,0), (6,0,0,0,1),
            (7,-1,1,6,0), (8,9,42,0,0), (9,-2,2,8,0),
            (10,-3,10,0,0), (11,24,10,0,1), (12,-1,2,3,0), (13,-2,1,6,0);
        UPDATE conditions SET value4 = 1 WHERE condition_entry = 12;
        UPDATE conditions SET value3 = 2 WHERE condition_entry = 13;
        DELETE FROM npc_vendor;
        DELETE FROM npc_vendor_template;
        DELETE FROM creature_loot_template;
        DELETE FROM forbidden_items;
    ",
    );
    for condition in 1..=13 {
        edit_source(&source, &format!("
            INSERT INTO npc_vendor VALUES (100,{condition},{},0,0,{condition},0);
            INSERT INTO creature_loot_template
                (entry,item,ChanceOrQuestChance,groupid,mincountOrRef,maxcount,condition_id,patch_min,patch_max)
                VALUES (3,{},100,0,1,1,{condition},0,10);
        ", 1000 + condition, 1000 + condition));
    }
    let (_, loot, vendors) = build_items_and_loot(
        &source,
        &HashMap::from([(100, 3)]),
        &HashSet::from([100]),
        &HashSet::new(),
        &HashSet::new(),
    )
    .unwrap();
    let items = |rows: &[String]| {
        rows.iter()
            .map(|row| {
                row.trim_matches(['(', ')'])
                    .split(',')
                    .nth(2)
                    .unwrap()
                    .parse::<u64>()
                    .unwrap()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        items(&vendors),
        [1002, 1003, 1004, 1005, 1008, 1009, 1010, 1013]
    );
    assert_eq!(items(&loot), items(&vendors));
    for row in source.rows("npc_vendor").unwrap().iter() {
        let item: u32 = row.number("item").unwrap();
        assert_eq!(
            row.number::<u32>("condition_id").unwrap(),
            if item <= 1005 || item == 1013 {
                0
            } else {
                item - 1000
            }
        );
    }
    let findings = items_findings(&source, &HashSet::from([100])).unwrap();
    assert_eq!(findings["findings"][0]["occurrences"], 3);
}

#[test]
fn malformed_item_and_vendor_numbers_stop_before_emitting_an_offer() {
    for (sql, column) in [
        (
            "UPDATE item_template SET required_level = 'invalid'",
            "required_level",
        ),
        (
            "UPDATE item_template SET spelltrigger_1 = 999",
            "spelltrigger_1",
        ),
        ("UPDATE npc_vendor SET maxcount = -1", "maxcount"),
        ("UPDATE creature_loot_template SET groupid = -1", "groupid"),
    ] {
        let source = item_source();
        edit_source(&source, sql);
        let error = build_items_and_loot(
            &source,
            &HashMap::from([(100, 3)]),
            &HashSet::from([100]),
            &HashSet::new(),
            &HashSet::new(),
        )
        .unwrap_err();
        assert!(error.to_string().contains(column), "{error}");
    }
}
