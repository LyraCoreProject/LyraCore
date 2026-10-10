//! `Coordinator`'s [`PartyStore`] adapter.

use anyhow::{anyhow, Result};
use lyracore_shared::group::{GroupKind, GroupRefusal, RaidSlot};
use spacetimedb_sdk::{Identity, Table};

use crate::stdb::bindings::game_world_entity_table::GameWorldEntityTableAccess;
use crate::stdb::bindings::*;
use crate::stdb::connection::call_reducer;
use crate::stdb::Coordinator;
use crate::stdb::{classify, DurableFailure};
use crate::world::party::{AdmittedCompanionCommand, CompanionCommandOutcome, PartyOutcome};
use crate::world::{Actor, PartyStore, ShardRoutingStore};

impl PartyStore for Coordinator {
    /// Claim and admit a Group Intent in one World Shard transaction.
    fn claim_bot_invite_intent(&self, intent_id: u64) -> Result<PartyOutcome> {
        party_outcome(call_reducer!(
            self.0.call_pipe().conn.reducers,
            "claim_bot_invite_intent",
            claim_bot_invite_intent_then(intent_id)
        ))
    }

    fn claim_party_command_intent(&self, intent_id: u64, claim_token: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "claim_party_command_intent",
            claim_party_command_intent_then(intent_id, claim_token)
        )
    }

    fn admit_party_command_authority(
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

    fn apply_admitted_party_command(
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

    fn finish_party_command_intent(
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

    fn confirm_party_command_receipt(
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

    fn confirm_party_command_holder(
        &self,
        bot_guid: u64,
    ) -> Result<crate::world::party::PartyCommandHolder> {
        match call_reducer!(
            self.0.call_pipe().conn.reducers,
            "confirm_party_command_holder",
            confirm_party_command_holder_then(bot_guid)
        ) {
            Ok(()) => Ok(crate::world::party::PartyCommandHolder::Present),
            Err(error) => match refusal_tag(&error) {
                Some("MissingBot") => Ok(crate::world::party::PartyCommandHolder::Missing),
                Some("TransferInProgress") => {
                    Ok(crate::world::party::PartyCommandHolder::InTransit)
                }
                _ => Err(error),
            },
        }
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

    /// Wait for the owning World Shard's current consent Gate, without subscription readback.
    fn admit_sessionless_group_action(&self, character_guid: u64) -> Result<PartyOutcome> {
        party_outcome(call_reducer!(
            self.0.call_pipe().conn.reducers,
            "admit_sessionless_group_action",
            admit_sessionless_group_action_then(character_guid)
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
    fn realm_group_op(
        &self,
        op: u8,
        actor: Actor,
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
                self.session_actor(actor),
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
    fn realm_group_op_visible(
        &self,
        op: u8,
        actor: Actor,
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
                self.session_actor(actor),
                target_guid,
                arg_a,
                arg_b,
                arg_c
            )
        ))
    }

    /// Remove a deleted Character from a party and return only after the Coordinator cache holds
    /// the committed Realm-core roster. The caller always runs off the Coordinator pump.
    fn deleted_character_party_leave(&self, character: Actor) -> Result<PartyOutcome> {
        let coordinator = self.0.visibility_pipe();
        party_outcome(call_reducer!(
            coordinator.conn.reducers,
            "realm_group_op",
            realm_group_op_then(
                lyracore_shared::group::realm_op::LEAVE,
                self.session_actor(character),
                0,
                lyracore_shared::group::leave_cause::CHARACTER_DELETED,
                0,
                0
            )
        ))
    }

    fn group_roster(
        &self,
        character_guid: u64,
    ) -> Result<Option<crate::world::party::GroupRoster>> {
        Ok(self.group_roster(character_guid))
    }

    /// Read only the bounded roster projection accepted by companion-command authority. The bound
    /// is the Raid cap: a longer list is a damaged cache.
    fn party_command_group_roster(
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

    fn group_roster_by_id(
        &self,
        group_id: u64,
    ) -> Result<Option<crate::world::party::GroupRoster>> {
        Ok(self.group_roster_by_id(group_id))
    }

    /// Realm-core's roster order survives disband in `game_group_roster_revision`.
    fn group_roster_revision(&self, group_id: u64) -> Result<u64> {
        Ok(self
            .0
            .coord()
            .conn
            .db
            .game_group_roster_revision()
            .group_id()
            .find(&group_id)
            .map_or(1, |row| row.revision))
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
        Ok(self
            .0
            .coord()
            .conn
            .db
            .game_group_member()
            .iter()
            .map(|member| member.character_guid)
            .collect())
    }

    fn party_group_ids(&self) -> Result<Vec<u64>> {
        if !self.0.coord().is_healthy() {
            anyhow::bail!(
                "{} has no healthy Coordinator subscription for party cleanup",
                self.shard_name()
            );
        }
        Ok(self
            .0
            .coord()
            .conn
            .db
            .game_group()
            .iter()
            .map(|group| group.group_id)
            .collect())
    }

    /// `sync_group_mirror` — replace THIS shard's mirror of one party with realm-core's roster.
    /// Operator-gated, coordinator connection, same reasoning as above; called
    /// on each WORLD shard after a party op and at world entry.
    fn sync_group_mirror(&self, roster: &crate::world::party::GroupRoster) -> Result<()> {
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

    /// `CMSG_GROUP_INVITE` — `target_guid` is already resolved by the gateway.
    fn group_invite(&self, actor: Actor, target_guid: u64) -> Result<PartyOutcome> {
        party_outcome(call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_group_invite",
            gw_group_invite_then(self.session_actor(actor), target_guid)
        ))
    }

    /// `CMSG_GROUP_ACCEPT`. Rides the coordinator connection as `gw_accept_group_invite`.
    fn group_accept(&self, actor: Actor) -> Result<PartyOutcome> {
        party_outcome(call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_accept_group_invite",
            gw_accept_group_invite_then(self.session_actor(actor))
        ))
    }

    fn group_decline(&self, actor: Actor) -> Result<PartyOutcome> {
        party_outcome(call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_group_decline",
            gw_group_decline_then(self.session_actor(actor))
        ))
    }

    /// `CMSG_GROUP_DISBAND` — leave the caller's group.
    fn group_leave(&self, actor: Actor) -> Result<PartyOutcome> {
        party_outcome(call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_group_leave",
            gw_group_leave_then(self.session_actor(actor))
        ))
    }

    /// `CMSG_GROUP_UNINVITE` or `CMSG_GROUP_UNINVITE_GUID` — the leader or an Assistant kicks
    /// `target_guid`.
    fn group_uninvite(&self, actor: Actor, target_guid: u64) -> Result<PartyOutcome> {
        party_outcome(call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_group_uninvite",
            gw_group_uninvite_then(self.session_actor(actor), target_guid)
        ))
    }

    /// `CMSG_LOOT_METHOD` — the leader sets the party's loot method/threshold/master. Echoed to
    /// every member via the existing `SMSG_GROUP_LIST` relay (the module's `group_loot_method`
    /// reducer re-renders the roster payload); no separate ack packet (vanilla sends none for this
    /// opcode either).
    fn group_loot_method(
        &self,
        actor: Actor,
        loot_setting: u8,
        master_guid: u64,
        loot_threshold: u8,
    ) -> Result<PartyOutcome> {
        party_outcome(call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_group_loot_method",
            gw_group_loot_method_then(
                self.session_actor(actor),
                loot_setting,
                master_guid,
                loot_threshold
            )
        ))
    }
}

impl Coordinator {
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
}

/// The Module's Refusal tag. A Transport Loss carries none, so its outcome stays unknown.
fn refusal_tag(error: &anyhow::Error) -> Option<&str> {
    match classify(error) {
        DurableFailure::Refusal { reason } => Some(reason),
        DurableFailure::TransportLoss => None,
    }
}

fn command_refusal(error: &anyhow::Error) -> Option<CompanionCommandOutcome> {
    let tag = refusal_tag(error)?;
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

/// A tagged party Refusal is an outcome the client renders; an untagged one or a Transport Loss
/// stays `Err` and ends the session.
fn party_outcome(result: Result<()>) -> Result<PartyOutcome> {
    let Err(error) = result else {
        return Ok(PartyOutcome::Ran);
    };
    match refusal_tag(&error).and_then(GroupRefusal::parse_tag) {
        Some(refusal) => Ok(refusal.into()),
        None => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdb::ReducerCallError;

    #[test]
    fn a_party_refusal_is_an_outcome_and_a_transport_loss_is_an_error() {
        let refused =
            ReducerCallError::refused("gw_group_invite", GroupRefusal::GroupFull.as_tag());
        assert_eq!(
            party_outcome(Err(refused.into())).unwrap(),
            PartyOutcome::Refused(GroupRefusal::GroupFull)
        );
        let lost = ReducerCallError::transport_lost("gw_group_invite");
        assert!(party_outcome(Err(lost.into())).is_err());
    }
}
