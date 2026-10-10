//! `Coordinator`'s [`LootRollStore`] adapter.

use anyhow::Result;

use crate::world::loot::{LootRollStore, PendingLootRoll};
use crate::world::LootActionStatus;

use crate::stdb::Coordinator;

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
