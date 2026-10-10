//! `Coordinator`'s [`VendorActionStore`] adapter.

use anyhow::Result;
use spacetimedb_sdk::Table;

use crate::codec;
use crate::stdb::bindings::*;
use crate::stdb::connection::call_reducer;
use crate::stdb::reads::property_enchant_ids;
use crate::world::{Actor, VendorActionStore};

impl VendorActionStore for crate::stdb::Coordinator {
    /// Read a vendor's stock for `SMSG_LIST_INVENTORY`. Resolve the vendor's creature entry from
    /// its `game_world_entity` row, filter `game_npc_vendor` to that entry, and join each
    /// `game_item_template` for the display id / buy price / max durability (carrying the
    /// npc_vendor row's `max_count`). Read from the privileged cache; the SDK exposes only the PK
    /// index, so iterate and filter. A stock line whose template isn't loaded is skipped: it can't
    /// be priced or displayed. Ordered by the vendor `slot`.
    fn vendor_stock(&self, vendor_guid: u64) -> Result<Vec<codec::VendorItemView>> {
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
                db.game_item_template()
                    .entry()
                    .find(&v.item_entry)
                    .map(|t| codec::VendorItemView {
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

    fn vendor_refuses_interaction(&self, vendor_guid: u64, actor: Actor) -> Result<bool> {
        self.npc_refuses_interaction(vendor_guid, actor.guid())
    }

    /// The module gates the purchase on the vendor (stock + NPC flags + range) and debits the
    /// buyer's copper.
    fn vendor_buy(
        &self,
        actor: Actor,
        vendor_guid: u64,
        item_entry: u32,
        count: u32,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_buy_item",
            gw_buy_item_then(self.session_actor(actor), vendor_guid, item_entry, count)
        )
    }

    /// The player's buyback ring, newest-first: `(item_entry, stack_count, price,
    /// random_property_id)`, at most 12.
    fn buyback_slots(&self, player_guid: u64) -> Vec<(u32, u32, u32, u32)> {
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

    fn random_property_enchant_ids(&self, random_property_id: u32) -> [u32; 3] {
        let guard = self.0.coord();
        property_enchant_ids(&guard.conn.db, random_property_id)
    }

    fn vendor_item_slot(&self, item_guid: u64) -> Option<u8> {
        crate::stdb::Coordinator::item_slot_by_guid(self, item_guid)
    }

    /// The module gates the REPAIR NPC and charges copper; the player's item and purse replicate
    /// back via subscription.
    fn vendor_repair(&self, actor: Actor, npc_guid: u64, slot: u8) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_repair_item",
            gw_repair_item_then(self.session_actor(actor), npc_guid, slot)
        )
    }

    /// The gateway resolves the client's item-INSTANCE guid to the owning slot before calling
    /// (the reducer takes the slot); the module credits the seller's copper.
    fn vendor_sell(&self, actor: Actor, vendor_guid: u64, slot: u8) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_sell_item",
            gw_sell_item_then(self.session_actor(actor), vendor_guid, slot)
        )
    }

    fn vendor_buyback(&self, actor: Actor, vendor_guid: u64, slot: u8) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_buyback_item",
            gw_buyback_item_then(self.session_actor(actor), vendor_guid, slot)
        )
    }
}
