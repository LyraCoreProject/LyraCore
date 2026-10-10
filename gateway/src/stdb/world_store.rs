//! `Coordinator`'s [`RealmDb`](crate::realm_core::RealmDb) adapter.

use anyhow::Result;

use crate::realm_core::SessionKey;

use super::connection::Coordinator;
use super::views::{AccountRow, RealmRow};

/// The realm-core split's store seam. Every method here is a view onto
/// `Coordinator`'s like-named INHERENT method — Rust resolves inherent methods first, so these are
/// forwards, not recursion, exactly like the Store family impls in `stdb::store`.
///
/// This block is the ONE layer `realm_core.rs`'s fake substitutes for wholesale. Keep it a block of
/// forwards; any logic that grows here is untested by construction. `has_escrow` narrows
/// `Option<TransferOut>` to a bool.
impl crate::realm_core::RealmDb for Coordinator {
    fn shard_name(&self) -> &str {
        self.shard_name()
    }
    fn is_sharded(&self) -> bool {
        self.is_sharded()
    }
    fn shard_map(&self) -> &crate::config::ShardMap {
        self.shard_map()
    }
    fn realm_core(&self) -> Result<Coordinator> {
        self.realm_core()
    }
    fn world_shards(&self) -> Vec<(String, Coordinator)> {
        self.world_shards()
    }
    fn account_by_username(&self, username: &str) -> Result<Option<AccountRow>> {
        self.account_by_username(username)
    }
    fn session_key(&self, account_id: u64) -> Result<Option<SessionKey>> {
        self.session_key(account_id)
    }
    fn bound_identity(&self, account_id: u64) -> Result<[u8; 32]> {
        self.bound_identity(account_id)
    }
    fn character_count(&self, account_id: u64) -> Result<u8> {
        self.character_count(account_id)
    }
    fn realm(&self) -> Result<RealmRow> {
        self.realm()
    }
    fn establish_session(
        &self,
        account_id: u64,
        session_key: &[u8; 40],
        bound_identity: [u8; 32],
    ) -> Result<()> {
        self.establish_session(account_id, session_key, bound_identity)
    }
    fn request_gm_command(
        &self,
        actor_guid: u64,
        alpha_test_tools: bool,
        text: String,
    ) -> Result<()> {
        self.request_gm_command(actor_guid, alpha_test_tools, text)
    }
    fn character_location(&self, guid: u64) -> Option<(u32, u64)> {
        self.character_location(guid)
    }
    fn character_shard(&self, guid: u64) -> Option<(u32, u64)> {
        self.character_shard(guid)
    }
    fn set_character_shard(&self, guid: u64, map_id: u32, instance_id: u64) -> Result<()> {
        self.set_character_shard(guid, map_id, instance_id)
    }
    fn realm_character_partition(
        &self,
        guid: u64,
    ) -> Result<Option<crate::world::party::RealmCharacterPartition>> {
        self.realm_character_partition(guid)
    }
    fn begin_character_shard_transfer(
        &self,
        source_map: u32,
        source_instance: u64,
        source_revision: u64,
        destination_map: u32,
        destination_instance: u64,
        source_module_identity: spacetimedb_sdk::Identity,
        intent_id: u64,
        controller_generation: u64,
        character_guid: u64,
    ) -> Result<()> {
        self.begin_character_shard_transfer(
            source_map,
            source_instance,
            source_revision,
            destination_map,
            destination_instance,
            source_module_identity,
            intent_id,
            controller_generation,
            character_guid,
        )
    }
    fn finish_character_shard_transfer(
        &self,
        intent: &crate::world::transfer::BotTransferIntent,
    ) -> Result<()> {
        self.finish_character_shard_transfer(intent)
    }
    fn finish_player_character_shard_transfer(
        &self,
        character_guid: u64,
        source_map: u32,
        source_instance: u64,
        source_revision: u64,
        destination_map: u32,
        destination_instance: u64,
    ) -> Result<()> {
        self.finish_player_character_shard_transfer(
            character_guid,
            source_map,
            source_instance,
            source_revision,
            destination_map,
            destination_instance,
        )
    }
    fn finish_pending_character_shard_transfer(
        &self,
        character_guid: u64,
        source_map: u32,
        source_instance: u64,
        source_revision: u64,
        destination_map: u32,
        destination_instance: u64,
        source_module_identity: spacetimedb_sdk::Identity,
        transfer_intent_id: u64,
        controller_generation: u64,
    ) -> Result<()> {
        self.finish_pending_character_shard_transfer(
            character_guid,
            source_map,
            source_instance,
            source_revision,
            destination_map,
            destination_instance,
            source_module_identity,
            transfer_intent_id,
            controller_generation,
        )
    }
    fn has_escrow(&self, guid: u64) -> bool {
        self.escrow_row(guid).is_some()
    }
    fn session_count(&self) -> usize {
        self.session_count()
    }
    fn record_shard_load(
        &self,
        shard: &str,
        writer_occupancy_pct: f32,
        sessions: u32,
        gateway_key: u64,
    ) -> Result<()> {
        self.record_shard_load(shard, writer_occupancy_pct, sessions, gateway_key)
    }
}
