// Behavior checks for quest_reward and quest_completion against vmangos d9173eca,
// src/game/Handlers/QuestHandler.cpp and Player::RewardQuest.
use lyracore_package_delta::fixture_ids::{
    FIXTURE_BLADE, FIXTURE_JERKY, FIXTURE_REINS, SCENARIO_QUESTGIVER_ENTRY,
};
use lyracore_shared::constants::npc_flags;
use lyracore_test_support::{ReducerConnection, Standalone};
use serde_json::json;

use super::buy_in_slot::{grant, inventory, request};
use super::{enter, fixture};

const QUEST: u32 = 5_091_200;
const OBJECTIVE_QUEST: u32 = 5_091_201;
const UNRELATED_QUEST: u32 = 5_091_202;
const WORN_ITEM_QUEST: u32 = 5_091_203;
const ABANDON_QUEST: u32 = 5_091_204;
const WORN_ITEM_STAMINA: u32 = 20;
const REQUIRED_COUNT: u32 = 5;
const CHOICE_COUNT: u32 = 2;

const CMSG_QUESTGIVER_HELLO: u32 = 0x184;
const CMSG_QUESTGIVER_COMPLETE_QUEST: u32 = 0x18A;
const CMSG_QUESTGIVER_REQUEST_REWARD: u32 = 0x18C;
const CMSG_QUESTGIVER_CHOOSE_REWARD: u32 = 0x18E;
const CMSG_QUESTLOG_REMOVE_QUEST: u32 = 0x194;
const SMSG_QUESTGIVER_REQUEST_ITEMS: u16 = 0x18B;
const SMSG_QUESTGIVER_OFFER_REWARD: u16 = 0x18D;
const SMSG_ITEM_PUSH_RESULT: u16 = 0x166;
const SMSG_LOG_XPGAIN: u16 = 0x1D0;
const SMSG_QUESTGIVER_QUEST_COMPLETE: u16 = 0x191;
const SMSG_QUESTGIVER_QUEST_FAILED: u16 = 0x192;
const SMSG_SET_FACTION_STANDING: u16 = 0x124;
const SMSG_UPDATE_OBJECT: u16 = 0xA9;
const PLAYER_XP: u16 = 716;
const PLAYER_NEXT_LEVEL_XP: u16 = 717;
const FACTION: u32 = 72;
const INVENTORY_FULL_REASON: u32 = 4;
const DUPLICATE_ITEM_REASON: u32 = 17;
const QUEST_COST: u32 = 500;
const QUEST_XP: u32 = 110;
const BACKPACK_SLOTS: u32 = 16;

fn stage(name: &str) -> (Standalone, ReducerConnection, u64, u64) {
    let (shard, guid, _) = fixture(name);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    // The seeded chicken and wolf wander into the replies unless the creature cycle stops.
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    shard.assert_call(
        "debug_spawn_at_feet",
        &[
            &guid.to_string(),
            &SCENARIO_QUESTGIVER_ENTRY.to_string(),
            "2.0",
        ],
    );
    let giver: u64 = shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {SCENARIO_QUESTGIVER_ENTRY}"
        ))
        .iter()
        .map(|row| row["guid"].parse().unwrap())
        .max()
        .unwrap();
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET npc_flags = {}, faction_template = 35 WHERE guid = {giver}",
        npc_flags::QUESTGIVER
    ));
    seed_quest(&shard, QUEST, 0, true);
    seed_quest(&shard, OBJECTIVE_QUEST, FIXTURE_REINS, true);
    seed_quest(&shard, UNRELATED_QUEST, 0, false);
    let mut connection = enter(&shard, guid);
    shard.assert_sql(&format!(
        "DELETE FROM game_item_instance WHERE owner_guid = {guid}"
    ));
    connection.drain_packets();
    (shard, connection, guid, giver)
}

