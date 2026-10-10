//! `Coordinator`'s [`LootRollStore`] adapter.

use anyhow::{anyhow, Result};
use lyracore_shared::loot::LootRefusal;
use spacetimedb_sdk::Table;

use crate::stdb::bindings::*;
use crate::stdb::connection::{call_reducer, reducer_refusal_reason};
use crate::stdb::Coordinator;
use crate::world::loot::{LootRollStore, PendingLootRoll};
use crate::world::{Actor, LootActionStatus};

impl LootRollStore for Coordinator {
    fn loot_roll(
        &self,
        account_id: u64,
        self_guid: u64,
        corpse_guid: u64,
        loot_slot: u32,
        vote: u8,
    ) -> Result<LootActionStatus> {
        self.loot_roll(account_id, self_guid, corpse_guid, loot_slot, vote)
    }

    fn realm_loot_op(
        &self,
        op: u8,
        corpse_guid: u64,
        slot: u8,
        item_entry: u32,
        actor_guid: u64,
        vote: u8,
        deadline_micros: i64,
        recipients: Vec<u64>,
        random_property_id: u32,
        promotion_source: spacetimedb_sdk::Identity,
        source_roll_id: u64,
    ) -> Result<()> {
        self.realm_loot_op(
            op,
            corpse_guid,
            slot,
            item_entry,
            actor_guid,
            vote,
            deadline_micros,
            recipients,
            random_property_id,
            promotion_source,
            source_roll_id,
        )
    }

    fn realm_loot_vote(
        &self,
        corpse_guid: u64,
        slot: u8,
        actor_guid: u64,
        vote: u8,
    ) -> Result<LootActionStatus> {
        self.realm_loot_vote(corpse_guid, slot, actor_guid, vote)
    }

    fn pending_local_rolls(&self) -> Result<Vec<PendingLootRoll>> {
        self.pending_local_rolls()
    }

    fn settle_loot_roll(&self, corpse_guid: u64, slot: u8, winner_guid: u64) -> Result<()> {
        self.settle_loot_roll(corpse_guid, slot, winner_guid)
    }

    fn clear_promoted_loot_roll(&self, roll_id: u64) -> Result<()> {
        self.clear_promoted_loot_roll(roll_id)
    }

    fn loot_won_since(&self, after_id: u64) -> Result<(u64, Vec<(u64, u8, u64)>)> {
        self.loot_won_since(after_id)
    }

    fn loot_master_give(
        &self,
        account_id: u64,
        self_guid: u64,
        corpse_guid: u64,
        loot_slot: u8,
        target_guid: u64,
    ) -> Result<LootActionStatus> {
        self.loot_master_give(account_id, self_guid, corpse_guid, loot_slot, target_guid)
    }
}

