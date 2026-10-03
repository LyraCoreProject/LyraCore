//! The Meeting Stone Queue. [`gw_admit_meeting_stone`] admits a Character at the stone on its Home
//! Shard and writes nothing. [`realm_meeting_stone_op`] queues it on Realm-core, where party
//! membership is authoritative, so Seekers on every World Shard share one queue. Realm-core holds
//! no Character rows, so the Gateway conveys race and class as [`SeekerFacts`].
//!
//! Matching runs in the transaction that changes a bucket ([`match_bucket`]): the cores' queue
//! thread reduces to longest wait per Open Role, so a tick would only add latency. A solo Seeker
//! lasts as long as its Account Claim ([`claim_ended`]); a party Seeker as long as its membership.

use std::collections::BTreeSet;

use spacetimedb::{
    reducer, table, ReducerContext, ScheduleAt, SpacetimeType, Table, TimeDuration, Timestamp,
};

use lyracore_shared::constants::go_type;
use lyracore_shared::faction::team_for_race;
use lyracore_shared::group::{GroupKind, GROUP_MAX_MEMBERS};
use lyracore_shared::meeting_stone::{
    encode_queue, event_kind, queue_status, realm_op, MeetingStoneRefusal,
};

mod matching;

use matching::{formation, next_stone_add, open_roles};

use crate::group::{Departure, JoinPlan, JoinTarget};
use crate::{game_account_claim, game_group};

/// A queued Party hears IN_PROGRESS this often (cm:LFG/LFGMgr.cpp:55).
const REMINDER_INTERVAL_MICROS: i64 = 5 * 60 * 1_000_000;

const REMINDER_TICK_MICROS: i64 = 5_000_000;

/// One Meeting Stone template's level range and dungeon area, imported from `data0` to `data2`.
/// Packages do not author it. [static]
#[table(accessor = game_meeting_stone)]
pub struct MeetingStone {
    #[primary_key]
    pub entry: u32,
    pub min_level: u32,
    /// 0 means no upper bound.
    pub max_level: u32,
    pub area_id: u32,
}

/// One Seeker. The Gateway reads it to answer `CMSG_MEETINGSTONE_INFO`. [entity]
#[table(
    accessor = game_meeting_stone_seeker,
    index(accessor = by_bucket, btree(columns = [area_id, team])),
    index(accessor = by_group, btree(columns = [group_id]))
)]
pub struct MeetingStoneSeeker {
    #[primary_key]
    pub character_guid: u64,
    pub area_id: u32,
    pub team: u32,
    /// 0 when the Gateway could not read it.
    pub class: u8,
    /// 0 for a solo Seeker.
    pub group_id: u64,
    pub queued_at: Timestamp,
}

/// One queued Party. It exists exactly while the Party is queued, and its Seeker rows are its
/// current members. [entity]
#[table(accessor = game_meeting_stone_party)]
pub struct MeetingStoneParty {
    #[primary_key]
    pub group_id: u64,
    pub area_id: u32,
    pub team: u32,
    pub queued_at: Timestamp,
    pub next_reminder_at: Timestamp,
}

#[table(accessor = game_meeting_stone_reminder_schedule, scheduled(remind_queued_parties))]
pub struct MeetingStoneReminderSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

/// The Seekers that can meet: one dungeon area and one team.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Bucket {
    pub(crate) area_id: u32,
    pub(crate) team: u32,
}

#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeekerFacts {
    pub character_guid: u64,
    pub race: u8,
    pub class: u8,
}

fn refused(refusal: MeetingStoneRefusal, detail: &str) -> String {
    let tag = refusal.as_tag();
    spacetimedb::log::info!("meeting stone refused {tag}: {detail}");
    tag.to_string()
}

