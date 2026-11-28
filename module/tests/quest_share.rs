mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_shared::quest::{share_event_kind, share_result, QUEST_FLAG_SHARABLE};
use serde_json::{json, Value};
use support::{stage_group, Standalone};

const BASE_QUEST: u32 = 7_700;
const PACKAGE_SHAREABLE: u32 = 8_000_001;
const PACKAGE_PLAIN: u32 = 8_000_002;

const HUMAN: u8 = 1;
const WARRIOR: u8 = 1;
const STONE_AT: (f32, f32, f32) = (-8949.95, -132.493, 83.5312);

const SHARER: u64 = 509_6200;
const RECEIVER: u64 = 509_6201;
const SHARED: u32 = 5_096_200;
const OTHER: u32 = SHARED + 1;
const UNSHAREABLE: u32 = SHARED + 2;
const FIRST_FILLER: u32 = SHARED + 100;
const LOG_SIZE: u32 = 20;

#[derive(Clone, Copy)]
struct Placement {
    map_id: u32,
    yards_east: f32,
}

const BESIDE_SHARER: Placement = Placement {
    map_id: 0,
    yards_east: 0.0,
};

fn start_party(name: &str) -> Standalone {
    start_party_with(name, BESIDE_SHARER)
}

fn start_party_with(name: &str, receiver: Placement) -> Standalone {
    let mut realm = Standalone::start(name);
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    realm.assert_call("install_guid_range", &["0"]);
    realm.clear_schedule("game_event_reaper_schedule");
    let (x, y, z) = STONE_AT;
    for (guid, placement) in [(SHARER, BESIDE_SHARER), (RECEIVER, receiver)] {
        realm.assert_call(
            "debug_stage_meeting_stone_character",
            &[
                &guid.to_string(),
                &guid.to_string(),
                &HUMAN.to_string(),
                &WARRIOR.to_string(),
                "18",
                &placement.map_id.to_string(),
                &(x + placement.yards_east).to_string(),
                &y.to_string(),
                &z.to_string(),
            ],
        );
    }
    stage_group(&realm, SHARER, &[RECEIVER]);
    realm
}

fn start(name: &str) -> Standalone {
    start_with(name, BESIDE_SHARER)
}

fn start_with(name: &str, receiver: Placement) -> Standalone {
    let realm = start_party_with(name, receiver);
    for (entry, flags) in [
        (SHARED, QUEST_FLAG_SHARABLE),
        (OTHER, QUEST_FLAG_SHARABLE),
        (UNSHAREABLE, 0),
    ] {
        realm.insert_row(
            "game_quest_template",
            &[
                ("entry", &entry),
                ("quest_level", &18),
                ("title", &"Shared quest"),
                ("zone_or_sort", &12),
            ],
        );
        realm.insert_row(
            "game_quest_flags",
            &[("quest_entry", &entry), ("flags", &flags)],
        );
        realm.assert_call(
            "debug_grant_quest",
            &[&SHARER.to_string(), &entry.to_string()],
        );
    }
    realm
}

fn package_quest(entry: u32) -> Value {
    json!({
        "table": "game_quest_template",
        "key": { "entry": entry },
        "operation": "insert",
        "fields": {
            "min_level": { "type": "u32", "value": 1 },
            "quest_level": { "type": "u32", "value": 18 },
            "title": { "type": "string", "value": "Package quest" },
            "reward_money": { "type": "u32", "value": 0 },
            "reward_xp": { "type": "u32", "value": 0 },
            "prev_quest_id": { "type": "u32", "value": 0 },
            "required_races": { "type": "u32", "value": 0 },
            "required_classes": { "type": "u32", "value": 0 },
            "zone_or_sort": { "type": "i32", "value": 12 },
            "rew_rep_faction_1": { "type": "u32", "value": 0 },
            "rew_rep_value_1": { "type": "i32", "value": 0 },
            "rew_rep_faction_2": { "type": "u32", "value": 0 },
            "rew_rep_value_2": { "type": "i32", "value": 0 },
            "src_item": { "type": "u32", "value": 0 },
            "src_item_count": { "type": "u32", "value": 0 },
            "repeatable": { "type": "bool", "value": false },
            "next_quest_id": { "type": "u32", "value": 0 },
            "limit_time": { "type": "u32", "value": 0 },
            "reward_money_max_level": { "type": "u32", "value": 0 },
            "quest_type": { "type": "u32", "value": 0 },
        },
    })
}

fn quest_flags_claim(entry: u32, operation: &str, flags: u32) -> Value {
    json!({
        "table": "game_quest_flags",
        "key": { "quest_entry": entry },
        "operation": operation,
        "fields": { "flags": { "type": "u32", "value": flags } },
    })
}