impl Coordinator {
    /// Every UNRESOLVED `game_loot_roll` row on THIS handle's database, joined with its votes.
    ///
    /// A cache read, like [`group_roster`](Self::group_roster) — this table is now part of the
    /// coordinator subscription list, so no reducer call is
    /// needed. Meaningful only on a WORLD SHARD in a sharded deployment: realm-core never has a row
    /// here that a world shard wrote (only its own `realm_loot_op` START arm inserts there), and on
    /// an unsharded gateway the relay that calls this never runs at all (`realm_store()` is `None`).
    pub fn pending_local_rolls(&self) -> Result<Vec<crate::world::loot::PendingLootRoll>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        Ok(db
            .game_loot_roll()
            .iter()
            .filter(|r| !r.resolved)
            .map(|r| {
                let recipients = db
                    .game_loot_roll_vote()
                    .iter()
                    .filter(|v| v.roll_id == r.id)
                    .map(|v| v.voter_guid)
                    .collect();
                crate::world::loot::PendingLootRoll {
                    roll_id: r.id,
                    corpse_guid: r.corpse_guid,
                    slot: r.slot,
                    item_entry: r.item_entry,
                    deadline_micros: r.deadline_micros,
                    recipients,
                    random_property_id: r.random_property_id,
                    promotion_source: r.promotion_source,
                }
            })
            .collect())
    }

    // The return tuple is the poll result: the new watermark plus the `(corpse, slot, winner)` triples read since the old one.
    #[allow(clippy::type_complexity)]
    /// Every `ROLL_WON` `game_group_event` row on THIS handle's database with `id > after_id`,
    /// decoded to `(corpse_guid, slot, winner_guid)`, plus the new high-water mark (the max id seen,
    /// or `after_id` unchanged if none). Meaningful on the **realm-core** handle: that is the only
    /// database `resolve_roll`/`force_resolve_rolls_for_disband` push a `ROLL_WON` event on in a
    /// sharded deployment (voting is routed there exclusively — `world::loot::run_vote`).
    ///
    /// An unparseable payload is skipped + logged rather than failing the whole scan — the module
    /// writes this grammar, so a decode failure here means the two crates' `lyracore_shared::loot_roll`
    /// copies have drifted, not that this event is meaningless.
    pub fn loot_won_since(&self, after_id: u64) -> Result<(u64, Vec<(u64, u8, u64)>)> {
        use lyracore_shared::loot_roll::event_kind as roll_kind;
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let mut watermark = after_id;
        let mut wins = Vec::new();
        for row in db.game_group_event().iter() {
            if row.id <= after_id || row.kind != roll_kind::ROLL_WON {
                continue;
            }
            watermark = watermark.max(row.id);
            match lyracore_shared::loot_roll::decode_won(&row.payload) {
                Some((corpse_guid, slot, ..)) => wins.push((corpse_guid, slot, row.other_guid)),
                None => log::warn!(
                    "loot-roll relay: unparseable ROLL_WON payload {:?} (event {})",
                    row.payload,
                    row.id
                ),
            }
        }
        Ok((watermark, wins))
    }

    /// `CMSG_LOOT_ROLL` — record the caller's need/greed/pass vote on a
    /// live roll. Live votes/roll numbers relay to every eligible member via the `game_group_event`
    /// roll-kind rows (`stdb/subscriptions.rs`).
    pub fn loot_roll(
        &self,
        _account_id: u64,
        actor_guid: u64,
        corpse_guid: u64,
        loot_slot: u32,
        vote: u8,
    ) -> Result<LootActionStatus> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("loot_roll: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        loot_action_status(call_reducer!(
            coord.conn.reducers,
            "gw_loot_roll",
            gw_loot_roll_then(self.session_actor(actor), corpse_guid, loot_slot, vote)
        ))
    }

    /// `CMSG_LOOT_MASTER_GIVE` — the master looter assigns an above-
    /// threshold row to `target_guid`.
    pub fn loot_master_give(
        &self,
        _account_id: u64,
        actor_guid: u64,
        corpse_guid: u64,
        loot_slot: u8,
        target_guid: u64,
    ) -> Result<LootActionStatus> {
        let actor = Actor::new(actor_guid)
            .ok_or_else(|| anyhow!("loot_master_give: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        loot_action_status(call_reducer!(
            coord.conn.reducers,
            "gw_loot_master_give",
            gw_loot_master_give_then(
                self.session_actor(actor),
                corpse_guid,
                loot_slot,
                target_guid
            )
        ))
    }

    /// `realm_loot_op` — one loot-roll op against the database THIS handle points at. The
    /// gateway calls it on the **realm-core** handle: START promotes a world shard's staging roll,
    /// VOTE casts `CMSG_LOOT_ROLL`'s vote.
    ///
    /// Through the COORDINATOR connection, not the player's: the reducer is operator-gated because
    /// it acts on realm-core, which has no live entity to derive an actor from. VOTE's `actor_guid`
    /// is the guid this socket authenticated into the world with (`InWorld::self_guid`), never a
    /// literal a client supplies; START's `recipients` are the spatial snapshot a world shard already
    /// computed at kill time.
    #[allow(clippy::too_many_arguments)]
    pub fn realm_loot_op(
        &self,
        op: u8,
        corpse_guid: u64,
        slot: u8,
        item_entry: u32,
        actor_guid: u64,
        vote: u8,
        deadline_micros: i64,
        recipients: Vec<u64>,
        random_property_id: u32,
        promotion_source: spacetimedb_sdk::Identity,
        source_roll_id: u64,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_loot_op",
            realm_loot_op_then(
                op,
                corpse_guid,
                slot,
                item_entry,
                self.actor_or_owner(actor_guid),
                vote,
                deadline_micros,
                recipients,
                random_property_id,
                promotion_source,
                source_roll_id
            )
        )
    }

    /// Cast a realm-core Loot Roll vote and classify only a known typed Refusal as an answer.
    pub fn realm_loot_vote(
        &self,
        corpse_guid: u64,
        slot: u8,
        actor_guid: u64,
        vote: u8,
    ) -> Result<LootActionStatus> {
        loot_action_status(call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_loot_op",
            realm_loot_op_then(
                lyracore_shared::loot_roll::loot_op::VOTE,
                corpse_guid,
                slot,
                0,
                self.actor_or_owner(actor_guid),
                vote,
                0,
                Vec::new(),
                0,
                spacetimedb_sdk::Identity::ZERO,
                0
            )
        ))
    }

    /// `settle_loot_roll` — grant a resolved roll's item on THIS world shard, if it holds the
    /// matching corpse row. Operator-gated, coordinator connection; the loot-roll relay calls
    /// it on every connected world shard after observing realm-core's `ROLL_WON` event — the
    /// module's own `withheld` guard makes a wrong-shard call a harmless no-op.
    pub fn settle_loot_roll(&self, corpse_guid: u64, slot: u8, winner_guid: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "settle_loot_roll",
            settle_loot_roll_then(corpse_guid, slot, winner_guid, self.owner_actor())
        )
    }

    /// `clear_promoted_loot_roll` — delete a staging roll's rows on THIS world shard, once the
    /// loot-roll relay has promoted it onto realm-core. Operator-gated, coordinator connection.
    pub fn clear_promoted_loot_roll(&self, roll_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "clear_promoted_loot_roll",
            clear_promoted_loot_roll_then(roll_id, self.owner_actor())
        )
    }
}

