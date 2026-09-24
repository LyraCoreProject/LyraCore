//! The Instance Removal countdown on a private Standalone: the Group mirror a World Shard receives
//! and the membership cores a single-database realm runs both arm and cancel it, and its expiry
//! moves a Character to its hearthstone home.

mod support;

use support::{actor, Standalone};

// The fixture Characters `debug_stage_instance_removal_fixture` stages. All but ECHO stand in
// Deadmines instance 509295100. DELTA is a GM. ECHO stands outside. FOXTROT has no World Session.
const ALPHA: u64 = 509_295_001;
const BRAVO: u64 = 509_295_002;
const CHARLIE: u64 = 509_295_003;
const DELTA: u64 = 509_295_004;
const ECHO: u64 = 509_295_005;
const FOXTROT: u64 = 509_295_006;
/// Has no Character on this database.
const GOLF: u64 = 509_295_007;
const INSTANCE: &str = "509295100";

const INVITE: &str = "0";
const ACCEPT: &str = "1";
const LEAVE: &str = "3";
const UNINVITE: &str = "4";
const RAID_CONVERT: &str = "6";
const SET_LEADER: &str = "7";
const SET_ASSISTANT: &str = "8";
const CHANGE_SUBGROUP: &str = "9";

fn start(name: &str) -> Standalone {
    let mut node = Standalone::start(name);
    node.publish_module();
    node.assert_call("claim_operator", &[]);
    node
}

/// `(character_guid, instance_id, group_id)` of every running countdown, by guid. Each countdown
/// is first pushed a day ahead, so only `debug_expire_instance_removal` ever fires one.
fn countdowns(node: &Standalone) -> Vec<(u64, String, String)> {
    node.assert_call("debug_hold_instance_removals", &[]);
    let mut rows: Vec<_> = node
        .query_rows("SELECT character_guid, instance_id, group_id FROM game_instance_removal")
        .into_iter()
        .map(|row| {
            (
                row["character_guid"].parse().unwrap(),
                row["instance_id"].clone(),
                row["group_id"].clone(),
            )
        })
        .collect();
    rows.sort_unstable();
    rows
}

fn counting(guids: &[u64], group: &str) -> Vec<(u64, String, String)> {
    guids
        .iter()
        .map(|&guid| (guid, INSTANCE.to_string(), group.to_string()))
        .collect()
}

/// Assert `guid` sits in the open world at the fixture home, Northshire, with no live entity.
fn assert_at_home(node: &Standalone, guid: u64) {
    let rows = node.query_rows(&format!(
        "SELECT map_id, x, y, z, pending_instance_id FROM game_character WHERE guid = {guid}"
    ));
    let row = &rows[0];
    assert_eq!(row["map_id"], "0", "{guid} is on its home map");
    assert_eq!(
        row["pending_instance_id"], "0",
        "{guid} is in the open world"
    );
    let position: Vec<f32> = ["x", "y", "z"]
        .map(|axis| row[axis].parse().unwrap())
        .into();
    assert_eq!(position, [-8949.95, -132.493, 83.5312], "{guid} is at home");
    assert!(
        node.query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE guid = {guid}"
        ))
        .is_empty(),
        "{guid} left the instance"
    );
}

fn partition(group_id: u64, guid: u64, membership_revision: u64) -> serde_json::Value {
    serde_json::json!({
        "character_guid": guid, "group_id": group_id,
        "membership_revision": membership_revision, "member_active": true,
        "map_id": 0, "instance_id": 0, "locator_revision": 0,
        "state": {"unknown": []},
    })
}

/// Push Group 900's roster at `revision`. `members` pairs each guid with its membership revision,
/// in that order.
fn mirror(node: &Standalone, revision: u64, kind: u8, members: &[(u64, u64)], slots: &[u8]) {
    mirror_group(node, 900, revision, kind, members, slots);
}

