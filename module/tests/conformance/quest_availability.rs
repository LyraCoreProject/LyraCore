// Quest availability and giver marks against vmangos d9173eca: Player::CanTakeQuest,
// Player::CanSeeStartQuest, WorldSession::GetDialogStatus and GameEventMgr::UpdateEventQuests.
use lyracore_package_delta::fixture_ids::SCENARIO_QUESTGIVER_ENTRY;
use lyracore_shared::constants::npc_flags;
use lyracore_test_support::{ReducerConnection, Standalone};

use super::buy_in_slot::request;
use super::{enter, fixture};

const FIRST: u32 = 5_113_000;
const SECOND: u32 = 5_113_001;
const EXCLUSIVE_GROUP: i32 = 5_113;
const CMSG_QUESTGIVER_STATUS_QUERY: u32 = 0x182;
const CMSG_QUESTGIVER_ACCEPT_QUEST: u32 = 0x189;
const CMSG_QUESTLOG_REMOVE_QUEST: u32 = 0x194;
const CMSG_QUESTGIVER_CHOOSE_REWARD: u32 = 0x18e;
const SMSG_QUESTGIVER_STATUS: u16 = 0x183;
const SMSG_QUESTGIVER_QUEST_INVALID: u16 = 0x18F;
const SMSG_GOSSIP_COMPLETE: u16 = 0x17E;
const DONT_HAVE_REQ: u32 = 0;
const STATUS_NONE: u8 = 0;
const STATUS_UNAVAILABLE: u8 = 1;
const STATUS_CHAT: u8 = 2;
const STATUS_REWARD_REP: u8 = 4;
const STATUS_AVAILABLE: u8 = 5;

struct Stage {
    shard: Standalone,
    connection: ReducerConnection,
    guid: u64,
    giver: u64,
}

fn stage(name: &str) -> Stage {
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
    shard.assert_sql(&format!(
        "DELETE FROM game_creature_quest WHERE creature_entry = {SCENARIO_QUESTGIVER_ENTRY}"
    ));
    let connection = enter(&shard, guid);
    Stage {
        shard,
        connection,
        guid,
        giver,
    }
}

fn offer(stage: &Stage, quest: u32, min_level: u32, quest_level: u32) {
    offer_repeatable(stage, quest, min_level, quest_level, false);
}

fn offer_repeatable(stage: &Stage, quest: u32, min_level: u32, quest_level: u32, repeatable: bool) {
    stage.shard.insert_row(
        "game_quest_template",
        &[
            ("entry", &quest),
            ("min_level", &min_level),
            ("quest_level", &quest_level),
            ("title", &"Availability quest"),
            ("zone_or_sort", &12),
            ("repeatable", &repeatable),
        ],
    );
    stage.shard.insert_row(
        "game_creature_quest",
        &[
            ("id", &u64::from(quest)),
            ("creature_entry", &SCENARIO_QUESTGIVER_ENTRY),
            ("quest_entry", &quest),
            ("role", &0),
        ],
    );
}

fn accept(stage: &mut Stage, quest: u32) -> Vec<(u16, Vec<u8>)> {
    let mut body = stage.giver.to_le_bytes().to_vec();
    body.extend(quest.to_le_bytes());
    request(&mut stage.connection, CMSG_QUESTGIVER_ACCEPT_QUEST, body)
}

fn status(stage: &mut Stage) -> Vec<(u16, Vec<u8>)> {
    request(
        &mut stage.connection,
        CMSG_QUESTGIVER_STATUS_QUERY,
        stage.giver.to_le_bytes().to_vec(),
    )
}

fn status_reply(stage: &Stage, status: u8) -> Vec<(u16, Vec<u8>)> {
    let mut body = stage.giver.to_le_bytes().to_vec();
    body.extend(u32::from(status).to_le_bytes());
    vec![(SMSG_QUESTGIVER_STATUS, body)]
}

fn dont_have_req() -> Vec<(u16, Vec<u8>)> {
    vec![
        (
            SMSG_QUESTGIVER_QUEST_INVALID,
            DONT_HAVE_REQ.to_le_bytes().to_vec(),
        ),
        (SMSG_GOSSIP_COMPLETE, Vec::new()),
    ]
}

fn has_quest(stage: &Stage, quest: u32) -> bool {
    !stage
        .shard
        .query_rows(&format!(
            "SELECT id FROM game_character_quest WHERE character_guid = {} AND quest_entry = {quest}",
            stage.guid
        ))
        .is_empty()
}

