//! `Coordinator`'s [`TransferStore`] adapter.

use anyhow::Result;

use crate::world::TransferStore;

use crate::stdb::bindings::game_instance_table::GameInstanceTableAccess;
use crate::stdb::bindings::game_transfer_in_table::GameTransferInTableAccess;
use crate::stdb::Coordinator;

impl TransferStore for Coordinator {
    fn escrowed_transfer(
        &self,
        character_guid: u64,
    ) -> Option<crate::world::transfer::EscrowedTransfer> {
        self.escrow_row(character_guid)
            .map(|r| crate::world::transfer::EscrowedTransfer {
                transfer_id: r.transfer_id,
                character_guid: r.character_guid,
                dest_map_id: r.dest_map_id,
                dest_instance_id: r.dest_instance_id,
                blob: r.blob,
            })
    }

    fn character_destination(
        &self,
        character_guid: u64,
    ) -> Option<crate::world::transfer::TransferPlan> {
        let c = self.character_row(character_guid)?;
        Some(crate::world::transfer::TransferPlan {
            transfer_id: crate::world::transfer::transfer_id_for(character_guid),
            character_guid,
            dest_map_id: c.map_id,
            dest_instance_id: c.pending_instance_id,
            dest_x: c.x,
            dest_y: c.y,
            dest_z: c.z,
            dest_o: c.orientation,
        })
    }

    fn begin_transfer(&self, plan: &crate::world::transfer::TransferPlan) -> Result<()> {
        self.begin_transfer(plan)
    }

    fn import_character_blob(
        &self,
        transfer_id: u64,
        blob: &[u8],
        source: crate::world::transfer::RealmLocatorPredecessor,
        bot_arrival: Option<&crate::world::transfer::BotTransferIntent>,
    ) -> Result<()> {
        match bot_arrival {
            Some(intent) => {
                if (source.map_id, source.instance_id, source.revision)
                    != (
                        intent.source_map,
                        intent.source_instance,
                        intent.source_locator_revision,
                    )
                {
                    anyhow::bail!("bot Transfer Realm locator binding changed before import");
                }
                self.import_bot_character_blob(transfer_id, blob, intent)
            }
            None => self.import_player_character_blob(transfer_id, blob, source),
        }
    }

    fn confirm_import(&self, transfer_id: u64) -> Result<()> {
        self.confirm_import(transfer_id)
    }

    fn finish_transfer(&self, transfer_id: u64) -> Result<()> {
        self.finish_transfer(transfer_id)
    }

    fn release_transfer(&self, transfer_id: u64) -> Result<()> {
        self.release_transfer(transfer_id)
    }

    fn release_player_transfer_arrival(
        &self,
        transfer_id: u64,
        character_guid: u64,
        source: crate::world::transfer::RealmLocatorPredecessor,
    ) -> Result<()> {
        Coordinator::release_player_transfer_arrival(self, transfer_id, character_guid, source)
    }

    fn transfer_arrival(
        &self,
        transfer_id: u64,
    ) -> Option<crate::world::transfer::TransferArrival> {
        self.0
            .coord()
            .conn
            .db
            .game_transfer_in()
            .transfer_id()
            .find(&transfer_id)
            .map(|row| crate::world::transfer::TransferArrival {
                character_guid: row.character_guid,
                source_map: row.source_map_id,
                source_instance: row.source_instance_id,
                source_locator_revision: row.source_locator_revision,
                bot_source_identity: row.bot_intent_source,
                bot_transfer_intent_id: row.bot_intent_id,
                bot_controller_generation: row.bot_controller_generation,
            })
    }

    fn instance_partition(&self, instance_id: u64) -> Option<(u32, u64)> {
        self.0
            .coord()
            .conn
            .db
            .game_instance()
            .instance_id()
            .find(&instance_id)
            .map(|row| (row.map_id, row.party_id))
    }

