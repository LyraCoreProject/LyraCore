//! Guild and officer chat through the Realm Chat path (`cm:Guild.cpp:554-594`), on a private
//! Standalone. Guild chat needs GCHATSPEAK to speak and GCHATLISTEN to hear; officer chat needs
//! OFFCHATSPEAK and OFFCHATLISTEN. The audience is read live in the line's own transaction, so a
//! rank edit takes effect on the very next line.

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

/// Found one Guild led by `LEADER` (rank 0) and add `OFFICER` (rank 1) and `VETERAN` (rank 2) at their
/// default rights, plus `MUTED` at a rank edited down to nothing. `STRANGER` never joins. Every
/// default rank keeps GCHATLISTEN|GCHATSPEAK; only rank 0 and 1 also keep OFFCHATLISTEN|OFFCHATSPEAK
/// (`lyracore_shared::guild::DEFAULT_RANKS`).
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

/// A request of `kind` from a Human (race 1) speaking Universal, tagged NONE.
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

/// One accepted `realm_chat` call's recipients (sorted) and `ignorable`. Waits for the event GC to
/// reap the row before returning, so the next capture never races a stray delete transaction — the
/// same discipline `realm_chat.rs`'s durable test follows.
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

/// The whole error text of a refused `realm_chat` call, which carries the Refusal tag.
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

/// **AC 1**: Guild chat reaches every member with GCHATLISTEN, the speaker included; Muted (edited
/// to no rights) is excluded, and the line is ignorable (`cm:Guild.cpp:570`).
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn guild_chat_reaches_every_member_with_gchatlisten() {
    let mut realm = Standalone::start("guild-chat-guild-audience");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    seeded_guild(&realm);

    let (recipients, ignorable) = line_of(&realm, VETERAN, KIND_GUILD, "form up");
    assert_eq!(recipients, [LEADER, OFFICER, VETERAN], "Muted is excluded");
    assert!(ignorable);
}

/// **AC 2**: Officer chat reaches only the ranks that keep OFFCHATLISTEN by default: the leader
/// and the Officer rank. Veteran and Muted lack it (`cm:Guild.cpp:591`).
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn officer_chat_reaches_only_officer_ranked_members() {
    let mut realm = Standalone::start("guild-chat-officer-audience");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    seeded_guild(&realm);

    let (recipients, ignorable) = line_of(&realm, OFFICER, KIND_OFFICER, "raid at 8");
    assert_eq!(recipients, [LEADER, OFFICER]);
    assert!(ignorable);
}

/// **AC 3**: a member whose rank lacks the speak right sends nothing, for both Chat Kinds, and a
/// Character outside any Guild sends nothing either (`cm:ChatHandler.cpp:367-369`,
/// `cm:Guild.cpp:559-561,580-582`).
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
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

/// **AC 5**: a rank edit takes effect on the very next line, because the audience is read fresh in
/// the line's own transaction rather than cached from an earlier read.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_rank_edit_changes_the_very_next_line() {
    let mut realm = Standalone::start("guild-chat-rank-edit-takes-effect");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    seeded_guild(&realm);

    let (recipients, _) = line_of(&realm, VETERAN, KIND_GUILD, "before");
    assert_eq!(recipients, [LEADER, OFFICER, VETERAN]);

    // Strip GCHATSPEAK from the Veteran rank, keeping GCHATLISTEN.
    realm.assert_call(
        "realm_guild_op",
        &[
            &actor(&LEADER.to_string()),
            &edit_rank(RANK_VETERAN, 0x41, "Veteran"),
        ],
    );
    refused(&realm, VETERAN, KIND_GUILD, "chat:no_guild_chat_right");

    // Restore it; the very next line succeeds again, Veteran included in its own audience.
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
