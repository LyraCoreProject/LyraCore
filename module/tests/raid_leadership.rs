//! Leadership and Assistants through the Realm-core party reducer, on a private Standalone: passing
//! the lead, succession, promotion, and the invite and kick rights of an Assistant.

mod support;

use lyracore_shared::group::{event_kind, RaidSlot, RosterPayload};
use support::{actor, Standalone};

const INVITE: &str = "0";
const ACCEPT: &str = "1";
const LEAVE: &str = "3";
const UNINVITE: &str = "4";
const LOOT_METHOD: &str = "5";
const RAID_CONVERT: &str = "6";
const SET_LEADER: &str = "7";
const SET_ASSISTANT: &str = "8";

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

/// The whole error text of a refused `realm_group_op`, which carries the Refusal tag.
fn refusal(node: &Standalone, op: &str, actor_guid: u64, target_guid: u64, arg_a: u8) -> String {
    let args = group_op_args(op, actor_guid, target_guid, arg_a);
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    let output = node.call("realm_group_op", &args);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "realm_group_op {op} unexpectedly committed: {text}"
    );
    text
}

fn assert_refused(node: &Standalone, op: &str, actor: u64, target: u64, arg_a: u8, tag: &str) {
    let text = refusal(node, op, actor, target, arg_a);
    assert!(
        text.contains(tag),
        "op {op} by {actor} on {target}: expected {tag}, got {text}"
    );
}

fn join(node: &Standalone, inviter: u64, guid: u64) {
    group_op(node, INVITE, inviter, guid, 0);
    group_op(node, ACCEPT, guid, 0, 0);
}

fn promote(node: &Standalone, leader: u64, guid: u64, assistant: bool) {
    group_op(node, SET_ASSISTANT, leader, guid, u8::from(assistant));
}

fn leader(node: &Standalone) -> u64 {
    node.query_rows("SELECT leader_guid FROM game_group")[0]["leader_guid"]
        .parse()
        .unwrap()
}

fn roster_revision(node: &Standalone) -> u64 {
    node.query_rows("SELECT revision FROM game_group_roster_revision")[0]["revision"]
        .parse()
        .unwrap()
}

fn slot_of(node: &Standalone, guid: u64) -> RaidSlot {
    let byte = node.query_rows(&format!(
        "SELECT raid_slot FROM game_group_member WHERE character_guid = {guid}"
    ))[0]["raid_slot"]
        .parse()
        .unwrap();
    RaidSlot::from_wire(byte).unwrap()
}

fn members(node: &Standalone) -> Vec<u64> {
    let mut guids: Vec<u64> = node
        .query_rows("SELECT character_guid FROM game_group_member")
        .iter()
        .map(|row| row["character_guid"].parse().unwrap())
        .collect();
    guids.sort_unstable();
    guids
}

/// One `game_group_event` row a reducer call inserted.
struct Event {
    id: u64,
    recipient: u64,
    kind: u8,
    other_guid: u64,
    payload: String,
}

impl Event {
    fn list(&self) -> RosterPayload {
        RosterPayload::decode(&self.payload).unwrap_or_else(|| panic!("bad LIST {}", self.payload))
    }
}

