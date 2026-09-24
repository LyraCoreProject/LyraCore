//! Realm-wide party state — the GROUP slice of realm-core's social & economy plane, part of the
//! elastic world-sharding design.
//!
//! # What moved, and why here
//!
//! `game_group` / `game_group_member` / `game_group_invite` are authoritative on **realm-core**.
//! None of the three is coupled to space — the spec's partition rule — and all three silently became
//! shard-local the moment a second database existed: `group_invite` resolved its target inside the
//! CALLING database, so a player inside Deadmines could not invite one standing in Elwynn, and the
//! escrowed transfer's interim membership mirror was a snapshot taken at `begin_transfer`, so a party
//! SPLIT across the boundary never saw itself. Both were observed live (2026-07-25), not theorised.
//!
//! This module is the ROUTING half of the fix and the only place that decides which database a party
//! op runs against. Everything in it is generic over [`WorldStore`], so the decisions execute under
//! test against the same in-memory shard topology the cross-database transfer uses — the seam the
//! transfer-transport test harness built. The database-specific halves are thin: `Coordinator`'s
//! trait impl (which database a handle names) and the module's `realm_group_op` (the rules, unchanged).
//!
//! # The three planes
//!
//! 1. **Realm-core** owns membership. Ops go there through the operator-gated `realm_group_op`, and
//!    it is where invites, rosters and the leadership/disband rules live. One authority, so an invite
//!    across a shard boundary is not a special case — it is the only case.
//! 2. **Each world shard** keeps a MIRROR of the roster, refreshed by [`sync_mirrors`] after every op
//!    and by [`on_world_entry`] when a character arrives. It exists because ~fifty in-world reads
//!    (kill-XP split, quest credit, loot rules, the party's dungeon binding) resolve
//!    membership locally on the hot path and must not become cross-database calls. Same relationship
//!    `game_account`/`game_session` have had with realm-core from the start.
//! 3. **A single-database gateway** has no realm-core to route to ([`WorldStore::realm_store`]
//!    answers `None`), so every op takes the pre-realm-core path: the player's own connection, the
//!    player-facing reducer, the shard's own tables. Byte-identical, and pinned by
//!    `an_unsharded_gateway_runs_every_party_op_on_the_players_own_shard`.
//!
//! # What did NOT move
//!
//! `/say`-range chat, `/yell` and targeted emotes stay on the world shards as AOI-scoped events:
//! they ARE spatial, which is the same rule that moved membership off them. Party (`/p`) chat is a
//! Realm Chat Line: Realm-core reads its own membership in the transaction that writes the line, so
//! the mirror plays no part in who hears it.

use anyhow::Result;

use super::{presence, send, Outbound, SessionTx, WorldStore};
use crate::codec;
use lyracore_shared::group::{
    bot_op, realm_op, GroupKind, GroupRefusal, RaidSlot, RosterMember, RosterPayload,
    COMMAND_RESULT_WINDOW_MICROS, GROUP_MAX_MEMBERS,
};

