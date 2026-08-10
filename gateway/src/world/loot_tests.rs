//! Realm-wide loot rolls — the routing + relay tests.
//!
//! What EXECUTES here is production `world::loot`, against the same in-memory multi-database
//! topology the group/whisper slices use (`party_tests::party_topology`). What the fakes stand in
//! for is named at each seam: `realm_loot_ops` is exactly the tuple the operator-gated
//! `realm_loot_op` reducer was handed, `pending_rolls` is a world shard's own `game_loot_roll`
//! staging table, `settled_rolls`/`cleared_rolls` are what the corresponding operator reducers were
//! asked to do, and `won_events` stands in for realm-core's `game_group_event` `ROLL_WON` queue.

use super::loot;
use super::party;
use super::party_tests::{form_split_party, party_topology, GINGER, TRIN, VIM};
use super::*;
use lyracore_shared::loot_roll::loot_op;

// ---- `run_vote` (CMSG_LOOT_ROLL routing) ----

/// Unsharded → the shard-local loot path, verbatim: the vote runs on the player's own reducer, and
/// realm-core (there isn't one) never hears about it.
#[test]
fn an_unsharded_store_routes_the_vote_through_the_players_own_reducer() {
    let store = InMemoryStore::default();
    loot::run_vote(
        &store,
        7,
        42,
        500,
        2,
        lyracore_shared::loot_roll::vote_kind::NEED,
    )
    .unwrap();
    assert_eq!(
        store.loot_rolls.lock().unwrap().clone(),
        vec![(500, 2, lyracore_shared::loot_roll::vote_kind::NEED)],
        "unsharded, the vote must land on the SHARD's own `loot_roll` reducer"
    );
    assert!(
        store.realm_loot_ops.lock().unwrap().is_empty(),
        "unsharded, nothing must be routed to a realm-core plane"
    );
}

/// Sharded → the vote is authorized by the guid the GATEWAY authenticated for this socket
/// (`self_guid`), never a value the client's packet could supply — `CMSG_LOOT_ROLL` carries no actor
/// field at all.
#[test]
fn a_sharded_store_routes_the_vote_to_realm_core_with_the_authenticated_guid() {
    let (realm, world, _instances, _calls) = party_topology();
    loot::run_vote(
        world.as_ref(),
        7,
        GINGER,
        500,
        2,
        lyracore_shared::loot_roll::vote_kind::GREED,
    )
    .unwrap();
    assert_eq!(
        realm.realm_loot_ops.lock().unwrap().clone(),
        vec![(
            loot_op::VOTE,
            500,
            2,
            0,
            GINGER,
            lyracore_shared::loot_roll::vote_kind::GREED,
            0,
            vec![]
        )],
        "sharded, the vote must reach REALM-CORE, addressed by the AUTHENTICATED guid"
    );
    assert!(
        world.loot_rolls.lock().unwrap().is_empty(),
        "a multi-database gateway must not run the vote through the shard's own reducer — that is \
         exactly the shard-local behaviour the realm-wide loot relay removes for a promoted roll"
    );
}

// ---- `relay_tick` (promotion + settlement) ----

/// Unsharded → no-op, before even reading a shard's pending rolls: `realm_store()` answers `None`
/// and `relay_tick` returns before `world_stores()` is ever consulted.
#[test]
fn an_unsharded_store_does_nothing_even_with_pending_rolls_on_a_connected_peer() {
    let peer = std::sync::Arc::new(InMemoryStore {
        shard: "peer".into(),
        ..Default::default()
    });
    *peer.pending_rolls.lock().unwrap() = vec![loot::PendingLootRoll {
        roll_id: 1,
        corpse_guid: 500,
        slot: 2,
        item_entry: 1234,
        deadline_micros: 999,
        recipients: vec![GINGER],
    }];
    let store = InMemoryStore {
        peers: std::sync::Mutex::new(vec![peer.clone()]),
        ..Default::default()
    };
    let mut watermark = 0u64;
    loot::relay_tick(&store, &mut watermark);
    assert!(
        peer.cleared_rolls.lock().unwrap().is_empty(),
        "no realm-core ⇒ nothing promoted, ever"
    );
    assert_eq!(
        watermark, 0,
        "the watermark must not advance on an unsharded store"
    );
}

/// A world shard's freshly kill-time-created staging roll is promoted onto realm-core — WITH its
/// original deadline (never restarting the 60s clock) and its full recipient snapshot — and then the
/// staging copy is cleared so it cannot ALSO run its own vote/resolution locally.
#[test]
fn relay_tick_promotes_a_staging_roll_and_clears_it() {
    let (realm, world, instances, _calls) = party_topology();
    *world.pending_rolls.lock().unwrap() = vec![loot::PendingLootRoll {
        roll_id: 77,
        corpse_guid: 500,
        slot: 2,
        item_entry: 1234,
        deadline_micros: 999_999,
        recipients: vec![GINGER, TRIN],
    }];
    let mut watermark = 0u64;
    loot::relay_tick(world.as_ref(), &mut watermark);
    assert_eq!(
        realm.realm_loot_ops.lock().unwrap().clone(),
        vec![(
            loot_op::START,
            500,
            2,
            1234,
            0,
            0,
            999_999,
            vec![GINGER, TRIN]
        )],
        "the promoted roll must carry the ORIGINAL deadline and the FULL recipient snapshot"
    );
    assert_eq!(
        world.cleared_rolls.lock().unwrap().clone(),
        vec![77],
        "the staging copy must be cleared on the shard that created it, by its own roll id"
    );
    assert!(
        instances.cleared_rolls.lock().unwrap().is_empty(),
        "a shard with no pending rolls of its own must not be touched"
    );
}

