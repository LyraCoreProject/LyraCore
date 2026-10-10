//! Realm loot routing. See `docs/realm-loot-routing.md` for the contract and rationale.

use crate::world::ShardRoutingStore;
use anyhow::Result;

use super::{Actor, LootActionStatus, WorldStore};

/// One unresolved loot roll a world shard has created but not yet had promoted onto realm-core —
/// [`LootRollStore::pending_local_rolls`]'s answer, and [`relay_tick`]'s promotion input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingLootRoll {
    /// This shard's own row id — what [`LootRollStore::clear_promoted_loot_roll`] addresses.
    pub roll_id: u64,
    pub corpse_guid: u64,
    pub slot: u8,
    pub item_entry: u32,
    pub random_property_id: u32,
    /// Preserved verbatim from the local row so promotion never restarts the 60s clock.
    pub deadline_micros: i64,
    /// Every eligible voter, snapshotted at kill time (`game_loot_roll_vote`'s rows for this roll).
    pub recipients: Vec<u64>,
    pub promotion_source: spacetimedb_sdk::Identity,
}

/// Durable Reads and Durable Requests for Loot Rolls: the vote, the master looter's assignment, and
/// the relay that promotes each roll onto Realm-core and settles its winner.
pub(crate) trait LootRollStore: Send + Sync {
    /// Promote a world shard's staging roll onto the database THIS handle names. Called on the
    /// **realm-core** handle; the Gateway acts as the session owner, not as a Character.
    #[allow(clippy::too_many_arguments)]
    fn realm_loot_start(
        &self,
        corpse_guid: u64,
        slot: u8,
        item_entry: u32,
        deadline_micros: i64,
        recipients: Vec<u64>,
        random_property_id: u32,
        promotion_source: spacetimedb_sdk::Identity,
        source_roll_id: u64,
    ) -> Result<()>;

    /// Cast one player vote on realm-core. The Store returns a typed gameplay answer while keeping
    /// failures with an unknown durable result as `Err`.
    fn realm_loot_vote(
        &self,
        corpse_guid: u64,
        slot: u8,
        actor: Actor,
        vote: u8,
    ) -> Result<LootActionStatus>;

    /// Every UNRESOLVED loot roll this WORLD SHARD has created but not yet had promoted onto
    /// realm-core — the relay's promotion queue. Empty on realm-core's own handle: nothing is ever
    /// created there directly — only `realm_loot_start` writes it, and that is not this
    /// method.
    fn pending_local_rolls(&self) -> Result<Vec<PendingLootRoll>>;

    /// `settle_loot_roll` — grant a resolved roll's item on THIS world shard, if it holds the
    /// matching corpse row. A shard that does not hold the corpse is unaffected: the module's own
    /// `withheld` guard makes a wrong-shard call harmless.
    fn settle_loot_roll(&self, corpse_guid: u64, slot: u8, winner_guid: u64) -> Result<()>;

    /// `clear_promoted_loot_roll` — delete a staging roll's rows on THIS world shard, once the relay
    /// has promoted it onto realm-core.
    fn clear_promoted_loot_roll(&self, roll_id: u64) -> Result<()>;

    // Same shape as `Coordinator::loot_won_since` (watermark + `(corpse, slot, winner)` triples) — the trait mirrors the read it fronts.
    #[allow(clippy::type_complexity)]
    /// Every `ROLL_WON` `game_group_event` row realm-core has pushed with an id greater than
    /// `after_id` — `(corpse_guid, slot, winner_guid)` triples, plus the new high-water mark to
    /// poll from next. Called on the **realm-core** handle.
    fn loot_won_since(&self, after_id: u64) -> Result<(u64, Vec<(u64, u8, u64)>)>;

    /// `CMSG_LOOT_ROLL` — record the caller's need/greed/pass vote.
    fn loot_roll(
        &self,
        actor: Actor,
        corpse_guid: u64,
        loot_slot: u32,
        vote: u8,
    ) -> Result<LootActionStatus>;

    /// `CMSG_LOOT_MASTER_GIVE` — the master looter assigns an above-
    /// threshold row to `target_guid`.
    fn loot_master_give(
        &self,
        actor: Actor,
        corpse_guid: u64,
        loot_slot: u8,
        target_guid: u64,
    ) -> Result<LootActionStatus>;
}

/// Route `CMSG_LOOT_ROLL` for the session that owns `actor`.
///
/// Unsharded → the pre-realm-core path, verbatim: `loot_roll` on the player's own connection. Sharded →
/// realm-core, where the roll is authoritative once promoted; `actor` is the Character the gateway
/// authenticated for this socket, never the client's own claim (there isn't one — `CMSG_LOOT_ROLL`
/// carries no actor field at all, only the roll's own `(corpse_guid, slot)` and the vote).
///
/// A client can vote as soon as `SMSG_LOOT_START_ROLL` arrives, before the next [`relay_tick`].
/// Realm-core refuses a vote for a roll it does not hold, so pending promotions are flushed first.
pub(crate) fn run_vote<St: LootRollStore + ShardRoutingStore + ?Sized>(
    store: &St,
    actor: Actor,
    corpse_guid: u64,
    slot: u32,
    vote: u8,
) -> Result<LootActionStatus> {
    let Some(realm) = store.realm_store() else {
        return store.loot_roll(actor, corpse_guid, slot, vote);
    };
    flush_pending_promotions(store, realm.as_ref());
    realm.realm_loot_vote(corpse_guid, slot as u8, actor, vote)
}

