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
        ["(1,7,100,4000,1,0,false)", "(2,7,103,500,1,0,true)"]
    );
    assert_eq!(
        loot.skinning_rows,
        ["(1,10,100,4000,1,0)", "(2,10,103,500,1,0)"]
    );
    assert_eq!(
        loot.gameobject_loot_rows,
        ["(1,10,100,4000,1,0,false)", "(2,10,103,500,1,0,true)"]
    );
    assert_eq!(
        loot.fishing_rows,
        ["(1,10,100,4000,1,0)", "(2,10,103,500,1,0)"]
    );
    let creature_loot = parse_creature_drops(&source).unwrap();
    assert_eq!(
        creature_loot[&10],
        [(100, 4000, 1, 0, false), (103, 500, 1, 0, true)]
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
        ("condition_id", "INTEGER DEFAULT 0"),
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
            (100, 2500, 1, 1000001, false),
            (101, 3750, 1, 1000001, false),
            (102, 3750, 1, 1000001, false)
        ]
    );
}

fn loot_source(
    drops: &[(i64, i64, f64, i64, i64, i64)],
    refs: &[(i64, i64, f64, i64, i64, i64)],
) -> WorldSource {
    let row = |&(entry, item, chance, group, reference, count): &(i64, i64, f64, i64, i64, i64)| {
        vec![
            ("entry", N(entry)),
            ("item", N(item)),
            ("ChanceOrQuestChance", F(chance)),
            ("groupid", N(group)),
            ("mincountOrRef", N(reference)),
            ("maxcount", N(count)),
            ("condition_id", N(0)),
            ("patch_min", N(0)),
            ("patch_max", N(10)),
        ]
    };
    let drops = drops.iter().map(row).collect::<Vec<_>>();
    let refs = refs.iter().map(row).collect::<Vec<_>>();
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
    sqlite_fixture(&[
        FixtureTable {
            name: "forbidden_items",
            columns: &[
                ("entry", "INTEGER"),
                ("patch", "INTEGER"),
                ("after_or_before", "INTEGER"),
            ],
            key: &[],
            rows: &[],
        },
        FixtureTable {
            name: "creature_loot_template",
            columns,
            key: &[],
            rows: &drops.iter().map(Vec::as_slice).collect::<Vec<_>>(),
        },
        FixtureTable {
            name: "reference_loot_template",
            columns,
            key: &[],
            rows: &refs.iter().map(Vec::as_slice).collect::<Vec<_>>(),
        },
    ])
    .unwrap()
}

// The Module rolls group zero independently and selects one cumulative band per other group.
fn loot_probability(drops: &[(u64, u32, u32, u32, bool)], item: u64) -> f64 {
    let mut groups = std::collections::BTreeMap::<u32, (u32, u32)>::new();
    let mut miss = 1.0;
    for &(entry, chance, _, group, _) in drops {
        if group == 0 {
            if entry == item {
                miss *= 1.0 - f64::from(chance) / 10000.0;
            }
        } else {
            let (used, hit) = groups.entry(group).or_default();
            let width = chance.min(10000 - *used);
            if entry == item {
                *hit += width;
            }
            *used += width;
        }
    }
    for (_, hit) in groups.values() {
        miss *= 1.0 - f64::from(*hit) / 10000.0;
    }
    1.0 - miss
}

#[test]
fn referenced_herbs_keep_the_guaranteed_herb_and_independent_bonus() {
    let source = loot_source(
        &[(1, 0, 100.0, 0, -20, 1), (2, 0, 50.0, 0, -20, 1)],
        &[(20, 100, 100.0, 0, 1, 1), (20, 101, 50.0, 0, 1, 1)],
    );
    let drops = parse_creature_drops(&source).unwrap();
    assert_eq!(loot_probability(&drops[&1], 100), 1.0);
    assert_eq!(loot_probability(&drops[&1], 101), 0.5);
    assert!(drops[&1].iter().all(|drop| drop.3 == 0));
    assert_eq!(loot_probability(&drops[&2], 100), 0.5);
    assert_eq!(loot_probability(&drops[&2], 101), 0.25);
}

#[test]
fn repeated_references_offer_each_roll_including_nested_and_selected_groups() {
    let source = loot_source(
        &[(1, 0, 100.0, 0, -20, 2), (2, 0, 100.0, 2, -21, 3)],
        &[
            (20, 0, 100.0, 1, -21, 2),
            (21, 100, 50.0, 1, 1, 1),
            (21, 101, 50.0, 1, 1, 1),
            (21, 102, 100.0, 2, 1, 1),
        ],
    );
    let drops = parse_creature_drops(&source).unwrap();
    assert_eq!(loot_probability(&drops[&1], 100), 0.9375);
    assert_eq!(loot_probability(&drops[&1], 101), 0.9375);
    assert_eq!(loot_probability(&drops[&1], 102), 0.0);
    assert_eq!(
        drops[&2]
            .iter()
            .filter(|drop| drop.0 == 102)
            .map(|drop| drop.2)
            .sum::<u32>(),
        3
    );
    assert_eq!(loot_probability(&drops[&2], 100), 0.0);
}

#[test]
fn every_member_of_a_large_equal_group_can_drop_and_the_group_always_drops() {
    let members = (1..=631)
        .map(|item| (1, item, 0.0, 1, 1, 1))
        .collect::<Vec<_>>();
    let source = loot_source(&members, &[]);
    let drops = parse_creature_drops(&source).unwrap();
    let chances = (1..=631)
        .map(|item| loot_probability(&drops[&1], item))
        .collect::<Vec<_>>();
    assert!(chances
        .iter()
        .all(|&chance| (0.00149..=0.00161).contains(&chance)));
    assert!((chances.iter().sum::<f64>() - 1.0).abs() < 1e-12);
    assert_eq!(drops[&1].iter().filter(|drop| drop.1 == 16).count(), 535);
    assert_eq!(drops[&1].iter().filter(|drop| drop.1 == 15).count(), 96);
}

#[test]
fn a_gated_reference_splits_its_exact_share_among_equal_members() {
    let source = loot_source(
        &[(1, 0, 37.0, 0, -20, 1)],
        &[
            (20, 100, 10.0, 1, 1, 1),
            (20, 101, 0.0, 1, 1, 1),
            (20, 102, 0.0, 1, 1, 1),
            (20, 103, 0.0, 1, 1, 1),
        ],
    );
    let drops = parse_creature_drops(&source).unwrap();
    assert_eq!(drops[&1].iter().map(|drop| drop.1).sum::<u32>(), 3700);
    assert!((loot_probability(&drops[&1], 100) - 0.037).abs() < 1e-12);
    assert!((loot_probability(&drops[&1], 103) - 0.111).abs() < 1e-12);
}

#[test]
fn a_negative_chance_reference_keeps_its_children_available_to_everyone() {
    let source = loot_source(&[(1, 0, -100.0, 0, -20, 1)], &[(20, 100, 100.0, 0, 1, 1)]);
    let drops = parse_creature_drops(&source).unwrap();
    assert_eq!(loot_probability(&drops[&1], 100), 1.0);
    assert!(drops[&1].iter().all(|drop| !drop.4));
}

#[test]
fn a_repeated_reference_below_full_chance_is_refused() {
    let source = loot_source(&[(1, 0, 50.0, 0, -20, 2)], &[(20, 100, 100.0, 0, 1, 1)]);
    let error = parse_creature_drops(&source).unwrap_err().to_string();
    assert!(error.contains("repeats 2 times at 50%"), "{error}");
}
