mod support;

use lyracore_shared::constants::{gossip_option, npc_flags};
use lyracore_shared::gossip::ConditionKind;
use support::Standalone;

const OPTION: u32 = 509_9001;
const CONDITION: u32 = 509_9001;
const QUEST: u32 = 50_900;
const VENDOR: u32 = 51_004;

fn xp(shard: &Standalone) -> u32 {
    shard.query_rows("SELECT xp FROM game_world_entity WHERE guid = 1")[0]["xp"]
        .parse()
        .unwrap()
}

fn select(shard: &Standalone, npc: &str, row: u32, allowed: bool) {
    select_from(shard, npc, row, 0, gossip_option::VENDOR, allowed);
}

fn select_from(
    shard: &Standalone,
    npc: &str,
    row: u32,
    source_menu_id: u32,
    action: u32,
    allowed: bool,
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
    assert_eq!(
        result.status.success(),
        allowed,
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        xp(shard),
        before + u32::from(allowed),
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
        "UPDATE game_quest_template SET repeatable = false WHERE entry = {QUEST}"
    ));
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call("debug_spawn_at_feet", &["1", &VENDOR.to_string(), "2"]);
    let npc = shard.query_rows(&format!(
        "SELECT guid FROM game_world_entity WHERE entry = {VENDOR}"
    ))[0]["guid"]
        .clone();
    shard.assert_sql(&format!("INSERT INTO game_gossip_option (row_id,entry,option_index,icon,text,action,action_menu_id,cond_type,cond_value1,cond_value2) VALUES ({OPTION},{VENDOR},0,0,'Quest supplies',3,0,7,{CONDITION},0)"));
    shard.assert_sql(&format!("INSERT INTO game_gossip_condition (condition_id,kind,value1,value2,value3,value4,flags) VALUES ({CONDITION},{},{QUEST},1,0,0,0)", ConditionKind::QuestTaken.code()));
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
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn imported_conditions_gate_current_quest_state_and_the_hook_in_one_transaction() {
    let rewarded = ConditionKind::QuestRewarded.code();
    let not = ConditionKind::Not.code();
    let (shard, npc) = fixture("gossip-conditions");
    select(&shard, &npc, OPTION, false);
    shard.assert_call("debug_grant_quest", &["1", &QUEST.to_string()]);
    select(&shard, &npc, OPTION, true);
    shard.assert_sql(&format!(
        "UPDATE game_gossip_condition SET value2 = 2 WHERE condition_id = {CONDITION}"
    ));
    select(&shard, &npc, OPTION, false);
    shard.assert_sql("UPDATE game_quest_objective SET required_count = 0 WHERE id = 5090000");
    select(&shard, &npc, OPTION, true);
    shard.assert_sql(&format!(
        "UPDATE game_character_quest SET rewarded = true WHERE quest_entry = {QUEST}"
    ));
    select(&shard, &npc, OPTION, false);
    shard.assert_sql(&format!(
        "UPDATE game_gossip_condition SET kind = {rewarded}, value2 = 0 WHERE condition_id = {CONDITION}"
    ));
    select(&shard, &npc, OPTION, true);

    shard.assert_sql(&format!(
        "UPDATE game_gossip_option SET entry = 51003 WHERE row_id = {OPTION}"
    ));
    select(&shard, &npc, OPTION, false);
    shard.assert_sql(&format!(
        "UPDATE game_gossip_option SET entry = {VENDOR} WHERE row_id = {OPTION}"
    ));
    select(&shard, &npc, OPTION + 1, false);
    shard.assert_sql(&format!("UPDATE game_gossip_condition SET kind = {not}, value1 = 5099002, value2 = 0 WHERE condition_id = {CONDITION}"));
    shard.assert_sql(&format!("INSERT INTO game_gossip_condition (condition_id,kind,value1,value2,value3,value4,flags) VALUES (5099002,{},0,0,0,0,1)", ConditionKind::Unsupported(99).code()));
    select(&shard, &npc, OPTION, false);
    shard.assert_sql("DELETE FROM game_gossip_condition WHERE condition_id = 5099002");
    select(&shard, &npc, OPTION, false);
    shard.assert_sql(&format!(
        "UPDATE game_gossip_condition SET value1 = {CONDITION} WHERE condition_id = {CONDITION}"
    ));
    select(&shard, &npc, OPTION, false);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_menu_switch_refuses_colliding_rows_and_synthesized_options_before_the_hook() {
    let always = ConditionKind::Always.code();
    const FIRST_MENU: u32 = 509_9001;
    const SECOND_MENU: u32 = 509_9002;
    let (shard, npc) = fixture("gossip-menu-source");
    shard.assert_sql(&format!("UPDATE game_gossip_condition SET kind = {always}, value1 = 0, value2 = 0 WHERE condition_id = {CONDITION}"));
    shard.assert_sql(&format!("INSERT INTO game_gossip_menu_profile (menu_id,text_id) VALUES ({FIRST_MENU},1),({SECOND_MENU},1)"));
    shard.assert_sql(&format!("INSERT INTO game_gossip_menu_profile_option (row_id,menu_id,option_index,icon,text,action,action_menu_id,cond_type,cond_value1,cond_value2) VALUES ({OPTION},{FIRST_MENU},0,0,'Profile supplies',3,0,0,0,0)"));
    select_from(&shard, &npc, OPTION, 0, gossip_option::VENDOR, true);

    let entity = &shard.query_rows(&format!(
        "SELECT map_id,instance_id FROM game_world_entity WHERE guid = {npc}"
    ))[0];
    shard.assert_sql(&format!("INSERT INTO game_creature_gossip_menu_override (creature_guid,menu_id,map_id,instance_id) VALUES ({npc},{FIRST_MENU},{},{})", entity["map_id"], entity["instance_id"]));
    select_from(&shard, &npc, OPTION, 0, gossip_option::VENDOR, false);
    select_from(&shard, &npc, u32::MAX, 0, gossip_option::VENDOR, false);
    select_from(
        &shard,
        &npc,
        OPTION,
        FIRST_MENU,
        gossip_option::VENDOR,
        true,
    );

    shard.assert_sql(&format!("UPDATE game_gossip_menu_profile_option SET menu_id = {SECOND_MENU} WHERE row_id = {OPTION}"));
    shard.assert_sql(&format!("UPDATE game_creature_gossip_menu_override SET menu_id = {SECOND_MENU} WHERE creature_guid = {npc}"));
    select_from(
        &shard,
        &npc,
        OPTION,
        FIRST_MENU,
        gossip_option::VENDOR,
        false,
    );
    select_from(
        &shard,
        &npc,
        u32::MAX,
        FIRST_MENU,
        gossip_option::VENDOR,
        false,
    );
    select_from(
        &shard,
        &npc,
        OPTION,
        SECOND_MENU,
        gossip_option::VENDOR,
        true,
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
        false,
    );
    select_from(&shard, &npc, OPTION, 0, gossip_option::VENDOR, true);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_rewarded_repeatable_quest_is_neither_current_nor_rewarded_for_gossip() {
    let (shard, npc) = fixture("gossip-repeatable-quest");
    shard.assert_sql(&format!(
        "UPDATE game_quest_template SET repeatable = true WHERE entry = {QUEST}"
    ));
    shard.assert_call("debug_grant_quest", &["1", &QUEST.to_string()]);
    shard.assert_sql("UPDATE game_quest_objective SET required_count = 0 WHERE id = 5090000");
    shard.assert_call("debug_spawn_at_feet", &["1", "51003", "2"]);
    let giver =
        &shard.query_rows("SELECT guid FROM game_world_entity WHERE entry = 51003")[0]["guid"];
    shard.assert_call(
        "debug_turn_in_quest",
        &["1", giver, &QUEST.to_string(), "0"],
    );
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT rewarded FROM game_character_quest WHERE quest_entry = {QUEST}"
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
            select(
                &shard,
                &npc,
                OPTION,
                if flags == 0 { expected } else { !expected },
            );
        }
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn changed_service_actions_and_new_authored_services_refuse_stale_clicks() {
    let always = ConditionKind::Always.code();
    const MENU: u32 = 509_9001;
    let (shard, npc) = fixture("gossip-service-identity");
    shard.assert_sql(&format!("UPDATE game_gossip_condition SET kind = {always}, value1 = 0, value2 = 0 WHERE condition_id = {CONDITION}"));
    shard.assert_sql(&format!(
        "INSERT INTO game_gossip_menu_profile (menu_id,text_id) VALUES ({MENU},1)"
    ));
    shard.assert_sql(&format!("INSERT INTO game_gossip_menu_profile_option (row_id,menu_id,option_index,icon,text,action,action_menu_id,cond_type,cond_value1,cond_value2) VALUES ({OPTION},{MENU},0,0,'Profile supplies',3,0,0,0,0)"));
    let entity = &shard.query_rows(&format!(
        "SELECT map_id,instance_id FROM game_world_entity WHERE guid = {npc}"
    ))[0];

    for (table, source) in [
        ("game_gossip_option", 0),
        ("game_gossip_menu_profile_option", MENU),
    ] {
        if source != 0 {
            shard.assert_sql(&format!("INSERT INTO game_creature_gossip_menu_override (creature_guid,menu_id,map_id,instance_id) VALUES ({npc},{MENU},{},{})", entity["map_id"], entity["instance_id"]));
        }
        select_from(&shard, &npc, OPTION, source, gossip_option::VENDOR, true);
        shard.assert_sql(&format!(
            "UPDATE {table} SET action = 1 WHERE row_id = {OPTION}"
        ));
        select_from(&shard, &npc, OPTION, source, gossip_option::VENDOR, false);
        select_from(&shard, &npc, OPTION, source, gossip_option::GOSSIP, true);
        select_from(&shard, &npc, u32::MAX, source, gossip_option::VENDOR, true);
        shard.assert_sql(&format!(
            "UPDATE {table} SET action = 3, cond_type = 3 WHERE row_id = {OPTION}"
        ));
        select_from(&shard, &npc, u32::MAX, source, gossip_option::VENDOR, false);
        select_from(&shard, &npc, OPTION, source, gossip_option::VENDOR, false);
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn synthesized_services_require_current_capability_and_resolvable_stock() {
    let (shard, npc) = fixture("gossip-service-capability");
    shard.assert_sql(&format!(
        "DELETE FROM game_gossip_option WHERE row_id = {OPTION}"
    ));
    select(&shard, &npc, u32::MAX, true);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET npc_flags = {} WHERE guid = {npc}",
        npc_flags::GOSSIP
    ));
    select(&shard, &npc, u32::MAX, false);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET npc_flags = {} WHERE guid = {npc}",
        npc_flags::GOSSIP | npc_flags::VENDOR
    ));
    select(&shard, &npc, u32::MAX, true);
    shard.assert_sql(&format!(
        "DELETE FROM game_npc_vendor WHERE creature_entry = {VENDOR}"
    ));
    select(&shard, &npc, u32::MAX, false);
    shard.assert_sql(&format!("INSERT INTO game_npc_vendor (id,creature_entry,item_entry,slot,max_count) VALUES (5099001,{VENDOR},5099999,0,0)"));
    select(&shard, &npc, u32::MAX, false);
    shard.assert_sql("UPDATE game_npc_vendor SET item_entry = 5090050 WHERE id = 5099001");
    select(&shard, &npc, u32::MAX, true);

    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET npc_flags = {} WHERE guid = {npc}",
        npc_flags::GOSSIP | npc_flags::INNKEEPER
    ));
    select_from(&shard, &npc, u32::MAX, 0, gossip_option::INNKEEPER, true);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET npc_flags = {} WHERE guid = {npc}",
        npc_flags::GOSSIP
    ));
    select_from(&shard, &npc, u32::MAX, 0, gossip_option::INNKEEPER, false);
    select_from(&shard, &npc, u32::MAX, 0, gossip_option::GOSSIP, false);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn authored_services_require_current_capability_before_the_hook() {
    const MENU: u32 = 509_9001;
    let (shard, npc) = fixture("gossip-authored-capability");
    shard.assert_sql("UPDATE game_world_entity SET level = 20 WHERE guid = 1");
    shard.assert_sql(&format!(
        "UPDATE game_gossip_option SET cond_type = 0 WHERE row_id = {OPTION}"
    ));
    shard.assert_sql(&format!(
        "INSERT INTO game_gossip_menu_profile (menu_id,text_id) VALUES ({MENU},1)"
    ));
    shard.assert_sql(&format!("INSERT INTO game_gossip_menu_profile_option (row_id,menu_id,option_index,icon,text,action,action_menu_id,cond_type,cond_value1,cond_value2) VALUES ({OPTION},{MENU},0,0,'Profile supplies',3,0,0,0,0)"));
    let entity = &shard.query_rows(&format!(
        "SELECT map_id,instance_id FROM game_world_entity WHERE guid = {npc}"
    ))[0];

    for (table, source) in [
        ("game_gossip_option", 0),
        ("game_gossip_menu_profile_option", MENU),
    ] {
        if source != 0 {
            shard.assert_sql(&format!("INSERT INTO game_creature_gossip_menu_override (creature_guid,menu_id,map_id,instance_id) VALUES ({npc},{MENU},{},{})", entity["map_id"], entity["instance_id"]));
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
            select_from(&shard, &npc, OPTION, source, action, true);
            shard.assert_sql(&format!(
                "UPDATE game_world_entity SET npc_flags = {} WHERE guid = {npc}",
                npc_flags::GOSSIP
            ));
            select_from(&shard, &npc, OPTION, source, action, false);
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
        "DELETE FROM game_npc_vendor WHERE creature_entry = {VENDOR}"
    ));
    select_from(&shard, &npc, OPTION, MENU, gossip_option::VENDOR, false);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
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
        "UPDATE game_creature_template SET trainer_type = 0, trainer_class = 1 WHERE entry = {VENDOR}"
    ));
    select_from(&shard, &npc, OPTION, 0, gossip_option::TRAINER, true);
    shard.assert_sql(&format!(
        "UPDATE game_creature_template SET trainer_class = 8 WHERE entry = {VENDOR}"
    ));
    select_from(&shard, &npc, OPTION, 0, gossip_option::TRAINER, false);
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
        false,
    );
    shard.assert_sql(&format!(
        "UPDATE game_creature_template SET trainer_class = 1 WHERE entry = {VENDOR}"
    ));
    select_from(&shard, &npc, OPTION, 0, gossip_option::UNLEARNTALENTS, true);
    shard.assert_sql("UPDATE game_world_entity SET level = 9 WHERE guid = 1");
    select_from(
        &shard,
        &npc,
        OPTION,
        0,
        gossip_option::UNLEARNTALENTS,
        false,
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_missing_quest_subject_refuses_selection_even_with_retained_progress() {
    let (shard, npc) = fixture("gossip-missing-quest");
    shard.assert_call("debug_grant_quest", &["1", &QUEST.to_string()]);
    select(&shard, &npc, OPTION, true);
    shard.assert_sql(&format!(
        "DELETE FROM game_quest_template WHERE entry = {QUEST}"
    ));
    for kind in [
        ConditionKind::QuestRewarded,
        ConditionKind::QuestTaken,
        ConditionKind::QuestAbsent,
    ] {
        let kind = kind.code();
        for flags in [0, 1] {
            shard.assert_sql(&format!("UPDATE game_gossip_condition SET kind = {kind}, value2 = 0, flags = {flags} WHERE condition_id = {CONDITION}"));
            select(&shard, &npc, OPTION, false);
        }
    }
    shard.assert_sql(&format!(
        "DELETE FROM game_character_quest WHERE quest_entry = {QUEST}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_gossip_condition SET flags = 0 WHERE condition_id = {CONDITION}"
    ));
    select(&shard, &npc, OPTION, false);
}
