//! The Meeting Stone Queue: a Character, or the Party it leads, waits at a dungeon's Meeting Stone
//! for other Seekers of the same dungeon area and team.
//!
//! Two Durable Requests split the work. [`gw_admit_meeting_stone`] runs on the Character's Home
//! Shard, the only database that holds the stone and the live entity, and writes nothing.
//! [`realm_meeting_stone_op`] runs on Realm-core, where party membership is authoritative, so
//! Seekers on different World Shards wait in one queue. Realm-core holds no Character rows, so the
//! Gateway conveys each Seeker's race and class as [`SeekerFacts`].
//!
//! Notifications are `game_group_event` rows of the
//! [`lyracore_shared::meeting_stone::event_kind`] kinds. A Refusal rolls its transaction back and
//! returns its stable tag; the Gateway answers the party Refusals with `SMSG_MEETINGSTONE_JOINFAILED`.
//!
//! A solo Seeker lasts as long as the Account Claim of its World Session: [`claim_ended`] runs
//! where a claim is released, replaced or reaped. A party Seeker lasts as long as its membership.
//!
//! Matching runs inside the transaction that changes a bucket ([`match_bucket`]), not on a tick:
//! once cmangos's `bool` priority compare is read literally, its queue thread reduces to longest
//! wait per Open Role, so a tick would add latency and no behavior. Every Stone Add goes through
//! the party authority's join core, [`crate::group::plan_join`], the same path an accepted invite
//! takes. The group cores call back here when a queued Party's roster changes.

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

use crate::group::{Departure, JoinPlan, JoinTarget};
use crate::{game_account_claim, game_group};

/// A queued party is reminded that the queue is still working every five minutes
/// (cm:LFG/LFGMgr.cpp:55).
const REMINDER_INTERVAL_MICROS: i64 = 5 * 60 * 1_000_000;

/// How often [`remind_queued_parties`] looks for a due reminder.
const REMINDER_TICK_MICROS: i64 = 5_000_000;

/// One Meeting Stone template's level range and dungeon area, from the dump's `data0`, `data1` and
/// `data2`. Private: the Gateway reads it with the owner token. Imported with the `gameobjects`
/// family; a Package does not author it. [static]
#[table(accessor = game_meeting_stone)]
pub struct MeetingStone {
    #[primary_key]
    pub entry: u32,
    pub min_level: u32,
    /// 0 means no upper bound.
    pub max_level: u32,
    /// The dungeon's AreaTable id.
    pub area_id: u32,
}

/// One Seeker: a Character in the Meeting Stone Queue, alone or as a member of a queued Party.
/// Private: the Gateway reads it with the owner token to answer `CMSG_MEETINGSTONE_INFO`. [entity]
#[table(
    accessor = game_meeting_stone_seeker,
    index(accessor = by_bucket, btree(columns = [area_id, team])),
    index(accessor = by_group, btree(columns = [group_id]))
)]
pub struct MeetingStoneSeeker {
    #[primary_key]
    pub character_guid: u64,
    pub area_id: u32,
    /// `lyracore_shared::faction` team id. Seekers of different teams never meet.
    pub team: u32,
    /// The class the Gateway conveyed. 0 when it could not read one.
    pub class: u8,
    /// The queued party. 0 for a solo Seeker.
    pub group_id: u64,
    pub queued_at: Timestamp,
}

/// One queued Party. A row exists exactly while the party is queued, and its Seeker rows are its
/// current members. Private and Module only. [entity]
#[table(accessor = game_meeting_stone_party)]
pub struct MeetingStoneParty {
    #[primary_key]
    pub group_id: u64,
    pub area_id: u32,
    pub team: u32,
    pub queued_at: Timestamp,
    /// When the members next get `SMSG_MEETINGSTONE_IN_PROGRESS`.
    pub next_reminder_at: Timestamp,
}

/// Drives [`remind_queued_parties`]. One row; its `scheduled_at` is the cadence.
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

/// What the Gateway read about one Seeker from whichever World Shard holds it.
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

