//! Realm-wide loot rolls (issue #50; spec issue #12).
//!
//! # What moved, and why here
//!
//! `game_loot_roll` / `game_loot_roll_vote` are authoritative on realm-core, alongside
//! `game_group` / `game_group_member` (#22) — a roll's audience is who is in the group, not where
//! anyone stands, exactly like the membership it is snapshotted from. Two rejected alternatives are
//! recorded on the issue and not re-litigated here: letting a world shard's MIRROR resolve rolls
//! (makes a cache a decision-maker, the exact property #22 removed) and leaving rolls inert to the
//! 60s deadline (a real regression against single-database behaviour).
//!
//! This module is the ROUTING half, the same shape [`super::party`] and [`super::whisper`] are:
//! generic over [`WorldStore`], so the decisions run under test against the in-memory topology those
//! two already built.
//!
//! # Why this is NOT a client-request routing decision alone
//!
//! Party ops and whispers are always triggered by a client opcode, so the gateway can pick which
//! reducer to call right there. A loot roll's CREATION is not: `apply_group_loot_rules` runs deep
//! inside `combat::kill_creature`'s own transaction, on the corpse's own world shard, with no client
//! request to intercept — the module has no path to another database from there. So kill-time
//! creation ALWAYS writes the roll locally first (`module/src/loot.rs::start_roll`, unchanged), and in
//! a sharded deployment that local row is a TRANSIENT staging copy: [`relay_tick`] is what promotes it
//! onto realm-core and clears the shard's copy, so voting and resolution never run twice for the same
//! roll. The `ROLL_START` popup itself needs no relay at all — it already fired locally, from the same
//! transaction that created the staging row, over the per-player relay every recipient already has
//! (they are, by construction, all on this shard the instant the roll starts — see below).
//!
//! Once promoted, a vote is a client action again (`CMSG_LOOT_ROLL`), so [`run_vote`] routes it the
//! same way `party::run`/`whisper::run` route theirs.
//!
//! # The new case (issue #50)
//!
//! A roll's participants could not previously be on different shards — `kill_reward_recipients` only
//! ever names members physically near the corpse, i.e. on its own shard, at the instant the roll
//! starts. What #50 makes possible is a participant LEAVING that shard mid-roll (a portal, a dungeon
//! entry) during the 60s window. Pre-#50 their vote row was simply unreachable from wherever they
//! went and auto-passed at the deadline, same as being offline. Post-#50 voting is realm-core state
//! reached through whichever shard the player is CURRENTLY connected to, so a mid-roll shard-hopper
//! can still vote from their new location — [`run_vote`] needs no special case for this, it falls out
//! of routing every vote to realm-core rather than to "the shard that created the roll".
//!
//! # What did NOT move
//!
//! `game_corpse_loot` (which corpse, which slot, item quality) and the item GRANT itself stay on the
//! world shard with their existing escrow guarantees (`items::grant_item`) — only the roll, the
//! group-scoped DECISION, travels. A winner decided on realm-core is handed their item by
//! `settle_loot_roll`, an operator reducer [`relay_tick`] calls on every connected world shard after
//! observing the `ROLL_WON` event realm-core pushed; the module's own `withheld` guard
//! (`settle_roll_grant`'s doc) is what makes a call to the WRONG shard harmless, so the relay never
//! needs to know in advance which one holds the corpse.
//!
//! # What did NOT need to change
//!
//! Disband-mid-roll resolution needed no new plumbing at all: `group::remove_member`'s disband branch
//! already calls `loot::force_resolve_rolls_for_disband` unconditionally, in the same transaction,
//! before it tears the group row down — and that code has no `Plane` branch to skip it. Once the roll
//! rows live on realm-core (via promotion), `remove_member` running there (through `realm_group_op`,
//! #22) resolves them on the spot: same database, same transaction, no mirror, no round trip.
//!
//! # The disband race the periodic relay alone cannot close (found in review)
//!
//! `remove_member`'s guarantee above is only as good as what realm-core's `game_loot_roll` actually
//! HOLDS at the moment it runs. A roll younger than one relay tick is still a local staging copy —
//! `remove_member` running on realm-core cannot see it, force-resolves nothing, and the group row is
//! torn down out from under a roll the relay then promotes onto an ORPHANED group id, which resolves
//! only at the 60s deadline: precisely the fallback the operator's decision rejected, reintroduced in
//! a narrow window. "Someone gets kicked right after a kill" is the ordinary shape of this, not a
//! contrived one — shrinking the poll interval shrinks the window, it does not close it.
//!
//! [`flush_pending_promotions`] closes it: `party::run` calls it, synchronously, immediately before
//! dispatching a LEAVE/UNINVITE — the only two ops that can shrink a group below 2 members and reach
//! `remove_member`'s disband branch. By the time `realm_group_op(LEAVE/UNINVITE, ..)` runs right
//! after it returns, every roll that existed anywhere in the realm at that moment has already landed
//! on realm-core, so `remove_member` sees it. This is a per-op, in-line, cross-database call — not a
//! poll — which is the only way to make the guarantee exact rather than merely "usually fast enough".
//!
//! # Why this relay is a POLL, not `on_insert` callbacks
//!
//! Every other cross-database relay in this codebase (`sync_group_mirror`'s consumers, the whisper
//! relay, the realm-core group-event relay) is registered per-PLAYER-SESSION, inside
//! `subscribe_player_events` — its callback's lifetime is tied to one socket, torn down and rebuilt
//! with that socket, and re-subscribes for free on the player's next login. `relay_tick` has no
//! session to hook into: it must run for the GATEWAY PROCESS's lifetime, independent of whether any
//! player is even connected. A callback registered once, at `Coordinator::connect` time, on a world
//! shard's coordinator `DbConnection` would silently stop firing the moment `spawn_coordinator_watchdog`
//! rebuilds that connection after a drop or a migration (`connection.rs`) — the fresh `LiveConn` has
//! no callbacks of its own, and re-registering one from inside the watchdog's generic reconnect path
//! would need Coordinator-level routing context (which world shard promotes to which realm) that the
//! per-shard `connect_blocking` function does not have. A poll reads whatever the CURRENT live
//! connection is on every tick — the same "ask fresh each time" shape every other coordinator
//! consumer already uses to survive a reconnect for free — which is why it is the simplest construct
//! that is actually correct here, not merely the laziest one. The 200ms interval bounds ordinary
//! promotion/settlement latency (a real client cannot render the roll popup and vote within one
//! tick); it plays no role in disband correctness now that [`flush_pending_promotions`] handles that
//! synchronously.