/// One group, as the database that holds it sees it. Read from realm-core it is the authority; read
/// from a world shard it is that shard's mirror. Names and online flags are deliberately NOT in it —
/// realm-core has no character rows to resolve either from, so they are filled at render time from
/// the shards ([`render_list`]).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct GroupRoster {
    pub group_id: u64,
    /// Realm-core's order for this complete roster. World Shards retain the value after disband.
    pub roster_revision: u64,
    pub leader_guid: u64,
    pub loot_method: u8,
    pub loot_threshold: u8,
    pub master_looter_guid: u64,
    pub kind: GroupKind,
    /// Members in join order (member-row id), which is the order leadership succeeds in, each with
    /// its Raid Slot.
    pub members: Vec<GroupRosterMember>,
    /// One ordered partition projection per member. Realm-core supplies both revisions; the
    /// Gateway confirms the location against the World Shard that currently holds the Character.
    pub partitions: Vec<GroupMemberPartition>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroupRosterMember {
    pub guid: u64,
    pub slot: RaidSlot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroupMemberPartition {
    pub character_guid: u64,
    pub group_id: u64,
    pub membership_revision: u64,
    pub member_active: bool,
    pub map_id: u32,
    pub instance_id: u64,
    pub locator_revision: u64,
    pub state: PartyPartitionState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartyPartitionState {
    Unknown,
    Known,
    PendingTransfer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RealmCharacterPartition {
    pub map_id: u32,
    pub instance_id: u64,
    pub revision: u64,
    pub transfer_pending: bool,
    pub pending_destination_map: u32,
    pub pending_destination_instance: u64,
    pub bot_source_identity: spacetimedb_sdk::Identity,
    pub bot_transfer_intent_id: u64,
    pub bot_controller_generation: u64,
}

const PARTY_LOCATION_SHARD_LIMIT: usize = 16;
pub(crate) const PARTY_COMMAND_ABORT_STEP: &str = "apply_party_command";

pub(crate) fn party_command_abort_configuration(
    configured: Option<String>,
) -> Result<Option<String>> {
    match configured {
        None => Ok(None),
        Some(step) if step == PARTY_COMMAND_ABORT_STEP => Ok(Some(step)),
        Some(step) => anyhow::bail!(
            "LYRACORE_PARTY_COMMAND_ABORT_AFTER={step} names no party command step; valid step: \
             {PARTY_COMMAND_ABORT_STEP}"
        ),
    }
}

#[cfg(not(test))]
fn die_by_party_command_injection() -> ! {
    log::logger().flush();
    std::process::abort()
}

#[cfg(test)]
fn die_by_party_command_injection() -> ! {
    panic!("LYRACORE_PARTY_COMMAND_ABORT_AFTER: injected abort");
}

#[inline]
fn party_command_abort_point(
    abort_after: Option<&str>,
    source_identity: spacetimedb_sdk::Identity,
    intent_id: u64,
) {
    if abort_after != Some(PARTY_COMMAND_ABORT_STEP) {
        return;
    }
    log::error!(
        "party command {source_identity}/{intent_id}: \
         LYRACORE_PARTY_COMMAND_ABORT_AFTER={PARTY_COMMAND_ABORT_STEP}; target apply committed, \
         aborting before source finalization for the Gateway recovery test"
    );
    die_by_party_command_injection()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PartyHolderObservation {
    pub(crate) serves_locator: bool,
    pub(crate) has_escrow: bool,
    pub(crate) character_partition: Option<(u32, u64)>,
}

fn classify_party_partition(
    locator: RealmCharacterPartition,
    observations: &[PartyHolderObservation],
) -> Result<PartyPartitionState> {
    let mut state = None;
    for observation in observations.iter().filter(|row| row.serves_locator) {
        let observed = if observation.has_escrow {
            Some(PartyPartitionState::PendingTransfer)
        } else if observation.character_partition == Some((locator.map_id, locator.instance_id)) {
            Some(PartyPartitionState::Known)
        } else {
            None
        };
        if let Some(observed) = observed {
            if state.replace(observed).is_some() {
                anyhow::bail!(
                    "more than one World Shard holds the Character at Realm locator revision {}",
                    locator.revision
                );
            }
        }
    }
    Ok(state.unwrap_or(PartyPartitionState::Unknown))
}

fn certify_roster_partitions<St: WorldStore + ?Sized>(
    store: &St,
    realm: &dyn WorldStore,
    mut roster: GroupRoster,
) -> Result<GroupRoster> {
    let shards = store.world_stores();
    if shards.len() > PARTY_LOCATION_SHARD_LIMIT {
        anyhow::bail!(
            "party partition certification exceeds the {PARTY_LOCATION_SHARD_LIMIT}-Shard limit"
        );
    }
    for projection in &mut roster.partitions {
        let Some(locator) = realm.realm_character_partition(projection.character_guid)? else {
            projection.map_id = 0;
            projection.instance_id = 0;
            projection.locator_revision = 0;
            projection.state = PartyPartitionState::Unknown;
            continue;
        };
        if locator.transfer_pending {
            projection.map_id = locator.map_id;
            projection.instance_id = locator.instance_id;
            projection.locator_revision = locator.revision;
            projection.state = PartyPartitionState::PendingTransfer;
            continue;
        }
        let observations: Vec<_> = shards
            .iter()
            .map(|shard| {
                shard.party_holder_observation(
                    projection.character_guid,
                    locator.map_id,
                    locator.instance_id,
                )
            })
            .collect::<Result<_>>()?;
        if realm.realm_character_partition(projection.character_guid)? != Some(locator) {
            anyhow::bail!(
                "party member {} changed Realm locator during certification",
                projection.character_guid
            );
        }
        projection.map_id = locator.map_id;
        projection.instance_id = locator.instance_id;
        projection.locator_revision = locator.revision;
        projection.state = classify_party_partition(locator, &observations)?;
    }
    Ok(roster)
}

fn append_departed_partitions(roster: &mut GroupRoster, previous: Option<&GroupRoster>) {
    let Some(previous) = previous.filter(|row| row.group_id == roster.group_id) else {
        return;
    };
    let departed: Vec<_> = previous
        .partitions
        .iter()
        .copied()
        .filter(|row| !roster.has_member(row.character_guid))
        .collect();
    for mut departed in departed {
        departed.member_active = false;
        departed.state = PartyPartitionState::Unknown;
        roster.partitions.push(departed);
    }
}

impl GroupRoster {
    /// The empty roster for a group that no longer exists — what [`sync_mirrors`] pushes to make a
    /// shard forget a disbanded party. `members` empty is the disband signal `sync_group_mirror`
    /// reads. The roster revision remains authoritative; the other fields are left at zero.
    pub fn disbanded(group_id: u64, roster_revision: u64) -> Self {
        Self {
            group_id,
            roster_revision,
            ..Default::default()
        }
    }

    /// Member guids in join order.
    pub fn member_guids(&self) -> Vec<u64> {
        self.members.iter().map(|member| member.guid).collect()
    }

    pub fn has_member(&self, guid: u64) -> bool {
        self.members.iter().any(|member| member.guid == guid)
    }

    /// The list this roster renders as. Names are blank: [`render_list`] reads them from the
    /// shards, since realm-core holds none. It carries no Target Icons: the Gateway reads none,
    /// and only a LIST event from the party authority carries them.
    pub fn list_payload(&self) -> RosterPayload {
        RosterPayload {
            leader: self.leader_guid,
            loot_method: self.loot_method,
            loot_threshold: self.loot_threshold,
            master_looter_guid: self.master_looter_guid,
            kind: self.kind,
            members: self
                .members
                .iter()
                .map(|member| RosterMember {
                    guid: member.guid,
                    name: String::new(),
                    slot: member.slot,
                })
                .collect(),
            target_icons: Vec::new(),
        }
    }
}

fn roster_or_disbanded(realm: &dyn WorldStore, group_id: u64) -> Result<GroupRoster> {
    Ok(match realm.group_roster_by_id(group_id)? {
        Some(roster) => roster,
        None => GroupRoster::disbanded(group_id, realm.group_roster_revision(group_id)?),
    })
}

/// One party op, in the client's own vocabulary. The argument packing into `realm_group_op`'s slots
/// happens once, in [`Op::realm_args`], against the shared
/// [`lyracore_shared::group::realm_op`] contract.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    /// `CMSG_GROUP_INVITE`, target already resolved to a guid.
    Invite(u64),
    /// `CMSG_GROUP_ACCEPT`.
    Accept,
    /// `CMSG_GROUP_DECLINE`.
    Decline,
    /// `CMSG_GROUP_DISBAND` — the client's "Leave Party".
    Leave,
    /// `CMSG_GROUP_UNINVITE` (target name already resolved to a guid) or
    /// `CMSG_GROUP_UNINVITE_GUID`: the leader or an Assistant kicks this member.
    Uninvite(u64),
    /// `CMSG_LOOT_METHOD`.
    LootMethod {
        setting: u8,
        master: u64,
        threshold: u8,
    },
    /// `CMSG_GROUP_RAID_CONVERT`.
    RaidConvert,
    /// `CMSG_GROUP_SET_LEADER`: pass the lead to this member.
    SetLeader(u64),
    /// `CMSG_GROUP_ASSISTANT_LEADER`: promote this member to Assistant, or demote it.
    SetAssistant { target: u64, promote: bool },
    /// `CMSG_GROUP_CHANGE_SUB_GROUP`, target already resolved to a guid.
    ChangeSubgroup { target: u64, subgroup: u8 },
    /// `CMSG_GROUP_SWAP_SUB_GROUP`, both targets already resolved to guids.
    SwapSubgroup { first: u64, second: u64 },
    /// `MSG_RAID_READY_CHECK` without a body.
    ReadyCheckStart,
    /// `MSG_RAID_READY_CHECK` with the answer's state byte.
    ReadyCheckAnswer(u8),
    /// `MSG_RAID_TARGET_UPDATE`: a Target Icon 0 to 7 on `target` (0 clears it), or
    /// [`lyracore_shared::group::TARGET_ICON_LIST_REQUEST`] for the full list.
    TargetIcon { icon: u8, target: u64 },
    /// `MSG_MINIMAP_PING`, the point on the minimap.
    MinimapPing { x: f32, y: f32 },
    /// `MSG_RANDOM_ROLL`, the bounds as the client sent them.
    RandomRoll { min: u32, max: u32 },
}

/// `realm_group_op`'s argument slots after the actor: `(op, target_guid, arg_a, arg_b, arg_c)`.
type RealmOpArgs = (u8, u64, u8, u8, u64);

impl Op {
    fn realm_args(self) -> RealmOpArgs {
        match self {
            Op::Invite(target) => (realm_op::INVITE, target, 0, 0, 0),
            Op::Accept => (realm_op::ACCEPT, 0, 0, 0, 0),
            Op::Decline => (realm_op::DECLINE, 0, 0, 0, 0),
            Op::Leave => (realm_op::LEAVE, 0, 0, 0, 0),
            Op::Uninvite(target) => (realm_op::UNINVITE, target, 0, 0, 0),
            Op::LootMethod {
                setting,
                master,
                threshold,
            } => (realm_op::LOOT_METHOD, master, setting, threshold, 0),
            Op::RaidConvert => (realm_op::RAID_CONVERT, 0, 0, 0, 0),
            Op::SetLeader(target) => (realm_op::SET_LEADER, target, 0, 0, 0),
            Op::SetAssistant { target, promote } => {
                (realm_op::SET_ASSISTANT, target, u8::from(promote), 0, 0)
            }
            Op::ChangeSubgroup { target, subgroup } => {
                (realm_op::CHANGE_SUBGROUP, target, subgroup, 0, 0)
            }
            Op::SwapSubgroup { first, second } => (realm_op::SWAP_SUBGROUP, first, 0, 0, second),
            Op::ReadyCheckStart => (realm_op::READY_CHECK_START, 0, 0, 0, 0),
            Op::ReadyCheckAnswer(state) => (realm_op::READY_CHECK_ANSWER, 0, state, 0, 0),
            Op::TargetIcon { icon, target } => (realm_op::TARGET_ICON, target, icon, 0, 0),
            // Bit patterns, so the floats reach the other members unchanged.
            Op::MinimapPing { x, y } => (
                realm_op::MINIMAP_PING,
                u64::from(x.to_bits()),
                0,
                0,
                u64::from(y.to_bits()),
            ),
            Op::RandomRoll { min, max } => {
                (realm_op::RANDOM_ROLL, u64::from(min), 0, 0, u64::from(max))
            }
        }
    }

    /// A Group Broadcast changes no roster, so it needs no roster read, no loot-roll flush and no
    /// mirror push.
    fn is_group_broadcast(self) -> bool {
        matches!(
            self,
            Op::ReadyCheckStart
                | Op::ReadyCheckAnswer(_)
                | Op::TargetIcon { .. }
                | Op::MinimapPing { .. }
                | Op::RandomRoll { .. }
        )
    }
}

/// Run `op` as `self_guid` through `realm_group_op` on `authority`, the database that holds the
/// party: Realm-core when sharded, the home shard otherwise.
fn run_on_authority<A: WorldStore + ?Sized>(
    authority: &A,
    self_guid: u64,
    op: Op,
) -> Result<PartyOutcome> {
    let (code, target, arg_a, arg_b, arg_c) = op.realm_args();
    authority.realm_group_op(code, self_guid, target, arg_a, arg_b, arg_c)
}

/// What one party op answered. A [`GroupRefusal`] is a gameplay answer the client renders, so it
/// arrives as `Ok`; a timeout, transport failure, or untagged reducer error stays `Err` and ends the
/// session, because the durable outcome is then unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PartyOutcome {
    Ran,
    Refused(GroupRefusal),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompanionCommandOutcome {
    Applied,
    Unchanged,
    Malformed,
    NotLeader,
    NotMember,
    StalePartyMirror,
    WrongAccount,
    MissingBot,
    WrongPartition,
    Suppressed,
    TargetDead,
    TargetUnavailable,
    TargetControlled,
    Expired,
    WaitingForCapacity,
    OutcomeUnknown,
    Superseded,
}

#[derive(Clone, Debug)]
pub struct PartyCommandIntent {
    pub id: u64,
    pub source_identity: spacetimedb_sdk::Identity,
    pub issuer_guid: u64,
    pub issuer_sequence: u64,
    pub kind: u8,
    pub bot_guid: u64,
    pub authority_member_guid: u64,
    pub exact_target_guid: u64,
    pub expires_micros: i64,
}

#[derive(Clone, Debug)]
pub struct AdmittedCompanionCommand {
    pub source_identity: spacetimedb_sdk::Identity,
    pub intent_id: u64,
    pub issuer_guid: u64,
    pub issuer_sequence: u64,
    pub group_id: u64,
    pub leader_guid: u64,
    pub members: Vec<u64>,
    pub kind: u8,
    pub bot_guid: u64,
    pub authority_member_guid: u64,
    pub exact_target_guid: u64,
    pub expires_micros: i64,
    pub receipt_retain_until_micros: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartyCommandHolder {
    Present,
    Missing,
    InTransit,
}

fn receipt_anywhere<St: WorldStore + ?Sized>(
    source: &St,
    source_identity: spacetimedb_sdk::Identity,
    intent_id: u64,
) -> Result<Option<CompanionCommandOutcome>> {
    let mut found = source.confirm_party_command_receipt(source_identity, intent_id)?;
    for shard in source.party_command_worlds()? {
        if let Some(outcome) = shard.confirm_party_command_receipt(source_identity, intent_id)? {
            if found.is_some_and(|previous| previous != outcome) {
                anyhow::bail!(
                    "party command receipt {source_identity}/{intent_id} has conflicting outcomes"
                );
            }
            found = Some(outcome);
        }
    }
    Ok(found)
}

pub(crate) fn finish_expired_party_command_intent<St: WorldStore + ?Sized>(
    source: &St,
    intent: &PartyCommandIntent,
    claim_token: u64,
) -> Result<CompanionCommandOutcome> {
    let outcome =
        receipt_anywhere(source, intent.source_identity, intent.id)?.unwrap_or_else(|| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_micros() as i64;
            if now
                >= intent
                    .expires_micros
                    .saturating_add(COMMAND_RESULT_WINDOW_MICROS)
            {
                CompanionCommandOutcome::OutcomeUnknown
            } else {
                CompanionCommandOutcome::Expired
            }
        });
    source.finish_party_command_intent(intent.id, claim_token, outcome)?;
    Ok(outcome)
}

fn same_authority(left: &GroupRoster, right: &GroupRoster) -> bool {
    let mut left_members = left.member_guids();
    let mut right_members = right.member_guids();
    left_members.sort_unstable();
    right_members.sort_unstable();
    left.group_id == right.group_id
        && left.leader_guid == right.leader_guid
        && left_members == right_members
}

/// Claim, certify on Realm-core, compare the target mirror, apply once, then finalize at source.
/// The Realm-core Durable Request is the authority point; a later party change cannot recall an
/// already applied order. Every retry before target application repeats this operation.
pub(crate) fn run_party_command_intent<St: WorldStore>(
    source: &St,
    intent: &PartyCommandIntent,
    claim_token: u64,
) -> Result<CompanionCommandOutcome> {
    let abort_after = party_command_abort_configuration(
        std::env::var("LYRACORE_PARTY_COMMAND_ABORT_AFTER").ok(),
    )?;
    source.claim_party_command_intent(intent.id, claim_token)?;

    if let Some(outcome) = receipt_anywhere(source, intent.source_identity, intent.id)? {
        source.finish_party_command_intent(intent.id, claim_token, outcome)?;
        return Ok(outcome);
    }

    let world_shards = source.party_command_worlds()?;
    let target: &dyn WorldStore;
    let owned_target;
    if world_shards.is_empty() {
        match source.confirm_party_command_holder(intent.bot_guid)? {
            PartyCommandHolder::Present => {}
            PartyCommandHolder::InTransit => anyhow::bail!("party command bot is in Transfer"),
            PartyCommandHolder::Missing => {
                let outcome = CompanionCommandOutcome::MissingBot;
                source.finish_party_command_intent(intent.id, claim_token, outcome)?;
                return Ok(outcome);
            }
        }
        target = source;
    } else {
        let mut holders = Vec::new();
        let mut in_transit = false;
        for shard in &world_shards {
            match shard.confirm_party_command_holder(intent.bot_guid)? {
                PartyCommandHolder::Present => holders.push(shard.clone()),
                PartyCommandHolder::InTransit => in_transit = true,
                PartyCommandHolder::Missing => {}
            }
        }
        if in_transit {
            anyhow::bail!("party command bot is in Transfer");
        }
        let Some(holder) = holders.pop() else {
            let outcome = CompanionCommandOutcome::MissingBot;
            source.finish_party_command_intent(intent.id, claim_token, outcome)?;
            return Ok(outcome);
        };
        if !holders.is_empty() {
            anyhow::bail!(
                "bot {} has more than one live World Shard holder",
                intent.bot_guid
            );
        }
        owned_target = holder;
        target = owned_target.as_ref();
    }

    let owned_realm;
    let realm: &dyn WorldStore = match source.party_command_realm()? {
        Some(handle) => {
            owned_realm = handle;
            owned_realm.as_ref()
        }
        None => source,
    };
    let Some(authority) = realm.party_command_group_roster(intent.issuer_guid)? else {
        let outcome = CompanionCommandOutcome::NotMember;
        source.finish_party_command_intent(intent.id, claim_token, outcome)?;
        return Ok(outcome);
    };
    let outcome = if authority.leader_guid != intent.issuer_guid {
        Some(CompanionCommandOutcome::NotLeader)
    } else if authority.members.len() > GROUP_MAX_MEMBERS {
        // Companion Orders keep the Party cap. The Module answers a Raid above five the same way.
        Some(CompanionCommandOutcome::StalePartyMirror)
    } else if !authority.has_member(intent.bot_guid)
        || (intent.authority_member_guid != 0
            && !authority.has_member(intent.authority_member_guid))
    {
        Some(CompanionCommandOutcome::NotMember)
    } else {
        None
    };
    if let Some(outcome) = outcome {
        source.finish_party_command_intent(intent.id, claim_token, outcome)?;
        return Ok(outcome);
    }
    let authority_outcome = realm.admit_party_command_authority(
        authority.group_id,
        intent.issuer_guid,
        intent.bot_guid,
        intent.authority_member_guid,
        authority.member_guids(),
    )?;
    if authority_outcome != CompanionCommandOutcome::Applied {
        let outcome = authority_outcome;
        source.finish_party_command_intent(intent.id, claim_token, outcome)?;
        return Ok(outcome);
    }
    let local = target.party_command_group_roster(intent.bot_guid)?;
    if local
        .as_ref()
        .is_none_or(|local| !same_authority(local, &authority))
    {
        let outcome = CompanionCommandOutcome::StalePartyMirror;
        source.finish_party_command_intent(intent.id, claim_token, outcome)?;
        return Ok(outcome);
    }
    let bot_partition = target.entity_partition(intent.bot_guid);
    let issuer_partition = source.entity_partition(intent.issuer_guid).or_else(|| {
        world_shards
            .iter()
            .find_map(|shard| shard.entity_partition(intent.issuer_guid))
    });
    let member_partition = (intent.authority_member_guid != 0)
        .then(|| {
            source
                .entity_partition(intent.authority_member_guid)
                .or_else(|| {
                    world_shards
                        .iter()
                        .find_map(|shard| shard.entity_partition(intent.authority_member_guid))
                })
        })
        .flatten();
    let member_partition_required = intent.authority_member_guid != 0;
    if bot_partition.is_none()
        || issuer_partition != bot_partition
        || (member_partition_required && member_partition != bot_partition)
    {
        let outcome = CompanionCommandOutcome::WrongPartition;
        source.finish_party_command_intent(intent.id, claim_token, outcome)?;
        return Ok(outcome);
    }
    let admitted = AdmittedCompanionCommand {
        source_identity: intent.source_identity,
        intent_id: intent.id,
        issuer_guid: intent.issuer_guid,
        issuer_sequence: intent.issuer_sequence,
        group_id: authority.group_id,
        leader_guid: authority.leader_guid,
        members: authority.member_guids(),
        kind: intent.kind,
        bot_guid: intent.bot_guid,
        authority_member_guid: intent.authority_member_guid,
        exact_target_guid: intent.exact_target_guid,
        expires_micros: intent.expires_micros,
        receipt_retain_until_micros: intent
            .expires_micros
            .saturating_add(COMMAND_RESULT_WINDOW_MICROS),
    };
    let outcome = target.apply_admitted_party_command(&admitted)?;
    if outcome != CompanionCommandOutcome::WaitingForCapacity {
        party_command_abort_point(abort_after.as_deref(), intent.source_identity, intent.id);
        source.finish_party_command_intent(intent.id, claim_token, outcome)?;
    }
    Ok(outcome)
}

impl From<GroupRefusal> for PartyOutcome {
    fn from(refusal: GroupRefusal) -> Self {
        Self::Refused(refusal)
    }
}

// `resolve_by_name`, `resolve_all_by_name`, `character_anywhere` and `live_anywhere` moved to
// `presence.rs`: every realm-wide Character read now lives in one place, which guild rosters,
// friends, `/who` and whisper all share. `world::mail` (the mail workstream) still calls three of
// them through `party::`; kept re-exported here until that workstream merges onto `presence::`
// directly.
pub(crate) use super::presence::{character_anywhere, live_anywhere, resolve_all_by_name};

/// `self_guid`'s own Group roster, read from the authority: Realm-core when sharded, this handle's
/// own tables otherwise. Shared by [`resolve_roster_member_by_name`] and
/// [`resolve_roster_members_by_name`] so a caller resolving more than one name reads the roster
/// once rather than once per name, which would let two names resolve against two different
/// snapshots of a roster another op changed in between.
fn own_group_roster<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: u64,
) -> Result<Option<GroupRoster>> {
    match store.realm_store() {
        Some(realm) => realm.group_roster(self_guid),
        None => store.group_roster(self_guid),
    }
}

/// Resolve a typed name against `roster`'s members: `None` for a name matching nobody there.
fn resolve_in_roster<St: WorldStore + ?Sized>(
    store: &St,
    roster: &GroupRoster,
    name: &str,
) -> Result<Option<u64>> {
    for member in &roster.members {
        if character_anywhere(store, member.guid)?
            .is_some_and(|character| character.name.eq_ignore_ascii_case(name))
        {
            return Ok(Some(member.guid));
        }
    }
    Ok(None)
}

/// Resolve a typed name against `self_guid`'s OWN roster, for `CMSG_GROUP_CHANGE_SUB_GROUP`.
/// cmangos's Swap Subgroup matches a typed name against the group's own member list this way
/// (cm:GroupHandler.cpp:919-936); its Change Subgroup instead resolves the name realm-wide and
/// refuses afterward when the result is not a member. LyraCore applies the member-list rule to
/// both opcodes, so neither can reach a namesake standing outside the Raid — unlike
/// [`presence::resolve_by_name`]. `None` for no Group, or a name matching no member.
pub(crate) fn resolve_roster_member_by_name<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: u64,
    name: &str,
) -> Result<Option<u64>> {
    let Some(roster) = own_group_roster(store, self_guid)? else {
        return Ok(None);
    };
    resolve_in_roster(store, &roster, name)
}

/// [`resolve_roster_member_by_name`] for `CMSG_GROUP_SWAP_SUB_GROUP`'s two names, resolved against
/// ONE roster read rather than two: reading the roster separately per name could resolve the pair
/// against two different snapshots if another op changed the roster in between, letting a swap
/// name a member who had already left. `(None, None)` for no Group.
pub(crate) fn resolve_roster_members_by_name<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: u64,
    first_name: &str,
    second_name: &str,
) -> Result<(Option<u64>, Option<u64>)> {
    let Some(roster) = own_group_roster(store, self_guid)? else {
        return Ok((None, None));
    };
    Ok((
        resolve_in_roster(store, &roster, first_name)?,
        resolve_in_roster(store, &roster, second_name)?,
    ))
}

