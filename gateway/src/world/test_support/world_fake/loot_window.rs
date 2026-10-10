use super::super::*;

#[derive(Default)]
pub(crate) struct LootWindowState {
    /// The lootable copper `loot_target_money` reports for any target (default 0).
    pub(crate) corpse_money: u32,
    /// Recorded `loot_money` targets — CMSG_LOOT_MONEY must drive the TRACKED guid.
    pub(crate) money_looted: std::sync::Mutex<Vec<u64>>,
    /// Per-VIEWER corpse loot fixture for `corpse_loot(corpse_guid, viewer_guid)`, keyed by viewer
    /// guid. Empty by default, so a test that never sets it sees an empty window.
    pub(crate) corpse_loot_by_viewer: std::collections::HashMap<u64, Vec<codec::LootItemView>>,
}

impl LootWindowStore for WorldFake {
    fn loot_target_money(&self, _target_guid: u64) -> Result<u32> {
        Ok(self.loot_window.corpse_money)
    }

    fn loot_target_items(
        &self,
        _target_guid: u64,
        viewer_guid: u64,
    ) -> Result<Vec<codec::LootItemView>> {
        Ok(self
            .loot_window
            .corpse_loot_by_viewer
            .get(&viewer_guid)
            .cloned()
            .unwrap_or_default())
    }

    fn use_gameobject(
        &self,
        _account_id: u64,
        _actor_guid: u64,
        _target_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        Ok(LootWindowRequestStatus::Applied)
    }

    fn open_creature_loot(
        &self,
        _account_id: u64,
        _actor_guid: u64,
        _corpse_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        Ok(LootWindowRequestStatus::Applied)
    }

    fn skin_corpse(
        &self,
        _account_id: u64,
        _actor_guid: u64,
        _target_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        Ok(LootWindowRequestStatus::Applied)
    }

    fn loot_money(
        &self,
        _account_id: u64,
        _actor_guid: u64,
        target_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        self.loot_window
            .money_looted
            .lock()
            .unwrap()
            .push(target_guid);
        Ok(LootWindowRequestStatus::Applied)
    }

    fn take_loot(
        &self,
        _account_id: u64,
        _actor_guid: u64,
        _target_guid: u64,
        _loot_slot: u8,
    ) -> Result<LootWindowRequestStatus> {
        Ok(LootWindowRequestStatus::Applied)
    }
}
