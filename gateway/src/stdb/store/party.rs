//! `Coordinator`'s [`PartyStore`] adapter.

use anyhow::{anyhow, Result};
use lyracore_shared::group::{GroupKind, GroupRefusal, RaidSlot};
use spacetimedb_sdk::{Identity, Table};

use crate::stdb::bindings::game_world_entity_table::GameWorldEntityTableAccess;
use crate::stdb::bindings::*;
use crate::stdb::connection::{call_reducer, reducer_refusal_reason};
use crate::stdb::Coordinator;
use crate::world::party::{AdmittedCompanionCommand, CompanionCommandOutcome, PartyOutcome};
use crate::world::{Actor, PartyStore, ShardRoutingStore};

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

impl Coordinator {
    pub(crate) fn stable_party_holder_observation(
        &self,
        character_guid: u64,
        serves_locator: bool,
    ) -> Result<crate::world::party::PartyHolderObservation> {
        let guard = self.0.coord();
        if !guard.is_healthy() {
            anyhow::bail!(
                "{} has no healthy Coordinator subscription for party partition certification",
                self.shard_name()
            );
        }
        let has_escrow = guard
            .conn
            .db
            .game_transfer_out()
            .transfer_id()
            .find(&crate::world::transfer::transfer_id_for(character_guid))
            .is_some_and(|row| row.character_guid == character_guid);
        let character_partition = guard
            .conn
            .db
            .game_character()
            .guid()
            .find(&character_guid)
            .map(|character| (character.map_id, character.pending_instance_id));
        Ok(crate::world::party::PartyHolderObservation {
            serves_locator,
            has_escrow,
            character_partition,
        })
    }

    /// Every party in this Realm-core or World Shard cache.
    pub fn party_group_ids(&self) -> Vec<u64> {
        self.0
            .coord()
            .conn
            .db
            .game_group()
            .iter()
            .map(|group| group.group_id)
            .collect()
    }

    /// Every Character with party membership in this Realm-core cache.
    pub fn party_member_guids(&self) -> Vec<u64> {
        self.0
            .coord()
            .conn
            .db
            .game_group_member()
            .iter()
            .map(|member| member.character_guid)
            .collect()
    }

