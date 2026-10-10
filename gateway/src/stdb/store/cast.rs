//! `Coordinator`'s [`CastStore`] adapter.

use anyhow::{anyhow, Result};
use spacetimedb_sdk::Table;

use crate::codec;
use crate::stdb::bindings::*;
use crate::stdb::connection::call_reducer;
use crate::stdb::views::item_template_view;
use crate::stdb::Coordinator;
use crate::world::{Actor, CastStore, EnchantRoute};

impl CastStore for crate::stdb::Coordinator {
    fn spell_is_ranged_auto_repeat(&self, spell_id: u32) -> bool {
        crate::stdb::Coordinator::spell_is_ranged_auto_repeat(self, spell_id)
    }

    fn enchant_route(&self, spell_id: u32) -> Option<EnchantRoute> {
        crate::stdb::Coordinator::enchant_route(self, spell_id)
    }

    fn spell_is_fishing(&self, spell_id: u32) -> bool {
        crate::stdb::Coordinator::spell_is_fishing(self, spell_id)
    }

    fn spell_is_open_lock(&self, spell_id: u32) -> bool {
        crate::stdb::Coordinator::spell_is_open_lock(self, spell_id)
    }

    fn spell_is_ground_area(&self, spell_id: u32) -> bool {
        crate::stdb::Coordinator::spell_is_ground_area(self, spell_id)
    }

    fn spell_cast_time(&self, spell_id: u32) -> Option<u32> {
        crate::stdb::Coordinator::spell_cast_time(self, spell_id)
    }

    fn spell_queues_next_swing(&self, spell_id: u32) -> bool {
        crate::stdb::Coordinator::spell_queues_next_swing(self, spell_id)
    }

    fn cast_spell(
        &self,
        account_id: u64,
        self_guid: u64,
        spell_id: u32,
        target_guid: u64,
    ) -> Result<()> {
        crate::stdb::Coordinator::cast_spell(self, account_id, self_guid, spell_id, target_guid)
    }

    fn cast_spell_at(
        &self,
        account_id: u64,
        self_guid: u64,
        spell_id: u32,
        target_guid: u64,
        x: f32,
        y: f32,
        z: f32,
    ) -> Result<()> {
        crate::stdb::Coordinator::cast_spell_at(
            self,
            account_id,
            self_guid,
            spell_id,
            target_guid,
            x,
            y,
            z,
        )
    }

    fn cast_item_target(
        &self,
        account_id: u64,
        self_guid: u64,
        spell_id: u32,
        slot: u8,
    ) -> Result<()> {
        crate::stdb::Coordinator::cast_item_target(self, account_id, self_guid, spell_id, slot)
    }

    fn start_ranged_attack(
        &self,
        account_id: u64,
        self_guid: u64,
        target_guid: u64,
        spell_id: u32,
    ) -> Result<()> {
        crate::stdb::Coordinator::start_ranged_attack(
            self,
            account_id,
            self_guid,
            target_guid,
            spell_id,
        )
    }

    fn item_slot_by_guid(&self, account_id: u64, item_guid: u64) -> Option<u8> {
        crate::stdb::Coordinator::item_slot_by_guid(self, account_id, item_guid)
    }

    fn disenchant_item(&self, account_id: u64, self_guid: u64, slot: u8) -> Result<()> {
        crate::stdb::Coordinator::disenchant_item(self, account_id, self_guid, slot)
    }

    fn enchant_item_on_slot(
        &self,
        account_id: u64,
        self_guid: u64,
        slot: u8,
        enchant_id: u32,
    ) -> Result<()> {
        crate::stdb::Coordinator::enchant_item_on_slot(
            self, account_id, self_guid, slot, enchant_id,
        )
    }

    fn fish(&self, account_id: u64, self_guid: u64) -> Result<()> {
        crate::stdb::Coordinator::fish(self, account_id, self_guid)
    }

    fn pick_lock(&self, account_id: u64, self_guid: u64, go_guid: u64) -> Result<()> {
        crate::stdb::Coordinator::pick_lock(self, account_id, self_guid, go_guid)
    }

