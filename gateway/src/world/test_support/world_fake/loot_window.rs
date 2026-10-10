use super::super::*;

#[derive(Default)]
pub(crate) struct LootWindowState {
    /// The lootable copper `loot_target_money` reports for any target (default 0).
    pub(crate) corpse_money: u32,
    /// Recorded `loot_money` targets — CMSG_LOOT_MONEY must drive the TRACKED guid.
    pub(crate) money_looted: std::sync::Mutex<Vec<u64>>,
    /// Recorded `take_loot` calls as (target guid, slot): a take must act on the open target.
    pub(crate) items_taken: std::sync::Mutex<Vec<(u64, u8)>>,
    /// Recorded `use_gameobject` target guids.
    pub(crate) gameobjects_used: std::sync::Mutex<Vec<u64>>,
    /// Per-VIEWER corpse loot fixture for `corpse_loot(corpse_guid, viewer_guid)`, keyed by viewer
    /// guid. Empty by default, so a test that never sets it sees an empty window.
    pub(crate) corpse_loot_by_viewer: std::collections::HashMap<u64, Vec<codec::LootItemView>>,
}

impl LootWindowStore for WorldFake {
    fn loot_target_money(&self, _target_guid: u64) -> Result<u32> {
        if let Some(state) = &self.benilla_gameplay {
            return Ok(state.lock().unwrap().corpse_money);
        }
        Ok(self.loot_window.corpse_money)
    }

    fn loot_target_items(
        &self,
        _target_guid: u64,
        viewer_guid: u64,
    ) -> Result<Vec<codec::LootItemView>> {
        if let Some(state) = &self.benilla_gameplay {
            return Ok(state.lock().unwrap().loot.clone());
        }
        Ok(self
            .loot_window
            .corpse_loot_by_viewer
            .get(&viewer_guid)
            .cloned()
            .unwrap_or_default())
    }

    fn use_gameobject(&self, _actor: Actor, target_guid: u64) -> Result<LootWindowRequestStatus> {
        self.loot_window
            .gameobjects_used
            .lock()
            .unwrap()
            .push(target_guid);
        Ok(LootWindowRequestStatus::Applied)
    }

    fn open_creature_loot(
        &self,
        _actor: Actor,
        _corpse_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        Ok(LootWindowRequestStatus::Applied)
    }

    fn skin_corpse(&self, _actor: Actor, _target_guid: u64) -> Result<LootWindowRequestStatus> {
        Ok(LootWindowRequestStatus::Applied)
    }

    fn loot_money(&self, _actor: Actor, target_guid: u64) -> Result<LootWindowRequestStatus> {
        self.loot_window
            .money_looted
            .lock()
            .unwrap()
            .push(target_guid);
        if let Some(state) = &self.benilla_gameplay {
            let mut state = state.lock().unwrap();
            state.copper += std::mem::take(&mut state.corpse_money);
        }
        Ok(LootWindowRequestStatus::Applied)
    }

    fn take_loot(
        &self,
        actor: Actor,
        target_guid: u64,
        loot_slot: u8,
    ) -> Result<LootWindowRequestStatus> {
        self.loot_window
            .items_taken
            .lock()
            .unwrap()
            .push((target_guid, loot_slot));
        if let Some(state) = &self.benilla_gameplay {
            let mut state = state.lock().unwrap();
            if let Some(index) = state.loot.iter().position(|item| item.0 == loot_slot) {
                let (_, entry, stack_count, _, random_property_id) = state.loot.remove(index);
                state.inventory.push(codec::ItemInstanceView {
                    guid: 0x4000_0000_0000_0000 | (u64::from(loot_slot) + 1),
                    owner_guid: actor.guid(),
                    entry,
                    stack_count,
                    random_property_id,
                    slot: 23,
                    ..Default::default()
                });
            }
        }
        Ok(LootWindowRequestStatus::Applied)
    }
}
