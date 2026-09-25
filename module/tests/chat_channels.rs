mod support;

use std::time::Duration;

use serde_json::{json, Value};
use support::{actor, Standalone};

const NOTICES: &str = "SELECT * FROM game_chat_channel_notice_event";
const LINES: &str = "SELECT * FROM game_realm_chat_event";

const HUMAN: u8 = 1;
const ORC: u8 = 2;

// Notice codes, cm:Channel.h:33-69.
const JOINED: u64 = 0x00;
const LEFT: u64 = 0x01;
const YOU_JOINED: u64 = 0x02;
const YOU_LEFT: u64 = 0x03;
const PASSWORD_CHANGED: u64 = 0x07;
const OWNER_CHANGED: u64 = 0x08;
const MODE_CHANGE: u64 = 0x0C;

// realm_channel_op op codes.
const JOIN: &str = "0";
const LEAVE: &str = "1";
const PASSWORD: &str = "2";

fn request(channel: &str, password: &str, race: u8) -> String {
    json!({
        "channel_name": channel,
        "password": password,
        "target_guid": 0,
        "target_name": "",
        "target_race": 0,
        "target_ignores_actor": false,
        "speaker": { "race": race, "chat_tag": 0 },
    })
    .to_string()
}

fn line(channel: &str, race: u8, language: u32, message: &str) -> String {
    json!({
        "kind": 0x0E,
        "language": language,
        "channel_name": channel,
        "target_guid": 0,
        "message": message,
        "speaker": { "race": race, "chat_tag": 0 },
    })
    .to_string()
}

fn start(name: &str) -> Standalone {
    let mut realm = Standalone::start(name);
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    realm.assert_call("install_guid_range", &["0"]);
    realm
}

fn op(realm: &Standalone, who: &str, code: &str, channel: &str, password: &str, race: u8) {
    realm.assert_call(
        "realm_channel_op",
        &[who, code, &request(channel, password, race)],
    );
}

fn refused(realm: &Standalone, reducer: &str, args: &[&str], tag: &str) {
    let result = realm.call(reducer, args);
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !result.status.success() && output.contains(tag),
        "{reducer} should refuse with {tag}: {output}"
    );
}

fn wait_until_empty(realm: &Standalone, query: &str) {
    assert!(
        support::poll_until(Duration::from_secs(20), || realm
            .query_rows(query)
            .is_empty()),
        "the event GC never reaped {query}"
    );
}