/// Route admission to the World Shard that reports the live entity. The acknowledged operation
/// checks current Session ownership and consent there, even if that presence read was stale.
fn admit_sessionless_answer<St: WorldStore + ?Sized>(
    store: &St,
    guid: u64,
) -> Result<PartyOutcome> {
    if store.entity_in_world(guid) {
        return store.admit_sessionless_group_action(guid);
    }
    if let Some(shard) = store
        .world_stores()
        .into_iter()
        .find(|shard| shard.entity_in_world(guid))
    {
        return shard.admit_sessionless_group_action(guid);
    }
    Ok(PartyOutcome::Refused(GroupRefusal::ActorUnavailable))
}

/// Admit the automatic answer on the owning World Shard, then apply party rules on Realm-core.
/// Suppression or unavailable admission leaves the invite untouched. Admission and membership
/// commit on separate Shards, so a later controller selection cannot recall an admitted answer.
fn answer_for_session_less<St: WorldStore + ?Sized>(store: &St, realm: &dyn WorldStore, guid: u64) {
    let admission = admit_sessionless_answer(store, guid);
    match admission {
        Ok(PartyOutcome::Ran) => {}
        Ok(PartyOutcome::Refused(_)) => return,
        Err(error) => {
            log::warn!("party: session-less {guid} admission unavailable: {error:#}");
            return;
        }
    }
    let joined = match run_on_authority(realm, guid, Op::Accept) {
        Ok(PartyOutcome::Ran) => {
            log::info!("party: session-less {guid} accepted its group invite");
            return;
        }
        Ok(PartyOutcome::Refused(refusal)) => format!("{refusal:?}"),
        Err(e) => format!("{e:#}"),
    };
    log::info!("party: session-less {guid} cannot join ({joined}), declining explicitly");
    match run_on_authority(realm, guid, Op::Decline) {
        Ok(PartyOutcome::Ran) => {}
        outcome => log::warn!(
            "party: session-less {guid} could neither join nor decline ({outcome:?}). The \
             inviter's dialog stands until realm-core's invite GC reaps it"
        ),
    }
}