/// Push `group_id`'s roster at `revision`, as [`mirror`] does for Group 900.
fn mirror_group(
    node: &Standalone,
    group_id: u64,
    revision: u64,
    kind: u8,
    members: &[(u64, u64)],
    slots: &[u8],
) {
    let guids: Vec<u64> = members.iter().map(|(guid, _)| *guid).collect();
    let partitions: Vec<_> = members
        .iter()
        .map(|&(g, r)| partition(group_id, g, r))
        .collect();
    let args = [
        group_id.to_string(),
        guids.first().copied().unwrap_or(ALPHA).to_string(),
        "3".to_string(),
        "2".to_string(),
        "0".to_string(),
        serde_json::to_string(&guids).unwrap(),
        actor("0"),
        serde_json::to_string(&partitions).unwrap(),
        revision.to_string(),
        kind.to_string(),
        serde_json::to_string(slots).unwrap(),
    ];
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    node.assert_call("sync_group_mirror", &args);
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn the_group_mirror_arms_and_cancels_the_countdown_in_roster_revision_order() {
    let node = start("instance-removal-mirror");
    node.assert_call("debug_stage_instance_removal_fixture", &["900"]);

    let all = [(ALPHA, 1), (BRAVO, 2), (CHARLIE, 3), (FOXTROT, 4)];
    mirror(&node, 1, 0, &all, &[0; 4]);
    assert_eq!(
        countdowns(&node),
        [],
        "every member stands in its own instance"
    );

    let charlie_left = [(ALPHA, 1), (BRAVO, 2), (FOXTROT, 4)];
    mirror(&node, 2, 0, &charlie_left, &[0; 3]);
    assert_eq!(countdowns(&node), counting(&[CHARLIE], "900"), "a leaver");

    let charlie_back = [(ALPHA, 1), (BRAVO, 2), (FOXTROT, 4), (CHARLIE, 10)];
    mirror(&node, 3, 0, &charlie_back, &[0; 4]);
    assert_eq!(countdowns(&node), [], "rejoining the Group cancels");

    mirror(&node, 2, 0, &charlie_left, &[0; 3]);
    assert_eq!(countdowns(&node), [], "an older roster arms nothing");

    mirror(&node, 4, 0, &charlie_left, &[0; 3]);
    assert_eq!(countdowns(&node), counting(&[CHARLIE], "900"));
    mirror(&node, 3, 0, &charlie_back, &[0; 4]);
    assert_eq!(
        countdowns(&node),
        counting(&[CHARLIE], "900"),
        "an older roster cancels nothing"
    );

    // A Raid, an Assistant and a Subgroup move change no membership.
    mirror(&node, 5, 1, &charlie_left, &[0x80, 1, 0]);
    assert_eq!(countdowns(&node), counting(&[CHARLIE], "900"));

    // BRAVO logs out first: a Character removed while logged out gets no countdown.
    node.assert_call("debug_logout_character", &[&BRAVO.to_string()]);
    mirror(&node, 6, 1, &[], &[]);
    assert_eq!(
        countdowns(&node),
        counting(&[ALPHA, CHARLIE], "900"),
        "a disband counts down for every former member in the world but the session-less one"
    );

    let updates = node.capture_updates("SELECT * FROM game_teleport_event", 1, || {
        node.assert_call("debug_expire_instance_removal", &[&ALPHA.to_string()]);
    });
    let teleports: Vec<(u64, u64, bool)> = updates
        .iter()
        .flat_map(|update| {
            update["game_teleport_event"]["inserts"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .map(|row| {
            (
                row["mover_guid"].as_u64().unwrap(),
                row["map_id"].as_u64().unwrap(),
                row["cross_map"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        teleports,
        [(ALPHA, 0, true)],
        "the live Character's client is sent home"
    );
    assert_at_home(&node, ALPHA);
    assert_eq!(countdowns(&node), counting(&[CHARLIE], "900"));

    // A GM teleport out of the instance ends the countdown at once, not at the next login.
    node.assert_call(
        "debug_teleport",
        &[
            &CHARLIE.to_string(),
            "0",
            "-8949.95",
            "-132.493",
            "83.5312",
            "0",
        ],
    );
    assert_eq!(countdowns(&node), []);
}

fn group_op(node: &Standalone, op: &str, actor_guid: u64, target_guid: u64, arg_a: u8) {
    let args = [
        op.to_string(),
        actor(&actor_guid.to_string()),
        target_guid.to_string(),
        arg_a.to_string(),
        "0".to_string(),
        "0".to_string(),
    ];
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    node.assert_call("realm_group_op", &args);
}

fn join(node: &Standalone, inviter: u64, guid: u64) {
    group_op(node, INVITE, inviter, guid, 0);
    group_op(node, ACCEPT, guid, 0, 0);
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn the_membership_cores_arm_and_cancel_the_countdown_on_one_database() {
    let node = start("instance-removal-cores");
    for guid in [BRAVO, CHARLIE, DELTA, ECHO] {
        join(&node, ALPHA, guid);
    }
    let group = node.query_rows("SELECT group_id FROM game_group")[0]["group_id"].clone();
    node.assert_call("debug_stage_instance_removal_fixture", &[&group]);

    group_op(&node, RAID_CONVERT, ALPHA, 0, 0);
    group_op(&node, SET_ASSISTANT, ALPHA, BRAVO, 1);
    group_op(&node, CHANGE_SUBGROUP, ALPHA, CHARLIE, 1);
    group_op(&node, SET_LEADER, ALPHA, BRAVO, 0);
    group_op(&node, SET_LEADER, BRAVO, ALPHA, 0);
    assert_eq!(countdowns(&node), [], "roster order changes no membership");

    group_op(&node, LEAVE, CHARLIE, 0, 0);
    assert_eq!(countdowns(&node), counting(&[CHARLIE], &group), "a leaver");
    group_op(&node, UNINVITE, ALPHA, BRAVO, 0);
    assert_eq!(
        countdowns(&node),
        counting(&[BRAVO, CHARLIE], &group),
        "a kicked member"
    );

    join(&node, ALPHA, CHARLIE);
    assert_eq!(
        countdowns(&node),
        counting(&[BRAVO], &group),
        "rejoining cancels"
    );
    join(&node, BRAVO, GOLF);
    assert_eq!(
        countdowns(&node),
        counting(&[BRAVO], &group),
        "joining another Group does not cancel"
    );

    group_op(&node, LEAVE, DELTA, 0, 0);
    group_op(&node, LEAVE, ECHO, 0, 0);
    assert_eq!(
        countdowns(&node),
        counting(&[BRAVO], &group),
        "neither a GM nor a Character outside the instance counts down"
    );

    group_op(&node, LEAVE, CHARLIE, 0, 0);
    assert_eq!(
        countdowns(&node),
        counting(&[ALPHA, BRAVO, CHARLIE], &group),
        "a disband counts down for both members"
    );

    node.assert_call("debug_logout_character", &[&CHARLIE.to_string()]);
    assert_eq!(
        countdowns(&node),
        counting(&[ALPHA, BRAVO, CHARLIE], &group),
        "logging out keeps the countdown"
    );
    node.assert_call("debug_expire_instance_removal", &[&CHARLIE.to_string()]);
    assert_at_home(&node, CHARLIE);

    node.assert_call("debug_expire_instance_removal", &[&ALPHA.to_string()]);
    assert_at_home(&node, ALPHA);
    assert_eq!(countdowns(&node), counting(&[BRAVO], &group));
}

/// The Group that owns the fixture instance now.
fn instance_owner(node: &Standalone) -> String {
    node.query_rows(&format!(
        "SELECT party_id FROM game_instance WHERE instance_id = {INSTANCE}"
    ))[0]["party_id"]
        .clone()
}

/// Assert `guid` still stands in the fixture instance.
fn assert_in_instance(node: &Standalone, guid: u64) {
    let rows = node.query_rows(&format!(
        "SELECT instance_id FROM game_world_entity WHERE guid = {guid}"
    ));
    assert_eq!(
        rows[0]["instance_id"], INSTANCE,
        "{guid} stays in the instance"
    );
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_party_of_two_that_forms_again_inside_the_instance_cancels_both_countdowns() {
    let node = start("instance-removal-regroup-cores");
    join(&node, ALPHA, CHARLIE);
    let first = node.query_rows("SELECT group_id FROM game_group")[0]["group_id"].clone();
    node.assert_call("debug_stage_instance_removal_fixture", &[&first]);

    group_op(&node, LEAVE, CHARLIE, 0, 0);
    assert_eq!(countdowns(&node), counting(&[ALPHA, CHARLIE], &first));

    join(&node, CHARLIE, ALPHA);
    let second = node.query_rows("SELECT group_id FROM game_group")[0]["group_id"].clone();
    assert_ne!(second, first, "the Group formed again is a new Group");
    assert_eq!(countdowns(&node), [], "nobody is sent home");
    assert_eq!(
        instance_owner(&node),
        second,
        "the new Group owns the instance"
    );
    for guid in [ALPHA, CHARLIE] {
        assert_in_instance(&node, guid);
    }
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_party_of_two_that_forms_again_reaches_the_instance_pool_mirror_and_cancels() {
    let node = start("instance-removal-regroup-mirror");
    node.assert_call("debug_stage_instance_removal_fixture", &["900"]);
    mirror(&node, 1, 0, &[(ALPHA, 1), (CHARLIE, 2)], &[0, 0]);
    mirror(&node, 2, 0, &[], &[]);
    assert_eq!(countdowns(&node), counting(&[ALPHA, CHARLIE], "900"));

    mirror_group(&node, 901, 1, 0, &[(CHARLIE, 20), (ALPHA, 21)], &[0, 0]);
    assert_eq!(countdowns(&node), [], "nobody is sent home");
    assert_eq!(instance_owner(&node), "901");
    for guid in [ALPHA, CHARLIE] {
        assert_in_instance(&node, guid);
    }
}

/// An Assistant's kick arms the countdown the same way the leader's does, and a rejoin through
/// that same Assistant's invite cancels it. Both rights ride `manages_raid`, so this pins that the
/// Instance Removal side does not quietly assume "the leader did it".
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn an_assistants_kick_arms_the_countdown_and_the_assistants_invite_cancels_it() {
    let node = start("instance-removal-assistant-rights");
    for guid in [BRAVO, CHARLIE] {
        join(&node, ALPHA, guid);
    }
    let group = node.query_rows("SELECT group_id FROM game_group")[0]["group_id"].clone();
    node.assert_call("debug_stage_instance_removal_fixture", &[&group]);
    group_op(&node, RAID_CONVERT, ALPHA, 0, 0);
    group_op(&node, SET_ASSISTANT, ALPHA, BRAVO, 1);

    group_op(&node, UNINVITE, BRAVO, CHARLIE, 0);
    assert_eq!(
        countdowns(&node),
        counting(&[CHARLIE], &group),
        "an Assistant's kick arms the countdown"
    );

    join(&node, BRAVO, CHARLIE);
    assert_eq!(
        countdowns(&node),
        [],
        "rejoining through the Assistant's invite cancels it"
    );
}