    /// Read only the bounded roster projection accepted by companion-command authority. The bound
    /// is the Raid cap: a longer list is a damaged cache.
    pub fn party_command_group_roster(
        &self,
        character_guid: u64,
    ) -> anyhow::Result<Option<crate::world::party::GroupRoster>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let Some((group_id, rows)) = guard
            .party_memberships
            .read()
            .unwrap()
            .bounded_member_rows(character_guid, lyracore_shared::group::RAID_MAX_MEMBERS)?
        else {
            return Ok(None);
        };
        let Some(group) = db.game_group().group_id().find(&group_id) else {
            return Ok(None);
        };
        let members = rows
            .into_iter()
            .map(|(row_id, guid)| crate::world::party::GroupRosterMember {
                guid,
                slot: db
                    .game_group_member()
                    .id()
                    .find(&row_id)
                    .map_or_else(Default::default, |row| {
                        RaidSlot::from_wire_or_default(row.raid_slot)
                    }),
            })
            .collect();
        Ok(Some(crate::world::party::GroupRoster {
            group_id,
            roster_revision: db
                .game_group_roster_revision()
                .group_id()
                .find(&group_id)
                .map_or(1, |row| row.revision),
            leader_guid: group.leader_guid,
            loot_method: group.loot_method,
            loot_threshold: group.loot_threshold,
            master_looter_guid: group.master_looter_guid,
            kind: GroupKind::from_wire_or_default(group.group_type),
            members,
            partitions: Vec::new(),
        }))
    }

    /// Realm-core's roster order survives disband in `game_group_roster_revision`.
    pub fn group_roster_revision(&self, group_id: u64) -> u64 {
        self.0
            .coord()
            .conn
            .db
            .game_group_roster_revision()
            .group_id()
            .find(&group_id)
            .map_or(1, |row| row.revision)
    }

    /// The Roster Revision this database holds for one party, or `None` when it holds no row. On a
    /// World Shard this is the mirror's revision, which outlives a disband.
    pub fn held_roster_revision(&self, group_id: u64) -> Option<u64> {
        self.0
            .coord()
            .conn
            .db
            .game_group_roster_revision()
            .group_id()
            .find(&group_id)
            .map(|row| row.revision)
    }

    pub fn claim_party_command_intent(&self, intent_id: u64, claim_token: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "claim_party_command_intent",
            claim_party_command_intent_then(intent_id, claim_token)
        )
    }

    pub fn admit_party_command_authority(
        &self,
        group_id: u64,
        leader_guid: u64,
        bot_guid: u64,
        authority_member_guid: u64,
        expected_members: Vec<u64>,
    ) -> Result<CompanionCommandOutcome> {
        match call_reducer!(
            self.0.call_pipe().conn.reducers,
            "admit_party_command_authority",
            admit_party_command_authority_then(
                group_id,
                leader_guid,
                bot_guid,
                authority_member_guid,
                expected_members
            )
        ) {
            Ok(()) => Ok(CompanionCommandOutcome::Applied),
            Err(error) => command_refusal(&error).ok_or(error),
        }
    }

    pub fn apply_admitted_party_command(
        &self,
        command: &AdmittedCompanionCommand,
    ) -> Result<CompanionCommandOutcome> {
        let applied = call_reducer!(
            self.0.call_pipe().conn.reducers,
            "apply_admitted_party_command",
            apply_admitted_party_command_then(
                command.source_identity,
                command.intent_id,
                command.issuer_guid,
                command.issuer_sequence,
                command.group_id,
                command.leader_guid,
                command.members.clone(),
                command.kind,
                command.bot_guid,
                command.authority_member_guid,
                command.exact_target_guid,
                command.expires_micros,
                command.receipt_retain_until_micros
            )
        );
        if let Err(error) = applied {
            return command_refusal(&error).ok_or(error);
        }
        self.confirm_party_command_receipt(command.source_identity, command.intent_id)?
            .ok_or_else(|| anyhow!("party command apply committed without a receipt"))
    }

    pub fn finish_party_command_intent(
        &self,
        intent_id: u64,
        claim_token: u64,
        outcome: CompanionCommandOutcome,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "finish_party_command_intent",
            finish_party_command_intent_then(
                intent_id,
                claim_token,
                command_outcome_binding(outcome)
            )
        )
    }

    pub fn confirm_party_command_receipt(
        &self,
        source_identity: Identity,
        intent_id: u64,
    ) -> Result<Option<CompanionCommandOutcome>> {
        match call_reducer!(
            self.0.call_pipe().conn.reducers,
            "confirm_party_command_receipt",
            confirm_party_command_receipt_then(source_identity, intent_id)
        ) {
            Ok(()) => Ok(None),
            Err(error) => command_refusal(&error).map(Some).ok_or(error),
        }
    }

    pub fn confirm_party_command_holder(
        &self,
        bot_guid: u64,
    ) -> Result<crate::world::party::PartyCommandHolder> {
        match call_reducer!(
            self.0.call_pipe().conn.reducers,
            "confirm_party_command_holder",
            confirm_party_command_holder_then(bot_guid)
        ) {
            Ok(()) => Ok(crate::world::party::PartyCommandHolder::Present),
            Err(error) => match reducer_refusal_reason(&error) {
                Some("MissingBot") => Ok(crate::world::party::PartyCommandHolder::Missing),
                Some("TransferInProgress") => {
                    Ok(crate::world::party::PartyCommandHolder::InTransit)
                }
                _ => Err(error),
            },
        }
    }

    /// Claim and admit a Group Intent in one World Shard transaction.
    pub fn claim_bot_invite_intent(&self, intent_id: u64) -> Result<PartyOutcome> {
        party_outcome(call_reducer!(
            self.0.call_pipe().conn.reducers,
            "claim_bot_invite_intent",
            claim_bot_invite_intent_then(intent_id)
        ))
    }

    /// Wait for the owning World Shard's current consent Gate, without subscription readback.
    pub fn admit_sessionless_group_action(&self, character_guid: u64) -> Result<PartyOutcome> {
        party_outcome(call_reducer!(
            self.0.call_pipe().conn.reducers,
            "admit_sessionless_group_action",
            admit_sessionless_group_action_then(character_guid)
        ))
    }

    /// `CMSG_GROUP_INVITE` — `target_guid` is already resolved by the gateway.
    pub fn group_invite(
        &self,
        _account_id: u64,
        actor_guid: u64,
        target_guid: u64,
    ) -> Result<PartyOutcome> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("group_invite: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        party_outcome(call_reducer!(
            coord.conn.reducers,
            "gw_group_invite",
            gw_group_invite_then(self.session_actor(actor), target_guid)
        ))
    }

    /// `CMSG_GROUP_ACCEPT`. Rides the coordinator connection as
    /// `gw_accept_group_invite`.
    pub fn group_accept(&self, _account_id: u64, actor_guid: u64) -> Result<PartyOutcome> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("group_accept: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        party_outcome(call_reducer!(
            coord.conn.reducers,
            "gw_accept_group_invite",
            gw_accept_group_invite_then(self.session_actor(actor))
        ))
    }

    /// `CMSG_GROUP_DECLINE`.
    pub fn group_decline(&self, _account_id: u64, actor_guid: u64) -> Result<PartyOutcome> {
        let actor = Actor::new(actor_guid)
            .ok_or_else(|| anyhow!("group_decline: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        party_outcome(call_reducer!(
            coord.conn.reducers,
            "gw_group_decline",
            gw_group_decline_then(self.session_actor(actor))
        ))
    }

    /// `CMSG_GROUP_DISBAND` — leave the caller's group.
    pub fn group_leave(&self, _account_id: u64, actor_guid: u64) -> Result<PartyOutcome> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("group_leave: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        party_outcome(call_reducer!(
            coord.conn.reducers,
            "gw_group_leave",
            gw_group_leave_then(self.session_actor(actor))
        ))
    }

    /// `CMSG_GROUP_UNINVITE` or `CMSG_GROUP_UNINVITE_GUID` — the leader or an Assistant kicks
    /// `target_guid`.
    pub fn group_uninvite(
        &self,
        _account_id: u64,
        actor_guid: u64,
        target_guid: u64,
    ) -> Result<PartyOutcome> {
        let actor = Actor::new(actor_guid)
            .ok_or_else(|| anyhow!("group_uninvite: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        party_outcome(call_reducer!(
            coord.conn.reducers,
            "gw_group_uninvite",
            gw_group_uninvite_then(self.session_actor(actor), target_guid)
        ))
    }

    /// `CMSG_LOOT_METHOD` — the leader sets the party's loot method/
    /// threshold/master. Echoed to every member via the existing `SMSG_GROUP_LIST` relay (the
    /// module's `group_loot_method` reducer re-renders the roster payload); no separate ack packet
    /// (vanilla sends none for this opcode either).
    pub fn group_loot_method(
        &self,
        _account_id: u64,
        actor_guid: u64,
        loot_setting: u8,
        master_guid: u64,
        loot_threshold: u8,
    ) -> Result<PartyOutcome> {
        let actor = Actor::new(actor_guid)
            .ok_or_else(|| anyhow!("group_loot_method: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        party_outcome(call_reducer!(
            coord.conn.reducers,
            "gw_group_loot_method",
            gw_group_loot_method_then(
                self.session_actor(actor),
                loot_setting,
                master_guid,
                loot_threshold
            )
        ))
    }

    /// `realm_group_op` — one party op against the database THIS handle points at. The gateway
    /// calls it on the **realm-core** handle, where membership is authoritative.
    ///
    /// Through an operator connection, not the player's: the reducer is operator-gated because it
    /// takes the acting character's guid as an argument (realm-core has no live entity to derive
    /// one from), so only the token that holds the operator identity may call it. The guid passed is
    /// the one this socket authenticated into the world with — see `world::party`.
    ///
    /// This form rides a call pipe, which subscribes no group table, so the Coordinator cache may
    /// still hold the old roster when it returns. Only an op that pushes no Group mirror after it
    /// uses it: a Group Broadcast or a Target Icon request. Every roster op uses
    /// [`Self::realm_group_op_visible`].
    pub fn realm_group_op(
        &self,
        op: u8,
        actor_guid: u64,
        target_guid: u64,
        arg_a: u8,
        arg_b: u8,
        arg_c: u64,
    ) -> Result<PartyOutcome> {
        party_outcome(call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_group_op",
            realm_group_op_then(
                op,
                self.actor_or_owner(actor_guid),
                target_guid,
                arg_a,
                arg_b,
                arg_c
            )
        ))
    }

    /// [`Self::realm_group_op`] on the visibility pipe: it returns only after the Coordinator cache
    /// holds the committed roster, so the mirror push that follows reads the op's own result. The
    /// caller must not run on the Coordinator pump: World Sessions and the bot intent threads.
    pub fn realm_group_op_visible(
        &self,
        op: u8,
        actor_guid: u64,
        target_guid: u64,
        arg_a: u8,
        arg_b: u8,
        arg_c: u64,
    ) -> Result<PartyOutcome> {
        let coordinator = self.0.visibility_pipe();
        party_outcome(call_reducer!(
            coordinator.conn.reducers,
            "realm_group_op",
            realm_group_op_then(
                op,
                self.actor_or_owner(actor_guid),
                target_guid,
                arg_a,
                arg_b,
                arg_c
            )
        ))
    }

    /// Remove a deleted Character from a party and return only after the Coordinator cache holds
    /// the committed Realm-core roster. The caller always runs off the Coordinator pump.
    pub fn deleted_character_party_leave(&self, character_guid: u64) -> Result<PartyOutcome> {
        let coordinator = self.0.visibility_pipe();
        party_outcome(call_reducer!(
            coordinator.conn.reducers,
            "realm_group_op",
            realm_group_op_then(
                lyracore_shared::group::realm_op::LEAVE,
                self.actor_or_owner(character_guid),
                0,
                lyracore_shared::group::leave_cause::CHARACTER_DELETED,
                0,
                0
            )
        ))
    }

    /// `sync_group_mirror` — replace THIS shard's mirror of one party with realm-core's roster.
    /// Operator-gated, coordinator connection, same reasoning as above; called
    /// on each WORLD shard after a party op and at world entry.
    pub fn sync_group_mirror(&self, roster: &crate::world::party::GroupRoster) -> Result<()> {
        let partitions = roster
            .partitions
            .iter()
            .map(|partition| GroupMemberPartition {
                character_guid: partition.character_guid,
                group_id: partition.group_id,
                membership_revision: partition.membership_revision,
                member_active: partition.member_active,
                map_id: partition.map_id,
                instance_id: partition.instance_id,
                locator_revision: partition.locator_revision,
                state: match partition.state {
                    crate::world::party::PartyPartitionState::Unknown => {
                        PartyPartitionState::Unknown
                    }
                    crate::world::party::PartyPartitionState::Known => PartyPartitionState::Known,
                    crate::world::party::PartyPartitionState::PendingTransfer => {
                        PartyPartitionState::PendingTransfer
                    }
                },
            })
            .collect();
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "sync_group_mirror",
            sync_group_mirror_then(
                roster.group_id,
                roster.leader_guid,
                roster.loot_method,
                roster.loot_threshold,
                roster.master_looter_guid,
                roster.member_guids(),
                self.owner_actor(),
                partitions,
                roster.roster_revision,
                roster.kind.wire(),
                roster
                    .members
                    .iter()
                    .map(|member| member.slot.wire())
                    .collect()
            )
        )
    }
}