/// Promote ONE world shard's staging roll onto realm-core, then clear the staging copy — the shared
/// core of [`relay_tick`]'s periodic sweep and [`flush_pending_promotions`]'s synchronous, per-op
/// flush. Best-effort: a failure here is logged and left for the periodic relay to retry, never
/// propagated as an error (the caller's own op — a vote route, a party op — must not fail because a
/// DIFFERENT roll's promotion did).
fn promote_one(shard: &dyn WorldStore, realm: &dyn WorldStore, roll: &PendingLootRoll) {
    match realm.realm_loot_start(
        roll.corpse_guid,
        roll.slot,
        roll.item_entry,
        roll.deadline_micros,
        roll.recipients.clone(),
        roll.random_property_id,
        roll.promotion_source,
        roll.roll_id,
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

/// Synchronously promote EVERY connected world shard's pending staging rolls onto realm-core.
///
/// [`run_vote`] calls it before each vote, so realm-core holds the roll the vote names. `party::run`
/// calls it immediately before dispatching a LEAVE/UNINVITE — the two ops that can shrink a group
/// below 2 members and reach `remove_member`'s disband branch. This is what closes the disband race
/// the periodic [`relay_tick`] alone cannot (see this module's doc): by the time
/// `realm_group_op(LEAVE/UNINVITE, ..)` runs right after this returns, every roll that existed
/// anywhere in the realm at that moment is already on realm-core for `remove_member` to see.
///
/// Every connected world shard, not just the acting character's own — a party's members, and
/// therefore the corpses their kills stage rolls on, can be split across shards (the premise
/// realm-core's group slice established), so narrowing this to one shard would silently reintroduce
/// the class of bug the group slice fixed for invites.
///
/// Best-effort per shard, the same posture `party::sync_mirrors` documents: a shard that cannot be
/// reached leaves its own pending rolls unpromoted for THIS call, and the periodic relay retries them
/// on its own cadence — a flush that failed must not turn a vote or LEAVE/UNINVITE that would
/// otherwise succeed into an error.
pub(crate) fn flush_pending_promotions<St: ShardRoutingStore + ?Sized>(
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

/// One promotion + settlement pass. No-op on an unsharded store (`realm_store()` answers
/// `None`), which is what makes running this on a timer free for a single-database gateway.
///
/// `won_watermark` advances only after every settlement in the batch succeeds on every Shard.
/// A failure leaves the result available to retry while its Realm-core event still exists.
/// A successful grant removes the original loot row, and the Module ignores rows that are not
/// withheld. These guards cannot distinguish a later loot row with the same corpse GUID and slot.
///
/// Both directions are BEST-EFFORT, deliberately, the same posture `party::sync_mirrors` documents:
/// realm-core has already committed (a promoted roll exists there, or a roll has already resolved)
/// by the time either loop runs, so a failed relay step must not undo or re-litigate that — it just
/// leaves the affected Shard's local state stale. Settlement retries still need the outcome event.
///
/// Ordinary promotion latency (a roll NOT caught by [`flush_pending_promotions`]) is bounded by this
/// function's own caller's poll interval, not by anything in here — see this module's doc for why
/// that caller is a poll rather than a persistent callback.
pub(crate) fn relay_tick<St: ShardRoutingStore + ?Sized>(store: &St, won_watermark: &mut u64) {
    let Some(realm) = store.realm_store() else {
        return;
    };
    flush_pending_promotions(store, realm.as_ref());

    // SETTLEMENT: every `ROLL_WON` realm-core has pushed since the last tick, fanned to every
    // connected world shard — `settle_loot_roll`'s own `withheld` guard makes the wrong shards' calls
    // harmless no-ops, so this does not need to know in advance which one holds the corpse.
    match realm.loot_won_since(*won_watermark) {
        Ok((new_watermark, wins)) => {
            let mut settled = true;
            for (corpse_guid, slot, winner_guid) in wins {
                for shard in store.world_stores() {
                    if let Err(e) = shard.settle_loot_roll(corpse_guid, slot, winner_guid) {
                        settled = false;
                        log::warn!(
                            "loot-roll relay: could not settle ({corpse_guid}, {slot}) on {} ({e:#}); \
                             retrying while the result event remains",
                            shard.shard_name()
                        );
                    }
                }
            }
            if settled {
                *won_watermark = new_watermark;
            }
        }
        Err(e) => {
            log::warn!("loot-roll relay: could not read realm-core's ROLL_WON events ({e:#})")
        }
    }
}
