//! A Fake for the traits `handle_loot` is bounded on: Death, Loot Window, Npc, Shard Routing and
//! Loot Roll. Death and the Loot Window carry state. The other three are not reachable from the
//! tests that use it.

use crate::codec;
use crate::stdb::ReducerCallError;
use crate::world::handlers::{
    DeathStore, LootActionStatus, LootWindowRequestStatus, LootWindowStore, NpcStore,
};
use crate::world::loot::{LootRollStore, PendingLootRoll};
use crate::world::{Actor, ShardRoutingStore, WorldStore};
use anyhow::{anyhow, Result};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Life {
    Dead,
    Ghost,
    Alive,
}

#[derive(Default)]
pub(crate) struct HandleLootFake {
    life: Mutex<HashMap<u64, Life>>,
    /// Owner guid to the guid of the corpse that owner can reclaim.
    corpses: HashMap<u64, u64>,
    /// Characters with an unanswered resurrect offer.
    res_offers: Mutex<HashSet<u64>>,
    /// Characters holding an unspent Self-Resurrection Option.
    self_res_options: Mutex<HashSet<u64>>,
    spirit_healers: HashSet<u64>,
    loot_by_viewer: HashMap<u64, Vec<codec::LootItemView>>,
    /// When set, every Durable Request fails as a Transport Loss.
    transport_lost: bool,
}

impl HandleLootFake {
    pub(crate) fn with_life(self, guid: u64, life: Life) -> Self {
        self.life.lock().unwrap().insert(guid, life);
        self
    }

    pub(crate) fn with_corpse(mut self, owner: u64, corpse: u64) -> Self {
        self.corpses.insert(owner, corpse);
        self
    }

    pub(crate) fn with_res_offer(self, guid: u64) -> Self {
        self.res_offers.lock().unwrap().insert(guid);
        self
    }

    pub(crate) fn with_self_res_option(self, guid: u64) -> Self {
        self.self_res_options.lock().unwrap().insert(guid);
        self
    }

    pub(crate) fn with_spirit_healer(mut self, guid: u64) -> Self {
        self.spirit_healers.insert(guid);
        self
    }

    pub(crate) fn with_transport_loss(mut self) -> Self {
        self.transport_lost = true;
        self
    }

    pub(crate) fn with_loot(mut self, viewer: u64, items: Vec<codec::LootItemView>) -> Self {
        self.loot_by_viewer.insert(viewer, items);
        self
    }

    /// Characters nobody set up are alive.
    pub(crate) fn life(&self, guid: u64) -> Life {
        self.life
            .lock()
            .unwrap()
            .get(&guid)
            .copied()
            .unwrap_or(Life::Alive)
    }

    pub(crate) fn has_res_offer(&self, guid: u64) -> bool {
        self.res_offers.lock().unwrap().contains(&guid)
    }

    pub(crate) fn has_self_res_option(&self, guid: u64) -> bool {
        self.self_res_options.lock().unwrap().contains(&guid)
    }

    /// The Module refuses a request the character's state does not allow.
    fn require(&self, operation: &str, guid: u64, needed: Life) -> Result<()> {
        self.reachable(operation)?;
        match self.life(guid) {
            life if life == needed => Ok(()),
            life => Err(ReducerCallError::refused(
                operation,
                &format!("character {guid} is {life:?}, not {needed:?}"),
            )
            .into()),
        }
    }

    fn reachable(&self, operation: &str) -> Result<()> {
        if self.transport_lost {
            return Err(ReducerCallError::transport_lost(operation).into());
        }
        Ok(())
    }

    fn revive(&self, guid: u64) {
        self.life.lock().unwrap().insert(guid, Life::Alive);
    }
}

impl DeathStore for HandleLootFake {
    fn repop(&self, actor: Actor) -> Result<()> {
        let self_guid = actor.guid();
        self.require("gw_repop", self_guid, Life::Dead)?;
        self.revive(self_guid);
        Ok(())
    }

    fn reclaim_corpse(&self, actor: Actor, corpse_guid: u64) -> Result<()> {
        let self_guid = actor.guid();
        self.require("gw_reclaim_corpse", self_guid, Life::Ghost)?;
        if self.corpses.get(&self_guid) != Some(&corpse_guid) {
            return Err(ReducerCallError::refused(
                "gw_reclaim_corpse",
                &format!("corpse {corpse_guid} is not character {self_guid}'s"),
            )
            .into());
        }
        self.revive(self_guid);
        Ok(())
    }

