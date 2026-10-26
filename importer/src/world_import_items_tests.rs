use super::*;
use crate::world_source::{sqlite_fixture, FixtureTable};
use rusqlite::types::Value::{Integer as N, Real as F, Text};

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
    assert!(sql.contains(",true,500,0,"));
    assert_eq!(sql.matches(",true,").count(), 1);
    assert!(sql.contains("(1,100,1001,1,0),(2,100,1000,2,0)"));
    assert!(!sql.contains("INSERT INTO game_creature_loot"));
    assert!(!sql.contains("1002"));
    assert!(plan.reducer_batches(&args).unwrap().is_empty());
}