/// The rows one action inserts into `table` in insert order, captured from its single transaction.
/// The table is drained first so the capture holds no GC delete.
fn inserted(realm: &Standalone, query: &str, table: &str, action: impl FnOnce()) -> Vec<Value> {
    wait_until_empty(realm, query);
    let updates = realm.capture_updates(query, 1, action);
    let mut rows = updates[0][table]["inserts"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    rows.sort_by_key(|row| row["id"].as_u64());
    rows
}

fn notices_of(realm: &Standalone, action: impl FnOnce()) -> Vec<(u64, u64, Vec<u64>)> {
    inserted(realm, NOTICES, "game_chat_channel_notice_event", action)
        .iter()
        .map(|row| {
            let subject = row["subject_guid"].as_u64().unwrap();
            let actor = row["actor_guid"].as_u64().unwrap();
            (
                row["notice"].as_u64().unwrap(),
                subject.max(actor),
                recipients(row),
            )
        })
        .collect()
}

fn recipients(row: &Value) -> Vec<u64> {
    row["recipients"]
        .as_array()
        .expect("recipients is an array")
        .iter()
        .map(|guid| guid.as_u64().expect("a guid"))
        .collect()
}

fn member_flags(realm: &Standalone, guid: u64) -> String {
    realm.query_rows(&format!(
        "SELECT member_flags FROM game_chat_channel_member WHERE character_guid = {guid}"
    ))[0]["member_flags"]
        .clone()
}

/// The stored spelling of every channel `guid` is a member of, sorted for a stable comparison.
fn member_channel_names(realm: &Standalone, guid: u64) -> Vec<String> {
    let mut names: Vec<String> = realm
        .query_rows(&format!(
            "SELECT channel_id FROM game_chat_channel_member WHERE character_guid = {guid}"
        ))
        .iter()
        .map(|member| {
            let channel_id = &member["channel_id"];
            realm.query_rows(&format!(
                "SELECT name FROM game_chat_channel WHERE channel_id = {channel_id}"
            ))[0]["name"]
                .clone()
        })
        .collect();
    names.sort();
    names
}

/// Criteria 1 to 3: one channel per team and name, lines in the speaker's language, and YOU_JOINED
/// with the wire flags.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn each_team_has_its_own_trade_channel_and_a_line_reaches_every_member() {
    let realm = start("chat-channels-teams");

    let joined = inserted(&realm, NOTICES, "game_chat_channel_notice_event", || {
        op(&realm, &actor("1"), JOIN, "Trade - City", "", HUMAN)
    });
    assert_eq!(
        joined.len(),
        1,
        "a built-in join is YOU_JOINED alone: {joined:?}"
    );
    assert_eq!(joined[0]["notice"], YOU_JOINED);
    assert_eq!(joined[0]["channel_flags"], 0x3C, "Trade");
    assert_eq!(recipients(&joined[0]), [1]);

    // A second Alliance joiner with another spelling shares the channel; nobody sees JOINED.
    let second = notices_of(&realm, || {
        op(&realm, &actor("2"), JOIN, "trade - city", "", HUMAN)
    });
    assert_eq!(second, [(YOU_JOINED, 0, vec![2])]);
    op(&realm, &actor("3"), JOIN, "Trade - City", "", ORC);
    let channels = realm.query_rows("SELECT team, name, flags FROM game_chat_channel");
    assert_eq!(channels.len(), 2, "{channels:?}");
    assert!(channels
        .iter()
        .any(|c| c["team"] == "469" && c["name"] == "Trade - City"));
    assert!(channels.iter().any(|c| c["team"] == "67"));

    let alliance = inserted(&realm, LINES, "game_realm_chat_event", || {
        realm.assert_call(
            "realm_chat",
            &[&actor("2"), &line("TRADE - CITY", HUMAN, 7, "wts linen")],
        )
    });
    assert_eq!(alliance.len(), 1);
    assert_eq!(alliance[0]["kind"], 0x0E);
    assert_eq!(alliance[0]["language"], 7, "the speaker's Common");
    assert_eq!(
        alliance[0]["channel_name"], "Trade - City",
        "the stored spelling"
    );
    let mut heard = recipients(&alliance[0]);
    heard.sort_unstable();
    assert_eq!(heard, [1, 2], "the Horde member hears nothing");

    let horde = inserted(&realm, LINES, "game_realm_chat_event", || {
        realm.assert_call(
            "realm_chat",
            &[&actor("3"), &line("Trade - City", ORC, 1, "wtb")],
        )
    });
    assert_eq!(recipients(&horde[0]), [3]);
    assert_eq!(horde[0]["language"], 1, "Orcish");

    let general = inserted(&realm, NOTICES, "game_chat_channel_notice_event", || {
        op(
            &realm,
            &actor("2"),
            JOIN,
            "General - Elwynn Forest",
            "",
            HUMAN,
        )
    });
    assert_eq!(general.len(), 1);
    assert_eq!(general[0]["channel_flags"], 0x18, "General");

    // A repeat built-in join answers nothing and changes nothing (cm:Channel.cpp:64-72).
    op(
        &realm,
        &actor("2"),
        JOIN,
        "General - Elwynn Forest",
        "",
        HUMAN,
    );
    assert_eq!(
        realm
            .query_rows("SELECT id FROM game_chat_channel_member WHERE character_guid = 2")
            .len(),
        2
    );

    // Built-in channels never announce a departure.
    let left = notices_of(&realm, || {
        op(&realm, &actor("2"), LEAVE, "Trade - City", "", HUMAN)
    });
    assert_eq!(left, [(YOU_LEFT, 0, vec![2])]);
}

