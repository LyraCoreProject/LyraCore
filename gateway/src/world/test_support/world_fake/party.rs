use super::super::*;

#[derive(Default)]
pub(crate) struct PartyState {
    pub(crate) group_invites: std::sync::Mutex<Vec<u64>>,
    /// Recorded `group_loot_method` calls: (loot_setting, master_guid, loot_threshold).
    pub(crate) group_loot_methods: std::sync::Mutex<Vec<(u8, u64, u8)>>,
    /// Unclaimed bot invite intent ids on this World Shard. Two concurrent consumers share this
    /// collection, matching the Module table both Gateways call into.
    pub(crate) bot_invite_intents: std::sync::Mutex<Vec<u64>>,
    /// Current consent returned by the owned admission seam, independent of presence reads.
    pub(crate) sessionless_admission:
        std::sync::Mutex<std::collections::HashMap<u64, GroupRefusal>>,
    pub(crate) sessionless_admission_unavailable: std::sync::Mutex<Vec<u64>>,
    pub(crate) suppress_after_admission: std::sync::Mutex<Vec<u64>>,
    pub(crate) bot_intent_claim_refusals:
        std::sync::Mutex<std::collections::HashMap<u64, GroupRefusal>>,
    /// The AUTHORITATIVE party state, when this handle is the realm-core one. Shared with
    /// nobody — a realm handle owns exactly one of these, and every shard reads its own `mirror`.
    pub(crate) party: std::sync::Arc<std::sync::Mutex<FakeParty>>,
    /// Realm-core's Coordinator cache lags a call-pipe commit: after `realm_group_op`, roster reads
    /// answer from `stale_party` until a `realm_group_op_visible` returns. The production shape of
    /// a call pipe, which subscribes no group table.
    pub(crate) cache_lags: std::sync::atomic::AtomicBool,
    /// The party as the lagging cache still shows it.
    pub(crate) stale_party: std::sync::Mutex<Option<FakeParty>>,
    /// True when this handle is realm-core, so `group_roster` answers from `party` (the
    /// authority) instead of `mirror` (this shard's cache of it).
    pub(crate) is_realm: bool,
    /// What `sync_group_mirror` wrote onto THIS shard, latest per group. The invalidation
    /// story, made observable.
    pub(crate) mirror: std::sync::Mutex<Vec<crate::world::party::GroupRoster>>,
    /// The Roster Revision each `sync_group_mirror` accepted on THIS shard, kept after the disband
    /// tombstone, as the Module's mirror keeps its `game_group_roster_revision` row.
    pub(crate) mirror_revisions: std::sync::Mutex<std::collections::HashMap<u64, u64>>,
    /// When set, `sync_group_mirror` fails with this message — a world shard that cannot be
    /// mirrored (an unreachable database), which must not fail a party op realm-core already took.
    pub(crate) mirror_error: Option<String>,
    /// How many mirror writes fail before this Shard accepts one.
    pub(crate) mirror_failures: std::sync::atomic::AtomicUsize,
    /// When set, `realm_group_op(ACCEPT, …)` fails with this message. INJECTED because a real
    /// one cannot be staged synchronously: every accept-time refusal the module has (already grouped,
    /// party full, the inviter no longer leads) needs the party to change BETWEEN the invite and the
    /// accept, and the gateway's bot answer runs in the same call as the invite. The failure is still
    /// reachable in production — a concurrent op on another socket — and what it must not do is leave
    /// the invite dialog hanging.
    pub(crate) party_accept_error: Option<String>,
    /// How many Realm-core LEAVE calls fail before one reaches the party state.
    pub(crate) party_leave_failures: std::sync::atomic::AtomicUsize,
    /// When set, every Group Broadcast op fails as a lost connection would.
    pub(crate) group_broadcast_error: bool,
    /// Return a connection failure after the next Realm-core LEAVE commits.
    pub(crate) party_leave_commit_then_error: std::sync::atomic::AtomicBool,
    /// Fail one `group_roster` read by its one-based call number.
    pub(crate) group_roster_error_on_read: std::sync::Mutex<Option<(usize, String)>>,
    /// Number of `group_roster` reads on this handle.
    pub(crate) group_roster_reads: std::sync::atomic::AtomicUsize,
    /// When set, this handle cannot enumerate its mirrored party ids.
    pub(crate) party_group_ids_error: std::sync::Mutex<Option<String>>,
    pub(crate) party_command_claims: std::sync::Mutex<Vec<(u64, u64)>>,
    pub(crate) party_command_finishes:
        std::sync::Mutex<Vec<(u64, u64, crate::world::party::CompanionCommandOutcome)>>,
    pub(crate) admitted_party_commands:
        std::sync::Mutex<Vec<crate::world::party::AdmittedCompanionCommand>>,
    pub(crate) party_command_apply_outcome: Option<crate::world::party::CompanionCommandOutcome>,
    pub(crate) party_command_receipt_error: std::sync::Mutex<Option<String>>,
    pub(crate) party_command_authority_members: std::sync::Mutex<Option<Vec<u64>>>,
    pub(crate) party_command_in_transit: std::sync::Mutex<Vec<u64>>,
    pub(crate) party_command_receipts: std::sync::Mutex<
        Vec<(
            spacetimedb_sdk::Identity,
            u64,
            crate::world::party::CompanionCommandOutcome,
        )>,
    >,
    pub(crate) entity_partitions: std::sync::Mutex<Vec<(u64, u32, u64)>>,
    /// Characters Realm-core reports in a pending Transfer. Read on the realm handle only.
    pub(crate) members_in_transit: std::sync::Mutex<Vec<u64>>,
}