    fn resurrect_response(&self, actor: Actor, accept: bool) -> Result<()> {
        let self_guid = actor.guid();
        self.reachable("gw_respond_resurrect")?;
        if !self.res_offers.lock().unwrap().remove(&self_guid) {
            return Err(ReducerCallError::refused(
                "gw_respond_resurrect",
                &format!("character {self_guid} has no pending offer"),
            )
            .into());
        }
        if accept {
            self.revive(self_guid);
        }
        Ok(())
    }

    fn self_resurrect(&self, actor: Actor) -> Result<()> {
        let self_guid = actor.guid();
        self.require("gw_self_resurrect", self_guid, Life::Dead)?;
        if !self.self_res_options.lock().unwrap().remove(&self_guid) {
            return Err(ReducerCallError::refused(
                "gw_self_resurrect",
                "no Self-Resurrection Option",
            )
            .into());
        }
        self.revive(self_guid);
        Ok(())
    }

    fn spirit_healer_res(&self, actor: Actor, healer_guid: u64) -> Result<()> {
        let self_guid = actor.guid();
        self.require("gw_spirit_res", self_guid, Life::Ghost)?;
        if !self.spirit_healers.contains(&healer_guid) {
            return Err(ReducerCallError::refused(
                "gw_spirit_res",
                &format!("{healer_guid} is not a Spirit Healer"),
            )
            .into());
        }
        self.revive(self_guid);
        Ok(())
    }

    fn corpse_location(&self, _owner_guid: u64) -> Result<Option<(u32, f32, f32, f32)>> {
        unreachable!("no test asks for a corpse location")
    }
}

impl LootWindowStore for HandleLootFake {
    fn loot_target_money(&self, _target_guid: u64) -> Result<u32> {
        unreachable!("handle_loot reads no loot money")
    }

    fn loot_target_items(
        &self,
        _target_guid: u64,
        viewer_guid: u64,
    ) -> Result<Vec<codec::LootItemView>> {
        Ok(self
            .loot_by_viewer
            .get(&viewer_guid)
            .cloned()
            .unwrap_or_default())
    }

    fn use_gameobject(&self, _actor: Actor, _target_guid: u64) -> Result<LootWindowRequestStatus> {
        Ok(LootWindowRequestStatus::Applied)
    }

    fn open_creature_loot(
        &self,
        _actor: Actor,
        _corpse_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        unreachable!("handle_loot opens no creature loot")
    }

    fn skin_corpse(&self, _actor: Actor, _target_guid: u64) -> Result<LootWindowRequestStatus> {
        unreachable!("handle_loot skins no corpse")
    }

    fn loot_money(&self, _actor: Actor, _target_guid: u64) -> Result<LootWindowRequestStatus> {
        unreachable!("handle_loot takes no money")
    }

    fn take_loot(
        &self,
        _actor: Actor,
        _target_guid: u64,
        _loot_slot: u8,
    ) -> Result<LootWindowRequestStatus> {
        unreachable!("handle_loot takes no item")
    }
}

impl NpcStore for HandleLootFake {
    fn creature_template(&self, _entry: u32) -> Result<Option<codec::CreatureView>> {
        unreachable!("no test reaches the NPC family")
    }

    fn pet_name(
        &self,
        _requester: Actor,
        _pet_number: u32,
        _pet_guid: u64,
    ) -> Result<Option<codec::PetNameView>> {
        unreachable!("no test reaches the NPC family")
    }

    fn gameobject_template(&self, _entry: u32) -> Result<Option<codec::GameObjectTemplateView>> {
        unreachable!("no test reaches the NPC family")
    }

    fn gameobject_type(&self, _go_guid: u64) -> Result<Option<u8>> {
        unreachable!("no test reaches the NPC family")
    }

    fn enter_areatrigger(&self, _actor: Actor, _trigger_id: u32) -> Result<()> {
        unreachable!("no test reaches the NPC family")
    }

    fn npc_refuses_interaction(&self, _npc_guid: u64, _player_guid: u64) -> Result<bool> {
        unreachable!("no test reaches the NPC family")
    }

