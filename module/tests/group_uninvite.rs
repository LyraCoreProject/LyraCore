mod support;

use serde_json::json;
use support::Standalone;

const HUMAN: u8 = 1;
const WARRIOR: u8 = 1;
const STONE_AT: (f32, f32, f32) = (-8949.95, -132.493, 83.5312);

const DESTROYED: u8 = 3;
const UNINVITED: u8 = 31;

fn start(name: &str) -> Standalone {
    let mut realm = Standalone::start(name);
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    realm.assert_call("install_guid_range", &["0"]);
    realm.assert_sql("DELETE FROM game_event_reaper_schedule");
    realm
}

fn stage(realm: &Standalone, guid: u64) {
    let (x, y, z) = STONE_AT;
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

fn stage_group(realm: &Standalone, leader: u64, members: &[u64]) {
    realm.assert_call(
        "debug_stage_meeting_stone_group",
        &[&leader.to_string(), &json!(members).to_string(), "false"],
    );
}

fn session(guid: u64) -> String {
    json!({
        "guid": guid,
        "ownership": { "some": { "account_id": guid, "generation": 1, "request_nonce": guid } },
    })
    .to_string()
}

fn heard(realm: &Standalone, guid: u64) -> Vec<u8> {
    let mut rows: Vec<(u64, u8)> = realm
        .query_rows(&format!(
            "SELECT id, kind FROM game_group_event WHERE recipient_guid = {guid} \
             AND (kind = {DESTROYED} OR kind = {UNINVITED})"
        ))
        .into_iter()
        .map(|row| (row["id"].parse().unwrap(), row["kind"].parse().unwrap()))
        .collect();
    rows.sort_unstable();
    rows.into_iter().map(|(_, kind)| kind).collect()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_member_kicked_from_a_surviving_group_hears_uninvite_before_destroyed() {
    let realm = start("group-uninvite-survives");
    const LEADER: u64 = 509_6100;
    const KICKED: u64 = 509_6101;
    const STAYS: u64 = 509_6102;
    for guid in [LEADER, KICKED, STAYS] {
        stage(&realm, guid);
    }
    stage_group(&realm, LEADER, &[KICKED, STAYS]);

    realm.assert_call(
        "gw_group_uninvite",
        &[&session(LEADER), &KICKED.to_string()],
    );

    assert_eq!(heard(&realm, KICKED), [UNINVITED, DESTROYED]);
    assert!(heard(&realm, STAYS).is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_kick_that_disbands_the_group_sends_only_destroyed() {
    let realm = start("group-uninvite-disbands");
    const LEADER: u64 = 509_6110;
    const KICKED: u64 = 509_6111;
    for guid in [LEADER, KICKED] {
        stage(&realm, guid);
    }
    stage_group(&realm, LEADER, &[KICKED]);

    realm.assert_call(
        "gw_group_uninvite",
        &[&session(LEADER), &KICKED.to_string()],
    );

    assert_eq!(heard(&realm, KICKED), [DESTROYED]);
    assert_eq!(heard(&realm, LEADER), [DESTROYED]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_member_who_leaves_hears_no_uninvite() {
    let realm = start("group-uninvite-leave");
    const LEADER: u64 = 509_6120;
    const LEAVER: u64 = 509_6121;
    const STAYS: u64 = 509_6122;
    for guid in [LEADER, LEAVER, STAYS] {
        stage(&realm, guid);
    }
    stage_group(&realm, LEADER, &[LEAVER, STAYS]);

    realm.assert_call("gw_group_leave", &[&session(LEAVER)]);

    assert_eq!(heard(&realm, LEAVER), [DESTROYED]);
}
