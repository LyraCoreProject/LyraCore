//! `Coordinator`'s [`RealmDb`] adapter.

use anyhow::{anyhow, Result};
use spacetimedb_sdk::Table;

use crate::realm_core::{RealmDb, SessionKey};
use crate::stdb::bindings::*;
use crate::stdb::connection::call_reducer;
use crate::stdb::{AccountRow, Coordinator, RealmRow};
use crate::world::Actor;

/// `realm_core.rs`'s Fake replaces this whole block. Reads that other code paths also use stay
/// inherent on `Coordinator`; a Realm-only read or reducer call lives here.
impl RealmDb for Coordinator {
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

    /// The Module combines the conveyed Account authority with its own Character GM level and
    /// remains the final Gate. Its Refusal reason is the text the GM sees.
    fn request_gm_command(&self, actor: Actor, alpha_test_tools: bool, text: String) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_gm_command",
            gw_gm_command_then(self.session_actor(actor), alpha_test_tools, text)
        )
    }

    fn character_location(&self, guid: u64) -> Option<(u32, u64)> {
        self.character_location(guid)
    }

    /// Call it on the Realm-core handle: on a World Shard `finish_transfer` maintains the index
    /// in the same transaction.
    fn set_character_shard(&self, guid: u64, map_id: u32, instance_id: u64) -> Result<()> {
        let actor = character_actor(self, guid)?;
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "set_character_shard",
            set_character_shard_then(guid, map_id, instance_id, actor)
        )
    }

    fn realm_character_partition(
        &self,
        guid: u64,
    ) -> Result<Option<crate::world::party::RealmCharacterPartition>> {
        self.realm_character_partition(guid)
    }

    // The arguments mirror the Realm reducer's exact Transfer Gate.
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
        let actor = character_actor(self, character_guid)?;
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "begin_character_shard_transfer",
            begin_character_shard_transfer_then(
                character_guid,
                source_map,
                source_instance,
                source_revision,
                destination_map,
                destination_instance,
                source_module_identity,
                intent_id,
                controller_generation,
                actor
            )
        )
    }

    fn finish_character_shard_transfer(
        &self,
        intent: &crate::world::transfer::BotTransferIntent,
    ) -> Result<()> {
        let actor = character_actor(self, intent.bot_guid)?;
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "finish_character_shard_transfer",
            finish_character_shard_transfer_then(
                intent.bot_guid,
                intent.source_map,
                intent.source_instance,
                intent.source_locator_revision,
                intent.destination_map,
                intent.destination_instance,
                intent.source_module_identity,
                intent.id,
                intent.controller_generation,
                actor
            )
        )
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
        let actor = character_actor(self, character_guid)?;
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "finish_character_shard_transfer",
            finish_character_shard_transfer_then(
                character_guid,
                source_map,
                source_instance,
                source_revision,
                destination_map,
                destination_instance,
                spacetimedb_sdk::Identity::ZERO,
                0,
                0,
                actor
            )
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
        let actor = character_actor(self, character_guid)?;
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "finish_pending_character_shard_transfer",
            finish_pending_character_shard_transfer_then(
                character_guid,
                source_map,
                source_instance,
                source_revision,
                destination_map,
                destination_instance,
                source_module_identity,
                transfer_intent_id,
                controller_generation,
                actor
            )
        )
    }

    fn has_escrow(&self, guid: u64) -> bool {
        self.escrow_row(guid).is_some()
    }

    /// Player entities (`account_id != 0`) in this shard's cache. Shard truth, so every Gateway
    /// reports the same number.
    fn session_count(&self) -> usize {
        self.0
            .coord()
            .conn
            .db
            .game_world_entity()
            .iter()
            .filter(|e| e.account_id != 0)
            .count()
    }

    /// Callers hold the Realm-core handle: only Realm-core reads `game_shard_load`.
    fn record_shard_load(
        &self,
        shard: &str,
        writer_occupancy_pct: f32,
        sessions: u32,
        gateway_key: u64,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "record_shard_load",
            record_shard_load_then(
                shard.to_string(),
                writer_occupancy_pct,
                sessions,
                gateway_key
            )
        )
    }
}

/// The Realm index reducers act for the Character they move.
fn character_actor(coordinator: &Coordinator, character_guid: u64) -> Result<SessionActor> {
    let actor = Actor::new(character_guid)
        .ok_or_else(|| anyhow!("Character guid 0 cannot act in the Realm index"))?;
    Ok(coordinator.session_actor(actor))
}
