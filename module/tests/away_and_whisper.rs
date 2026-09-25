mod support;

use serde_json::{json, Value};
use support::{actor, Standalone};

// Chat Kinds, cm:SharedDefines.h:1545-1582.
const WHISPER: u64 = 0x06;
const WHISPER_INFORM: u64 = 0x07;
const AFK: u64 = 0x14;
const DND: u64 = 0x15;
const IGNORED: u64 = 0x16;

const HUMAN: u8 = 1;
const ORC: u8 = 2;
const DWARF: u8 = 3;

fn start(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard
}

fn refused(shard: &Standalone, reducer: &str, args: &[&str], tag: &str) {
    let output = shard.call(reducer, args);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success() && text.contains(tag),
        "{reducer} should refuse with {tag}: {text}"
    );
}

fn player_flags(shard: &Standalone) -> String {
    shard.query_rows("SELECT player_flags FROM game_world_entity WHERE guid = 1")[0]["player_flags"]
        .clone()
}

/// `(kind, message)` of Character 1's Auto-Reply row, or `None` without one.
fn auto_reply(shard: &Standalone) -> Option<(String, String)> {
    shard
        .query_rows("SELECT kind, message FROM game_character_away WHERE character_guid = 1")
        .first()
        .map(|row| (row["kind"].clone(), row["message"].clone()))
}

fn set_away(shard: &Standalone, kind: u64, message: &str) {
    shard.assert_call(
        "gw_set_away",
        &[&actor("1"), &kind.to_string(), &json!(message).to_string()],
    );
}

fn reply(kind: u64, message: &str) -> Option<(String, String)> {
    Some((kind.to_string(), message.to_string()))
}

