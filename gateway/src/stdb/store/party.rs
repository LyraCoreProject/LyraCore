//! `Coordinator`'s [`PartyStore`] adapter.

use anyhow::Result;

use crate::world::{PartyStore, ShardRoutingStore};

use crate::stdb::bindings::game_world_entity_table::GameWorldEntityTableAccess;
use crate::stdb::Coordinator;

impl PartyStore for Coordinator {
    // The SINGLE-DATABASE party path: unchanged reducer calls on the player's own connection.
    // `self_guid` is unused here on purpose — the module resolves the actor from `ctx.sender()`'s
    // live entity, which is the whole reason this arm needs no operator gate. `world::party` is what
    // chooses between this and the realm-core arm below.
    fn group_invite(
        &self,
        account_id: u64,
        self_guid: u64,
        target_guid: u64,
    ) -> Result<crate::world::party::PartyOutcome> {
        self.group_invite(account_id, self_guid, target_guid)
    }

    fn group_accept(
        &self,
        account_id: u64,
        self_guid: u64,
    ) -> Result<crate::world::party::PartyOutcome> {
        self.group_accept(account_id, self_guid)
    }

    fn group_decline(
        &self,
        account_id: u64,
        self_guid: u64,
    ) -> Result<crate::world::party::PartyOutcome> {
        self.group_decline(account_id, self_guid)
    }

    fn group_leave(
        &self,
        account_id: u64,
        self_guid: u64,
    ) -> Result<crate::world::party::PartyOutcome> {
        self.group_leave(account_id, self_guid)
    }

    fn group_uninvite(
        &self,
        account_id: u64,
        self_guid: u64,
        target_guid: u64,
    ) -> Result<crate::world::party::PartyOutcome> {
        self.group_uninvite(account_id, self_guid, target_guid)
    }

    fn group_loot_method(
        &self,
        account_id: u64,
        self_guid: u64,
        loot_setting: u8,
        master_guid: u64,
        loot_threshold: u8,
    ) -> Result<crate::world::party::PartyOutcome> {
        self.group_loot_method(
            account_id,
            self_guid,
            loot_setting,
            master_guid,
            loot_threshold,
        )
    }

    fn claim_bot_invite_intent(&self, intent_id: u64) -> Result<crate::world::party::PartyOutcome> {
        self.claim_bot_invite_intent(intent_id)
    }

    fn claim_party_command_intent(&self, intent_id: u64, claim_token: u64) -> Result<()> {
        Coordinator::claim_party_command_intent(self, intent_id, claim_token)
    }

    fn admit_party_command_authority(
        &self,
        group_id: u64,
        leader_guid: u64,
        bot_guid: u64,
        authority_member_guid: u64,
        expected_members: Vec<u64>,
    ) -> Result<crate::world::party::CompanionCommandOutcome> {
        Coordinator::admit_party_command_authority(
            self,
            group_id,
            leader_guid,
            bot_guid,
            authority_member_guid,
            expected_members,
        )
    }

    fn apply_admitted_party_command(
        &self,
        command: &crate::world::party::AdmittedCompanionCommand,
    ) -> Result<crate::world::party::CompanionCommandOutcome> {
        Coordinator::apply_admitted_party_command(self, command)
    }

    fn finish_party_command_intent(
        &self,
        intent_id: u64,
        claim_token: u64,
        outcome: crate::world::party::CompanionCommandOutcome,
    ) -> Result<()> {
        Coordinator::finish_party_command_intent(self, intent_id, claim_token, outcome)
    }

    fn confirm_party_command_receipt(
        &self,
        source_identity: spacetimedb_sdk::Identity,
        intent_id: u64,
    ) -> Result<Option<crate::world::party::CompanionCommandOutcome>> {
        Coordinator::confirm_party_command_receipt(self, source_identity, intent_id)
    }

    fn confirm_party_command_holder(
        &self,
        bot_guid: u64,
    ) -> Result<crate::world::party::PartyCommandHolder> {
        Coordinator::confirm_party_command_holder(self, bot_guid)
    }

