use super::*;
use crate::world_source::{sqlite_fixture, FixtureTable, WorldSource};
use rusqlite::types::Value::{Integer as N, Text};

fn quest<'a>(
    columns: &[(&'a str, &str)],
    entry: i64,
    patch: i64,
    title: &str,
) -> Vec<(&'a str, rusqlite::types::Value)> {
    let mut row = columns
        .iter()
        .map(|(name, kind)| {
            (
                *name,
                if *kind == "TEXT" {
                    Text(String::new())
                } else {
                    N(0)
                },
            )
        })
        .collect::<Vec<_>>();
    for (name, value) in [
        ("entry", N(entry)),
        ("patch", N(patch)),
        ("Title", Text(title.into())),
        ("Details", rusqlite::types::Value::Null),
        ("OfferRewardText", Text("NULL".into())),
        ("MinLevel", N(5)),
        ("QuestLevel", N(8)),
        ("RewXP", N(123)),
        ("RewMailTemplateId", N(5)),
        ("RewMailDelaySecs", N(12)),
        ("RewMailMoney", N(34)),
        ("RewSpellCast", N(500)),
        ("ReqCreatureOrGOId1", N(200)),
        ("ReqCreatureOrGOCount1", N(2)),
        ("ReqSpellCast1", N(700)),
        ("ReqCreatureOrGOId2", N(-300)),
        ("ReqCreatureOrGOCount2", N(1)),
        ("ReqItemId1", N(900)),
        ("ReqItemCount1", N(3)),
        ("RewItemId1", N(901)),
        ("RewItemCount1", N(1)),
        ("RewChoiceItemId1", N(902)),
        ("RewChoiceItemCount1", N(1)),
    ] {
        row.iter_mut().find(|(key, _)| *key == name).unwrap().1 = value;
    }
    row
}

fn source() -> WorldSource {
    let mut names = "entry patch ZoneOrSort MinLevel QuestLevel Type RequiredClasses RequiredRaces LimitTime SpecialFlags PrevQuestId NextQuestInChain SrcItemId SrcItemCount RewRepFaction1 RewRepValue1 RewRepFaction2 RewRepValue2 RewOrReqMoney RewSpell RewSpellCast RewMoneyMaxLevel RewMailTemplateId RewMailDelaySecs RewMailMoney RewXP".split_whitespace().map(str::to_string).collect::<Vec<_>>();
    for slot in 1..=4 {
        for prefix in [
            "ReqCreatureOrGOId",
            "ReqCreatureOrGOCount",
            "ReqSpellCast",
            "ReqItemId",
            "ReqItemCount",
            "RewItemId",
            "RewItemCount",
        ] {
            names.push(format!("{prefix}{slot}"));
        }
    }
    for slot in 1..=6 {
        names.extend([
            format!("RewChoiceItemId{slot}"),
            format!("RewChoiceItemCount{slot}"),
        ]);
    }
    names.reverse();
    let mut columns = names
        .iter()
        .map(|name| (name.as_str(), "INTEGER"))
        .collect::<Vec<_>>();
    for text in [
        "Title",
        "Details",
        "Objectives",
        "OfferRewardText",
        "RequestItemsText",
    ] {
        columns.push((text, "TEXT"));
    }
    let relation_columns = &[
        ("quest", "INTEGER"),
        ("id", "INTEGER"),
        ("patch_min", "INTEGER"),
        ("patch_max", "INTEGER"),
    ];
    let relation = |entry, quest, last| {
        [
            ("id", N(entry)),
            ("quest", N(quest)),
            ("patch_min", N(10)),
            ("patch_max", N(last)),
        ]
    };
    let spell = |build, target| {
        [
            ("entry", N(500)),
            ("build", N(build)),
            ("effect1", N(36)),
            ("effect2", N(0)),
            ("effect3", N(0)),
            ("effectTriggerSpell1", N(target)),
            ("effectTriggerSpell2", N(0)),
            ("effectTriggerSpell3", N(0)),
        ]
    };
    let loot_columns = &[
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
    let loot = |entry, item| {
        [
            ("entry", N(entry)),
            ("item", N(item)),
            ("ChanceOrQuestChance", N(100)),
            ("groupid", N(0)),
            ("mincountOrRef", N(1)),
            ("maxcount", N(1)),
            ("condition_id", N(0)),
            ("patch_min", N(0)),
            ("patch_max", N(10)),
        ]
    };
    sqlite_fixture(&[
        FixtureTable {
            name: "quest_template",
            columns: &columns,
            key: &["entry", "patch"],
            rows: &[
                &quest(&columns, 100, 9, "Old"),
                &quest(&columns, 100, 10, "Snow 雪"),
                &quest(&columns, 100, 11, "Future"),
                &quest(&columns, 101, 10, "No selected relation"),
            ],
        },
        FixtureTable {
            name: "creature_questrelation",
            columns: relation_columns,
            key: &["id", "quest"],
            rows: &[&relation(200, 100, 10)],
        },
        FixtureTable {
            name: "creature_involvedrelation",
            columns: relation_columns,
            key: &["id", "quest"],
            rows: &[&relation(200, 100, 10)],
        },
        FixtureTable {
            name: "gameobject_questrelation",
            columns: relation_columns,
            key: &["id", "quest"],
            rows: &[&[
                ("id", N(300)),
                ("quest", N(101)),
                ("patch_min", N(0)),
                ("patch_max", N(9)),
            ]],
        },
        FixtureTable {
            name: "gameobject_involvedrelation",
            columns: relation_columns,
            key: &["id", "quest"],
            rows: &[],
        },
        FixtureTable {
            name: "spell_template",
            columns: &[
                ("entry", "INTEGER"),
                ("build", "INTEGER"),
                ("effect1", "INTEGER"),
                ("effect2", "INTEGER"),
                ("effect3", "INTEGER"),
                ("effectTriggerSpell1", "INTEGER"),
                ("effectTriggerSpell2", "INTEGER"),
                ("effectTriggerSpell3", "INTEGER"),
            ],
            key: &["entry", "build"],
            rows: &[&spell(5860, 599), &spell(5875, 600), &spell(6000, 601)],
        },
        FixtureTable {
            name: "areatrigger_involvedrelation",
            columns: &[("id", "INTEGER"), ("quest", "INTEGER")],
            key: &["id"],
            rows: &[&[("id", N(400)), ("quest", N(100))]],
        },
        FixtureTable {
            name: "mail_loot_template",
            columns: loot_columns,
            key: &["entry", "item"],
            rows: &[&loot(5, 903), &loot(6, 904)],
        },
        FixtureTable {
            name: "forbidden_items",
            columns: &[
                ("entry", "INTEGER"),
                ("patch", "INTEGER"),
                ("after_or_before", "INTEGER"),
            ],
            key: &["entry", "patch"],
            rows: &[&[
                ("entry", N(904)),
                ("patch", N(10)),
                ("after_or_before", N(0)),
            ]],
        },
    ])
    .unwrap()
}

#[test]
fn named_quests_select_revisions_relations_mail_money_xp_and_learn_spells() {
    let source = source();
    let quests = build_quests(
        &source,
        &HashSet::from([200]),
        &HashSet::from([300]),
        &HashSet::from([300]),
        &HashSet::from([900]),
        None,
    )
    .unwrap();
    assert_eq!(quests.entries, HashSet::from([100]));
    assert!(quests.templates[0].starts_with("(100,5,8,'Snow 雪',0,123,"));
    assert_eq!(quests.texts, ["(100,'','','NULL','')"]);
    assert_eq!(quests.reward_mail, ["(100,5,12,0,34)"]);
    assert_eq!(quests.reward_spells, ["(100,600)"]);
    assert_eq!(
        quests.objectives,
        [
            "(1,100,0,0,200,2)",
            "(2,100,1,2,300,1)",
            "(3,100,2,1,900,3)",
            "(4,100,3,3,400,1)"
        ]
    );
    assert_eq!(quests.cast_objectives, ["(1,100,0,700)"]);
    assert_eq!(quests.relations, ["(1,200,100,0)", "(2,200,100,1)"]);
    let (mail, items) = build_mail_loot(&source, &HashSet::from([5, 6])).unwrap();
    assert_eq!(mail, ["(5,903,1)"]);
    assert_eq!(items, HashSet::from([903]));
}

#[test]
fn local_givers_do_not_change_the_selected_quest_catalogue() {
    let source = source();
    let quests = build_quests(
        &source,
        &HashSet::from([200]),
        &HashSet::from([300]),
        &HashSet::from([300]),
        &HashSet::from([900]),
        Some((&HashSet::new(), &HashSet::new())),
    )
    .unwrap();
    assert_eq!(quests.entries, HashSet::from([100]));
    assert!(quests.relations.is_empty());
    assert_eq!(quests.reward_mail, ["(100,5,12,0,34)"]);
}

#[test]
fn local_givers_use_selected_enabled_spawn_choices_inside_the_profile() {
    let columns = &[
        ("guid", "INTEGER"),
        ("id", "INTEGER"),
        ("id2", "INTEGER"),
        ("id3", "INTEGER"),
        ("id4", "INTEGER"),
        ("id5", "INTEGER"),
        ("map", "INTEGER"),
        ("position_x", "REAL"),
        ("position_y", "REAL"),
        ("position_z", "REAL"),
        ("spawn_flags", "INTEGER"),
        ("patch_min", "INTEGER"),
        ("patch_max", "INTEGER"),
    ];
    let row = |guid, x, flags, last| {
        [
            ("guid", N(guid)),
            ("id", N(guid * 100)),
            ("id2", N(guid * 100 + 1)),
            ("id3", N(0)),
            ("id4", N(0)),
            ("id5", N(0)),
            ("map", N(0)),
            ("position_x", N(x)),
            ("position_y", N(-132)),
            ("position_z", N(83)),
            ("spawn_flags", N(flags)),
            ("patch_min", N(0)),
            ("patch_max", N(last)),
        ]
    };
    let source = sqlite_fixture(&[FixtureTable {
        name: "creature",
        columns,
        key: &["guid"],
        rows: &[
            &row(1, -8949, 0, 10),
            &row(2, 0, 0, 10),
            &row(3, -8949, 2, 10),
            &row(4, -8949, 0, 9),
        ],
    }])
    .unwrap();
    let scope = WorldImportScope::canonical(WorldImportProfile::StartingEastern).unwrap();
    assert_eq!(
        placed_creatures(&source, &scope).unwrap(),
        HashSet::from([100, 101])
    );
}

#[test]
fn classic_nullable_quest_text_keeps_its_existing_output() {
    let dump = "INSERT INTO `creature_questrelation` (`id`, `quest`) VALUES (200,100); INSERT INTO `quest_template` (`entry`, `Title`, `Details`, `Objectives`, `OfferRewardText`, `RequestItemsText`) VALUES (100,'Fixture',NULL,'',NULL,NULL);";
    let quests = build_quests(
        dump,
        &HashSet::from([200]),
        &HashSet::new(),
        &HashSet::new(),
        &HashSet::new(),
        None,
    )
    .unwrap();
    assert_eq!(quests.texts, ["(100,'','','','')"]);
}

#[test]
fn event_quests_wait_for_credit_unless_an_area_objective_supplies_it() {
    let quest = |entry, flags| {
        [
            ("entry", N(entry)),
            ("patch", N(10)),
            ("SpecialFlags", N(flags)),
        ]
    };
    let source = sqlite_fixture(&[
        FixtureTable {
            name: "quest_template",
            columns: &[
                ("entry", "INTEGER"),
                ("patch", "INTEGER"),
                ("SpecialFlags", "INTEGER"),
            ],
            key: &["entry", "patch"],
            rows: &[
                &quest(100, 2),
                &quest(101, 3),
                &quest(102, 1),
                &quest(103, 2),
            ],
        },
        FixtureTable {
            name: "areatrigger_involvedrelation",
            columns: &[("id", "INTEGER"), ("quest", "INTEGER")],
            key: &["id"],
            rows: &[&[("id", N(400)), ("quest", N(100))]],
        },
    ])
    .unwrap();
    assert_eq!(
        event_requirements(&source, &HashSet::from([100, 101, 102])).unwrap(),
        ["(1,101)"]
    );
}