/// The Module's typed party Refusal. Only a reducer the Module rejected carries a tag; a timeout,
/// transport, or SDK failure stays an error with an unknown outcome.
fn group_refusal(error: &anyhow::Error) -> Option<GroupRefusal> {
    reducer_refusal_reason(error).and_then(GroupRefusal::parse_tag)
}

fn command_refusal(error: &anyhow::Error) -> Option<CompanionCommandOutcome> {
    let tag = reducer_refusal_reason(error)?;
    Some(match tag {
        "Applied" => CompanionCommandOutcome::Applied,
        "Unchanged" => CompanionCommandOutcome::Unchanged,
        "Malformed" => CompanionCommandOutcome::Malformed,
        "NotLeader" => CompanionCommandOutcome::NotLeader,
        "NotMember" => CompanionCommandOutcome::NotMember,
        "StalePartyMirror" => CompanionCommandOutcome::StalePartyMirror,
        "WrongAccount" => CompanionCommandOutcome::WrongAccount,
        "MissingBot" => CompanionCommandOutcome::MissingBot,
        "WrongPartition" => CompanionCommandOutcome::WrongPartition,
        "Suppressed" => CompanionCommandOutcome::Suppressed,
        "TargetDead" => CompanionCommandOutcome::TargetDead,
        "TargetUnavailable" => CompanionCommandOutcome::TargetUnavailable,
        "TargetControlled" => CompanionCommandOutcome::TargetControlled,
        "Expired" => CompanionCommandOutcome::Expired,
        "WaitingForCapacity" => CompanionCommandOutcome::WaitingForCapacity,
        "OutcomeUnknown" => CompanionCommandOutcome::OutcomeUnknown,
        "Superseded" => CompanionCommandOutcome::Superseded,
        _ => return None,
    })
}