use anyhow::Result;

use super::WorldStore;
use lyracore_shared::loot_roll::loot_op;

/// One unresolved loot roll a world shard has created but not yet had promoted onto realm-core
/// (#50) — [`WorldStore::pending_local_rolls`]'s answer, and [`relay_tick`]'s promotion input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingLootRoll {
    /// This shard's own row id — what [`WorldStore::clear_promoted_loot_roll`] addresses.
    pub roll_id: u64,
    pub corpse_guid: u64,
    pub slot: u8,
    pub item_entry: u32,
    /// Preserved verbatim from the local row so promotion never restarts the 60s clock.
    pub deadline_micros: i64,
    /// Every eligible voter, snapshotted at kill time (`game_loot_roll_vote`'s rows for this roll).
    pub recipients: Vec<u64>,
}

/// Route `CMSG_LOOT_ROLL` for the session that owns `self_guid`.
///
/// Unsharded → the pre-#50 path, verbatim: `loot_roll` on the player's own connection. Sharded →
/// realm-core, where the roll is authoritative once promoted; `actor_guid` is the guid the gateway
/// authenticated for this socket, never the client's own claim (there isn't one — `CMSG_LOOT_ROLL`
/// carries no actor field at all, only the roll's own `(corpse_guid, slot)` and the vote).
pub(crate) fn run_vote<St: WorldStore + ?Sized>(
    store: &St,
    account_id: u64,
    self_guid: u64,
    corpse_guid: u64,
    slot: u32,
    vote: u8,
) -> Result<()> {
    let Some(realm) = store.realm_store() else {
        return store.loot_roll(account_id, self_guid, corpse_guid, slot, vote);
    };
    realm.realm_loot_op(
        loot_op::VOTE,
        corpse_guid,
        slot as u8,
        0,
        self_guid,
        vote,
        0,
        Vec::new(),
    )
}

/// Promote ONE world shard's staging roll onto realm-core, then clear the staging copy — the shared
/// core of [`relay_tick`]'s periodic sweep and [`flush_pending_promotions`]'s synchronous, per-op
/// flush. Best-effort: a failure here is logged and left for the periodic relay to retry, never
/// propagated as an error (the caller's own op — a vote route, a party op — must not fail because a
/// DIFFERENT roll's promotion did).
fn promote_one(shard: &dyn WorldStore, realm: &dyn WorldStore, roll: &PendingLootRoll) {
    match realm.realm_loot_op(
        loot_op::START,
        roll.corpse_guid,
        roll.slot,
        roll.item_entry,
        0,
        0,
        roll.deadline_micros,
        roll.recipients.clone(),
    ) {
        Ok(()) => {
            if let Err(e) = shard.clear_promoted_loot_roll(roll.roll_id) {
                log::warn!(
                    "loot-roll relay: promoted roll {} on {} but could not clear the staging copy \
                     ({e:#}) — retried next tick; the roll is authoritative on realm-core either way",
                    roll.roll_id,
                    shard.shard_name()
                );
            }
        }
        Err(e) => log::warn!(
            "loot-roll relay: could not promote roll {} on {} ({e:#}) — retried next tick",
            roll.roll_id,
            shard.shard_name()
        ),
    }
}

