mod support;

use std::collections::BTreeMap;
use support::Standalone;

const GM: u64 = 5_091_001;
const LOW_RANK: u64 = 5_091_002;
const STRANGER: u64 = 5_091_003;

/// A tokenless actor: Realm-core holds no Account Claim for the reserved guids.
fn actor(guid: u64) -> String {
    format!(r#"{{"guid":{guid},"ownership":null}}"#)
}

fn gm_create(leader: u64, leader_name: &str, name: &str) -> String {
    format!(
        r#"{{"gmCreate":{{"leader_guid":{leader},"leader_name":"{leader_name}","leader_team":469,"leader_realm_account":{leader},"gm_level":1,"name":"{name}"}}}}"#
    )
}

fn set_motd(text: &str) -> String {
    format!(r#"{{"setMotd":"{text}"}}"#)
}

fn set_info(text: &str) -> String {
    format!(r#"{{"setInfo":"{text}"}}"#)
}

fn set_public_note(target_guid: u64, text: &str) -> String {
    format!(r#"{{"setPublicNote":{{"target_guid":{target_guid},"text":"{text}"}}}}"#)
}

fn set_officer_note(target_guid: u64, text: &str) -> String {
    format!(r#"{{"setOfficerNote":{{"target_guid":{target_guid},"text":"{text}"}}}}"#)
}

fn edit_rank(rank_id: u32, rank_rights: u32, name: &str) -> String {
    format!(r#"{{"editRank":{{"rank_id":{rank_id},"rights":{rank_rights},"name":"{name}"}}}}"#)
}

fn add_rank(name: &str) -> String {
    format!(r#"{{"addRank":"{name}"}}"#)
}

const DELETE_RANK: &str = r#"{"deleteRank":{}}"#;

fn refused(realm: &Standalone, args: &[&str], tag: &str) {
    let result = realm.call("realm_guild_op", args);
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !result.status.success() && output.contains(tag),
        "expected {tag}: {output}"
    );
}

/// Found `name` led by `GM` and answer its `guild_id`.
fn found(realm: &Standalone, name: &str) -> String {
    realm.assert_call(
        "realm_guild_op",
        &[&actor(GM), &gm_create(GM, "Leader", name)],
    );
    realm.query_rows(&format!("SELECT * FROM game_guild WHERE name = '{name}'"))[0]["guild_id"]
        .clone()
}

/// Seed a member row directly: these tests need a second member's data, not a full invite and
/// accept.
fn insert_member(realm: &Standalone, guild_id: &str, guid: u64, rank_id: u32, name: &str) {
    realm.assert_sql(&format!(
        "INSERT INTO game_guild_member (character_guid,guild_id,rank_id,name,public_note,officer_note,realm_account_id,joined_micros) VALUES ({guid},{guild_id},{rank_id},'{name}','','',{guid},0)"
    ));
}

fn member_row(realm: &Standalone, guid: u64) -> BTreeMap<String, String> {
    realm.query_rows(&format!(
        "SELECT * FROM game_guild_member WHERE character_guid = {guid}"
    ))[0]
        .clone()
}

fn guild_row(realm: &Standalone, guild_id: &str) -> BTreeMap<String, String> {
    realm.query_rows(&format!(
        "SELECT * FROM game_guild WHERE guild_id = {guild_id}"
    ))[0]
        .clone()
}

fn rank_row(realm: &Standalone, guild_id: &str, rank_id: u32) -> BTreeMap<String, String> {
    realm.query_rows(&format!(
        "SELECT * FROM game_guild_rank WHERE guild_id = {guild_id} AND rank_id = {rank_id}"
    ))[0]
        .clone()
}

fn event_count(realm: &Standalone, guild_id: &str) -> usize {
    realm
        .query_rows(&format!(
            "SELECT * FROM game_guild_event WHERE guild_id = {guild_id}"
        ))
        .len()
}

/// Runs only when requested because it builds and publishes the Wasm module to its own standalone.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn motd_and_info_text_gate_on_rank_rights_and_length_caps() {
    let mut realm = Standalone::start("guild-settings-motd-info");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);

    let guild_id = found(&realm, "Tracer Guild");
    insert_member(&realm, &guild_id, LOW_RANK, 4, "Rookie");

    realm.assert_call("realm_guild_op", &[&actor(GM), &set_motd("Assemble!")]);
    assert_eq!(guild_row(&realm, &guild_id)["motd"], "Assemble!");
    assert_eq!(
        event_count(&realm, &guild_id),
        1,
        "MOTD broadcasts one event"
    );

    refused(
        &realm,
        &[&actor(LOW_RANK), &set_motd("Nope")],
        "guild:no_permission",
    );
    assert_eq!(guild_row(&realm, &guild_id)["motd"], "Assemble!");

    refused(
        &realm,
        &[&actor(GM), &set_motd(&"x".repeat(129))],
        "guild:too_long",
    );
    assert_eq!(guild_row(&realm, &guild_id)["motd"], "Assemble!");

    // An empty MOTD is allowed (`cm:GuildHandler.cpp:494-497`).
    realm.assert_call("realm_guild_op", &[&actor(GM), &set_motd("")]);
    assert_eq!(guild_row(&realm, &guild_id)["motd"], "");

    realm.assert_call(
        "realm_guild_op",
        &[&actor(GM), &set_info("A tracer guild.")],
    );
    assert_eq!(guild_row(&realm, &guild_id)["info"], "A tracer guild.");
    assert_eq!(
        event_count(&realm, &guild_id),
        2,
        "guild info text changes push no event of its own"
    );

    refused(
        &realm,
        &[&actor(LOW_RANK), &set_info("Nope")],
        "guild:no_permission",
    );
    refused(
        &realm,
        &[&actor(GM), &set_info(&"x".repeat(501))],
        "guild:too_long",
    );
    assert_eq!(guild_row(&realm, &guild_id)["info"], "A tracer guild.");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn notes_gate_on_rank_rights_and_target_membership_and_answer_the_editor_with_a_roster_event() {
    let mut realm = Standalone::start("guild-settings-notes");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);

    let guild_id = found(&realm, "Tracer Guild");
    insert_member(&realm, &guild_id, LOW_RANK, 4, "Rookie");

    realm.assert_call(
        "realm_guild_op",
        &[&actor(GM), &set_public_note(LOW_RANK, "reliable")],
    );
    assert_eq!(member_row(&realm, LOW_RANK)["public_note"], "reliable");
    let events = realm.query_rows(&format!(
        "SELECT * FROM game_guild_event WHERE guild_id = {guild_id}"
    ));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["kind"], "80", "ROSTER_TO_ACTOR is 0x50");
    assert_eq!(events[0]["recipient_guid"], GM.to_string());

    realm.assert_call(
        "realm_guild_op",
        &[&actor(GM), &set_officer_note(LOW_RANK, "watch closely")],
    );
    assert_eq!(
        member_row(&realm, LOW_RANK)["officer_note"],
        "watch closely"
    );
    assert_eq!(event_count(&realm, &guild_id), 2);

    refused(
        &realm,
        &[&actor(LOW_RANK), &set_public_note(GM, "nope")],
        "guild:no_permission",
    );
    refused(
        &realm,
        &[&actor(LOW_RANK), &set_officer_note(GM, "nope")],
        "guild:no_permission",
    );
    assert_eq!(member_row(&realm, GM)["public_note"], "");
    assert_eq!(member_row(&realm, GM)["officer_note"], "");

    refused(
        &realm,
        &[&actor(GM), &set_public_note(STRANGER, "nope")],
        "guild:target_not_in_guild",
    );

    refused(
        &realm,
        &[&actor(GM), &set_public_note(LOW_RANK, &"x".repeat(32))],
        "guild:too_long",
    );
    assert_eq!(member_row(&realm, LOW_RANK)["public_note"], "reliable");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn rank_edit_add_and_delete_respect_leadership_five_to_ten_bounds_and_move_members() {
    let mut realm = Standalone::start("guild-settings-ranks");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);

    let guild_id = found(&realm, "Tracer Guild");
    insert_member(&realm, &guild_id, LOW_RANK, 4, "Rookie");

    refused(
        &realm,
        &[&actor(LOW_RANK), &edit_rank(2, 0x43, "Veteran+")],
        "guild:not_leader",
    );
    assert_eq!(rank_row(&realm, &guild_id, 2)["name"], "Veteran");

    realm.assert_call(
        "realm_guild_op",
        &[&actor(GM), &edit_rank(2, 0x1040, "Veteran+")],
    );
    let veteran = rank_row(&realm, &guild_id, 2);
    assert_eq!(veteran["name"], "Veteran+");
    assert_eq!(veteran["rights"], "4160");
    assert_eq!(event_count(&realm, &guild_id), 1);
    let events = realm.query_rows(&format!(
        "SELECT * FROM game_guild_event WHERE guild_id = {guild_id}"
    ));
    assert_eq!(events[0]["kind"], "128", "ROSTER_REFRESH is 0x80");
    assert_eq!(events[0]["recipient_guid"], "0");

    realm.assert_call(
        "realm_guild_op",
        &[&actor(GM), &edit_rank(0, 0, "Overlord")],
    );
    let leader_rank = rank_row(&realm, &guild_id, 0);
    assert_eq!(leader_rank["name"], "Overlord");
    assert_eq!(leader_rank["rights"], "1044991", "rank 0 keeps every right");
    assert_eq!(event_count(&realm, &guild_id), 2);

    realm.assert_call("realm_guild_op", &[&actor(GM), &edit_rank(99, 0, "Nobody")]);
    assert_eq!(
        realm
            .query_rows(&format!(
                "SELECT * FROM game_guild_rank WHERE guild_id = {guild_id}"
            ))
            .len(),
        5,
        "an unknown rank id changes nothing"
    );
    assert_eq!(
        event_count(&realm, &guild_id),
        2,
        "an unknown rank id pushes no event"
    );

    refused(
        &realm,
        &[&actor(GM), &edit_rank(2, 0, &"x".repeat(16))],
        "guild:too_long",
    );
    assert_eq!(rank_row(&realm, &guild_id, 2)["name"], "Veteran+");

    realm.assert_call("realm_guild_op", &[&actor(GM), &add_rank("Recruit")]);
    assert_eq!(
        realm
            .query_rows(&format!(
                "SELECT * FROM game_guild_rank WHERE guild_id = {guild_id}"
            ))
            .len(),
        6
    );
    let recruit = rank_row(&realm, &guild_id, 5);
    assert_eq!(recruit["name"], "Recruit");
    assert_eq!(recruit["rights"], "67", "GCHATLISTEN|GCHATSPEAK");

    insert_member(&realm, &guild_id, STRANGER, 5, "Newbie");
    realm.assert_call("realm_guild_op", &[&actor(GM), DELETE_RANK]);
    assert_eq!(
        realm
            .query_rows(&format!(
                "SELECT * FROM game_guild_rank WHERE guild_id = {guild_id} AND rank_id = 5"
            ))
            .len(),
        0
    );
    assert_eq!(
        member_row(&realm, STRANGER)["rank_id"],
        "4",
        "a deleted rank's members move to the new lowest rank"
    );

    refused(&realm, &[&actor(GM), DELETE_RANK], "guild:ranks_at_limit");
    refused(
        &realm,
        &[&actor(LOW_RANK), &add_rank("Nope")],
        "guild:not_leader",
    );
    refused(&realm, &[&actor(LOW_RANK), DELETE_RANK], "guild:not_leader");
    refused(
        &realm,
        &[&actor(GM), &add_rank(&"x".repeat(16))],
        "guild:too_long",
    );

    for extra in 0..5 {
        realm.assert_call(
            "realm_guild_op",
            &[&actor(GM), &add_rank(&format!("Extra{extra}"))],
        );
    }
    assert_eq!(
        realm
            .query_rows(&format!(
                "SELECT * FROM game_guild_rank WHERE guild_id = {guild_id}"
            ))
            .len(),
        10
    );
    refused(
        &realm,
        &[&actor(GM), &add_rank("Overflow")],
        "guild:ranks_at_limit",
    );
}
