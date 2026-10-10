//! `Coordinator`'s [`NpcStore`] adapter.

use anyhow::Result;

use crate::codec;
use crate::world::NpcStore;

use crate::stdb::Coordinator;

impl NpcStore for Coordinator {
    fn creature_template(&self, entry: u32) -> Result<Option<codec::CreatureView>> {
        self.creature_template(entry)
    }

    fn pet_name(
        &self,
        requester_guid: u64,
        pet_number: u32,
        pet_guid: u64,
    ) -> Result<Option<codec::PetNameView>> {
        self.pet_name(requester_guid, pet_number, pet_guid)
    }

    fn gameobject_template(&self, entry: u32) -> Result<Option<codec::GameObjectTemplateView>> {
        self.gameobject_template(entry)
    }

    fn gameobject_type(&self, go_guid: u64) -> Result<Option<u8>> {
        self.gameobject_type(go_guid)
    }

    fn enter_areatrigger(&self, account_id: u64, self_guid: u64, trigger_id: u32) -> Result<()> {
        self.enter_areatrigger(account_id, self_guid, trigger_id)
    }

    fn npc_refuses_interaction(&self, npc_guid: u64, player_guid: u64) -> Result<bool> {
        self.npc_refuses_interaction(npc_guid, player_guid)
    }

    fn bind_home(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.bind_home(account_id, self_guid)
    }

    fn npc_is_innkeeper(&self, guid: u64) -> Result<bool> {
        self.npc_is_innkeeper(guid)
    }

    fn npc_gossip_text_id(&self, npc_guid: u64) -> u32 {
        self.npc_gossip_text_id(npc_guid)
    }

    fn npc_text_for_id(&self, text_id: u32) -> Option<codec::NpcTextView> {
        self.npc_text_for_id(text_id)
    }

    fn gossip_options(&self, npc_guid: u64) -> Result<Vec<codec::GossipOptionView>> {
        self.gossip_options(npc_guid)
    }

    fn inspect(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()> {
        self.inspect(account_id, self_guid, target_guid)
    }

    fn gossip_select(
        &self,
        account_id: u64,
        self_guid: u64,
        npc_guid: u64,
        option_id: u32,
        option_row_id: u32,
    ) -> Result<()> {
        self.gossip_select(account_id, self_guid, npc_guid, option_id, option_row_id)
    }
}