/// The Fake's reducer edge for a party op: the Module answers a Refusal as the bare tag, and
/// anything else — a timeout, a dead transport — is a failure with an unknown durable outcome.
pub(crate) fn faked_party(operation: &str, error: &str) -> Result<PartyOutcome> {
    match lyracore_shared::group::GroupRefusal::parse_tag(error) {
        Some(refusal) => Ok(refusal.into()),
        None => Err(crate::stdb::ReducerCallError::transport_lost(operation).into()),
    }
}

impl PartyStore for WorldFake {
    fn realm_character_partition(
        &self,
        character_guid: u64,
    ) -> Result<Option<crate::world::party::RealmCharacterPartition>> {
        // `members_in_transit` is Member Stats' own per-guid fixture (`presence::of`'s
        // `realm_transfer_pending` reads this trait method on the realm handle, same as
        // `Coordinator::member_presence` did before the two merged). Checked first so it can name
        // one guid as pending without disturbing `realm_partition`, which every other test that
        // exercises a real Transfer already drives.
        if self
            .party
            .members_in_transit
            .lock()
            .unwrap()
            .contains(&character_guid)
        {
            return Ok(Some(crate::world::party::RealmCharacterPartition {
                map_id: 0,
                instance_id: 0,
                revision: 1,
                transfer_pending: true,
                pending_destination_map: 0,
                pending_destination_instance: 0,
                bot_source_identity: spacetimedb_sdk::Identity::ZERO,
                bot_transfer_intent_id: 0,
                bot_controller_generation: 0,
            }));
        }
        Ok(*self.transfer.realm_partition.lock().unwrap())
    }

    fn party_holder_observation(
        &self,
        character_guid: u64,
        map_id: u32,
        instance_id: u64,
    ) -> Result<crate::world::party::PartyHolderObservation> {
        Ok(crate::world::party::PartyHolderObservation {
            serves_locator: self.shard_for_location(map_id, instance_id).is_none(),
            has_escrow: self.escrowed_transfer(character_guid).is_some(),
            character_partition: self
                .character_destination(character_guid)
                .map(|character| (character.dest_map_id, character.dest_instance_id)),
        })
    }

    // Group: a minimal in-memory party — enough for the dispatch tests to drive
    // invite-result mapping and the GROUP_LIST build without a live module.
    //
    // Each of these records the SHARD it ran on (`rec`), so a test can tell the
    // single-database path (the op lands on the player's own shard, here) apart from the realm-core
    // one (it lands in `FakeParty::ops` and never reaches these at all).
    fn group_invite(&self, _actor: Actor, target_guid: u64) -> Result<PartyOutcome> {
        self.rec("group_invite");
        if let Some(e) = &self.trade_error {
            return faked_party("gw_group_invite", e);
        }
        self.party.group_invites.lock().unwrap().push(target_guid);
        Ok(PartyOutcome::Ran)
    }

    fn group_accept(&self, _actor: Actor) -> Result<PartyOutcome> {
        self.rec("group_accept");
        Ok(PartyOutcome::Ran)
    }

    fn group_decline(&self, _actor: Actor) -> Result<PartyOutcome> {
        self.rec("group_decline");
        Ok(PartyOutcome::Ran)
    }

    fn group_leave(&self, _actor: Actor) -> Result<PartyOutcome> {
        self.rec("group_leave");
        Ok(PartyOutcome::Ran)
    }

    fn group_loot_method(
        &self,
        _actor: Actor,
        loot_setting: u8,
        master_guid: u64,
        loot_threshold: u8,
    ) -> Result<PartyOutcome> {
        self.rec("group_loot_method");
        if let Some(e) = &self.trade_error {
            return faked_party("gw_group_loot_method", e);
        }
        self.party.group_loot_methods.lock().unwrap().push((
            loot_setting,
            master_guid,
            loot_threshold,
        ));
        Ok(PartyOutcome::Ran)
    }

    fn claim_party_command_intent(&self, intent_id: u64, claim_token: u64) -> Result<()> {
        self.party
            .party_command_claims
            .lock()
            .unwrap()
            .push((intent_id, claim_token));
        Ok(())
    }

