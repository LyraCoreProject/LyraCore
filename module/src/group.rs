mod mirror;
#[cfg_attr(not(feature = "debug_reducers"), allow(unused_imports))]
pub use mirror::sync_group_mirror;

use spacetimedb::{reducer, table, Identity, ReducerContext, Table, Timestamp};

use crate::native::game_native_player;
use crate::{game_character, game_melee_attack, game_pending_cast, game_threat, game_world_entity};

pub use lyracore_shared::group::GROUP_MAX_MEMBERS;
use lyracore_shared::group::{
    GroupKind, RaidSlot, RosterMember, RosterPayload, TargetIcon, RAID_MAX_MEMBERS, RAID_SUBGROUPS,
    TARGET_ICON_COUNT, TARGET_ICON_LIST_REQUEST,
};

pub const GROUP_XP_RANGE_SQ: f32 = 74.0 * 74.0;

pub mod loot_method {
    pub const FFA: u8 = 0;
    pub const ROUND_ROBIN: u8 = 1;
    pub const MASTER: u8 = 2;
    pub const GROUP: u8 = 3;
    pub const NEED_BEFORE_GREED: u8 = 4;
}

#[table(accessor = game_group, public)]
pub struct Group {
    #[primary_key]
    #[auto_inc]
    pub group_id: u64,
    pub leader_guid: u64,

    #[default(3)]
    pub loot_method: u8,
    #[default(2)]
    pub loot_threshold: u8,
    #[default(0)]
    pub rr_cursor: u32,
    #[default(0u64)]
    pub master_looter_guid: u64,
    #[default(0u8)]
    pub group_type: u8,
}

#[table(
    accessor = game_group_member,
    public,
    index(accessor = by_group, btree(columns = [group_id])),
    index(accessor = by_character, btree(columns = [character_guid]))
)]
pub struct GroupMember {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub group_id: u64,
    pub character_guid: u64,
    pub owner_identity: Identity,
    #[default(0u8)]
    pub raid_slot: u8,
}

// Keep this revision after disband so a delayed roster cannot recreate the party.
#[table(accessor = game_group_roster_revision, public)]
pub struct GroupRosterRevision {
    #[primary_key]
    pub group_id: u64,
    pub revision: u64,
    pub active: bool,
}

