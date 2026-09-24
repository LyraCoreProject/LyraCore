//! `ForgetDeletedCharacter` on a private Standalone: the Gateway's character-gone reconciliation
//! sends it as the deleted Character, with no ownership token, once no World Shard holds that
//! Character. A deleted Guild Leader passes leadership to the highest Guild Rank, earliest join
//! first (`cm:Guild.cpp:493-552`); the last member disbands the Guild; a deleted owner's Petition
//! closes and a deleted signer's Signature is struck (`cm:Player.cpp:4061-4062`).
//!
//! Each event capture subscribes to rows no earlier step wrote, so a reap of older Guild Events
//! can never be the transaction it reads.

mod support;

use support::{actor, Standalone};

const TEAM_ALLIANCE: u32 = 469;

const LEADER: u64 = 5_093_001;
const LATE_OFFICER: u64 = 5_093_002;
const EARLY_OFFICER: u64 = 5_093_003;
const INVITEE: u64 = 5_093_004;
const OWNER: u64 = 5_093_010;
const SIGNER: u64 = 5_093_011;
const PETITION: u32 = 5_093_020;
const CHARTER: u64 = 5_093_021;

const FORGET: &str = r#"{"forgetDeletedCharacter":{}}"#;

/// Guild Event kinds (`lyracore_shared::guild::event_kind`).
const LEFT: u64 = 4;
const LEADER_CHANGED: u64 = 7;
const DISBANDED: u64 = 8;
const PETITION_CHANGED: u64 = 0x75;

fn forget(realm: &Standalone, guid: u64) {
    realm.assert_call("realm_guild_op", &[&actor(&guid.to_string()), FORGET]);
}