    fn cancel_cast(&self, account_id: u64, self_guid: u64) -> Result<()> {
        crate::stdb::Coordinator::cancel_cast(self, account_id, self_guid)
    }

    fn cancel_aura(&self, account_id: u64, self_guid: u64, spell_id: u32) -> Result<()> {
        crate::stdb::Coordinator::cancel_aura(self, account_id, self_guid, spell_id)
    }

    fn player_items(&self, owner_guid: u64) -> Result<Vec<codec::ItemInstanceView>> {
        crate::stdb::Coordinator::player_items(self, owner_guid)
    }

    fn item_template(&self, entry: u32) -> Result<Option<codec::ItemTemplateView>> {
        crate::stdb::Coordinator::item_template(self, entry)
    }
}

impl Coordinator {
    /// The spell's cast time (ms) from the static `game_spell` header — 0 = instant. Used by the
    /// CMSG_CAST_SPELL handler to clear an instant cast SYNCHRONOUSLY (the async relay delivers START/GO
    /// after the aura effects, which wedges the 5875 client's cast slot). None = unknown spell.
    pub fn spell_cast_time(&self, spell_id: u32) -> Option<u32> {
        self.0
            .coord()
            .conn
            .db
            .game_spell()
            .spell_id()
            .find(&spell_id)
            .map(|s| s.cast_time_ms)
    }

    /// Enchant/disenchant routing for `spell_id`, read off the spell's effect rows — `None` if the spell
    /// has no item-target enchanting effect (a normal cast). The gateway uses this to route ITEM-target
    /// casts by effect KIND, with the enchant id carried in the effect's `p_0`, instead of a hardcoded
    /// spell-id list — a new enchant spell is a data row, no gateway change.
    pub fn enchant_route(&self, spell_id: u32) -> Option<crate::world::EnchantRoute> {
        use crate::world::EnchantRoute;
        const E_ENCHANT_ITEM: u8 = 0x17; // taxonomy E_ENCHANT_ITEM (p_0 = enchant_id)
        const E_DISENCHANT: u8 = 0x18; // taxonomy E_DISENCHANT
        self.0
            .coord()
            .conn
            .db
            .game_spell_effect()
            .iter()
            .filter(|e| e.spell_id == spell_id)
            .find_map(|e| match e.kind {
                E_ENCHANT_ITEM => Some(EnchantRoute::Enchant(e.p_0 as u32)),
                E_DISENCHANT => Some(EnchantRoute::Disenchant),
                _ => None,
            })
    }

    /// True iff `spell_id` has a ground-area effect (taxonomy `E_PERSISTENT_AREA` 0x1B —
    /// Consecration etc.). The instant-cast GO for such a spell must carry an EMPTY hit list: the
    /// default self-cast fallback put the CASTER in `hits[]`, and the 5875 client rendered the
    /// spell's impact animation ON the paladin ("hit animation on the caster"). Kind-routed
    /// like `enchant_route` — a new ground spell is a data row.
    pub fn spell_is_ground_area(&self, spell_id: u32) -> bool {
        const E_PERSISTENT_AREA: u8 = 0x1B; // lockstep with module taxonomy
        self.0
            .coord()
            .conn
            .db
            .game_spell_effect()
            .iter()
            .any(|e| e.spell_id == spell_id && e.kind == E_PERSISTENT_AREA)
    }

    /// True iff `spell_id` is a FISHING cast (taxonomy `E_FISH` 0x1C — the three tier ids carry a
    /// synthesized marker effect row). Kind-routed like `enchant_route`/`spell_is_ground_area`.
    pub fn spell_is_fishing(&self, spell_id: u32) -> bool {
        const E_FISH: u8 = 0x1C; // lockstep with module taxonomy
        self.0
            .coord()
            .conn
            .db
            .game_spell_effect()
            .iter()
            .any(|e| e.spell_id == spell_id && e.kind == E_FISH)
    }