fn seed_quest(shard: &Standalone, quest: u32, required_item: u32, giver_ends_it: bool) {
    let id = u64::from(quest);
    shard.insert_row(
        "game_quest_template",
        &[
            ("entry", &quest),
            ("quest_level", &2),
            ("title", &"Turn-in quest"),
            ("reward_xp", &QUEST_XP),
            ("zone_or_sort", &12),
            ("repeatable", &false),
        ],
    );
    shard.insert_row(
        "game_quest_text",
        &[
            ("quest_entry", &quest),
            ("details", &"Details."),
            ("objectives", &"Objectives."),
            ("offer_reward_text", &"Reward."),
            ("request_items_text", &"Request."),
        ],
    );
    shard.insert_row(
        "game_quest_flags",
        &[("quest_entry", &quest), ("flags", &8)],
    );
    shard.insert_row(
        "game_quest_emote",
        &[
            ("quest_entry", &quest),
            ("incomplete_emote", &6),
            ("complete_emote", &1),
            ("offer_reward_emote_1", &2),
            ("offer_reward_emote_2", &0),
            ("offer_reward_emote_3", &0),
            ("offer_reward_emote_4", &0),
            ("offer_reward_delay_1", &0),
            ("offer_reward_delay_2", &0),
            ("offer_reward_delay_3", &0),
            ("offer_reward_delay_4", &0),
        ],
    );
    shard.insert_rows(
        "game_quest_reward_choice",
        &[
            &[
                ("id", &(id * 2)),
                ("quest_entry", &quest),
                ("choice_index", &0),
                ("item_entry", &FIXTURE_BLADE),
                ("count", &1),
            ],
            &[
                ("id", &(id * 2 + 1)),
                ("quest_entry", &quest),
                ("choice_index", &1),
                ("item_entry", &FIXTURE_REINS),
                ("count", &CHOICE_COUNT),
            ],
        ],
    );
    if giver_ends_it {
        shard.insert_row(
            "game_creature_quest",
            &[
                ("id", &id),
                ("creature_entry", &SCENARIO_QUESTGIVER_ENTRY),
                ("quest_entry", &quest),
                ("role", &1),
            ],
        );
    }
    if required_item != 0 {
        shard.insert_row(
            "game_quest_objective",
            &[
                ("id", &id),
                ("quest_entry", &quest),
                ("obj_index", &0),
                ("kind", &1),
                ("target_entry", &required_item),
                ("required_count", &REQUIRED_COUNT),
            ],
        );
    }
}

fn grant_quest(shard: &Standalone, guid: u64, quest: u32) {
    shard.assert_call(
        "debug_grant_quest",
        &[&guid.to_string(), &quest.to_string()],
    );
}

fn quest_request(
    connection: &mut ReducerConnection,
    opcode: u32,
    giver: u64,
    quest: u32,
    reward: Option<u32>,
) -> Vec<(u16, Vec<u8>)> {
    let mut body = giver.to_le_bytes().to_vec();
    body.extend(quest.to_le_bytes());
    body.extend(reward.map(u32::to_le_bytes).into_iter().flatten());
    request(connection, opcode, body)
}

fn hello(connection: &mut ReducerConnection, giver: u64) -> Vec<(u16, Vec<u8>)> {
    request(
        connection,
        CMSG_QUESTGIVER_HELLO,
        giver.to_le_bytes().to_vec(),
    )
}

fn only_ends(shard: &Standalone, quest: u32) {
    shard.assert_sql(&format!(
        "DELETE FROM game_creature_quest WHERE quest_entry != {quest}"
    ));
}

fn completable(request_tail: &[u8]) -> u32 {
    let items = word(request_tail, 16) as usize;
    word(request_tail, 24 + 12 * items)
}

fn pushed_items(replies: &[(u16, Vec<u8>)]) -> Vec<u32> {
    let mut items: Vec<u32> = replies
        .iter()
        .filter(|(opcode, _)| *opcode == SMSG_ITEM_PUSH_RESULT)
        .map(|(_, body)| word(body, 25))
        .collect();
    items.dedup();
    items
}

fn opcodes(packets: &[(u16, Vec<u8>)]) -> Vec<u16> {
    packets.iter().map(|(opcode, _)| *opcode).collect()
}

fn word(body: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(body[offset..offset + 4].try_into().unwrap())
}

// Request-items body after the guid, quest id and the title and text strings.
fn after_request_strings(body: &[u8]) -> &[u8] {
    let mut rest = &body[12..];
    for _ in 0..2 {
        let end = rest.iter().position(|byte| *byte == 0).unwrap();
        rest = &rest[end + 1..];
    }
    rest
}