/// Run one party op for the session that owns `self_guid`.
///
/// Unsharded → the pre-realm-core path, verbatim: the player's own connection calls the player-facing
/// reducer on the player's own shard, and nothing else happens. A raid op, a leadership op or a
/// Group Broadcast has no player-facing reducer, so it calls `realm_group_op` on that same shard.
///
/// Sharded → realm-core runs the op, then every connected world shard's mirror is refreshed. The
/// mirror refresh is best-effort BY DESIGN (see [`sync_mirrors`]); the op's own result is not. A
/// Group Broadcast runs on Realm-core and nothing else happens.
pub(crate) fn run<St: WorldStore + ?Sized>(
    store: &St,
    account_id: u64,
    self_guid: u64,
    op: Op,
) -> Result<PartyOutcome> {
    if let Op::Invite(target) = op {
        if cross_faction_invite(store, self_guid, target)? {
            return Ok(GroupRefusal::WrongFaction.into());
        }
    }
    // Vanilla passes the lead only to an online member (cm:GroupHandler.cpp:352-356). The party
    // authority runs its rules without presence on both planes, so the Gateway answers it here.
    if let Op::SetLeader(target) = op {
        if !live_anywhere(store, target) {
            return Ok(GroupRefusal::TargetOffline.into());
        }
    }
    let Some(realm) = store.realm_store() else {
        return match op {
            Op::Invite(target) => store.group_invite(account_id, self_guid, target),
            Op::Accept => store.group_accept(account_id, self_guid),
            Op::Decline => store.group_decline(account_id, self_guid),
            Op::Leave => store.group_leave(account_id, self_guid),
            Op::Uninvite(target) => store.group_uninvite(account_id, self_guid, target),
            Op::LootMethod {
                setting,
                master,
                threshold,
            } => store.group_loot_method(account_id, self_guid, setting, master, threshold),
            // A raid op, a leadership op or a Group Broadcast has no player-facing reducer. With one
            // database, the home shard holds the party, so it runs the same `realm_group_op`
            // Realm-core would.
            Op::RaidConvert
            | Op::SetLeader(_)
            | Op::SetAssistant { .. }
            | Op::ChangeSubgroup { .. }
            | Op::SwapSubgroup { .. }
            | Op::ReadyCheckStart
            | Op::ReadyCheckAnswer(_)
            | Op::TargetIcon { .. }
            | Op::MinimapPing { .. }
            | Op::RandomRoll { .. } => run_on_authority(store, self_guid, op),
        };
    };
    if op.is_group_broadcast() {
        return run_on_authority(realm.as_ref(), self_guid, op);
    }
    // The two gates realm-core cannot run for itself, because the directory database holds neither
    // characters nor live entities: does the target EXIST, and is it ONLINE. The gateway is the only
    // party that can answer them across a boundary — which is precisely the bug being fixed — and it
    // answers with the module's own Refusals, so the client-facing code is the same on both planes.
    //
    // Each gate is the module's own read, unioned across the shards — EXISTS is a `game_character`
    // row ([`presence::of`]), ONLINE is a `game_world_entity` row ([`presence::live_anywhere`]).
    // Reading the session flag for the second would silently refuse every playerbot; see
    // [`presence::live_anywhere`].
    if let Op::Invite(target) = op {
        if let Some(refusal) = invite_gate(store, target)? {
            return Ok(refusal.into());
        }
    }
    // The group this character was in BEFORE the op — the only way to reach the party they may have
    // just left (their membership row is gone by the time we look again, but the members still in it
    // need their mirrors updated too).
    let before = realm.group_roster(self_guid)?;
    // Found in adversarial review: LEAVE/UNINVITE are the only two ops that can shrink a group
    // below 2 members and reach `remove_member`'s disband branch on realm-core — and that branch
    // force-resolves live loot rolls, which the periodic loot-roll relay may not have promoted yet. A
    // roll staged in the gap between its kill-time creation and its next scheduled promotion is
    // invisible to `remove_member` if a disband lands in that gap ("someone gets kicked right after a
    // kill"), and the relay then promotes an ORPHANED roll onto a group id that no longer exists,
    // which resolves only at the 60s deadline — exactly the fallback the operator's decision
    // rejected, reintroduced in a narrow window. Flushing HERE, synchronously, in-line with the
    // dispatch below (not on the relay's own timer), closes it: see `loot::flush_pending_promotions`.
    if matches!(op, Op::Leave | Op::Uninvite(_)) {
        crate::world::loot::flush_pending_promotions(store, realm.as_ref());
    }
    if let PartyOutcome::Refused(refusal) = run_on_authority(realm.as_ref(), self_guid, op)? {
        return Ok(PartyOutcome::Refused(refusal));
    }
    // Nobody is at the keyboard of a playerbot, so nobody answers its dialog. Done
    // BEFORE the mirror push, so the ONE push that follows already carries the bot as a member —
    // which is what the shard's own party reads (kill-XP split, `/p`, follow-the-leader) need.
    if let Op::Invite(target) = op {
        answer_for_session_less(store, realm.as_ref(), target);
    }
    if op_changed_nothing(op, before.as_ref()) {
        return Ok(PartyOutcome::Ran);
    }
    sync_mirrors(store, realm.as_ref(), self_guid, before);
    Ok(PartyOutcome::Ran)
}