    /// True iff `spell_id` is an OPEN-LOCK cast (taxonomy `E_OPEN_LOCK` 0x1D — Pick Lock 1804). Routed to
    /// the `pick_lock` reducer, gateway-intercepted exactly like `spell_is_fishing`/`enchant_route`. The
    /// GO guid the pick targets rides the cast's SpellCastTargets (GAMEOBJECT flag).
    pub fn spell_is_open_lock(&self, spell_id: u32) -> bool {
        const E_OPEN_LOCK: u8 = 0x1D; // lockstep with module taxonomy
        self.0
            .coord()
            .conn
            .db
            .game_spell_effect()
            .iter()
            .any(|e| e.spell_id == spell_id && e.kind == E_OPEN_LOCK)
    }

    /// Resolve a trainer offering's LEARN TARGET (the rank the buy actually granted) so
    /// SMSG_LEARNED_SPELL books the REAL spell, not the LearnSpell wrapper (live find 2026-07-11:
    /// "Devotion Aura appeared in my General tab as the spell that teaches Devotion Aura").
    /// LOCKSTEP with module trainer.rs::resolve_learn_target — the same first-qualifying-trigger
    /// rule; the excluded kinds are the module's taxonomy values (A_PERIODIC_TRIGGER 0x93,
    /// A_FLAG 0xBE, A_PROC_TRIGGER 0xAB, E_TRIGGER 0x05).
    /// True iff `spell_id` is an on-next-swing QUEUE spell (Heroic Strike/Cleave — any effect of kind
    /// E_NEXT_SWING). The CMSG_CAST_SPELL handler then sends NO synchronous START/CAST_RESULT/GO: the
    /// 5875 client lights the button locally on the press and holds it as a pending cast until the
    /// swing-fire GO arrives. Kind value LOCKSTEP with module taxonomy.rs::E_NEXT_SWING (0x13).
    pub fn spell_queues_next_swing(&self, spell_id: u32) -> bool {
        const E_NEXT_SWING: u8 = 0x13;
        let guard = self.0.coord();
        let queues = guard
            .conn
            .db
            .game_spell_effect()
            .iter()
            .any(|e| e.spell_id == spell_id && e.kind == E_NEXT_SWING);
        queues
    }

    /// True iff `spell_id` is an AUTO-REPEAT ranged attack (Auto Shot / wand Shoot) — the
    /// `SPELL_ATTR_RANGED_AUTO_REPEAT` cast_flags bit set by the importer (from the DBC AttributesEx2
    /// AUTOREPEAT bit / by name). The CMSG_CAST_SPELL handler routes on this instead of a hardcoded
    /// `spell == 75 || 5019` id list, so a new ranged auto-repeat ability onboards as data.
    pub fn spell_is_ranged_auto_repeat(&self, spell_id: u32) -> bool {
        const SPELL_ATTR_RANGED_AUTO_REPEAT: u32 = 0x0200; // lockstep with importer::spell.rs
        let guard = self.0.coord();
        guard
            .conn
            .db
            .game_spell()
            .spell_id()
            .find(&spell_id)
            .is_some_and(|s| s.cast_flags & SPELL_ATTR_RANGED_AUTO_REPEAT != 0)
    }

    /// Read an item template by entry for a `CMSG_ITEM_QUERY_SINGLE` reply.
    pub fn item_template(&self, entry: u32) -> Result<Option<crate::codec::ItemTemplateView>> {
        Ok(self
            .0
            .coord()
            .conn
            .db
            .game_item_template()
            .entry()
            .find(&entry)
            .map(item_template_view))
    }

    /// Start the player's RANGED auto-attack on `target_guid` with `spell_id` (75 Auto Shot / 5019 Shoot)
    /// over the coordinator connection so the module attributes the shot to the caller.
    /// Rides the coordinator connection as `gw_ranged_attack`.
    pub fn start_ranged_attack(
        &self,
        _account_id: u64,
        actor_guid: u64,
        target_guid: u64,
        spell_id: u32,
    ) -> Result<()> {
        let actor = Actor::new(actor_guid)
            .ok_or_else(|| anyhow!("start_ranged_attack: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_ranged_attack",
            gw_ranged_attack_then(self.session_actor(actor), target_guid, spell_id)
        )
    }

    /// Cast a spell (`CMSG_CAST_SPELL`, aura tracer) over the coordinator connection so the module
    /// attributes the cast to the caller. `target_guid` is the client's selected unit (0 = none/self →
    /// the module substitutes the caster), threaded so target-keyed effects see the real target.
    pub fn cast_spell(
        &self,
        _account_id: u64,
        actor_guid: u64,
        spell_id: u32,
        target_guid: u64,
    ) -> Result<()> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("cast_spell: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_cast_spell",
            gw_cast_spell_then(self.session_actor(actor), spell_id, target_guid)
        )
    }