/// May `actor_guid` use the Meeting Stone `go_guid`? Writes nothing. The level check has no 1.12
/// failure code, so the Gateway drops that Refusal silently, as the client checks it first.
#[reducer]
pub fn gw_admit_meeting_stone(
    ctx: &ReducerContext,
    request_actor: crate::SessionActor,
    go_guid: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    admit(ctx, request_actor, actor_guid, go_guid).map_err(|refusal| {
        refused(
            refusal,
            &format!("admission of {actor_guid} at gameobject {go_guid}"),
        )
    })
}

fn admit(
    ctx: &ReducerContext,
    request_actor: crate::SessionActor,
    actor_guid: u64,
    go_guid: u64,
) -> Result<(), MeetingStoneRefusal> {
    // Humans only: no World Session, no queue.
    if request_actor.ownership.is_none() || crate::taxi::is_in_flight(ctx, actor_guid) {
        return Err(MeetingStoneRefusal::ActorUnavailable);
    }
    let (_, template) = crate::gameobject::usable_go(ctx, actor_guid, go_guid)
        .map_err(|refusal| gate_refusal(refusal.kind))?;
    if template.type_id != go_type::MEETINGSTONE {
        return Err(MeetingStoneRefusal::NotAMeetingStone);
    }
    let stone = ctx
        .db
        .game_meeting_stone()
        .entry()
        .find(template.entry)
        .ok_or(MeetingStoneRefusal::NotAMeetingStone)?;
    let level = crate::helpers::live_entity(ctx, actor_guid)
        .map_err(|_| MeetingStoneRefusal::ActorUnavailable)?
        .level;
    if !level_in_range(level, stone.min_level, stone.max_level) {
        return Err(MeetingStoneRefusal::LevelOutOfRange);
    }
    Ok(())
}

fn gate_refusal(kind: crate::actor::ActionRefusalKind) -> MeetingStoneRefusal {
    use crate::actor::ActionRefusalKind;
    match kind {
        ActionRefusalKind::MissingTarget => MeetingStoneRefusal::NotAMeetingStone,
        ActionRefusalKind::OtherPartition => MeetingStoneRefusal::OtherPartition,
        ActionRefusalKind::OutOfRange => MeetingStoneRefusal::OutOfRange,
        _ => MeetingStoneRefusal::ActorUnavailable,
    }
}

