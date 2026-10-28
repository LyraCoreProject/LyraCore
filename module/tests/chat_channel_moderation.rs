mod support;

use serde_json::{json, Value};
use support::{actor, Standalone};

const NOTICES: &str = "SELECT * FROM game_chat_channel_notice_event";
const LINES: &str = "SELECT * FROM game_realm_chat_event";

const HUMAN: u8 = 1;
const ORC: u8 = 2;

const YOU_JOINED: u64 = 0x02;
const YOU_LEFT: u64 = 0x03;
const OWNER_CHANGED: u64 = 0x08;
const MODE_CHANGE: u64 = 0x0C;
const ANNOUNCEMENTS_OFF: u64 = 0x0E;
const MODERATION_ON: u64 = 0x0F;
const MODERATION_OFF: u64 = 0x10;
const PLAYER_KICKED: u64 = 0x12;
const PLAYER_BANNED: u64 = 0x14;
const PLAYER_UNBANNED: u64 = 0x15;
const INVITE: u64 = 0x18;
const PLAYER_INVITED: u64 = 0x1D;

const JOIN: &str = "0";
const LEAVE: &str = "1";
const OP_SET_OWNER: &str = "3";
const OP_MODERATOR: &str = "4";
const OP_UNMODERATOR: &str = "5";
const OP_MUTE: &str = "6";
const OP_UNMUTE: &str = "7";
const OP_KICK: &str = "8";
const OP_BAN: &str = "9";
const OP_UNBAN: &str = "10";
const OP_INVITE: &str = "11";
const OP_ANNOUNCEMENTS: &str = "12";
const OP_MODERATE: &str = "13";

#[derive(Clone, Copy)]
struct Target<'a> {
    guid: u64,
    name: &'a str,
    race: u8,
}

impl Target<'_> {
    const NONE: Target<'static> = Target {
        guid: 0,
        name: "",
        race: 0,
    };
}

fn target(guid: u64, name: &str, race: u8) -> Target<'_> {
    Target { guid, name, race }
}