fn accepted(stage: &mut Stage, quest: u32) {
    let replies = accept(stage, quest);
    assert!(
        !replies
            .iter()
            .any(|(opcode, _)| *opcode == SMSG_QUESTGIVER_QUEST_INVALID),
        "{replies:?}"
    );
    assert!(has_quest(stage, quest), "{replies:?}");
}

fn refused(stage: &mut Stage, quest: u32, replies: Vec<(u16, Vec<u8>)>) {
    assert_eq!(accept(stage, quest), replies);
    assert!(!has_quest(stage, quest));
}

fn prerequisite(stage: &Stage, quest: u32, prev_quest: u32, must_be_active: bool) {
    stage.shard.insert_row(
        "game_quest_prerequisite",
        &[
            ("id", &u64::from(quest)),
            ("quest_entry", &quest),
            ("prev_quest_entry", &prev_quest),
            ("must_be_active", &must_be_active),
        ],
    );
}

fn set_level(stage: &Stage, level: u32) {
    stage.shard.assert_call(
        "debug_set_level",
        &[&stage.guid.to_string(), &level.to_string()],
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_seeded_prerequisite_row_holds_the_quest_until_its_predecessor_is_rewarded() {
    let mut stage = stage("quest-availability-next-quest-id");
    offer(&stage, FIRST, 1, 1);
    offer(&stage, SECOND, 1, 1);
    prerequisite(&stage, SECOND, FIRST, false);

    refused(&mut stage, SECOND, dont_have_req());
    accepted(&mut stage, FIRST);
    refused(&mut stage, SECOND, dont_have_req());
    stage.shard.assert_sql(&format!(
        "UPDATE game_character_quest SET rewarded = true WHERE quest_entry = {FIRST}"
    ));
    accepted(&mut stage, SECOND);
}

fn turn_in(stage: &Stage, quest: u32) {
    let relation = u64::from(quest) + 1_000_000_000;
    stage.shard.insert_row(
        "game_creature_quest",
        &[
            ("id", &relation),
            ("creature_entry", &SCENARIO_QUESTGIVER_ENTRY),
            ("quest_entry", &quest),
            ("role", &1),
        ],
    );
    stage.shard.assert_call(
        "debug_turn_in_quest",
        &[
            &stage.guid.to_string(),
            &stage.giver.to_string(),
            &quest.to_string(),
            "0",
        ],
    );
    stage.shard.assert_sql(&format!(
        "DELETE FROM game_creature_quest WHERE id = {relation}"
    ));
}

fn retakes(stage: &Stage) -> usize {
    stage
        .shard
        .query_rows(&format!(
            "SELECT id FROM game_character_quest_retake WHERE character_guid = {}",
            stage.guid
        ))
        .len()
}

fn repeatable_unlocks_second(stage: &mut Stage) {
    offer_repeatable(stage, FIRST, 1, 1, true);
    offer(stage, SECOND, 1, 1);
    prerequisite(stage, SECOND, FIRST, false);
    accepted(stage, FIRST);
    turn_in(stage, FIRST);
    accepted(stage, FIRST);
    assert_eq!(retakes(stage), 1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn retaking_a_rewarded_repeatable_quest_keeps_the_quest_it_unlocked() {
    let mut stage = stage("quest-availability-repeatable-retake");
    repeatable_unlocks_second(&mut stage);

    assert_eq!(status(&mut stage), status_reply(&stage, STATUS_AVAILABLE));
    accepted(&mut stage, SECOND);
    turn_in(&stage, FIRST);
    assert_eq!(retakes(&stage), 0, "the turn-in holds the reward record");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn abandoning_a_retaken_repeatable_quest_keeps_its_reward_record() {
    let mut stage = stage("quest-availability-repeatable-abandon");
    repeatable_unlocks_second(&mut stage);

    request(&mut stage.connection, CMSG_QUESTLOG_REMOVE_QUEST, vec![0]);
    let rows = stage.shard.query_rows(&format!(
        "SELECT rewarded FROM game_character_quest WHERE quest_entry = {FIRST}"
    ));
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["rewarded"], "true");
    accepted(&mut stage, SECOND);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_negative_prerequisite_needs_its_quest_in_the_log() {
    let mut stage = stage("quest-availability-active-prerequisite");
    offer(&stage, FIRST, 1, 1);
    offer(&stage, SECOND, 1, 1);
    prerequisite(&stage, SECOND, FIRST, true);

    refused(&mut stage, SECOND, dont_have_req());
    accepted(&mut stage, FIRST);
    accepted(&mut stage, SECOND);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_exclusive_group_lets_a_character_take_one_of_its_quests() {
    let mut stage = stage("quest-availability-exclusive-group");
    for quest in [FIRST, SECOND] {
        offer(&stage, quest, 1, 1);
        stage.shard.insert_row(
            "game_quest_availability",
            &[
                ("quest_entry", &quest),
                ("exclusive_group", &EXCLUSIVE_GROUP),
                ("max_level", &0),
            ],
        );
    }

    accepted(&mut stage, FIRST);
    refused(&mut stage, SECOND, dont_have_req());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_maximum_level_closes_the_quest_one_level_above_it() {
    let mut stage = stage("quest-availability-maximum-level");
    offer(&stage, FIRST, 1, 1);
    stage.shard.insert_row(
        "game_quest_availability",
        &[
            ("quest_entry", &FIRST),
            ("exclusive_group", &0),
            ("max_level", &5),
        ],
    );

    set_level(&stage, 6);
    refused(&mut stage, FIRST, vec![(SMSG_GOSSIP_COMPLETE, Vec::new())]);
    set_level(&stage, 5);
    accepted(&mut stage, FIRST);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_quest_of_the_chain_in_the_log_closes_the_quests_after_it() {
    let mut stage = stage("quest-availability-previous-chain");
    offer(&stage, FIRST, 1, 1);
    offer(&stage, SECOND, 1, 1);
    stage.shard.assert_sql(&format!(
        "UPDATE game_quest_template SET next_quest_id = {SECOND} WHERE entry = {FIRST}"
    ));
    assert_eq!(status(&mut stage), status_reply(&stage, STATUS_AVAILABLE));

    accepted(&mut stage, FIRST);
    assert_eq!(status(&mut stage), status_reply(&stage, STATUS_NONE));
    refused(&mut stage, SECOND, dont_have_req());

    request(&mut stage.connection, CMSG_QUESTLOG_REMOVE_QUEST, vec![0]);
    assert_eq!(status(&mut stage), status_reply(&stage, STATUS_AVAILABLE));
    accepted(&mut stage, SECOND);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_rewarded_repeatable_quest_shows_the_blue_mark() {
    let mut stage = stage("quest-availability-repeatable-mark");
    offer_repeatable(&stage, FIRST, 1, 1, true);
    assert_eq!(status(&mut stage), status_reply(&stage, STATUS_AVAILABLE));

    accepted(&mut stage, FIRST);
    turn_in(&stage, FIRST);
    assert_eq!(status(&mut stage), status_reply(&stage, STATUS_REWARD_REP));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_quest_up_to_seven_levels_ahead_shows_the_grey_mark() {
    let mut stage = stage("quest-availability-upcoming");
    set_level(&stage, 10);
    offer(&stage, FIRST, 18, 20);
    assert_eq!(status(&mut stage), status_reply(&stage, STATUS_NONE));

    offer(&stage, SECOND, 13, 15);
    assert_eq!(status(&mut stage), status_reply(&stage, STATUS_UNAVAILABLE));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_quest_five_levels_below_the_character_shows_no_yellow_mark() {
    let mut stage = stage("quest-availability-trivial");
    set_level(&stage, 10);
    offer(&stage, FIRST, 1, 5);
    assert_eq!(status(&mut stage), status_reply(&stage, STATUS_CHAT));

    offer(&stage, SECOND, 1, 6);
    assert_eq!(status(&mut stage), status_reply(&stage, STATUS_AVAILABLE));
}

const CHARACTER_FACTION: u32 = 5_113_000;
const GIVER_FACTION: u32 = 5_113_001;

fn set_factions(stage: &Stage, giver_enemy_group: u32) {
    for (id, faction_group, enemy_group) in [
        (CHARACTER_FACTION, 1u32, 0u32),
        (GIVER_FACTION, 0, giver_enemy_group),
    ] {
        stage.shard.insert_row(
            "game_faction_template",
            &[
                ("id", &id),
                ("faction", &id),
                ("faction_group", &faction_group),
                ("enemy_group", &enemy_group),
            ],
        );
    }
    for (guid, template) in [
        (stage.guid, CHARACTER_FACTION),
        (stage.giver, GIVER_FACTION),
    ] {
        stage.shard.assert_sql(&format!(
            "UPDATE game_world_entity SET faction_template = {template} WHERE guid = {guid}"
        ));
    }
}

fn grant_reputation(stage: &Stage, amount: i32) {
    stage.shard.assert_call(
        "debug_grant_reputation",
        &[
            &stage.guid.to_string(),
            &GIVER_FACTION.to_string(),
            &amount.to_string(),
        ],
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_hostile_quest_giver_shows_no_mark() {
    let mut stage = stage("quest-availability-hostile");
    offer(&stage, FIRST, 1, 1);
    assert_eq!(status(&mut stage), status_reply(&stage, STATUS_AVAILABLE));

    set_factions(&stage, 1);
    assert_eq!(status(&mut stage), status_reply(&stage, STATUS_NONE));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_reputation_giver_hides_its_mark_only_from_a_hostile_rank() {
    let mut stage = stage("quest-availability-reputation");
    offer(&stage, FIRST, 1, 1);
    set_factions(&stage, 1);
    stage.shard.insert_row(
        "game_faction",
        &[
            ("faction_id", &GIVER_FACTION),
            ("reputation_index", &60),
            ("base_standing", &0),
        ],
    );
    grant_reputation(&stage, -3000);
    stage.shard.assert_sql(&format!(
        "UPDATE game_player_reputation SET at_war = true WHERE character_guid = {}",
        stage.guid
    ));
    assert_eq!(
        status(&mut stage),
        status_reply(&stage, STATUS_AVAILABLE),
        "an Unfriendly rank at war is not hostile"
    );

    grant_reputation(&stage, -1);
    assert_eq!(status(&mut stage), status_reply(&stage, STATUS_NONE));
}

const REQUIRED_FACTION: u32 = 5_113_002;
const FRIENDLY: i32 = 3000;
const HONORED: i32 = 9000;
const COOKING: u32 = 185;

fn require(stage: &Stage, quest: u32, columns: &str) {
    stage.shard.assert_sql(&format!(
        "UPDATE game_quest_template SET {columns} WHERE entry = {quest}"
    ));
}

fn reputation_quest(stage: &Stage, columns: &str, base_standing: i32) {
    offer(stage, FIRST, 1, 1);
    require(stage, FIRST, columns);
    stage.shard.insert_row(
        "game_faction",
        &[
            ("faction_id", &REQUIRED_FACTION),
            ("reputation_index", &61),
            ("base_standing", &base_standing),
        ],
    );
}

fn gain_required_reputation(stage: &Stage, amount: i32) {
    stage.shard.assert_call(
        "debug_grant_reputation",
        &[
            &stage.guid.to_string(),
            &REQUIRED_FACTION.to_string(),
            &amount.to_string(),
        ],
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_minimum_reputation_quest_opens_at_friendly() {
    let mut stage = stage("quest-availability-minimum-reputation");
    reputation_quest(
        &stage,
        &format!(
            "required_min_rep_faction = {REQUIRED_FACTION}, required_min_rep_value = {FRIENDLY}"
        ),
        FRIENDLY - 1,
    );

    assert_eq!(status(&mut stage), status_reply(&stage, STATUS_NONE));
    refused(&mut stage, FIRST, dont_have_req());
    gain_required_reputation(&stage, 1);
    assert_eq!(status(&mut stage), status_reply(&stage, STATUS_AVAILABLE));
    accepted(&mut stage, FIRST);
}

// Cenarion Circle's Faction.dbc slots: a Night Elf or Tauren Druid starts at 2000.
#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_tauren_druid_needs_only_1000_more_for_a_friendly_cenarion_circle_quest() {
    let mut stage = stage("quest-availability-druid-base-reputation");
    offer(&stage, FIRST, 1, 1);
    require(
        &stage,
        FIRST,
        &format!(
            "required_min_rep_faction = {REQUIRED_FACTION}, required_min_rep_value = {FRIENDLY}"
        ),
    );
    stage.shard.insert_row(
        "game_faction",
        &[
            ("faction_id", &REQUIRED_FACTION),
            ("reputation_index", &61),
            ("base_standing", &0),
            ("base_race_mask_1", &255),
            ("base_class_mask_1", &479),
            ("base_standing_2", &2000),
            ("base_race_mask_2", &40),
            ("base_class_mask_2", &1024),
        ],
    );
    stage.shard.assert_sql(&format!(
        "UPDATE game_character SET race = 6, class = 11 WHERE guid = {}",
        stage.guid
    ));

    assert_eq!(status(&mut stage), status_reply(&stage, STATUS_NONE));
    gain_required_reputation(&stage, 999);
    assert_eq!(status(&mut stage), status_reply(&stage, STATUS_NONE));
    gain_required_reputation(&stage, 1);
    assert_eq!(status(&mut stage), status_reply(&stage, STATUS_AVAILABLE));
    accepted(&mut stage, FIRST);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_maximum_reputation_quest_closes_at_its_cap() {
    let mut stage = stage("quest-availability-maximum-reputation");
    reputation_quest(
        &stage,
        &format!(
            "required_max_rep_faction = {REQUIRED_FACTION}, required_max_rep_value = {HONORED}"
        ),
        HONORED,
    );

    assert_eq!(
        status(&mut stage),
        status_reply(&stage, STATUS_NONE),
        "with no stored standing the faction's base standing counts"
    );
    refused(&mut stage, FIRST, dont_have_req());
    gain_required_reputation(&stage, -1);
    assert_eq!(status(&mut stage), status_reply(&stage, STATUS_AVAILABLE));
    accepted(&mut stage, FIRST);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_skill_quest_is_refused_below_its_skill_value() {
    let mut stage = stage("quest-availability-skill");
    offer(&stage, FIRST, 1, 1);
    require(
        &stage,
        FIRST,
        &format!("required_skill = {COOKING}, required_skill_value = 50"),
    );
    stage.shard.assert_call(
        "debug_learn_profession",
        &[&stage.guid.to_string(), &COOKING.to_string(), "75"],
    );

    set_cooking(&stage, 49);
    assert_eq!(status(&mut stage), status_reply(&stage, STATUS_NONE));
    refused(&mut stage, FIRST, dont_have_req());
    set_cooking(&stage, 50);
    accepted(&mut stage, FIRST);
}

fn set_cooking(stage: &Stage, value: u32) {
    stage.shard.assert_sql(&format!(
        "UPDATE game_player_skill SET current = {value} WHERE character_guid = {} AND skill_line = {COOKING}",
        stage.guid
    ));
}

const FESTIVAL: u32 = 5_113_100;

/// Ties `quest` to a scripted Game Event, which stays stopped until an Operator starts it.
fn festival_quest(stage: &Stage, quest: u32) {
    stage.shard.insert_row(
        "game_event",
        &[
            ("event_entry", &FESTIVAL),
            ("start_secs", &0i64),
            ("end_secs", &i64::from(i32::MAX)),
            ("occurrence_mins", &525_600u32),
            ("length_mins", &1_440u32),
            ("scripted", &true),
            ("disabled", &false),
            ("description", &"Fixture festival"),
        ],
    );
    stage.shard.insert_row(
        "game_event_quest",
        &[("quest_entry", &quest), ("event_entry", &FESTIVAL)],
    );
}

fn festival(stage: &Stage, reducer: &str) {
    stage.shard.assert_call(reducer, &[&FESTIVAL.to_string()]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_event_quest_is_offered_only_while_its_event_runs() {
    let mut stage = stage("quest-availability-event");
    offer(&stage, FIRST, 1, 1);
    festival_quest(&stage, FIRST);

    assert_eq!(status(&mut stage), status_reply(&stage, STATUS_NONE));
    refused(&mut stage, FIRST, vec![(SMSG_GOSSIP_COMPLETE, Vec::new())]);

    festival(&stage, "debug_event_start");
    assert_eq!(status(&mut stage), status_reply(&stage, STATUS_AVAILABLE));
    accepted(&mut stage, FIRST);
}

// vmangos GetDialogStatus hides an inactive quest from its ender; CanRewardQuest does not check it.
#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_accepted_event_quest_stays_in_the_log_and_rewards_when_its_event_ends() {
    let mut stage = stage("quest-availability-event-end");
    offer(&stage, FIRST, 1, 1);
    stage.shard.insert_row(
        "game_creature_quest",
        &[
            ("id", &(u64::from(FIRST) + 1_000_000_000)),
            ("creature_entry", &SCENARIO_QUESTGIVER_ENTRY),
            ("quest_entry", &FIRST),
            ("role", &1),
        ],
    );
    festival_quest(&stage, FIRST);
    festival(&stage, "debug_event_start");
    accepted(&mut stage, FIRST);

    festival(&stage, "debug_event_stop");

    assert!(has_quest(&stage, FIRST));
    assert_eq!(status(&mut stage), status_reply(&stage, STATUS_NONE));
    let mut body = stage.giver.to_le_bytes().to_vec();
    body.extend(FIRST.to_le_bytes());
    body.extend(0u32.to_le_bytes());
    request(&mut stage.connection, CMSG_QUESTGIVER_CHOOSE_REWARD, body);
    let rows = stage.shard.query_rows(&format!(
        "SELECT rewarded FROM game_character_quest WHERE quest_entry = {FIRST}"
    ));
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["rewarded"], "true");
}
