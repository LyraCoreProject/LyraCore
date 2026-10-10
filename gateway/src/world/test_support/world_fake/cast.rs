use super::super::*;

#[derive(Default)]
pub(crate) struct CastState {
    /// Imported item templates the query path resolves by entry. Keeping this keyed fixture here
    /// makes socket tests exercise the same durable read shape as the Coordinator cache.
    pub(crate) item_templates: Vec<codec::ItemTemplateView>,
    /// The caller's owned items, read by world entry.
    pub(crate) player_items_fixture: Vec<codec::ItemInstanceView>,
}

/// Cast behaviour is tested through `InMemoryCasts`. Here every cast operation succeeds and every
/// spell reads as an unknown, ordinary one; only the two item reads world entry makes are live.
impl CastStore for WorldFake {
    fn cast_item_target(&self, _actor: Actor, _spell_id: u32, _slot: u8) -> Result<()> {
        Ok(())
    }

    fn cancel_aura(&self, _actor: Actor, _spell_id: u32) -> Result<()> {
        Ok(())
    }
    fn cancel_cast(&self, _actor: Actor) -> Result<()> {
        Ok(())
    }
    fn cast_spell(&self, _actor: Actor, _spell_id: u32, _target_guid: u64) -> Result<()> {
        Ok(())
    }
    fn start_ranged_attack(&self, _actor: Actor, _target_guid: u64, _spell_id: u32) -> Result<()> {
        Ok(())
    }
    fn spell_is_ranged_auto_repeat(&self, _spell_id: u32) -> bool {
        false
    }
    fn spell_cast_time(&self, _spell_id: u32) -> Option<u32> {
        None
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
        _actor: Actor,
        _spell_id: u32,
        _target_guid: u64,
        _x: f32,
        _y: f32,
        _z: f32,
    ) -> Result<()> {
        Ok(())
    }
    fn item_slot_by_guid(&self, _item_guid: u64) -> Option<u8> {
        None
    }
    fn disenchant_item(&self, _actor: Actor, _slot: u8) -> Result<()> {
        Ok(())
    }
    fn enchant_item_on_slot(&self, _actor: Actor, _slot: u8, _enchant_id: u32) -> Result<()> {
        Ok(())
    }
    fn fish(&self, _actor: Actor) -> Result<()> {
        Ok(())
    }
    fn pick_lock(&self, _actor: Actor, _go_guid: u64) -> Result<()> {
        Ok(())
    }

    // Shared with the character, vendor and query paths, so these two keep real fixtures.

    fn player_items(&self, _owner_guid: u64) -> Result<Vec<codec::ItemInstanceView>> {
        if let Some(state) = &self.benilla_gameplay {
            return Ok(state.lock().unwrap().inventory.clone());
        }
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
