//! `Coordinator`'s [`ItemActionStore`] adapter.

use anyhow::Result;
use lyracore_shared::item::ItemRefusal;

use crate::stdb::bindings::*;
use crate::stdb::connection::{call_reducer, reducer_refusal_reason};
use crate::stdb::Coordinator;
use crate::world::{Actor, ItemActionResult, ItemActionStore};

impl ItemActionStore for crate::stdb::Coordinator {
    fn equip_item(
        &self,
        account_id: u64,
        actor_guid: u64,
        from_slot: u8,
    ) -> Result<ItemActionResult> {
        crate::stdb::Coordinator::equip_item(self, account_id, actor_guid, from_slot)
    }

    fn unequip_item(
        &self,
        account_id: u64,
        actor_guid: u64,
        from_slot: u8,
    ) -> Result<ItemActionResult> {
        crate::stdb::Coordinator::unequip_item(self, account_id, actor_guid, from_slot)
    }

    fn move_item(
        &self,
        account_id: u64,
        actor_guid: u64,
        from_slot: u8,
        to_slot: u8,
    ) -> Result<ItemActionResult> {
        crate::stdb::Coordinator::move_item(self, account_id, actor_guid, from_slot, to_slot)
    }

    fn use_item(&self, account_id: u64, actor_guid: u64, slot: u8) -> Result<ItemActionResult> {
        crate::stdb::Coordinator::use_item(self, account_id, actor_guid, slot)
    }
}

impl Coordinator {
    /// Equip the item in main-inventory `from_slot` (`CMSG_AUTOEQUIP_ITEM`) over the coordinator
    /// connection. The module resolves the matching equipment slot and gates the required level.
    /// Rides the coordinator connection as `gw_equip_item`.
    pub fn equip_item(
        &self,
        _account_id: u64,
        actor_guid: u64,
        from_slot: u8,
    ) -> Result<ItemActionResult> {
        let Some(actor) = resolved_item_actor("equip_item", actor_guid) else {
            return Ok(ItemRefusal::Internal.into());
        };
        let coord = self.0.call_pipe();
        item_action(call_reducer!(
            coord.conn.reducers,
            "gw_equip_item",
            gw_equip_item_then(self.session_actor(actor), from_slot)
        ))
    }

    /// Unequip the item in equipment `from_slot` to a free backpack slot (`CMSG_AUTOSTORE_BAG_ITEM`)
    /// over the coordinator connection. The module gates "is equipped" + "backpack has room".
    pub fn unequip_item(
        &self,
        _account_id: u64,
        actor_guid: u64,
        from_slot: u8,
    ) -> Result<ItemActionResult> {
        let Some(actor) = resolved_item_actor("unequip_item", actor_guid) else {
            return Ok(ItemRefusal::Internal.into());
        };
        let coord = self.0.call_pipe();
        item_action(call_reducer!(
            coord.conn.reducers,
            "gw_unequip_item",
            gw_unequip_item_then(self.session_actor(actor), from_slot)
        ))
    }

    /// Use the consumable in main-inventory `slot` (`CMSG_USE_ITEM`) over the coordinator connection —
    /// eat/drink/potion/bandage. The module applies the on-use effect (flat heal for slice food) and
    /// decrements the stack; a gameplay `Err` (no item / not usable) is per-action.
    pub fn use_item(
        &self,
        _account_id: u64,
        actor_guid: u64,
        slot: u8,
    ) -> Result<ItemActionResult> {
        let Some(actor) = resolved_item_actor("use_item", actor_guid) else {
            return Ok(ItemRefusal::Internal.into());
        };
        let coord = self.0.call_pipe();
        item_action(call_reducer!(
            coord.conn.reducers,
            "gw_use_item",
            gw_use_item_then(self.session_actor(actor), slot)
        ))
    }

    /// Move (or swap) main-inventory `from_slot` → `to_slot` (`CMSG_SWAP_INV_ITEM`/`CMSG_SWAP_ITEM`)
    /// over the coordinator connection. The module's move primitive validates equip-slot transitions.
    pub fn move_item(
        &self,
        _account_id: u64,
        actor_guid: u64,
        from_slot: u8,
        to_slot: u8,
    ) -> Result<ItemActionResult> {
        let Some(actor) = resolved_item_actor("move_item", actor_guid) else {
            return Ok(ItemRefusal::Internal.into());
        };
        let coord = self.0.call_pipe();
        item_action(call_reducer!(
            coord.conn.reducers,
            "gw_move_item",
            gw_move_item_then(self.session_actor(actor), from_slot, to_slot)
        ))
    }
}

/// The Module's typed item Refusal, on the same rule as the auction family: only a reducer the
/// Module rejected carries a tag, so a timeout or transport failure keeps its unknown outcome.
fn item_action(result: Result<()>) -> Result<ItemActionResult> {
    match result {
        Ok(()) => Ok(ItemActionResult::Done),
        Err(error) => match reducer_refusal_reason(&error).and_then(ItemRefusal::parse_tag) {
            Some(refusal) => Ok(refusal.into()),
            None => Err(error),
        },
    }
}

/// An item action needs the caller's own entity. Without one there is nothing to request, so the
/// client gets a Refusal rather than a dead session.
fn resolved_item_actor(operation: &str, actor_guid: u64) -> Option<Actor> {
    let actor = Actor::new(actor_guid);
    if actor.is_none() {
        log::warn!("stdb: {operation} has no resolved actor");
    }
    actor
}

#[cfg(test)]
mod item_reducer_tests {
    use super::*;
    use crate::stdb::connection::ReducerCallError;
    use anyhow::anyhow;

    #[test]
    fn only_a_rejected_reducer_carries_a_typed_refusal() {
        for refusal in ItemRefusal::ALL {
            let rejected = Err(anyhow::Error::from(ReducerCallError::Rejected {
                operation: "gw_move_item".to_string(),
                reason: refusal.as_tag().to_string(),
            })
            .context("move phase"));
            assert_eq!(
                item_action(rejected).unwrap(),
                ItemActionResult::Refused(refusal)
            );
        }

        assert_eq!(item_action(Ok(())).unwrap(), ItemActionResult::Done);

        let not_refusals = [
            anyhow::Error::from(ReducerCallError::fatal(
                "gw_move_item reducer timed out after 10s".to_string(),
            )),
            anyhow::Error::from(ReducerCallError::fatal(
                "gw_use_item reducer failed: transport disconnected".to_string(),
            )),
            anyhow::Error::from(ReducerCallError::Rejected {
                operation: "gw_equip_item".to_string(),
                reason: "operator only".to_string(),
            }),
            anyhow!(
                "wrapped text that mentions {}",
                ItemRefusal::Internal.as_tag()
            ),
        ];
        for error in not_refusals {
            let text = format!("{error:#}");
            assert!(item_action(Err(error)).is_err(), "{text}");
        }
    }
}