/// Criteria 3 to 6: custom channel flags, JOINED, the first owner, succession and the password.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_custom_channel_announces_names_an_owner_and_hands_ownership_on() {
    let realm = start("chat-channels-custom");

    let first = inserted(&realm, NOTICES, "game_chat_channel_notice_event", || {
        op(&realm, &actor("1"), JOIN, "Raiders", "", HUMAN)
    });
    assert_eq!(first.len(), 2, "{first:?}");
    assert_eq!(first[0]["notice"], YOU_JOINED);
    assert_eq!(first[0]["channel_flags"], 0x01, "CUSTOM");
    assert_eq!(first[1]["notice"], MODE_CHANGE);
    assert_eq!(first[1]["subject_guid"], 1);
    assert_eq!(first[1]["old_flags"], 0x00);
    assert_eq!(first[1]["new_flags"], 0x03, "OWNER and MODERATOR");
    assert_eq!(recipients(&first[1]), [1]);
    assert_eq!(member_flags(&realm, 1), "3");

    let second = notices_of(&realm, || {
        op(&realm, &actor("2"), JOIN, "raiders", "", HUMAN)
    });
    assert_eq!(second, [(JOINED, 2, vec![1]), (YOU_JOINED, 0, vec![2])]);
    refused(
        &realm,
        "realm_channel_op",
        &[&actor("2"), JOIN, &request("Raiders", "", HUMAN)],
        "chat:channel:player_already_member",
    );

    // The owner sets a password; a wrong one is refused and the right one admits.
    let changed = notices_of(&realm, || {
        op(&realm, &actor("1"), PASSWORD, "Raiders", "sesame", HUMAN)
    });
    assert_eq!(changed, [(PASSWORD_CHANGED, 1, vec![1, 2])]);
    refused(
        &realm,
        "realm_channel_op",
        &[&actor("2"), PASSWORD, &request("Raiders", "mine", HUMAN)],
        "chat:channel:not_moderator",
    );
    refused(
        &realm,
        "realm_channel_op",
        &[&actor("4"), JOIN, &request("Raiders", "open", HUMAN)],
        "chat:channel:wrong_password",
    );
    op(&realm, &actor("4"), JOIN, "Raiders", "sesame", HUMAN);
    op(&realm, &actor("5"), JOIN, "Raiders", "sesame", HUMAN);

    // Character 5 joined last but moderates, so it succeeds before 2 and 4.
    realm.assert_sql(
        "UPDATE game_chat_channel_member SET member_flags = 2 WHERE character_guid = 5",
    );
    let succession = notices_of(&realm, || {
        op(&realm, &actor("1"), LEAVE, "Raiders", "", HUMAN)
    });
    assert_eq!(
        succession,
        [
            (YOU_LEFT, 0, vec![1]),
            (LEFT, 1, vec![2, 4, 5]),
            (MODE_CHANGE, 5, vec![2, 4, 5]),
            (OWNER_CHANGED, 5, vec![2, 4, 5]),
        ]
    );
    assert_eq!(member_flags(&realm, 5), "3");

    // Without a moderator the earliest joiner succeeds.
    realm.assert_sql(
        "UPDATE game_chat_channel_member SET member_flags = 1 WHERE character_guid = 5",
    );
    let succession = notices_of(&realm, || {
        op(&realm, &actor("5"), LEAVE, "Raiders", "", HUMAN)
    });
    assert_eq!(
        succession,
        [
            (YOU_LEFT, 0, vec![5]),
            (LEFT, 5, vec![2, 4]),
            (MODE_CHANGE, 2, vec![2, 4]),
            (OWNER_CHANGED, 2, vec![2, 4]),
        ]
    );
    assert_eq!(
        realm.query_rows("SELECT owner_guid FROM game_chat_channel")[0]["owner_guid"],
        "2"
    );
}