/// Whether a successful op changed nothing, so no mirror needs a push: converting a Raid again,
/// repeating a promotion or a demotion, a Change Subgroup into the Subgroup a member already
/// holds, or a Swap Subgroup between two members of one Subgroup. The answer comes from the roster
/// read before the op, not after it. The op returns on a call pipe before the Coordinator cache
/// holds its rows, so a read after it can still show the old roster and hide a real change. A
/// stale `before` costs at most a missed push, which the next op or world entry repairs, as
/// [`sync_mirrors`] documents.
fn op_changed_nothing(op: Op, before: Option<&GroupRoster>) -> bool {
    let Some(before) = before else {
        return false;
    };
    let subgroup_of = |guid| {
        before
            .members
            .iter()
            .find(|member| member.guid == guid)
            .map(|member| member.slot.subgroup())
    };
    match op {
        // A Raid never converts back.
        Op::RaidConvert => before.kind == GroupKind::Raid,
        Op::SetAssistant { target, promote } => before
            .members
            .iter()
            .any(|member| member.guid == target && member.slot.is_assistant() == promote),
        Op::ChangeSubgroup { target, subgroup } => subgroup_of(target) == Some(subgroup),
        Op::SwapSubgroup { first, second } => {
            let (first, second) = (subgroup_of(first), subgroup_of(second));
            first.is_some() && first == second
        }
        _ => false,
    }
}

/// The invite gates realm-core cannot run for itself: does the target exist anywhere, and is it in
/// the world anywhere. `None` means the invite may proceed.
fn invite_gate<St: WorldStore + ?Sized>(store: &St, target: u64) -> Result<Option<GroupRefusal>> {
    if presence::of(store, target)?.is_none() {
        return Ok(Some(GroupRefusal::NoSuchPlayer));
    }
    if !presence::live_anywhere(store, target) {
        return Ok(Some(GroupRefusal::TargetOffline));
    }
    Ok(None)
}

/// Vanilla's default refuses a party across factions (cm:GroupHandler.cpp:80,
/// `AllowTwoSide.Interaction.Group = 0`). Neither the Module nor Realm-core can read both races, so
/// the Gateway answers it on both planes, the way mail applies `same_team`. It runs only for a live
/// target: vanilla looks the target up among online players first, so a missing or offline target
/// keeps its own Refusal.
fn cross_faction_invite<St: WorldStore + ?Sized>(
    store: &St,
    inviter: u64,
    target: u64,
) -> Result<bool> {
    if !presence::live_anywhere(store, target) {
        return Ok(false);
    }
    let (Some(inviter), Some(target)) = (
        presence::character_anywhere(store, inviter)?,
        presence::character_anywhere(store, target)?,
    ) else {
        return Ok(false);
    };
    Ok(!lyracore_shared::faction::same_team(
        inviter.race,
        target.race,
    ))
}