/// Every event row the first committed transaction of `action` that inserts one carries.
fn events_pushed_by(node: &Standalone, action: impl FnOnce()) -> Vec<Event> {
    let updates = node.capture_updates("SELECT * FROM game_group_event", 1, action);
    updates
        .iter()
        .flat_map(|update| {
            update["game_group_event"]["inserts"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .map(|row| Event {
            id: row["id"].as_u64().unwrap(),
            recipient: row["recipient_guid"].as_u64().unwrap(),
            kind: u8::try_from(row["kind"].as_u64().unwrap()).unwrap(),
            other_guid: row["other_guid"].as_u64().unwrap(),
            payload: row["payload"].as_str().unwrap().to_string(),
        })
        .collect()
}

/// The kinds `guid` received, in insertion order.
fn kinds_for(events: &[Event], guid: u64) -> Vec<u8> {
    let mut mine: Vec<_> = events.iter().filter(|e| e.recipient == guid).collect();
    mine.sort_unstable_by_key(|e| e.id);
    mine.iter().map(|e| e.kind).collect()
}

fn start(name: &str) -> Standalone {
    let mut realm = Standalone::start(name);
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    realm
}

/// AC 1 and 2: the leader passes the lead to a member. Every member hears the new leader's
/// announcement before the list naming it. Passing the lead to yourself, as a member who does not
/// lead, or to a Character outside the Group changes nothing. In a Raid the new leader keeps its
/// Raid Slot.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn the_leader_passes_the_lead_and_every_member_hears_it_before_the_list() {
    let realm = start("raid-leadership-set-leader");
    join(&realm, 1, 2);
    join(&realm, 1, 3);
    let revision_before = roster_revision(&realm);

    assert_refused(&realm, SET_LEADER, 1, 1, 0, "group:target_is_self");
    assert_refused(&realm, SET_LEADER, 2, 3, 0, "group:not_leader");
    assert_refused(&realm, SET_LEADER, 1, 99, 0, "group:target_not_in_group");
    assert_refused(&realm, SET_LEADER, 99, 2, 0, "group:not_in_group");
    assert_eq!(leader(&realm), 1);
    assert_eq!(roster_revision(&realm), revision_before);

    let events = events_pushed_by(&realm, || group_op(&realm, SET_LEADER, 1, 2, 0));

    assert_eq!(leader(&realm), 2);
    assert_eq!(roster_revision(&realm), revision_before + 1);
    for guid in [1, 2, 3] {
        assert_eq!(
            kinds_for(&events, guid),
            [event_kind::SET_LEADER, event_kind::LIST],
            "member {guid}"
        );
    }
    for event in &events {
        match event.kind {
            event_kind::SET_LEADER => assert_eq!(event.other_guid, 2),
            _ => assert_eq!(event.list().leader, 2),
        }
    }

    // In a Raid, the new leader keeps its Raid Slot: here an Assistant in Subgroup 1, 0x81.
    join(&realm, 2, 4);
    join(&realm, 2, 5);
    group_op(&realm, RAID_CONVERT, 2, 0, 0);
    join(&realm, 2, 6);
    promote(&realm, 2, 6, true);
    assert_eq!(slot_of(&realm, 6), RaidSlot::new(1, true).unwrap());

    group_op(&realm, SET_LEADER, 2, 6, 0);

    assert_eq!(leader(&realm), 6);
    assert_eq!(
        slot_of(&realm, 6).wire(),
        0x81,
        "the new leader keeps its Raid Slot"
    );
}

/// AC 3: when a Raid leader leaves, the first Assistant in join order leads, whoever was promoted
/// first, and the remaining members hear it before their list. Without an Assistant the first
/// member in join order leads.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_leaving_raid_leader_passes_the_lead_to_the_first_assistant_in_join_order() {
    let realm = start("raid-leadership-succession");
    for guid in 2..=4 {
        join(&realm, 1, guid);
    }
    group_op(&realm, RAID_CONVERT, 1, 0, 0);
    promote(&realm, 1, 4, true);
    promote(&realm, 1, 3, true);

    let events = events_pushed_by(&realm, || group_op(&realm, LEAVE, 1, 0, 0));

    assert_eq!(leader(&realm), 3, "3 joined before 4");
    assert_eq!(kinds_for(&events, 1), [event_kind::DESTROYED]);
    for guid in [2, 3, 4] {
        assert_eq!(
            kinds_for(&events, guid),
            [event_kind::SET_LEADER, event_kind::LIST],
            "member {guid}"
        );
    }
    assert!(events
        .iter()
        .filter(|e| e.kind == event_kind::SET_LEADER)
        .all(|e| e.other_guid == 3));

    promote(&realm, 3, 4, false);
    group_op(&realm, LEAVE, 3, 0, 0);
    assert_eq!(
        leader(&realm),
        2,
        "no Assistant is left, so 2 leads by join order"
    );
}

/// AC 4 and 9: the Raid leader promotes and demotes an Assistant. Every list shows the flags byte,
/// and each change advances the Roster Revision. Repeating a promotion changes nothing. A Party
/// refuses both, and only the leader may do either.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn the_raid_leader_promotes_and_demotes_an_assistant() {
    let realm = start("raid-leadership-assistant");
    join(&realm, 1, 2);
    join(&realm, 1, 3);
    assert_refused(&realm, SET_ASSISTANT, 1, 2, 1, "group:not_raid");
    group_op(&realm, RAID_CONVERT, 1, 0, 0);
    assert_refused(&realm, SET_ASSISTANT, 2, 3, 1, "group:not_leader");
    assert_refused(&realm, SET_ASSISTANT, 1, 1, 1, "group:target_is_self");
    assert_refused(&realm, SET_ASSISTANT, 1, 99, 1, "group:target_not_in_group");

    for (assistant, flags) in [(true, 0x80), (false, 0x00)] {
        let revision_before = roster_revision(&realm);
        let events = events_pushed_by(&realm, || promote(&realm, 1, 2, assistant));
        assert_eq!(roster_revision(&realm), revision_before + 1);
        assert_eq!(slot_of(&realm, 2).wire(), flags);
        let mut recipients: Vec<_> = events.iter().map(|e| e.recipient).collect();
        recipients.sort_unstable();
        assert_eq!(recipients, [1, 2, 3], "one list each");
        for event in &events {
            let list = event.list();
            let member = list.members.iter().find(|m| m.guid == 2).unwrap();
            assert_eq!(member.slot.wire(), flags);
        }
    }

    promote(&realm, 1, 2, true);
    let promoted = roster_revision(&realm);
    let events = events_pushed_by(&realm, || {
        promote(&realm, 1, 2, true);
        group_op(&realm, LOOT_METHOD, 1, 0, 0);
    });
    assert_eq!(
        events[0].list().loot_method,
        0,
        "a repeated promotion pushed a list of its own"
    );
    assert_eq!(
        roster_revision(&realm),
        promoted + 1,
        "only the loot change advanced the Roster Revision"
    );
}

