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

use spacetimedb::{reducer, table, ReducerContext, SpacetimeType, Table, TimeDuration, Timestamp};

use lyracore_shared::constants::go_type;
use lyracore_shared::faction::team_for_race;
use lyracore_shared::group::{GroupKind, GROUP_MAX_MEMBERS};
use lyracore_shared::meeting_stone::{
    encode_queue, event_kind, queue_status, realm_op, MeetingStoneRefusal,
};

/// A queued party is reminded that the queue is still working every five minutes
/// (cm:LFG/LFGMgr.cpp:55).
const REMINDER_INTERVAL_MICROS: i64 = 5 * 60 * 1_000_000;

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
/// or of every member of the party it leads. A missing member's class is 0. LEAVE reads neither.
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
    let team = team_for_race(actor.race);
    let joined = encode_queue(area_id, queue_status::JOINED_QUEUE);
    let Some((group, members)) = membership_of(ctx, actor_guid)? else {
        upsert_seeker(ctx, actor_guid, area_id, team, actor.class, 0);
        crate::group::push_event(ctx, actor_guid, event_kind::QUEUE, 0, joined);
        return Ok(());
    };
    party_join_gate(Membership {
        leads: group.leader_guid == actor_guid,
        kind: crate::group::group_kind_of(&group),
        members: members.len(),
    })?;
    let group_id = group.group_id;
    let next_reminder_at = ctx
        .timestamp
        .checked_add(TimeDuration::from_micros(REMINDER_INTERVAL_MICROS))
        .ok_or_else(|| StoneOpError::Invalid("reminder time overflow".into()))?;
    let row = MeetingStoneParty {
        group_id,
        area_id,
        team,
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
        .filter(|guid| !members.iter().any(|m| m.character_guid == *guid))
        .collect();
    for guid in departed {
        seeker_rows.character_guid().delete(guid);
    }
    let class_of = |guid: u64| {
        seekers
            .iter()
            .find(|facts| facts.character_guid == guid)
            .map_or(0, |facts| facts.class)
    };
    for member in &members {
        let guid = member.character_guid;
        upsert_seeker(ctx, guid, area_id, team, class_of(guid), group_id);
    }
    for member in &members {
        crate::group::push_event(
            ctx,
            member.character_guid,
            event_kind::QUEUE,
            0,
            joined.clone(),
        );
    }
    Ok(())
}

/// Queue `character_guid`, replacing any earlier row and its wait time.
fn upsert_seeker(
    ctx: &ReducerContext,
    character_guid: u64,
    area_id: u32,
    team: u32,
    class: u8,
    group_id: u64,
) {
    let row = MeetingStoneSeeker {
        character_guid,
        area_id,
        team,
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

/// cm:LFG/LFGHandler.cpp:86-110. A solo Seeker leaves with LEAVE_QUEUE. The leader of a queued
/// party takes the whole party out, and every member gets LEAVE_QUEUE. Anyone else in a party only
/// gets NONE. Nobody queued and no party: nothing happens.
fn leave(ctx: &ReducerContext, actor_guid: u64) -> Result<(), StoneOpError> {
    let left = encode_queue(0, queue_status::LEAVE_QUEUE);
    let Some((group, members)) = membership_of(ctx, actor_guid)? else {
        let seekers = ctx.db.game_meeting_stone_seeker();
        if seekers.character_guid().find(actor_guid).is_some() {
            seekers.character_guid().delete(actor_guid);
            crate::group::push_event(ctx, actor_guid, event_kind::QUEUE, 0, left);
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
        dequeue_party_rows(ctx, group.group_id);
        for member in &members {
            crate::group::push_event(
                ctx,
                member.character_guid,
                event_kind::QUEUE,
                0,
                left.clone(),
            );
        }
    } else {
        crate::group::push_event(
            ctx,
            actor_guid,
            event_kind::QUEUE,
            0,
            encode_queue(0, queue_status::NONE),
        );
    }
    Ok(())
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