/// Criteria 1, 2 and 8: `/afk` and `/dnd` on the live entity's PLAYER_FLAGS (AFK 0x02, DND 0x04)
/// with the Auto-Reply beside it, as cm:ChatHandler.cpp:641-693 applies them, and login clears
/// both (cm:Player.cpp:2932).
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn away_status_lives_on_the_entity_and_ends_at_login() {
    let shard = start("away-status");
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    assert_eq!(player_flags(&shard), "0");

    set_away(&shard, AFK, "");
    assert_eq!(player_flags(&shard), "2");
    assert_eq!(auto_reply(&shard), reply(AFK, "Away from Keyboard"));

    set_away(&shard, AFK, "Brb");
    assert_eq!(player_flags(&shard), "2", "only the text changes");
    assert_eq!(auto_reply(&shard), reply(AFK, "Brb"));

    set_away(&shard, DND, "");
    assert_eq!(player_flags(&shard), "4", "DND ends AFK");
    assert_eq!(auto_reply(&shard), reply(DND, "Do not Disturb"));

    set_away(&shard, AFK, "lunch");
    assert_eq!(player_flags(&shard), "2", "AFK ends DND");
    assert_eq!(auto_reply(&shard), reply(AFK, "lunch"));

    set_away(&shard, AFK, "");
    assert_eq!(player_flags(&shard), "0");
    assert_eq!(auto_reply(&shard), None);

    refused(
        &shard,
        "gw_set_away",
        &[&actor("1"), "7", &json!("x").to_string()],
        "neither AFK nor DND",
    );

    set_away(&shard, DND, "busy");
    assert_eq!(player_flags(&shard), "4");
    let key = serde_json::to_string(&vec![7u8; 40]).unwrap();
    shard.assert_call(
        "establish_session",
        &["1", &key, r#"{"__identity__":"0x1"}"#],
    );
    shard.assert_call("gw_heartbeat", &[]);
    shard.assert_call("gw_player_login", &["1", &actor("1")]);
    assert_eq!(player_flags(&shard), "0", "login ends the Away Status");
    assert_eq!(auto_reply(&shard), None, "and deletes the Auto-Reply");
}

fn whisper(target: u64, target_race: u8, away_kind: u64, ignores: bool) -> String {
    json!({
        "language": 7,
        "message": " meet me at the gate ",
        "speaker": { "race": HUMAN, "chat_tag": 2 },
        "target": {
            "guid": target,
            "race": target_race,
            "name": "Vim",
            "ignores_speaker": ignores,
            "away_kind": away_kind,
            "away_message": "",
        },
    })
    .to_string()
}

/// The lines one call inserts that name `a` or `b`, in insert order. The filter keeps the event
/// GC's deletes of earlier scenarios out of the one transaction captured.
fn lines_naming(shard: &Standalone, a: u64, b: u64, action: impl FnOnce()) -> Vec<Value> {
    let query = format!(
        "SELECT * FROM game_realm_chat_event WHERE speaker_guid = {a} OR speaker_guid = {b}"
    );
    let updates = shard.capture_updates(&query, 1, action);
    let mut rows = updates[0]["game_realm_chat_event"]["inserts"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    rows.sort_by_key(|row| row["id"].as_u64());
    rows
}

/// `(kind, speaker_guid, chat_tag, message, recipients)` of each line.
fn plan(rows: &[Value]) -> Vec<(u64, u64, u64, String, Vec<u64>)> {
    rows.iter()
        .map(|row| {
            (
                row["kind"].as_u64().unwrap(),
                row["speaker_guid"].as_u64().unwrap(),
                row["chat_tag"].as_u64().unwrap(),
                row["message"].as_str().unwrap().to_string(),
                row["recipients"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|guid| guid.as_u64().unwrap())
                    .collect(),
            )
        })
        .collect()
}

/// Criteria 4 to 6 on Realm-core: an AFK target's Auto-Reply follows the echo, an ignoring target
/// gets nothing and the speaker learns it, and a cross-faction whisper writes no row.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn realm_whisper_writes_the_vanilla_lines() {
    let realm = start("away-whisper");
    let text = "meet me at the gate".to_string();

    // cm:Player.cpp:16601-16644: the line in Universal with the speaker's tag, the echo, then the
    // default Auto-Reply.
    let afk = lines_naming(&realm, 100, 101, || {
        realm.assert_call(
            "realm_whisper",
            &[&actor("100"), &whisper(101, DWARF, AFK, false)],
        )
    });
    assert_eq!(
        plan(&afk),
        [
            (WHISPER, 100, 2, text.clone(), vec![101]),
            (WHISPER_INFORM, 101, 0, text.clone(), vec![100]),
            (AFK, 101, 0, "Away from Keyboard".to_string(), vec![100]),
        ]
    );
    assert!(afk.iter().all(|row| row["language"] == 0), "{afk:?}");

    // cm:ChatHandler.cpp:801-815: no line for the ignorer; the speaker gets the echo and the
    // target's name on IGNORED.
    let ignored = lines_naming(&realm, 200, 201, || {
        realm.assert_call(
            "realm_whisper",
            &[&actor("200"), &whisper(201, HUMAN, 0, true)],
        )
    });
    assert_eq!(
        plan(&ignored),
        [
            (WHISPER_INFORM, 201, 0, text.clone(), vec![200]),
            (IGNORED, 201, 0, "Vim".to_string(), vec![200]),
        ]
    );

    // cm:ChatHandler.cpp:268-275: a Human whispering an Orc is refused, and the rolled-back
    // transaction wrote nothing.
    refused(
        &realm,
        "realm_whisper",
        &[&actor("300"), &whisper(301, ORC, 0, false)],
        "chat:wrong_faction",
    );
    assert!(realm
        .query_rows(
            "SELECT id FROM game_realm_chat_event WHERE speaker_guid = 300 OR speaker_guid = 301"
        )
        .is_empty());
}

/// Criterion 7: `CMSG_CHAT_IGNORED` becomes IGNORED to the dropped speaker, naming the ignorer.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn an_ignored_notice_reaches_the_dropped_speaker() {
    let realm = start("away-ignored");
    let request = json!({
        "kind": IGNORED,
        "language": 0,
        "channel_name": "",
        "target_guid": 401,
        "message": "Ignorer",
        "speaker": { "race": HUMAN, "chat_tag": 1 },
    })
    .to_string();
    let notice = lines_naming(&realm, 400, 400, || {
        realm.assert_call("realm_chat", &[&actor("400"), &request])
    });
    assert_eq!(
        plan(&notice),
        [(IGNORED, 400, 0, "Ignorer".to_string(), vec![401])]
    );
}