fn command_outcome_binding(
    outcome: CompanionCommandOutcome,
) -> crate::stdb::bindings::CommandOutcome {
    use crate::stdb::bindings::CommandOutcome as Row;
    match outcome {
        CompanionCommandOutcome::Applied => Row::Applied,
        CompanionCommandOutcome::Unchanged => Row::Unchanged,
        CompanionCommandOutcome::Malformed => Row::Malformed,
        CompanionCommandOutcome::NotLeader => Row::NotLeader,
        CompanionCommandOutcome::NotMember => Row::NotMember,
        CompanionCommandOutcome::StalePartyMirror => Row::StalePartyMirror,
        CompanionCommandOutcome::WrongAccount => Row::WrongAccount,
        CompanionCommandOutcome::MissingBot => Row::MissingBot,
        CompanionCommandOutcome::WrongPartition => Row::WrongPartition,
        CompanionCommandOutcome::Suppressed => Row::Suppressed,
        CompanionCommandOutcome::TargetDead => Row::TargetDead,
        CompanionCommandOutcome::TargetUnavailable => Row::TargetUnavailable,
        CompanionCommandOutcome::TargetControlled => Row::TargetControlled,
        CompanionCommandOutcome::Expired => Row::Expired,
        CompanionCommandOutcome::WaitingForCapacity => Row::WaitingForCapacity,
        CompanionCommandOutcome::OutcomeUnknown => Row::OutcomeUnknown,
        CompanionCommandOutcome::Superseded => Row::Superseded,
    }
}

/// A refused party reducer is an outcome the client renders; anything else ends the session.
fn party_outcome(result: Result<()>) -> Result<PartyOutcome> {
    match result {
        Ok(()) => Ok(PartyOutcome::Ran),
        Err(error) => match group_refusal(&error) {
            Some(refusal) => Ok(refusal.into()),
            None => Err(error),
        },
    }
}