/// The Guild Event rows one `forget` inserted into the rows `filter` selects.
fn forget_events(realm: &Standalone, guid: u64, filter: &str) -> Vec<serde_json::Value> {
    let updates = realm.capture_updates(
        &format!("SELECT * FROM game_guild_event WHERE {filter}"),
        1,
        || forget(realm, guid),
    );
    updates[0]["game_guild_event"]["inserts"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn strings(event: &serde_json::Value) -> Vec<String> {
    event["strings"]
        .as_array()
        .expect("strings is an array")
        .iter()
        .map(|text| text.as_str().expect("a string").to_string())
        .collect()
}

fn rows(realm: &Standalone, query: &str) -> Vec<std::collections::BTreeMap<String, String>> {
    realm.query_rows(query)
}

fn insert_member(realm: &Standalone, guild_id: &str, guid: u64, name: &str, joined_micros: i64) {
    realm.assert_sql(&format!(
        "INSERT INTO game_guild_member (character_guid,guild_id,rank_id,name,public_note,officer_note,realm_account_id,joined_micros) VALUES ({guid},{guild_id},1,'{name}','','',{guid},{joined_micros})"
    ));
}

/// Leader, then two Officers at rank 1: the later one first in guid order, so only the join time
/// can pick the earlier one. The Leader has invited one more Character.
fn seeded_guild(realm: &Standalone) -> String {
    realm.assert_call(
        "realm_guild_op",
        &[
            &actor(&LEADER.to_string()),
            &format!(
                r#"{{"gmCreate":{{"leader_guid":{LEADER},"leader_name":"Leader","leader_team":{TEAM_ALLIANCE},"leader_realm_account":{LEADER},"gm_level":1,"name":"Cleanup Guild"}}}}"#
            ),
        ],
    );
    let guild_id = rows(
        realm,
        "SELECT * FROM game_guild WHERE name = 'Cleanup Guild'",
    )[0]["guild_id"]
        .clone();
    insert_member(realm, &guild_id, LATE_OFFICER, "Late", 2_000);
    insert_member(realm, &guild_id, EARLY_OFFICER, "Early", 1_000);
    realm.assert_call(
        "realm_guild_op",
        &[
            &actor(&LEADER.to_string()),
            &format!(
                r#"{{"invite":{{"target_guid":{INVITEE},"actor_team":{TEAM_ALLIANCE},"target_team":{TEAM_ALLIANCE},"target_ignores_actor":false}}}}"#
            ),
        ],
    );
    guild_id
}

fn member_rank(realm: &Standalone, guid: u64) -> Option<String> {
    rows(
        realm,
        &format!("SELECT * FROM game_guild_member WHERE character_guid = {guid}"),
    )
    .first()
    .map(|row| row["rank_id"].clone())
}

/// Found a Guild with three members, forget the Guild Leader, and read the succession; forget it
/// again and see nothing change; forget the rest and see the Guild disband.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_deleted_leaders_guild_passes_on_then_disbands_with_its_last_member() {
    let mut realm = Standalone::start("guild-cleanup-succession");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    let guild_id = seeded_guild(&realm);

    let events = forget_events(
        &realm,
        LEADER,
        &format!("guild_id = {guild_id} AND recipient_guid = 0"),
    );
    let kinds: Vec<u64> = events.iter().map(|e| e["kind"].as_u64().unwrap()).collect();
    assert_eq!(kinds, [LEADER_CHANGED, LEFT]);
    assert_eq!(strings(&events[0]), ["Leader", "Early"]);
    assert_eq!(strings(&events[1]), ["Leader"]);
    assert_eq!(events[1]["subject_guid"].as_u64(), Some(LEADER));
    assert_eq!(
        rows(
            &realm,
            &format!("SELECT * FROM game_guild WHERE guild_id = {guild_id}")
        )[0]["leader_guid"],
        EARLY_OFFICER.to_string()
    );
    assert_eq!(member_rank(&realm, EARLY_OFFICER).as_deref(), Some("0"));
    assert_eq!(member_rank(&realm, LATE_OFFICER).as_deref(), Some("1"));
    assert_eq!(member_rank(&realm, LEADER), None);
    assert!(
        rows(
            &realm,
            &format!("SELECT * FROM game_guild_invite WHERE target_guid = {INVITEE}")
        )
        .is_empty(),
        "the deleted Leader's Guild Invite goes with it"
    );

    // A replay finds nothing and still succeeds.
    forget(&realm, LEADER);
    assert_eq!(member_rank(&realm, EARLY_OFFICER).as_deref(), Some("0"));

    let events = forget_events(
        &realm,
        LATE_OFFICER,
        &format!("subject_guid = {LATE_OFFICER}"),
    );
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["kind"].as_u64(), Some(LEFT));
    assert_eq!(strings(&events[0]), ["Late"]);
    assert_eq!(member_rank(&realm, LATE_OFFICER), None);

    let events = forget_events(
        &realm,
        EARLY_OFFICER,
        &format!("recipient_guid = {EARLY_OFFICER}"),
    );
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["kind"].as_u64(), Some(DISBANDED));
    for table in ["game_guild", "game_guild_rank", "game_guild_member"] {
        assert!(
            rows(
                &realm,
                &format!("SELECT * FROM {table} WHERE guild_id = {guild_id}")
            )
            .is_empty(),
            "{table} still holds the disbanded Guild"
        );
    }
}

/// A deleted signer's Signature is struck and the owner hears PETITION_CHANGED; a deleted owner's
/// Petition closes.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_deleted_signer_and_owner_leave_no_petition_behind() {
    let mut realm = Standalone::start("guild-cleanup-petition");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    realm.assert_sql(&format!(
        "INSERT INTO game_guild_petition (petition_id,charter_item_guid,owner_guid,owner_name,team,name,created_micros) VALUES ({PETITION},{CHARTER},{OWNER},'Owner',{TEAM_ALLIANCE},'Cleanup Charter',0)"
    ));
    let signature_key = u64::from(PETITION) << 8;
    realm.assert_sql(&format!(
        "INSERT INTO game_guild_petition_signature (signature_key,petition_id,signer_guid,signer_name,signer_realm_account,signed_micros) VALUES ({signature_key},{PETITION},{SIGNER},'Signer',{SIGNER},0)"
    ));

    let events = forget_events(&realm, SIGNER, &format!("recipient_guid = {OWNER}"));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["kind"].as_u64(), Some(PETITION_CHANGED));
    assert_eq!(events[0]["other_guid"].as_u64(), Some(CHARTER));
    assert!(rows(
        &realm,
        &format!("SELECT * FROM game_guild_petition_signature WHERE signer_guid = {SIGNER}")
    )
    .is_empty());

    forget(&realm, OWNER);
    assert!(rows(
        &realm,
        &format!("SELECT * FROM game_guild_petition WHERE petition_id = {PETITION}")
    )
    .is_empty());
}