    fn bind_home(&self, _actor: Actor) -> Result<()> {
        unreachable!("no test reaches the NPC family")
    }

    fn npc_is_innkeeper(&self, _guid: u64) -> Result<bool> {
        unreachable!("no test reaches the NPC family")
    }

    fn npc_gossip_text_id(&self, _npc_guid: u64) -> u32 {
        unreachable!("no test reaches the NPC family")
    }

    fn npc_text_for_id(&self, _text_id: u32) -> Option<codec::NpcTextView> {
        unreachable!("no test reaches the NPC family")
    }

    fn gossip_options(&self, _npc_guid: u64) -> Result<Vec<codec::GossipOptionView>> {
        unreachable!("no test reaches the NPC family")
    }

    fn inspect(&self, _actor: Actor, _target_guid: u64) -> Result<()> {
        unreachable!("no test reaches the NPC family")
    }

    fn gossip_select(
        &self,
        _actor: Actor,
        _npc_guid: u64,
        _option_id: u32,
        _option_row_id: u32,
    ) -> Result<()> {
        unreachable!("no test reaches the NPC family")
    }
}

impl LootRollStore for HandleLootFake {
    fn realm_loot_start(
        &self,
        _corpse_guid: u64,
        _slot: u8,
        _item_entry: u32,
        _deadline_micros: i64,
        _recipients: Vec<u64>,
        _random_property_id: u32,
        _promotion_source: spacetimedb_sdk::Identity,
        _source_roll_id: u64,
    ) -> Result<()> {
        unreachable!("no test rolls for loot")
    }

    fn realm_loot_vote(
        &self,
        _corpse_guid: u64,
        _slot: u8,
        _actor: Actor,
        _vote: u8,
    ) -> Result<LootActionStatus> {
        unreachable!("no test rolls for loot")
    }

    fn pending_local_rolls(&self) -> Result<Vec<PendingLootRoll>> {
        unreachable!("no test rolls for loot")
    }

    fn settle_loot_roll(&self, _corpse_guid: u64, _slot: u8, _winner_guid: u64) -> Result<()> {
        unreachable!("no test rolls for loot")
    }

    fn clear_promoted_loot_roll(&self, _roll_id: u64) -> Result<()> {
        unreachable!("no test rolls for loot")
    }

    fn loot_won_since(&self, _after_id: u64) -> Result<(u64, Vec<(u64, u8, u64)>)> {
        unreachable!("no test rolls for loot")
    }

    fn loot_roll(
        &self,
        _actor: Actor,
        _corpse_guid: u64,
        _loot_slot: u32,
        _vote: u8,
    ) -> Result<LootActionStatus> {
        unreachable!("no test rolls for loot")
    }

    fn loot_master_give(
        &self,
        _actor: Actor,
        _corpse_guid: u64,
        _loot_slot: u8,
        _target_guid: u64,
    ) -> Result<LootActionStatus> {
        unreachable!("no test rolls for loot")
    }
}

/// A single-database gateway: every handle serves every location, and nothing routes.
impl ShardRoutingStore for HandleLootFake {
    fn shard_name(&self) -> &str {
        "handle-loot-fake"
    }

    fn settle_home_shard(&self, _character_guid: u64) -> Result<Option<Arc<dyn WorldStore>>> {
        Ok(None)
    }

    fn shard_for_location(&self, _map_id: u32, _instance_id: u64) -> Option<Arc<dyn WorldStore>> {
        None
    }

    fn bind_shard_session(&self, _account_id: u64, _session_key: &[u8; 40]) -> Result<()> {
        Ok(())
    }

    fn realm_store(&self) -> Option<Arc<dyn WorldStore>> {
        None
    }

    fn party_cleanup_realm(&self) -> Result<Option<Arc<dyn WorldStore>>> {
        Ok(None)
    }

    fn party_command_realm(&self) -> Result<Option<Arc<dyn WorldStore>>> {
        Ok(None)
    }

    fn transfer_realm(&self) -> Result<Option<Arc<dyn WorldStore>>> {
        Ok(None)
    }

    fn world_stores(&self) -> Vec<Arc<dyn WorldStore>> {
        Vec::new()
    }

    fn party_command_worlds(&self) -> Result<Vec<Arc<dyn WorldStore>>> {
        Ok(Vec::new())
    }
}
