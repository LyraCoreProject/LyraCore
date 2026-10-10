use super::super::*;

/// Item actions shared by the family and Benilla World Session tests.
impl ItemActionStore for WorldFake {
    fn destroy_item(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _slot: u8,
        _count: u32,
    ) -> Result<ItemActionResult> {
        Ok(lyracore_shared::item::ItemRefusal::ItemNotFound.into())
    }
    fn split_item(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _from_slot: u8,
        _to_slot: u8,
        _count: u32,
    ) -> Result<ItemActionResult> {
        Ok(lyracore_shared::item::ItemRefusal::ItemNotFound.into())
    }

    fn equip_item(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _from_slot: u8,
    ) -> Result<ItemActionResult> {
        Ok(ItemActionResult::Done)
    }

    fn unequip_item(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _from_slot: u8,
    ) -> Result<ItemActionResult> {
        Ok(ItemActionResult::Done)
    }

    fn move_item(
        &self,
        _account_id: u64,
        _self_guid: u64,
        from_slot: u8,
        to_slot: u8,
    ) -> Result<ItemActionResult> {
        if let Some(state) = &self.benilla_gameplay {
            let mut state = state.lock().unwrap();
            for item in &mut state.inventory {
                if item.slot == from_slot {
                    item.slot = to_slot;
                } else if item.slot == to_slot {
                    item.slot = from_slot;
                }
            }
        }
        Ok(ItemActionResult::Done)
    }

    fn use_item(&self, _account_id: u64, _self_guid: u64, _slot: u8) -> Result<ItemActionResult> {
        Ok(ItemActionResult::Done)
    }
}