    fn admit_party_command_authority(
        &self,
        group_id: u64,
        leader_guid: u64,
        bot_guid: u64,
        authority_member_guid: u64,
        mut expected_members: Vec<u64>,
    ) -> Result<crate::world::party::CompanionCommandOutcome> {
        let roster = self.group_roster(leader_guid)?;
        let Some(roster) = roster else {
            return Ok(crate::world::party::CompanionCommandOutcome::NotMember);
        };
        if roster.group_id != group_id || roster.leader_guid != leader_guid {
            return Ok(crate::world::party::CompanionCommandOutcome::NotLeader);
        }
        expected_members.sort_unstable();
        let mut current_members = self
            .party
            .party_command_authority_members
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| roster.member_guids());
        current_members.sort_unstable();
        if expected_members != current_members {
            return Ok(crate::world::party::CompanionCommandOutcome::StalePartyMirror);
        }
        if !roster.has_member(bot_guid)
            || (authority_member_guid != 0 && !roster.has_member(authority_member_guid))
        {
            return Ok(crate::world::party::CompanionCommandOutcome::NotMember);
        }
        Ok(crate::world::party::CompanionCommandOutcome::Applied)
    }

    fn apply_admitted_party_command(
        &self,
        command: &crate::world::party::AdmittedCompanionCommand,
    ) -> Result<crate::world::party::CompanionCommandOutcome> {
        self.party
            .admitted_party_commands
            .lock()
            .unwrap()
            .push(command.clone());
        let outcome = self
            .party
            .party_command_apply_outcome
            .unwrap_or(crate::world::party::CompanionCommandOutcome::Applied);
        if outcome != crate::world::party::CompanionCommandOutcome::WaitingForCapacity {
            self.party.party_command_receipts.lock().unwrap().push((
                command.source_identity,
                command.intent_id,
                outcome,
            ));
        }
        Ok(outcome)
    }

    fn finish_party_command_intent(
        &self,
        intent_id: u64,
        claim_token: u64,
        outcome: crate::world::party::CompanionCommandOutcome,
    ) -> Result<()> {
        self.party
            .party_command_finishes
            .lock()
            .unwrap()
            .push((intent_id, claim_token, outcome));
        Ok(())
    }

    fn confirm_party_command_receipt(
        &self,
        source_identity: spacetimedb_sdk::Identity,
        intent_id: u64,
    ) -> Result<Option<crate::world::party::CompanionCommandOutcome>> {
        if let Some(error) = self
            .party
            .party_command_receipt_error
            .lock()
            .unwrap()
            .as_ref()
        {
            return Err(anyhow!(error.clone()));
        }
        Ok(self
            .party
            .party_command_receipts
            .lock()
            .unwrap()
            .iter()
            .find(|(source, id, _)| *source == source_identity && *id == intent_id)
            .map(|(_, _, outcome)| *outcome))
    }

    fn confirm_party_command_holder(
        &self,
        guid: u64,
    ) -> Result<crate::world::party::PartyCommandHolder> {
        Ok(
            if self
                .party
                .party_command_in_transit
                .lock()
                .unwrap()
                .contains(&guid)
            {
                crate::world::party::PartyCommandHolder::InTransit
            } else if self
                .characters
                .iter()
                .any(|character| character.guid == guid)
            {
                crate::world::party::PartyCommandHolder::Present
            } else {
                crate::world::party::PartyCommandHolder::Missing
            },
        )
    }

    fn entity_partition(&self, guid: u64) -> Option<(u32, u64)> {
        self.party
            .entity_partitions
            .lock()
            .unwrap()
            .iter()
            .find(|(candidate, _, _)| *candidate == guid)
            .map(|(_, map, instance)| (*map, *instance))
    }

    fn claim_bot_invite_intent(&self, intent_id: u64) -> Result<PartyOutcome> {
        if let Some(refusal) = self
            .party
            .bot_intent_claim_refusals
            .lock()
            .unwrap()
            .get(&intent_id)
        {
            return Ok(PartyOutcome::Refused(*refusal));
        }
        let mut intents = self.party.bot_invite_intents.lock().unwrap();
        let Some(index) = intents.iter().position(|id| *id == intent_id) else {
            return Ok(PartyOutcome::Refused(GroupRefusal::IntentAlreadyClaimed));
        };
        intents.swap_remove(index);
        Ok(PartyOutcome::Ran)
    }

    fn admit_sessionless_group_action(&self, character_guid: u64) -> Result<PartyOutcome> {
        self.rec("admit_sessionless_group_action");
        if self
            .party
            .sessionless_admission_unavailable
            .lock()
            .unwrap()
            .contains(&character_guid)
        {
            anyhow::bail!("World Shard admission unavailable");
        }
        if let Some(refusal) = self
            .party
            .sessionless_admission
            .lock()
            .unwrap()
            .get(&character_guid)
        {
            return Ok(PartyOutcome::Refused(*refusal));
        }
        if !self.entity_in_world(character_guid)
            || !matches!(
                self.character_presence(character_guid),
                Ok(Some((false, ..)))
            )
        {
            return Ok(PartyOutcome::Refused(GroupRefusal::ActorUnavailable));
        }
        if self
            .party
            .suppress_after_admission
            .lock()
            .unwrap()
            .contains(&character_guid)
        {
            self.party
                .sessionless_admission
                .lock()
                .unwrap()
                .insert(character_guid, GroupRefusal::ActionSuppressed);
        }
        Ok(PartyOutcome::Ran)
    }

    /// The module's `realm_group_op`, modelled: the rules the ROUTING depends on, applied to the
    /// authority this handle owns.
    #[allow(clippy::too_many_lines)] // One arm per realm group op, as the Module's reducer has.
    fn realm_group_op(
        &self,
        op: u8,
        actor: Actor,
        target_guid: u64,
        arg_a: u8,
        arg_b: u8,
        arg_c: u64,
    ) -> Result<PartyOutcome> {
        use lyracore_shared::group::{event_kind as kind, realm_op, GroupRefusal};
        self.rec("realm_group_op");
        let actor_guid = actor.guid();
        let mut p = self.party.party.lock().unwrap();
        if self
            .party
            .cache_lags
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            let mut stale = self.party.stale_party.lock().unwrap();
            if stale.is_none() {
                *stale = Some(p.clone());
            }
        }
        p.ops
            .push((op, actor_guid, target_guid, arg_a, arg_b, arg_c));
        let full = |p: &FakeParty, group_id: u64| {
            p.member_guids(group_id).len() >= p.kind_of(group_id).member_cap()
        };
        match op {
            realm_op::INVITE => {
                if p.group_of(target_guid).is_some() {
                    return Ok(GroupRefusal::AlreadyInGroup.into());
                }
                if p.group_of(actor_guid)
                    .is_some_and(|group_id| full(&p, group_id))
                {
                    return Ok(GroupRefusal::GroupFull.into());
                }
                p.invites.retain(|(t, _)| *t != target_guid);
                p.invites.push((target_guid, actor_guid));
                p.events.push((target_guid, kind::INVITE));
            }
            realm_op::ACCEPT => {
                if let Some(e) = &self.party.party_accept_error {
                    return faked_party("realm_group_op", e);
                }
                let Some(inviter) = p
                    .invites
                    .iter()
                    .find(|(t, _)| *t == actor_guid)
                    .map(|(_, i)| *i)
                else {
                    return Ok(GroupRefusal::NoPendingInvite.into());
                };
                p.invites.retain(|(t, _)| *t != actor_guid);
                let group_id = match p.group_of(inviter) {
                    Some(g) if full(&p, g) => return Ok(GroupRefusal::GroupFull.into()),
                    Some(g) => g,
                    None => {
                        p.next_group_id += 1;
                        let g = p.next_group_id;
                        // The vanilla defaults a freshly-formed party gets: GROUP loot, Uncommon.
                        p.groups.push((g, inviter, 3, 2, 0));
                        p.members.push((g, inviter));
                        g
                    }
                };
                let slot = p.joining_slot(group_id);
                if slot != RaidSlot::default() {
                    p.slots.insert(actor_guid, slot);
                }
                p.members.push((group_id, actor_guid));
                p.push_list(group_id);
            }
            realm_op::RAID_CONVERT => {
                let Some(group_id) = p.group_of(actor_guid) else {
                    return Ok(GroupRefusal::NotInGroup.into());
                };
                if p.groups.iter().find(|(g, ..)| *g == group_id).map(|e| e.1) != Some(actor_guid) {
                    return Ok(GroupRefusal::NotLeader.into());
                }
                if p.kind_of(group_id) == GroupKind::Party {
                    p.raids.push(group_id);
                    p.push_list(group_id);
                }
            }
            realm_op::DECLINE => {
                let Some(inviter) = p
                    .invites
                    .iter()
                    .find(|(t, _)| *t == actor_guid)
                    .map(|(_, i)| *i)
                else {
                    return Ok(GroupRefusal::NoPendingInvite.into());
                };
                p.invites.retain(|(t, _)| *t != actor_guid);
                p.events.push((inviter, kind::DECLINE));
            }
            realm_op::LEAVE => {
                if self
                    .party
                    .party_leave_failures
                    .fetch_update(
                        std::sync::atomic::Ordering::SeqCst,
                        std::sync::atomic::Ordering::SeqCst,
                        |remaining| remaining.checked_sub(1),
                    )
                    .is_ok()
                {
                    return Err(anyhow!("Realm-core LEAVE connection interrupted"));
                }
                if p.group_of(actor_guid).is_none() {
                    return Ok(GroupRefusal::NotInGroup.into());
                }
                if self
                    .party
                    .party_leave_commit_then_error
                    .swap(false, std::sync::atomic::Ordering::SeqCst)
                {
                    p.remove_member(actor_guid);
                    return Err(anyhow!("Realm-core LEAVE reply was lost after commit"));
                }
                p.remove_member(actor_guid);
            }
            realm_op::UNINVITE => {
                let Some(group_id) = p.group_of(actor_guid) else {
                    return Ok(GroupRefusal::NotInGroup.into());
                };
                if !p.manages(group_id, actor_guid) {
                    return Ok(GroupRefusal::NotLeader.into());
                }
                if p.group_of(target_guid) != Some(group_id) {
                    return Ok(GroupRefusal::TargetNotInGroup.into());
                }
                if target_guid == actor_guid {
                    return Ok(GroupRefusal::KickSelf.into());
                }
                if target_guid == p.leader_of(group_id) {
                    return Ok(GroupRefusal::NotLeader.into());
                }
                p.remove_member(target_guid);
            }
            realm_op::LOOT_METHOD => {
                let Some(group_id) = p.group_of(actor_guid) else {
                    return Ok(GroupRefusal::NotInGroup.into());
                };
                if let Some(entry) = p.groups.iter_mut().find(|(g, ..)| *g == group_id) {
                    if entry.1 != actor_guid {
                        return Ok(GroupRefusal::NotLeader.into());
                    }
                    entry.2 = arg_a;
                    entry.3 = arg_b;
                    entry.4 = target_guid;
                }
                p.push_list(group_id);
            }
            realm_op::SET_LEADER => {
                let Some(group_id) = p.group_of(actor_guid) else {
                    return Ok(GroupRefusal::NotInGroup.into());
                };
                if p.leader_of(group_id) != actor_guid {
                    return Ok(GroupRefusal::NotLeader.into());
                }
                if target_guid == actor_guid {
                    return Ok(GroupRefusal::TargetIsSelf.into());
                }
                if p.group_of(target_guid) != Some(group_id) {
                    return Ok(GroupRefusal::TargetNotInGroup.into());
                }
                p.set_leader(group_id, target_guid);
                p.push_list(group_id);
            }
            realm_op::SET_ASSISTANT => {
                let Some(group_id) = p.group_of(actor_guid) else {
                    return Ok(GroupRefusal::NotInGroup.into());
                };
                if p.leader_of(group_id) != actor_guid {
                    return Ok(GroupRefusal::NotLeader.into());
                }
                if p.kind_of(group_id) != GroupKind::Raid {
                    return Ok(GroupRefusal::NotRaid.into());
                }
                if target_guid == actor_guid {
                    return Ok(GroupRefusal::TargetIsSelf.into());
                }
                if p.group_of(target_guid) != Some(group_id) {
                    return Ok(GroupRefusal::TargetNotInGroup.into());
                }
                let slot = p.slots.get(&target_guid).copied().unwrap_or_default();
                let assigned = slot.with_assistant(arg_a != 0);
                if assigned != slot {
                    p.slots.insert(target_guid, assigned);
                    p.push_list(group_id);
                }
            }
            realm_op::CHANGE_SUBGROUP => {
                return Ok(p.change_subgroup(actor_guid, target_guid, arg_a))
            }
            realm_op::SWAP_SUBGROUP => return Ok(p.swap_subgroup(actor_guid, target_guid, arg_c)),
            // Group Broadcasts. Who hears each one is the Module's rule; the routing needs only
            // the op on the authority, and a Refusal for a member of no group.
            realm_op::READY_CHECK_START..=realm_op::RANDOM_ROLL
                if self.party.group_broadcast_error =>
            {
                return Err(anyhow!("Realm-core call pipe timed out"));
            }
            realm_op::READY_CHECK_START
            | realm_op::READY_CHECK_ANSWER
            | realm_op::TARGET_ICON
            | realm_op::MINIMAP_PING => {
                if p.group_of(actor_guid).is_none() {
                    return Ok(GroupRefusal::NotInGroup.into());
                }
            }
            realm_op::RANDOM_ROLL => {}
            other => return Err(anyhow!("unknown realm group op {other}")),
        }
        Ok(PartyOutcome::Ran)
    }

    /// The visibility receipt: the op commits, then the Coordinator cache holds it.
    fn realm_group_op_visible(
        &self,
        op: u8,
        actor: Actor,
        target_guid: u64,
        arg_a: u8,
        arg_b: u8,
        arg_c: u64,
    ) -> Result<PartyOutcome> {
        let outcome = self.realm_group_op(op, actor, target_guid, arg_a, arg_b, arg_c);
        *self.party.stale_party.lock().unwrap() = None;
        outcome
    }

    fn deleted_character_party_leave(
        &self,
        character: Actor,
    ) -> Result<crate::world::party::PartyOutcome> {
        self.realm_group_op(
            lyracore_shared::group::realm_op::LEAVE,
            character,
            0,
            lyracore_shared::group::leave_cause::CHARACTER_DELETED,
            0,
            0,
        )
    }

    fn group_roster(
        &self,
        character_guid: u64,
    ) -> Result<Option<crate::world::party::GroupRoster>> {
        let read = self
            .party
            .group_roster_reads
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        if let Some((error_read, error)) = &*self.party.group_roster_error_on_read.lock().unwrap() {
            if read == *error_read {
                return Err(anyhow!(error.clone()));
            }
        }
        if self.party.is_realm {
            return Ok(self.realm_cache(|p| p.group_of(character_guid).and_then(|g| p.roster(g))));
        }
        Ok(self
            .party
            .mirror
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.has_member(character_guid))
            .cloned())
    }

    fn party_command_group_roster(
        &self,
        character_guid: u64,
    ) -> Result<Option<crate::world::party::GroupRoster>> {
        let roster = self.group_roster(character_guid)?;
        if roster
            .as_ref()
            .is_some_and(|roster| roster.members.len() > lyracore_shared::group::RAID_MAX_MEMBERS)
        {
            anyhow::bail!("party command roster exceeds the member limit");
        }
        Ok(roster)
    }

    fn group_roster_by_id(
        &self,
        group_id: u64,
    ) -> Result<Option<crate::world::party::GroupRoster>> {
        if self.party.is_realm {
            return Ok(self.realm_cache(|p| p.roster(group_id)));
        }
        Ok(self
            .party
            .mirror
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.group_id == group_id)
            .cloned())
    }

    fn party_cleanup_group_roster_by_id(
        &self,
        group_id: u64,
    ) -> Result<Option<crate::world::party::GroupRoster>> {
        self.group_roster_by_id(group_id)
    }

    fn group_roster_revision(&self, group_id: u64) -> Result<u64> {
        Ok(self.held_roster_revision(group_id)?.unwrap_or(1))
    }

    fn held_roster_revision(&self, group_id: u64) -> Result<Option<u64>> {
        if self.party.is_realm {
            return Ok(self.realm_cache(|p| p.revisions.get(&group_id).copied()));
        }
        Ok(self
            .party
            .mirror_revisions
            .lock()
            .unwrap()
            .get(&group_id)
            .copied())
    }

    fn party_member_guids(&self) -> Result<Vec<u64>> {
        if !self.party.is_realm {
            return Ok(Vec::new());
        }
        Ok(self
            .party
            .party
            .lock()
            .unwrap()
            .members
            .iter()
            .map(|(_, guid)| *guid)
            .collect())
    }

    fn party_group_ids(&self) -> Result<Vec<u64>> {
        if let Some(error) = &*self.party.party_group_ids_error.lock().unwrap() {
            return Err(anyhow!(error.clone()));
        }
        if self.party.is_realm {
            return Ok(self
                .party
                .party
                .lock()
                .unwrap()
                .groups
                .iter()
                .map(|(group_id, ..)| *group_id)
                .collect());
        }
        Ok(self
            .party
            .mirror
            .lock()
            .unwrap()
            .iter()
            .map(|roster| roster.group_id)
            .collect())
    }

    fn sync_group_mirror(&self, roster: &crate::world::party::GroupRoster) -> Result<()> {
        self.rec("sync_group_mirror");
        if self
            .party
            .mirror_failures
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |remaining| remaining.checked_sub(1),
            )
            .is_ok()
        {
            return Err(anyhow!("World Shard mirror connection interrupted"));
        }
        if let Some(e) = &self.party.mirror_error {
            return Err(anyhow!("{e}"));
        }
        self.party
            .mirror_revisions
            .lock()
            .unwrap()
            .insert(roster.group_id, roster.roster_revision);
        let mut mirror = self.party.mirror.lock().unwrap();
        mirror.retain(|r| r.group_id != roster.group_id);
        // An empty roster is the disband tombstone — the shard forgets the party rather than
        // keeping an empty one, which is what the module's own `sync_group_mirror` does.
        if !roster.members.is_empty() {
            mirror.push(roster.clone());
        }
        Ok(())
    }

    fn group_uninvite(&self, _actor: Actor, _target_guid: u64) -> Result<PartyOutcome> {
        self.rec("group_uninvite");
        Ok(PartyOutcome::Ran)
    }
}

