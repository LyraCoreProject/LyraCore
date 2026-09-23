//! A leader converts a Party to a Raid through the Realm-core party reducer, and a World Shard
//! mirror takes the kind and every Raid Slot, on a private Standalone.

mod support;

use lyracore_shared::group::{GroupKind, RaidSlot, RosterPayload};
use support::{actor, Standalone};

const INVITE: &str = "0";
const ACCEPT: &str = "1";
const RAID_CONVERT: &str = "6";

fn group_op_args(op: &str, actor_guid: u64, target_guid: u64) -> Vec<String> {
    vec![
        op.to_string(),
        actor(&actor_guid.to_string()),
        target_guid.to_string(),
        "0".to_string(),
        "0".to_string(),
        "0".to_string(),
    ]
}

fn group_op(node: &Standalone, op: &str, actor_guid: u64, target_guid: u64) {
    let args = group_op_args(op, actor_guid, target_guid);
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    node.assert_call("realm_group_op", &args);
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

/// Every LIST row one reducer call inserted, decoded.
fn lists_pushed_by(node: &Standalone, action: impl FnOnce()) -> Vec<(u64, RosterPayload)> {
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
                RosterPayload::decode(payload).unwrap_or_else(|| panic!("bad LIST {payload}")),
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

fn refused_invite(node: &Standalone, leader: u64, target: u64) -> String {
    let args = group_op_args(INVITE, leader, target);
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    failure_text(node, "realm_group_op", &args)
}

fn join(node: &Standalone, leader: u64, guid: u64) {
    group_op(node, INVITE, leader, guid);
    group_op(node, ACCEPT, guid, 0);
}

fn roster_revision(node: &Standalone) -> u64 {
    node.query_rows("SELECT revision FROM game_group_roster_revision")[0]["revision"]
        .parse()
        .unwrap()
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_converted_raid_takes_members_past_five_into_the_next_subgroup() {
    let mut realm = Standalone::start("raid-convert");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    join(&realm, 1, 2);

    let args = group_op_args(RAID_CONVERT, 2, 0);
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    let text = failure_text(&realm, "realm_group_op", &args);
    assert!(text.contains("group:not_leader"), "{text}");
    assert_eq!(
        realm.query_rows("SELECT group_type FROM game_group")[0]["group_type"],
        "0",
        "a refused convert leaves the Party as it was"
    );

    let revision_before = roster_revision(&realm);
    let converted = lists_pushed_by(&realm, || group_op(&realm, RAID_CONVERT, 1, 0));
    assert_eq!(
        roster_revision(&realm),
        revision_before + 1,
        "the kind is part of the Roster Revision"
    );
    let mut recipients: Vec<_> = converted.iter().map(|(guid, _)| *guid).collect();
    recipients.sort_unstable();
    assert_eq!(recipients, [1, 2], "every member receives the raid list");
    for (_, list) in &converted {
        assert_eq!(list.kind, GroupKind::Raid);
        assert!(list
            .members
            .iter()
            .all(|member| member.slot == RaidSlot::default()));
    }

    // Converting a Raid again writes nothing: the Roster Revision stays, and the first LIST after
    // it comes from the next op that changes the Group, here the loot rules.
    let revision_converted = roster_revision(&realm);
    let lists = lists_pushed_by(&realm, || {
        group_op(&realm, RAID_CONVERT, 1, 0);
        realm.assert_call("realm_group_op", &["5", &actor("1"), "0", "0", "2", "0"]);
    });
    assert_eq!(
        lists[0].1.loot_method, 0,
        "a second convert pushed a LIST of its own"
    );
    assert_eq!(
        roster_revision(&realm),
        revision_converted + 1,
        "only the loot change advanced the Roster Revision"
    );

    for guid in 3..=5 {
        join(&realm, 1, guid);
    }
    group_op(&realm, INVITE, 1, 6);
    let sixth = lists_pushed_by(&realm, || group_op(&realm, ACCEPT, 6, 0));

    assert_eq!(
        realm.query_rows("SELECT group_type FROM game_group")[0]["group_type"],
        "1"
    );
    for guid in 1..=5 {
        assert_eq!(slot_of(&realm, guid), "0", "member {guid} fills Subgroup 0");
    }
    assert_eq!(
        slot_of(&realm, 6),
        "1",
        "Subgroup 0 is full, so the sixth member takes Subgroup 1"
    );
    assert_eq!(sixth.len(), 6, "all six members receive the new list");
    for (_, list) in &sixth {
        assert_eq!(list.kind, GroupKind::Raid);
        let joiner = list.members.iter().find(|member| member.guid == 6).unwrap();
        assert_eq!(joiner.slot, RaidSlot::new(1, false).unwrap());
    }

    for guid in 7..=40 {
        join(&realm, 1, guid);
    }
    let mut subgroup_sizes = [0; 8];
    for row in realm.query_rows("SELECT raid_slot FROM game_group_member") {
        let slot = RaidSlot::from_wire(row["raid_slot"].parse().unwrap()).unwrap();
        subgroup_sizes[usize::from(slot.subgroup())] += 1;
    }
    assert_eq!(
        subgroup_sizes, [5; 8],
        "40 members fill all eight Subgroups"
    );
    let text = refused_invite(&realm, 1, 41);
    assert!(text.contains("group:group_full"), "a Raid of 40: {text}");
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_party_still_caps_at_five() {
    let mut realm = Standalone::start("raid-convert-party-cap");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    for guid in 2..=5 {
        join(&realm, 1, guid);
    }
    let text = refused_invite(&realm, 1, 6);
    assert!(text.contains("group:group_full"), "a Party of 5: {text}");
    assert!(realm
        .query_rows("SELECT raid_slot FROM game_group_member")
        .iter()
        .all(|row| row["raid_slot"] == "0"));
}

fn partition(guid: u64, membership_revision: u64) -> serde_json::Value {
    serde_json::json!({
        "character_guid": guid, "group_id": 900,
        "membership_revision": membership_revision, "member_active": true,
        "map_id": 0, "instance_id": 0, "locator_revision": 0,
        "state": {"unknown": []},
    })
}

fn mirror_args(revision: u64, kind: u8, slots: &[u8]) -> Vec<String> {
    vec![
        "900".to_string(),
        "10".to_string(),
        "3".to_string(),
        "2".to_string(),
        "0".to_string(),
        "[10,11]".to_string(),
        actor("0"),
        serde_json::to_string(&[partition(10, 1), partition(11, 2)]).unwrap(),
        revision.to_string(),
        kind.to_string(),
        serde_json::to_string(slots).unwrap(),
    ]
}

fn mirror_refusal(node: &Standalone, revision: u64, kind: u8, slots: &[u8]) -> String {
    let args = mirror_args(revision, kind, slots);
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    failure_text(node, "sync_group_mirror", &args)
}

fn mirror(node: &Standalone, revision: u64, kind: u8, slots: &[u8]) {
    let args = mirror_args(revision, kind, slots);
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    node.assert_call("sync_group_mirror", &args);
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_world_shard_mirror_takes_the_kind_and_every_raid_slot_or_refuses_the_push() {
    let mut shard = Standalone::start("raid-convert-mirror");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);

    let text = mirror_refusal(&shard, 1, 0, &[0]);
    assert!(text.contains("1 Raid Slots for 2 members"), "{text}");
    let text = mirror_refusal(&shard, 1, 1, &[0, 8]);
    assert!(text.contains("invalid Raid Slot 8"), "{text}");
    let text = mirror_refusal(&shard, 1, 2, &[0, 0]);
    assert!(text.contains("unknown group kind 2"), "{text}");
    assert!(shard
        .query_rows("SELECT * FROM game_group WHERE group_id = 900")
        .is_empty());

    mirror(&shard, 1, 0, &[0, 0]);
    let text = mirror_refusal(&shard, 1, 1, &[0, 0]);
    assert!(
        text.contains("conflicts with the accepted party rules"),
        "a different kind at the accepted Roster Revision: {text}"
    );
    let text = mirror_refusal(&shard, 1, 0, &[0, 1]);
    assert!(
        text.contains("conflicts with the accepted party rules"),
        "a different Raid Slot at the accepted Roster Revision: {text}"
    );

    mirror(&shard, 2, 1, &[0, 0x81]);
    assert_eq!(
        shard.query_rows("SELECT group_type FROM game_group WHERE group_id = 900")[0]["group_type"],
        "1"
    );
    assert_eq!(slot_of(&shard, 10), "0");
    assert_eq!(slot_of(&shard, 11), "129");
}