/// A resolved roll's winner is settled on EVERY connected world shard — the module's own `withheld`
/// guard (not the relay) is what makes the wrong shards' calls harmless, so the relay fans out rather
/// than trying to guess which shard holds the corpse.
#[test]
fn relay_tick_settles_every_connected_shard_for_a_resolved_roll() {
    let (realm, world, instances, _calls) = party_topology();
    *realm.won_events.lock().unwrap() = vec![(500, 2, GINGER)];
    let mut watermark = 0u64;
    loot::relay_tick(world.as_ref(), &mut watermark);
    assert_eq!(
        world.settled_rolls.lock().unwrap().clone(),
        vec![(500, 2, GINGER)]
    );
    assert_eq!(
        instances.settled_rolls.lock().unwrap().clone(),
        vec![(500, 2, GINGER)],
        "settlement must fan out to EVERY connected shard, not just the one that happens to hold \
         the corpse — the relay cannot know which one that is in advance"
    );
    assert_eq!(
        watermark, 1,
        "the watermark must advance past the one event just settled"
    );

    // A second tick with the SAME advanced watermark must not re-settle the same win.
    loot::relay_tick(world.as_ref(), &mut watermark);
    assert_eq!(
        world.settled_rolls.lock().unwrap().len(),
        1,
        "an already-settled win must not be re-applied on the next tick"
    );
}

// ---- `flush_pending_promotions` (the disband race, found in adversarial review) ----

/// **AC: a disband-capable op promotes every connected shard's pending rolls BEFORE it reaches
/// realm-core.** Without this, `remove_member`'s disband branch (on realm-core) cannot see a roll
/// that is still a local staging copy, and the periodic relay's own 200ms cadence is not fast enough
/// to guarantee it lands first — the exact gap "someone gets kicked right after a kill" falls into.
///
/// The pending roll is staged on `world`, but the LEAVE is dispatched through `instances` (Vim's own
/// connection) — proving the flush covers EVERY connected shard, not just the caller's own, which
/// matters because a party's members (and therefore the corpses their kills stage rolls on) can be
/// split across shards.
#[test]
fn a_leave_flushes_every_connected_shards_pending_rolls_before_the_disband_reaches_realm_core() {
    let (realm, world, instances, calls) = party_topology();
    form_split_party(&world, &instances); // Ginger (world) + Vim (instances) — a 2-member party
    *world.pending_rolls.lock().unwrap() = vec![loot::PendingLootRoll {
        roll_id: 99,
        corpse_guid: 500,
        slot: 2,
        item_entry: 1234,
        deadline_micros: 999,
        recipients: vec![GINGER],
    }];

    // Vim leaving a 2-member party disbands it (vanilla: a party of one is no party) — the exact
    // shape `remove_member`'s disband branch, and therefore `force_resolve_rolls_for_disband`, needs.
    party::run(instances.as_ref(), 8, VIM, party::Op::Leave).expect("Vim leaves");

    assert_eq!(
        realm.realm_loot_ops.lock().unwrap().clone(),
        vec![(loot_op::START, 500, 2, 1234, 0, 0, 999, vec![GINGER])],
        "the pending roll on the OTHER shard must be promoted before the disband, not left behind"
    );
    assert_eq!(
        world.cleared_rolls.lock().unwrap().clone(),
        vec![99],
        "the staging copy must be cleared once promoted, same as a periodic relay tick would"
    );

    // ORDERING is the whole point: a promotion that landed AFTER the disband would still be an
    // orphaned roll naming a group that no longer exists — the disband-mid-roll defect this flush
    // exists to close.
    // `rposition` for the disband dispatch: `form_split_party` above already made two of its own
    // `realm_group_op` calls (the invite, the accept) BEFORE the LEAVE under test — the disband is
    // the LAST one in the log, not the first.
    let log = calls.lock().unwrap().clone();
    let flush_at = log
        .iter()
        .position(|(_, c)| c == "realm_loot_op")
        .expect("the flush must have called realm_loot_op");
    let disband_at = log
        .iter()
        .rposition(|(_, c)| c == "realm_group_op")
        .expect("the LEAVE must have called realm_group_op");
    assert!(
        flush_at < disband_at,
        "the flush must land on realm-core BEFORE the disband op does. Calls were {log:?}"
    );
}

/// A plain party op that CANNOT disband anything (Accept, Decline, LootMethod, and the surviving half
/// of an Invite) must not pay for a flush it does not need — only LEAVE/UNINVITE do.
#[test]
fn a_non_disband_capable_op_never_flushes_pending_rolls() {
    let (realm, world, _instances, _calls) = party_topology();
    *world.pending_rolls.lock().unwrap() = vec![loot::PendingLootRoll {
        roll_id: 1,
        corpse_guid: 500,
        slot: 2,
        item_entry: 1234,
        deadline_micros: 999,
        recipients: vec![GINGER],
    }];
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(TRIN)).expect("the invite runs");
    assert!(
        realm.realm_loot_ops.lock().unwrap().is_empty(),
        "an op that cannot shrink a group must not trigger a flush"
    );
    assert!(world.cleared_rolls.lock().unwrap().is_empty());
}