/// Realm-core's authoritative party state, as far as the gateway's routing can see it —
/// the module's `realm_group_op` rules, modelled at the granularity the ROUTING depends on:
/// which database the op lands on, who ends up in which group, and who gets notified.
///
/// Same instrument, and same limits, as [`FakeShardDb`]: the module's own reducer bodies cannot run
/// in a gateway test (no `ReducerContext`), so what executes here is the gateway's production
/// routing (`world::party`) against a faithful stand-in for the authority. The rules themselves are
/// the module's to test.
#[derive(Default, Clone)]
pub(crate) struct FakeParty {
    pub(crate) next_group_id: u64,
    /// group_id → (leader, loot_method, loot_threshold, master_looter).
    pub(crate) groups: Vec<(u64, u64, u8, u8, u64)>,
    /// (group_id, character_guid), in join order — the order realm-core's roster read returns.
    pub(crate) members: Vec<(u64, u64)>,
    /// (target_guid, inviter_guid) — at most one pending per target, newest wins.
    pub(crate) invites: Vec<(u64, u64)>,
    /// Groups their leader converted to a Raid.
    pub(crate) raids: Vec<u64>,
    /// Each Raid member's Raid Slot; a member absent here holds the Party default.
    pub(crate) slots: std::collections::HashMap<u64, RaidSlot>,
    /// group_id → how many times [`Self::push_list`] fired for it — this Fake's stand-in for the
    /// Roster Revision. A group absent here has never had a list pushed. Bumped exactly where a
    /// real accepted change would advance the revision, so [`crate::world::party::roster_unchanged`]'s
    /// no-mirror-push optimization is exercised against a real signal rather than a constant.
    pub(crate) revisions: std::collections::HashMap<u64, u64>,
    /// Every op that reached the AUTHORITY: `(op, actor, target, arg_a, arg_b, arg_c)`. The
    /// assertion that a party op ran on realm-core rather than on the player's shard.
    pub(crate) ops: Vec<(u8, u64, u64, u8, u8, u64)>,
    /// Every notification the authority pushed: `(recipient_guid, kind)` — the relay's input.
    pub(crate) events: Vec<(u64, u8)>,
}

