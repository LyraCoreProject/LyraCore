mod support;

use lyracore_shared::quest::{share_event_kind, share_result, QUEST_FLAG_SHARABLE};
use serde_json::json;
use support::{stage_group, Standalone};

const HUMAN: u8 = 1;
const WARRIOR: u8 = 1;
const STONE_AT: (f32, f32, f32) = (-8949.95, -132.493, 83.5312);

const SHARER: u64 = 509_6200;
const RECEIVER: u64 = 509_6201;
const SHARED: u32 = 5_096_200;
const OTHER: u32 = SHARED + 1;
const UNSHAREABLE: u32 = SHARED + 2;

fn start(name: &str) -> Standalone {
    let mut realm = Standalone::start(name);
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    realm.assert_call("install_guid_range", &["0"]);
    realm.clear_schedule("game_event_reaper_schedule");
    let (x, y, z) = STONE_AT;
    for guid in [SHARER, RECEIVER] {
        realm.assert_call(
            "debug_stage_meeting_stone_character",
            &[
                &guid.to_string(),
                &guid.to_string(),
                &HUMAN.to_string(),
                &WARRIOR.to_string(),
                "18",
                "0",
                &x.to_string(),
                &y.to_string(),
                &z.to_string(),
            ],
        );
    }
    stage_group(&realm, SHARER, &[RECEIVER]);
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
