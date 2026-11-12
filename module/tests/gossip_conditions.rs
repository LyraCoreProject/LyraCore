mod support;

use lyracore_package_delta::fixture_ids::{
    FIXTURE_BLADE, SCENARIO_QUEST, SCENARIO_QUESTGIVER_ENTRY, SCENARIO_VENDOR_ENTRY,
};
use lyracore_shared::constants::{gossip_option, npc_flags};
use lyracore_shared::gossip::ConditionKind;
use support::Standalone;

const OPTION: u32 = 509_9001;
const CONDITION: u32 = 509_9001;

fn xp(shard: &Standalone) -> u32 {
    shard.query_rows("SELECT xp FROM game_world_entity WHERE guid = 1")[0]["xp"]
        .parse()
        .unwrap()
}

type Outcome = Result<(), &'static str>;

const ALLOWED: Outcome = Ok(());
const MENU_CHANGED: Outcome = Err("gossip menu has changed");
const SERVICE_GONE: Outcome = Err("gossip service is no longer available");
const NOT_IN_MENU: Outcome = Err("gossip option does not belong to this creature's active menu");
const ACTION_CHANGED: Outcome = Err("gossip option action has changed");
const CONDITION_FAILS: Outcome = Err("gossip option is no longer available");

#[track_caller]
fn select(shard: &Standalone, npc: &str, row: u32, outcome: Outcome) {
    select_from(shard, npc, row, 0, gossip_option::VENDOR, outcome);
}