impl FakeParty {
    pub(crate) fn group_of(&self, guid: u64) -> Option<u64> {
        self.members
            .iter()
            .find(|(_, g)| *g == guid)
            .map(|(gid, _)| *gid)
    }

    pub(crate) fn kind_of(&self, group_id: u64) -> GroupKind {
        if self.raids.contains(&group_id) {
            GroupKind::Raid
        } else {
            GroupKind::Party
        }
    }

    pub(crate) fn member_guids(&self, group_id: u64) -> Vec<u64> {
        self.members
            .iter()
            .filter(|(g, _)| *g == group_id)
            .map(|(_, guid)| *guid)
            .collect()
    }

    /// The Module's own placement rule, `RaidSlot::for_raid_joiner`, over this Fake's slots.
    pub(crate) fn joining_slot(&self, group_id: u64) -> RaidSlot {
        if self.kind_of(group_id) == GroupKind::Party {
            return RaidSlot::default();
        }
        let current = self
            .member_guids(group_id)
            .into_iter()
            .map(|guid| self.slots.get(&guid).copied().unwrap_or_default());
        RaidSlot::for_raid_joiner(current).expect("the cap check leaves room")
    }

    pub(crate) fn roster(&self, group_id: u64) -> Option<crate::world::party::GroupRoster> {
        let (gid, leader, method, threshold, master) =
            *self.groups.iter().find(|(g, ..)| *g == group_id)?;
        Some(crate::world::party::GroupRoster {
            group_id: gid,
            roster_revision: self.revisions.get(&group_id).copied().unwrap_or(0),
            leader_guid: leader,
            loot_method: method,
            loot_threshold: threshold,
            master_looter_guid: master,
            kind: self.kind_of(group_id),
            members: self
                .member_guids(group_id)
                .into_iter()
                .map(|guid| crate::world::party::GroupRosterMember {
                    guid,
                    slot: self.slots.get(&guid).copied().unwrap_or_default(),
                })
                .collect(),
            partitions: Vec::new(),
        })
    }