/// A `max_level` of 0 has no upper bound.
fn level_in_range(level: u32, min_level: u32, max_level: u32) -> bool {
    level >= min_level && (max_level == 0 || level <= max_level)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Membership {
    leads: bool,
    kind: GroupKind,
    members: usize,
}

/// A grouped actor queues only as the leader of a Party with room. The order of the checks picks
/// the failure code (cm:LFG/LFGHandler.cpp:58-77).
fn party_join_gate(membership: Membership) -> Result<(), MeetingStoneRefusal> {
    if !membership.leads {
        return Err(MeetingStoneRefusal::NotLeader);
    }
    if membership.kind == GroupKind::Raid {
        return Err(MeetingStoneRefusal::RaidGroup);
    }
    if membership.members >= GROUP_MAX_MEMBERS {
        return Err(MeetingStoneRefusal::PartyFull);
    }
    Ok(())
}

enum StoneOpError {
    Refused(MeetingStoneRefusal),
    /// Untagged, so the Gateway treats the outcome as unknown.
    Invalid(String),
}

impl From<MeetingStoneRefusal> for StoneOpError {
    fn from(refusal: MeetingStoneRefusal) -> Self {
        Self::Refused(refusal)
    }
}

impl From<crate::group::GroupOpError> for StoneOpError {
    fn from(error: crate::group::GroupOpError) -> Self {
        Self::Invalid(format!("{error:?}"))
    }
}

/// One Meeting Stone Queue op on the party authority. Operator-gated because the actor is an
/// argument. JOIN reads `area_id` and the `seekers` of the actor or its whole Party; LEAVE reads
/// neither. Too many facts, facts without the actor, or an unknown op is an untagged error.
#[reducer]
pub fn realm_meeting_stone_op(
    ctx: &ReducerContext,
    op: u8,
    request_actor: crate::SessionActor,
    area_id: u32,
    seekers: Vec<SeekerFacts>,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    if seekers.len() > GROUP_MAX_MEMBERS {
        return Err(format!(
            "meeting stone op carries {} seeker facts, more than a party holds",
            seekers.len()
        ));
    }
    let outcome = if request_actor.ownership.is_none() {
        Err(MeetingStoneRefusal::ActorUnavailable.into())
    } else {
        match op {
            realm_op::JOIN => join(ctx, actor_guid, area_id, &seekers),
            realm_op::LEAVE => leave(ctx, actor_guid),
            other => return Err(format!("unknown meeting stone op {other}")),
        }
    };
    outcome.map_err(|error| match error {
        StoneOpError::Refused(refusal) => refused(
            refusal,
            &format!("op {op} for {actor_guid}, area {area_id}"),
        ),
        StoneOpError::Invalid(reason) => {
            spacetimedb::log::error!("meeting stone op {op} for {actor_guid} failed: {reason}");
            reason
        }
    })
}

fn join(
    ctx: &ReducerContext,
    actor_guid: u64,
    area_id: u32,
    seekers: &[SeekerFacts],
) -> Result<(), StoneOpError> {
    if area_id == 0 {
        return Err(MeetingStoneRefusal::UnknownArea.into());
    }
    let actor = seekers
        .iter()
        .find(|facts| facts.character_guid == actor_guid)
        .ok_or_else(|| {
            StoneOpError::Invalid(format!("seeker facts omit the actor {actor_guid}"))
        })?;
    let bucket = Bucket {
        area_id,
        team: team_for_race(actor.race),
    };
    if let Some((group, members)) = crate::group::group_with_members(ctx, actor_guid)? {
        party_join_gate(Membership {
            leads: group.leader_guid == actor_guid,
            kind: crate::group::group_kind_of(&group),
            members: members.len(),
        })?;
        let class_of = |guid: u64| {
            seekers
                .iter()
                .find(|facts| facts.character_guid == guid)
                .map_or(0, |facts| facts.class)
        };
        let members: Vec<(u64, u8)> = members
            .iter()
            .map(|member| (member.character_guid, class_of(member.character_guid)))
            .collect();
        let next_reminder_at = first_reminder_at(ctx).map_err(StoneOpError::Invalid)?;
        queue_party(ctx, group.group_id, bucket, &members, next_reminder_at);
    } else {
        upsert_seeker(ctx, actor_guid, bucket, actor.class, 0);
        push_queue(ctx, actor_guid, area_id, queue_status::JOINED_QUEUE);
    }
    match_bucket(ctx, bucket).map_err(StoneOpError::Invalid)
}

fn first_reminder_at(ctx: &ReducerContext) -> Result<Timestamp, String> {
    ctx.timestamp
        .checked_add(TimeDuration::from_micros(REMINDER_INTERVAL_MICROS))
        .ok_or_else(|| "reminder time overflow".to_string())
}

/// Queue `group_id` with `members`, `(guid, class)` each, and tell each JOINED. Re-queueing resets
/// the area and wait time and drops Seeker rows of former members.
fn queue_party(
    ctx: &ReducerContext,
    group_id: u64,
    bucket: Bucket,
    members: &[(u64, u8)],
    next_reminder_at: Timestamp,
) {
    let row = MeetingStoneParty {
        group_id,
        area_id: bucket.area_id,
        team: bucket.team,
        queued_at: ctx.timestamp,
        next_reminder_at,
    };
    let parties = ctx.db.game_meeting_stone_party();
    if parties.group_id().find(group_id).is_some() {
        parties.group_id().update(row);
    } else {
        parties.insert(row);
    }
    let seeker_rows = ctx.db.game_meeting_stone_seeker();
    let departed: Vec<u64> = seeker_rows
        .by_group()
        .filter(group_id)
        .map(|seeker| seeker.character_guid)
        .filter(|guid| !members.iter().any(|(member, _)| member == guid))
        .collect();
    for guid in departed {
        seeker_rows.character_guid().delete(guid);
    }
    for &(guid, class) in members {
        upsert_seeker(ctx, guid, bucket, class, group_id);
    }
    for &(guid, _) in members {
        push_queue(ctx, guid, bucket.area_id, queue_status::JOINED_QUEUE);
    }
}

fn upsert_seeker(
    ctx: &ReducerContext,
    character_guid: u64,
    bucket: Bucket,
    class: u8,
    group_id: u64,
) {
    let row = MeetingStoneSeeker {
        character_guid,
        area_id: bucket.area_id,
        team: bucket.team,
        class,
        group_id,
        queued_at: ctx.timestamp,
    };
    let seekers = ctx.db.game_meeting_stone_seeker();
    if seekers.character_guid().find(character_guid).is_some() {
        seekers.character_guid().update(row);
    } else {
        seekers.insert(row);
    }
}

fn push_queue(ctx: &ReducerContext, recipient_guid: u64, area_id: u32, status: u8) {
    crate::group::push_event(
        ctx,
        recipient_guid,
        event_kind::QUEUE,
        0,
        encode_queue(area_id, status),
    );
}

fn push_to_each(ctx: &ReducerContext, recipients: &[u64], kind: u8, other_guid: u64) {
    for &recipient in recipients {
        crate::group::push_event(ctx, recipient, kind, other_guid, String::new());
    }
}

fn member_guids(ctx: &ReducerContext, group_id: u64) -> Vec<u64> {
    let mut members = crate::group::members_of(ctx, group_id);
    members.sort_unstable_by_key(|member| member.id);
    members
        .into_iter()
        .map(|member| member.character_guid)
        .collect()
}

/// A solo Seeker leaves with LEAVE_QUEUE, and a queued Party's leader takes the whole Party out.
/// Any other member only hears NONE.
fn leave(ctx: &ReducerContext, actor_guid: u64) -> Result<(), StoneOpError> {
    let Some((group, _)) = crate::group::group_with_members(ctx, actor_guid)? else {
        let seekers = ctx.db.game_meeting_stone_seeker();
        if seekers.character_guid().find(actor_guid).is_some() {
            seekers.character_guid().delete(actor_guid);
            push_queue(ctx, actor_guid, 0, queue_status::LEAVE_QUEUE);
        }
        return Ok(());
    };
    let queued = ctx
        .db
        .game_meeting_stone_party()
        .group_id()
        .find(group.group_id)
        .is_some();
    if group.leader_guid == actor_guid && queued {
        dequeue_party(ctx, group.group_id, queue_status::LEAVE_QUEUE);
    } else {
        push_queue(ctx, actor_guid, 0, queue_status::NONE);
    }
    Ok(())
}

/// Take a queued Party out of the queue. Every member hears `SETQUEUE(0, status)`.
pub(crate) fn dequeue_party(ctx: &ReducerContext, group_id: u64, status: u8) {
    if ctx
        .db
        .game_meeting_stone_party()
        .group_id()
        .find(group_id)
        .is_none()
    {
        return;
    }
    for guid in member_guids(ctx, group_id) {
        push_queue(ctx, guid, 0, status);
    }
    dequeue_party_rows(ctx, group_id);
}

fn dequeue_party_rows(ctx: &ReducerContext, group_id: u64) {
    ctx.db
        .game_meeting_stone_party()
        .group_id()
        .delete(group_id);
    let seekers = ctx.db.game_meeting_stone_seeker();
    let guids: Vec<u64> = seekers
        .by_group()
        .filter(group_id)
        .map(|seeker| seeker.character_guid)
        .collect();
    for guid in guids {
        seekers.character_guid().delete(guid);
    }
}

/// Every member hears COMPLETE, then `SETQUEUE(0, NONE)`, and the Party leaves the queue.
fn complete_party(ctx: &ReducerContext, group_id: u64) {
    let members = member_guids(ctx, group_id);
    push_to_each(ctx, &members, event_kind::COMPLETE, 0);
    for &guid in &members {
        push_queue(ctx, guid, 0, queue_status::NONE);
    }
    dequeue_party_rows(ctx, group_id);
}

// =================================================================================================
//  The bucket pass
// =================================================================================================

/// Match `bucket` until nothing changes: queued Parties fill oldest first, then five solo Seekers
/// form a Party and the Parties fill again. An `Err` is a broken durable relationship.
pub(crate) fn match_bucket(ctx: &ReducerContext, bucket: Bucket) -> Result<(), String> {
    loop {
        let (mut solos, parties) = read_bucket(ctx, bucket);
        for party in &parties {
            fill_party(ctx, party, &mut solos)?;
        }
        let Some((leader, member)) = formation(&solos) else {
            return Ok(());
        };
        form_party(ctx, bucket, leader, member)?;
    }
}

/// The bucket's solo Seekers, and its queued Parties oldest first. Drops what could make a Stone
/// Add fail: a solo Seeker without a live claim (the lease reaper may take 15 s) or in a Group, and
/// a party row whose Group is gone or is a Raid. `remove_member` cannot roll a pass back.
fn read_bucket(
    ctx: &ReducerContext,
    bucket: Bucket,
) -> (Vec<MeetingStoneSeeker>, Vec<MeetingStoneParty>) {
    let seekers = ctx.db.game_meeting_stone_seeker();
    let rows: Vec<MeetingStoneSeeker> = seekers
        .by_bucket()
        .filter((bucket.area_id, bucket.team))
        .collect();
    let mut solos = Vec::new();
    let mut group_ids = BTreeSet::new();
    for seeker in rows {
        if seeker.group_id != 0 {
            group_ids.insert(seeker.group_id);
        } else if has_live_claim(ctx, seeker.character_guid)
            && crate::group::group_of(ctx, seeker.character_guid).is_none()
        {
            solos.push(seeker);
        } else {
            seekers.character_guid().delete(seeker.character_guid);
        }
    }
    let mut parties: Vec<MeetingStoneParty> = group_ids
        .into_iter()
        .filter_map(|group_id| ctx.db.game_meeting_stone_party().group_id().find(group_id))
        .filter(|party| {
            let kind = ctx
                .db
                .game_group()
                .group_id()
                .find(party.group_id)
                .map(|group| crate::group::group_kind_of(&group));
            let queueable = kind == Some(GroupKind::Party);
            if !queueable {
                spacetimedb::log::warn!(
                    "meeting stone party {} left the queue: its Group is {kind:?}",
                    party.group_id
                );
                dequeue_party_rows(ctx, party.group_id);
            }
            queueable
        })
        .collect();
    parties.sort_unstable_by_key(|party| (party.queued_at, party.group_id));
    (solos, parties)
}

fn has_live_claim(ctx: &ReducerContext, character_guid: u64) -> bool {
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    ctx.db
        .game_account_claim()
        .by_character()
        .filter(character_guid)
        .any(|claim| !claim.closed && claim.expires_micros > now)
}

/// Give `party` Stone Adds while a Seeker fits an Open Role. The current members hear MEMBER_ADDED
/// before each add. A full Party is completed, never joined.
fn fill_party(
    ctx: &ReducerContext,
    party: &MeetingStoneParty,
    solos: &mut Vec<MeetingStoneSeeker>,
) -> Result<(), String> {
    let bucket = Bucket {
        area_id: party.area_id,
        team: party.team,
    };
    loop {
        let members = member_guids(ctx, party.group_id);
        if members.len() >= GROUP_MAX_MEMBERS {
            complete_party(ctx, party.group_id);
            return Ok(());
        }
        let classes: Vec<u8> = members
            .iter()
            .map(|&guid| party_class(ctx, guid, party.group_id))
            .collect();
        let Some(pick) = next_stone_add(open_roles(&classes), solos) else {
            return Ok(());
        };
        let joiner = solos.remove(pick);
        let plan = plan_stone_add(ctx, &joiner, JoinTarget::Group(party.group_id))?;
        push_to_each(
            ctx,
            &members,
            event_kind::MEMBER_ADDED,
            joiner.character_guid,
        );
        stone_add(ctx, &joiner, plan);
        upsert_seeker(
            ctx,
            joiner.character_guid,
            bucket,
            joiner.class,
            party.group_id,
        );
    }
}

fn party_class(ctx: &ReducerContext, guid: u64, group_id: u64) -> u8 {
    ctx.db
        .game_meeting_stone_seeker()
        .character_guid()
        .find(guid)
        .filter(|seeker| seeker.group_id == group_id)
        .map_or(0, |seeker| seeker.class)
}

/// Form a Party of `leader` and `member` and queue it. The leader hears MEMBER_ADDED before the
/// Party exists, then both hear JOINED.
fn form_party(
    ctx: &ReducerContext,
    bucket: Bucket,
    leader: &MeetingStoneSeeker,
    member: &MeetingStoneSeeker,
) -> Result<(), String> {
    let plan = plan_stone_add(
        ctx,
        member,
        JoinTarget::NewParty {
            leader_guid: leader.character_guid,
        },
    )?;
    let next_reminder_at = first_reminder_at(ctx)?;
    crate::group::push_event(
        ctx,
        leader.character_guid,
        event_kind::MEMBER_ADDED,
        member.character_guid,
        String::new(),
    );
    ctx.db
        .game_meeting_stone_seeker()
        .character_guid()
        .delete(leader.character_guid);
    let group_id = stone_add(ctx, member, plan);
    queue_party(
        ctx,
        group_id,
        bucket,
        &[
            (leader.character_guid, leader.class),
            (member.character_guid, member.class),
        ],
        next_reminder_at,
    );
    Ok(())
}

fn plan_stone_add(
    ctx: &ReducerContext,
    joiner: &MeetingStoneSeeker,
    target: JoinTarget,
) -> Result<JoinPlan, String> {
    crate::group::plan_join(ctx, joiner.character_guid, target).map_err(|error| {
        format!(
            "stone add of {} to {target:?} failed: {error:?}",
            joiner.character_guid
        )
    })
}

/// Add `joiner` through `plan` and advance the Group's Roster Revision. Returns the Group id.
fn stone_add(ctx: &ReducerContext, joiner: &MeetingStoneSeeker, plan: JoinPlan) -> u64 {
    ctx.db
        .game_meeting_stone_seeker()
        .character_guid()
        .delete(joiner.character_guid);
    let joined = plan.apply(ctx);
    crate::group::advance_group_revision(ctx, joined.group_id, !joined.formed, true);
    joined.group_id
}

// =================================================================================================
//  Group core hooks
// =================================================================================================

/// `character_guid` joined `group_id` through an accepted invite. A solo Seeker leaves the queue,
/// with LEAVE_QUEUE unless the Party is queued for the same area. A queued Party gains a Seeker,
/// completes at five, and otherwise rematches, because the new member moves the Open Roles.
pub(crate) fn party_joined(
    ctx: &ReducerContext,
    group_id: u64,
    character_guid: u64,
    class: u8,
) -> Result<(), String> {
    let party = ctx.db.game_meeting_stone_party().group_id().find(group_id);
    let seekers = ctx.db.game_meeting_stone_seeker();
    if let Some(solo) = seekers
        .character_guid()
        .find(character_guid)
        .filter(|seeker| seeker.group_id == 0)
    {
        seekers.character_guid().delete(character_guid);
        if party.as_ref().map(|party| party.area_id) != Some(solo.area_id) {
            push_queue(ctx, character_guid, 0, queue_status::LEAVE_QUEUE);
        }
    }
    let Some(party) = party else {
        return Ok(());
    };
    let bucket = Bucket {
        area_id: party.area_id,
        team: party.team,
    };
    upsert_seeker(ctx, character_guid, bucket, class, group_id);
    if crate::group::members_of(ctx, group_id).len() >= GROUP_MAX_MEMBERS {
        complete_party(ctx, group_id);
        return Ok(());
    }
    match_bucket(ctx, bucket)
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PartyDeparture {
    pub(crate) group_id: u64,
    pub(crate) character_guid: u64,
    pub(crate) departure: Departure,
    pub(crate) leader_changed: bool,
}

/// A member left a queued Party that survives it, after the member row is gone and before the
/// roster list (cm:Groups/Group.cpp:413-476). Returns the bucket to rematch, if any.
///
/// - Left, same leader: the leaver hears NONE, the rest PARTY_MEMBER_LEFT_LFG, the Party stays.
/// - Left, new leader: the leaver hears NONE and the Party leaves with LEAVE_QUEUE.
/// - Kicked: the rest hear PARTY_MEMBER_REMOVED_PARTY_REMOVED and the Party leaves with
///   LEAVE_QUEUE. The kicked Character, if it has a World Session, hears
///   LOOKING_FOR_NEW_PARTY_IN_QUEUE and waits alone.
pub(crate) fn party_left(ctx: &ReducerContext, departed: PartyDeparture) -> Option<Bucket> {
    let party = ctx
        .db
        .game_meeting_stone_party()
        .group_id()
        .find(departed.group_id)?;
    let bucket = Bucket {
        area_id: party.area_id,
        team: party.team,
    };
    let guid = departed.character_guid;
    match departed.departure {
        Departure::Kicked => {
            let class = party_class(ctx, guid, departed.group_id);
            for rest in member_guids(ctx, departed.group_id) {
                push_queue(
                    ctx,
                    rest,
                    0,
                    queue_status::PARTY_MEMBER_REMOVED_PARTY_REMOVED,
                );
            }
            dequeue_party(ctx, departed.group_id, queue_status::LEAVE_QUEUE);
            // A kicked playerbot or offline member only leaves, as in cmangos (`if (player)`).
            if !has_live_claim(ctx, guid) {
                return None;
            }
            push_queue(
                ctx,
                guid,
                bucket.area_id,
                queue_status::LOOKING_FOR_NEW_PARTY_IN_QUEUE,
            );
            upsert_seeker(ctx, guid, bucket, class, 0);
            push_queue(ctx, guid, bucket.area_id, queue_status::JOINED_QUEUE);
            Some(bucket)
        }
        Departure::Left => {
            push_queue(ctx, guid, 0, queue_status::NONE);
            if departed.leader_changed {
                dequeue_party(ctx, departed.group_id, queue_status::LEAVE_QUEUE);
                return None;
            }
            for rest in member_guids(ctx, departed.group_id) {
                push_queue(
                    ctx,
                    rest,
                    bucket.area_id,
                    queue_status::PARTY_MEMBER_LEFT_LFG,
                );
            }
            ctx.db
                .game_meeting_stone_seeker()
                .character_guid()
                .delete(guid);
            Some(bucket)
        }
    }
}

/// A queued Party disbanded: every former member hears NONE, and nobody waits on alone.
pub(crate) fn party_disbanded(ctx: &ReducerContext, group_id: u64, former_members: &[u64]) {
    if ctx
        .db
        .game_meeting_stone_party()
        .group_id()
        .find(group_id)
        .is_none()
    {
        return;
    }
    for &guid in former_members {
        push_queue(ctx, guid, 0, queue_status::NONE);
    }
    dequeue_party_rows(ctx, group_id);
}

// =================================================================================================
//  Reminders
// =================================================================================================

/// Every member of a queued Party whose reminder is due hears IN_PROGRESS. Scheduler-only. The
/// party table holds only queued Parties, so this reads it whole.
#[reducer]
pub fn remind_queued_parties(ctx: &ReducerContext, _schedule: MeetingStoneReminderSchedule) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    let Some(next_reminder_at) = ctx
        .timestamp
        .checked_add(TimeDuration::from_micros(REMINDER_INTERVAL_MICROS))
    else {
        spacetimedb::log::error!("meeting stone reminder time overflow");
        return;
    };
    let parties = ctx.db.game_meeting_stone_party();
    let due: Vec<MeetingStoneParty> = parties
        .iter()
        .filter(|party| party.next_reminder_at <= ctx.timestamp)
        .collect();
    for party in due {
        let members = member_guids(ctx, party.group_id);
        push_to_each(ctx, &members, event_kind::IN_PROGRESS, 0);
        parties.group_id().update(MeetingStoneParty {
            next_reminder_at,
            ..party
        });
    }
}

/// Arm the reminder tick with exactly one schedule row. `init` and `debug_repair_after_publish`
/// both call this.
pub(crate) fn rearm_reminder_schedule(ctx: &ReducerContext) {
    let schedule = ctx.db.game_meeting_stone_reminder_schedule();
    let stale: Vec<u64> = schedule.iter().map(|row| row.scheduled_id).collect();
    for scheduled_id in stale {
        schedule.scheduled_id().delete(scheduled_id);
    }
    schedule.insert(MeetingStoneReminderSchedule {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(REMINDER_TICK_MICROS)),
    });
}