    fn ensure_instance(&self, instance_id: u64, map_id: u32, party_id: u64) -> Result<()> {
        self.ensure_instance(instance_id, map_id, party_id)
    }

    fn evict_instance_population(&self, instance_id: u64) -> Result<()> {
        self.evict_instance_population(instance_id)
    }

    fn begin_shard_index_transfer(
        &self,
        plan: &crate::world::transfer::TransferPlan,
        bot_intent: Option<(&crate::world::transfer::BotTransferIntent, u64)>,
    ) -> Result<crate::world::party::RealmCharacterPartition> {
        crate::realm_core::begin_shard_index_transfer(self, plan, bot_intent)
    }

    fn finish_player_shard_index_transfer(
        &self,
        plan: &crate::world::transfer::TransferPlan,
        source_map: u32,
        source_instance: u64,
        source_revision: u64,
    ) -> Result<()> {
        crate::realm_core::finish_player_shard_index_transfer(
            self,
            plan,
            source_map,
            source_instance,
            source_revision,
        )
    }

    fn finish_pending_shard_index_transfer(
        &self,
        character_guid: u64,
        destination_map: u32,
        destination_instance: u64,
        arrival: &crate::world::transfer::TransferArrival,
    ) -> Result<()> {
        crate::realm_core::finish_pending_shard_index_transfer(
            self,
            character_guid,
            destination_map,
            destination_instance,
            arrival,
        )
    }

    fn bind_bot_transfer_locator(
        &self,
        intent: &crate::world::transfer::BotTransferIntent,
        source_revision: u64,
        claim_token: u64,
    ) -> Result<()> {
        self.bind_bot_transfer_locator(intent, source_revision, claim_token)
    }

    fn publish_bot_shard_index(
        &self,
        intent: &crate::world::transfer::BotTransferIntent,
    ) -> Result<()> {
        crate::realm_core::publish_bot_shard_index(self, intent)
    }

    fn sync_transfer_arrival(&self, character_guid: u64) -> Result<()> {
        crate::world::party::sync_transfer_arrival_mirror(self, character_guid)
    }

    fn sync_transfer_pending(&self, character_guid: u64) -> Result<()> {
        crate::world::party::sync_transfer_arrival_mirror(self, character_guid)
    }

    fn mark_bot_transfer_arrival_ready(
        &self,
        intent_id: u64,
        bot_guid: u64,
        controller_generation: u64,
        claim_token: u64,
    ) -> Result<()> {
        Coordinator::mark_bot_transfer_arrival_ready(
            self,
            intent_id,
            bot_guid,
            controller_generation,
            claim_token,
        )
    }

    fn bot_transfer_arrival_matches(
        &self,
        transfer_id: u64,
        intent: &crate::world::transfer::BotTransferIntent,
    ) -> bool {
        self.0
            .coord()
            .conn
            .db
            .game_transfer_in()
            .transfer_id()
            .find(&transfer_id)
            .is_some_and(|arrival| {
                arrival.character_guid == intent.bot_guid
                    && arrival.bot_intent_source == intent.source_module_identity
                    && arrival.bot_intent_id == intent.id
                    && arrival.bot_controller_generation == intent.controller_generation
                    && arrival.bot_intent_created_micros == intent.created_micros
                    && arrival.source_map_id == intent.source_map
                    && arrival.source_instance_id == intent.source_instance
                    && arrival.source_locator_revision == intent.source_locator_revision
            })
    }

    fn release_bot_transfer_arrival(
        &self,
        transfer_id: u64,
        intent: &crate::world::transfer::BotTransferIntent,
    ) -> Result<()> {
        Coordinator::release_bot_transfer_arrival(
            self,
            transfer_id,
            intent.bot_guid,
            intent.source_module_identity,
            intent.id,
            intent.controller_generation,
            intent.created_micros,
            intent.source_map,
            intent.source_instance,
            intent.source_locator_revision,
        )
    }
}