/// Claim one subscribed intent on its World Shard, then execute it only for the winning Gateway.
///
/// `op` says which party op the row asks for ([`bot_op`]). A byte this gateway does not know is a
/// module newer than the gateway, which is a deployment fault rather than a party outcome — it is
/// refused by name instead of falling through to an invite.
pub(crate) fn run_bot_invite_intent<St: WorldStore>(
    store: &St,
    intent_id: u64,
    op: u8,
    inviter_guid: u64,
    target_guid: u64,
) -> Result<PartyOutcome> {
    match store.claim_bot_invite_intent(intent_id)? {
        PartyOutcome::Ran => {}
        PartyOutcome::Refused(GroupRefusal::IntentAlreadyClaimed) => return Ok(PartyOutcome::Ran),
        refusal @ PartyOutcome::Refused(_) => return Ok(refusal),
    }
    match op {
        bot_op::INVITE => run_bot_invite(store, inviter_guid, target_guid),
        bot_op::LEAVE => run_bot_leave(store, inviter_guid),
        unknown => anyhow::bail!("unknown bot group intent op {unknown}"),
    }
}

/// Run a SERVER-DRIVEN invite with no client behind it — a playerbot's serendipity pick, closing
/// the gap the group slice opened: the module used to write this shard's LOCAL
/// `game_group`/`game_group_member` rows directly, which the next `sync_group_mirror` push wiped
/// because realm-core had never heard of them.
///
/// There is no `account_id` here on purpose: a bot has no per-account connection for a reducer to
/// authenticate as, on EITHER topology. So this never takes [`run`]'s unsharded arm (which needs
/// exactly that connection to resolve the acting character via `ctx.sender()`) — it always uses the
/// guid-based `realm_group_op`, targeting realm-core when the gateway is sharded and this database's
/// OWN `game_group`/`game_group_member` otherwise. Unsharded that is still correct, not a special
/// case: with one database, "realm-core" and "the world shard" are the same physical database, so
/// writing there directly cannot diverge from itself — there is no mirror to contradict.
///
/// Otherwise this mirrors [`run`]'s sharded arm: same existence/online gates, same
/// [`answer_for_session_less`] (a bot target still has no client to answer its own dialog — true of
/// the INVITER here too, which is the whole point), same [`sync_mirrors`] push so the shard's own
/// party reads (kill-XP split, `/p`, follow-the-leader) see the new member immediately.
pub(crate) fn run_bot_invite<St: WorldStore>(
    store: &St,
    inviter_guid: u64,
    target_guid: u64,
) -> Result<PartyOutcome> {
    let owned_realm;
    let realm: &dyn WorldStore = match store.realm_store() {
        Some(r) => {
            owned_realm = r;
            owned_realm.as_ref()
        }
        None => store,
    };
    if let Some(refusal) = invite_gate(store, target_guid)? {
        return Ok(refusal.into());
    }
    let before = realm.group_roster(inviter_guid)?;
    if let PartyOutcome::Refused(refusal) =
        run_on_authority(realm, inviter_guid, Op::Invite(target_guid))?
    {
        return Ok(PartyOutcome::Refused(refusal));
    }
    answer_for_session_less(store, realm, target_guid);
    sync_mirrors(store, realm, inviter_guid, before);
    Ok(PartyOutcome::Ran)
}

/// Run a SERVER-DRIVEN leave with no client behind it — a bot leader whose party has run out of
/// shared work parts ways with it.
///
/// The same authority wall as [`run_bot_invite`], for the same reason: membership is authoritative
/// on realm-core, and a module-side `leave_group_for` would write this shard's own member rows for
/// the next `sync_group_mirror` push to put straight back.
///
/// No existence or online gate. Those two answer "may this target be invited"; a leave names only
/// the actor, and `leave_group_for` refuses a Character that is in no party — which is also what a
/// decision the previous tick already got executed looks like.
///
/// The pending-roll flush is [`run`]'s, verbatim, and belongs here for the reason it gives: a LEAVE
/// can shrink a group below two members and reach realm-core's disband branch, which force-resolves
/// live loot rolls that the periodic relay may not have promoted yet.
pub(crate) fn run_bot_leave<St: WorldStore>(store: &St, leaver_guid: u64) -> Result<PartyOutcome> {
    let owned_realm;
    let realm: &dyn WorldStore = match store.realm_store() {
        Some(r) => {
            owned_realm = r;
            owned_realm.as_ref()
        }
        None => store,
    };
    let leave = run_server_leave(store, realm, leaver_guid, 1, |realm, character_guid| {
        run_on_authority(realm, character_guid, Op::Leave)
    })?;
    if leave.outcome == PartyOutcome::Ran {
        sync_mirrors(store, realm, leaver_guid, leave.previous_roster);
    }
    Ok(leave.outcome)
}

struct ServerLeave {
    outcome: PartyOutcome,
    previous_roster: Option<GroupRoster>,
}