/// `character_guid`'s Account Claim ended: drop its solo Seeker row without a packet, since no
/// session is left to hear one. A party Seeker row ends with the membership instead.
pub(crate) fn claim_ended(ctx: &ReducerContext, character_guid: u64) {
    let seekers = ctx.db.game_meeting_stone_seeker();
    if seekers
        .character_guid()
        .find(character_guid)
        .is_some_and(|seeker| seeker.group_id == 0)
    {
        seekers.character_guid().delete(character_guid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_level_range_is_inclusive_and_a_zero_maximum_has_no_upper_bound() {
        assert!(!level_in_range(14, 15, 20));
        assert!(level_in_range(15, 15, 20));
        assert!(level_in_range(20, 15, 20));
        assert!(!level_in_range(21, 15, 20));
        assert!(level_in_range(60, 15, 0));
        assert!(!level_in_range(14, 15, 0));
    }

    fn party(leads: bool, kind: GroupKind, members: usize) -> Membership {
        Membership {
            leads,
            kind,
            members,
        }
    }

    #[test]
    fn a_party_leader_with_room_queues_the_party() {
        for members in 2..GROUP_MAX_MEMBERS {
            assert_eq!(
                party_join_gate(party(true, GroupKind::Party, members)),
                Ok(())
            );
        }
    }

    /// A member of a full raid hears only that it does not lead.
    #[test]
    fn the_join_gate_checks_leader_then_raid_then_size() {
        use MeetingStoneRefusal::*;
        assert_eq!(
            party_join_gate(party(false, GroupKind::Raid, 40)),
            Err(NotLeader)
        );
        assert_eq!(
            party_join_gate(party(false, GroupKind::Party, 2)),
            Err(NotLeader)
        );
        assert_eq!(
            party_join_gate(party(true, GroupKind::Raid, 40)),
            Err(RaidGroup)
        );
        assert_eq!(
            party_join_gate(party(true, GroupKind::Raid, 2)),
            Err(RaidGroup)
        );
        assert_eq!(
            party_join_gate(party(true, GroupKind::Party, 5)),
            Err(PartyFull)
        );
    }
}