    pub(crate) fn leader_of(&self, group_id: u64) -> u64 {
        self.groups
            .iter()
            .find(|(g, ..)| *g == group_id)
            .map_or(0, |group| group.1)
    }

    /// The leader, or an Assistant.
    pub(crate) fn manages(&self, group_id: u64, guid: u64) -> bool {
        self.leader_of(group_id) == guid
            || self
                .slots
                .get(&guid)
                .is_some_and(|slot| slot.is_assistant())
    }

    /// The Module's `change_subgroup_on`, modelled: leader-or-Assistant, Raid-only, capacity
    /// decided by the shared [`RaidSlot::moved_to_subgroup`] rule.
    pub(crate) fn change_subgroup(
        &mut self,
        actor_guid: u64,
        target_guid: u64,
        subgroup: u8,
    ) -> PartyOutcome {
        use lyracore_shared::group::GroupRefusal;
        let Some(group_id) = self.group_of(actor_guid) else {
            return GroupRefusal::NotInGroup.into();
        };
        if self.kind_of(group_id) != GroupKind::Raid {
            return GroupRefusal::NotRaid.into();
        }
        if !self.manages(group_id, actor_guid) {
            return GroupRefusal::NotLeader.into();
        }
        if self.group_of(target_guid) != Some(group_id) {
            return GroupRefusal::TargetNotInGroup.into();
        }
        let current = self.slots.get(&target_guid).copied().unwrap_or_default();
        let destination_size = self
            .member_guids(group_id)
            .into_iter()
            .filter(|&guid| {
                guid != target_guid
                    && self
                        .slots
                        .get(&guid)
                        .copied()
                        .unwrap_or_default()
                        .subgroup()
                        == subgroup
            })
            .count();
        match current.moved_to_subgroup(subgroup, destination_size) {
            Err(refusal) => refusal.into(),
            Ok(None) => PartyOutcome::Ran,
            Ok(Some(new_slot)) => {
                self.slots.insert(target_guid, new_slot);
                self.push_list(group_id);
                PartyOutcome::Ran
            }
        }
    }

