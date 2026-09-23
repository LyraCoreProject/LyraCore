mod support;

use serde_json::{json, Value};
use support::{actor, Standalone};

const REALM_CHAT_ROWS: &str = "SELECT * FROM game_realm_chat_event";

/// A PARTY request from a Human (race 1) with the given language and message, tagged DND.
fn party_request(language: u32, message: &str) -> String {
    json!({
        "kind": 1,
        "language": language,
        "channel_name": "",
        "target_guid": 0,
        "message": message,
        "speaker": { "race": 1, "chat_tag": 2 },
    })
    .to_string()
}

fn refused(realm: &Standalone, args: &[&str], tag: &str) {
    let result = realm.call("realm_chat", args);
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !result.status.success() && output.contains(tag),
        "realm_chat should refuse with {tag}: {output}"
    );
}

/// The inserted Realm Chat Lines of one captured transaction.
fn inserted_lines(update: &Value) -> Vec<Value> {
    update["game_realm_chat_event"]["inserts"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_party_line_is_one_row_naming_every_member_of_the_realm_core_party() {
    let mut realm = Standalone::start("realm-chat-party");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    realm.assert_call("install_guid_range", &["0"]);
    // Characters 1 and 2 form a party on Realm-core, which holds no Character rows at all.
    realm.assert_call("realm_group_op", &["0", &actor("1"), "2", "0", "0", "0"]);
    realm.assert_call("realm_group_op", &["1", &actor("2"), "0", "0", "0", "0"]);

    let common = party_request(7, "  form up  ");
    let updates = realm.capture_updates(REALM_CHAT_ROWS, 1, || {
        realm.assert_call("realm_chat", &[&actor("1"), &common]);
    });
    let lines = inserted_lines(&updates[0]);
    assert_eq!(lines.len(), 1, "one line is one row: {updates:?}");
    let line = &lines[0];
    assert_eq!(line["kind"], 1);
    assert_eq!(line["speaker_guid"], 1);
    assert_eq!(line["language"], 7, "a Human's Common stays Common");
    assert_eq!(line["chat_tag"], 2, "the Speaker Facts tag is stamped");
    assert_eq!(line["ignorable"], false, "party lines skip ignore lists");
    assert_eq!(line["message"], "form up");
    assert_eq!(line["channel_name"], "");
    let mut recipients: Vec<u64> = line["recipients"]
        .as_array()
        .expect("recipients is an array")
        .iter()
        .map(|guid| guid.as_u64().expect("a guid"))
        .collect();
    recipients.sort_unstable();
    assert_eq!(recipients, [1, 2], "the speaker hears their own line");

    // The shared event GC reaps the line, which also keeps the next capture free of deletes.
    assert!(
        support::poll_until(std::time::Duration::from_secs(20), || realm
            .query_rows(REALM_CHAT_ROWS)
            .is_empty()),
        "the event GC never reaped the Realm Chat Line"
    );

    // Refusals roll back: the only row the next transaction carries is the sentinel line.
    let sentinel = party_request(0, "sentinel");
    let updates = realm.capture_updates(REALM_CHAT_ROWS, 1, || {
        refused(
            &realm,
            &[&actor("3"), &party_request(7, "anyone?")],
            "chat:not_in_group",
        );
        refused(
            &realm,
            &[&actor("1"), &party_request(1, "zug zug")],
            "chat:unknown_language",
        );
        realm.assert_call("realm_chat", &[&actor("1"), &sentinel]);
    });
    let lines = inserted_lines(&updates[0]);
    assert_eq!(lines.len(), 1, "{updates:?}");
    assert_eq!(lines[0]["message"], "sentinel");
    assert_eq!(lines[0]["language"], 0, "Universal passes for party");
}