fn entries(shard: &Standalone, guid: u64) -> Vec<(String, String)> {
    inventory(shard, guid)
        .iter()
        .map(|row| (row["entry"].clone(), row["stack_count"].clone()))
        .collect()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn the_second_choice_is_the_only_choice_reward_and_its_push_hides_from_chat() {
    let (shard, mut connection, guid, giver) = stage("quest-turn-in-second-choice");
    grant_quest(&shard, guid, QUEST);

    let replies = quest_request(
        &mut connection,
        CMSG_QUESTGIVER_CHOOSE_REWARD,
        giver,
        QUEST,
        Some(1),
    );

    let granted = entries(&shard, guid);
    assert!(granted
        .iter()
        .all(|(entry, _)| *entry == FIXTURE_REINS.to_string()));
    assert_eq!(
        granted
            .iter()
            .map(|(_, count)| count.parse::<u32>().unwrap())
            .sum::<u32>(),
        CHOICE_COUNT
    );
    let push = &replies
        .iter()
        .find(|(opcode, _)| *opcode == SMSG_ITEM_PUSH_RESULT)
        .expect("the reward item push")
        .1;
    assert_eq!(word(push, 8), 1, "received from an NPC");
    assert_eq!(word(push, 16), 0, "no chat line for a quest reward");
    assert_eq!(word(push, 25), FIXTURE_REINS);
    let mut announced = opcodes(&replies);
    announced.retain(|opcode| {
        [
            SMSG_ITEM_PUSH_RESULT,
            SMSG_LOG_XPGAIN,
            SMSG_QUESTGIVER_QUEST_COMPLETE,
        ]
        .contains(opcode)
    });
    announced.dedup();
    assert_eq!(
        announced,
        [
            SMSG_ITEM_PUSH_RESULT,
            SMSG_LOG_XPGAIN,
            SMSG_QUESTGIVER_QUEST_COMPLETE
        ],
        "items, XP gain, then the completion popup"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_refused_turn_in_re_offers_the_reward_only_to_a_giver_that_ends_the_quest() {
    let (shard, mut connection, guid, giver) = stage("quest-turn-in-refused");
    grant_quest(&shard, guid, QUEST);
    grant_quest(&shard, guid, UNRELATED_QUEST);

    let beyond_the_choices = quest_request(
        &mut connection,
        CMSG_QUESTGIVER_CHOOSE_REWARD,
        giver,
        QUEST,
        Some(6),
    );
    assert!(beyond_the_choices.is_empty());
    let rewarded = shard.query_rows(&format!(
        "SELECT rewarded FROM game_character_quest WHERE quest_entry = {QUEST}"
    ));
    assert_eq!(rewarded[0]["rewarded"], "false");

    let not_ended_here = quest_request(
        &mut connection,
        CMSG_QUESTGIVER_CHOOSE_REWARD,
        giver,
        UNRELATED_QUEST,
        Some(0),
    );
    assert!(not_ended_here.is_empty());

    quest_request(
        &mut connection,
        CMSG_QUESTGIVER_CHOOSE_REWARD,
        giver,
        QUEST,
        Some(0),
    );
    let repeated = quest_request(
        &mut connection,
        CMSG_QUESTGIVER_CHOOSE_REWARD,
        giver,
        QUEST,
        Some(0),
    );
    assert_eq!(opcodes(&repeated), [SMSG_QUESTGIVER_OFFER_REWARD]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_quest_with_items_shows_the_request_window_until_the_items_are_handed_in() {
    let (shard, mut connection, guid, giver) = stage("quest-turn-in-request-items");
    grant_quest(&shard, guid, OBJECTIVE_QUEST);

    let incomplete = quest_request(
        &mut connection,
        CMSG_QUESTGIVER_COMPLETE_QUEST,
        giver,
        OBJECTIVE_QUEST,
        None,
    );
    assert_eq!(opcodes(&incomplete), [SMSG_QUESTGIVER_REQUEST_ITEMS]);
    let tail = after_request_strings(&incomplete[0].1);
    assert_eq!(word(tail, 4), 6, "incomplete emote");
    assert_eq!(word(tail, 16), 1, "one required item");
    assert_eq!(word(tail, 20), FIXTURE_REINS);
    assert_eq!(word(tail, 24), REQUIRED_COUNT);
    assert_eq!(word(tail, 32 + 4), 0, "not completable");
    assert!(quest_request(
        &mut connection,
        CMSG_QUESTGIVER_REQUEST_REWARD,
        giver,
        OBJECTIVE_QUEST,
        None
    )
    .is_empty());

    shard.assert_call(
        "debug_grant_item",
        &[
            &guid.to_string(),
            &FIXTURE_REINS.to_string(),
            &REQUIRED_COUNT.to_string(),
        ],
    );
    let complete = quest_request(
        &mut connection,
        CMSG_QUESTGIVER_COMPLETE_QUEST,
        giver,
        OBJECTIVE_QUEST,
        None,
    );
    assert_eq!(opcodes(&complete), [SMSG_QUESTGIVER_REQUEST_ITEMS]);
    let tail = after_request_strings(&complete[0].1);
    assert_eq!(word(tail, 4), 1, "complete emote");
    assert_eq!(word(tail, 32 + 4), 3, "completable");

    let offer = quest_request(
        &mut connection,
        CMSG_QUESTGIVER_REQUEST_REWARD,
        giver,
        OBJECTIVE_QUEST,
        None,
    );
    assert_eq!(opcodes(&offer), [SMSG_QUESTGIVER_OFFER_REWARD]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn hello_at_a_giver_of_one_item_quest_shows_the_request_window_that_closes_on_cancel() {
    let (shard, mut connection, guid, giver) = stage("quest-hello-request-items");
    only_ends(&shard, OBJECTIVE_QUEST);
    grant_quest(&shard, guid, OBJECTIVE_QUEST);

    let incomplete = hello(&mut connection, giver);
    assert_eq!(opcodes(&incomplete), [SMSG_QUESTGIVER_REQUEST_ITEMS]);
    let tail = after_request_strings(&incomplete[0].1);
    assert_eq!(word(tail, 8), 1, "close on cancel");
    assert_eq!(completable(tail), 0);

    shard.assert_call(
        "debug_grant_item",
        &[
            &guid.to_string(),
            &FIXTURE_REINS.to_string(),
            &REQUIRED_COUNT.to_string(),
        ],
    );
    let complete = hello(&mut connection, giver);
    assert_eq!(opcodes(&complete), [SMSG_QUESTGIVER_REQUEST_ITEMS]);
    let tail = after_request_strings(&complete[0].1);
    assert_eq!(word(tail, 8), 1, "close on cancel");
    assert_eq!(completable(tail), 3);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn hello_offers_the_reward_of_an_unfinished_quest_that_has_no_request_text() {
    let (shard, mut connection, guid, giver) = stage("quest-hello-no-request-text");
    only_ends(&shard, OBJECTIVE_QUEST);
    shard.assert_sql(&format!(
        "UPDATE game_quest_text SET request_items_text = '' WHERE quest_entry = {OBJECTIVE_QUEST}"
    ));
    grant_quest(&shard, guid, OBJECTIVE_QUEST);

    assert_eq!(
        opcodes(&hello(&mut connection, giver)),
        [SMSG_QUESTGIVER_OFFER_REWARD]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn complete_quest_offers_the_reward_of_an_unfinished_quest_that_has_no_request_text() {
    let (shard, mut connection, guid, giver) = stage("quest-complete-no-request-text");
    shard.assert_sql(&format!(
        "UPDATE game_quest_text SET request_items_text = '' WHERE quest_entry = {OBJECTIVE_QUEST}"
    ));
    grant_quest(&shard, guid, OBJECTIVE_QUEST);

    let replies = quest_request(
        &mut connection,
        CMSG_QUESTGIVER_COMPLETE_QUEST,
        giver,
        OBJECTIVE_QUEST,
        None,
    );
    assert_eq!(opcodes(&replies), [SMSG_QUESTGIVER_OFFER_REWARD]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn complete_quest_offers_the_reward_of_a_finished_quest_that_needs_no_items() {
    let (shard, mut connection, guid, giver) = stage("quest-complete-no-items");
    grant_quest(&shard, guid, QUEST);

    let replies = quest_request(
        &mut connection,
        CMSG_QUESTGIVER_COMPLETE_QUEST,
        giver,
        QUEST,
        None,
    );
    assert_eq!(opcodes(&replies), [SMSG_QUESTGIVER_OFFER_REWARD]);
}

fn assert_refused_with_notice(replies: &[(u16, Vec<u8>)], reason: u32) {
    assert_eq!(
        opcodes(replies),
        [SMSG_QUESTGIVER_QUEST_FAILED, SMSG_QUESTGIVER_OFFER_REWARD],
        "the notice comes before the re-offer"
    );
    assert_eq!(word(&replies[0].1, 0), QUEST);
    assert_eq!(word(&replies[0].1, 4), reason);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_full_backpack_refuses_the_turn_in_with_an_inventory_full_notice() {
    let (shard, mut connection, guid, giver) = stage("quest-turn-in-full-bags");
    grant_quest(&shard, guid, QUEST);
    for _ in 0..BACKPACK_SLOTS {
        shard.assert_call(
            "debug_grant_item",
            &[&guid.to_string(), &FIXTURE_BLADE.to_string(), "1"],
        );
    }
    let before = entries(&shard, guid);

    let replies = quest_request(
        &mut connection,
        CMSG_QUESTGIVER_CHOOSE_REWARD,
        giver,
        QUEST,
        Some(0),
    );

    assert_refused_with_notice(&replies, INVENTORY_FULL_REASON);
    assert_eq!(entries(&shard, guid), before);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_unique_reward_the_character_cannot_hold_refuses_the_turn_in_as_a_duplicate_item() {
    let (shard, mut connection, guid, giver) = stage("quest-turn-in-duplicate");
    grant_quest(&shard, guid, QUEST);
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET max_count = 1 WHERE entry = {FIXTURE_REINS}"
    ));

    let replies = quest_request(
        &mut connection,
        CMSG_QUESTGIVER_CHOOSE_REWARD,
        giver,
        QUEST,
        Some(1),
    );

    assert_refused_with_notice(&replies, DUPLICATE_ITEM_REASON);
    assert!(entries(&shard, guid).is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn reputation_is_announced_before_the_xp_gain() {
    let (shard, mut connection, guid, giver) = stage("quest-turn-in-reputation");
    shard.assert_sql(&format!(
        "DELETE FROM game_faction WHERE faction_id = {FACTION}"
    ));
    shard.insert_row(
        "game_faction",
        &[("faction_id", &FACTION), ("reputation_index", &19)],
    );
    shard.assert_sql(&format!(
        "UPDATE game_quest_template SET rew_rep_faction_1 = {FACTION}, rew_rep_value_1 = 25 WHERE entry = {QUEST}"
    ));
    grant_quest(&shard, guid, QUEST);

    let replies = quest_request(
        &mut connection,
        CMSG_QUESTGIVER_CHOOSE_REWARD,
        giver,
        QUEST,
        Some(0),
    );

    let mut announced = opcodes(&replies);
    announced.retain(|opcode| [SMSG_SET_FACTION_STANDING, SMSG_LOG_XPGAIN].contains(opcode));
    assert_eq!(announced, [SMSG_SET_FACTION_STANDING, SMSG_LOG_XPGAIN]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn the_chosen_item_is_announced_before_the_fixed_reward_items() {
    let (shard, mut connection, guid, giver) = stage("quest-turn-in-reward-order");
    shard.insert_row(
        "game_quest_reward_item",
        &[
            ("quest_entry", &QUEST),
            ("item_entry", &FIXTURE_JERKY),
            ("count", &1),
        ],
    );
    grant_quest(&shard, guid, QUEST);

    let replies = quest_request(
        &mut connection,
        CMSG_QUESTGIVER_CHOOSE_REWARD,
        giver,
        QUEST,
        Some(1),
    );

    assert_eq!(pushed_items(&replies), [FIXTURE_REINS, FIXTURE_JERKY]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_level_up_turn_in_sends_the_next_level_threshold_with_the_xp() {
    let (shard, mut connection, guid, giver) = stage("quest-turn-in-level-up");
    shard.assert_sql(&format!(
        "UPDATE game_quest_template SET reward_xp = 1000 WHERE entry = {QUEST}"
    ));
    grant_quest(&shard, guid, QUEST);

    let replies = quest_request(
        &mut connection,
        CMSG_QUESTGIVER_CHOOSE_REWARD,
        giver,
        QUEST,
        Some(0),
    );

    let fields: std::collections::BTreeMap<u16, u32> = replies
        .iter()
        .filter(|(opcode, _)| *opcode == SMSG_UPDATE_OBJECT)
        .flat_map(|(opcode, body)| super::properties::updates(*opcode, body).unwrap())
        .filter(|update| update.guid == guid)
        .flat_map(|update| update.fields)
        .collect();
    assert!(fields.contains_key(&PLAYER_XP), "{fields:?}");
    assert!(fields.contains_key(&PLAYER_NEXT_LEVEL_XP), "{fields:?}");
}

fn money(shard: &Standalone, guid: u64) -> u32 {
    shard.query_rows(&format!(
        "SELECT money FROM game_world_entity WHERE guid = {guid}"
    ))[0]["money"]
        .parse()
        .unwrap()
}

fn xp(shard: &Standalone, guid: u64) -> u32 {
    shard.query_rows(&format!(
        "SELECT xp FROM game_world_entity WHERE guid = {guid}"
    ))[0]["xp"]
        .parse()
        .unwrap()
}

fn rewarded(shard: &Standalone) -> String {
    shard.query_rows(&format!(
        "SELECT rewarded FROM game_character_quest WHERE quest_entry = {QUEST}"
    ))[0]["rewarded"]
        .clone()
}

fn choose_reward(connection: &mut ReducerConnection, giver: u64) -> Vec<(u16, Vec<u8>)> {
    quest_request(
        connection,
        CMSG_QUESTGIVER_CHOOSE_REWARD,
        giver,
        QUEST,
        Some(0),
    )
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_quest_cost_stays_incomplete_without_the_money_and_is_taken_with_it() {
    let (shard, mut connection, guid, giver) = stage("quest-turn-in-cost");
    shard.assert_sql(&format!(
        "UPDATE game_quest_template SET required_money = {QUEST_COST} WHERE entry = {QUEST}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET money = {} WHERE guid = {guid}",
        QUEST_COST - 1
    ));
    grant_quest(&shard, guid, QUEST);
    let xp_before = xp(&shard, guid);

    let window = quest_request(
        &mut connection,
        CMSG_QUESTGIVER_COMPLETE_QUEST,
        giver,
        QUEST,
        None,
    );
    assert_eq!(opcodes(&window), [SMSG_QUESTGIVER_REQUEST_ITEMS]);
    let tail = after_request_strings(&window[0].1);
    assert_eq!(word(tail, 12), QUEST_COST, "the cost is the required money");
    assert_eq!(completable(tail), 0);
    let refused = choose_reward(&mut connection, giver);
    assert_eq!(opcodes(&refused), [SMSG_QUESTGIVER_OFFER_REWARD]);
    assert_eq!(money(&shard, guid), QUEST_COST - 1);
    assert_eq!(xp(&shard, guid), xp_before);
    assert_eq!(rewarded(&shard), "false");

    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET money = {} WHERE guid = {guid}",
        QUEST_COST + 40
    ));
    let offer = quest_request(
        &mut connection,
        CMSG_QUESTGIVER_COMPLETE_QUEST,
        giver,
        QUEST,
        None,
    );
    assert_eq!(opcodes(&offer), [SMSG_QUESTGIVER_OFFER_REWARD]);
    let accepted = choose_reward(&mut connection, giver);

    let complete = accepted
        .iter()
        .find(|(opcode, _)| *opcode == SMSG_QUESTGIVER_QUEST_COMPLETE)
        .expect("the completion popup");
    assert_eq!(word(&complete.1, 12), 0u32.wrapping_sub(QUEST_COST));
    assert_eq!(money(&shard, guid), 40);
    assert_eq!(rewarded(&shard), "true");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_quest_with_explicit_zero_xp_grants_none_and_a_quest_with_xp_grants_it() {
    let (shard, mut connection, guid, giver) = stage("quest-turn-in-zero-xp");
    shard.assert_sql(&format!(
        "UPDATE game_quest_template SET reward_xp = 0 WHERE entry = {QUEST}"
    ));
    grant_quest(&shard, guid, QUEST);
    grant_quest(&shard, guid, OBJECTIVE_QUEST);
    shard.assert_sql(&format!(
        "DELETE FROM game_quest_objective WHERE quest_entry = {OBJECTIVE_QUEST}"
    ));

    let xp_before = xp(&shard, guid);

    let zero = choose_reward(&mut connection, giver);

    assert!(opcodes(&zero).contains(&SMSG_QUESTGIVER_QUEST_COMPLETE));
    assert!(!opcodes(&zero).contains(&SMSG_LOG_XPGAIN));
    assert_eq!(xp(&shard, guid), xp_before);

    let normal = quest_request(
        &mut connection,
        CMSG_QUESTGIVER_CHOOSE_REWARD,
        giver,
        OBJECTIVE_QUEST,
        Some(0),
    );

    assert!(opcodes(&normal).contains(&SMSG_LOG_XPGAIN));
    assert_eq!(xp(&shard, guid), xp_before + QUEST_XP);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_quest_with_five_reputation_rewards_grants_all_five() {
    const FACTIONS: [(u32, i32); 5] = [
        (9101, 250),
        (9102, 100),
        (9103, 75),
        (9104, -50),
        (9105, 25),
    ];
    let (shard, mut connection, guid, giver) = stage("quest-turn-in-five-reputations");
    for (slot, (faction, value)) in FACTIONS.iter().enumerate() {
        shard.insert_row(
            "game_faction",
            &[
                ("faction_id", faction),
                ("reputation_index", &(30 + slot as i32)),
            ],
        );
        shard.assert_sql(&format!(
            "UPDATE game_quest_template SET rew_rep_faction_{n} = {faction}, rew_rep_value_{n} = {value} WHERE entry = {QUEST}",
            n = slot + 1
        ));
    }
    grant_quest(&shard, guid, QUEST);

    choose_reward(&mut connection, giver);

    let mut granted: Vec<(u32, i32)> = shard
        .query_rows(&format!(
            "SELECT faction_id, standing FROM game_player_reputation WHERE character_guid = {guid} AND faction_id > 9100"
        ))
        .iter()
        .map(|row| {
            (
                row["faction_id"].parse().unwrap(),
                row["standing"].parse().unwrap(),
            )
        })
        .collect();
    granted.sort_unstable();
    assert_eq!(granted, FACTIONS);
}

fn completion_popup(replies: &[(u16, Vec<u8>)]) -> &[u8] {
    &replies
        .iter()
        .find(|(opcode, _)| *opcode == SMSG_QUESTGIVER_QUEST_COMPLETE)
        .expect("the completion popup")
        .1
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn quest_xp_falls_with_the_level_above_the_quest_and_the_popup_shows_it() {
    let (shard, mut connection, guid, giver) = stage("quest-turn-in-xp-scaling");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET level = 8 WHERE guid = {guid}"
    ));
    grant_quest(&shard, guid, QUEST);
    let xp_before = xp(&shard, guid);

    let replies = choose_reward(&mut connection, giver);

    assert_eq!(
        xp(&shard, guid),
        xp_before + 88,
        "quest level 2, Character level 8: 80%"
    );
    assert_eq!(word(completion_popup(&replies), 8), 88);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_level_60_turn_in_pays_the_max_level_money_instead_of_xp() {
    let (shard, mut connection, guid, giver) = stage("quest-turn-in-max-level");
    shard.assert_sql(&format!(
        "UPDATE game_quest_template SET reward_money = 30, reward_money_max_level = 870 WHERE entry = {QUEST}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET level = 60, money = 5 WHERE guid = {guid}"
    ));
    grant_quest(&shard, guid, QUEST);

    let replies = choose_reward(&mut connection, giver);

    assert_eq!(money(&shard, guid), 5 + 30 + 870);
    assert!(!opcodes(&replies).contains(&SMSG_LOG_XPGAIN));
    let popup = completion_popup(&replies);
    assert_eq!(word(popup, 8), 0, "no XP at the cap");
    assert_eq!(word(popup, 12), 30 + 870);
}

fn max_health(shard: &Standalone, guid: u64) -> u32 {
    shard.query_rows(&format!(
        "SELECT max_health FROM game_world_entity WHERE guid = {guid}"
    ))[0]["max_health"]
        .parse()
        .unwrap()
}

fn blade_slots(shard: &Standalone, guid: u64) -> Vec<String> {
    item_slots(shard, guid, FIXTURE_BLADE)
}

fn stage_worn_item_quest(name: &str) -> (Standalone, ReducerConnection, u64, u64, u32) {
    let (shard, connection, guid, giver) = stage(name);
    seed_quest(&shard, WORN_ITEM_QUEST, FIXTURE_BLADE, true);
    shard.assert_sql(&format!(
        "UPDATE game_quest_objective SET required_count = 1 WHERE quest_entry = {WORN_ITEM_QUEST}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET stat_stamina = {WORN_ITEM_STAMINA} WHERE entry = {FIXTURE_BLADE}"
    ));
    let base_health = max_health(&shard, guid);
    (shard, connection, guid, giver, base_health)
}

fn turn_in_for_reins(connection: &mut ReducerConnection, giver: u64) {
    quest_request(
        connection,
        CMSG_QUESTGIVER_CHOOSE_REWARD,
        giver,
        WORN_ITEM_QUEST,
        Some(1),
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_collect_turn_in_hands_in_the_carried_copy_and_keeps_the_worn_one() {
    let (shard, mut connection, guid, giver, base_health) =
        stage_worn_item_quest("quest-worn-copy");
    grant(&shard, guid, FIXTURE_BLADE, 1, 23);
    connection
        .call("debug_equip_item", json!([guid, 23]))
        .unwrap();
    grant(&shard, guid, FIXTURE_BLADE, 1, 23);
    let worn_health = max_health(&shard, guid);
    assert!(worn_health > base_health, "the worn blade adds stamina");
    grant_quest(&shard, guid, WORN_ITEM_QUEST);

    turn_in_for_reins(&mut connection, giver);

    assert_eq!(blade_slots(&shard, guid), ["15"]);
    assert_eq!(max_health(&shard, guid), worn_health);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn handing_in_the_only_copy_while_it_is_worn_removes_its_stats() {
    let (shard, mut connection, guid, giver, base_health) =
        stage_worn_item_quest("quest-worn-only-copy");
    grant(&shard, guid, FIXTURE_BLADE, 1, 23);
    connection
        .call("debug_equip_item", json!([guid, 23]))
        .unwrap();
    assert!(
        max_health(&shard, guid) > base_health,
        "the worn blade adds stamina"
    );
    grant_quest(&shard, guid, WORN_ITEM_QUEST);

    turn_in_for_reins(&mut connection, giver);

    assert!(blade_slots(&shard, guid).is_empty());
    assert_eq!(max_health(&shard, guid), base_health);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn abandoning_a_quest_whose_source_item_is_worn_removes_the_item_and_its_stats() {
    let (shard, mut connection, guid, _giver, base_health) =
        stage_worn_item_quest("quest-abandon-worn-source");
    grant(&shard, guid, FIXTURE_BLADE, 1, 23);
    connection
        .call("debug_equip_item", json!([guid, 23]))
        .unwrap();
    assert!(
        max_health(&shard, guid) > base_health,
        "the worn blade adds stamina"
    );
    grant_quest(&shard, guid, WORN_ITEM_QUEST);
    shard.assert_sql(&format!(
        "UPDATE game_quest_template SET src_item = {FIXTURE_BLADE}, src_item_count = 1 \
         WHERE entry = {WORN_ITEM_QUEST}"
    ));

    request(&mut connection, CMSG_QUESTLOG_REMOVE_QUEST, vec![0]);

    assert!(blade_slots(&shard, guid).is_empty());
    assert_eq!(max_health(&shard, guid), base_health);
}

fn item_slots(shard: &Standalone, guid: u64, entry: u32) -> Vec<String> {
    let mut slots: Vec<String> = shard
        .query_rows(&format!(
            "SELECT slot FROM game_item_instance WHERE owner_guid = {guid} AND entry = {entry}"
        ))
        .iter()
        .map(|row| row["slot"].clone())
        .collect();
    slots.sort();
    slots
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn abandoning_a_quest_destroys_its_quest_bonded_required_items_in_bags_and_bank() {
    let (shard, mut connection, guid, _giver) = stage("quest-abandon-bonded-items");
    seed_quest(&shard, ABANDON_QUEST, FIXTURE_REINS, true);
    shard.insert_row(
        "game_quest_objective",
        &[
            ("id", &(u64::from(ABANDON_QUEST) + 1_000_000)),
            ("quest_entry", &ABANDON_QUEST),
            ("obj_index", &1),
            ("kind", &1),
            ("target_entry", &FIXTURE_JERKY),
            ("required_count", &1),
        ],
    );
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET bonding = 4, max_stack = 1 WHERE entry = {FIXTURE_REINS}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET bonding = 0, max_stack = 1 WHERE entry = {FIXTURE_JERKY}"
    ));
    grant(&shard, guid, FIXTURE_REINS, 1, 23);
    grant(&shard, guid, FIXTURE_REINS, 1, 24);
    grant(&shard, guid, FIXTURE_REINS, 1, 39);
    grant(&shard, guid, FIXTURE_JERKY, 1, 25);
    grant(&shard, guid, FIXTURE_JERKY, 1, 40);
    grant_quest(&shard, guid, ABANDON_QUEST);

    request(&mut connection, CMSG_QUESTLOG_REMOVE_QUEST, vec![0]);

    assert!(
        item_slots(&shard, guid, FIXTURE_REINS).is_empty(),
        "every quest-bonded copy is destroyed, bank included"
    );
    assert_eq!(item_slots(&shard, guid, FIXTURE_JERKY), ["25", "40"]);
    assert!(shard
        .query_rows(&format!(
            "SELECT * FROM game_character_quest WHERE quest_entry = {ABANDON_QUEST}"
        ))
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn failing_a_repeatable_untimed_quest_destroys_its_quest_bonded_required_items() {
    let (shard, _connection, guid, _giver) = stage("quest-fail-bonded-items");
    seed_quest(&shard, ABANDON_QUEST, FIXTURE_REINS, true);
    shard.assert_sql(&format!(
        "UPDATE game_quest_template SET repeatable = true, limit_time = 0 WHERE entry = {ABANDON_QUEST}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET bonding = 4, max_stack = 1 WHERE entry = {FIXTURE_REINS}"
    ));
    grant(&shard, guid, FIXTURE_REINS, 1, 23);
    grant_quest(&shard, guid, ABANDON_QUEST);

    shard.assert_call(
        "debug_expire_quest",
        &[&guid.to_string(), &ABANDON_QUEST.to_string()],
    );

    assert!(item_slots(&shard, guid, FIXTURE_REINS).is_empty());
    assert!(shard
        .query_rows(&format!(
            "SELECT * FROM game_character_quest WHERE quest_entry = {ABANDON_QUEST}"
        ))
        .is_empty());
}
