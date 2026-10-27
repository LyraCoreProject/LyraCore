use super::*;
use crate::world_source::{sqlite_fixture, FixtureTable, WorldSource};
use lyracore_shared::gossip::ConditionKind;
use rusqlite::types::Value::{self, Integer as N, Text};

fn option(
    columns: &[(&'static str, &str)],
    [menu, id, action, reference, condition, flags, money]: [i64; 7],
) -> Vec<(&'static str, Value)> {
    let mut row = columns
        .iter()
        .map(|(name, kind)| (*name, if *kind == "TEXT" { Value::Null } else { N(0) }))
        .collect::<Vec<_>>();
    for (name, value) in [
        ("menu_id", menu),
        ("id", id),
        ("option_id", action),
        ("option_broadcast_text", reference),
        ("condition_id", condition),
        ("npc_option_npcflag", flags),
        ("box_money", money),
    ] {
        row.iter_mut()
            .find(|(column, _)| *column == name)
            .unwrap()
            .1 = N(value);
    }
    row.iter_mut()
        .find(|(column, _)| *column == "option_text")
        .unwrap()
        .1 = Text("123".into());
    row
}

const BROADCAST_COLUMNS: &[(&str, &str)] = &[
    ("entry", "INTEGER"),
    ("chat_type", "INTEGER"),
    ("sound_id", "INTEGER"),
    ("language_id", "INTEGER"),
    ("emote_id1", "INTEGER"),
    ("emote_id2", "INTEGER"),
    ("emote_id3", "INTEGER"),
    ("emote_delay1", "INTEGER"),
    ("emote_delay2", "INTEGER"),
    ("emote_delay3", "INTEGER"),
    ("female_text", "TEXT"),
    ("male_text", "TEXT"),
];

fn broadcast_row() -> Vec<(&'static str, Value)> {
    let mut broadcast = BROADCAST_COLUMNS
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
        ("entry", N(50)),
        ("male_text", Text("雪's greeting".into())),
        ("female_text", Text("NULL".into())),
    ] {
        broadcast
            .iter_mut()
            .find(|(column, _)| *column == name)
            .unwrap()
            .1 = value;
    }
    broadcast
}

fn source() -> WorldSource {
    let broadcast = broadcast_row();
    let mut npc_names = vec!["ID".to_string()];
    for index in 0..8 {
        npc_names.extend([
            format!("Probability{index}"),
            format!("BroadcastTextID{index}"),
        ]);
    }
    let npc_columns = npc_names
        .iter()
        .map(|name| {
            (
                name.as_str(),
                if name.starts_with("Probability") {
                    "REAL"
                } else {
                    "INTEGER"
                },
            )
        })
        .collect::<Vec<_>>();
    let mut npc = npc_columns
        .iter()
        .map(|(name, kind)| {
            (
                *name,
                if *kind == "REAL" {
                    Value::Real(0.0)
                } else {
                    N(0)
                },
            )
        })
        .collect::<Vec<_>>();
    for (name, value) in [
        ("ID", N(70)),
        ("BroadcastTextID3", N(50)),
        ("Probability3", Value::Real(0.75)),
    ] {
        npc.iter_mut()
            .find(|(column, _)| *column == name)
            .unwrap()
            .1 = value;
    }
    let mut option_columns = [
        "menu_id",
        "id",
        "option_icon",
        "option_broadcast_text",
        "option_id",
        "npc_option_npcflag",
        "action_menu_id",
        "action_poi_id",
        "action_script_id",
        "box_coded",
        "box_money",
        "box_broadcast_text",
        "condition_id",
    ]
    .map(|name| (name, "INTEGER"))
    .to_vec();
    option_columns.extend([("option_text", "TEXT"), ("box_text", "TEXT")]);
    option_columns.reverse();
    let option = |menu, id, action, reference, condition, flags, money| {
        option(
            &option_columns,
            [menu, id, action, reference, condition, flags, money],
        )
    };
    let condition_columns = [
        "condition_entry",
        "type",
        "value1",
        "value2",
        "value3",
        "value4",
        "flags",
    ]
    .map(|name| (name, "INTEGER"));
    let condition = |id, kind, a, b, flags| {
        condition_columns
            .iter()
            .zip([id, kind, a, b, 0, 0, flags])
            .map(|((name, _), value)| (*name, N(value)))
            .collect::<Vec<_>>()
    };
    sqlite_fixture(&[
        FixtureTable {
            name: "npc_gossip",
            columns: &[("npc_guid", "INTEGER"), ("textid", "INTEGER")],
            key: &["npc_guid"],
            rows: &[],
        },
        FixtureTable {
            name: "broadcast_text",
            columns: BROADCAST_COLUMNS,
            key: &["entry"],
            rows: &[&broadcast],
        },
        FixtureTable {
            name: "npc_text",
            columns: &npc_columns,
            key: &["ID"],
            rows: &[&npc],
        },
        FixtureTable {
            name: "conditions",
            columns: &condition_columns,
            key: &["condition_entry"],
            rows: &[
                &condition(1, 24, 10, 0, 0),
                &condition(2, 24, 9, 2, 0),
                &condition(3, 24, 9, 2, 1),
                &condition(4, 54, -100, 0, 0),
                &condition(5, -3, 4, 0, 0),
                &condition(6, -2, 1, 5, 0),
                &condition(7, 9, 800, 2, 0),
            ],
        },
        FixtureTable {
            name: "gossip_menu",
            columns: &[
                ("text_id", "INTEGER"),
                ("entry", "INTEGER"),
                ("condition_id", "INTEGER"),
                ("script_id", "INTEGER"),
            ],
            key: &["entry", "text_id"],
            rows: &[
                &[
                    ("entry", N(900)),
                    ("text_id", N(70)),
                    ("condition_id", N(0)),
                    ("script_id", N(0)),
                ],
                &[
                    ("entry", N(900)),
                    ("text_id", N(69)),
                    ("condition_id", N(0)),
                    ("script_id", N(0)),
                ],
                &[
                    ("entry", N(901)),
                    ("text_id", N(70)),
                    ("condition_id", N(7)),
                    ("script_id", N(0)),
                ],
            ],
        },
        FixtureTable {
            name: "gossip_menu_option",
            columns: &option_columns,
            key: &["menu_id", "id"],
            rows: &[
                &option(900, 7, 3, 50, 1, 4, 0),
                &option(900, 2, 1, 0, 2, 1, 0),
                &option(900, 4, 2, 0, 0, 2, 0),
                &option(900, 9, 1, 0, 0, 1, 100),
                &option(0, 0, 1, 0, 0, 1, 0),
            ],
        },
        FixtureTable {
            name: "creature_template",
            columns: &[
                ("entry", "INTEGER"),
                ("gossip_menu_id", "INTEGER"),
                ("patch", "INTEGER"),
                ("npc_flags", "INTEGER"),
            ],
            key: &["entry", "patch"],
            rows: &[
                &[
                    ("entry", N(100)),
                    ("gossip_menu_id", N(901)),
                    ("patch", N(0)),
                    ("npc_flags", N(1)),
                ],
                &[
                    ("entry", N(100)),
                    ("gossip_menu_id", N(900)),
                    ("patch", N(10)),
                    ("npc_flags", N(1)),
                ],
                &[
                    ("entry", N(101)),
                    ("gossip_menu_id", N(0)),
                    ("patch", N(10)),
                    ("npc_flags", N(1)),
                ],
            ],
        },
    ])
    .unwrap()
}

#[test]
fn patch_constants_and_unsupported_predicates_keep_boolean_gates() {
    let conditions = gossip_conditions::read(&source()).unwrap();
    let evaluate = |id| {
        evaluate_reference(
            id,
            CharacterFacts::default(),
            |_| QuestState::Complete,
            |id| conditions.get(&id).copied(),
        )
    };
    assert_eq!(evaluate(1), Ok(true));
    assert_eq!(evaluate(2), Ok(false));
    assert_eq!(evaluate(3), Ok(true));
    for id in [4, 5, 6] {
        assert!(evaluate(id).is_err());
    }
    assert_eq!(conditions[&7].kind, ConditionKind::QuestTaken);
    assert_eq!(evaluate(7), Ok(true));
}

#[test]
fn named_gossip_rows_preserve_slots_literal_text_flags_and_default_options() {
    let args = crate::parse_args_from([
        "--dump",
        "fixture.sqlite",
        "--family",
        "gossip",
        "--world-profile",
        "full-world",
    ])
    .unwrap()
    .world_import_args()
    .unwrap();
    let plan = plan(&source(), &args).unwrap();
    let insert = |table| {
        plan.stmts
            .iter()
            .find(|sql| sql.starts_with(&format!("INSERT INTO {table} ")))
            .unwrap()
    };
    assert!(insert("game_gossip_menu").ends_with("(100,70)"));
    assert!(insert("game_gossip_menu_profile").ends_with("(900,70),(901,16777215)"));
    assert!(insert("game_npc_text_slot").ends_with("(1,70,3,'雪''s greeting','NULL',0.75)"));
    assert!(insert("game_gossip_option").ends_with(
        "(1,100,0,0,'123',1,0,7,2,0),(2,100,1,0,'123',1,0,3,0,0),(3,101,0,0,'123',1,0,0,0,0)"
    ));
    assert!(insert("game_gossip_menu_profile_option").contains("'雪''s greeting',3,0,7,1,0"));
    let manifest = plan.world_source_manifest.unwrap();
    for code in [
        "gossip.conditional-greeting",
        "gossip.option-payment",
        "gossip.condition-kind",
    ] {
        assert!(manifest.contains(code));
    }
}
