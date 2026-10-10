//! The Roster Revision Relay against the realm-wide party topology. Each test changes the Fake
//! Realm-core party directly, as a Module rule that no `party::run` drives would, then runs one
//! worker pass and reads both World Shard mirrors.

use super::party_tests::{
    form_split_party, party_topology, party_topology_with, GINGER, TRIN, VIM,
};
use super::*;
use crate::world::party_mirror::RosterRevisionRelay;

/// The split party's id after `form_split_party`.
fn split_party_id(realm: &WorldFake) -> u64 {
    realm
        .group_roster(GINGER)
        .unwrap()
        .expect("the split party exists on Realm-core")
        .group_id
}

/// Add `guid` to `group_id` on Realm-core and advance its Roster Revision, with no party op.
fn join_on_realm(realm: &WorldFake, group_id: u64, guid: u64) {
    let mut party = realm.party.party.lock().unwrap();
    party.members.push((group_id, guid));
    *party.revisions.entry(group_id).or_insert(0) += 1;
}

fn mirror_calls(calls: &ShardCallLog) -> Vec<String> {
    calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, call)| call == "sync_group_mirror")
        .map(|(shard, _)| shard.clone())
        .collect()
}

#[test]
fn a_roster_change_no_party_op_made_reaches_every_shard_mirror() {
    let (realm, world, instances, _calls) = party_topology();
    form_split_party(&world, &instances);
    let group_id = split_party_id(&realm);
    join_on_realm(&realm, group_id, TRIN);

    let relay = RosterRevisionRelay::default();
    relay.mark_dirty(group_id);
    relay.push_dirty(world.as_ref());

    let realm_revision = realm.held_roster_revision(group_id).unwrap();
    for shard in [&world, &instances] {
        let mirror = shard.group_roster_by_id(group_id).unwrap().unwrap();
        assert_eq!(
            mirror.member_guids(),
            [GINGER, VIM, TRIN],
            "{}",
            shard.topology.shard
        );
        assert_eq!(
            shard.held_roster_revision(group_id).unwrap(),
            realm_revision,
            "{} holds Realm-core's Roster Revision",
            shard.topology.shard
        );
    }
}

#[test]
fn a_disband_reaches_every_shard_as_the_tombstone() {
    let (realm, world, instances, _calls) = party_topology();
    form_split_party(&world, &instances);
    let group_id = split_party_id(&realm);
    {
        let mut party = realm.party.party.lock().unwrap();
        party.members.retain(|(group, _)| *group != group_id);
        party.groups.retain(|(group, ..)| *group != group_id);
        *party.revisions.get_mut(&group_id).unwrap() += 1;
    }

    let relay = RosterRevisionRelay::default();
    relay.mark_dirty(group_id);
    relay.push_dirty(world.as_ref());

    let realm_revision = realm.held_roster_revision(group_id).unwrap();
    for shard in [&world, &instances] {
        assert_eq!(
            shard.group_roster_by_id(group_id).unwrap(),
            None,
            "{} forgets the party",
            shard.topology.shard
        );
        assert_eq!(
            shard.held_roster_revision(group_id).unwrap(),
            realm_revision,
            "{} keeps the tombstone's Roster Revision",
            shard.topology.shard
        );
    }
}

#[test]
fn a_shard_already_at_the_revision_gets_no_push() {
    let (realm, world, instances, calls) = party_topology();
    form_split_party(&world, &instances);
    let group_id = split_party_id(&realm);
    join_on_realm(&realm, group_id, TRIN);
    let current = realm.group_roster_by_id(group_id).unwrap().unwrap();
    world.sync_group_mirror(&current).unwrap();
    calls.lock().unwrap().clear();

    let relay = RosterRevisionRelay::default();
    relay.mark_dirty(group_id);
    relay.push_dirty(world.as_ref());

    assert_eq!(mirror_calls(&calls), ["instances"]);
}

#[test]
fn five_revisions_before_the_worker_runs_cause_one_push_per_stale_shard() {
    let (realm, world, instances, calls) = party_topology();
    form_split_party(&world, &instances);
    let group_id = split_party_id(&realm);
    calls.lock().unwrap().clear();

    let relay = RosterRevisionRelay::default();
    for _ in 0..5 {
        *realm
            .party
            .party
            .lock()
            .unwrap()
            .revisions
            .get_mut(&group_id)
            .unwrap() += 1;
        relay.mark_dirty(group_id);
    }
    relay.push_dirty(world.as_ref());

    let mut pushed = mirror_calls(&calls);
    pushed.sort();
    assert_eq!(pushed, ["instances", "world"]);
}

#[test]
fn an_unavailable_shard_does_not_hold_back_the_other_mirror() {
    let (realm, world, instances, _calls) = party_topology_with(Some("instances is down"), None);
    form_split_party(&world, &instances);
    let group_id = split_party_id(&realm);
    join_on_realm(&realm, group_id, TRIN);

    let relay = RosterRevisionRelay::default();
    relay.mark_dirty(group_id);
    relay.push_dirty(world.as_ref());

    assert_eq!(
        world.held_roster_revision(group_id).unwrap(),
        realm.held_roster_revision(group_id).unwrap()
    );
    assert_eq!(instances.group_roster_by_id(group_id).unwrap(), None);
}