/// The typed answer for Loot Roll and master-loot requests. An unknown rejection or transport
/// failure stays an error with an unknown durable result.
fn loot_action_status(result: Result<()>) -> Result<LootActionStatus> {
    match result {
        Ok(()) => Ok(LootActionStatus::Applied),
        Err(error) => match reducer_refusal_reason(&error).and_then(LootRefusal::parse_tag) {
            Some(refusal) => {
                log::debug!("stdb: loot action refused: {error:#}");
                Ok(LootActionStatus::Refused(refusal))
            }
            None => Err(error),
        },
    }
}

#[cfg(test)]
mod loot_reducer_tests {
    use super::*;
    use crate::stdb::connection::ReducerCallError;

    fn rejected(reason: &str) -> Result<()> {
        Err(anyhow::Error::from(ReducerCallError::Rejected {
            operation: "gw_take_loot".to_string(),
            reason: reason.to_string(),
        })
        .context("loot window"))
    }

    #[test]
    fn loot_action_status_keeps_unknown_results_as_errors() {
        for refusal in [
            LootRefusal::RollUnavailable,
            LootRefusal::NotMasterLooter,
            LootRefusal::RecipientUnavailable,
            LootRefusal::RecipientInventoryFull,
        ] {
            assert_eq!(
                loot_action_status(rejected(refusal.as_tag())).unwrap(),
                LootActionStatus::Refused(refusal)
            );
        }

        for error in [
            anyhow::Error::from(ReducerCallError::Rejected {
                operation: "gw_loot_roll".to_string(),
                reason: "loot:newer_module_refusal".to_string(),
            }),
            anyhow::Error::from(ReducerCallError::fatal(
                "gw_loot_roll reducer timed out after 10s".to_string(),
            )),
        ] {
            assert!(loot_action_status(Err(error)).is_err());
        }
    }
}
