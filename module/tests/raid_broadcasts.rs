//! Group Broadcasts through the party reducer on a private Standalone: the Ready Check, Target
//! Icons, the minimap ping and `/roll`. Each op writes one `game_group_event` row per recipient and
//! leaves the Roster Revision alone.

mod support;

use lyracore_shared::group::{
    decode_random_roll, decode_target_icons, event_kind, RosterPayload, TargetIcon,
};
use support::{actor, Standalone};

const INVITE: u8 = 0;
const ACCEPT: u8 = 1;
const LEAVE: u8 = 3;
const LOOT_METHOD: u8 = 5;
const RAID_CONVERT: u8 = 6;
const READY_CHECK_START: u8 = 11;
const READY_CHECK_ANSWER: u8 = 12;
const TARGET_ICON: u8 = 13;
const MINIMAP_PING: u8 = 14;
const RANDOM_ROLL: u8 = 15;

/// A creature guid. Target Icons name any unit.
const DEFIAS: u64 = 0xF130_0000_0000_0042;
const MURLOC: u64 = 0xF130_0000_0000_0043;
const OUTSIDER: u64 = 9;

fn group_op_args(op: u8, actor_guid: u64, target_guid: u64, arg_a: u8, arg_c: u64) -> Vec<String> {
    vec![
        op.to_string(),
        actor(&actor_guid.to_string()),
        target_guid.to_string(),
        arg_a.to_string(),
        "0".to_string(),
        arg_c.to_string(),
    ]
}

fn group_op(node: &Standalone, op: u8, actor_guid: u64, target_guid: u64, arg_a: u8, arg_c: u64) {
    let args = group_op_args(op, actor_guid, target_guid, arg_a, arg_c);
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    node.assert_call("realm_group_op", &args);
}

fn refusal(
    node: &Standalone,
    op: u8,
    actor_guid: u64,
    target_guid: u64,
    arg_a: u8,
    arg_c: u64,
) -> String {
    let args = group_op_args(op, actor_guid, target_guid, arg_a, arg_c);
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    let output = node.call("realm_group_op", &args);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success(), "op {op} unexpectedly committed");
    text
}

/// One `game_group_event` row as the Gateway reads it.
#[derive(Debug, PartialEq, Eq)]
struct Event {
    recipient: u64,
    kind: u8,
    other: u64,
    payload: String,
}

