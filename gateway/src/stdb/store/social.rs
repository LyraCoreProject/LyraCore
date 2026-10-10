//! `Coordinator`'s [`SocialStore`] adapter.

use anyhow::Result;

use crate::codec;
use crate::world::SocialStore;

use crate::stdb::Coordinator;

impl SocialStore for Coordinator {
    fn character_identity(
        &self,
        guid: u64,
    ) -> Result<Option<crate::world::presence::CharacterIdentity>> {
        self.character_identity(guid)
    }

    fn live_entity(&self, guid: u64) -> Option<codec::MemberEntity> {
        self.live_entity(guid)
    }

    fn character_in_transit(&self, guid: u64) -> bool {
        self.character_in_transit(guid)
    }

    fn auto_reply_text(&self, guid: u64) -> Result<Option<String>> {
        Ok(self.auto_reply_text(guid))
    }

    fn every_shard_vouches_for_absence(&self) -> Result<()> {
        self.world_shards_for_absence()?;
        Ok(())
    }

    fn in_world_players(&self) -> Result<Vec<crate::world::presence::RealmPresence>> {
        self.in_world_players()
    }

    fn zone_name(&self, zone_id: u32) -> String {
        self.zone_name(zone_id)
    }

    fn contact_lists(&self, self_guid: u64) -> Result<(Vec<u64>, Vec<u64>)> {
        self.contact_lists(self_guid)
    }

    fn ignored_guids(&self, owner_guid: u64) -> Result<Vec<u64>> {
        self.ignored_guids(owner_guid)
    }

    fn character_guid_by_name(&self, name: &str) -> Result<Option<u64>> {
        self.character_guid_by_name(name)
    }

    fn add_friend(
        &self,
        account_id: u64,
        self_guid: u64,
        target_guid: u64,
        target_race: u8,
    ) -> Result<crate::world::ContactOutcome> {
        self.add_friend(account_id, self_guid, target_guid, target_race)
    }

    fn del_friend(
        &self,
        account_id: u64,
        self_guid: u64,
        target_guid: u64,
    ) -> Result<crate::world::ContactOutcome> {
        self.del_friend(account_id, self_guid, target_guid)
    }

    fn add_ignore(
        &self,
        account_id: u64,
        self_guid: u64,
        target_guid: u64,
    ) -> Result<crate::world::ContactOutcome> {
        self.add_ignore(account_id, self_guid, target_guid)
    }

    fn del_ignore(
        &self,
        account_id: u64,
        self_guid: u64,
        target_guid: u64,
    ) -> Result<crate::world::ContactOutcome> {
        self.del_ignore(account_id, self_guid, target_guid)
    }
}