#[track_caller]
fn select_from(
    shard: &Standalone,
    npc: &str,
    row: u32,
    source_menu_id: u32,
    action: u32,
    outcome: Outcome,
) {
    let before = xp(shard);
    let result = shard.call(
        "debug_gossip_select",
        &[
            "1",
            npc,
            "0",
            &row.to_string(),
            &source_menu_id.to_string(),
            &action.to_string(),
        ],
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    match outcome {
        Ok(()) => assert!(result.status.success(), "{stderr}"),
        Err(reason) => assert!(
            !result.status.success() && stderr.contains(reason),
            "the click should refuse with {reason}: {stderr}"
        ),
    }
    assert_eq!(
        xp(shard),
        before + u32::from(outcome.is_ok()),
        "only an eligible click may run the Runtime Script"
    );
}

fn fixture(name: &str) -> (Standalone, String) {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_sql(&format!(
        "UPDATE game_quest_template SET repeatable = false WHERE entry = {SCENARIO_QUEST}"
    ));
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call(
        "debug_spawn_at_feet",
        &["1", &SCENARIO_VENDOR_ENTRY.to_string(), "2"],
    );
    let npc = shard.query_rows(&format!(
        "SELECT guid FROM game_world_entity WHERE entry = {SCENARIO_VENDOR_ENTRY}"
    ))[0]["guid"]
        .clone();
    shard.insert_row(
        "game_gossip_option",
        &[
            ("row_id", &OPTION),
            ("entry", &SCENARIO_VENDOR_ENTRY),
            ("text", &"Quest supplies"),
            ("action", &3),
            ("cond_type", &7),
            ("cond_value_1", &CONDITION),
        ],
    );
    shard.insert_row(
        "game_gossip_condition",
        &[
            ("condition_id", &CONDITION),
            ("kind", &ConditionKind::QuestTaken.code()),
            ("value_1", &SCENARIO_QUEST),
            ("value_2", &1),
        ],
    );
    let artifact = serde_json::json!({
        "kind": "script", "version": 1, "package": "test_fixture.gossip",
        "source_hash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "scripts": [{ "script_id": 100_901, "name": "gossip.clicked", "event": "on_gossip_select", "priority": 0, "enabled": true, "source": "grant_xp(event.actor, 1)" }]
    }).to_string();
    shard.assert_call(
        "apply_package_deltas",
        &["\"script\"", &serde_json::to_string(&artifact).unwrap()],
    );

    (shard, npc)
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn imported_conditions_gate_current_quest_state_and_the_hook_in_one_transaction() {
    let rewarded = ConditionKind::QuestRewarded.code();
    let not = ConditionKind::Not.code();
    let (shard, npc) = fixture("gossip-conditions");
    select(&shard, &npc, OPTION, CONDITION_FAILS);
    shard.assert_call("debug_grant_quest", &["1", &SCENARIO_QUEST.to_string()]);
    select(&shard, &npc, OPTION, ALLOWED);
    shard.assert_sql(&format!(
        "UPDATE game_gossip_condition SET value2 = 2 WHERE condition_id = {CONDITION}"
    ));
    select(&shard, &npc, OPTION, CONDITION_FAILS);
    shard.assert_sql("UPDATE game_quest_objective SET required_count = 0 WHERE id = 5090000");
    select(&shard, &npc, OPTION, ALLOWED);
    shard.assert_sql(&format!(
        "UPDATE game_character_quest SET rewarded = true WHERE quest_entry = {SCENARIO_QUEST}"
    ));
    select(&shard, &npc, OPTION, CONDITION_FAILS);
    shard.assert_sql(&format!(
        "UPDATE game_gossip_condition SET kind = {rewarded}, value2 = 0 WHERE condition_id = {CONDITION}"
    ));
    select(&shard, &npc, OPTION, ALLOWED);

    shard.assert_sql(&format!(
        "UPDATE game_gossip_option SET entry = {SCENARIO_QUESTGIVER_ENTRY} WHERE row_id = {OPTION}"
    ));
    select(&shard, &npc, OPTION, NOT_IN_MENU);
    shard.assert_sql(&format!(
        "UPDATE game_gossip_option SET entry = {SCENARIO_VENDOR_ENTRY} WHERE row_id = {OPTION}"
    ));
    select(&shard, &npc, OPTION + 1, NOT_IN_MENU);
    shard.assert_sql(&format!("UPDATE game_gossip_condition SET kind = {not}, value1 = 5099002, value2 = 0 WHERE condition_id = {CONDITION}"));
    shard.insert_row(
        "game_gossip_condition",
        &[
            ("condition_id", &5099002),
            ("kind", &ConditionKind::Unsupported(99).code()),
            ("flags", &1),
        ],
    );
    select(
        &shard,
        &npc,
        OPTION,
        Err("gossip option condition is unavailable: Unsupported(99)"),
    );
    shard.assert_sql("DELETE FROM game_gossip_condition WHERE condition_id = 5099002");
    select(
        &shard,
        &npc,
        OPTION,
        Err("gossip option condition is unavailable: Missing(5099002)"),
    );
    shard.assert_sql(&format!(
        "UPDATE game_gossip_condition SET value1 = {CONDITION} WHERE condition_id = {CONDITION}"
    ));
    select(
        &shard,
        &npc,
        OPTION,
        Err("gossip option condition is unavailable: Cycle(5099001)"),
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_menu_switch_refuses_colliding_rows_and_synthesized_options_before_the_hook() {
    let always = ConditionKind::Always.code();
    const FIRST_MENU: u32 = 509_9001;
    const SECOND_MENU: u32 = 509_9002;
    let (shard, npc) = fixture("gossip-menu-source");
    shard.assert_sql(&format!("UPDATE game_gossip_condition SET kind = {always}, value1 = 0, value2 = 0 WHERE condition_id = {CONDITION}"));
    shard.insert_rows(
        "game_gossip_menu_profile",
        &[
            &[("menu_id", &FIRST_MENU), ("text_id", &1)],
            &[("menu_id", &SECOND_MENU), ("text_id", &1)],
        ],
    );
    shard.insert_row(
        "game_gossip_menu_profile_option",
        &[
            ("row_id", &OPTION),
            ("menu_id", &FIRST_MENU),
            ("text", &"Profile supplies"),
            ("action", &3),
        ],
    );
    select_from(&shard, &npc, OPTION, 0, gossip_option::VENDOR, ALLOWED);

    let entity = &shard.query_rows(&format!(
        "SELECT map_id,instance_id FROM game_world_entity WHERE guid = {npc}"
    ))[0];
    shard.insert_row(
        "game_creature_gossip_menu_override",
        &[
            ("creature_guid", &npc),
            ("menu_id", &FIRST_MENU),
            ("map_id", &entity["map_id"]),
            ("instance_id", &entity["instance_id"]),
        ],
    );
    select_from(&shard, &npc, OPTION, 0, gossip_option::VENDOR, MENU_CHANGED);
    select_from(
        &shard,
        &npc,
        u32::MAX,
        0,
        gossip_option::VENDOR,
        MENU_CHANGED,
    );
    select_from(
        &shard,
        &npc,
        OPTION,
        FIRST_MENU,
        gossip_option::VENDOR,
        ALLOWED,
    );

    shard.assert_sql(&format!("UPDATE game_gossip_menu_profile_option SET menu_id = {SECOND_MENU} WHERE row_id = {OPTION}"));
    shard.assert_sql(&format!("UPDATE game_creature_gossip_menu_override SET menu_id = {SECOND_MENU} WHERE creature_guid = {npc}"));
    select_from(
        &shard,
        &npc,
        OPTION,
        FIRST_MENU,
        gossip_option::VENDOR,
        MENU_CHANGED,
    );
    select_from(
        &shard,
        &npc,
        u32::MAX,
        FIRST_MENU,
        gossip_option::VENDOR,
        MENU_CHANGED,
    );
    select_from(
        &shard,
        &npc,
        OPTION,
        SECOND_MENU,
        gossip_option::VENDOR,
        ALLOWED,
    );

    shard.assert_sql(&format!(
        "DELETE FROM game_creature_gossip_menu_override WHERE creature_guid = {npc}"
    ));
    select_from(
        &shard,
        &npc,
        OPTION,
        SECOND_MENU,
        gossip_option::VENDOR,
        MENU_CHANGED,
    );
    select_from(&shard, &npc, OPTION, 0, gossip_option::VENDOR, ALLOWED);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_rewarded_repeatable_quest_is_neither_current_nor_rewarded_for_gossip() {
    let (shard, npc) = fixture("gossip-repeatable-quest");
    shard.assert_sql(&format!(
        "UPDATE game_quest_template SET repeatable = true WHERE entry = {SCENARIO_QUEST}"
    ));
    shard.assert_call("debug_grant_quest", &["1", &SCENARIO_QUEST.to_string()]);
    shard.assert_sql("UPDATE game_quest_objective SET required_count = 0 WHERE id = 5090000");
    shard.assert_call(
        "debug_spawn_at_feet",
        &["1", &SCENARIO_QUESTGIVER_ENTRY.to_string(), "2"],
    );
    let giver = &shard.query_rows(&format!(
        "SELECT guid FROM game_world_entity WHERE entry = {SCENARIO_QUESTGIVER_ENTRY}"
    ))[0]["guid"];
    shard.assert_call(
        "debug_turn_in_quest",
        &["1", giver, &SCENARIO_QUEST.to_string(), "0"],
    );
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT rewarded FROM game_character_quest WHERE quest_entry = {SCENARIO_QUEST}"
        ))[0]["rewarded"],
        "true"
    );

    for (kind, expected) in [
        (ConditionKind::QuestRewarded, false),
        (ConditionKind::QuestTaken, false),
        (ConditionKind::QuestAbsent, true),
    ] {
        let kind = kind.code();
        for flags in [0, 1] {
            shard.assert_sql(&format!("UPDATE game_gossip_condition SET kind = {kind}, value2 = 0, flags = {flags} WHERE condition_id = {CONDITION}"));
            let holds = (flags == 0) == expected;
            select(
                &shard,
                &npc,
                OPTION,
                if holds { ALLOWED } else { CONDITION_FAILS },
            );
        }
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn changed_service_actions_and_new_authored_services_refuse_stale_clicks() {
    let always = ConditionKind::Always.code();
    const MENU: u32 = 509_9001;
    let (shard, npc) = fixture("gossip-service-identity");
    shard.assert_sql(&format!("UPDATE game_gossip_condition SET kind = {always}, value1 = 0, value2 = 0 WHERE condition_id = {CONDITION}"));
    shard.insert_row(
        "game_gossip_menu_profile",
        &[("menu_id", &MENU), ("text_id", &1)],
    );
    shard.insert_row(
        "game_gossip_menu_profile_option",
        &[
            ("row_id", &OPTION),
            ("menu_id", &MENU),
            ("text", &"Profile supplies"),
            ("action", &3),
        ],
    );
    let entity = &shard.query_rows(&format!(
        "SELECT map_id,instance_id FROM game_world_entity WHERE guid = {npc}"
    ))[0];

    for (table, source) in [
        ("game_gossip_option", 0),
        ("game_gossip_menu_profile_option", MENU),
    ] {
        if source != 0 {
            shard.insert_row(
                "game_creature_gossip_menu_override",
                &[
                    ("creature_guid", &npc),
                    ("menu_id", &MENU),
                    ("map_id", &entity["map_id"]),
                    ("instance_id", &entity["instance_id"]),
                ],
            );
        }
        select_from(&shard, &npc, OPTION, source, gossip_option::VENDOR, ALLOWED);
        shard.assert_sql(&format!(
            "UPDATE {table} SET action = 1 WHERE row_id = {OPTION}"
        ));
        select_from(
            &shard,
            &npc,
            OPTION,
            source,
            gossip_option::VENDOR,
            ACTION_CHANGED,
        );
        select_from(&shard, &npc, OPTION, source, gossip_option::GOSSIP, ALLOWED);
        select_from(
            &shard,
            &npc,
            u32::MAX,
            source,
            gossip_option::VENDOR,
            ALLOWED,
        );
        shard.assert_sql(&format!(
            "UPDATE {table} SET action = 3, cond_type = 3 WHERE row_id = {OPTION}"
        ));
        select_from(
            &shard,
            &npc,
            u32::MAX,
            source,
            gossip_option::VENDOR,
            SERVICE_GONE,
        );
        select_from(
            &shard,
            &npc,
            OPTION,
            source,
            gossip_option::VENDOR,
            CONDITION_FAILS,
        );
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn synthesized_services_require_current_capability_and_resolvable_stock() {
    let (shard, npc) = fixture("gossip-service-capability");
    shard.assert_sql(&format!(
        "DELETE FROM game_gossip_option WHERE row_id = {OPTION}"
    ));
    select(&shard, &npc, u32::MAX, ALLOWED);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET npc_flags = {} WHERE guid = {npc}",
        npc_flags::GOSSIP
    ));
    select(&shard, &npc, u32::MAX, SERVICE_GONE);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET npc_flags = {} WHERE guid = {npc}",
        npc_flags::GOSSIP | npc_flags::VENDOR
    ));
    select(&shard, &npc, u32::MAX, ALLOWED);
    shard.assert_sql(&format!(
        "DELETE FROM game_npc_vendor WHERE creature_entry = {SCENARIO_VENDOR_ENTRY}"
    ));
    select(&shard, &npc, u32::MAX, SERVICE_GONE);
    shard.insert_row(
        "game_npc_vendor",
        &[
            ("id", &5099001),
            ("creature_entry", &SCENARIO_VENDOR_ENTRY),
            ("item_entry", &5099999),
        ],
    );
    select(&shard, &npc, u32::MAX, SERVICE_GONE);
    shard.assert_sql(&format!(
        "UPDATE game_npc_vendor SET item_entry = {FIXTURE_BLADE} WHERE id = 5099001"
    ));
    select(&shard, &npc, u32::MAX, ALLOWED);

    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET npc_flags = {} WHERE guid = {npc}",
        npc_flags::GOSSIP | npc_flags::INNKEEPER
    ));
    select_from(&shard, &npc, u32::MAX, 0, gossip_option::INNKEEPER, ALLOWED);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET npc_flags = {} WHERE guid = {npc}",
        npc_flags::GOSSIP
    ));
    select_from(
        &shard,
        &npc,
        u32::MAX,
        0,
        gossip_option::INNKEEPER,
        SERVICE_GONE,
    );
    select_from(
        &shard,
        &npc,
        u32::MAX,
        0,
        gossip_option::GOSSIP,
        SERVICE_GONE,
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn authored_services_require_current_capability_before_the_hook() {
    const MENU: u32 = 509_9001;
    let (shard, npc) = fixture("gossip-authored-capability");
    shard.assert_sql(&format!(
        "UPDATE game_creature_template SET trainer_type = 0, trainer_class = 1 WHERE entry = {SCENARIO_VENDOR_ENTRY}"
    ));
    shard.assert_sql("UPDATE game_world_entity SET level = 20 WHERE guid = 1");
    shard.assert_sql(&format!(
        "UPDATE game_gossip_option SET cond_type = 0 WHERE row_id = {OPTION}"
    ));
    shard.insert_row(
        "game_gossip_menu_profile",
        &[("menu_id", &MENU), ("text_id", &1)],
    );
    shard.insert_row(
        "game_gossip_menu_profile_option",
        &[
            ("row_id", &OPTION),
            ("menu_id", &MENU),
            ("text", &"Profile supplies"),
            ("action", &3),
        ],
    );
    let entity = &shard.query_rows(&format!(
        "SELECT map_id,instance_id FROM game_world_entity WHERE guid = {npc}"
    ))[0];

    for (table, source) in [
        ("game_gossip_option", 0),
        ("game_gossip_menu_profile_option", MENU),
    ] {
        if source != 0 {
            shard.insert_row(
                "game_creature_gossip_menu_override",
                &[
                    ("creature_guid", &npc),
                    ("menu_id", &MENU),
                    ("map_id", &entity["map_id"]),
                    ("instance_id", &entity["instance_id"]),
                ],
            );
        }
        for (action, flag) in [
            (gossip_option::VENDOR, npc_flags::VENDOR),
            (gossip_option::INNKEEPER, npc_flags::INNKEEPER),
            (gossip_option::TRAINER, npc_flags::TRAINER),
            (gossip_option::UNLEARNTALENTS, npc_flags::TRAINER),
            (gossip_option::TAXI, npc_flags::TAXI),
            (gossip_option::BANKER, npc_flags::BANKER),
            (gossip_option::PETITIONER, npc_flags::PETITIONER),
            (gossip_option::TABARDDESIGNER, npc_flags::TABARDDESIGNER),
        ] {
            shard.assert_sql(&format!(
                "UPDATE {table} SET action = {action} WHERE row_id = {OPTION}"
            ));
            shard.assert_sql(&format!(
                "UPDATE game_world_entity SET npc_flags = {} WHERE guid = {npc}",
                npc_flags::GOSSIP | flag
            ));
            select_from(&shard, &npc, OPTION, source, action, ALLOWED);
            shard.assert_sql(&format!(
                "UPDATE game_world_entity SET npc_flags = {} WHERE guid = {npc}",
                npc_flags::GOSSIP
            ));
            select_from(&shard, &npc, OPTION, source, action, SERVICE_GONE);
        }
    }

    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET npc_flags = {} WHERE guid = {npc}",
        npc_flags::GOSSIP | npc_flags::VENDOR
    ));
    shard.assert_sql(&format!(
        "UPDATE game_gossip_menu_profile_option SET action = 3 WHERE row_id = {OPTION}"
    ));
    shard.assert_sql(&format!(
        "DELETE FROM game_npc_vendor WHERE creature_entry = {SCENARIO_VENDOR_ENTRY}"
    ));
    select_from(
        &shard,
        &npc,
        OPTION,
        MENU,
        gossip_option::VENDOR,
        SERVICE_GONE,
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn trainer_options_repeat_current_class_and_talent_level_gates_before_the_hook() {
    let (shard, npc) = fixture("gossip-trainer-eligibility");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET npc_flags = {} WHERE guid = {npc}",
        npc_flags::GOSSIP | npc_flags::TRAINER
    ));
    shard.assert_sql(&format!(
        "UPDATE game_gossip_option SET action = 5, cond_type = 0 WHERE row_id = {OPTION}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_creature_template SET trainer_type = 0, trainer_class = 1 WHERE entry = {SCENARIO_VENDOR_ENTRY}"
    ));
    select_from(&shard, &npc, OPTION, 0, gossip_option::TRAINER, ALLOWED);
    shard.assert_sql(&format!(
        "UPDATE game_creature_template SET trainer_class = 8 WHERE entry = {SCENARIO_VENDOR_ENTRY}"
    ));
    select_from(
        &shard,
        &npc,
        OPTION,
        0,
        gossip_option::TRAINER,
        SERVICE_GONE,
    );
    shard.assert_sql(&format!(
        "UPDATE game_gossip_option SET action = 16 WHERE row_id = {OPTION}"
    ));
    shard.assert_sql("UPDATE game_world_entity SET level = 10 WHERE guid = 1");
    select_from(
        &shard,
        &npc,
        OPTION,
        0,
        gossip_option::UNLEARNTALENTS,
        SERVICE_GONE,
    );
    shard.assert_sql(&format!(
        "UPDATE game_creature_template SET trainer_class = 1 WHERE entry = {SCENARIO_VENDOR_ENTRY}"
    ));
    select_from(
        &shard,
        &npc,
        OPTION,
        0,
        gossip_option::UNLEARNTALENTS,
        ALLOWED,
    );
    for trainer_type in [1, 2, 3] {
        shard.assert_sql(&format!(
            "UPDATE game_creature_template SET trainer_type = {trainer_type} WHERE entry = {SCENARIO_VENDOR_ENTRY}"
        ));
        select_from(
            &shard,
            &npc,
            OPTION,
            0,
            gossip_option::UNLEARNTALENTS,
            SERVICE_GONE,
        );
    }
    shard.assert_sql(&format!(
        "UPDATE game_creature_template SET trainer_type = 0 WHERE entry = {SCENARIO_VENDOR_ENTRY}"
    ));
    shard.assert_sql("UPDATE game_world_entity SET level = 9 WHERE guid = 1");
    select_from(
        &shard,
        &npc,
        OPTION,
        0,
        gossip_option::UNLEARNTALENTS,
        SERVICE_GONE,
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_missing_quest_subject_refuses_selection_even_with_retained_progress() {
    let (shard, npc) = fixture("gossip-missing-quest");
    shard.assert_call("debug_grant_quest", &["1", &SCENARIO_QUEST.to_string()]);
    select(&shard, &npc, OPTION, ALLOWED);
    shard.assert_sql(&format!(
        "DELETE FROM game_quest_template WHERE entry = {SCENARIO_QUEST}"
    ));
    for kind in [
        ConditionKind::QuestRewarded,
        ConditionKind::QuestTaken,
        ConditionKind::QuestAbsent,
    ] {
        let kind = kind.code();
        for flags in [0, 1] {
            shard.assert_sql(&format!("UPDATE game_gossip_condition SET kind = {kind}, value2 = 0, flags = {flags} WHERE condition_id = {CONDITION}"));
            select(
                &shard,
                &npc,
                OPTION,
                Err("gossip option condition is unavailable: MissingQuest(50900)"),
            );
        }
    }
    shard.assert_sql(&format!(
        "DELETE FROM game_character_quest WHERE quest_entry = {SCENARIO_QUEST}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_gossip_condition SET flags = 0 WHERE condition_id = {CONDITION}"
    ));
    select(
        &shard,
        &npc,
        OPTION,
        Err("gossip option condition is unavailable: MissingQuest(50900)"),
    );
}