/// Criterion 7: leaving, NOT_MEMBER, and the last member taking the password and bans with them.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn the_last_member_leaving_deletes_the_channel_its_password_and_its_bans() {
    let realm = start("chat-channels-delete");
    refused(
        &realm,
        "realm_channel_op",
        &[&actor("1"), LEAVE, &request("Raiders", "", HUMAN)],
        "chat:channel:not_member",
    );
    refused(
        &realm,
        "realm_channel_op",
        &[&actor("1"), JOIN, &request("1st", "", HUMAN)],
        "chat:channel:invalid_name",
    );

    op(&realm, &actor("1"), JOIN, "Raiders", "", HUMAN);
    op(&realm, &actor("1"), PASSWORD, "Raiders", "sesame", HUMAN);
    let channel_id =
        realm.query_rows("SELECT channel_id FROM game_chat_channel")[0]["channel_id"].clone();
    realm.assert_sql(&format!(
        "INSERT INTO game_chat_channel_ban (id, channel_id, character_guid) VALUES (9001, {channel_id}, 9)"
    ));
    refused(
        &realm,
        "realm_channel_op",
        &[&actor("9"), JOIN, &request("Raiders", "sesame", HUMAN)],
        "chat:channel:banned",
    );

    let left = notices_of(&realm, || {
        op(&realm, &actor("1"), LEAVE, "Raiders", "", HUMAN)
    });
    assert_eq!(left, [(YOU_LEFT, 0, vec![1])]);
    assert!(realm
        .query_rows("SELECT * FROM game_chat_channel")
        .is_empty());
    assert!(realm
        .query_rows("SELECT * FROM game_chat_channel_ban")
        .is_empty());
    assert!(realm
        .query_rows("SELECT * FROM game_chat_channel_member")
        .is_empty());

    // The name is free again: no password, no ban.
    op(&realm, &actor("9"), JOIN, "Raiders", "", HUMAN);
}

fn token(generation: &str, nonce: &str) -> String {
    format!(r#"{{"account_id":1,"generation":{generation},"request_nonce":{nonce}}}"#)
}

fn claimed_actor(generation: &str, nonce: &str) -> String {
    format!(
        r#"{{"guid":1,"ownership":{{"some":{}}}}}"#,
        token(generation, nonce)
    )
}

fn claim(realm: &Standalone, nonce: &str) -> String {
    realm.assert_call("claim_account", &["1", "1", nonce]);
    realm.query_rows("SELECT generation FROM game_account_claim WHERE account_id = 1")[0]
        ["generation"]
        .clone()
}

/// Criterion 8: releasing, replacing and reaping an Account Claim each leave every channel, with
/// LEFT to the others and no YOU_LEFT.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn channel_membership_ends_with_the_account_claim_that_admitted_it() {
    let realm = start("chat-channels-claims");
    op(&realm, &actor("2"), JOIN, "Raiders", "", HUMAN);

    // Released.
    let generation = claim(&realm, "101");
    let first = claimed_actor(&generation, "101");
    op(&realm, &first, JOIN, "Raiders", "", HUMAN);
    op(&realm, &first, JOIN, "Trade - City", "", HUMAN);
    let member = &realm.query_rows(
        "SELECT account_id, claim_generation FROM game_chat_channel_member WHERE character_guid = 1",
    )[0];
    assert_eq!(member["account_id"], "1");
    assert_eq!(member["claim_generation"], generation);
    let released = notices_of(&realm, || {
        realm.assert_call("release_account_claim", &[&token(&generation, "101")])
    });
    assert_eq!(released, [(LEFT, 1, vec![2])]);
    assert!(realm
        .query_rows("SELECT * FROM game_chat_channel_member WHERE character_guid = 1")
        .is_empty());

    // Replaced: an expired claim the reaper has not closed yet.
    let generation = claim(&realm, "102");
    op(
        &realm,
        &claimed_actor(&generation, "102"),
        JOIN,
        "Raiders",
        "",
        HUMAN,
    );
    realm.assert_sql("UPDATE game_account_claim SET expires_micros = 0");
    let replaced = notices_of(&realm, || {
        realm.assert_call("claim_account", &["1", "1", "103"])
    });
    assert_eq!(replaced, [(LEFT, 1, vec![2])]);

    // Reaped by the Gateway lease schedule.
    let generation = realm
        .query_rows("SELECT generation FROM game_account_claim WHERE account_id = 1")[0]
        ["generation"]
        .clone();
    op(
        &realm,
        &claimed_actor(&generation, "103"),
        JOIN,
        "Raiders",
        "",
        HUMAN,
    );
    realm.assert_sql("UPDATE game_account_claim SET expires_micros = 0");
    realm.assert_call("gw_heartbeat", &[]);
    assert!(
        support::poll_until(Duration::from_secs(30), || realm
            .query_rows("SELECT * FROM game_chat_channel_member WHERE character_guid = 1")
            .is_empty()),
        "the claim reaper never took character 1 out of the channel"
    );
    assert_eq!(
        realm.query_rows("SELECT closed FROM game_account_claim")[0]["closed"],
        "true"
    );
}