/// Synchronously promote EVERY connected world shard's pending staging rolls onto realm-core (#50).
///
/// Called from `party::run`, immediately before dispatching a LEAVE/UNINVITE — the two ops that can
/// shrink a group below 2 members and reach `remove_member`'s disband branch. This is what closes the
/// disband race the periodic [`relay_tick`] alone cannot (see this module's doc): by the time
/// `realm_group_op(LEAVE/UNINVITE, ..)` runs right after this returns, every roll that existed
/// anywhere in the realm at that moment is already on realm-core for `remove_member` to see.
///
/// Every connected world shard, not just the acting character's own — a party's members, and
/// therefore the corpses their kills stage rolls on, can be split across shards (#22's own premise),
/// so narrowing this to one shard would silently reintroduce the class of bug #22 fixed for invites.
///
/// Best-effort per shard, the same posture `party::sync_mirrors` documents: a shard that cannot be
/// reached leaves its own pending rolls unpromoted for THIS call, and the periodic relay retries them
/// on its own cadence — a flush that failed must not turn a LEAVE/UNINVITE that would otherwise
/// succeed into an error.
pub(crate) fn flush_pending_promotions<St: WorldStore + ?Sized>(
    store: &St,
    realm: &dyn WorldStore,
) {
    for shard in store.world_stores() {
        let pending = match shard.pending_local_rolls() {
            Ok(p) => p,
            Err(e) => {
                log::warn!(
                    "loot-roll flush: could not read {}'s pending rolls before a disband-capable op \
                     ({e:#}) — its rolls stay unpromoted until the next scheduled relay tick",
                    shard.shard_name()
                );
                continue;
            }
        };
        for roll in &pending {
            promote_one(shard.as_ref(), realm, roll);
        }
    }
}

/// One promotion + settlement pass (#50). No-op on an unsharded store (`realm_store()` answers
/// `None`), which is what makes running this on a timer free for a single-database gateway.
///
/// `won_watermark` is the caller's own `game_group_event.id` high-water mark, threaded through
/// call to call so the relay never re-settles an old win after a restart within one process
/// lifetime forgets nothing it already saw — advanced only up to what [`WorldStore::loot_won_since`]
/// actually returns, never optimistically.
///
/// Both directions are BEST-EFFORT, deliberately, the same posture `party::sync_mirrors` documents:
/// realm-core has already committed (a promoted roll exists there, or a roll has already resolved)
/// by the time either loop runs, so a failed relay step must not undo or re-litigate that — it just
/// leaves the affected shard's local state stale until the next tick.
///
/// Ordinary promotion latency (a roll NOT caught by [`flush_pending_promotions`]) is bounded by this
/// function's own caller's poll interval, not by anything in here — see this module's doc for why
/// that caller is a poll rather than a persistent callback.
pub(crate) fn relay_tick<St: WorldStore + ?Sized>(store: &St, won_watermark: &mut u64) {
    let Some(realm) = store.realm_store() else {
        return;
    };
    flush_pending_promotions(store, realm.as_ref());

    // SETTLEMENT: every `ROLL_WON` realm-core has pushed since the last tick, fanned to every
    // connected world shard — `settle_loot_roll`'s own `withheld` guard makes the wrong shards' calls
    // harmless no-ops, so this does not need to know in advance which one holds the corpse.
    match realm.loot_won_since(*won_watermark) {
        Ok((new_watermark, wins)) => {
            for (corpse_guid, slot, winner_guid) in wins {
                for shard in store.world_stores() {
                    if let Err(e) = shard.settle_loot_roll(corpse_guid, slot, winner_guid) {
                        log::warn!(
                            "loot-roll relay: could not settle ({corpse_guid}, {slot}) on {} ({e:#}) \
                             — the winner's item stays on the corpse until the next successful settle",
                            shard.shard_name()
                        );
                    }
                }
            }
            *won_watermark = new_watermark;
        }
        Err(e) => {
            log::warn!("loot-roll relay: could not read realm-core's ROLL_WON events ({e:#})")
        }
    }
}