fn run_server_leave<St: WorldStore + ?Sized>(
    store: &St,
    realm: &dyn WorldStore,
    leaver_guid: u64,
    attempts: usize,
    leave_party: impl Fn(&dyn WorldStore, u64) -> Result<PartyOutcome>,
) -> Result<ServerLeave> {
    let before = realm.group_roster(leaver_guid)?;
    crate::world::loot::flush_pending_promotions(store, realm);
    let mut last_error = None;
    for _ in 0..attempts {
        match leave_party(realm, leaver_guid) {
            Ok(PartyOutcome::Ran) => {
                return Ok(ServerLeave {
                    outcome: PartyOutcome::Ran,
                    previous_roster: before,
                });
            }
            Ok(PartyOutcome::Refused(GroupRefusal::NotInGroup))
                if before.is_some() && last_error.is_some() =>
            {
                return Ok(ServerLeave {
                    outcome: PartyOutcome::Ran,
                    previous_roster: before,
                });
            }
            Ok(PartyOutcome::Refused(refusal)) => {
                return Ok(ServerLeave {
                    outcome: PartyOutcome::Refused(refusal),
                    previous_roster: before,
                });
            }
            Err(error) => last_error = Some(error),
        }
    }

    let Some(error) = last_error else {
        return Ok(ServerLeave {
            outcome: PartyOutcome::Refused(GroupRefusal::NotInGroup),
            previous_roster: before,
        });
    };
    if before.is_some() {
        match realm.group_roster(leaver_guid) {
            Ok(None) => {
                return Ok(ServerLeave {
                    outcome: PartyOutcome::Ran,
                    previous_roster: before,
                });
            }
            Ok(Some(_)) => {}
            Err(confirmation_error) => {
                log::warn!(
                    "party: could not confirm Character {leaver_guid} membership after Realm-core \
                     LEAVE failed ({confirmation_error:#}); retrying"
                );
            }
        }
    }
    Err(error)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeletedCharacterPartyCleanup {
    Preserved,
    Removed,
    AlreadyClean,
}

const DELETED_CHARACTER_LEAVE_ATTEMPTS: usize = 3;

/// Remove a deleted Character from its realm-core party after every World Shard confirms absence.
pub(crate) fn cleanup_deleted_character<St: WorldStore>(
    store: &St,
    character_guid: u64,
) -> Result<DeletedCharacterPartyCleanup> {
    let Some(realm) = store.party_cleanup_realm()? else {
        return Ok(DeletedCharacterPartyCleanup::AlreadyClean);
    };
    if store.character_exists_on_any_world_shard(character_guid)? {
        return Ok(DeletedCharacterPartyCleanup::Preserved);
    }
    let leave = run_server_leave(
        store,
        realm.as_ref(),
        character_guid,
        DELETED_CHARACTER_LEAVE_ATTEMPTS,
        |realm, character_guid| realm.deleted_character_party_leave(character_guid),
    )?;
    match leave.outcome {
        PartyOutcome::Ran => {
            if let Some(previous) = leave.previous_roster {
                sync_group_mirrors_required(
                    store,
                    realm.as_ref(),
                    previous.group_id,
                    Some(&previous),
                )?;
            }
            Ok(DeletedCharacterPartyCleanup::Removed)
        }
        PartyOutcome::Refused(GroupRefusal::NotInGroup) => {
            Ok(DeletedCharacterPartyCleanup::AlreadyClean)
        }
        PartyOutcome::Refused(refusal) => {
            anyhow::bail!("realm-core refused deleted Character cleanup: {refusal:?}")
        }
    }
}

/// Recheck authoritative party members after startup or a Coordinator reconnect. Row-delete
/// callbacks are not replayed, so this closes cleanup attempts deferred while a Shard was down.
pub(crate) fn reconcile_deleted_character_parties<St: WorldStore>(store: &St) -> Result<()> {
    let Some(realm) = store.party_cleanup_realm()? else {
        return Ok(());
    };
    let mut failures = 0usize;
    let mut last_error = None;
    for character_guid in realm.party_member_guids()? {
        if let Err(error) = cleanup_deleted_character(store, character_guid) {
            failures += 1;
            log::warn!(
                "party: could not finish reconciling Character {character_guid} ({error:#}); retrying"
            );
            last_error = Some(error);
        }
    }
    let mut group_ids: std::collections::HashSet<u64> =
        realm.party_group_ids()?.into_iter().collect();
    for shard in store.world_stores() {
        match shard.party_group_ids() {
            Ok(ids) => group_ids.extend(ids),
            Err(error) => {
                failures += 1;
                log::warn!(
                    "party: could not enumerate mirrored groups on {} ({error:#}); retrying",
                    shard.shard_name()
                );
                last_error = Some(error);
            }
        }
    }
    for group_id in group_ids {
        if let Err(error) = sync_group_mirrors_required(store, realm.as_ref(), group_id, None) {
            failures += 1;
            log::warn!("party: could not reconcile group {group_id} mirrors ({error:#}); retrying");
            last_error = Some(error);
        }
    }
    match last_error {
        Some(error) => Err(error.context(format!(
            "{failures} deleted Character party reconciliation attempt(s) were deferred"
        ))),
        None => Ok(()),
    }
}

const DELETED_CHARACTER_MIRROR_ATTEMPTS: usize = 3;

fn sync_group_mirrors_required<St: WorldStore + ?Sized>(
    store: &St,
    realm: &dyn WorldStore,
    group_id: u64,
    previous: Option<&GroupRoster>,
) -> Result<()> {
    let roster = match realm.party_cleanup_group_roster_by_id(group_id)? {
        Some(roster) => roster,
        None => GroupRoster::disbanded(group_id, realm.group_roster_revision(group_id)?),
    };
    let mut roster = certify_roster_partitions(store, realm, roster)?;
    append_departed_partitions(&mut roster, previous);
    let mut failures = 0usize;
    let mut last_error = None;
    for shard in store.world_stores() {
        let mut synced = false;
        for _ in 0..DELETED_CHARACTER_MIRROR_ATTEMPTS {
            match shard.sync_group_mirror(&roster) {
                Ok(()) => {
                    synced = true;
                    break;
                }
                Err(error) => last_error = Some(error),
            }
        }
        if !synced {
            failures += 1;
        }
    }
    match last_error {
        Some(error) if failures > 0 => Err(error.context(format!(
            "{failures} World Shard party mirror update(s) failed after \
             {DELETED_CHARACTER_MIRROR_ATTEMPTS} attempts"
        ))),
        _ => Ok(()),
    }
}

/// Push realm-core's roster for every party this op could have touched onto every connected world
/// shard.
///
/// Two groups at most: the one the character is in NOW and the one they were in BEFORE (a leave, a
/// kick, or an accept that moved them). A group that realm-core no longer has is pushed as
/// [`GroupRoster::disbanded`], which is how a shard learns to drop it.
///
/// **Best-effort, deliberately.** The authoritative write has already committed on realm-core by the
/// time this runs, so a failed mirror push must not turn a party op that DID happen into an error
/// the client renders as a failure. The cost of a miss is bounded and self-healing: that shard's
/// local reads (XP split, loot rules) use a stale roster until the next op or the next world entry
/// re-pushes it, and the party FRAME — which is what the player sees — is rendered from realm-core
/// through the relay, not from the mirror.
pub(crate) fn sync_mirrors<St: WorldStore + ?Sized>(
    store: &St,
    realm: &dyn WorldStore,
    self_guid: u64,
    before: Option<GroupRoster>,
) {
    let now = match realm.group_roster(self_guid) {
        Ok(r) => r,
        Err(e) => {
            log::warn!(
                "party: could not read the realm-core roster for {self_guid} ({e:#}) — the shard mirrors keep their previous roster until the next op or world entry"
            );
            return;
        }
    };
    let mut touched: Vec<u64> = Vec::new();
    if let Some(r) = &now {
        touched.push(r.group_id);
    }
    if let Some(before) = &before {
        if !touched.contains(&before.group_id) {
            touched.push(before.group_id);
        }
    }
    for group_id in touched {
        let roster = match roster_or_disbanded(realm, group_id) {
            Ok(r) => r,
            // Gone from the authority = disbanded. Push the tombstone rather than skipping, or the
            // shards keep a party that no longer exists and its members stay grouped locally.
            Err(e) => {
                log::warn!(
                    "party: could not read realm-core group {group_id} ({e:#}) — shard mirrors unchanged"
                );
                continue;
            }
        };
        let mut roster = match certify_roster_partitions(store, realm, roster) {
            Ok(roster) => roster,
            Err(e) => {
                log::warn!(
                    "party: could not certify group {group_id} member partitions ({e:#}); shard mirrors unchanged"
                );
                continue;
            }
        };
        append_departed_partitions(&mut roster, before.as_ref());
        for shard in store.world_stores() {
            if let Err(e) = shard.sync_group_mirror(&roster) {
                log::warn!(
                    "party: could not mirror group {group_id} onto shard {} ({e:#}) — that shard's \
                     in-world party reads stay stale until the next op or world entry",
                    shard.shard_name()
                );
            }
        }
    }
}

/// World entry (login, and every cross-shard arrival): put the party the player is actually in onto
/// the shard they just entered, and re-render their party frame.
/// A Party member also gets its Target Icons again, after the frame.
///
/// **This is what carries a party across a shard boundary now that the escrowed transfer's blob
/// mirror is gone.** The
/// blob could only carry the membership the character had when it stepped into the portal; this
/// reads the authority at the moment of arrival, so a party formed — or joined, or left — while the
/// player was on the loading screen is what lands.
///
/// Unsharded → returns immediately, before any read: the shard's own tables already are the
/// authority and the login path is unchanged.
pub(crate) fn on_world_entry<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    self_guid: u64,
) -> Result<()> {
    let Some(roster) = sync_arrival_mirror(store, self_guid)? else {
        return Ok(());
    };
    send(tx, render_list(store, self_guid, &roster.list_payload()))?;
    if roster.kind == GroupKind::Party {
        request_target_icons(store, self_guid);
    }
    Ok(())
}

/// A Party client clears its marks on every `SMSG_GROUP_LIST` (vm:Group.cpp:1343-1360). The list
/// above carries no Target Icons, so ask the party authority for the full list, as the client's
/// own `0xFF` request does. The answer rides the group event relay onto the same session writer
/// and so lands after the list. A failure costs only the marks, so it logs.
fn request_target_icons<St: WorldStore + ?Sized>(store: &St, self_guid: u64) {
    let Some(realm) = store.realm_store() else {
        return;
    };
    let op = Op::TargetIcon {
        icon: lyracore_shared::group::TARGET_ICON_LIST_REQUEST,
        target: 0,
    };
    match run_on_authority(realm.as_ref(), self_guid, op) {
        Ok(PartyOutcome::Ran) => {}
        outcome => log::warn!(
            "party: Target Icon list for {self_guid} at world entry not requested: {outcome:?}"
        ),
    }
}

