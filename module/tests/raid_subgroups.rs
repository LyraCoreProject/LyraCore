//! The raid leader moves and swaps members between Subgroups through the Realm-core party
//! reducer, on a private Standalone.

mod support;

use lyracore_shared::group::RaidSlot;
use support::{actor, Standalone};

const INVITE: &str = "0";
const ACCEPT: &str = "1";
const RAID_CONVERT: &str = "6";
const CHANGE_SUBGROUP: &str = "9";
const SWAP_SUBGROUP: &str = "10";

fn realm_group_op_args(
    op: &str,
    actor_guid: u64,
    target_guid: u64,
    arg_a: u8,
    arg_c: u64,
) -> Vec<String> {
    vec![
        op.to_string(),
        actor(&actor_guid.to_string()),
        target_guid.to_string(),
        arg_a.to_string(),
        "0".to_string(),
        arg_c.to_string(),
    ]
}

fn call_ok(node: &Standalone, op: &str, actor_guid: u64, target_guid: u64, arg_a: u8, arg_c: u64) {
    let args = realm_group_op_args(op, actor_guid, target_guid, arg_a, arg_c);
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    node.assert_call("realm_group_op", &args);
}

fn call_failure(
    node: &Standalone,
    op: &str,
    actor_guid: u64,
    target_guid: u64,
    arg_a: u8,
    arg_c: u64,
) -> String {
    let args = realm_group_op_args(op, actor_guid, target_guid, arg_a, arg_c);
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    failure_text(node, "realm_group_op", &args)
}

fn failure_text(node: &Standalone, reducer: &str, args: &[&str]) -> String {
    let output = node.call(reducer, args);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "{reducer} unexpectedly committed: {text}"
    );
    text
}

