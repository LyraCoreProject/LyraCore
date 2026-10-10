use super::super::*;

#[derive(Default)]
pub(crate) struct LootWindowState {
    /// The lootable copper `loot_target_money` reports for any target (default 0).
    pub(crate) corpse_money: u32,
    /// Recorded `loot_money` targets — CMSG_LOOT_MONEY must drive the TRACKED guid.
    pub(crate) money_looted: std::sync::Mutex<Vec<u64>>,
    /// Recorded target and slot for `CMSG_AUTOSTORE_LOOT_ITEM`.
    pub(crate) items_looted: std::sync::Mutex<Vec<(u64, u8)>>,
    /// Recorded `skin_corpse` targets (the empty-loot-window skinning fallback).
    pub(crate) skinned: std::sync::Mutex<Vec<u64>>,
    /// Typed legacy skinning refusal returned by the empty-loot fallback.
    pub(crate) skinning_refusal: Option<LootWindowRefusal>,
    /// Infrastructure failure returned by the empty-loot skinning fallback.
    pub(crate) skinning_failure: Option<String>,
    /// Per-VIEWER corpse loot fixture for `corpse_loot(corpse_guid, viewer_guid)` — different viewers
    /// of the SAME corpse can see different windows (`quest_only` rows are per-looter) — keyed by
    /// viewer guid, standing in for whatever the real per-viewer read
    /// (`gateway/src/stdb/reads.rs::corpse_loot`) would return for that viewer; its own filtering
    /// decision is unit-tested directly in `reads.rs`, not reproduced here. Empty by default — every
    /// test that never sets this keeps seeing an empty window, byte-identical to before.
    pub(crate) corpse_loot_by_viewer: std::collections::HashMap<u64, Vec<codec::LootItemView>>,
    /// Recorded non-questgiver GameObject uses owned by the loot-window seam.
    pub(crate) gameobjects_used: std::sync::Mutex<Vec<u64>>,
    pub(crate) corpse_loot_reads: std::sync::Mutex<Vec<(u64, u64)>>,
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
        self.loot_window
            .corpse_loot_reads
            .lock()
            .unwrap()
            .push((_target_guid, viewer_guid));
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
        target_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        self.loot_window
            .gameobjects_used
            .lock()
            .unwrap()
            .push(target_guid);
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
        target_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        if let Some(error) = &self.loot_window.skinning_failure {
            return Err(anyhow!(error.clone()));
        }
        if let Some(refusal) = self.loot_window.skinning_refusal {
            return Ok(LootWindowRequestStatus::Refused(refusal));
        }
        self.loot_window.skinned.lock().unwrap().push(target_guid);
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
        target_guid: u64,
        loot_slot: u8,
    ) -> Result<LootWindowRequestStatus> {
        self.loot_window
            .items_looted
            .lock()
            .unwrap()
            .push((target_guid, loot_slot));
        Ok(LootWindowRequestStatus::Applied)
    }
}