    fn entity_partition(&self, guid: u64) -> Option<(u32, u64)> {
        self.0
            .coord()
            .conn
            .db
            .game_world_entity()
            .guid()
            .find(&guid)
            .map(|entity| (entity.map_id, entity.instance_id))
    }

    fn admit_sessionless_group_action(
        &self,
        character_guid: u64,
    ) -> Result<crate::world::party::PartyOutcome> {
        self.admit_sessionless_group_action(character_guid)
    }

    fn realm_group_op(
        &self,
        op: u8,
        actor_guid: u64,
        target_guid: u64,
        arg_a: u8,
        arg_b: u8,
        arg_c: u64,
    ) -> Result<crate::world::party::PartyOutcome> {
        self.realm_group_op(op, actor_guid, target_guid, arg_a, arg_b, arg_c)
    }

    fn realm_group_op_visible(
        &self,
        op: u8,
        actor_guid: u64,
        target_guid: u64,
        arg_a: u8,
        arg_b: u8,
        arg_c: u64,
    ) -> Result<crate::world::party::PartyOutcome> {
        self.realm_group_op_visible(op, actor_guid, target_guid, arg_a, arg_b, arg_c)
    }

    fn deleted_character_party_leave(
        &self,
        character_guid: u64,
    ) -> Result<crate::world::party::PartyOutcome> {
        self.deleted_character_party_leave(character_guid)
    }

    fn group_roster(
        &self,
        character_guid: u64,
    ) -> Result<Option<crate::world::party::GroupRoster>> {
        Ok(self.group_roster(character_guid))
    }

    fn party_command_group_roster(
        &self,
        character_guid: u64,
    ) -> Result<Option<crate::world::party::GroupRoster>> {
        self.party_command_group_roster(character_guid)
    }

    fn group_roster_by_id(
        &self,
        group_id: u64,
    ) -> Result<Option<crate::world::party::GroupRoster>> {
        Ok(self.group_roster_by_id(group_id))
    }

    fn group_roster_revision(&self, group_id: u64) -> Result<u64> {
        Ok(self.group_roster_revision(group_id))
    }

    fn held_roster_revision(&self, group_id: u64) -> Result<Option<u64>> {
        Ok(self.held_roster_revision(group_id))
    }

    fn realm_character_partition(
        &self,
        character_guid: u64,
    ) -> Result<Option<crate::world::party::RealmCharacterPartition>> {
        self.realm_character_partition(character_guid)
    }

    fn party_holder_observation(
        &self,
        character_guid: u64,
        map_id: u32,
        instance_id: u64,
    ) -> Result<crate::world::party::PartyHolderObservation> {
        let serves_locator = self.shard_for_location(map_id, instance_id).is_none();
        self.stable_party_holder_observation(character_guid, serves_locator)
    }

    fn party_cleanup_group_roster_by_id(
        &self,
        group_id: u64,
    ) -> Result<Option<crate::world::party::GroupRoster>> {
        let healthy = {
            let live = self.0.coord();
            live.is_healthy()
        };
        if !healthy {
            anyhow::bail!(
                "{} has no healthy Coordinator subscription for party cleanup",
                self.shard_name()
            );
        }
        Ok(self.group_roster_by_id(group_id))
    }

    fn party_member_guids(&self) -> Result<Vec<u64>> {
        if !self.0.coord().is_healthy() {
            anyhow::bail!(
                "{} has no healthy Coordinator subscription for party cleanup",
                self.shard_name()
            );
        }
        Ok(self.party_member_guids())
    }

    fn party_group_ids(&self) -> Result<Vec<u64>> {
        if !self.0.coord().is_healthy() {
            anyhow::bail!(
                "{} has no healthy Coordinator subscription for party cleanup",
                self.shard_name()
            );
        }
        Ok(self.party_group_ids())
    }

    fn sync_group_mirror(&self, roster: &crate::world::party::GroupRoster) -> Result<()> {
        self.sync_group_mirror(roster)
    }
}