/// Every LIST row one action inserted, decoded.
fn lists_pushed_by(
    node: &Standalone,
    action: impl FnOnce(),
) -> Vec<(u64, lyracore_shared::group::RosterPayload)> {
    let updates = node.capture_updates("SELECT * FROM game_group_event WHERE kind = 1", 1, action);
    updates
        .iter()
        .flat_map(|update| {
            update["game_group_event"]["inserts"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .map(|row| {
            let recipient = row["recipient_guid"].as_u64().unwrap();
            let payload = row["payload"].as_str().unwrap();
            (
                recipient,
                lyracore_shared::group::RosterPayload::decode(payload)
                    .unwrap_or_else(|| panic!("bad LIST {payload}")),
            )
        })
        .collect()
}

fn slot_of(node: &Standalone, guid: u64) -> String {
    node.query_rows(&format!(
        "SELECT raid_slot FROM game_group_member WHERE character_guid = {guid}"
    ))[0]["raid_slot"]
        .clone()
}

fn roster_revision(node: &Standalone) -> u64 {
    node.query_rows("SELECT revision FROM game_group_roster_revision")[0]["revision"]
        .parse()
        .unwrap()
}

fn join(node: &Standalone, leader: u64, guid: u64) {
    let args = realm_group_op_args(INVITE, leader, guid, 0, 0);
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    node.assert_call("realm_group_op", &args);
    let args = realm_group_op_args(ACCEPT, guid, 0, 0, 0);
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    node.assert_call("realm_group_op", &args);
}

/// **AC 1-4, 9**: a 6-member Raid — Subgroup 0 full with 1 through 5, member 6 alone in Subgroup 1.
/// The leader moves a member; a plain member, an out-of-range Subgroup, and a full destination are
/// each refused; only the accepted move advances the Roster Revision.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_leader_moves_a_member_while_a_plain_member_and_bad_input_are_refused() {
    let mut realm = Standalone::start("raid-subgroups-move");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    for guid in 2..=5 {
        join(&realm, 1, guid);
    }
    call_ok(&realm, RAID_CONVERT, 1, 0, 0, 0);
    join(&realm, 1, 6);
    assert_eq!(
        slot_of(&realm, 6),
        "1",
        "Subgroup 0 already holds 5, so member 6 fills Subgroup 1"
    );

    // AC 2: a plain member (6, no Assistant bit, not the leader) cannot move anybody.
    let revision_before = roster_revision(&realm);
    let text = call_failure(&realm, CHANGE_SUBGROUP, 6, 2, 3, 0);
    assert!(text.contains("group:not_leader"), "{text}");
    assert_eq!(slot_of(&realm, 2), "0", "the refused move changes nothing");

    // AC 4: a Raid has only 8 Subgroups, 0 to 7.
    let text = call_failure(&realm, CHANGE_SUBGROUP, 1, 2, 8, 0);
    assert!(text.contains("group:invalid_subgroup"), "{text}");

    // AC 3: Subgroup 0 already holds 5.
    let text = call_failure(&realm, CHANGE_SUBGROUP, 1, 6, 0, 0);
    assert!(text.contains("group:subgroup_full"), "{text}");
    assert_eq!(slot_of(&realm, 6), "1", "the refused move changes nothing");
    assert_eq!(
        roster_revision(&realm),
        revision_before,
        "no refusal above advanced the Roster Revision"
    );

    // AC 1: the leader moves member 2 into Subgroup 1, which has room (member 6 alone). Every
    // member's list shows member 2's flags byte as 1, and the Roster Revision advances.
    let lists = lists_pushed_by(&realm, || call_ok(&realm, CHANGE_SUBGROUP, 1, 2, 1, 0));
    assert_eq!(lists.len(), 6, "every member of the Raid receives the list");
    for (_, roster) in &lists {
        let moved = roster.members.iter().find(|m| m.guid == 2).unwrap();
        assert_eq!(moved.slot, RaidSlot::new(1, false).unwrap());
    }
    assert_eq!(slot_of(&realm, 2), "1");
    assert_eq!(
        roster_revision(&realm),
        revision_before + 1,
        "the accepted move advances the Roster Revision"
    );

    // The same Subgroup succeeds and changes nothing: member 2 already holds Subgroup 1.
    let revision_before = roster_revision(&realm);
    call_ok(&realm, CHANGE_SUBGROUP, 1, 2, 1, 0);
    assert_eq!(slot_of(&realm, 2), "1");
    assert_eq!(
        roster_revision(&realm),
        revision_before,
        "moving into your own Subgroup is a no-op"
    );
}

/// **AC 6, 7, 9**: a 10-member Raid with Subgroup 0 and Subgroup 1 both full. Swapping two members
/// of the two full Subgroups needs no capacity Gate; swapping two members of one Subgroup changes
/// nothing.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_leader_swaps_members_of_two_full_subgroups_and_a_same_subgroup_swap_is_a_no_op() {
    let mut realm = Standalone::start("raid-subgroups-swap");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    for guid in 2..=5 {
        join(&realm, 1, guid);
    }
    call_ok(&realm, RAID_CONVERT, 1, 0, 0, 0);
    for guid in 6..=10 {
        join(&realm, 1, guid);
    }
    for guid in 1..=5 {
        assert_eq!(
            slot_of(&realm, guid),
            "0",
            "Subgroup 0 holds members 1 through 5"
        );
    }
    for guid in 6..=10 {
        assert_eq!(
            slot_of(&realm, guid),
            "1",
            "Subgroup 0 is full, so members 6 through 10 fill Subgroup 1"
        );
    }

    // AC 6: swap a member of the full Subgroup 0 with a member of the full Subgroup 1. One atomic
    // swap can never overfill a Subgroup, so cmangos applies no capacity Gate here.
    let revision_before = roster_revision(&realm);
    let lists = lists_pushed_by(&realm, || call_ok(&realm, SWAP_SUBGROUP, 1, 1, 0, 6));
    assert_eq!(lists.len(), 10, "one list per member, not two moves' worth");
    assert_eq!(slot_of(&realm, 1), "1");
    assert_eq!(slot_of(&realm, 6), "0");
    assert_eq!(
        roster_revision(&realm),
        revision_before + 1,
        "the swap advances the Roster Revision"
    );

    // AC 7: members 2 and 3 both still sit in Subgroup 0 — swapping them changes nothing.
    let revision_before = roster_revision(&realm);
    call_ok(&realm, SWAP_SUBGROUP, 1, 2, 0, 3);
    assert_eq!(slot_of(&realm, 2), "0");
    assert_eq!(slot_of(&realm, 3), "0");
    assert_eq!(
        roster_revision(&realm),
        revision_before,
        "swapping two members of one Subgroup is a no-op"
    );
}

/// **AC 5**: in a Party, both ops refuse and change nothing.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn subgroup_ops_in_a_party_are_refused() {
    let mut party = Standalone::start("raid-subgroups-party-refuses");
    party.publish_module();
    party.assert_call("claim_operator", &[]);
    join(&party, 1, 2);

    let text = call_failure(&party, CHANGE_SUBGROUP, 1, 2, 1, 0);
    assert!(text.contains("group:not_raid"), "{text}");
    let text = call_failure(&party, SWAP_SUBGROUP, 1, 2, 0, 2);
    assert!(text.contains("group:not_raid"), "{text}");
    assert_eq!(slot_of(&party, 2), "0", "a refused op changes nothing");
}