/// Criteria 9 and 10: no death check, MUTED and WorldDefense refuse, and only a moderator's line
/// reaches listeners who ignore the speaker.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn channel_speech_follows_the_members_flags_not_the_speakers_health() {
    let realm = start("chat-channels-speech");
    op(&realm, &actor("1"), JOIN, "Raiders", "", HUMAN);
    op(&realm, &actor("2"), JOIN, "Raiders", "", HUMAN);

    // A dead owner still speaks, and a moderator's line ignores ignore lists.
    realm.assert_call("debug_spawn_player_entity", &["1"]);
    realm.assert_sql("UPDATE game_world_entity SET dead = true WHERE guid = 1");
    let owner_line = inserted(&realm, LINES, "game_realm_chat_event", || {
        realm.assert_call(
            "realm_chat",
            &[&actor("1"), &line("Raiders", HUMAN, 7, "from beyond")],
        )
    });
    assert_eq!(recipients(&owner_line[0]), [1, 2]);
    assert_eq!(owner_line[0]["ignorable"], false, "a moderator's line");

    let member_line = inserted(&realm, LINES, "game_realm_chat_event", || {
        realm.assert_call(
            "realm_chat",
            &[&actor("2"), &line("Raiders", HUMAN, 0, "hi")],
        )
    });
    assert_eq!(member_line[0]["ignorable"], true, "a plain member's line");

    realm.assert_sql(
        "UPDATE game_chat_channel_member SET member_flags = 8 WHERE character_guid = 2",
    );
    refused(
        &realm,
        "realm_chat",
        &[&actor("2"), &line("Raiders", HUMAN, 0, "hush")],
        "chat:channel:muted",
    );
    refused(
        &realm,
        "realm_chat",
        &[&actor("3"), &line("Raiders", HUMAN, 0, "hello?")],
        "chat:channel:not_member",
    );

    op(&realm, &actor("3"), JOIN, "WorldDefense", "", HUMAN);
    refused(
        &realm,
        "realm_chat",
        &[&actor("3"), &line("WorldDefense", HUMAN, 0, "inc")],
        "chat:channel:muted",
    );
}

/// A zone walk across a border replays as a LEAVE of the old zone-named channel followed by a JOIN
/// of the new one, wire order from benilla's `plan_walk` (`crates/benilla-app/src/ui_chat/channels.rs`):
/// leave General's old name, join its new name, join Trade (already registered, no leave needed),
/// leave LocalDefense's old name, join its new name, join GuildRecruitment (freshly registered, no
/// leave needed). The Character must hold all four new memberships afterward, the same as a
/// zone-named channel joined with no preceding leave.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_zone_border_crossing_keeps_every_new_zone_channel_membership() {
    let realm = start("chat-channels-zone-walk");

    op(
        &realm,
        &actor("1"),
        JOIN,
        "General - Northshire Abbey",
        "",
        HUMAN,
    );
    op(
        &realm,
        &actor("1"),
        JOIN,
        "LocalDefense - Northshire Abbey",
        "",
        HUMAN,
    );

    op(
        &realm,
        &actor("1"),
        LEAVE,
        "General - Northshire Abbey",
        "",
        HUMAN,
    );
    op(
        &realm,
        &actor("1"),
        JOIN,
        "General - Stormwind City",
        "",
        HUMAN,
    );
    op(&realm, &actor("1"), JOIN, "Trade - City", "", HUMAN);
    op(
        &realm,
        &actor("1"),
        LEAVE,
        "LocalDefense - Northshire Abbey",
        "",
        HUMAN,
    );
    op(
        &realm,
        &actor("1"),
        JOIN,
        "LocalDefense - Stormwind City",
        "",
        HUMAN,
    );
    op(
        &realm,
        &actor("1"),
        JOIN,
        "GuildRecruitment - City",
        "",
        HUMAN,
    );

    assert_eq!(
        member_channel_names(&realm, 1),
        [
            "General - Stormwind City",
            "GuildRecruitment - City",
            "LocalDefense - Stormwind City",
            "Trade - City",
        ],
        "every zone channel joined on the way into Stormwind must survive, whether or not its JOIN \
         was preceded by a LEAVE of the old zone's channel"
    );
}