    /// The Module's `swap_subgroup_on`, modelled: same gates as [`Self::change_subgroup`], the
    /// shared [`RaidSlot::swapped_with`] rule, and no capacity Gate.
    pub(crate) fn swap_subgroup(
        &mut self,
        actor_guid: u64,
        first_guid: u64,
        second_guid: u64,
    ) -> PartyOutcome {
        use lyracore_shared::group::GroupRefusal;
        let Some(group_id) = self.group_of(actor_guid) else {
            return GroupRefusal::NotInGroup.into();
        };
        if self.kind_of(group_id) != GroupKind::Raid {
            return GroupRefusal::NotRaid.into();
        }
        if !self.manages(group_id, actor_guid) {
            return GroupRefusal::NotLeader.into();
        }
        if self.group_of(first_guid) != Some(group_id)
            || self.group_of(second_guid) != Some(group_id)
        {
            return GroupRefusal::TargetNotInGroup.into();
        }
        let first_slot = self.slots.get(&first_guid).copied().unwrap_or_default();
        let second_slot = self.slots.get(&second_guid).copied().unwrap_or_default();
        if let Some((new_first, new_second)) = first_slot.swapped_with(second_slot) {
            self.slots.insert(first_guid, new_first);
            self.slots.insert(second_guid, new_second);
            self.push_list(group_id);
        }
        PartyOutcome::Ran
    }