/// The mirror half of [`on_world_entry`], without a client: put the party realm-core says
/// `self_guid` is in onto the shard `store` names, and clear a mirror row of a party they have
/// left. Answers the roster it pushed so the caller can render a frame for a player. Transfer
/// settlement uses the strict sibling below before it drops an arrival fence.
///
/// Unsharded → `Ok(None)` before any read: the shard's own tables already are the authority.
pub(crate) fn sync_arrival_mirror<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: u64,
) -> Result<Option<GroupRoster>> {
    let Some(realm) = store.realm_store() else {
        return Ok(None);
    };
    let roster = realm
        .group_roster(self_guid)?
        .map(|roster| certify_roster_partitions(store, realm.as_ref(), roster))
        .transpose()?;
    // A mirror on THIS shard that still has the arriving character in a party realm-core no longer
    // has them in is the ONE staleness "re-syncs on the next op or world entry" does not cover by
    // itself: with no roster to push there was nothing to overwrite it with, and the ops of the party
    // they left never name them again — so the stale membership row survived every arrival, forever.
    // The shard then runs that character's kill-XP split, quest credit, loot rules and
    // dungeon binding against a party they are not in. Re-push the AUTHORITY's version of the group
    // the mirror thinks they are in (a tombstone if it is gone) — the same repair `sync_mirrors`
    // applies to the group an actor was in BEFORE an op.
    if let Some(stale) = store.group_roster(self_guid)? {
        if roster.as_ref().is_none_or(|r| r.group_id != stale.group_id) {
            let repair = roster_or_disbanded(realm.as_ref(), stale.group_id)?;
            let repair = certify_roster_partitions(store, realm.as_ref(), repair)?;
            // Best-effort, like every other mirror write: this is a cache repair, and failing the
            // world entry over it would be strictly worse than arriving with a stale roster.
            if let Err(e) = store.sync_group_mirror(&repair) {
                log::warn!(
                    "party: could not clear guid {self_guid} from shard {}'s stale mirror of group \
                     {} ({e:#}) — that shard's party reads for them stay wrong until they join one",
                    store.shard_name(),
                    stale.group_id
                );
            }
        }
    }
    let Some(roster) = roster else {
        return Ok(None);
    };
    // The shard the player just entered. `store` is already the session's home-shard handle (every
    // world-entry caller runs under `on_home_shard!`), so this is the one mirror that must exist
    // before the player takes a single action here.
    store.sync_group_mirror(&roster)?;
    Ok(Some(roster))
}

/// Reconcile the same authoritative roster as [`sync_arrival_mirror`], but make every destination
/// write part of Transfer settlement. The arrival fence stays up when realm-core or the mirror is
/// unavailable, and a later Transfer retry repeats this operation before release.
pub(crate) fn sync_transfer_arrival_mirror<St: WorldStore + ?Sized>(
    store: &St,
    character_guid: u64,
) -> Result<()> {
    let Some(realm) = store.party_cleanup_realm()? else {
        return Ok(());
    };
    let roster = realm
        .group_roster(character_guid)?
        .map(|roster| certify_roster_partitions(store, realm.as_ref(), roster))
        .transpose()?;
    if let Some(stale) = store.group_roster(character_guid)? {
        if roster
            .as_ref()
            .is_none_or(|row| row.group_id != stale.group_id)
        {
            let repair = roster_or_disbanded(realm.as_ref(), stale.group_id)?;
            let repair = certify_roster_partitions(store, realm.as_ref(), repair)?;
            store.sync_group_mirror(&repair)?;
        }
    }
    if let Some(roster) = roster {
        let shards = store.world_stores();
        if shards.is_empty() {
            store.sync_group_mirror(&roster)?;
        } else {
            for shard in shards {
                shard.sync_group_mirror(&roster)?;
            }
        }
    }
    Ok(())
}

/// Build `SMSG_GROUP_LIST` for `self_guid`. This is the one renderer: the LIST relay passes the
/// event's payload, and world entry passes the authoritative roster's [`GroupRoster::list_payload`].
/// The packet is raw because it ends with a byte gtker cannot encode (`codec::build_group_list_raw`).
///
/// Every member's ONLINE flag, and each blank NAME, comes from the shards. That is the price of
/// realm-core owning membership: the directory database has no `game_character` or
/// `game_world_entity` rows, so it cannot know what its members are called or whether they are in
/// the world. The gateway can — it reads every connected shard's cache — and it is the only party
/// that can answer for a member standing on a different database than the viewer. A name the
/// payload already carries is kept; the Module wrote it with the change. A member whose name will
/// not resolve (a shard that is down) renders with an empty name rather than being dropped from the
/// frame: a missing row in the party UI reads as "they left", which is a worse lie than a blank one.
pub(crate) fn render_list<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: u64,
    roster: &RosterPayload,
) -> Outbound {
    let mut roster = roster.clone();
    for member in &mut roster.members {
        if member.name.is_empty() {
            // This handle first (the viewer's own shard, where most of the party is), then every
            // other connected one — the member standing inside an instance is on a database this
            // handle never reads. Empty on a single-database gateway, so the union never leaves it.
            member.name = presence::character_anywhere(store, member.guid)
                .ok()
                .flatten()
                .map(|c| c.name)
                .unwrap_or_default();
        }
    }
    let list = codec::build_group_list(self_guid, &roster, |guid| {
        presence::live_anywhere(store, guid)
    });
    let (opcode, body) = codec::build_group_list_raw(&list);
    Outbound::Raw { opcode, body }
}

#[cfg(test)]
mod partition_tests {
    use super::*;

    const LOCATOR: RealmCharacterPartition = RealmCharacterPartition {
        map_id: 0,
        instance_id: 0,
        revision: 7,
        transfer_pending: false,
        pending_destination_map: 0,
        pending_destination_instance: 0,
        bot_source_identity: spacetimedb_sdk::Identity::ZERO,
        bot_transfer_intent_id: 0,
        bot_controller_generation: 0,
    };

    #[test]
    fn only_the_shard_serving_the_captured_locator_can_report_a_pending_transfer() {
        let state = classify_party_partition(
            LOCATOR,
            &[
                PartyHolderObservation {
                    serves_locator: false,
                    has_escrow: true,
                    character_partition: Some((36, 1)),
                },
                PartyHolderObservation {
                    serves_locator: true,
                    has_escrow: false,
                    character_partition: Some((0, 0)),
                },
            ],
        )
        .expect("one settled holder");

        assert_eq!(state, PartyPartitionState::Known);
    }

    #[test]
    fn the_serving_shards_escrow_suppresses_its_stale_character_location() {
        let state = classify_party_partition(
            LOCATOR,
            &[PartyHolderObservation {
                serves_locator: true,
                has_escrow: true,
                character_partition: Some((36, 1)),
            }],
        )
        .expect("one pending holder");

        assert_eq!(state, PartyPartitionState::PendingTransfer);
    }

    #[test]
    fn a_departed_member_keeps_the_realms_membership_revision_as_a_removal_fence() {
        let departed = GroupMemberPartition {
            character_guid: 100,
            group_id: 7,
            membership_revision: 42,
            member_active: true,
            map_id: 0,
            instance_id: 0,
            locator_revision: 3,
            state: PartyPartitionState::Known,
        };
        let previous = GroupRoster {
            group_id: 7,
            members: vec![GroupRosterMember {
                guid: 100,
                slot: RaidSlot::default(),
            }],
            partitions: vec![departed],
            ..Default::default()
        };
        let mut current = GroupRoster::disbanded(7, 2);

        append_departed_partitions(&mut current, Some(&previous));

        assert_eq!(
            current.partitions,
            vec![GroupMemberPartition {
                member_active: false,
                state: PartyPartitionState::Unknown,
                ..departed
            }]
        );
    }
}
