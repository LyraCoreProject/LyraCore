use super::super::*;

/// Item behaviour is tested through `InMemoryItemActions`; every verb here succeeds.
impl ItemActionStore for WorldFake {
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
        _from_slot: u8,
        _to_slot: u8,
    ) -> Result<ItemActionResult> {
        Ok(ItemActionResult::Done)
    }

    fn use_item(&self, _account_id: u64, _self_guid: u64, _slot: u8) -> Result<ItemActionResult> {
        Ok(ItemActionResult::Done)
    }
}
