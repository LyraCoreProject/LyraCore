use super::super::*;

#[derive(Default)]
pub(crate) struct VendorState {
    /// Vendor stock the seam's `vendor_stock` read returns (empty by default).
    pub(crate) vendor_stock: Vec<codec::VendorItemView>,
    /// The player's buyback ring as `(item_entry, stack_count, price)`; empty by default, so a
    /// fixture login replays no buyback tab.
    pub(crate) buyback_ring: Vec<(u32, u32, u32, u32)>,
    /// Item-instance guid → bag slot, for the vendor repair target.
    pub(crate) item_slots: Vec<(u64, u8)>,
    /// Recorded `vendor_buyback` calls: (vendor_guid, slot) — pins the 69→0 slot mapping.
    pub(crate) bought_back: std::sync::Mutex<Vec<(u64, u8)>>,
}

impl VendorActionStore for WorldFake {
    fn vendor_stock(&self, _vendor_guid: u64) -> Result<Vec<codec::VendorItemView>> {
        Ok(self.vendor.vendor_stock.clone())
    }

    fn vendor_refuses_interaction(&self, _vendor_guid: u64, _player_guid: u64) -> Result<bool> {
        Ok(self.npc_refuses)
    }

    fn vendor_buy(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _vendor_guid: u64,
        _item_entry: u32,
        _count: u32,
    ) -> Result<()> {
        match &self.trade_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }

    fn buyback_slots(&self, _player_guid: u64) -> Vec<(u32, u32, u32, u32)> {
        self.vendor.buyback_ring.clone()
    }

    fn random_property_enchant_ids(&self, _random_property_id: u32) -> [u32; 3] {
        [0; 3]
    }

    fn vendor_item_slot(&self, item_guid: u64) -> Option<u8> {
        self.vendor
            .item_slots
            .iter()
            .find(|(g, _)| *g == item_guid)
            .map(|&(_, s)| s)
    }

    fn vendor_repair(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _npc_guid: u64,
        _slot: u8,
    ) -> Result<()> {
        match &self.trade_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }

    fn vendor_sell(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _vendor_guid: u64,
        _slot: u8,
    ) -> Result<()> {
        match &self.trade_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }

    fn vendor_buyback(
        &self,
        _account_id: u64,
        _self_guid: u64,
        vendor_guid: u64,
        slot: u8,
    ) -> Result<()> {
        if let Some(e) = &self.trade_error {
            return Err(anyhow!("{e}"));
        }
        self.vendor
            .bought_back
            .lock()
            .unwrap()
            .push((vendor_guid, slot));
        Ok(())
    }
}
