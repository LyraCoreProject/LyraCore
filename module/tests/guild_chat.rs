mod support;

use serde_json::json;
use support::{actor, Standalone};

const REALM_CHAT_ROWS: &str = "SELECT * FROM game_realm_chat_event";

const KIND_GUILD: u8 = 3;
const KIND_OFFICER: u8 = 4;

const LEADER: u64 = 5_092_001;
const OFFICER: u64 = 5_092_002;
const VETERAN: u64 = 5_092_003;
const MUTED: u64 = 5_092_004;
const STRANGER: u64 = 5_092_005;

const RANK_OFFICER: u32 = 1;
const RANK_VETERAN: u32 = 2;
const RANK_MUTED: u32 = 4;

fn gm_create(name: &str) -> String {
    format!(
        r#"{{"gmCreate":{{"leader_guid":{LEADER},"leader_name":"Leader","leader_team":469,"leader_realm_account":{LEADER},"gm_level":1,"name":"{name}"}}}}"#
    )
}

fn edit_rank(rank_id: u32, rights: u32, name: &str) -> String {
    format!(r#"{{"editRank":{{"rank_id":{rank_id},"rights":{rights},"name":"{name}"}}}}"#)
}

fn insert_member(realm: &Standalone, guild_id: &str, guid: u64, rank_id: u32, name: &str) {
    realm.assert_sql(&format!(
        "INSERT INTO game_guild_member (character_guid,guild_id,rank_id,name,public_note,officer_note,realm_account_id,joined_micros) VALUES ({guid},{guild_id},{rank_id},'{name}','','',{guid},0)"
    ));
}

fn seeded_guild(realm: &Standalone) -> String {
    realm.assert_call(
        "realm_guild_op",
        &[&actor(&LEADER.to_string()), &gm_create("Tracer Guild")],
    );
    let guild_id = realm.query_rows("SELECT * FROM game_guild WHERE name = 'Tracer Guild'")[0]
        ["guild_id"]
        .clone();
    insert_member(realm, &guild_id, OFFICER, RANK_OFFICER, "Officer");
    insert_member(realm, &guild_id, VETERAN, RANK_VETERAN, "Veteran");
    insert_member(realm, &guild_id, MUTED, RANK_MUTED, "Muted");
    realm.assert_call(
        "realm_guild_op",
        &[
            &actor(&LEADER.to_string()),
            &edit_rank(RANK_MUTED, 0, "Muted"),
        ],
    );
    guild_id
}

fn request(kind: u8, message: &str) -> String {
    json!({
        "kind": kind,
        "language": 0,
        "channel_name": "",
        "target_guid": 0,
        "message": message,
        "speaker": { "race": 1, "chat_tag": 0 },
    })
    .to_string()
}

fn line_of(realm: &Standalone, actor_guid: u64, kind: u8, message: &str) -> (Vec<u64>, bool) {
    let req = request(kind, message);
    let updates = realm.capture_updates(REALM_CHAT_ROWS, 1, || {
        realm.assert_call("realm_chat", &[&actor(&actor_guid.to_string()), &req]);
    });
    let inserts = updates[0]["game_realm_chat_event"]["inserts"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(inserts.len(), 1, "one line is one row: {updates:?}");
    let mut recipients: Vec<u64> = inserts[0]["recipients"]
        .as_array()
        .expect("recipients is an array")
        .iter()
        .map(|guid| guid.as_u64().expect("a guid"))
        .collect();
    recipients.sort_unstable();
    let ignorable = inserts[0]["ignorable"]
        .as_bool()
        .expect("ignorable is a bool");
    assert!(
        support::poll_until(std::time::Duration::from_secs(20), || realm
            .query_rows(REALM_CHAT_ROWS)
            .is_empty()),
        "the event GC never reaped the Realm Chat Line"
    );
    (recipients, ignorable)
}

fn refused(realm: &Standalone, actor_guid: u64, kind: u8, tag: &str) {
    let req = request(kind, "anyone?");
    let output = realm.call("realm_chat", &[&actor(&actor_guid.to_string()), &req]);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "realm_chat kind {kind} by {actor_guid} unexpectedly committed: {text}"
    );
    assert!(
        text.contains(tag),
        "kind {kind} by {actor_guid}: expected {tag}, got {text}"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn guild_chat_reaches_every_member_with_gchatlisten() {
    let mut realm = Standalone::start("guild-chat-guild-audience");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    seeded_guild(&realm);

    let (recipients, ignorable) = line_of(&realm, VETERAN, KIND_GUILD, "form up");
    assert_eq!(recipients, [LEADER, OFFICER, VETERAN], "Muted is excluded");
    assert!(ignorable);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn officer_chat_reaches_only_officer_ranked_members() {
    let mut realm = Standalone::start("guild-chat-officer-audience");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    seeded_guild(&realm);

    let (recipients, ignorable) = line_of(&realm, OFFICER, KIND_OFFICER, "raid at 8");
    assert_eq!(recipients, [LEADER, OFFICER]);
    assert!(ignorable);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_rank_without_the_speak_right_and_a_non_member_are_refused() {
    let mut realm = Standalone::start("guild-chat-refusals");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    seeded_guild(&realm);

    refused(&realm, MUTED, KIND_GUILD, "chat:no_guild_chat_right");
    refused(&realm, VETERAN, KIND_OFFICER, "chat:no_guild_chat_right");
    refused(&realm, STRANGER, KIND_GUILD, "chat:not_in_guild");
    refused(&realm, STRANGER, KIND_OFFICER, "chat:not_in_guild");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_rank_edit_changes_the_very_next_line() {
    let mut realm = Standalone::start("guild-chat-rank-edit-takes-effect");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    seeded_guild(&realm);

    let (recipients, _) = line_of(&realm, VETERAN, KIND_GUILD, "before");
    assert_eq!(recipients, [LEADER, OFFICER, VETERAN]);

    realm.assert_call(
        "realm_guild_op",
        &[
            &actor(&LEADER.to_string()),
            &edit_rank(RANK_VETERAN, 0x41, "Veteran"),
        ],
    );
    refused(&realm, VETERAN, KIND_GUILD, "chat:no_guild_chat_right");

    realm.assert_call(
        "realm_guild_op",
        &[
            &actor(&LEADER.to_string()),
            &edit_rank(RANK_VETERAN, 0x43, "Veteran"),
        ],
    );
    let (recipients, _) = line_of(&realm, VETERAN, KIND_GUILD, "after");
    assert_eq!(recipients, [LEADER, OFFICER, VETERAN]);
}