    /// Resolve an item-target cast through the module's generic item-effect seam.
    pub fn cast_item_target(
        &self,
        _account_id: u64,
        actor_guid: u64,
        spell_id: u32,
        slot: u8,
    ) -> Result<()> {
        let actor = Actor::new(actor_guid)
            .ok_or_else(|| anyhow!("cast_item_target: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_cast_item_target",
            gw_cast_item_target_then(self.session_actor(actor), spell_id, slot)
        )
    }

    /// Cast a GROUND-TARGETED spell at a clicked world point (`CMSG_CAST_SPELL` with a DEST_LOCATION —
    /// Flamestrike/Blizzard/Rain of Fire). Same per-account attribution as `cast_spell`; the `(x,y,z)` is
    /// the ground click so the module anchors the AoE/patch there.
    // The ground point rides flat because it mirrors the `gw_cast_spell_at` reducer signature.
    #[allow(clippy::too_many_arguments)]
    pub fn cast_spell_at(
        &self,
        _account_id: u64,
        actor_guid: u64,
        spell_id: u32,
        target_guid: u64,
        x: f32,
        y: f32,
        z: f32,
    ) -> Result<()> {
        let actor = Actor::new(actor_guid)
            .ok_or_else(|| anyhow!("cast_spell_at: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_cast_spell_at",
            gw_cast_spell_at_then(self.session_actor(actor), spell_id, target_guid, x, y, z)
        )
    }

    /// Cancel one of the caller's own auras by spell id (`CMSG_CANCEL_AURA`) over the coordinator
    /// connection so the module attributes the removal to the caller.
    pub fn cancel_aura(&self, _account_id: u64, actor_guid: u64, spell_id: u32) -> Result<()> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("cancel_aura: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_cancel_aura",
            gw_cancel_aura_then(self.session_actor(actor), spell_id)
        )
    }

    /// Cancel the caller's in-progress cast (`CMSG_CANCEL_CAST`) over the coordinator connection so the
    /// module clears the caller's pending cast — no phantom completion GO.
    pub fn cancel_cast(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("cancel_cast: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_cancel_cast",
            gw_cancel_cast_then(self.session_actor(actor))
        )
    }

    pub fn disenchant_item(&self, _account_id: u64, actor_guid: u64, slot: u8) -> Result<()> {
        let actor = Actor::new(actor_guid)
            .ok_or_else(|| anyhow!("disenchant_item: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_disenchant",
            gw_disenchant_then(self.session_actor(actor), slot)
        )
    }

    pub fn enchant_item_on_slot(
        &self,
        _account_id: u64,
        actor_guid: u64,
        slot: u8,
        enchant_id: u32,
    ) -> Result<()> {
        let actor = Actor::new(actor_guid)
            .ok_or_else(|| anyhow!("enchant_item_on_slot: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_enchant_item",
            gw_enchant_item_then(self.session_actor(actor), slot, enchant_id)
        )
    }

    /// Fishing cast: instant-resolve catch — the module's lenient alpha gate auto-learns the
    /// skill and grants the fish straight to the bag. Caller resolved via ctx.sender.
    pub fn fish(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        let actor = Actor::new(actor_guid).ok_or_else(|| anyhow!("fish: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_fish",
            gw_fish_then(self.session_actor(actor))
        )
    }

    /// Pick Lock: unlock the locked GameObject `go_guid` over the coordinator connection (so the
    /// module attributes the pick to the caller via ctx.sender). The module gates range / lock
    /// requirement / Lockpicking skill; on success it records the GO unlocked + climbs the skill.
    pub fn pick_lock(&self, _account_id: u64, actor_guid: u64, go_guid: u64) -> Result<()> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("pick_lock: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_pick_lock",
            gw_pick_lock_then(self.session_actor(actor), go_guid)
        )
    }
}