/// May `actor_guid` use the Meeting Stone `go_guid`? Runs on the actor's Home Shard and writes
/// nothing. Refuses an actor without a World Session, one on a taxi flight, a GameObject out of
/// reach or not a stone, and a level outside the stone's range. Neither core checks the level;
/// the client does, and 1.12 has no failure code for it, so the Gateway drops this Refusal.
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
    // Humans only. An Operator-driven Character without a World Session never queues.
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

/// The shared GameObject Gate's answer as a Meeting Stone Refusal. `usable_go` produces only the
/// four kinds named here.
fn gate_refusal(kind: crate::actor::ActionRefusalKind) -> MeetingStoneRefusal {
    use crate::actor::ActionRefusalKind;
    match kind {
        ActionRefusalKind::MissingTarget => MeetingStoneRefusal::NotAMeetingStone,
        ActionRefusalKind::OtherPartition => MeetingStoneRefusal::OtherPartition,
        ActionRefusalKind::OutOfRange => MeetingStoneRefusal::OutOfRange,
        _ => MeetingStoneRefusal::ActorUnavailable,
    }
}

/// Inclusive on both ends. A `max_level` of 0 has no upper bound.
fn level_in_range(level: u32, min_level: u32, max_level: u32) -> bool {
    level >= min_level && (max_level == 0 || level <= max_level)
}

/// The actor's party, as the JOIN Gate needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Membership {
    leads: bool,
    kind: GroupKind,
    members: usize,
}

/// A grouped actor queues only as the leader of a Party with room, checked in cmangos's order:
/// not the leader, then a raid, then a full party (cm:LFG/LFGHandler.cpp:58-77).
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
    /// A broken durable relationship or a malformed request. Untagged, so the Gateway treats it
    /// as a failure with an unknown outcome.
    Invalid(String),
}

impl From<MeetingStoneRefusal> for StoneOpError {
    fn from(refusal: MeetingStoneRefusal) -> Self {
        Self::Refused(refusal)
    }
}

/// Run one Meeting Stone Queue op for `request_actor` on the party authority.
///
/// Operator-gated because the actor is an argument: Realm-core has no live entity to derive it
/// from. `area_id` and `seekers` are JOIN's: the stone's dungeon area, and the facts of the actor
/// or of every member of the party it leads. A missing member's class is 0. A JOIN ends with the
/// bucket pass, so it can form or fill Parties in the same transaction. LEAVE reads neither.
/// More than [`GROUP_MAX_MEMBERS`] facts, facts that omit the actor, or an unknown op byte is an
/// untagged error.
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

