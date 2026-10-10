use super::super::*;
use crate::world::Actor;

#[derive(Default)]
pub(crate) struct VendorState {
    /// Vendor stock the seam's `vendor_stock` read returns (empty by default).
    pub(crate) vendor_stock: Vec<codec::VendorItemView>,
    /// The player's buyback ring as `(item_entry, stack_count, price)`; empty by default, so a
    /// fixture login replays no buyback tab.
    pub(crate) buyback_ring: Vec<(u32, u32, u32, u32)>,
}

/// Vendor behaviour is tested through `InMemoryVendorActions`; here only the reads the session and
/// the gossip menu make are live.
impl VendorActionStore for WorldFake {
    fn vendor_stock(&self, _vendor_guid: u64) -> Result<Vec<codec::VendorItemView>> {
        Ok(self.vendor.vendor_stock.clone())
    }

    fn vendor_refuses_interaction(&self, _vendor_guid: u64, _actor: Actor) -> Result<bool> {
        Ok(self.npc_refuses)
    }

    fn vendor_buy(
        &self,
        _actor: Actor,
        _vendor_guid: u64,
        _item_entry: u32,
        _count: u32,
    ) -> Result<()> {
        Ok(())
    }

    fn buyback_slots(&self, _player_guid: u64) -> Vec<(u32, u32, u32, u32)> {
        self.vendor.buyback_ring.clone()
    }

    fn random_property_enchant_ids(&self, _random_property_id: u32) -> [u32; 3] {
        [0; 3]
    }

    fn vendor_item_slot(&self, _item_guid: u64) -> Option<u8> {
        None
    }

    fn vendor_repair(&self, _actor: Actor, _npc_guid: u64, _slot: u8) -> Result<()> {
        Ok(())
    }

    fn vendor_sell(&self, _actor: Actor, _vendor_guid: u64, _slot: u8) -> Result<()> {
        Ok(())
    }

    fn vendor_buyback(&self, _actor: Actor, _vendor_guid: u64, _slot: u8) -> Result<()> {
        Ok(())
    }
}