#[table(
    accessor = game_group_member_partition,
    public,
    index(accessor = by_group, btree(columns = [group_id]))
)]
pub struct GroupMemberPartition {
    #[primary_key]
    pub character_guid: u64,
    pub group_id: u64,
    pub membership_revision: u64,
    // A removed member remains fenced until a newer membership revision arrives.
    pub member_active: bool,
    pub map_id: u32,
    pub instance_id: u64,
    pub locator_revision: u64,
    pub state: PartyPartitionState,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartyPartitionState {
    Unknown,
    Known,
    PendingTransfer,
}

crate::character_owned!(delete, fn sweep_delete_game_group_member_partition(ctx, character_guid) {
    ctx.db
.game_group_member_partition()
.character_guid()
.delete(character_guid);
});
crate::character_owned!(not_transported, fn sweep_transfer_game_group_member_partition());

crate::character_owned!(delete, fn sweep_delete_game_group_member(ctx, character_guid) {
    if group_of(ctx, character_guid).is_none() {
        return;
    }
    let deleted = GroupAction::Leave {
        cause: leave_cause::CHARACTER_DELETED,
    };
    let left = apply_action(ctx, Plane::Shard, character_guid, deleted);
    // A member row whose Group is missing still goes with its Character.
    if let Err(GroupOpError::Invariant(_)) = left {
        remove_member(ctx, character_guid, Departure::Left);
    }
});
crate::character_owned!(restamp, fn sweep_restamp_game_group_member(ctx, character_guid, identity) {
    let members = ctx.db.game_group_member();
    for mut m in members.by_character().filter(&character_guid).collect::<Vec<_>>() {
        if m.owner_identity != identity {
            m.owner_identity = identity;
            members.id().update(m);
        }
    }
});

// Membership snapshots must not travel because they can overwrite a newer roster.
crate::character_owned!(not_transported, fn sweep_transfer_game_group_member());

// Transfer removes the local mirror without leave or disband semantics.
pub(crate) fn detach_for_transfer(ctx: &ReducerContext, character_guid: u64) {
    let members = ctx.db.game_group_member();
    for m in members
        .by_character()
        .filter(&character_guid)
        .collect::<Vec<_>>()
    {
        members.id().delete(m.id);
    }
}

#[table(accessor = game_group_invite, index(accessor = by_target, btree(columns = [target_guid])))]
pub struct GroupInvite {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub target_guid: u64,
    pub inviter_guid: u64,
    pub created_at: Timestamp,
    #[default(0u64)]
    pub group_id: u64,
}

crate::character_owned!(delete, fn sweep_delete_game_group_invite(ctx, character_guid) {
    let invites = ctx.db.game_group_invite();
    for inv in invites.by_target().filter(&character_guid).collect::<Vec<_>>() {
        invites.id().delete(inv.id);
    }
    let sent: Vec<u64> = invites
.iter()
.filter(|i| i.inviter_guid == character_guid)
.map(|i| i.id)
.collect();
    for id in sent {
        invites.id().delete(id);
    }
});
crate::character_owned!(not_transported, fn sweep_transfer_game_group_invite());

#[table(accessor = game_bot_invite_intent, index(accessor = by_actor, btree(columns = [inviter_guid])))]
pub struct BotInviteIntent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub inviter_guid: u64,
    pub target_guid: u64,
    pub created_at: Timestamp,
    #[default(0u8)]
    pub op: u8,
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn emit_bot_invite_intent(ctx: &ReducerContext, inviter_guid: u64, target_guid: u64) {
    apply_sessionless_group_action(ctx, inviter_guid, GroupAction::Invite(target_guid));
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn emit_bot_leave_intent(ctx: &ReducerContext, leaver_guid: u64) {
    apply_sessionless_group_action(
        ctx,
        leaver_guid,
        GroupAction::Leave {
            cause: leave_cause::LEFT,
        },
    );
}

#[cfg_attr(not(has_packages), allow(dead_code))]
fn apply_sessionless_group_action(ctx: &ReducerContext, actor_guid: u64, action: GroupAction) {
    if let Err(refusal) = crate::sessionless::group_action_gate(ctx, actor_guid) {
        refused(refusal, "Companion group action");
        return;
    }
    let result = apply_action(ctx, Plane::Shard, actor_guid, action).and_then(|()| {
        if let GroupAction::Invite(target_guid) = action {
            answer_sessionless_invite(ctx, target_guid)
        } else {
            Ok(())
        }
    });
    if let Err(error @ GroupOpError::Invariant(_)) = result {
        // The Package API returns no outcome; an invariant failure must abort this transaction.
        panic!("Companion group invariant failed for {actor_guid}: {error}");
    }
}

pub(crate) fn answer_sessionless_invite(
    ctx: &ReducerContext,
    guid: u64,
) -> Result<(), GroupOpError> {
    let Some(character) = crate::helpers::character_by_guid(ctx, guid).filter(|c| !c.online) else {
        return Ok(());
    };
    if crate::sessionless::group_action_gate(ctx, guid).is_err()
        || ctx
            .db
            .game_group_invite()
            .by_target()
            .filter(guid)
            .next()
            .is_none()
    {
        return Ok(());
    }
    match apply_action(
        ctx,
        Plane::Shard,
        guid,
        GroupAction::Accept {
            class: character.class,
        },
    ) {
        Ok(()) => Ok(()),
        Err(GroupOpError::Refused(_)) => {
            apply_action(ctx, Plane::Shard, guid, GroupAction::Decline)
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn clear_unclaimed_group_intents(ctx: &ReducerContext, character_guid: u64) {
    let intents = ctx.db.game_bot_invite_intent();
    for intent in intents
        .by_actor()
        .filter(&character_guid)
        .collect::<Vec<_>>()
    {
        intents.id().delete(intent.id);
    }
}

pub(crate) fn refused(refusal: GroupRefusal, detail: &str) -> String {
    let tag = refusal.as_tag();
    spacetimedb::log::info!("group refused {tag}: {detail}");
    tag.to_string()
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum GroupOpError {
    Refused(GroupRefusal),
    Invariant(String),
}

impl From<GroupRefusal> for GroupOpError {
    fn from(refusal: GroupRefusal) -> Self {
        Self::Refused(refusal)
    }
}

impl std::fmt::Display for GroupOpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(refusal) => refusal.as_tag().fmt(f),
            Self::Invariant(reason) => write!(f, "group invariant failed: {reason}"),
        }
    }
}

fn logged(error: GroupOpError, detail: &str) -> GroupOpError {
    match &error {
        GroupOpError::Refused(refusal) => {
            spacetimedb::log::info!("group refused {}: {detail}", refusal.as_tag());
        }
        GroupOpError::Invariant(reason) => {
            spacetimedb::log::error!("group invariant failed: {detail}: {reason}");
        }
    }
    error
}

pub(crate) fn apply_action_for(
    ctx: &ReducerContext,
    actor_guid: u64,
    action: GroupAction,
) -> Result<(), String> {
    apply_action(ctx, Plane::Shard, actor_guid, action).map_err(|error| error.to_string())
}

use lyracore_shared::group::{event_kind as group_event_kind, leave_cause, GroupRefusal};

#[table(accessor = game_group_event, public, index(accessor = by_recipient, btree(columns = [recipient_identity])))]
pub struct GroupEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub recipient_identity: Identity,
    pub kind: u8,
    pub other_guid: u64,
    pub other_name: String,
    pub created_at: Timestamp,
    pub payload: String,
    #[default(0u64)]
    pub recipient_guid: u64,
}

pub(crate) fn push_event(
    ctx: &ReducerContext,
    recipient_guid: u64,
    kind: u8,
    other_guid: u64,
    payload: String,
) {
    crate::native::group_event(ctx, recipient_guid, kind, other_guid, &payload);
    let bound = ctx
        .db
        .game_character()
        .guid()
        .find(recipient_guid)
        .map(|c| c.owner_identity);
    ctx.db.game_group_event().insert(GroupEvent {
        id: 0,
        recipient_identity: crate::helpers::event_recipient_identity(bound),
        kind,
        other_guid,
        other_name: String::new(),
        created_at: ctx.timestamp,
        payload,
        recipient_guid,
    });
}

pub(crate) fn roster_payload(ctx: &ReducerContext, group_id: u64) -> Option<String> {
    let group = ctx.db.game_group().group_id().find(group_id)?;
    let members = members_of(ctx, group_id)
        .into_iter()
        .map(|m| RosterMember {
            guid: m.character_guid,
            name: ctx
                .db
                .game_character()
                .guid()
                .find(m.character_guid)
                .map(|c| c.name)
                .unwrap_or_default(),
            slot: raid_slot_of(&m),
        })
        .collect();
    let kind = group_kind_of(&group);
    Some(
        RosterPayload {
            leader: group.leader_guid,
            loot_method: group.loot_method,
            loot_threshold: group.loot_threshold,
            master_looter_guid: group.master_looter_guid,
            kind,
            members,
            // The Party client clears marks on each roster list, so send them back with it.
            target_icons: match kind {
                GroupKind::Party => target_icons_of(ctx, group_id),
                GroupKind::Raid => Vec::new(),
            },
        }
        .encode(),
    )
}

pub(crate) fn group_kind_of(group: &Group) -> GroupKind {
    GroupKind::from_wire_or_default(group.group_type)
}

pub(crate) fn raid_slot_of(member: &GroupMember) -> RaidSlot {
    RaidSlot::from_wire_or_default(member.raid_slot)
}

pub(crate) fn in_raid(ctx: &ReducerContext, character_guid: u64) -> bool {
    group_of(ctx, character_guid)
        .and_then(|member| ctx.db.game_group().group_id().find(member.group_id))
        .is_some_and(|group| group_kind_of(&group) == GroupKind::Raid)
}

pub(crate) fn manages_raid(group: &Group, member: &GroupMember) -> bool {
    group.leader_guid == member.character_guid || raid_slot_of(member).is_assistant()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GroupAudience {
    Everyone,
    AllExcept(u64),
    Leader,
    Subgroup(u8),
}

impl GroupAudience {
    fn includes(self, group: &Group, member: &GroupMember) -> bool {
        match self {
            Self::Everyone => true,
            Self::AllExcept(excluded) => member.character_guid != excluded,
            Self::Leader => member.character_guid == group.leader_guid,
            Self::Subgroup(subgroup) => raid_slot_of(member).subgroup() == subgroup,
        }
    }
}

pub(crate) fn group_broadcast(
    ctx: &ReducerContext,
    group_id: u64,
    audience: GroupAudience,
    kind: u8,
    other_guid: u64,
    payload: String,
) {
    let Some(group) = ctx.db.game_group().group_id().find(group_id) else {
        return;
    };
    for member in members_of(ctx, group_id) {
        if audience.includes(&group, &member) {
            push_event(
                ctx,
                member.character_guid,
                kind,
                other_guid,
                payload.clone(),
            );
        }
    }
}

fn has_room(kind: GroupKind, member_count: usize) -> bool {
    member_count < kind.member_cap()
}

pub(crate) fn group_of(ctx: &ReducerContext, character_guid: u64) -> Option<GroupMember> {
    ctx.db
        .game_group_member()
        .by_character()
        .filter(&character_guid)
        .next()
}

#[derive(Clone, Debug, PartialEq)]
pub struct PartyUnitFacts {
    pub map_id: u32,
    pub instance_id: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub health: u32,
    pub max_health: u32,
    pub dead: bool,
    pub target_guid: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PartyMemberFacts {
    pub character_guid: u64,
    pub unit: Option<PartyUnitFacts>,
    pub partition: Option<PartyPartitionFacts>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PartyPartitionFacts {
    pub map_id: u32,
    pub instance_id: u64,
    pub locator_revision: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PartyFacts {
    pub group_id: u64,
    pub leader_guid: u64,
    pub members: Vec<PartyMemberFacts>,
    pub enemies: Vec<PartyEnemyFacts>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PartyEnemyFacts {
    pub guid: u64,
    pub map_id: u32,
    pub instance_id: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub health: u32,
    pub max_health: u32,
    pub attacking_party: bool,
    pub party_attacking: bool,
    pub party_casting: bool,
    pub party_has_threat: bool,
    pub current_target_guid: Option<u64>,
    pub top_threat_guid: Option<u64>,
    pub control: Option<crate::spell::UnitControl>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompanionTargetFacts {
    pub guid: u64,
    pub map_id: u32,
    pub instance_id: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub health: u32,
    pub max_health: u32,
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn companion_target_facts(
    ctx: &ReducerContext,
    bot_guid: u64,
    target_guid: u64,
) -> Result<CompanionTargetFacts, crate::bridge::CommandOutcome> {
    let bot = ctx
        .db
        .game_world_entity()
        .guid()
        .find(bot_guid)
        .ok_or(crate::bridge::CommandOutcome::MissingBot)?;
    let target = ctx
        .db
        .game_world_entity()
        .guid()
        .find(target_guid)
        .ok_or(crate::bridge::CommandOutcome::TargetUnavailable)?;
    if (bot.map_id, bot.instance_id) != (target.map_id, target.instance_id) {
        return Err(crate::bridge::CommandOutcome::WrongPartition);
    }
    if target.dead || target.health == 0 {
        return Err(crate::bridge::CommandOutcome::TargetDead);
    }
    if target.is_player() || !crate::combat::may_harm(ctx, &bot, &target) {
        return Err(crate::bridge::CommandOutcome::TargetUnavailable);
    }
    if crate::spell::control_status(ctx, target_guid, 64)
        .map_err(|_| crate::bridge::CommandOutcome::TargetUnavailable)?
        .is_some()
    {
        return Err(crate::bridge::CommandOutcome::TargetControlled);
    }
    Ok(CompanionTargetFacts {
        guid: target.guid,
        map_id: target.map_id,
        instance_id: target.instance_id,
        x: target.x,
        y: target.y,
        z: target.z,
        health: target.health,
        max_health: target.max_health,
    })
}

pub(crate) fn admit_party_command(
    ctx: &ReducerContext,
    admitted: &crate::bridge::AdmittedClientCommand,
) -> Option<crate::bridge::CommandOutcome> {
    use crate::bridge::CommandOutcome;

    if let Err(refusal) = crate::sessionless::action_gate(ctx, admitted.command.bot_guid) {
        return Some(match refusal.kind {
            crate::actor::ActionRefusalKind::MissingActor => CommandOutcome::MissingBot,
            crate::actor::ActionRefusalKind::CannotAct => CommandOutcome::WrongAccount,
            _ => CommandOutcome::Suppressed,
        });
    }
    if let Err(refusal) = crate::sessionless::group_action_gate(ctx, admitted.command.bot_guid) {
        return Some(match refusal {
            lyracore_shared::group::GroupRefusal::ActorUnavailable => CommandOutcome::MissingBot,
            lyracore_shared::group::GroupRefusal::ActionSuppressed => CommandOutcome::Suppressed,
            _ => CommandOutcome::Suppressed,
        });
    }
    let Some(member) = group_of(ctx, admitted.command.bot_guid) else {
        return Some(CommandOutcome::StalePartyMirror);
    };
    let Some(group) = ctx.db.game_group().group_id().find(member.group_id) else {
        return Some(CommandOutcome::StalePartyMirror);
    };
    let Some(roster) = bounded_roster(ctx, member.group_id, group_kind_of(&group)) else {
        return Some(CommandOutcome::StalePartyMirror);
    };
    let mut local: Vec<_> = roster.into_iter().map(|row| row.character_guid).collect();
    let mut certified = admitted.members.clone();
    local.sort_unstable();
    certified.sort_unstable();
    if member.group_id != admitted.group_id
        || group.leader_guid != admitted.leader_guid
        || admitted.leader_guid != admitted.issuer_guid
        || local != certified
    {
        return Some(CommandOutcome::StalePartyMirror);
    }
    if admitted.command.authority_member_guid != 0
        && !local.contains(&admitted.command.authority_member_guid)
    {
        return Some(CommandOutcome::NotMember);
    }
    if admitted.command.exact_target_guid != 0 {
        if let Err(outcome) = companion_target_facts(
            ctx,
            admitted.command.bot_guid,
            admitted.command.exact_target_guid,
        ) {
            return Some(outcome);
        }
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PartyFactsUnavailable {
    pub group_id: u64,
    pub reason: PartyFactsUnavailableReason,
}

pub(crate) fn has_known_party_member_in_partition(
    ctx: &ReducerContext,
    character_guid: u64,
    partition: (u32, u64),
) -> Result<bool, PartyFactsUnavailableReason> {
    let Some(member) = group_of(ctx, character_guid) else {
        return Ok(false);
    };
    let group = ctx
        .db
        .game_group()
        .group_id()
        .find(member.group_id)
        .ok_or(PartyFactsUnavailableReason::MissingGroup)?;
    let members = bounded_roster(ctx, member.group_id, group_kind_of(&group))
        .ok_or(PartyFactsUnavailableReason::FightLimit)?;
    Ok(members.into_iter().any(|candidate| {
        candidate.character_guid != character_guid
            && current_party_partition(ctx, &candidate)
                .is_some_and(|known| (known.map_id, known.instance_id) == partition)
    }))
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartyFactsUnavailableReason {
    MissingGroup,
    FightLimit,
}

fn current_party_partition(
    ctx: &ReducerContext,
    member: &GroupMember,
) -> Option<PartyPartitionFacts> {
    if let Some(entity) = crate::helpers::acting_entity_by_guid(ctx, member.character_guid) {
        return Some(PartyPartitionFacts {
            map_id: entity.map_id,
            instance_id: entity.instance_id,
            locator_revision: 0,
        });
    }
    ctx.db
        .game_group_member_partition()
        .character_guid()
        .find(member.character_guid)
        .filter(|partition| {
            partition.group_id == member.group_id
                && partition.member_active
                && partition.state == PartyPartitionState::Known
        })
        .map(|partition| PartyPartitionFacts {
            map_id: partition.map_id,
            instance_id: partition.instance_id,
            locator_revision: partition.locator_revision,
        })
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn party_facts(
    ctx: &ReducerContext,
    character_guid: u64,
) -> Result<Option<PartyFacts>, PartyFactsUnavailable> {
    let Some(member) = group_of(ctx, character_guid) else {
        return Ok(None);
    };
    let group =
        ctx.db
            .game_group()
            .group_id()
            .find(member.group_id)
            .ok_or(PartyFactsUnavailable {
                group_id: member.group_id,
                reason: PartyFactsUnavailableReason::MissingGroup,
            })?;
    let unavailable = || PartyFactsUnavailable {
        group_id: member.group_id,
        reason: PartyFactsUnavailableReason::FightLimit,
    };
    let members =
        bounded_roster(ctx, member.group_id, group_kind_of(&group)).ok_or_else(unavailable)?;
    let members: Vec<_> = members
        .into_iter()
        .map(|member| {
            let unit = ctx
                .db
                .game_world_entity()
                .guid()
                .find(member.character_guid)
                .map(|entity| PartyUnitFacts {
                    map_id: entity.map_id,
                    instance_id: entity.instance_id,
                    x: entity.x,
                    y: entity.y,
                    z: entity.z,
                    health: entity.health,
                    max_health: entity.max_health,
                    dead: entity.dead,
                    target_guid: entity.target_guid,
                });
            PartyMemberFacts {
                character_guid: member.character_guid,
                unit,
                partition: current_party_partition(ctx, &member),
            }
        })
        .collect();
    let party_guids: std::collections::BTreeSet<u64> =
        members.iter().map(|member| member.character_guid).collect();
    let mut party_melee_targets = std::collections::BTreeSet::new();
    let anchor = ctx.db.game_world_entity().guid().find(character_guid);
    let melee = ctx.db.game_melee_attack();
    let pending = ctx.db.game_pending_cast();
    let mut enemy_guids = std::collections::BTreeSet::new();
    let mut pending_damage_targets = std::collections::BTreeSet::new();
    let mut pending_control_targets = std::collections::BTreeMap::new();
    const ENEMY_LIMIT: usize = 24;
    const THREAT_SOURCE_LIMIT: usize = RAID_MAX_MEMBERS * 2;
    for guid in &party_guids {
        if let Some(attack) = melee.attacker_guid().find(*guid) {
            enemy_guids.insert(attack.target_guid);
            party_melee_targets.insert(attack.target_guid);
        }
        let attacks: Vec<_> = melee
            .by_target()
            .filter(guid)
            .take(ENEMY_LIMIT + 1)
            .collect();
        if attacks.len() > ENEMY_LIMIT {
            return Err(unavailable());
        }
        for attack in attacks {
            enemy_guids.insert(attack.attacker_guid);
        }
        let casts: Vec<_> = pending.by_caster().filter(guid).take(2).collect();
        if casts.len() > 1 {
            return Err(unavailable());
        }
        for cast in casts {
            enemy_guids.insert(cast.target_guid);
            match crate::spell::spell_control(ctx, cast.spell_id).map_err(|_| unavailable())? {
                Some(control) => {
                    pending_control_targets
                        .entry(cast.target_guid)
                        .and_modify(|current| {
                            if matches!(control, crate::spell::UnitControl::Incapacitated)
                                || matches!(
                                    (control, *current),
                                    (
                                        crate::spell::UnitControl::Feared,
                                        crate::spell::UnitControl::Rooted
                                    )
                                )
                            {
                                *current = control;
                            }
                        })
                        .or_insert(control);
                }
                None => {
                    pending_damage_targets.insert(cast.target_guid);
                }
            }
        }
        let threat_rows: Vec<_> = ctx
            .db
            .game_threat()
            .by_source()
            .filter(guid)
            .take(ENEMY_LIMIT + 1)
            .collect();
        if threat_rows.len() > ENEMY_LIMIT {
            return Err(unavailable());
        }
        for row in threat_rows {
            enemy_guids.insert(row.creature_guid);
        }
        if enemy_guids.len() > ENEMY_LIMIT {
            return Err(unavailable());
        }
    }
    let mut enemies = Vec::new();
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000).max(0) as u64;
    for guid in enemy_guids {
        let Some(entity) = ctx.db.game_world_entity().guid().find(guid) else {
            continue;
        };
        let Some(anchor) = anchor.as_ref() else {
            continue;
        };
        if entity.is_player()
            || entity.dead
            || (entity.map_id, entity.instance_id) != (anchor.map_id, anchor.instance_id)
            || !crate::combat::may_harm(ctx, anchor, &entity)
        {
            continue;
        }
        let current_target_guid = melee.attacker_guid().find(guid).map(|row| row.target_guid);
        let attacking_party =
            current_target_guid.is_some_and(|target| party_guids.contains(&target));
        let party_attacking = party_melee_targets.contains(&guid);
        let party_casting = pending_damage_targets.contains(&guid);
        let threat =
            crate::threat::party_threat_facts(ctx, guid, &party_guids, THREAT_SOURCE_LIMIT)
                .map_err(|_| unavailable())?;
        let active_threat = threat.party_has_threat && entity.combat_until_ms > now_ms;
        let pending_control = pending_control_targets.get(&guid).copied();
        if !(attacking_party
            || party_attacking
            || party_casting
            || active_threat
            || pending_control.is_some())
        {
            continue;
        }
        let control = pending_control
            .or(crate::spell::control_status(ctx, guid, 64).map_err(|_| unavailable())?);
        enemies.push(PartyEnemyFacts {
            guid,
            map_id: entity.map_id,
            instance_id: entity.instance_id,
            x: entity.x,
            y: entity.y,
            z: entity.z,
            health: entity.health,
            max_health: entity.max_health,
            attacking_party,
            party_attacking,
            party_casting,
            party_has_threat: active_threat,
            current_target_guid,
            top_threat_guid: threat.top_target_guid,
            control,
        });
    }
    Ok(Some(PartyFacts {
        group_id: member.group_id,
        leader_guid: group.leader_guid,
        members,
        enemies,
    }))
}

pub(crate) fn checked_group_membership(
    ctx: &ReducerContext,
    character_guid: u64,
) -> Result<Option<(GroupMember, Group)>, GroupOpError> {
    let Some(member) = group_of(ctx, character_guid) else {
        return Ok(None);
    };
    let group = ctx
        .db
        .game_group()
        .group_id()
        .find(member.group_id)
        .ok_or_else(|| {
            GroupOpError::Invariant(format!(
                "member {character_guid} points to missing group {}",
                member.group_id
            ))
        })?;
    Ok(Some((member, group)))
}

pub(crate) fn group_with_members(
    ctx: &ReducerContext,
    character_guid: u64,
) -> Result<Option<(Group, Vec<GroupMember>)>, GroupOpError> {
    Ok(
        checked_group_membership(ctx, character_guid)?.map(|(_, group)| {
            let members = members_of(ctx, group.group_id);
            (group, members)
        }),
    )
}

fn led_group_of(ctx: &ReducerContext, guid: u64) -> Result<(GroupMember, Group), GroupOpError> {
    let (m, group) = checked_group_membership(ctx, guid)?.ok_or(GroupRefusal::NotInGroup)?;
    if group.leader_guid != guid {
        return Err(GroupRefusal::NotLeader.into());
    }
    Ok((m, group))
}

pub(crate) fn members_of(ctx: &ReducerContext, group_id: u64) -> Vec<GroupMember> {
    ctx.db
        .game_group_member()
        .by_group()
        .filter(&group_id)
        .collect()
}

pub(crate) fn bounded_roster(
    ctx: &ReducerContext,
    group_id: u64,
    kind: GroupKind,
) -> Option<Vec<GroupMember>> {
    let cap = kind.member_cap();
    let roster: Vec<_> = ctx
        .db
        .game_group_member()
        .by_group()
        .filter(&group_id)
        .take(cap + 1)
        .collect();
    (roster.len() <= cap).then_some(roster)
}

pub(crate) fn party_chat_audience(
    ctx: &ReducerContext,
    speaker_guid: u64,
) -> Result<crate::realm_chat::ChatAudience, lyracore_shared::chat::ChatRefusal> {
    let (member, group) = checked_group_membership(ctx, speaker_guid)
        .map_err(|error| {
            spacetimedb::log::error!(
                "chat: {speaker_guid} group membership lookup broken: {error:?}"
            );
            lyracore_shared::chat::ChatRefusal::NotInGroup
        })?
        .ok_or(lyracore_shared::chat::ChatRefusal::NotInGroup)?;
    let audience = if group_kind_of(&group) == GroupKind::Raid {
        GroupAudience::Subgroup(raid_slot_of(&member).subgroup())
    } else {
        GroupAudience::Everyone
    };
    Ok(crate::realm_chat::ChatAudience {
        recipients: members_of(ctx, member.group_id)
            .into_iter()
            .filter(|candidate| audience.includes(&group, candidate))
            .map(|candidate| candidate.character_guid)
            .collect(),
        ignorable: false,
        channel_name: String::new(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RaidChatKind {
    Raid,
    RaidLeader,
    RaidWarning,
}

pub(crate) fn raid_chat_audience(
    ctx: &ReducerContext,
    speaker_guid: u64,
    kind: RaidChatKind,
) -> Result<crate::realm_chat::ChatAudience, lyracore_shared::chat::ChatRefusal> {
    let (member, group) = checked_group_membership(ctx, speaker_guid)
        .map_err(|error| {
            spacetimedb::log::error!(
                "chat: {speaker_guid} group membership lookup broken: {error:?}"
            );
            lyracore_shared::chat::ChatRefusal::NotInGroup
        })?
        .ok_or(lyracore_shared::chat::ChatRefusal::NotInGroup)?;
    if let Some(refusal) = raid_chat_refusal(
        kind,
        group_kind_of(&group),
        group.leader_guid == speaker_guid,
        manages_raid(&group, &member),
    ) {
        return Err(refusal);
    }
    Ok(crate::realm_chat::ChatAudience {
        recipients: members_of(ctx, group.group_id)
            .into_iter()
            .map(|candidate| candidate.character_guid)
            .collect(),
        ignorable: false,
        channel_name: String::new(),
    })
}

fn raid_chat_refusal(
    kind: RaidChatKind,
    group_kind: GroupKind,
    is_leader: bool,
    manages: bool,
) -> Option<lyracore_shared::chat::ChatRefusal> {
    use lyracore_shared::chat::ChatRefusal;
    if group_kind != GroupKind::Raid {
        return Some(ChatRefusal::NotRaid);
    }
    match kind {
        RaidChatKind::Raid => None,
        RaidChatKind::RaidLeader => (!is_leader).then_some(ChatRefusal::NotRaidLeader),
        RaidChatKind::RaidWarning => (!manages).then_some(ChatRefusal::NotRaidLeaderOrAssistant),
    }
}

fn push_list_to_all(ctx: &ReducerContext, group_id: u64) {
    let Some(payload) = roster_payload(ctx, group_id) else {
        spacetimedb::log::warn!(
            "push_list_to_all: group {group_id} row missing — skipping roster push (broken invariant, not a fabricated empty roster)"
        );
        return;
    };
    group_broadcast(
        ctx,
        group_id,
        GroupAudience::Everyone,
        group_event_kind::LIST,
        0,
        payload,
    );
}

fn character_online(ctx: &ReducerContext, guid: u64) -> bool {
    ctx.db
        .game_native_player()
        .character_guid()
        .find(guid)
        .is_some()
        || ctx.db.game_world_entity().guid().find(guid).is_some()
}

fn invite_core_on(
    ctx: &ReducerContext,
    plane: Plane,
    inviter_guid: u64,
    target_guid: u64,
) -> Result<(), GroupOpError> {
    if target_guid == inviter_guid {
        return Err(GroupRefusal::InviteSelf.into());
    }

    if plane == Plane::Shard {
        if ctx.db.game_character().guid().find(target_guid).is_none() {
            return Err(GroupRefusal::NoSuchPlayer.into());
        }
        if !character_online(ctx, target_guid) {
            return Err(GroupRefusal::TargetOffline.into());
        }
    }
    if checked_group_membership(ctx, target_guid)?.is_some()
        || has_pending_leader_invite(ctx, target_guid)
    {
        return Err(GroupRefusal::AlreadyInGroup.into());
    }
    let inviter_group_id = match checked_group_membership(ctx, inviter_guid)? {
        Some((m, group)) => {
            if !manages_raid(&group, &m) {
                return Err(GroupRefusal::NotLeader.into());
            }
            if !has_room(group_kind_of(&group), members_of(ctx, m.group_id).len()) {
                return Err(GroupRefusal::GroupFull.into());
            }
            m.group_id
        }
        None => 0,
    };
    let invites = ctx.db.game_group_invite();
    for stale in invites.by_target().filter(&target_guid).collect::<Vec<_>>() {
        invites.id().delete(stale.id);
    }
    invites.insert(GroupInvite {
        id: 0,
        target_guid,
        inviter_guid,
        created_at: ctx.timestamp,
        group_id: inviter_group_id,
    });
    push_event(
        ctx,
        target_guid,
        group_event_kind::INVITE,
        inviter_guid,
        String::new(),
    );
    crate::hooks::fire_on_group_invite(
        ctx,
        &crate::hooks::GroupInvitePayload {
            target_guid,
            inviter_guid,
        },
    );
    Ok(())
}

pub(crate) fn accept_invite_for(ctx: &ReducerContext, acceptor_guid: u64) -> Result<(), String> {
    let acceptor = crate::helpers::character_by_guid(ctx, acceptor_guid);
    if acceptor.as_ref().is_some_and(|character| !character.online) {
        crate::sessionless::group_action_gate(ctx, acceptor_guid)
            .map_err(|refusal| refused(refusal, "session-less group acceptance"))?;
    }
    let class = acceptor.map_or(0, |character| character.class);
    apply_action_for(ctx, acceptor_guid, GroupAction::Accept { class })
}

fn has_pending_leader_invite(ctx: &ReducerContext, character_guid: u64) -> bool {
    ctx.db
        .game_group_invite()
        .iter()
        .any(|invite| invite.inviter_guid == character_guid && invite.group_id == 0)
}

fn accept_invite_on(
    ctx: &ReducerContext,
    plane: Plane,
    acceptor_guid: u64,
    class: u8,
) -> Result<(), GroupOpError> {
    let invites = ctx.db.game_group_invite();
    let invite = invites
        .by_target()
        .filter(&acceptor_guid)
        .next()
        .ok_or(GroupRefusal::NoPendingInvite)?;
    let target = match invite.group_id {
        0 => JoinTarget::NewParty {
            leader_guid: invite.inviter_guid,
        },
        group_id => JoinTarget::Group(group_id),
    };
    let plan = plan_join(ctx, acceptor_guid, target)?;
    if plane == Plane::Shard {
        if let JoinTarget::NewParty { leader_guid } = target {
            crate::helpers::character_by_guid(ctx, leader_guid)
                .ok_or(GroupRefusal::InviterUnavailable)?;
        }
        crate::helpers::character_by_guid(ctx, acceptor_guid).ok_or(GroupRefusal::NoSuchPlayer)?;
    }
    invites.id().delete(invite.id);
    let joined = plan.apply(ctx);
    if joined.formed {
        crate::meeting_stone::party_joined(ctx, joined.group_id, invite.inviter_guid, 0)
            .map_err(GroupOpError::Invariant)?;
    }
    crate::meeting_stone::party_joined(ctx, joined.group_id, acceptor_guid, class)
        .map_err(GroupOpError::Invariant)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JoinTarget {
    Group(u64),
    NewParty { leader_guid: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct JoinedGroup {
    group_id: u64,
    formed: bool,
}

// A checked JoinPlan remains valid only while intervening writes leave membership unchanged.
#[derive(Clone, Copy, Debug)]
pub(crate) struct JoinPlan {
    joiner_guid: u64,
    target: JoinTarget,
    slot: RaidSlot,
}

pub(crate) fn plan_join(
    ctx: &ReducerContext,
    joiner_guid: u64,
    target: JoinTarget,
) -> Result<JoinPlan, GroupOpError> {
    if checked_group_membership(ctx, joiner_guid)?.is_some() {
        return Err(GroupRefusal::AlreadyInGroup.into());
    }
    let slot = match target {
        JoinTarget::Group(group_id) => {
            let current = members_of(ctx, group_id);
            let Some(group) = ctx.db.game_group().group_id().find(group_id) else {
                if current.is_empty() {
                    return Err(GroupRefusal::NoPendingInvite.into());
                }
                return Err(GroupOpError::Invariant(format!(
                    "members point to missing group {group_id}"
                )));
            };
            let kind = group_kind_of(&group);
            if !has_room(kind, current.len()) {
                return Err(GroupRefusal::GroupFull.into());
            }
            match kind {
                GroupKind::Party => RaidSlot::default(),
                GroupKind::Raid => RaidSlot::for_raid_joiner(current.iter().map(raid_slot_of))
                    .ok_or(GroupRefusal::GroupFull)?,
            }
        }
        JoinTarget::NewParty { leader_guid } => {
            if checked_group_membership(ctx, leader_guid)?.is_some() {
                return Err(GroupRefusal::InviterUnavailable.into());
            }
            RaidSlot::default()
        }
    };
    Ok(JoinPlan {
        joiner_guid,
        target,
        slot,
    })
}

impl JoinPlan {
    fn apply(self, ctx: &ReducerContext) -> JoinedGroup {
        let Self {
            joiner_guid,
            target,
            slot,
        } = self;
        let members = ctx.db.game_group_member();
        let joined = match target {
            JoinTarget::Group(group_id) => JoinedGroup {
                group_id,
                formed: false,
            },
            JoinTarget::NewParty { leader_guid } => {
                let group = ctx.db.game_group().insert(Group {
                    group_id: 0,
                    leader_guid,
                    loot_method: loot_method::GROUP,
                    loot_threshold: 2,
                    rr_cursor: 0,
                    master_looter_guid: 0,
                    group_type: GroupKind::Party.wire(),
                });
                members.insert(GroupMember {
                    id: 0,
                    group_id: group.group_id,
                    character_guid: leader_guid,
                    owner_identity: member_identity(ctx, leader_guid),
                    raid_slot: RaidSlot::default().wire(),
                });
                let invites = ctx.db.game_group_invite();
                let pending: Vec<GroupInvite> = invites
                    .iter()
                    .filter(|other| other.inviter_guid == leader_guid && other.group_id == 0)
                    .collect();
                for other in pending {
                    invites.id().update(GroupInvite {
                        group_id: group.group_id,
                        ..other
                    });
                }
                JoinedGroup {
                    group_id: group.group_id,
                    formed: true,
                }
            }
        };
        members.insert(GroupMember {
            id: 0,
            group_id: joined.group_id,
            character_guid: joiner_guid,
            owner_identity: member_identity(ctx, joiner_guid),
            raid_slot: slot.wire(),
        });
        push_list_to_all(ctx, joined.group_id);
        crate::instance::reconcile_instance_removal(ctx, joiner_guid);
        if let JoinTarget::NewParty { leader_guid } = target {
            crate::instance::reconcile_instance_removal(ctx, leader_guid);
        }
        joined
    }
}

fn member_identity(ctx: &ReducerContext, character_guid: u64) -> Identity {
    crate::helpers::event_recipient_identity(
        crate::helpers::character_by_guid(ctx, character_guid)
            .map(|character| character.owner_identity),
    )
}

fn decline_invite_on(ctx: &ReducerContext, decliner_guid: u64) -> Result<(), GroupOpError> {
    let invites = ctx.db.game_group_invite();
    let invite = invites
        .by_target()
        .filter(&decliner_guid)
        .next()
        .ok_or(GroupRefusal::NoPendingInvite)?;
    invites.id().delete(invite.id);
    push_event(
        ctx,
        invite.inviter_guid,
        group_event_kind::DECLINE,
        decliner_guid,
        String::new(),
    );
    Ok(())
}

pub(crate) fn leave_group_for(ctx: &ReducerContext, leaver_guid: u64) -> Result<(), String> {
    apply_action_for(
        ctx,
        leaver_guid,
        GroupAction::Leave {
            cause: leave_cause::LEFT,
        },
    )
}

fn leave_group_on(ctx: &ReducerContext, leaver_guid: u64, cause: u8) -> Result<(), GroupOpError> {
    if checked_group_membership(ctx, leaver_guid)?.is_none() {
        return Err(GroupRefusal::NotInGroup.into());
    }
    if cause == leave_cause::CHARACTER_DELETED {
        delete_target_icons_on(ctx, leaver_guid);
    }
    remove_member(ctx, leaver_guid, Departure::Left);
    Ok(())
}

fn uninvite_on(
    ctx: &ReducerContext,
    actor_guid: u64,
    target_guid: u64,
) -> Result<(), GroupOpError> {
    let (m, group) = checked_group_membership(ctx, actor_guid)?.ok_or(GroupRefusal::NotInGroup)?;
    if !manages_raid(&group, &m) {
        return Err(GroupRefusal::NotLeader.into());
    }
    let (target, _target_group) =
        checked_group_membership(ctx, target_guid)?.ok_or(GroupRefusal::TargetNotInGroup)?;
    if target.group_id != m.group_id {
        return Err(GroupRefusal::TargetNotInGroup.into());
    }
    if target_guid == actor_guid {
        return Err(GroupRefusal::KickSelf.into());
    }
    if target_guid == group.leader_guid {
        return Err(GroupRefusal::NotLeader.into());
    }
    remove_member(ctx, target_guid, Departure::Kicked);
    Ok(())
}

fn set_loot_method_on(
    ctx: &ReducerContext,
    leader_guid: u64,
    loot_setting: u8,
    master_guid: u64,
    loot_threshold: u8,
) -> Result<(), GroupOpError> {
    let (m, mut group) = led_group_of(ctx, leader_guid)?;
    if !valid_loot_method(loot_setting) || !valid_loot_threshold(loot_threshold) {
        return Err(GroupRefusal::InvalidLootRules.into());
    }
    let resolved_master = if loot_setting == loot_method::MASTER {
        let (target, _target_group) =
            checked_group_membership(ctx, master_guid)?.ok_or(GroupRefusal::TargetNotInGroup)?;
        if target.group_id != m.group_id {
            return Err(GroupRefusal::TargetNotInGroup.into());
        }
        master_guid
    } else {
        0
    };
    group.loot_method = loot_setting;
    group.loot_threshold = loot_threshold;
    group.master_looter_guid = resolved_master;
    ctx.db.game_group().group_id().update(group);
    push_list_to_all(ctx, m.group_id);
    Ok(())
}

pub(crate) fn valid_loot_method(method: u8) -> bool {
    matches!(
        method,
        loot_method::FFA
            | loot_method::ROUND_ROBIN
            | loot_method::MASTER
            | loot_method::GROUP
            | loot_method::NEED_BEFORE_GREED
    )
}

pub(crate) fn valid_loot_threshold(threshold: u8) -> bool {
    threshold <= 6
}

fn raid_convert_on(ctx: &ReducerContext, actor_guid: u64) -> Result<RosterChange, GroupOpError> {
    let (member, mut group) = led_group_of(ctx, actor_guid)?;
    if group_kind_of(&group) == GroupKind::Raid {
        return Ok(RosterChange::Unchanged);
    }
    group.group_type = GroupKind::Raid.wire();
    ctx.db.game_group().group_id().update(group);
    crate::meeting_stone::dequeue_party(
        ctx,
        member.group_id,
        lyracore_shared::meeting_stone::queue_status::LEAVE_QUEUE,
    );
    push_list_to_all(ctx, member.group_id);
    Ok(RosterChange::Changed)
}

fn set_leader_on(
    ctx: &ReducerContext,
    actor_guid: u64,
    target_guid: u64,
) -> Result<RosterChange, GroupOpError> {
    let (member, mut group) = led_group_of(ctx, actor_guid)?;
    if target_guid == actor_guid {
        return Err(GroupRefusal::TargetIsSelf.into());
    }
    let (target, _target_group) =
        checked_group_membership(ctx, target_guid)?.ok_or(GroupRefusal::TargetNotInGroup)?;
    if target.group_id != member.group_id {
        return Err(GroupRefusal::TargetNotInGroup.into());
    }
    group.leader_guid = target_guid;
    ctx.db.game_group().group_id().update(group);
    announce_leader(ctx, member.group_id, target_guid);
    push_list_to_all(ctx, member.group_id);
    Ok(RosterChange::Changed)
}

fn set_assistant_on(
    ctx: &ReducerContext,
    actor_guid: u64,
    target_guid: u64,
    promote: bool,
) -> Result<RosterChange, GroupOpError> {
    let (member, group) = led_group_of(ctx, actor_guid)?;
    if group_kind_of(&group) != GroupKind::Raid {
        return Err(GroupRefusal::NotRaid.into());
    }
    if target_guid == actor_guid {
        return Err(GroupRefusal::TargetIsSelf.into());
    }
    let (mut target, _target_group) =
        checked_group_membership(ctx, target_guid)?.ok_or(GroupRefusal::TargetNotInGroup)?;
    if target.group_id != member.group_id {
        return Err(GroupRefusal::TargetNotInGroup.into());
    }
    let slot = raid_slot_of(&target);
    let assigned = slot.with_assistant(promote);
    if assigned == slot {
        return Ok(RosterChange::Unchanged);
    }
    target.raid_slot = assigned.wire();
    ctx.db.game_group_member().id().update(target);
    push_list_to_all(ctx, member.group_id);
    Ok(RosterChange::Changed)
}

fn member_of_group(
    ctx: &ReducerContext,
    target_guid: u64,
    group_id: u64,
) -> Result<GroupMember, GroupOpError> {
    group_of(ctx, target_guid)
        .filter(|member| member.group_id == group_id)
        .ok_or_else(|| GroupOpError::from(GroupRefusal::TargetNotInGroup))
}

fn change_subgroup_on(
    ctx: &ReducerContext,
    actor_guid: u64,
    target_guid: u64,
    subgroup: u8,
) -> Result<RosterChange, GroupOpError> {
    let (actor_member, group) =
        checked_group_membership(ctx, actor_guid)?.ok_or(GroupRefusal::NotInGroup)?;
    if group_kind_of(&group) != GroupKind::Raid {
        return Err(GroupRefusal::NotRaid.into());
    }
    if !manages_raid(&group, &actor_member) {
        return Err(GroupRefusal::NotLeader.into());
    }
    if subgroup >= RAID_SUBGROUPS {
        return Err(GroupRefusal::InvalidSubgroup.into());
    }
    let target = member_of_group(ctx, target_guid, group.group_id)?;
    let destination_size = members_of(ctx, group.group_id)
        .iter()
        .filter(|member| {
            member.character_guid != target_guid && raid_slot_of(member).subgroup() == subgroup
        })
        .count();
    let Some(new_slot) = raid_slot_of(&target).moved_to_subgroup(subgroup, destination_size)?
    else {
        return Ok(RosterChange::Unchanged);
    };
    let mut target = target;
    target.raid_slot = new_slot.wire();
    ctx.db.game_group_member().id().update(target);
    push_list_to_all(ctx, group.group_id);
    Ok(RosterChange::Changed)
}

fn swap_subgroup_on(
    ctx: &ReducerContext,
    actor_guid: u64,
    first_guid: u64,
    second_guid: u64,
) -> Result<RosterChange, GroupOpError> {
    let (actor_member, group) =
        checked_group_membership(ctx, actor_guid)?.ok_or(GroupRefusal::NotInGroup)?;
    if group_kind_of(&group) != GroupKind::Raid {
        return Err(GroupRefusal::NotRaid.into());
    }
    if !manages_raid(&group, &actor_member) {
        return Err(GroupRefusal::NotLeader.into());
    }
    let first = member_of_group(ctx, first_guid, group.group_id)?;
    let second = member_of_group(ctx, second_guid, group.group_id)?;
    let Some((first_slot, second_slot)) = raid_slot_of(&first).swapped_with(raid_slot_of(&second))
    else {
        return Ok(RosterChange::Unchanged);
    };
    let members = ctx.db.game_group_member();
    let mut first = first;
    first.raid_slot = first_slot.wire();
    members.id().update(first);
    let mut second = second;
    second.raid_slot = second_slot.wire();
    members.id().update(second);
    push_list_to_all(ctx, group.group_id);
    Ok(RosterChange::Changed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RosterChange {
    Changed,
    Unchanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Departure {
    Left,
    Kicked,
}

pub(crate) fn remove_member(ctx: &ReducerContext, character_guid: u64, departure: Departure) {
    let Some(m) = group_of(ctx, character_guid) else {
        return;
    };
    let group_id = m.group_id;
    crate::loot::tag::revoke_group_member(ctx, group_id, character_guid);
    ctx.db.game_group_member().id().delete(m.id);
    let remaining = members_of(ctx, group_id);
    if departure == Departure::Kicked && remaining.len() >= 2 {
        push_event(
            ctx,
            character_guid,
            group_event_kind::UNINVITED,
            0,
            String::new(),
        );
    }
    push_event(
        ctx,
        character_guid,
        group_event_kind::DESTROYED,
        0,
        String::new(),
    );

    let group = ctx.db.game_group().group_id().find(group_id);
    let rematch = match leader_after_removal(
        group.as_ref().map_or(GroupKind::Party, group_kind_of),
        &remaining,
        character_guid,
        group.as_ref().map_or(0, |g| g.leader_guid),
    ) {
        None => {
            // Include the leaver in the former roster when resolving rolls after disband.
            let survivors: Vec<u64> = remaining.iter().map(|r| r.character_guid).collect();
            let mut all_guids = survivors.clone();
            all_guids.push(character_guid);
            let survivor = if survivors.len() == 1 {
                Some(survivors[0])
            } else {
                None
            };
            crate::loot::force_resolve_rolls_for_disband(ctx, &all_guids, survivor);
            for r in &remaining {
                push_event(
                    ctx,
                    r.character_guid,
                    group_event_kind::DESTROYED,
                    0,
                    String::new(),
                );
                ctx.db.game_group_member().id().delete(r.id);
            }
            crate::meeting_stone::party_disbanded(ctx, group_id, &all_guids);
            clear_target_icons(ctx, group_id);
            ctx.db.game_group().group_id().delete(group_id);
            for guid in all_guids {
                crate::instance::reconcile_instance_removal(ctx, guid);
            }
            return;
        }
        Some(new_leader) => {
            let mut leader_changed = false;
            if let Some(mut group) = group {
                if group.leader_guid != new_leader {
                    group.leader_guid = new_leader;
                    ctx.db.game_group().group_id().update(group);
                    announce_leader(ctx, group_id, new_leader);
                    leader_changed = true;
                }
            }
            crate::meeting_stone::party_left(
                ctx,
                crate::meeting_stone::PartyDeparture {
                    group_id,
                    character_guid,
                    departure,
                    leader_changed,
                },
            )
        }
    };
    push_list_to_all(ctx, group_id);
    crate::instance::reconcile_instance_removal(ctx, character_guid);
    if let Some(bucket) = rematch {
        if let Err(reason) = crate::meeting_stone::match_bucket(ctx, bucket) {
            spacetimedb::log::error!("meeting stone pass after {character_guid} left: {reason}");
        }
    }
}

fn announce_leader(ctx: &ReducerContext, group_id: u64, leader_guid: u64) {
    group_broadcast(
        ctx,
        group_id,
        GroupAudience::Everyone,
        group_event_kind::SET_LEADER,
        leader_guid,
        String::new(),
    );
}

pub(crate) fn leader_after_removal(
    kind: GroupKind,
    remaining: &[GroupMember],
    leaving_guid: u64,
    leader_guid: u64,
) -> Option<u64> {
    if remaining.len() < 2 {
        return None;
    }
    if leader_guid != leaving_guid {
        return Some(leader_guid);
    }
    let first_assistant = remaining
        .iter()
        .filter(|member| kind == GroupKind::Raid && raid_slot_of(member).is_assistant())
        .min_by_key(|member| member.id);
    let first_member = remaining.iter().min_by_key(|member| member.id);
    first_assistant
        .or(first_member)
        .map(|heir| heir.character_guid)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Plane {
    Shard,
    RealmCore,
}

#[reducer]
pub fn realm_group_op(
    ctx: &ReducerContext,
    op: u8,
    request_actor: crate::account_ownership::SessionActor,
    target_guid: u64,
    arg_a: u8,
    arg_b: u8,
    arg_c: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    use lyracore_shared::group::realm_op as op;
    let action = match op {
        op::INVITE => GroupAction::Invite(target_guid),
        op::ACCEPT => GroupAction::Accept { class: arg_a },
        op::DECLINE => GroupAction::Decline,
        op::LEAVE => GroupAction::Leave { cause: arg_a },
        op::UNINVITE => GroupAction::Uninvite(target_guid),
        op::LOOT_METHOD => GroupAction::LootMethod {
            method: arg_a,
            master: target_guid,
            threshold: arg_b,
        },
        op::RAID_CONVERT => GroupAction::RaidConvert,
        op::SET_LEADER => GroupAction::SetLeader(target_guid),
        op::SET_ASSISTANT => GroupAction::SetAssistant {
            target: target_guid,
            promote: arg_a != 0,
        },
        op::CHANGE_SUBGROUP => GroupAction::ChangeSubgroup {
            target: target_guid,
            subgroup: arg_a,
        },
        op::SWAP_SUBGROUP => GroupAction::SwapSubgroup {
            first: target_guid,
            second: arg_c,
        },
        op::READY_CHECK_START => GroupAction::ReadyCheckStart,
        op::READY_CHECK_ANSWER => GroupAction::ReadyCheckAnswer(arg_a),
        op::TARGET_ICON => GroupAction::TargetIcon {
            icon: arg_a,
            target: target_guid,
        },
        op::MINIMAP_PING => GroupAction::MinimapPing {
            x_bits: target_guid,
            y_bits: arg_c,
        },
        op::RANDOM_ROLL => GroupAction::RandomRoll {
            min: target_guid,
            max: arg_c,
        },
        other => return Err(format!("unknown realm group op {other}")),
    };
    apply_action(ctx, Plane::RealmCore, actor_guid, action).map_err(|error| error.to_string())
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum GroupAction {
    Invite(u64),
    Accept {
        class: u8,
    },
    Decline,
    Leave {
        cause: u8,
    },
    Uninvite(u64),
    LootMethod {
        method: u8,
        master: u64,
        threshold: u8,
    },
    RaidConvert,
    SetLeader(u64),
    SetAssistant {
        target: u64,
        promote: bool,
    },
    ChangeSubgroup {
        target: u64,
        subgroup: u8,
    },
    SwapSubgroup {
        first: u64,
        second: u64,
    },
    ReadyCheckStart,
    ReadyCheckAnswer(u8),
    TargetIcon {
        icon: u8,
        target: u64,
    },
    MinimapPing {
        x_bits: u64,
        y_bits: u64,
    },
    RandomRoll {
        min: u64,
        max: u64,
    },
    StoneAdd(JoinPlan),
}

pub(crate) fn apply_action(
    ctx: &ReducerContext,
    plane: Plane,
    actor_guid: u64,
    action: GroupAction,
) -> Result<(), GroupOpError> {
    use GroupAction::*;
    use RosterChange::{Changed, Unchanged};
    if plane == Plane::Shard {
        if let Invite(target) = action {
            let actor = crate::helpers::character_by_guid(ctx, actor_guid)
                .ok_or_else(|| logged(GroupRefusal::ActorUnavailable.into(), "group invite"))?;
            if let Some(target) = crate::helpers::character_by_guid(ctx, target) {
                if character_online(ctx, target.guid)
                    && !lyracore_shared::faction::same_team(actor.race, target.race)
                {
                    return Err(logged(GroupRefusal::WrongFaction.into(), "group invite"));
                }
            }
        }
        if let SetLeader(target) = action {
            if !character_online(ctx, target) {
                return Err(logged(GroupRefusal::TargetOffline.into(), "group leader"));
            }
        }
        if let Accept { .. } = action {
            if accepts_other_team(ctx, actor_guid) {
                return Err(logged(GroupRefusal::WrongFaction.into(), "group accept"));
            }
        }
    }
    let before_groups = action_groups(ctx, actor_guid, action);
    let ran = match action {
        Invite(target) => invite_core_on(ctx, plane, actor_guid, target).map(|()| Unchanged),
        Accept { class } => accept_invite_on(ctx, plane, actor_guid, class).map(|()| Changed),
        Decline => decline_invite_on(ctx, actor_guid).map(|()| Unchanged),
        Leave { cause } => leave_group_on(ctx, actor_guid, cause).map(|()| Changed),
        Uninvite(target) => uninvite_on(ctx, actor_guid, target).map(|()| Changed),
        LootMethod {
            method,
            master,
            threshold,
        } => set_loot_method_on(ctx, actor_guid, method, master, threshold).map(|()| Changed),
        RaidConvert => raid_convert_on(ctx, actor_guid),
        SetLeader(target) => set_leader_on(ctx, actor_guid, target),
        SetAssistant { target, promote } => set_assistant_on(ctx, actor_guid, target, promote),
        ChangeSubgroup { target, subgroup } => {
            change_subgroup_on(ctx, actor_guid, target, subgroup)
        }
        SwapSubgroup { first, second } => swap_subgroup_on(ctx, actor_guid, first, second),
        ReadyCheckStart => ready_check_start_on(ctx, actor_guid).map(|()| Unchanged),
        ReadyCheckAnswer(state) => {
            ready_check_answer_on(ctx, actor_guid, state).map(|()| Unchanged)
        }
        TargetIcon { icon, target } => {
            target_icon_on(ctx, actor_guid, icon, target).map(|()| Unchanged)
        }
        MinimapPing { x_bits, y_bits } => {
            minimap_ping_on(ctx, actor_guid, x_bits, y_bits).map(|()| Unchanged)
        }
        RandomRoll { min, max } => random_roll_on(ctx, actor_guid, min, max).map(|()| Unchanged),
        StoneAdd(plan) => {
            plan.apply(ctx);
            Ok(Changed)
        }
    };
    let change =
        ran.map_err(|error| logged(error, &format!("group action {action:?} for {actor_guid}")))?;
    if change == Changed {
        let after_groups = action_groups(ctx, actor_guid, action);
        for group_id in before_groups.union(&after_groups) {
            advance_group_revision(
                ctx,
                *group_id,
                before_groups.contains(group_id),
                ctx.db.game_group().group_id().find(*group_id).is_some(),
            );
        }
    }
    Ok(())
}

fn accepts_other_team(ctx: &ReducerContext, acceptor_guid: u64) -> bool {
    let Some(invite) = ctx
        .db
        .game_group_invite()
        .by_target()
        .filter(&acceptor_guid)
        .next()
    else {
        return false;
    };
    let race = |guid| crate::helpers::character_by_guid(ctx, guid).map(|c| c.race);
    race(acceptor_guid)
        .zip(race(invite.inviter_guid))
        .is_some_and(|(acceptor, inviter)| !lyracore_shared::faction::same_team(acceptor, inviter))
}

fn action_groups(
    ctx: &ReducerContext,
    actor_guid: u64,
    action: GroupAction,
) -> std::collections::BTreeSet<u64> {
    let mut guids = vec![actor_guid];
    let mut joined = None;
    match action {
        GroupAction::Uninvite(target) => guids.push(target),
        GroupAction::Accept { .. } => {
            if let Some(invite) = ctx
                .db
                .game_group_invite()
                .by_target()
                .filter(&actor_guid)
                .next()
            {
                match invite.group_id {
                    0 => guids.push(invite.inviter_guid),
                    group_id => joined = Some(group_id),
                }
            }
        }
        GroupAction::StoneAdd(plan) => match plan.target {
            JoinTarget::Group(group_id) => joined = Some(group_id),
            JoinTarget::NewParty { leader_guid } => guids.push(leader_guid),
        },
        _ => {}
    }
    guids
        .into_iter()
        .filter_map(|guid| {
            ctx.db
                .game_group_member()
                .by_character()
                .filter(&guid)
                .next()
                .map(|member| member.group_id)
        })
        .chain(joined)
        .filter(|group_id| ctx.db.game_group().group_id().find(*group_id).is_some())
        .collect()
}

fn advance_group_revision(ctx: &ReducerContext, group_id: u64, was_active: bool, active: bool) {
    let table = ctx.db.game_group_roster_revision();
    let current = table.group_id().find(group_id);
    let revision = next_group_revision(current.as_ref().map(|row| row.revision), was_active);
    let row = GroupRosterRevision {
        group_id,
        revision,
        active,
    };
    if current.is_some() {
        table.group_id().update(row);
    } else {
        table.insert(row);
    }
}

fn next_group_revision(current: Option<u64>, was_active: bool) -> u64 {
    current
        .unwrap_or(u64::from(was_active))
        .checked_add(1)
        .expect("party roster revision exhausted")
}

#[reducer]
pub fn admit_party_command_authority(
    ctx: &ReducerContext,
    group_id: u64,
    leader_guid: u64,
    bot_guid: u64,
    authority_member_guid: u64,
    mut expected_members: Vec<u64>,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let stale = || {
        crate::bridge::CommandOutcome::StalePartyMirror
            .tag()
            .to_string()
    };
    let group = ctx
        .db
        .game_group()
        .group_id()
        .find(group_id)
        .ok_or_else(stale)?;
    if group.leader_guid != leader_guid {
        return Err(crate::bridge::CommandOutcome::NotLeader.tag().to_string());
    }
    let kind = group_kind_of(&group);
    if expected_members.len() > kind.member_cap() {
        return Err(stale());
    }
    let mut current_members: Vec<_> = bounded_roster(ctx, group_id, kind)
        .ok_or_else(stale)?
        .into_iter()
        .map(|member| member.character_guid)
        .collect();
    current_members.sort_unstable();
    expected_members.sort_unstable();
    if current_members != expected_members {
        return Err(stale());
    }
    let members = ctx.db.game_group_member();
    for guid in [leader_guid, bot_guid, authority_member_guid]
        .into_iter()
        .filter(|guid| *guid != 0)
    {
        if members
            .by_character()
            .filter(&guid)
            .next()
            .is_none_or(|member| member.group_id != group_id)
        {
            return Err(crate::bridge::CommandOutcome::NotMember.tag().to_string());
        }
    }
    Ok(())
}

pub(crate) fn kill_reward_recipients(
    ctx: &ReducerContext,
    killer_guid: u64,
    x: f32,
    y: f32,
    map_id: u32,
    instance_id: u64,
) -> Vec<u64> {
    let Some(m) = group_of(ctx, killer_guid) else {
        return vec![killer_guid];
    };
    let entities = ctx.db.game_world_entity();
    members_of(ctx, m.group_id)
        .into_iter()
        .filter_map(|member| {
            let e = entities.guid().find(member.character_guid)?;
            let (dx, dy) = (e.x - x, e.y - y);
            eligible_for_kill_reward(
                e.dead,
                e.map_id == map_id,
                e.instance_id == instance_id,
                dx * dx + dy * dy,
            )
            .then_some(member.character_guid)
        })
        .collect()
}

pub(crate) fn eligible_for_kill_reward(
    dead: bool,
    same_map: bool,
    same_instance: bool,
    dist_sq: f32,
) -> bool {
    !dead && same_map && same_instance && dist_sq <= GROUP_XP_RANGE_SQ
}

#[table(
    accessor = game_group_target_icon,
    index(accessor = by_group, btree(columns = [group_id])),
    index(accessor = by_target, btree(columns = [target_guid]))
)]
pub struct GroupTargetIcon {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub group_id: u64,
    pub icon: u8,
    pub target_guid: u64,
}

crate::character_owned!(delete, fn sweep_delete_game_group_target_icon(ctx, character_guid) {
    delete_target_icons_on(ctx, character_guid);
});

fn delete_target_icons_on(ctx: &ReducerContext, target_guid: u64) {
    let icons = ctx.db.game_group_target_icon();
    for row in icons.by_target().filter(&target_guid).collect::<Vec<_>>() {
        icons.id().delete(row.id);
    }
}
crate::character_owned!(not_transported, fn sweep_transfer_game_group_target_icon());

pub(crate) fn target_icons_of(ctx: &ReducerContext, group_id: u64) -> Vec<TargetIcon> {
    let mut icons: Vec<TargetIcon> = ctx
        .db
        .game_group_target_icon()
        .by_group()
        .filter(&group_id)
        .map(|row| TargetIcon {
            icon: row.icon,
            target_guid: row.target_guid,
        })
        .collect();
    icons.sort_unstable_by_key(|icon| icon.icon);
    icons
}

fn target_icon_updates(
    held: &[TargetIcon],
    icon: u8,
    target_guid: u64,
) -> Result<Vec<TargetIcon>, GroupRefusal> {
    if icon >= TARGET_ICON_COUNT {
        return Err(GroupRefusal::InvalidTargetIcon);
    }
    let mut updates: Vec<TargetIcon> = held
        .iter()
        .filter(|held| target_guid != 0 && held.target_guid == target_guid)
        .map(|held| TargetIcon {
            icon: held.icon,
            target_guid: 0,
        })
        .collect();
    updates.push(TargetIcon { icon, target_guid });
    Ok(updates)
}

fn write_target_icon(ctx: &ReducerContext, group_id: u64, update: TargetIcon) {
    let icons = ctx.db.game_group_target_icon();
    let row = icons
        .by_group()
        .filter(&group_id)
        .find(|row| row.icon == update.icon);
    match (row, update.target_guid) {
        (Some(row), 0) => {
            icons.id().delete(row.id);
        }
        (Some(mut row), target_guid) => {
            row.target_guid = target_guid;
            icons.id().update(row);
        }
        (None, 0) => {}
        (None, target_guid) => {
            icons.insert(GroupTargetIcon {
                id: 0,
                group_id,
                icon: update.icon,
                target_guid,
            });
        }
    }
}

fn clear_target_icons(ctx: &ReducerContext, group_id: u64) {
    let icons = ctx.db.game_group_target_icon();
    for row in icons.by_group().filter(&group_id).collect::<Vec<_>>() {
        icons.id().delete(row.id);
    }
}

fn managed_group_of(
    ctx: &ReducerContext,
    actor_guid: u64,
) -> Result<(GroupMember, Group), GroupOpError> {
    let (member, group) =
        checked_group_membership(ctx, actor_guid)?.ok_or(GroupRefusal::NotInGroup)?;
    if !manages_raid(&group, &member) {
        return Err(GroupRefusal::NotLeader.into());
    }
    Ok((member, group))
}

fn wire_u32(value: u64, slot: &str) -> Result<u32, GroupOpError> {
    u32::try_from(value)
        .map_err(|_| GroupOpError::Invariant(format!("{slot} {value} does not fit 32 bits")))
}

fn ready_check_start_on(ctx: &ReducerContext, actor_guid: u64) -> Result<(), GroupOpError> {
    let (member, _group) = managed_group_of(ctx, actor_guid)?;
    group_broadcast(
        ctx,
        member.group_id,
        GroupAudience::Everyone,
        group_event_kind::READY_CHECK,
        actor_guid,
        String::new(),
    );
    Ok(())
}

fn ready_check_answer_on(
    ctx: &ReducerContext,
    actor_guid: u64,
    state: u8,
) -> Result<(), GroupOpError> {
    let (member, _group) =
        checked_group_membership(ctx, actor_guid)?.ok_or(GroupRefusal::NotInGroup)?;
    group_broadcast(
        ctx,
        member.group_id,
        GroupAudience::Leader,
        group_event_kind::READY_CHECK_ANSWER,
        actor_guid,
        lyracore_shared::group::encode_ready_check_answer(state),
    );
    Ok(())
}

fn target_icon_on(
    ctx: &ReducerContext,
    actor_guid: u64,
    icon: u8,
    target_guid: u64,
) -> Result<(), GroupOpError> {
    if icon == TARGET_ICON_LIST_REQUEST {
        let (member, _group) =
            checked_group_membership(ctx, actor_guid)?.ok_or(GroupRefusal::NotInGroup)?;
        push_event(
            ctx,
            actor_guid,
            group_event_kind::TARGET_ICON_LIST,
            0,
            lyracore_shared::group::encode_target_icons(&target_icons_of(ctx, member.group_id)),
        );
        return Ok(());
    }
    let (member, _group) = managed_group_of(ctx, actor_guid)?;
    let held = target_icons_of(ctx, member.group_id);
    for update in target_icon_updates(&held, icon, target_guid)? {
        write_target_icon(ctx, member.group_id, update);
        group_broadcast(
            ctx,
            member.group_id,
            GroupAudience::Everyone,
            group_event_kind::TARGET_ICON_UPDATE,
            actor_guid,
            update.encode(),
        );
    }
    Ok(())
}

fn minimap_ping_on(
    ctx: &ReducerContext,
    actor_guid: u64,
    x_bits: u64,
    y_bits: u64,
) -> Result<(), GroupOpError> {
    let (member, _group) =
        checked_group_membership(ctx, actor_guid)?.ok_or(GroupRefusal::NotInGroup)?;
    let x = f32::from_bits(wire_u32(x_bits, "minimap ping x")?);
    let y = f32::from_bits(wire_u32(y_bits, "minimap ping y")?);
    group_broadcast(
        ctx,
        member.group_id,
        GroupAudience::AllExcept(actor_guid),
        group_event_kind::MINIMAP_PING,
        actor_guid,
        lyracore_shared::group::encode_minimap_ping(x, y),
    );
    Ok(())
}

fn random_roll_on(
    ctx: &ReducerContext,
    actor_guid: u64,
    min_roll: u64,
    max_roll: u64,
) -> Result<(), GroupOpError> {
    let (lo, hi) = crate::chat::normalized_roll_range(
        wire_u32(min_roll, "roll minimum")?,
        wire_u32(max_roll, "roll maximum")?,
    );
    let result = lo + ctx.random::<u32>() % (hi - lo + 1);
    let payload = lyracore_shared::group::encode_random_roll(lo, hi, result);
    match checked_group_membership(ctx, actor_guid)? {
        Some((member, _group)) => group_broadcast(
            ctx,
            member.group_id,
            GroupAudience::Everyone,
            group_event_kind::RANDOM_ROLL,
            actor_guid,
            payload,
        ),
        None => push_event(
            ctx,
            actor_guid,
            group_event_kind::RANDOM_ROLL,
            actor_guid,
            payload,
        ),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn realm_roster_revision_advances_from_new_existing_and_migrated_groups() {
        assert_eq!(next_group_revision(None, false), 1, "new party");
        assert_eq!(next_group_revision(None, true), 2, "migrated live party");
        assert_eq!(
            next_group_revision(Some(7), true),
            8,
            "existing party change"
        );
    }

    use super::*;

    #[test]
    fn broken_group_relationship_is_not_a_gameplay_refusal() {
        let failure =
            GroupOpError::Invariant("member 7 points to missing group 9".to_string()).to_string();

        assert!(failure.contains("group invariant failed"));
        assert_eq!(
            GroupOpError::Refused(GroupRefusal::NotLeader).to_string(),
            GroupRefusal::NotLeader.as_tag()
        );
    }

    #[test]
    fn kill_reward_needs_an_alive_member_on_the_same_map_within_range() {
        assert!(eligible_for_kill_reward(false, true, true, 0.0));
        assert!(eligible_for_kill_reward(
            false,
            true,
            true,
            GROUP_XP_RANGE_SQ
        ));
        assert!(!eligible_for_kill_reward(true, true, true, 0.0));
        assert!(!eligible_for_kill_reward(false, false, true, 0.0));
        assert!(!eligible_for_kill_reward(false, true, false, 0.0));
        assert!(!eligible_for_kill_reward(
            false,
            true,
            true,
            GROUP_XP_RANGE_SQ + 1.0
        ));
    }

    fn member_row(id: u64, character_guid: u64, assistant: bool) -> GroupMember {
        GroupMember {
            id,
            group_id: 7,
            character_guid,
            owner_identity: Identity::ZERO,
            raid_slot: RaidSlot::new(0, assistant).unwrap().wire(),
        }
    }

    #[test]
    fn leadership_passes_to_the_longest_standing_member_or_the_group_disbands() {
        let party = GroupKind::Party;
        let row = |id, guid| member_row(id, guid, false);
        assert_eq!(leader_after_removal(party, &[], 7, 7), None);
        assert_eq!(leader_after_removal(party, &[row(3, 30)], 7, 7), None);
        assert_eq!(
            leader_after_removal(party, &[row(3, 30)], 7, 30),
            None,
            "even a surviving leader disbands alone"
        );
        assert_eq!(
            leader_after_removal(party, &[row(1, 10), row(2, 20)], 99, 10),
            Some(10),
            "a member who does not lead leaves the incumbent in place"
        );
        assert_eq!(
            leader_after_removal(party, &[row(5, 90), row(9, 20)], 10, 10),
            Some(90)
        );
        assert_eq!(
            leader_after_removal(party, &[row(9, 20), row(5, 90)], 10, 10),
            Some(90)
        );
    }

    #[test]
    fn a_raid_leader_leaving_passes_the_lead_to_the_first_assistant_in_join_order() {
        let raid = GroupKind::Raid;
        let with_assistants = [
            member_row(2, 20, false),
            member_row(9, 90, true),
            member_row(5, 50, true),
        ];
        assert_eq!(
            leader_after_removal(raid, &with_assistants, 10, 10),
            Some(50)
        );
        assert_eq!(
            leader_after_removal(
                raid,
                &[member_row(3, 30, false), member_row(2, 20, false)],
                10,
                10
            ),
            Some(20),
            "without an Assistant the first member leads"
        );
        assert_eq!(
            leader_after_removal(raid, &with_assistants, 30, 10),
            Some(10),
            "a member who does not lead leaves the incumbent in place"
        );
    }

    #[test]
    fn a_party_passes_the_lead_in_join_order_whatever_a_slot_says() {
        let remaining = [member_row(2, 20, false), member_row(5, 50, true)];
        assert_eq!(
            leader_after_removal(GroupKind::Party, &remaining, 10, 10),
            Some(20)
        );
    }

    fn raid_member(character_guid: u64, subgroup: u8, assistant: bool) -> GroupMember {
        GroupMember {
            id: character_guid,
            group_id: 7,
            character_guid,
            owner_identity: Identity::ZERO,
            raid_slot: RaidSlot::new(subgroup, assistant).unwrap().wire(),
        }
    }

    #[test]
    fn the_member_cap_follows_the_group_kind() {
        assert!(has_room(GroupKind::Party, 4));
        assert!(!has_room(GroupKind::Party, 5));
        assert!(has_room(GroupKind::Raid, 5));
        assert!(has_room(GroupKind::Raid, 39));
        assert!(!has_room(GroupKind::Raid, 40));
    }

    fn raid_led_by(leader_guid: u64) -> Group {
        Group {
            group_id: 7,
            leader_guid,
            loot_method: loot_method::GROUP,
            loot_threshold: 2,
            rr_cursor: 0,
            master_looter_guid: 0,
            group_type: GroupKind::Raid.wire(),
        }
    }

    #[test]
    fn each_group_audience_selects_its_members() {
        let group = raid_led_by(1);
        let members = [
            raid_member(1, 0, false),
            raid_member(2, 0, true),
            raid_member(3, 1, false),
        ];
        let selected = |audience: GroupAudience| -> Vec<u64> {
            members
                .iter()
                .filter(|member| audience.includes(&group, member))
                .map(|member| member.character_guid)
                .collect()
        };
        assert_eq!(selected(GroupAudience::Everyone), [1, 2, 3]);
        assert_eq!(selected(GroupAudience::AllExcept(2)), [1, 3]);
        assert_eq!(selected(GroupAudience::Leader), [1]);
        assert_eq!(selected(GroupAudience::Subgroup(0)), [1, 2]);
        assert_eq!(selected(GroupAudience::Subgroup(1)), [3]);
    }

    #[test]
    fn the_leader_and_assistants_manage_a_raid() {
        let group = raid_led_by(1);
        assert!(manages_raid(&group, &raid_member(1, 0, false)));
        assert!(manages_raid(&group, &raid_member(2, 3, true)));
        assert!(!manages_raid(&group, &raid_member(3, 3, false)));
        let demoted = GroupMember {
            raid_slot: raid_slot_of(&raid_member(2, 3, true))
                .with_assistant(false)
                .wire(),
            ..raid_member(2, 3, true)
        };
        assert!(
            !manages_raid(&group, &demoted),
            "a demoted Assistant no longer invites or kicks"
        );
    }

    #[test]
    fn the_raid_chat_gate_follows_the_audience_table() {
        use lyracore_shared::chat::ChatRefusal;

        for kind in [
            RaidChatKind::Raid,
            RaidChatKind::RaidLeader,
            RaidChatKind::RaidWarning,
        ] {
            for (is_leader, manages) in [(false, false), (false, true), (true, true)] {
                assert_eq!(
                    raid_chat_refusal(kind, GroupKind::Party, is_leader, manages),
                    Some(ChatRefusal::NotRaid),
                    "kind {kind:?}, leader {is_leader}, manages {manages}: a Party has no Raid audience"
                );
            }
        }

        assert_eq!(
            raid_chat_refusal(RaidChatKind::Raid, GroupKind::Raid, false, false),
            None
        );
        assert_eq!(
            raid_chat_refusal(RaidChatKind::Raid, GroupKind::Raid, false, true),
            None
        );
        assert_eq!(
            raid_chat_refusal(RaidChatKind::Raid, GroupKind::Raid, true, true),
            None
        );

        assert_eq!(
            raid_chat_refusal(RaidChatKind::RaidLeader, GroupKind::Raid, true, true),
            None
        );
        assert_eq!(
            raid_chat_refusal(RaidChatKind::RaidLeader, GroupKind::Raid, false, true),
            Some(ChatRefusal::NotRaidLeader),
            "an Assistant is not the leader"
        );
        assert_eq!(
            raid_chat_refusal(RaidChatKind::RaidLeader, GroupKind::Raid, false, false),
            Some(ChatRefusal::NotRaidLeader)
        );

        assert_eq!(
            raid_chat_refusal(RaidChatKind::RaidWarning, GroupKind::Raid, true, true),
            None
        );
        assert_eq!(
            raid_chat_refusal(RaidChatKind::RaidWarning, GroupKind::Raid, false, true),
            None,
            "an Assistant may send a Raid Warning"
        );
        assert_eq!(
            raid_chat_refusal(RaidChatKind::RaidWarning, GroupKind::Raid, false, false),
            Some(ChatRefusal::NotRaidLeaderOrAssistant)
        );
    }

    fn held(icon: u8, target_guid: u64) -> TargetIcon {
        TargetIcon { icon, target_guid }
    }

    #[test]
    fn marking_a_new_unit_sets_one_icon() {
        assert_eq!(target_icon_updates(&[], 7, 100), Ok(vec![held(7, 100)]));
    }

    #[test]
    fn a_marked_unit_loses_its_old_icon_before_it_takes_the_new_one() {
        assert_eq!(
            target_icon_updates(&[held(7, 100)], 6, 100),
            Ok(vec![held(7, 0), held(6, 100)])
        );
    }

    #[test]
    fn an_icon_given_to_another_unit_replaces_the_old_holder_in_one_update() {
        assert_eq!(
            target_icon_updates(&[held(7, 100), held(6, 200)], 7, 300),
            Ok(vec![held(7, 300)])
        );
    }

    #[test]
    fn marking_a_unit_with_its_own_icon_clears_it_and_sets_it_again() {
        assert_eq!(
            target_icon_updates(&[held(7, 100)], 7, 100),
            Ok(vec![held(7, 0), held(7, 100)])
        );
    }

    #[test]
    fn target_zero_clears_the_icon_and_touches_no_other() {
        assert_eq!(
            target_icon_updates(&[held(7, 100), held(1, 200)], 7, 0),
            Ok(vec![held(7, 0)])
        );
    }

    #[test]
    fn an_icon_index_of_eight_or_more_is_refused() {
        for icon in [8, 9, 0xFE] {
            assert_eq!(
                target_icon_updates(&[], icon, 100),
                Err(GroupRefusal::InvalidTargetIcon)
            );
        }
    }

    #[test]
    fn loot_method_encoding_matches_the_real_wire_grouplootsetting_order() {
        assert_eq!(loot_method::FFA, 0);
        assert_eq!(loot_method::ROUND_ROBIN, 1);
        assert_eq!(loot_method::MASTER, 2);
        assert_eq!(loot_method::GROUP, 3);
        assert_eq!(loot_method::NEED_BEFORE_GREED, 4);
    }

    #[test]
    fn valid_loot_method_admits_only_the_five_known_values() {
        for m in [
            loot_method::FFA,
            loot_method::ROUND_ROBIN,
            loot_method::MASTER,
            loot_method::GROUP,
            loot_method::NEED_BEFORE_GREED,
        ] {
            assert!(valid_loot_method(m), "method {m} should be valid");
        }
        assert!(!valid_loot_method(5));
        assert!(!valid_loot_method(255));
    }

    #[test]
    fn valid_loot_threshold_admits_the_seven_itemquality_values_only() {
        for t in 0u8..=6 {
            assert!(
                valid_loot_threshold(t),
                "quality {t} should be a valid threshold"
            );
        }
        assert!(!valid_loot_threshold(7));
        assert!(!valid_loot_threshold(255));
    }
}
