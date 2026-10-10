//! `Coordinator`'s [`VendorActionStore`] adapter.

use anyhow::{anyhow, Result};
use spacetimedb_sdk::Table;

use crate::codec;
use crate::stdb::bindings::*;
use crate::stdb::connection::call_reducer;
use crate::stdb::reads::property_enchant_ids;
use crate::stdb::Coordinator;
use crate::world::{Actor, VendorActionStore};

impl VendorActionStore for crate::stdb::Coordinator {
    fn vendor_stock(&self, vendor_guid: u64) -> Result<Vec<codec::VendorItemView>> {
        crate::stdb::Coordinator::vendor_items(self, vendor_guid)
    }

    fn vendor_refuses_interaction(&self, vendor_guid: u64, player_guid: u64) -> Result<bool> {
        crate::stdb::Coordinator::npc_refuses_interaction(self, vendor_guid, player_guid)
    }

    fn vendor_buy(
        &self,
        account_id: u64,
        self_guid: u64,
        vendor_guid: u64,
        item_entry: u32,
        count: u32,
    ) -> Result<()> {
        crate::stdb::Coordinator::buy_item(
            self,
            account_id,
            self_guid,
            vendor_guid,
            item_entry,
            count,
        )
    }

    fn buyback_slots(&self, player_guid: u64) -> Vec<(u32, u32, u32, u32)> {
        crate::stdb::Coordinator::buyback_ring(self, player_guid)
    }

    fn random_property_enchant_ids(&self, random_property_id: u32) -> [u32; 3] {
        crate::stdb::Coordinator::random_property_enchant_ids(self, random_property_id)
    }

    fn vendor_item_slot(&self, item_guid: u64) -> Option<u8> {
        crate::stdb::Coordinator::item_slot_by_guid(self, 0, item_guid)
    }

    fn vendor_repair(
        &self,
        account_id: u64,
        self_guid: u64,
        npc_guid: u64,
        slot: u8,
    ) -> Result<()> {
        crate::stdb::Coordinator::repair_item(self, account_id, self_guid, npc_guid, slot)
    }

    fn vendor_sell(
        &self,
        account_id: u64,
        self_guid: u64,
        vendor_guid: u64,
        slot: u8,
    ) -> Result<()> {
        crate::stdb::Coordinator::sell_item(self, account_id, self_guid, vendor_guid, slot)
    }

    fn vendor_buyback(
        &self,
        account_id: u64,
        self_guid: u64,
        vendor_guid: u64,
        slot: u8,
    ) -> Result<()> {
        crate::stdb::Coordinator::buyback_item(self, account_id, self_guid, vendor_guid, slot)
    }
}

impl Coordinator {
    /// The three enchant ids of `random_property_id`, zero where it names none.
    pub fn random_property_enchant_ids(&self, random_property_id: u32) -> [u32; 3] {
        let guard = self.0.coord();
        property_enchant_ids(&guard.conn.db, random_property_id)
    }

    /// The player's buyback ring, newest-first: `(item_entry, stack_count, price)` ≤12.
    pub fn buyback_ring(&self, player_guid: u64) -> Vec<(u32, u32, u32, u32)> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let mut rows: Vec<_> = db
            .game_character_buyback()
            .iter()
            .filter(|b| b.player_guid == player_guid)
            .collect();
        rows.sort_by(|a, b| b.id.cmp(&a.id));
        rows.into_iter()
            .map(|b| (b.item_entry, b.stack_count, b.price, b.random_property_id))
            .collect()
    }

    /// Read a vendor's stock for `SMSG_LIST_INVENTORY` (Tier 2 / vendors). Resolve the vendor's
    /// creature entry from its `game_world_entity` row, filter `game_npc_vendor` to that entry, and
    /// join each `game_item_template` for the display id / buy price / max durability (carrying the
    /// npc_vendor row's `max_count`). Read from the privileged cache (the coordinator bypasses RLS);
    /// the SDK exposes only the PK index, so iterate+filter like the other row queries. Items whose
    /// template isn't loaded are skipped (we can't price/display them). Ordered by the vendor `slot`.
    pub fn vendor_items(&self, vendor_guid: u64) -> Result<Vec<crate::codec::VendorItemView>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        // The vendor's creature entry drives the stock lookup; a missing entity → no stock.
        let Some(creature_entry) = db
            .game_world_entity()
            .guid()
            .find(&vendor_guid)
            .map(|e| e.entry)
        else {
            return Ok(Vec::new());
        };
        let mut rows: Vec<_> = db
            .game_npc_vendor()
            .iter()
            .filter(|v| v.creature_entry == creature_entry)
            .collect();
        rows.sort_by_key(|v| v.slot); // stable vendor-slot order (SQL has no ORDER BY in 2.5)
        let items = rows
            .into_iter()
            .filter_map(|v| {
                // Skip a stock line whose template isn't loaded — we can't price or display it.
                db.game_item_template()
                    .entry()
                    .find(&v.item_entry)
                    .map(|t| crate::codec::VendorItemView {
                        item_entry: v.item_entry,
                        display_id: t.display_id,
                        buy_price: t.buy_price,
                        max_durability: t.max_durability,
                        max_count: v.max_count,
                        buy_count: t.buy_count,
                    })
            })
            .collect();
        Ok(items)
    }

    /// Buy `count` of `item_entry` from the vendor `vendor_guid` (`CMSG_BUY_ITEM`, Tier 2) over the
    /// coordinator connection so the module attributes the purchase to the caller. The module gates
    /// it on the vendor (stock + NPC flags + range) and debits the buyer's copper.
    pub fn buy_item(
        &self,
        _account_id: u64,
        actor_guid: u64,
        vendor_guid: u64,
        item_entry: u32,
        count: u32,
    ) -> Result<()> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("buy_item: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_buy_item",
            gw_buy_item_then(self.session_actor(actor), vendor_guid, item_entry, count)
        )
    }

    /// Sell the item in inventory `slot` back to a vendor (`CMSG_SELL_ITEM`, Tier 2) over the
    /// coordinator connection. The gateway resolves the client's item-INSTANCE guid to the owning
    /// slot before calling (the reducer takes the slot); the module credits the seller's copper.
    /// Rides the coordinator connection as `gw_sell_item`.
    pub fn sell_item(
        &self,
        _account_id: u64,
        actor_guid: u64,
        vendor_guid: u64,
        slot: u8,
    ) -> Result<()> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("sell_item: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_sell_item",
            gw_sell_item_then(self.session_actor(actor), vendor_guid, slot)
        )
    }

    pub fn buyback_item(
        &self,
        _account_id: u64,
        actor_guid: u64,
        vendor_guid: u64,
        slot: u8,
    ) -> Result<()> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("buyback_item: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_buyback_item",
            gw_buyback_item_then(self.session_actor(actor), vendor_guid, slot)
        )
    }

    /// Repair the item in inventory `slot` at REPAIR-NPC `npc_guid` (`CMSG_REPAIR_ITEM`) over the
    /// coordinator connection. The module gates the NPC + charges copper; the player's item +
    /// purse replicate back via subscription.
    pub fn repair_item(
        &self,
        _account_id: u64,
        actor_guid: u64,
        npc_guid: u64,
        slot: u8,
    ) -> Result<()> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("repair_item: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_repair_item",
            gw_repair_item_then(self.session_actor(actor), npc_guid, slot)
        )
    }
}
