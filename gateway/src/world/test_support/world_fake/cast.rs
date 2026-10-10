use super::super::*;

#[derive(Default)]
pub(crate) struct CastState {
    /// Imported item templates the query path resolves by entry. Keeping this keyed fixture here
    /// makes socket tests exercise the same durable read shape as the Coordinator cache.
    pub(crate) item_templates: Vec<codec::ItemTemplateView>,
    /// Recorded `cast_spell` dispatches: (spell_id, target_guid) — pins target threading.
    pub(crate) casts: std::sync::Mutex<Vec<(u32, u64)>>,
    /// Recorded `start_ranged_attack` dispatches: (target_guid, spell_id) — the Auto Shot intercept.
    pub(crate) ranged_attacks: std::sync::Mutex<Vec<(u64, u32)>>,
    /// The caller's owned items, for the Auto Shot ammo block on the activation START.
    pub(crate) player_items_fixture: Vec<codec::ItemInstanceView>,
    /// Recorded `cancel_aura` spell ids — CMSG_CANCEL_AURA.
    pub(crate) cancelled_auras: std::sync::Mutex<Vec<u32>>,
    /// Recorded `cancel_cast` self_guids — CMSG_CANCEL_CAST.
    pub(crate) cancelled_casts: std::sync::Mutex<Vec<u64>>,
}

/// Cast operations for the shared adapter. It configures no cast state: the surviving encrypted
/// tests need one instant ordinary cast, the two auto-repeat spells, and a record of what each
/// cancellation asked for. Route variation — cast time, next-swing, ground area, enchant,
/// disenchant, fishing and lock opening — belongs to the cast seam's own focused adapter.
impl CastStore for WorldFake {
    fn cast_item_target(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _spell_id: u32,
        _slot: u8,
    ) -> Result<()> {
        Ok(())
    }

    fn cancel_aura(&self, _account_id: u64, _self_guid: u64, spell_id: u32) -> Result<()> {
        self.cast.cancelled_auras.lock().unwrap().push(spell_id);
        Ok(())
    }
    fn cancel_cast(&self, _account_id: u64, self_guid: u64) -> Result<()> {
        self.cast.cancelled_casts.lock().unwrap().push(self_guid);
        Ok(())
    }
    fn cast_spell(
        &self,
        _account_id: u64,
        _self_guid: u64,
        spell_id: u32,
        target_guid: u64,
    ) -> Result<()> {
        self.cast
            .casts
            .lock()
            .unwrap()
            .push((spell_id, target_guid));
        Ok(())
    }
    fn start_ranged_attack(
        &self,
        _account_id: u64,
        _self_guid: u64,
        target_guid: u64,
        spell_id: u32,
    ) -> Result<()> {
        self.cast
            .ranged_attacks
            .lock()
            .unwrap()
            .push((target_guid, spell_id));
        Ok(())
    }
    fn spell_is_ranged_auto_repeat(&self, spell_id: u32) -> bool {
        // Mirrors the real RANGED_AUTO_REPEAT cast_flags bit for the two vanilla auto-repeat abilities.
        matches!(spell_id, 75 | 5019)
    }
    /// Every other spell is an instant ordinary cast — the one route these tests send over a socket.
    fn spell_cast_time(&self, _spell_id: u32) -> Option<u32> {
        Some(0)
    }
    fn spell_queues_next_swing(&self, _spell_id: u32) -> bool {
        false
    }
    fn spell_is_ground_area(&self, _spell_id: u32) -> bool {
        false
    }
    fn enchant_route(&self, _spell_id: u32) -> Option<crate::world::EnchantRoute> {
        None
    }
    fn spell_is_fishing(&self, _spell_id: u32) -> bool {
        false
    }
    fn spell_is_open_lock(&self, _spell_id: u32) -> bool {
        false
    }
    fn cast_spell_at(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _spell_id: u32,
        _target_guid: u64,
        _x: f32,
        _y: f32,
        _z: f32,
    ) -> Result<()> {
        Ok(())
    }
    fn item_slot_by_guid(&self, _account_id: u64, _item_guid: u64) -> Option<u8> {
        None
    }
    fn disenchant_item(&self, _account_id: u64, _self_guid: u64, _slot: u8) -> Result<()> {
        Ok(())
    }
    fn enchant_item_on_slot(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _slot: u8,
        _enchant_id: u32,
    ) -> Result<()> {
        Ok(())
    }
    fn fish(&self, _account_id: u64, _self_guid: u64) -> Result<()> {
        Ok(())
    }
    fn pick_lock(&self, _account_id: u64, _self_guid: u64, _go_guid: u64) -> Result<()> {
        Ok(())
    }

    // Shared with the character, vendor and query paths, so these two keep real fixtures.

    fn player_items(&self, _owner_guid: u64) -> Result<Vec<codec::ItemInstanceView>> {
        Ok(self.cast.player_items_fixture.clone())
    }
    fn item_template(&self, entry: u32) -> Result<Option<codec::ItemTemplateView>> {
        Ok(self
            .cast
            .item_templates
            .iter()
            .find(|template| template.entry == entry)
            .cloned())
    }
}