/// Every `game_group_event` row one reducer call inserts, by recipient guid, and each recipient's
/// rows in insert order. Only rows newer than every row present before the call match, so the
/// event GC deleting older rows cannot stand in for the call's own transaction.
fn events_of(node: &Standalone, action: impl FnOnce()) -> Vec<Event> {
    let newest = node
        .query_rows("SELECT id FROM game_group_event")
        .iter()
        .map(|row| row["id"].parse::<u64>().unwrap())
        .max()
        .unwrap_or(0);
    let query = format!("SELECT * FROM game_group_event WHERE id > {newest}");
    let updates = node.capture_updates(&query, 1, action);
    let mut rows: Vec<(u64, Event)> = updates
        .iter()
        .flat_map(|update| {
            update["game_group_event"]["inserts"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .map(|row| {
            (
                row["id"].as_u64().unwrap(),
                Event {
                    recipient: row["recipient_guid"].as_u64().unwrap(),
                    kind: u8::try_from(row["kind"].as_u64().unwrap()).unwrap(),
                    other: row["other_guid"].as_u64().unwrap(),
                    payload: row["payload"].as_str().unwrap().to_string(),
                },
            )
        })
        .collect();
    rows.sort_by_key(|(id, _)| *id);
    rows.sort_by_key(|(_, event)| event.recipient);
    rows.into_iter().map(|(_, event)| event).collect()
}

fn event(recipient: u64, kind: u8, other: u64, payload: &str) -> Event {
    Event {
        recipient,
        kind,
        other,
        payload: payload.to_string(),
    }
}

/// The same row for each recipient.
fn to_each(recipients: &[u64], kind: u8, other: u64, payload: &str) -> Vec<Event> {
    recipients
        .iter()
        .map(|&recipient| event(recipient, kind, other, payload))
        .collect()
}

fn roster_revision(node: &Standalone) -> String {
    node.query_rows("SELECT revision FROM game_group_roster_revision")[0]["revision"].clone()
}

fn target_icons(node: &Standalone) -> Vec<TargetIcon> {
    let mut icons: Vec<TargetIcon> = node
        .query_rows("SELECT icon, target_guid FROM game_group_target_icon")
        .iter()
        .map(|row| TargetIcon {
            icon: row["icon"].parse().unwrap(),
            target_guid: row["target_guid"].parse().unwrap(),
        })
        .collect();
    icons.sort_unstable_by_key(|icon| icon.icon);
    icons
}

fn icon(icon: u8, target_guid: u64) -> TargetIcon {
    TargetIcon { icon, target_guid }
}

/// A Party led by 1 with `members` after it, in join order.
fn party(name: &str, members: &[u64]) -> Standalone {
    let mut node = Standalone::start(name);
    node.publish_module();
    node.assert_call("claim_operator", &[]);
    for &guid in members {
        group_op(&node, INVITE, 1, guid, 0, 0);
        group_op(&node, ACCEPT, guid, 0, 0, 0);
    }
    node
}

/// A Raid of 1, 2, 3 and 4 led by 1, with 3 as its Assistant. The promote op belongs to another
/// change, so the fixture writes the Raid Slot byte `0x80` (Subgroup 0, Assistant) directly.
fn raid(name: &str) -> Standalone {
    let node = party(name, &[2, 3, 4]);
    group_op(&node, RAID_CONVERT, 1, 0, 0, 0);
    node.assert_sql("UPDATE game_group_member SET raid_slot = 128 WHERE character_guid = 3");
    node
}

/// **AC 1, 2 and 10.** The leader or an Assistant asks every member, the asker included
/// (cm:GroupHandler.cpp:549-564). Each answer reaches the leader alone (cm:GroupHandler.cpp:566-581).
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_ready_check_asks_every_member_and_the_answers_reach_the_leader_alone() {
    let realm = raid("raid-broadcasts-ready-check");
    let revision = roster_revision(&realm);

    let text = refusal(&realm, READY_CHECK_START, 2, 0, 0, 0);
    assert!(text.contains("group:not_leader"), "a plain member: {text}");

    let started = events_of(&realm, || group_op(&realm, READY_CHECK_START, 1, 0, 0, 0));
    assert_eq!(
        started,
        to_each(&[1, 2, 3, 4], event_kind::READY_CHECK, 1, "")
    );
    let started = events_of(&realm, || group_op(&realm, READY_CHECK_START, 3, 0, 0, 0));
    assert_eq!(
        started,
        to_each(&[1, 2, 3, 4], event_kind::READY_CHECK, 3, ""),
        "an Assistant may start one"
    );

    let answered = events_of(&realm, || group_op(&realm, READY_CHECK_ANSWER, 2, 0, 1, 0));
    assert_eq!(answered, [event(1, event_kind::READY_CHECK_ANSWER, 2, "1")]);
    let answered = events_of(&realm, || group_op(&realm, READY_CHECK_ANSWER, 4, 0, 0, 0));
    assert_eq!(answered, [event(1, event_kind::READY_CHECK_ANSWER, 4, "0")]);

    let text = refusal(&realm, READY_CHECK_ANSWER, OUTSIDER, 0, 1, 0);
    assert!(text.contains("group:not_in_group"), "{text}");
    assert_eq!(
        roster_revision(&realm),
        revision,
        "a Ready Check changes no roster"
    );
}

/// **AC 3, 4, 5, 7 and 10.** cm:Group.cpp:583-601 and cm:GroupHandler.cpp:444-471.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn target_icons_follow_the_vanilla_placement_and_go_when_the_raid_disbands() {
    let realm = raid("raid-broadcasts-target-icons");
    let revision = roster_revision(&realm);
    let everyone = [1, 2, 3, 4];

    let skull = events_of(&realm, || group_op(&realm, TARGET_ICON, 1, DEFIAS, 7, 0));
    assert_eq!(
        skull,
        to_each(
            &everyone,
            event_kind::TARGET_ICON_UPDATE,
            1,
            &format!("7,{DEFIAS}")
        )
    );
    assert_eq!(target_icons(&realm), [icon(7, DEFIAS)]);

    let cross = events_of(&realm, || group_op(&realm, TARGET_ICON, 1, DEFIAS, 6, 0));
    let expected: Vec<_> = everyone
        .iter()
        .flat_map(|&member| {
            [
                event(member, event_kind::TARGET_ICON_UPDATE, 1, "7,0"),
                event(
                    member,
                    event_kind::TARGET_ICON_UPDATE,
                    1,
                    &format!("6,{DEFIAS}"),
                ),
            ]
        })
        .collect();
    assert_eq!(
        cross, expected,
        "each member sees the unit lose skull before it takes cross"
    );
    assert_eq!(target_icons(&realm), [icon(6, DEFIAS)]);

    let text = refusal(&realm, TARGET_ICON, 2, MURLOC, 0, 0);
    assert!(text.contains("group:not_leader"), "a plain member: {text}");
    let text = refusal(&realm, TARGET_ICON, 1, MURLOC, 8, 0);
    assert!(text.contains("group:invalid_target_icon"), "{text}");
    assert_eq!(target_icons(&realm), [icon(6, DEFIAS)]);

    let star = events_of(&realm, || group_op(&realm, TARGET_ICON, 3, MURLOC, 0, 0));
    assert_eq!(
        star,
        to_each(
            &everyone,
            event_kind::TARGET_ICON_UPDATE,
            3,
            &format!("0,{MURLOC}")
        ),
        "an Assistant may mark"
    );

    let list = events_of(&realm, || group_op(&realm, TARGET_ICON, 4, 0, 0xFF, 0));
    assert_eq!(
        list,
        [event(
            4,
            event_kind::TARGET_ICON_LIST,
            0,
            &format!("0,{MURLOC};6,{DEFIAS}")
        )],
        "the list goes to the member who asked, and to nobody else"
    );
    assert_eq!(
        decode_target_icons(&list[0].payload),
        Some(vec![icon(0, MURLOC), icon(6, DEFIAS)])
    );
    assert_eq!(
        roster_revision(&realm),
        revision,
        "Target Icons change no roster"
    );

    for guid in [4, 3, 2] {
        group_op(&realm, LEAVE, guid, 0, 0, 0);
    }
    assert!(realm.query_rows("SELECT * FROM game_group").is_empty());
    assert!(
        target_icons(&realm).is_empty(),
        "the disbanded Group's icons are gone"
    );
}

/// **AC 6.** A Party client clears its marks on every list, so a Party's LIST carries its icons
/// (vm:Group.cpp:1343-1360). A Raid keeps its marks, and its LIST carries none.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_party_list_carries_its_target_icons_and_a_raid_list_carries_none() {
    let realm = party("raid-broadcasts-party-icons", &[2]);
    group_op(&realm, TARGET_ICON, 1, DEFIAS, 7, 0);

    let lists = events_of(&realm, || group_op(&realm, LOOT_METHOD, 1, 0, 0, 0));
    assert_eq!(lists.len(), 2, "each member gets the list");
    for list in &lists {
        assert_eq!(list.kind, event_kind::LIST);
        let roster = RosterPayload::decode(&list.payload).unwrap();
        assert_eq!(roster.target_icons, [icon(7, DEFIAS)], "{}", list.recipient);
    }

    let lists = events_of(&realm, || group_op(&realm, RAID_CONVERT, 1, 0, 0, 0));
    for list in &lists {
        let roster = RosterPayload::decode(&list.payload).unwrap();
        assert!(roster.target_icons.is_empty(), "{}", list.recipient);
    }
    assert_eq!(
        target_icons(&realm),
        [icon(7, DEFIAS)],
        "the Raid keeps them"
    );
}

/// **AC 8, 9 and 10.** The ping reaches everyone but the sender with the same floats
/// (cm:GroupHandler.cpp:395-415). A grouped roll reaches every member, the roller included; an
/// ungrouped one reaches the roller alone (cm:GroupHandler.cpp:436-441, vm:GroupHandler.cpp:419-431).
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_ping_skips_the_sender_and_a_roll_reaches_the_whole_group() {
    let realm = raid("raid-broadcasts-ping-roll");
    let revision = roster_revision(&realm);
    let (x_bits, y_bits) = (0.5f32.to_bits(), (-0.25f32).to_bits());

    let pinged = events_of(&realm, || {
        group_op(
            &realm,
            MINIMAP_PING,
            2,
            u64::from(x_bits),
            0,
            u64::from(y_bits),
        )
    });
    assert_eq!(
        pinged,
        to_each(
            &[1, 3, 4],
            event_kind::MINIMAP_PING,
            2,
            "1056964608,3196059648"
        )
    );
    let text = refusal(&realm, MINIMAP_PING, OUTSIDER, 0, 0, 0);
    assert!(
        text.contains("group:not_in_group"),
        "an ungrouped ping: {text}"
    );

    for (min, max, (lo, hi)) in [
        (1, 100, (1, 100)),
        (100, 1, (1, 100)),
        (1, 2_000_000, (1, 10_000)),
    ] {
        let rolled = events_of(&realm, || group_op(&realm, RANDOM_ROLL, 2, min, 0, max));
        let recipients: Vec<_> = rolled.iter().map(|event| event.recipient).collect();
        assert_eq!(recipients, [1, 2, 3, 4], "the roller included");
        assert!(rolled
            .iter()
            .all(|event| event.kind == event_kind::RANDOM_ROLL
                && event.other == 2
                && event.payload == rolled[0].payload));
        let (sent_lo, sent_hi, result) = decode_random_roll(&rolled[0].payload).unwrap();
        assert_eq!((sent_lo, sent_hi), (lo, hi), "{min}..{max}");
        assert!((lo..=hi).contains(&result));
    }

    let alone = events_of(&realm, || {
        group_op(&realm, RANDOM_ROLL, OUTSIDER, 1, 0, 100)
    });
    assert_eq!(alone.len(), 1);
    assert_eq!(
        (alone[0].recipient, alone[0].kind, alone[0].other),
        (OUTSIDER, event_kind::RANDOM_ROLL, OUTSIDER)
    );
    assert_eq!(
        roster_revision(&realm),
        revision,
        "a ping or a roll changes no roster"
    );
}
