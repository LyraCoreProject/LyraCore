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
const CHANGE_SUBGROUP: &str = "9";
const READY_CHECK_START: &str = "11";
const TARGET_ICON: &str = "13";

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

/// AC 5 to 8: an Assistant's invite joins the Raid, and an Assistant removes plain members and
/// Assistants but never the leader. The leader removes a member by guid. As in cmangos, an invite
/// joins its Group even after its sender is demoted or leaves, and ends once the Group is gone.
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
    group_op(&realm, ACCEPT, 6, 0, 0);
    assert_eq!(
        members(&realm),
        [1, 2, 3, 4, 5, 6],
        "a demoted Assistant's pending invite still joins its Group"
    );

    group_op(&realm, UNINVITE, 2, 4, 0);
    promote(&realm, 1, 5, true);
    group_op(&realm, UNINVITE, 2, 5, 0);
    assert_eq!(
        members(&realm),
        [1, 2, 3, 6],
        "an Assistant removes a plain member and another Assistant"
    );
    let revision_before = roster_revision(&realm);
    assert_refused(&realm, UNINVITE, 2, 1, 0, "group:not_leader");
    assert_refused(&realm, UNINVITE, 3, 2, 0, "group:not_leader");
    assert_eq!(leader(&realm), 1);
    assert_eq!(members(&realm), [1, 2, 3, 6]);
    assert_eq!(roster_revision(&realm), revision_before);

    group_op(&realm, UNINVITE, 1, 3, 0);
    assert_eq!(
        members(&realm),
        [1, 2, 6],
        "the leader removes a member by guid"
    );

    group_op(&realm, INVITE, 2, 7, 0);
    group_op(&realm, LEAVE, 2, 0, 0);
    group_op(&realm, ACCEPT, 7, 0, 0);
    assert_eq!(
        members(&realm),
        [1, 6, 7],
        "an Assistant's invite joins its Group after the Assistant left"
    );
    assert_eq!(realm.query_rows("SELECT group_id FROM game_group").len(), 1);

    group_op(&realm, INVITE, 1, 8, 0);
    group_op(&realm, LEAVE, 6, 0, 0);
    group_op(&realm, LEAVE, 7, 0, 0);
    assert!(members(&realm).is_empty(), "the Group disbanded");
    assert_refused(&realm, ACCEPT, 8, 0, 0, "group:no_pending_invite");
    assert!(
        realm
            .query_rows("SELECT group_id FROM game_group")
            .is_empty(),
        "an invite whose Group is gone forms no new Party"
    );
}

/// A solo inviter's first accept forms its Group, and its other pending invites join that same
/// Group. A solo inviter who joins another Group first speaks for nobody: its pending invite
/// forms no Party and does not lead into the other Group. Nobody can invite a Character that has
/// invited someone while it has no Group (cm:GroupHandler.cpp:105-114).
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_solo_inviters_pending_invites_follow_the_group_its_first_accept_forms() {
    let realm = start("raid-leadership-solo-inviter");
    group_op(&realm, INVITE, 1, 2, 0);
    group_op(&realm, INVITE, 1, 3, 0);
    group_op(&realm, ACCEPT, 2, 0, 0);
    group_op(&realm, ACCEPT, 3, 0, 0);
    assert_eq!(members(&realm), [1, 2, 3]);
    assert_eq!(realm.query_rows("SELECT group_id FROM game_group").len(), 1);

    group_op(&realm, INVITE, 4, 5, 0);
    assert_refused(&realm, INVITE, 6, 4, 0, "group:already_in_group");

    group_op(&realm, INVITE, 6, 7, 0);
    group_op(&realm, INVITE, 7, 8, 0);
    group_op(&realm, ACCEPT, 7, 0, 0);
    assert_refused(&realm, ACCEPT, 8, 0, 0, "group:inviter_unavailable");
    assert_eq!(
        members(&realm),
        [1, 2, 3, 6, 7],
        "a solo inviter's invite does not lead into the Group it joined since"
    );
}

/// Succession feeds the Group Broadcast gate too: once the sole Assistant inherits the lead, it
/// may start a Ready Check that reaches every remaining member, and the departed leader, no
/// longer in any Group, may not.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn the_assistant_who_inherits_the_lead_can_start_a_ready_check() {
    let realm = start("raid-leadership-succession-ready-check");
    for guid in 2..=4 {
        join(&realm, 1, guid);
    }
    group_op(&realm, RAID_CONVERT, 1, 0, 0);
    promote(&realm, 1, 3, true);

    group_op(&realm, LEAVE, 1, 0, 0);
    assert_eq!(leader(&realm), 3, "the sole Assistant inherits the lead");

    let events = events_pushed_by(&realm, || group_op(&realm, READY_CHECK_START, 3, 0, 0));
    let mut recipients: Vec<_> = events.iter().map(|e| e.recipient).collect();
    recipients.sort_unstable();
    assert_eq!(
        recipients,
        [2, 3, 4],
        "the new leader's Ready Check reaches every remaining member"
    );
    for event in &events {
        assert_eq!(event.kind, event_kind::READY_CHECK);
    }

    assert_refused(&realm, READY_CHECK_START, 1, 0, 0, "group:not_in_group");
}

/// A demoted Assistant loses both rights the flag granted: it may no longer move a member to
/// another Subgroup, nor mark a Target Icon.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_demoted_assistant_can_no_longer_move_a_member_or_mark_a_target_icon() {
    let realm = start("raid-leadership-demoted-assistant-rights");
    for guid in 2..=3 {
        join(&realm, 1, guid);
    }
    group_op(&realm, RAID_CONVERT, 1, 0, 0);
    promote(&realm, 1, 2, true);
    group_op(&realm, CHANGE_SUBGROUP, 2, 3, 1);
    assert_eq!(
        slot_of(&realm, 3).subgroup(),
        1,
        "the Assistant could still move a member"
    );

    promote(&realm, 1, 2, false);

    assert_refused(&realm, CHANGE_SUBGROUP, 2, 3, 0, "group:not_leader");
    assert_eq!(
        slot_of(&realm, 3).subgroup(),
        1,
        "the demoted Assistant's move changed nothing"
    );
    assert_refused(&realm, TARGET_ICON, 2, 900, 7, "group:not_leader");
}