    /// Hand the lead to `leader` and announce it to every member, as the Module does, before the
    /// list.
    pub(crate) fn set_leader(&mut self, group_id: u64, leader: u64) {
        if let Some(entry) = self.groups.iter_mut().find(|(g, ..)| *g == group_id) {
            entry.1 = leader;
        }
        for member in self.member_guids(group_id) {
            self.events
                .push((member, lyracore_shared::group::event_kind::SET_LEADER));
        }
    }

    pub(crate) fn push_list(&mut self, group_id: u64) {
        *self.revisions.entry(group_id).or_insert(0) += 1;
        let recipients: Vec<u64> = self
            .members
            .iter()
            .filter(|(g, _)| *g == group_id)
            .map(|(_, guid)| *guid)
            .collect();
        for r in recipients {
            self.events
                .push((r, lyracore_shared::group::event_kind::LIST));
        }
    }

    /// Drop `guid` from its party, applying the module's own disband rule (a party of one is no
    /// party) — the half `sync_mirrors` has to observe to push the right tombstone.
    pub(crate) fn remove_member(&mut self, guid: u64) {
        use lyracore_shared::group::event_kind as kind;
        let Some(group_id) = self.group_of(guid) else {
            return;
        };
        self.members
            .retain(|(g, m)| !(*g == group_id && *m == guid));
        self.slots.remove(&guid);
        self.events.push((guid, kind::DESTROYED));
        let remaining: Vec<u64> = self
            .members
            .iter()
            .filter(|(g, _)| *g == group_id)
            .map(|(_, m)| *m)
            .collect();
        if remaining.len() < 2 {
            for r in remaining {
                self.events.push((r, kind::DESTROYED));
                self.slots.remove(&r);
            }
            self.members.retain(|(g, _)| *g != group_id);
            self.groups.retain(|(g, ..)| *g != group_id);
            self.raids.retain(|g| *g != group_id);
        } else {
            if self.leader_of(group_id) == guid {
                // The Module's succession: a Raid's first Assistant in join order, else the
                // longest-standing member.
                let raid = self.kind_of(group_id) == GroupKind::Raid;
                let heir = remaining
                    .iter()
                    .copied()
                    .find(|member| raid && self.manages(group_id, *member))
                    .unwrap_or(remaining[0]);
                self.set_leader(group_id, heir);
            }
            self.push_list(group_id);
        }
    }
}

impl WorldFake {
    /// Read realm-core's party the way its Coordinator cache shows it: stale after a lagging
    /// call-pipe commit, current otherwise.
    pub(crate) fn realm_cache<R>(&self, read: impl FnOnce(&FakeParty) -> R) -> R {
        if let Some(stale) = &*self.party.stale_party.lock().unwrap() {
            return read(stale);
        }
        read(&self.party.party.lock().unwrap())
    }
}