fn membership_of(
    ctx: &ReducerContext,
    character_guid: u64,
) -> Result<Option<(crate::Group, Vec<crate::GroupMember>)>, StoneOpError> {
    let membership = crate::group::checked_group_membership(ctx, character_guid)
        .map_err(|error| StoneOpError::Invalid(format!("{error:?}")))?;
    Ok(membership.map(|(_, group)| {
        let members = crate::group::members_of(ctx, group.group_id);
        (group, members)
    }))
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
    if let Some((group, members)) = membership_of(ctx, actor_guid)? {
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

/// When a Party queued now first hears IN_PROGRESS.
fn first_reminder_at(ctx: &ReducerContext) -> Result<Timestamp, String> {
    ctx.timestamp
        .checked_add(TimeDuration::from_micros(REMINDER_INTERVAL_MICROS))
        .ok_or_else(|| "reminder time overflow".to_string())
}

/// Queue `group_id` in `bucket` with `members` as its Seekers, `(guid, class)` each, and tell each
/// of them JOINED. Re-queueing a party replaces its area and wait time and drops the
/// Seeker rows of Characters that are no longer members.
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

/// Queue `character_guid`, replacing any earlier row and its wait time.
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

/// `SMSG_MEETINGSTONE_SETQUEUE(area_id, status)` to `recipient_guid`.
fn push_queue(ctx: &ReducerContext, recipient_guid: u64, area_id: u32, status: u8) {
    crate::group::push_event(
        ctx,
        recipient_guid,
        event_kind::QUEUE,
        0,
        encode_queue(area_id, status),
    );
}

/// An event of `kind` with no payload to each of `recipients`.
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

/// cm:LFG/LFGHandler.cpp:86-110. A solo Seeker leaves with LEAVE_QUEUE. The leader of a queued
/// party takes the whole party out, and every member gets LEAVE_QUEUE. Anyone else in a party only
/// gets NONE. Nobody queued and no party: nothing happens.
fn leave(ctx: &ReducerContext, actor_guid: u64) -> Result<(), StoneOpError> {
    let Some((group, _)) = membership_of(ctx, actor_guid)? else {
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

/// Take a queued party out of the queue: every member gets `SETQUEUE(0, status)`, and the party
/// row and its Seeker rows go. Nothing happens for a party that is not queued.
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

/// Delete a queued party's row and every Seeker row it holds.
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

/// A queued party with five members is complete: every member gets COMPLETE, then
/// `SETQUEUE(0, NONE)`, and the party leaves the queue (cm:LFG/LFGQueue.cpp:420-454).
fn complete_party(ctx: &ReducerContext, group_id: u64) {
    let members = member_guids(ctx, group_id);
    push_to_each(ctx, &members, event_kind::COMPLETE, 0);
    for &guid in &members {
        push_queue(ctx, guid, 0, queue_status::NONE);
    }
    dequeue_party_rows(ctx, group_id);
}

// =================================================================================================
//  Roles
// =================================================================================================

/// 1.12 class ids (ChrClasses.dbc).
mod class {
    pub const WARRIOR: u8 = 1;
    pub const PALADIN: u8 = 2;
    pub const HUNTER: u8 = 3;
    pub const ROGUE: u8 = 4;
    pub const PRIEST: u8 = 5;
    pub const SHAMAN: u8 = 7;
    pub const MAGE: u8 = 8;
    pub const WARLOCK: u8 = 9;
    pub const DRUID: u8 = 11;
}

/// A Party's dungeon roles, in the order a Party fills them (cm:Groups/Group.cpp:1594-1599).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Tank,
    Healer,
    Damage,
}

const ROLE_ORDER: [Role; 3] = [Role::Tank, Role::Healer, Role::Damage];

/// A Party has three damage roles (cm:LFG/LFGMgr.h:51).
const DAMAGE_ROLES: u8 = 3;

/// How well `class` fills `role`: 0 not at all, then 1 low, 2 normal, 3 high
/// (cm:LFG/LFGMgr.cpp:108-158). A class fills exactly the roles it has a priority for, which is
/// cmangos's class role table (cm:LFG/LFGMgr.cpp:91-106). An unknown class fills nothing.
fn role_priority(class: u8, role: Role) -> u8 {
    use class::*;
    const LOW: u8 = 1;
    const NORMAL: u8 = 2;
    const HIGH: u8 = 3;
    match (role, class) {
        (Role::Tank, WARRIOR) => HIGH,
        (Role::Tank, DRUID | PALADIN) => NORMAL,
        (Role::Healer, DRUID | PALADIN | PRIEST | SHAMAN) => HIGH,
        (Role::Damage, HUNTER | MAGE | ROGUE | WARLOCK) => HIGH,
        (Role::Damage, DRUID | PALADIN | SHAMAN | WARRIOR) => NORMAL,
        (Role::Damage, PRIEST) => LOW,
        _ => 0,
    }
}

fn fills(class: u8, role: Role) -> bool {
    role_priority(class, role) > 0
}

/// A queued Party's Open Roles: which of its one tank, one healer and three damage roles no
/// member's class fills yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OpenRoles {
    tank: bool,
    healer: bool,
    damage: u8,
}

impl OpenRoles {
    const ALL: Self = Self {
        tank: true,
        healer: true,
        damage: DAMAGE_ROLES,
    };

    fn is_open(self, role: Role) -> bool {
        match role {
            Role::Tank => self.tank,
            Role::Healer => self.healer,
            Role::Damage => self.damage > 0,
        }
    }

    fn fill(&mut self, role: Role) {
        match role {
            Role::Tank => self.tank = false,
            Role::Healer => self.healer = false,
            Role::Damage => self.damage = self.damage.saturating_sub(1),
        }
    }
}

/// The Open Roles of a Party whose members have `classes`, in join order
/// (cm:Groups/Group.cpp:1589-1685). Each member takes the first of tank, healer and damage that its
/// class fills and that is still open, unless a member not yet placed has a higher priority for it.
/// A member left without a role holds a seat and fills nothing.
fn open_roles(classes: &[u8]) -> OpenRoles {
    let mut open = OpenRoles::ALL;
    let mut placed = vec![false; classes.len()];
    for (seat, &class) in classes.iter().enumerate() {
        let outranked = |role: Role| {
            classes.iter().enumerate().any(|(other, &other_class)| {
                other != seat
                    && !placed[other]
                    && role_priority(other_class, role) > role_priority(class, role)
            })
        };
        let taken = ROLE_ORDER
            .into_iter()
            .find(|&role| fills(class, role) && open.is_open(role) && !outranked(role));
        if let Some(role) = taken {
            open.fill(role);
            placed[seat] = true;
        }
    }
    open
}

/// The longest wait first, ties by guid.
fn wait_order(seeker: &MeetingStoneSeeker) -> (Timestamp, u64) {
    (seeker.queued_at, seeker.character_guid)
}

/// The index in `solos` of the next Stone Add for a Party with `open` roles: for tank, then healer,
/// then damage, if open, the longest-waiting Seeker whose class fills it. cm:LFG/LFGQueue.cpp:292-390
/// compares class priority as a `bool`, so every capable Seeker ties on class and the wait decides.
fn next_stone_add(open: OpenRoles, solos: &[MeetingStoneSeeker]) -> Option<usize> {
    ROLE_ORDER
        .into_iter()
        .filter(|&role| open.is_open(role))
        .find_map(|role| {
            solos
                .iter()
                .enumerate()
                .filter(|(_, seeker)| fills(seeker.class, role))
                .min_by_key(|(_, seeker)| wait_order(seeker))
                .map(|(index, _)| index)
        })
}

/// Five solo Seekers in one bucket form a Party: the longest wait leads, the next joins, and the
/// bucket pass fills the rest (cm:LFG/LFGQueue.cpp:173-221). Both cores count Seekers across every
/// area and then block on the first Seeker's area; counting one bucket removes that block.
fn formation(solos: &[MeetingStoneSeeker]) -> Option<(&MeetingStoneSeeker, &MeetingStoneSeeker)> {
    if solos.len() < GROUP_MAX_MEMBERS {
        return None;
    }
    let mut waiting: Vec<&MeetingStoneSeeker> = solos.iter().collect();
    waiting.sort_unstable_by_key(|seeker| wait_order(seeker));
    Some((waiting[0], waiting[1]))
}

// =================================================================================================
//  The bucket pass
// =================================================================================================

/// Match `bucket` until nothing changes. Each queued Party, oldest first, takes Stone Adds for its
/// Open Roles; then, while the bucket holds five solo Seekers, they form a Party and the Parties fill
/// again. Every Party this forms or changes advances its Roster Revision.
///
/// A solo Seeker whose Account Claim is closed or expired leaves the queue silently when the pass
/// meets it. The Account Claim's end drops it too, but the lease reaper may take up to 15 s to close
/// an expired claim. An `Err` is a broken durable relationship.
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

/// The bucket's waiting solo Seekers, and its queued Parties oldest first. Reads the bucket through
/// its index. Drops a solo Seeker without a live Account Claim, a stale solo row of a Character
/// that is in a Group, and a party row whose Group is gone or is a Raid. What is left cannot make
/// a Stone Add fail, so a pass that runs where it cannot roll back leaves no half-done add.
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

/// Whether `character_guid` has an open Account Claim before its deadline.
fn has_live_claim(ctx: &ReducerContext, character_guid: u64) -> bool {
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    ctx.db
        .game_account_claim()
        .by_character()
        .filter(character_guid)
        .any(|claim| !claim.closed && claim.expires_micros > now)
}

/// Give `party` Stone Adds from `solos` while it has fewer than five members and a Seeker fits an
/// Open Role. Before each add the current members get MEMBER_ADDED; at five members the party is
/// complete (cm:LFG/LFGQueue.cpp:372-383). A full Party is completed, never joined, so a Stone Add
/// never meets a full Group.
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

/// The class a queued party's Seeker row holds for `guid`, or 0.
fn party_class(ctx: &ReducerContext, guid: u64, group_id: u64) -> u8 {
    ctx.db
        .game_meeting_stone_seeker()
        .character_guid()
        .find(guid)
        .filter(|seeker| seeker.group_id == group_id)
        .map_or(0, |seeker| seeker.class)
}

/// Form a Party of `leader` and `member` and queue it in `bucket`. The leader hears MEMBER_ADDED
/// before the Party exists, then both hear JOINED (cm:LFG/LFGQueue.cpp:190-218).
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

/// Check that `joiner` can join `target`, before the Stone Add writes anything.
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

/// Take `joiner` out of the solo queue and add it through `plan`, advancing the Group's Roster
/// Revision. Returns the Group id.
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

/// `character_guid` joined `group_id` through an accepted invite. A solo Seeker leaves the queue
/// and hears LEAVE_QUEUE unless the Party is queued for the same area (cm:Groups/Group.cpp:360-374).
/// A queued Party gains the Character as a Seeker with `class`; at five members it is complete,
/// below five the bucket pass runs, because the new member can move the Open Roles.
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

/// A member left a Party that survives it. See [`party_left`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct PartyDeparture {
    pub(crate) group_id: u64,
    pub(crate) character_guid: u64,
    pub(crate) departure: Departure,
    pub(crate) leader_changed: bool,
}

/// A member left a queued Party that survives it. Runs after the member row is gone and before the
/// roster list.
///
/// - Left, leader unchanged: the leaver hears NONE, the rest PARTY_MEMBER_LEFT_LFG, and the party
///   stays queued without the leaver (cm:Groups/Group.cpp:437-447).
/// - Left, leader changed: the leaver hears NONE and the party leaves the queue with LEAVE_QUEUE
///   (cm:Groups/Group.cpp:464-476).
/// - Kicked: the rest hear PARTY_MEMBER_REMOVED_PARTY_REMOVED and the party leaves the queue with
///   LEAVE_QUEUE. A kicked Character with a live Account Claim hears
///   LOOKING_FOR_NEW_PARTY_IN_QUEUE and waits alone with its class and team
///   (cm:Groups/Group.cpp:413-435). A kicked playerbot or offline member only leaves.
///
/// Returns the bucket to match when a role opened or a Character was queued.
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
            // Only a Character with a World Session waits alone: a kicked playerbot or offline
            // member just leaves, as in cmangos (`if (player)`).
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

/// A queued Party disbanded: every former member hears NONE and the party leaves the queue. Nobody
/// waits on alone (cm:Groups/Group.cpp:550-562).
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

/// Remind every queued Party whose reminder is due that the queue is still working: each member
/// gets IN_PROGRESS, and the next reminder is five minutes on (cm:LFG/LFGQueue.cpp:149-163).
/// Scheduler-only. The party table is small and holds only queued Parties, so this reads it whole.
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

/// Arm the reminder tick, leaving exactly one schedule row. `init` and `debug_repair_after_publish`
/// both call this, so the two paths arm one interval.
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

/// The Account Claim that admitted `character_guid` ended: drop its solo Seeker row, silently. The
/// Character's World Session is gone, so no packet could reach it. A party Seeker row stays, because
/// it ends with the party membership, not the session.
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

    /// cm:LFG/LFGMgr.cpp:91-158, row by row: the roles each class fills and its priority in each.
    #[test]
    fn the_role_table_is_the_cmangos_class_table() {
        use class::*;
        let row = |class: u8| ROLE_ORDER.map(|role| role_priority(class, role));
        assert_eq!(row(WARRIOR), [3, 0, 2]);
        assert_eq!(row(PALADIN), [2, 3, 2]);
        assert_eq!(row(HUNTER), [0, 0, 3]);
        assert_eq!(row(ROGUE), [0, 0, 3]);
        assert_eq!(row(PRIEST), [0, 3, 1]);
        assert_eq!(row(SHAMAN), [0, 3, 2]);
        assert_eq!(row(MAGE), [0, 0, 3]);
        assert_eq!(row(WARLOCK), [0, 0, 3]);
        assert_eq!(row(DRUID), [2, 3, 2]);
        assert_eq!(row(0), [0, 0, 0], "an unknown class fills nothing");
        assert_eq!(row(6), [0, 0, 0], "class 6 does not exist in 1.12");
    }

    fn open(tank: bool, healer: bool, damage: u8) -> OpenRoles {
        OpenRoles {
            tank,
            healer,
            damage,
        }
    }

    /// The paladin outranks nobody for tank while the warrior waits, so it heals.
    #[test]
    fn a_paladin_then_a_warrior_leave_three_damage_roles_open() {
        use class::*;
        assert_eq!(open_roles(&[PALADIN, WARRIOR]), open(false, false, 3));
    }

    /// Equal priority does not outrank, so the first warrior tanks and the second deals damage.
    #[test]
    fn two_warriors_leave_the_healer_and_two_damage_roles_open() {
        use class::*;
        assert_eq!(open_roles(&[WARRIOR, WARRIOR]), open(false, true, 2));
    }

    #[test]
    fn a_full_party_has_no_open_role() {
        use class::*;
        assert_eq!(
            open_roles(&[WARRIOR, PRIEST, MAGE, ROGUE, HUNTER]),
            open(false, false, 0)
        );
    }

    #[test]
    fn a_member_of_unknown_class_holds_a_seat_and_no_role() {
        use class::*;
        assert_eq!(open_roles(&[]), OpenRoles::ALL);
        assert_eq!(open_roles(&[0, MAGE]), open(true, true, 2));
    }

    /// A fourth damage dealer finds every damage role taken and fills nothing.
    #[test]
    fn a_member_whose_roles_are_taken_fills_nothing() {
        use class::*;
        assert_eq!(open_roles(&[MAGE, MAGE, MAGE, MAGE]), open(true, true, 0));
    }

    fn seeker(character_guid: u64, class: u8, waited_secs: i64) -> MeetingStoneSeeker {
        MeetingStoneSeeker {
            character_guid,
            area_id: 1581,
            team: 469,
            class,
            group_id: 0,
            queued_at: Timestamp::from_micros_since_unix_epoch(
                1_000_000_000_000 - waited_secs * 1_000_000,
            ),
        }
    }

    fn picked(open: OpenRoles, solos: &[MeetingStoneSeeker]) -> Option<u64> {
        next_stone_add(open, solos).map(|index| solos[index].character_guid)
    }

    #[test]
    fn the_next_stone_add_takes_tank_then_healer_then_damage() {
        use class::*;
        let solos = [
            seeker(1, MAGE, 90),
            seeker(2, PRIEST, 10),
            seeker(3, WARRIOR, 5),
        ];
        assert_eq!(picked(OpenRoles::ALL, &solos), Some(3));
        assert_eq!(picked(open(false, true, 3), &solos), Some(2));
        assert_eq!(picked(open(false, false, 3), &solos), Some(1));
        assert_eq!(picked(open(false, false, 0), &solos), None);
    }

    #[test]
    fn the_longest_wait_takes_a_role_and_the_lower_guid_breaks_a_tie() {
        use class::*;
        let solos = [
            seeker(7, PRIEST, 30),
            seeker(5, DRUID, 60),
            seeker(4, SHAMAN, 60),
        ];
        assert_eq!(picked(open(false, true, 0), &solos), Some(4));
        let solos = [
            seeker(7, PRIEST, 61),
            seeker(5, DRUID, 60),
            seeker(4, SHAMAN, 60),
        ];
        assert_eq!(picked(open(false, true, 0), &solos), Some(7));
    }

    #[test]
    fn a_seeker_of_unknown_class_is_never_a_stone_add() {
        assert_eq!(picked(OpenRoles::ALL, &[seeker(1, 0, 600)]), None);
    }

    #[test]
    fn five_solo_seekers_form_a_party_led_by_the_longest_wait() {
        use class::*;
        let waiting = || {
            vec![
                seeker(1, MAGE, 10),
                seeker(2, MAGE, 50),
                seeker(3, MAGE, 40),
                seeker(4, MAGE, 40),
            ]
        };
        assert!(formation(&waiting()).is_none());
        let mut five = waiting();
        five.push(seeker(5, MAGE, 5));
        let (leader, member) = formation(&five).expect("five Seekers form a party");
        assert_eq!((leader.character_guid, member.character_guid), (2, 3));
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

    /// cm:LFG/LFGHandler.cpp:58-77 checks the leader first, then the raid, then the size, so a
    /// member of a full raid hears only that it does not lead.
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