fn join_request(channel: &str, password: &str, race: u8) -> String {
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

fn request(channel: &str, target: Target, target_ignores_actor: bool, race: u8) -> String {
    json!({
        "channel_name": channel,
        "password": "",
        "target_guid": target.guid,
        "target_name": target.name,
        "target_race": target.race,
        "target_ignores_actor": target_ignores_actor,
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

fn join(realm: &Standalone, who: &str, channel: &str, race: u8) {
    realm.assert_call(
        "realm_channel_op",
        &[who, JOIN, &join_request(channel, "", race)],
    );
}

fn op(realm: &Standalone, who: &str, code: &str, channel: &str, target: Target, race: u8) {
    realm.assert_call(
        "realm_channel_op",
        &[who, code, &request(channel, target, false, race)],
    );
}

fn invite(
    realm: &Standalone,
    who: &str,
    channel: &str,
    target: Target,
    target_ignores_actor: bool,
    race: u8,
) {
    realm.assert_call(
        "realm_channel_op",
        &[
            who,
            OP_INVITE,
            &request(channel, target, target_ignores_actor, race),
        ],
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
        support::poll_until(std::time::Duration::from_secs(20), || realm
            .query_rows(query)
            .is_empty()),
        "the event GC never reaped {query}"
    );
}

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

fn notice_rows(realm: &Standalone, action: impl FnOnce()) -> Vec<Value> {
    inserted(realm, NOTICES, "game_chat_channel_notice_event", action)
}

fn notices_of(realm: &Standalone, action: impl FnOnce()) -> Vec<(u64, u64, Vec<u64>)> {
    notice_rows(realm, action)
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

fn member_flags(realm: &Standalone, channel: &str, guid: u64) -> String {
    let id = channel_id(realm, channel);
    realm.query_rows(&format!(
        "SELECT member_flags FROM game_chat_channel_member WHERE channel_id = {id} AND character_guid = {guid}"
    ))[0]["member_flags"]
        .clone()
}

fn channel_id(realm: &Standalone, name: &str) -> String {
    realm.query_rows(&format!(
        "SELECT channel_id FROM game_chat_channel WHERE name = '{name}'"
    ))[0]["channel_id"]
        .clone()
}

fn owner_guid(realm: &Standalone, name: &str) -> String {
    realm.query_rows(&format!(
        "SELECT owner_guid FROM game_chat_channel WHERE name = '{name}'"
    ))[0]["owner_guid"]
        .clone()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn moderator_rights_gate_ownership_and_moderator_changes() {
    let realm = start("chat-channel-moderation-rights");
    join(&realm, &actor("1"), "Raiders", HUMAN);
    join(&realm, &actor("2"), "Raiders", HUMAN);
    join(&realm, &actor("3"), "Raiders", HUMAN);
    assert_eq!(
        member_flags(&realm, "Raiders", 1),
        "3",
        "the first joiner owns and moderates"
    );

    refused(
        &realm,
        "realm_channel_op",
        &[
            &actor("3"),
            OP_MUTE,
            &request("Raiders", target(2, "Two", HUMAN), false, HUMAN),
        ],
        "chat:channel:not_moderator",
    );
    assert_eq!(member_flags(&realm, "Raiders", 2), "0");

    let promoted = notice_rows(&realm, || {
        op(
            &realm,
            &actor("1"),
            OP_MODERATOR,
            "Raiders",
            target(2, "Two", HUMAN),
            HUMAN,
        )
    });
    assert_eq!(promoted.len(), 1);
    assert_eq!(promoted[0]["notice"], MODE_CHANGE);
    assert_eq!(promoted[0]["subject_guid"], 2);
    assert_eq!(promoted[0]["old_flags"], 0);
    assert_eq!(promoted[0]["new_flags"], 2);
    assert_eq!(recipients(&promoted[0]), [1, 2, 3]);
    assert_eq!(member_flags(&realm, "Raiders", 2), "2");

    refused(
        &realm,
        "realm_channel_op",
        &[
            &actor("2"),
            OP_KICK,
            &request("Raiders", target(1, "One", HUMAN), false, HUMAN),
        ],
        "chat:channel:not_owner",
    );
    refused(
        &realm,
        "realm_channel_op",
        &[
            &actor("2"),
            OP_UNMODERATOR,
            &request("Raiders", target(1, "One", HUMAN), false, HUMAN),
        ],
        "chat:channel:not_owner",
    );
    assert_eq!(
        member_flags(&realm, "Raiders", 1),
        "3",
        "the owner is untouched"
    );

    op(
        &realm,
        &actor("1"),
        OP_MODERATOR,
        "Raiders",
        target(1, "One", HUMAN),
        HUMAN,
    );
    assert_eq!(member_flags(&realm, "Raiders", 1), "3");

    join(&realm, &actor("1"), "Trade - City", HUMAN);
    refused(
        &realm,
        "realm_channel_op",
        &[
            &actor("1"),
            OP_SET_OWNER,
            &request("Trade - City", target(1, "One", HUMAN), false, HUMAN),
        ],
        "chat:channel:not_owner",
    );
    assert_eq!(owner_guid(&realm, "Trade - City"), "0");

    let handed = notices_of(&realm, || {
        op(
            &realm,
            &actor("1"),
            OP_SET_OWNER,
            "Raiders",
            target(2, "Two", HUMAN),
            HUMAN,
        )
    });
    assert_eq!(
        handed,
        [
            (MODE_CHANGE, 1, vec![1, 2, 3]),
            (MODE_CHANGE, 2, vec![1, 2, 3]),
            (OWNER_CHANGED, 2, vec![1, 2, 3]),
        ]
    );
    assert_eq!(
        member_flags(&realm, "Raiders", 1),
        "2",
        "keeps MODERATOR, loses OWNER"
    );
    assert_eq!(
        member_flags(&realm, "Raiders", 2),
        "3",
        "gains OWNER and MODERATOR"
    );
    assert_eq!(owner_guid(&realm, "Raiders"), "2");

    refused(
        &realm,
        "realm_channel_op",
        &[
            &actor("1"),
            OP_SET_OWNER,
            &request("Raiders", target(3, "Three", HUMAN), false, HUMAN),
        ],
        "chat:channel:not_owner",
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn mute_kick_and_ban_remove_or_silence_members() {
    let realm = start("chat-channel-moderation-removal");
    join(&realm, &actor("1"), "Raiders", HUMAN);
    join(&realm, &actor("2"), "Raiders", HUMAN);
    join(&realm, &actor("3"), "Raiders", HUMAN);
    op(
        &realm,
        &actor("1"),
        OP_MODERATOR,
        "Raiders",
        target(2, "Two", HUMAN),
        HUMAN,
    );

    let muted = notices_of(&realm, || {
        op(
            &realm,
            &actor("2"),
            OP_MUTE,
            "Raiders",
            target(3, "Three", HUMAN),
            HUMAN,
        )
    });
    assert_eq!(muted, [(MODE_CHANGE, 3, vec![1, 2, 3])]);
    assert_eq!(member_flags(&realm, "Raiders", 3), "8");
    refused(
        &realm,
        "realm_chat",
        &[&actor("3"), &line("Raiders", HUMAN, 0, "hush")],
        "chat:channel:muted",
    );
    op(
        &realm,
        &actor("2"),
        OP_UNMUTE,
        "Raiders",
        target(3, "Three", HUMAN),
        HUMAN,
    );
    assert_eq!(member_flags(&realm, "Raiders", 3), "0");
    let spoken = inserted(&realm, LINES, "game_realm_chat_event", || {
        realm.assert_call(
            "realm_chat",
            &[&actor("3"), &line("Raiders", HUMAN, 0, "hi again")],
        )
    });
    assert_eq!(recipients(&spoken[0]), [1, 2, 3]);

    join(&realm, &actor("4"), "Raiders", HUMAN);
    let kicked = notices_of(&realm, || {
        op(
            &realm,
            &actor("2"),
            OP_KICK,
            "Raiders",
            target(4, "Four", HUMAN),
            HUMAN,
        )
    });
    assert_eq!(kicked, [(PLAYER_KICKED, 4, vec![1, 2, 3, 4])]);
    assert!(realm
        .query_rows("SELECT id FROM game_chat_channel_member WHERE character_guid = 4")
        .is_empty());
    join(&realm, &actor("4"), "Raiders", HUMAN);

    let banned = notices_of(&realm, || {
        op(
            &realm,
            &actor("2"),
            OP_BAN,
            "Raiders",
            target(4, "Four", HUMAN),
            HUMAN,
        )
    });
    assert_eq!(banned, [(PLAYER_BANNED, 4, vec![1, 2, 3, 4])]);
    refused(
        &realm,
        "realm_channel_op",
        &[&actor("4"), JOIN, &join_request("Raiders", "", HUMAN)],
        "chat:channel:banned",
    );
    let unbanned = notices_of(&realm, || {
        op(
            &realm,
            &actor("2"),
            OP_UNBAN,
            "Raiders",
            target(4, "Four", HUMAN),
            HUMAN,
        )
    });
    assert_eq!(unbanned, [(PLAYER_UNBANNED, 4, vec![1, 2, 3])]);
    refused(
        &realm,
        "realm_channel_op",
        &[
            &actor("2"),
            OP_UNBAN,
            &request("Raiders", target(4, "Four", HUMAN), false, HUMAN),
        ],
        "chat:channel:player_not_banned",
    );
    join(&realm, &actor("4"), "Raiders", HUMAN);

    let raiders = channel_id(&realm, "Raiders");
    realm.assert_sql(&format!(
        "INSERT INTO game_chat_channel_ban (id, channel_id, character_guid) VALUES (9101, {raiders}, 4)"
    ));
    let second_ban = notices_of(&realm, || {
        op(
            &realm,
            &actor("2"),
            OP_BAN,
            "Raiders",
            target(4, "Four", HUMAN),
            HUMAN,
        )
    });
    assert_eq!(second_ban, [(PLAYER_KICKED, 4, vec![1, 2, 3, 4])]);
    assert_eq!(
        realm
            .query_rows("SELECT * FROM game_chat_channel_ban")
            .len(),
        1,
        "no second ban row"
    );

    let self_banned = notices_of(&realm, || {
        op(
            &realm,
            &actor("1"),
            OP_BAN,
            "Raiders",
            target(1, "One", HUMAN),
            HUMAN,
        )
    });
    assert_eq!(
        self_banned,
        [
            (PLAYER_BANNED, 1, vec![1, 2, 3]),
            (MODE_CHANGE, 2, vec![2, 3]),
            (OWNER_CHANGED, 2, vec![2, 3]),
        ]
    );
    assert_eq!(owner_guid(&realm, "Raiders"), "2");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn kick_or_ban_of_the_owner_sends_owner_changed_only_past_one_remaining_member() {
    let realm = start("chat-channel-moderation-owner-changed");
    join(&realm, &actor("1"), "Pair", HUMAN);
    join(&realm, &actor("2"), "Pair", HUMAN);

    let departed = notices_of(&realm, || {
        op(
            &realm,
            &actor("1"),
            OP_BAN,
            "Pair",
            target(1, "One", HUMAN),
            HUMAN,
        )
    });
    assert_eq!(
        departed,
        [(PLAYER_BANNED, 1, vec![1, 2]), (MODE_CHANGE, 2, vec![2])]
    );
    assert_eq!(owner_guid(&realm, "Pair"), "2");

    join(&realm, &actor("3"), "Pair", HUMAN);
    join(&realm, &actor("4"), "Pair", HUMAN);
    let departed = notices_of(&realm, || {
        op(
            &realm,
            &actor("2"),
            OP_KICK,
            "Pair",
            target(2, "Two", HUMAN),
            HUMAN,
        )
    });
    assert_eq!(
        departed,
        [
            (PLAYER_KICKED, 2, vec![2, 3, 4]),
            (MODE_CHANGE, 3, vec![3, 4]),
            (OWNER_CHANGED, 3, vec![3, 4]),
        ]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn invite_notifies_the_target_and_follows_faction_and_ban_rules() {
    let realm = start("chat-channel-moderation-invite");
    join(&realm, &actor("1"), "Raiders", HUMAN);

    refused(
        &realm,
        "realm_channel_op",
        &[
            &actor("1"),
            OP_INVITE,
            &request("Raiders", target(99, "Thrall", ORC), false, HUMAN),
        ],
        "chat:channel:invite_wrong_faction",
    );

    let invited = notice_rows(&realm, || {
        invite(
            &realm,
            &actor("1"),
            "Raiders",
            target(2, "Two", HUMAN),
            false,
            HUMAN,
        )
    });
    assert_eq!(invited.len(), 2);
    assert_eq!(invited[0]["notice"], INVITE);
    assert_eq!(invited[0]["subject_guid"], 1, "names the inviter");
    assert_eq!(recipients(&invited[0]), [2]);
    assert_eq!(invited[1]["notice"], PLAYER_INVITED);
    assert_eq!(invited[1]["text"], "Two");
    assert_eq!(recipients(&invited[1]), [1]);

    let ignored_invite = notice_rows(&realm, || {
        invite(
            &realm,
            &actor("1"),
            "Raiders",
            target(3, "Three", HUMAN),
            true,
            HUMAN,
        )
    });
    assert_eq!(ignored_invite.len(), 1, "{ignored_invite:?}");
    assert_eq!(ignored_invite[0]["notice"], PLAYER_INVITED);
    assert_eq!(ignored_invite[0]["text"], "Three");

    join(&realm, &actor("2"), "Raiders", HUMAN);
    refused(
        &realm,
        "realm_channel_op",
        &[
            &actor("1"),
            OP_INVITE,
            &request("Raiders", target(2, "Two", HUMAN), false, HUMAN),
        ],
        "chat:channel:player_already_member",
    );

    join(&realm, &actor("3"), "Raiders", HUMAN);
    op(
        &realm,
        &actor("1"),
        OP_BAN,
        "Raiders",
        target(3, "Three", HUMAN),
        HUMAN,
    );
    refused(
        &realm,
        "realm_channel_op",
        &[
            &actor("1"),
            OP_INVITE,
            &request("Raiders", target(3, "Three", HUMAN), false, HUMAN),
        ],
        "chat:channel:player_invite_banned",
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn moderation_and_announcements_toggle_for_moderators_only() {
    let realm = start("chat-channel-moderation-toggles");
    join(&realm, &actor("1"), "Officers", HUMAN);
    join(&realm, &actor("2"), "Officers", HUMAN);

    refused(
        &realm,
        "realm_channel_op",
        &[
            &actor("2"),
            OP_MODERATE,
            &request("Officers", Target::NONE, false, HUMAN),
        ],
        "chat:channel:not_moderator",
    );

    let moderation_on = notices_of(&realm, || {
        op(
            &realm,
            &actor("1"),
            OP_MODERATE,
            "Officers",
            Target::NONE,
            HUMAN,
        )
    });
    assert_eq!(moderation_on, [(MODERATION_ON, 1, vec![1, 2])]);
    refused(
        &realm,
        "realm_chat",
        &[&actor("2"), &line("Officers", HUMAN, 0, "hi")],
        "chat:channel:not_moderator",
    );
    let owner_line = inserted(&realm, LINES, "game_realm_chat_event", || {
        realm.assert_call(
            "realm_chat",
            &[&actor("1"), &line("Officers", HUMAN, 0, "hi")],
        )
    });
    assert_eq!(recipients(&owner_line[0]), [1, 2]);

    let moderation_off = notices_of(&realm, || {
        op(
            &realm,
            &actor("1"),
            OP_MODERATE,
            "Officers",
            Target::NONE,
            HUMAN,
        )
    });
    assert_eq!(moderation_off, [(MODERATION_OFF, 1, vec![1, 2])]);

    let announcements_off = notices_of(&realm, || {
        op(
            &realm,
            &actor("1"),
            OP_ANNOUNCEMENTS,
            "Officers",
            Target::NONE,
            HUMAN,
        )
    });
    assert_eq!(announcements_off, [(ANNOUNCEMENTS_OFF, 1, vec![1, 2])]);

    let joined = notices_of(&realm, || join(&realm, &actor("3"), "Officers", HUMAN));
    assert_eq!(joined, [(YOU_JOINED, 0, vec![3])], "announcements are off");

    let left = notices_of(&realm, || {
        realm.assert_call(
            "realm_channel_op",
            &[&actor("3"), LEAVE, &join_request("Officers", "", HUMAN)],
        )
    });
    assert_eq!(left, [(YOU_LEFT, 0, vec![3])]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn an_unresolved_target_refuses_after_membership_and_rights_checks() {
    let realm = start("chat-channel-moderation-order");
    join(&realm, &actor("1"), "Raiders", HUMAN);
    join(&realm, &actor("2"), "Raiders", HUMAN);

    refused(
        &realm,
        "realm_channel_op",
        &[
            &actor("9"),
            OP_KICK,
            &request("Raiders", Target::NONE, false, HUMAN),
        ],
        "chat:channel:not_member",
    );

    refused(
        &realm,
        "realm_channel_op",
        &[
            &actor("2"),
            OP_MUTE,
            &request("Raiders", Target::NONE, false, HUMAN),
        ],
        "chat:channel:not_moderator",
    );

    for op_code in [OP_SET_OWNER, OP_KICK, OP_UNBAN, OP_INVITE] {
        refused(
            &realm,
            "realm_channel_op",
            &[
                &actor("1"),
                op_code,
                &request("Raiders", Target::NONE, false, HUMAN),
            ],
            "chat:channel:player_not_found",
        );
    }
}