/// AC 5 to 8: an Assistant's invite joins the Raid, a demoted Assistant's pending invite no longer
/// stands, and an Assistant removes plain members and Assistants but never the leader. The leader
/// removes a member by guid.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn an_assistant_invites_and_removes_members_but_never_the_leader() {
    let realm = start("raid-leadership-rights");
    for guid in 2..=4 {
        join(&realm, 1, guid);
    }
    group_op(&realm, RAID_CONVERT, 1, 0, 0);
    promote(&realm, 1, 2, true);
    promote(&realm, 1, 3, true);
    assert_refused(&realm, INVITE, 4, 9, 0, "group:not_leader");

    join(&realm, 2, 5);
    assert_eq!(
        members(&realm),
        [1, 2, 3, 4, 5],
        "an Assistant's invite joins"
    );

    group_op(&realm, INVITE, 3, 6, 0);
    promote(&realm, 1, 3, false);
    assert_refused(&realm, ACCEPT, 6, 0, 0, "group:inviter_unavailable");
    assert_eq!(members(&realm), [1, 2, 3, 4, 5]);

    group_op(&realm, UNINVITE, 2, 4, 0);
    promote(&realm, 1, 5, true);
    group_op(&realm, UNINVITE, 2, 5, 0);
    assert_eq!(
        members(&realm),
        [1, 2, 3],
        "an Assistant removes a plain member and another Assistant"
    );
    let revision_before = roster_revision(&realm);
    assert_refused(&realm, UNINVITE, 2, 1, 0, "group:not_leader");
    assert_refused(&realm, UNINVITE, 3, 2, 0, "group:not_leader");
    assert_eq!(leader(&realm), 1);
    assert_eq!(members(&realm), [1, 2, 3]);
    assert_eq!(roster_revision(&realm), revision_before);

    group_op(&realm, UNINVITE, 1, 3, 0);
    assert_eq!(
        members(&realm),
        [1, 2],
        "the leader removes a member by guid"
    );
}