fn apply_quests(realm: &Standalone, claims: &[Value]) {
    let artifact = json!({
        "version": 1,
        "package": "example.sharing",
        "source_hash": "1".repeat(64),
        "claims": claims,
    });
    realm.assert_call(
        "apply_package_deltas",
        &[
            &json!("quests").to_string(),
            &json!(artifact.to_string()).to_string(),
        ],
    );
}

fn stored_flags(realm: &Standalone, quest: u32) -> Vec<String> {
    realm
        .query_rows(&format!(
            "SELECT flags FROM game_quest_flags WHERE quest_entry = {quest}"
        ))
        .into_iter()
        .map(|row| row["flags"].clone())
        .collect()
}

fn assert_cannot_be_shared(realm: &Standalone, quest: u32) {
    let output = push(realm, quest);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("that quest cannot be shared"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn push(realm: &Standalone, quest: u32) -> std::process::Output {
    realm.call(
        "debug_push_quest",
        &[&SHARER.to_string(), &quest.to_string()],
    )
}

fn accept_from_sharer(realm: &Standalone, quest: u32) -> std::process::Output {
    realm.call(
        "debug_accept_quest",
        &[
            &RECEIVER.to_string(),
            &SHARER.to_string(),
            &quest.to_string(),
        ],
    )
}

fn assert_refused(output: &std::process::Output, reason: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "accepted: {stderr}");
    assert!(stderr.contains(reason), "{stderr}");
}

fn quest_shares(realm: &Standalone) -> Vec<(u64, u64, u32)> {
    realm
        .query_rows("SELECT receiver_guid, sharer_guid, quest_entry FROM game_quest_share")
        .into_iter()
        .map(|row| {
            (
                row["receiver_guid"].parse().unwrap(),
                row["sharer_guid"].parse().unwrap(),
                row["quest_entry"].parse().unwrap(),
            )
        })
        .collect()
}

fn push_results(realm: &Standalone) -> Vec<u8> {
    let mut rows: Vec<(u64, u8)> = realm
        .query_rows(&format!(
            "SELECT id, payload FROM game_group_event WHERE recipient_guid = {SHARER} \
             AND kind = {} AND other_guid = {RECEIVER}",
            share_event_kind::QUEST_PUSH_RESULT
        ))
        .into_iter()
        .map(|row| (row["id"].parse().unwrap(), row["payload"].parse().unwrap()))
        .collect();
    rows.sort_unstable();
    rows.into_iter().map(|(_, result)| result).collect()
}

fn receiver_quests(realm: &Standalone) -> Vec<u32> {
    realm
        .query_rows(&format!(
            "SELECT quest_entry FROM game_character_quest WHERE character_guid = {RECEIVER}"
        ))
        .into_iter()
        .map(|row| row["quest_entry"].parse().unwrap())
        .collect()
}

fn move_east(realm: &Standalone, guid: u64, yards: f32) {
    realm.assert_sql(&format!(
        "UPDATE game_world_entity SET x = {} WHERE guid = {guid}",
        STONE_AT.0 + yards
    ));
}

fn grant(realm: &Standalone, guid: u64, quest: u32) {
    realm.assert_call(
        "debug_grant_quest",
        &[&guid.to_string(), &quest.to_string()],
    );
}

fn session(guid: u64) -> String {
    json!({
        "guid": guid,
        "ownership": { "some": { "account_id": guid, "generation": 1, "request_nonce": guid } },
    })
    .to_string()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_party_member_takes_only_the_quest_shared_with_them() {
    let realm = start("quest-share-accept");
    assert_refused(
        &accept_from_sharer(&realm, SHARED),
        "no party member shared that quest",
    );

    assert!(push(&realm, SHARED).status.success());
    assert_eq!(quest_shares(&realm), [(RECEIVER, SHARER, SHARED)]);
    assert!(push(&realm, OTHER).status.success());
    assert_eq!(
        quest_shares(&realm),
        [(RECEIVER, SHARER, SHARED)],
        "a held share is not replaced"
    );
    assert!(
        accept_from_sharer(&realm, OTHER).status.success(),
        "a refused accept that ends a share commits"
    );
    assert!(receiver_quests(&realm).is_empty());
    assert!(quest_shares(&realm).is_empty());

    assert!(push(&realm, SHARED).status.success());
    assert!(accept_from_sharer(&realm, SHARED).status.success());
    assert_eq!(receiver_quests(&realm), [SHARED]);
    assert!(quest_shares(&realm).is_empty());
    assert_eq!(
        push_results(&realm),
        [
            share_result::SHARING_QUEST,
            share_result::SHARING_QUEST,
            share_result::BUSY,
            share_result::SHARING_QUEST,
            share_result::ACCEPT_QUEST,
        ]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_refused_accept_ends_the_share() {
    let realm = start("quest-share-refused");
    assert!(push(&realm, SHARED).status.success());
    realm.assert_call("gw_abandon_quest", &[&session(SHARER), &SHARED.to_string()]);
    assert!(accept_from_sharer(&realm, SHARED).status.success());
    assert!(receiver_quests(&realm).is_empty());
    assert!(quest_shares(&realm).is_empty());

    realm.assert_call(
        "debug_grant_quest",
        &[&SHARER.to_string(), &SHARED.to_string()],
    );
    assert!(push(&realm, SHARED).status.success());
    assert_eq!(quest_shares(&realm), [(RECEIVER, SHARER, SHARED)]);
    assert_eq!(
        push_results(&realm),
        [share_result::SHARING_QUEST, share_result::SHARING_QUEST]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_quest_without_the_shareable_flag_cannot_be_pushed() {
    let realm = start("quest-share-unshareable");
    let output = push(&realm, UNSHAREABLE);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("that quest cannot be shared"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(quest_shares(&realm).is_empty());
    assert!(push_results(&realm).is_empty());
    assert_refused(
        &accept_from_sharer(&realm, UNSHAREABLE),
        "that quest giver does not offer this quest",
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_decline_or_the_sharer_leaving_the_group_ends_the_share() {
    let realm = start("quest-share-ends");
    assert!(push(&realm, SHARED).status.success());
    realm.assert_call(
        "debug_answer_quest_share",
        &[
            &RECEIVER.to_string(),
            &share_result::DECLINE_QUEST.to_string(),
        ],
    );
    assert!(quest_shares(&realm).is_empty());
    assert_eq!(
        push_results(&realm),
        [share_result::SHARING_QUEST, share_result::DECLINE_QUEST]
    );
    assert_refused(
        &accept_from_sharer(&realm, SHARED),
        "no party member shared that quest",
    );

    assert!(push(&realm, SHARED).status.success());
    assert_eq!(quest_shares(&realm), [(RECEIVER, SHARER, SHARED)]);
    realm.assert_call("gw_group_leave", &[&session(SHARER)]);
    assert!(quest_shares(&realm).is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_package_quest_is_shared_only_when_its_package_marks_it_shareable() {
    let realm = start_party("quest-share-package");
    apply_quests(
        &realm,
        &[
            package_quest(PACKAGE_SHAREABLE),
            quest_flags_claim(PACKAGE_SHAREABLE, "insert", QUEST_FLAG_SHARABLE | 0x2),
            package_quest(PACKAGE_PLAIN),
        ],
    );
    assert_eq!(
        stored_flags(&realm, PACKAGE_SHAREABLE),
        [(QUEST_FLAG_SHARABLE | 0x2).to_string()]
    );
    for quest in [PACKAGE_SHAREABLE, PACKAGE_PLAIN] {
        realm.assert_call(
            "debug_grant_quest",
            &[&SHARER.to_string(), &quest.to_string()],
        );
    }

    assert_cannot_be_shared(&realm, PACKAGE_PLAIN);
    assert!(quest_shares(&realm).is_empty());

    assert!(push(&realm, PACKAGE_SHAREABLE).status.success());
    assert!(accept_from_sharer(&realm, PACKAGE_SHAREABLE)
        .status
        .success());
    assert_eq!(receiver_quests(&realm), [PACKAGE_SHAREABLE]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_package_replays_its_quest_flags_after_the_importer_refills_the_flags_table() {
    let realm = start_party("quest-share-package-replay");
    realm.insert_row(
        "game_quest_template",
        &[
            ("entry", &BASE_QUEST),
            ("quest_level", &18),
            ("title", &"Base quest"),
            ("zone_or_sort", &12),
        ],
    );
    realm.insert_row(
        "game_quest_flags",
        &[("quest_entry", &BASE_QUEST), ("flags", &0u32)],
    );
    realm.assert_call(
        "debug_grant_quest",
        &[&SHARER.to_string(), &BASE_QUEST.to_string()],
    );
    assert_cannot_be_shared(&realm, BASE_QUEST);

    let package = [
        package_quest(PACKAGE_SHAREABLE),
        quest_flags_claim(PACKAGE_SHAREABLE, "insert", QUEST_FLAG_SHARABLE),
        quest_flags_claim(BASE_QUEST, "update", QUEST_FLAG_SHARABLE),
    ];
    apply_quests(&realm, &package);
    apply_quests(&realm, &package);
    assert_eq!(
        stored_flags(&realm, PACKAGE_SHAREABLE),
        [QUEST_FLAG_SHARABLE.to_string()],
        "applying the plan again replaces the Package's flags row"
    );
    assert_eq!(
        stored_flags(&realm, BASE_QUEST),
        [QUEST_FLAG_SHARABLE.to_string()]
    );
    assert!(push(&realm, BASE_QUEST).status.success());
    realm.assert_call(
        "debug_answer_quest_share",
        &[
            &RECEIVER.to_string(),
            &share_result::DECLINE_QUEST.to_string(),
        ],
    );

    realm.assert_sql("DELETE FROM game_quest_flags WHERE quest_entry > 0");
    realm.insert_row(
        "game_quest_flags",
        &[("quest_entry", &BASE_QUEST), ("flags", &0u32)],
    );
    assert!(stored_flags(&realm, PACKAGE_SHAREABLE).is_empty());
    assert_eq!(stored_flags(&realm, BASE_QUEST), ["0"]);
    assert_cannot_be_shared(&realm, BASE_QUEST);

    apply_quests(&realm, &package);
    assert_eq!(
        stored_flags(&realm, PACKAGE_SHAREABLE),
        [QUEST_FLAG_SHARABLE.to_string()]
    );
    assert_eq!(
        stored_flags(&realm, BASE_QUEST),
        [QUEST_FLAG_SHARABLE.to_string()]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_push_offers_the_quest_to_a_member_within_14_yards() {
    let realm = start_with(
        "quest-share-near",
        Placement {
            map_id: 0,
            yards_east: 13.0,
        },
    );
    assert!(push(&realm, SHARED).status.success());
    assert_eq!(quest_shares(&realm), [(RECEIVER, SHARER, SHARED)]);
    assert_eq!(push_results(&realm), [share_result::SHARING_QUEST]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_push_tells_the_sharer_too_far_for_a_member_beyond_14_yards() {
    let realm = start_with(
        "quest-share-far",
        Placement {
            map_id: 0,
            yards_east: 15.0,
        },
    );
    assert!(push(&realm, SHARED).status.success());
    assert!(quest_shares(&realm).is_empty());
    assert_eq!(
        push_results(&realm),
        [share_result::SHARING_QUEST, share_result::TOO_FAR]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_push_tells_the_sharer_too_far_for_a_member_on_another_map() {
    let realm = start_with(
        "quest-share-other-map",
        Placement {
            map_id: 1,
            yards_east: 0.0,
        },
    );
    assert!(push(&realm, SHARED).status.success());
    assert!(quest_shares(&realm).is_empty());
    assert_eq!(
        push_results(&realm),
        [share_result::SHARING_QUEST, share_result::TOO_FAR]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_member_whose_quest_is_complete_hears_finish_quest() {
    let realm = start("quest-share-finished");
    grant(&realm, RECEIVER, SHARED);
    assert!(push(&realm, SHARED).status.success());
    assert!(quest_shares(&realm).is_empty());
    assert_eq!(
        push_results(&realm),
        [share_result::SHARING_QUEST, share_result::FINISH_QUEST]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_member_whose_quest_is_unfinished_hears_have_quest() {
    let realm = start_party("quest-share-have");
    realm.insert_row(
        "game_quest_template",
        &[
            ("entry", &SHARED),
            ("quest_level", &18),
            ("title", &"Shared quest"),
            ("zone_or_sort", &12),
        ],
    );
    realm.insert_row(
        "game_quest_flags",
        &[("quest_entry", &SHARED), ("flags", &QUEST_FLAG_SHARABLE)],
    );
    realm.insert_row(
        "game_quest_objective",
        &[
            ("id", &SHARED),
            ("quest_entry", &SHARED),
            ("target_entry", &1),
            ("required_count", &1),
        ],
    );
    grant(&realm, SHARER, SHARED);
    grant(&realm, RECEIVER, SHARED);
    assert!(push(&realm, SHARED).status.success());
    assert!(quest_shares(&realm).is_empty());
    assert_eq!(
        push_results(&realm),
        [share_result::SHARING_QUEST, share_result::HAVE_QUEST]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_accept_beyond_14_yards_tells_the_sharer_too_far_and_takes_nothing() {
    let realm = start("quest-share-accept-far");
    assert!(push(&realm, SHARED).status.success());
    move_east(&realm, SHARER, 15.0);
    assert!(
        accept_from_sharer(&realm, SHARED).status.success(),
        "a refused accept that ends a share commits"
    );
    assert!(receiver_quests(&realm).is_empty());
    assert!(quest_shares(&realm).is_empty());
    assert_eq!(
        push_results(&realm),
        [share_result::SHARING_QUEST, share_result::TOO_FAR]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_sharer_on_another_map_hears_nothing_when_the_accept_is_refused() {
    let realm = start("quest-share-accept-other-map");
    assert!(push(&realm, SHARED).status.success());
    realm.assert_sql(&format!(
        "UPDATE game_world_entity SET map_id = 1 WHERE guid = {SHARER}"
    ));
    assert!(accept_from_sharer(&realm, SHARED).status.success());
    assert!(receiver_quests(&realm).is_empty());
    assert!(quest_shares(&realm).is_empty());
    assert_eq!(push_results(&realm), [share_result::SHARING_QUEST]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_accept_the_take_check_refuses_sends_the_sharer_nothing_even_when_apart() {
    let realm = start("quest-share-accept-take-check");
    assert!(push(&realm, SHARED).status.success());
    realm.assert_sql(&format!(
        "UPDATE game_quest_template SET min_level = 60 WHERE entry = {SHARED}"
    ));
    move_east(&realm, SHARER, 15.0);
    assert!(accept_from_sharer(&realm, SHARED).status.success());
    assert!(receiver_quests(&realm).is_empty());
    assert!(quest_shares(&realm).is_empty());
    assert_eq!(push_results(&realm), [share_result::SHARING_QUEST]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn the_sharer_hears_accept_quest_before_the_log_check_refuses_the_accept() {
    let realm = start("quest-share-accept-log-full");
    assert!(push(&realm, SHARED).status.success());
    for offset in 0..LOG_SIZE {
        let filler = FIRST_FILLER + offset;
        realm.insert_row(
            "game_quest_template",
            &[
                ("entry", &filler),
                ("quest_level", &18),
                ("title", &"Filler quest"),
                ("zone_or_sort", &12),
            ],
        );
        grant(&realm, RECEIVER, filler);
    }
    assert!(
        accept_from_sharer(&realm, SHARED).status.success(),
        "a refused accept that ends a share commits"
    );
    assert_eq!(receiver_quests(&realm).len(), LOG_SIZE as usize);
    assert!(quest_shares(&realm).is_empty());
    assert_eq!(
        push_results(&realm),
        [share_result::SHARING_QUEST, share_result::ACCEPT_QUEST]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_npc_accept_of_the_shared_quest_beyond_14_yards_of_the_sharer_sends_too_far() {
    let realm = start("quest-share-npc-accept");
    realm.assert_call("debug_seed_scenario_fixtures", &[]);
    realm.assert_sql(&format!(
        "DELETE FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
    ));
    realm.assert_call(
        "debug_spawn_at_feet",
        &[&RECEIVER.to_string(), &TEST_WOLF_ENTRY.to_string(), "3"],
    );
    let giver = realm
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
        ))
        .remove(0)["guid"]
        .clone();
    realm.assert_sql(&format!(
        "UPDATE game_world_entity SET npc_flags = {}, faction_template = 35 WHERE guid = {giver}",
        lyracore_shared::constants::npc_flags::QUESTGIVER
    ));
    realm.insert_row(
        "game_creature_quest",
        &[
            ("id", &u64::from(SHARED)),
            ("creature_entry", &TEST_WOLF_ENTRY),
            ("quest_entry", &SHARED),
        ],
    );
    assert!(push(&realm, SHARED).status.success());
    move_east(&realm, SHARER, 15.0);
    let output = realm.call(
        "debug_accept_quest",
        &[&RECEIVER.to_string(), &giver, &SHARED.to_string()],
    );
    assert!(output.status.success());
    assert!(receiver_quests(&realm).is_empty());
    assert!(quest_shares(&realm).is_empty());
    assert_eq!(
        push_results(&realm),
        [share_result::SHARING_QUEST, share_result::TOO_FAR]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_dead_sharer_is_refused_and_hears_nothing() {
    let realm = start("quest-share-dead-sharer");
    assert!(push(&realm, SHARED).status.success());
    realm.assert_sql(&format!(
        "UPDATE game_world_entity SET dead = true WHERE guid = {SHARER}"
    ));
    assert!(
        accept_from_sharer(&realm, SHARED).status.success(),
        "a refused accept that ends a share commits"
    );
    assert!(receiver_quests(&realm).is_empty());
    assert!(quest_shares(&realm).is_empty());
    assert_eq!(push_results(&realm), [share_result::SHARING_QUEST]);
}
