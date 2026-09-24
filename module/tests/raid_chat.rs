//! Raid chat, Raid Leader chat and Raid Warning through the Realm Chat path, on a private
//! Standalone. Party chat narrows to the speaker's Subgroup inside a Raid; Raid, Raid Leader and
//! Raid Warning reach the whole Raid, gated by who may send each one.

mod support;

use serde_json::json;
use support::{actor, Standalone};

const REALM_CHAT_ROWS: &str = "SELECT * FROM game_realm_chat_event";

const INVITE: &str = "0";
const ACCEPT: &str = "1";
const RAID_CONVERT: &str = "6";
const SET_ASSISTANT: &str = "8";

const KIND_PARTY: u8 = 1;
const KIND_RAID: u8 = 2;
const KIND_RAID_LEADER: u8 = 87;
const KIND_RAID_WARNING: u8 = 88;

fn group_op_args(op: &str, actor_guid: u64, target_guid: u64, arg_a: u8) -> Vec<String> {
    vec![
        op.to_string(),
        actor(&actor_guid.to_string()),
        target_guid.to_string(),
        arg_a.to_string(),
        "0".to_string(),
        "0".to_string(),
    ]
}

fn group_op(node: &Standalone, op: &str, actor_guid: u64, target_guid: u64, arg_a: u8) {
    let args = group_op_args(op, actor_guid, target_guid, arg_a);
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    node.assert_call("realm_group_op", &args);
}

fn join(node: &Standalone, inviter: u64, guid: u64) {
    group_op(node, INVITE, inviter, guid, 0);
    group_op(node, ACCEPT, guid, 0, 0);
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

/// One accepted `realm_chat` call's recipients, sorted. Waits for the event GC to reap the row
/// before returning, so the next capture never races a stray delete transaction — the same
/// discipline `realm_chat.rs`'s durable test follows.
fn recipients_of(node: &Standalone, actor_guid: u64, kind: u8, message: &str) -> Vec<u64> {
    let req = request(kind, message);
    let updates = node.capture_updates(REALM_CHAT_ROWS, 1, || {
        node.assert_call("realm_chat", &[&actor(&actor_guid.to_string()), &req]);
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
    assert!(
        support::poll_until(std::time::Duration::from_secs(20), || node
            .query_rows(REALM_CHAT_ROWS)
            .is_empty()),
        "the event GC never reaped the Realm Chat Line"
    );
    recipients
}

/// The whole error text of a refused `realm_chat` call, which carries the Refusal tag.
fn refusal(node: &Standalone, actor_guid: u64, kind: u8) -> String {
    let req = request(kind, "anyone?");
    let output = node.call("realm_chat", &[&actor(&actor_guid.to_string()), &req]);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "realm_chat kind {kind} by {actor_guid} unexpectedly committed: {text}"
    );
    text
}

fn assert_refused(node: &Standalone, actor_guid: u64, kind: u8, tag: &str) {
    let text = refusal(node, actor_guid, kind);
    assert!(
        text.contains(tag),
        "kind {kind} by {actor_guid}: expected {tag}, got {text}"
    );
}

/// A 7-member Raid: Subgroup 0 holds the leader and members 2-5, who joined as a Party before the
/// convert and stay in Subgroup 0 (cm:GroupHandler.cpp:473-490). Members 6 and 7 join afterward,
/// find Subgroup 0 full, and fill Subgroup 1 (cm:Group.cpp:817-838).
fn seven_member_raid(node: &Standalone) {
    for guid in 2..=5 {
        join(node, 1, guid);
    }
    group_op(node, RAID_CONVERT, 1, 0, 0);
    for guid in 6..=7 {
        join(node, 1, guid);
    }
}

/// **AC 1**: in a Raid, Party chat reaches only the speaker's Subgroup, the speaker included.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn party_chat_in_a_raid_reaches_only_the_speakers_subgroup() {
    let mut realm = Standalone::start("raid-chat-party-subgroup");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    seven_member_raid(&realm);

    assert_eq!(
        recipients_of(&realm, 3, KIND_PARTY, "form up"),
        [1, 2, 3, 4, 5],
        "Subgroup 0 hears its own member, not Subgroup 1"
    );
    assert_eq!(
        recipients_of(&realm, 6, KIND_PARTY, "form up"),
        [6, 7],
        "Subgroup 1 hears its own member, not Subgroup 0"
    );
}

/// **AC 2**: in a Party (no Raid), Party chat still reaches every member, unchanged.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn party_chat_outside_a_raid_still_reaches_everyone() {
    let mut realm = Standalone::start("raid-chat-party-unaffected");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    join(&realm, 1, 2);
    join(&realm, 1, 3);

    assert_eq!(recipients_of(&realm, 2, KIND_PARTY, "form up"), [1, 2, 3]);
}

/// **AC 3**: `/ra` reaches every Raid member across both Subgroups; refused in a Party.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn raid_chat_reaches_the_whole_raid_and_is_refused_in_a_party() {
    let mut realm = Standalone::start("raid-chat-raid-wide");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    seven_member_raid(&realm);

    assert_eq!(
        recipients_of(&realm, 6, KIND_RAID, "incoming"),
        [1, 2, 3, 4, 5, 6, 7],
        "every Raid member hears it, not only the speaker's Subgroup"
    );

    let mut party = Standalone::start("raid-chat-raid-in-a-party");
    party.publish_module();
    party.assert_call("claim_operator", &[]);
    join(&party, 1, 2);
    assert_refused(&party, 1, KIND_RAID, "chat:not_raid");
}

/// **AC 4**: a Raid Leader line from the leader reaches everyone; from anyone else it is refused.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn raid_leader_chat_is_leader_only() {
    let mut realm = Standalone::start("raid-chat-raid-leader");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    seven_member_raid(&realm);

    assert_eq!(
        recipients_of(&realm, 1, KIND_RAID_LEADER, "stack up"),
        [1, 2, 3, 4, 5, 6, 7]
    );
    assert_refused(&realm, 3, KIND_RAID_LEADER, "chat:not_raid_leader");
}

/// **AC 5**: a Raid Warning from the leader or an Assistant reaches everyone; from a plain member,
/// or in a Party, it is refused.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn raid_warning_needs_the_leader_or_an_assistant() {
    let mut realm = Standalone::start("raid-chat-raid-warning");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    seven_member_raid(&realm);
    group_op(&realm, SET_ASSISTANT, 1, 6, 1);

    assert_eq!(
        recipients_of(&realm, 1, KIND_RAID_WARNING, "boss incoming"),
        [1, 2, 3, 4, 5, 6, 7],
        "the leader may send a Raid Warning"
    );
    assert_eq!(
        recipients_of(&realm, 6, KIND_RAID_WARNING, "boss incoming"),
        [1, 2, 3, 4, 5, 6, 7],
        "an Assistant may send a Raid Warning"
    );
    assert_refused(
        &realm,
        3,
        KIND_RAID_WARNING,
        "chat:not_raid_leader_or_assistant",
    );

    let mut party = Standalone::start("raid-chat-raid-warning-in-a-party");
    party.publish_module();
    party.assert_call("claim_operator", &[]);
    join(&party, 1, 2);
    assert_refused(&party, 1, KIND_RAID_WARNING, "chat:not_raid");
}
