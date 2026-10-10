use super::super::*;

#[derive(Default)]
pub(crate) struct ItemState {
    /// Recorded `use_item` slots.
    pub(crate) used_items: std::sync::Mutex<Vec<u8>>,
}

impl ItemActionStore for WorldFake {
    fn equip_item(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _from_slot: u8,
    ) -> Result<ItemActionResult> {
        self.canned_item_action()
    }

    fn unequip_item(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _from_slot: u8,
    ) -> Result<ItemActionResult> {
        self.canned_item_action()
    }

    fn move_item(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _from_slot: u8,
        _to_slot: u8,
    ) -> Result<ItemActionResult> {
        self.canned_item_action()
    }

    fn use_item(&self, _account_id: u64, _self_guid: u64, slot: u8) -> Result<ItemActionResult> {
        self.item.used_items.lock().unwrap().push(slot);
        self.canned_item_action()
    }
}

impl WorldFake {
    /// The Coordinator answers a Refusal tag as an outcome and anything else as a failure with an
    /// unknown durable result, so `trade_error` reaches the item family the same way.
    pub(crate) fn canned_item_action(&self) -> Result<ItemActionResult> {
        match &self.trade_error {
            None => Ok(ItemActionResult::Done),
            Some(e) => ItemRefusal::parse_tag(e)
                .map(ItemActionResult::from)
                .ok_or_else(|| anyhow!("{e}")),
        }
    }
}
