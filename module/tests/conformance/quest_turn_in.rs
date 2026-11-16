// Behavior checks for quest_reward and quest_completion against vmangos d9173eca,
// src/game/Handlers/QuestHandler.cpp and Player::RewardQuest.
use lyracore_package_delta::fixture_ids::{
    FIXTURE_BLADE, FIXTURE_REINS, SCENARIO_QUESTGIVER_ENTRY,
};
use lyracore_shared::constants::npc_flags;
use lyracore_test_support::{ReducerConnection, Standalone};

use super::buy_in_slot::{inventory, request};
use super::{enter, fixture};

const QUEST: u32 = 5_091_200;
const OBJECTIVE_QUEST: u32 = 5_091_201;
const UNRELATED_QUEST: u32 = 5_091_202;
const REQUIRED_COUNT: u32 = 5;
const CHOICE_COUNT: u32 = 2;

const CMSG_QUESTGIVER_COMPLETE_QUEST: u32 = 0x18A;
const CMSG_QUESTGIVER_REQUEST_REWARD: u32 = 0x18C;
const CMSG_QUESTGIVER_CHOOSE_REWARD: u32 = 0x18E;
const SMSG_QUESTGIVER_REQUEST_ITEMS: u16 = 0x18B;
const SMSG_QUESTGIVER_OFFER_REWARD: u16 = 0x18D;
const SMSG_ITEM_PUSH_RESULT: u16 = 0x166;
const SMSG_LOG_XPGAIN: u16 = 0x1D0;
const SMSG_QUESTGIVER_QUEST_COMPLETE: u16 = 0x191;

fn stage(name: &str) -> (Standalone, ReducerConnection, u64, u64) {
    let (shard, guid, _) = fixture(name);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
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
            ("reward_xp", &110),
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
    assert_eq!(
        opcodes(&replies),
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
