use lyracore_shared::guild::{event_kind, has_right, rights, GuildRefusal, LEADER_RANK};
use spacetimedb::{table, ReducerContext, Table, Timestamp};

use super::{
    add_member, game_guild, game_guild_member, game_guild_rank, lowest_rank, member, push_event,
    GuildAcceptRequest, GuildInviteRequest, GuildMember,
};

const OFFICER_RANK: u32 = 1;

#[table(accessor = game_guild_invite, index(accessor = by_guild, btree(columns = [guild_id])))]
pub struct GuildInvite {
    #[primary_key]
    pub target_guid: u64,
    pub guild_id: u32,
    pub inviter_guid: u64,
    pub created_at: Timestamp,
}

pub(crate) fn invite(
    ctx: &ReducerContext,
    actor_guid: u64,
    request: GuildInviteRequest,
) -> Result<(), GuildRefusal> {
    let actor = member(ctx, actor_guid).ok_or(GuildRefusal::NotInGuild)?;
    if request.target_ignores_actor {
        return Ok(());
    }
    if request.actor_team != request.target_team {
        return Err(GuildRefusal::NotAllied);
    }
    if member(ctx, request.target_guid).is_some() {
        return Err(GuildRefusal::AlreadyInGuild);
    }
    if ctx
        .db
        .game_guild_invite()
        .target_guid()
        .find(request.target_guid)
        .is_some()
    {
        return Err(GuildRefusal::AlreadyInvited);
    }
    let actor_rights = super::rank_rights(ctx, actor.guild_id, actor.rank_id);
    if !has_right(actor_rights, rights::INVITE) {
        return Err(GuildRefusal::NoPermission);
    }
    let guild_name = ctx
        .db
        .game_guild()
        .guild_id()
        .find(actor.guild_id)
        .map_or(String::new(), |guild| guild.name);
    ctx.db.game_guild_invite().insert(GuildInvite {
        target_guid: request.target_guid,
        guild_id: actor.guild_id,
        inviter_guid: actor_guid,
        created_at: ctx.timestamp,
    });
    push_event(
        ctx,
        actor.guild_id,
        request.target_guid,
        event_kind::INVITE,
        0,
        0,
        vec![actor.name, guild_name],
    );
    Ok(())
}

pub(crate) fn accept(
    ctx: &ReducerContext,
    actor_guid: u64,
    actor_account: u64,
    request: GuildAcceptRequest,
) -> Result<(), GuildRefusal> {
    if member(ctx, actor_guid).is_some() {
        return Err(GuildRefusal::NoPendingInvite);
    }
    let invite = ctx
        .db
        .game_guild_invite()
        .target_guid()
        .find(actor_guid)
        .ok_or(GuildRefusal::NoPendingInvite)?;
    let guild = ctx
        .db
        .game_guild()
        .guild_id()
        .find(invite.guild_id)
        .ok_or(GuildRefusal::NoPendingInvite)?;
    if request.actor_team != guild.team {
        return Err(GuildRefusal::NotAllied);
    }
    let rank_id = lowest_rank(ctx, guild.guild_id);
    add_member(
        ctx,
        guild.guild_id,
        actor_guid,
        &request.actor_name,
        rank_id,
        actor_account,
    )?;
    ctx.db.game_guild_invite().target_guid().delete(actor_guid);
    push_event(
        ctx,
        guild.guild_id,
        0,
        event_kind::JOINED,
        actor_guid,
        0,
        vec![request.actor_name],
    );
    Ok(())
}

pub(crate) fn decline(
    ctx: &ReducerContext,
    actor_guid: u64,
    actor_name: &str,
) -> Result<(), GuildRefusal> {
    if member(ctx, actor_guid).is_some() {
        return Err(GuildRefusal::NoPendingInvite);
    }
    let invite = ctx
        .db
        .game_guild_invite()
        .target_guid()
        .find(actor_guid)
        .ok_or(GuildRefusal::NoPendingInvite)?;
    ctx.db.game_guild_invite().target_guid().delete(actor_guid);
    push_event(
        ctx,
        invite.guild_id,
        invite.inviter_guid,
        event_kind::DECLINE,
        0,
        0,
        vec![actor_name.to_string()],
    );
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum LeaveOutcome {
    Disband,
    RemoveSelf,
}

fn leave_outcome(is_leader: bool, member_count: usize) -> Result<LeaveOutcome, GuildRefusal> {
    match (is_leader, member_count) {
        (true, count) if count > 1 => Err(GuildRefusal::LeaderCannotLeave),
        (true, _) => Ok(LeaveOutcome::Disband),
        (false, _) => Ok(LeaveOutcome::RemoveSelf),
    }
}

pub(crate) fn leave(ctx: &ReducerContext, actor_guid: u64) -> Result<(), GuildRefusal> {
    let actor = member(ctx, actor_guid).ok_or(GuildRefusal::NotInGuild)?;
    let guild = ctx
        .db
        .game_guild()
        .guild_id()
        .find(actor.guild_id)
        .ok_or(GuildRefusal::NotInGuild)?;
    let is_leader = guild.leader_guid == actor_guid;
    let member_count = ctx
        .db
        .game_guild_member()
        .by_guild()
        .filter(actor.guild_id)
        .count();
    match leave_outcome(is_leader, member_count)? {
        LeaveOutcome::Disband => disband_guild(ctx, actor.guild_id),
        LeaveOutcome::RemoveSelf => {
            GuildMember::delete(ctx, actor_guid);
            push_event(
                ctx,
                actor.guild_id,
                0,
                event_kind::LEFT,
                actor_guid,
                0,
                vec![actor.name],
            );
        }
    }
    Ok(())
}

fn remove_gate(actor_rank: u32, target_rank: u32) -> Result<(), GuildRefusal> {
    if target_rank == LEADER_RANK {
        return Err(GuildRefusal::LeaderCannotLeave);
    }
    if actor_rank >= target_rank {
        return Err(GuildRefusal::RankTooHigh);
    }
    Ok(())
}

pub(crate) fn remove(
    ctx: &ReducerContext,
    actor_guid: u64,
    target_guid: u64,
) -> Result<(), GuildRefusal> {
    let actor = member(ctx, actor_guid).ok_or(GuildRefusal::NotInGuild)?;
    let actor_rights = super::rank_rights(ctx, actor.guild_id, actor.rank_id);
    if !has_right(actor_rights, rights::REMOVE) {
        return Err(GuildRefusal::NoPermission);
    }
    let target = member(ctx, target_guid)
        .filter(|row| row.guild_id == actor.guild_id)
        .ok_or(GuildRefusal::TargetNotInGuild)?;
    remove_gate(actor.rank_id, target.rank_id)?;
    GuildMember::delete(ctx, target_guid);
    push_event(
        ctx,
        actor.guild_id,
        0,
        event_kind::REMOVED,
        0,
        0,
        vec![target.name, actor.name],
    );
    Ok(())
}

fn promote_target_rank(
    actor_rank: u32,
    target_rank: u32,
    target_is_actor: bool,
) -> Result<u32, GuildRefusal> {
    if target_is_actor {
        return Err(GuildRefusal::TargetIsSelf);
    }
    if actor_rank + 1 >= target_rank {
        return Err(GuildRefusal::RankTooHigh);
    }
    Ok(target_rank - 1)
}

pub(crate) fn promote(
    ctx: &ReducerContext,
    actor_guid: u64,
    target_guid: u64,
) -> Result<(), GuildRefusal> {
    let actor = member(ctx, actor_guid).ok_or(GuildRefusal::NotInGuild)?;
    let actor_rights = super::rank_rights(ctx, actor.guild_id, actor.rank_id);
    if !has_right(actor_rights, rights::PROMOTE) {
        return Err(GuildRefusal::NoPermission);
    }
    let mut target = member(ctx, target_guid)
        .filter(|row| row.guild_id == actor.guild_id)
        .ok_or(GuildRefusal::TargetNotInGuild)?;
    let new_rank = promote_target_rank(actor.rank_id, target.rank_id, target_guid == actor_guid)?;
    target.rank_id = new_rank;
    let target = target.update(ctx);
    let rank_name = rank_name(ctx, actor.guild_id, new_rank);
    push_event(
        ctx,
        actor.guild_id,
        0,
        event_kind::PROMOTION,
        0,
        0,
        vec![actor.name, target.name, rank_name],
    );
    Ok(())
}

fn demote_target_rank(
    actor_rank: u32,
    target_rank: u32,
    lowest: u32,
    target_is_actor: bool,
) -> Result<u32, GuildRefusal> {
    if target_is_actor {
        return Err(GuildRefusal::TargetIsSelf);
    }
    if actor_rank >= target_rank {
        return Err(GuildRefusal::RankTooHigh);
    }
    if target_rank >= lowest {
        return Err(GuildRefusal::RankTooLow);
    }
    Ok(target_rank + 1)
}

pub(crate) fn demote(
    ctx: &ReducerContext,
    actor_guid: u64,
    target_guid: u64,
) -> Result<(), GuildRefusal> {
    let actor = member(ctx, actor_guid).ok_or(GuildRefusal::NotInGuild)?;
    let actor_rights = super::rank_rights(ctx, actor.guild_id, actor.rank_id);
    if !has_right(actor_rights, rights::DEMOTE) {
        return Err(GuildRefusal::NoPermission);
    }
    let mut target = member(ctx, target_guid)
        .filter(|row| row.guild_id == actor.guild_id)
        .ok_or(GuildRefusal::TargetNotInGuild)?;
    let lowest = lowest_rank(ctx, actor.guild_id);
    let new_rank = demote_target_rank(
        actor.rank_id,
        target.rank_id,
        lowest,
        target_guid == actor_guid,
    )?;
    target.rank_id = new_rank;
    let target = target.update(ctx);
    let rank_name = rank_name(ctx, actor.guild_id, new_rank);
    push_event(
        ctx,
        actor.guild_id,
        0,
        event_kind::DEMOTION,
        0,
        0,
        vec![actor.name, target.name, rank_name],
    );
    Ok(())
}

pub(crate) fn set_leader(
    ctx: &ReducerContext,
    actor_guid: u64,
    target_guid: u64,
) -> Result<(), GuildRefusal> {
    let actor = member(ctx, actor_guid).ok_or(GuildRefusal::NotInGuild)?;
    let mut guild = ctx
        .db
        .game_guild()
        .guild_id()
        .find(actor.guild_id)
        .ok_or(GuildRefusal::NotInGuild)?;
    if guild.leader_guid != actor_guid {
        return Err(GuildRefusal::NotLeader);
    }
    if target_guid == actor_guid {
        return Ok(());
    }
    let mut target = member(ctx, target_guid)
        .filter(|row| row.guild_id == actor.guild_id)
        .ok_or(GuildRefusal::TargetNotInGuild)?;
    let guild_id = actor.guild_id;
    let old_leader_name = actor.name.clone();
    let new_leader_name = target.name.clone();
    target.rank_id = LEADER_RANK;
    target.update(ctx);
    let mut old_leader = actor;
    old_leader.rank_id = OFFICER_RANK;
    old_leader.update(ctx);
    guild.leader_guid = target_guid;
    ctx.db.game_guild().guild_id().update(guild);
    push_event(
        ctx,
        guild_id,
        0,
        event_kind::LEADER_CHANGED,
        0,
        0,
        vec![old_leader_name, new_leader_name],
    );
    Ok(())
}

pub(crate) fn disband(ctx: &ReducerContext, actor_guid: u64) -> Result<(), GuildRefusal> {
    let actor = member(ctx, actor_guid).ok_or(GuildRefusal::NotInGuild)?;
    let guild = ctx
        .db
        .game_guild()
        .guild_id()
        .find(actor.guild_id)
        .ok_or(GuildRefusal::NotInGuild)?;
    if guild.leader_guid != actor_guid {
        return Err(GuildRefusal::NotLeader);
    }
    disband_guild(ctx, actor.guild_id);
    Ok(())
}

// Address DISBANDED before deletion; the later broadcast Relay cannot read deleted membership.
pub(super) fn disband_guild(ctx: &ReducerContext, guild_id: u32) {
    let member_guids: Vec<u64> = ctx
        .db
        .game_guild_member()
        .by_guild()
        .filter(guild_id)
        .map(|row| row.character_guid)
        .collect();
    for guid in &member_guids {
        push_event(
            ctx,
            guild_id,
            *guid,
            event_kind::DISBANDED,
            0,
            0,
            Vec::new(),
        );
    }
    for guid in member_guids {
        GuildMember::delete(ctx, guid);
    }
    let rank_ids: Vec<u64> = ctx
        .db
        .game_guild_rank()
        .by_guild()
        .filter(guild_id)
        .map(|row| row.id)
        .collect();
    for id in rank_ids {
        ctx.db.game_guild_rank().id().delete(id);
    }
    let invite_targets: Vec<u64> = ctx
        .db
        .game_guild_invite()
        .by_guild()
        .filter(guild_id)
        .map(|row| row.target_guid)
        .collect();
    for target_guid in invite_targets {
        ctx.db.game_guild_invite().target_guid().delete(target_guid);
    }
    ctx.db.game_guild().guild_id().delete(guild_id);
}

fn rank_name(ctx: &ReducerContext, guild_id: u32, rank_id: u32) -> String {
    ctx.db
        .game_guild_rank()
        .by_guild()
        .filter(guild_id)
        .find(|rank| rank.rank_id == rank_id)
        .map_or(String::new(), |rank| rank.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promote_lands_one_rank_above_and_refuses_past_the_actors_reach() {
        assert_eq!(promote_target_rank(1, 3, false), Ok(2));
        assert_eq!(
            promote_target_rank(1, 2, false),
            Err(GuildRefusal::RankTooHigh)
        );
        assert_eq!(
            promote_target_rank(0, 1, false),
            Err(GuildRefusal::RankTooHigh)
        );
        assert_eq!(
            promote_target_rank(1, 3, true),
            Err(GuildRefusal::TargetIsSelf)
        );
    }

    #[test]
    fn demote_lands_one_rank_below_and_refuses_past_the_lowest() {
        assert_eq!(demote_target_rank(1, 3, 4, false), Ok(4));
        assert_eq!(
            demote_target_rank(1, 4, 4, false),
            Err(GuildRefusal::RankTooLow)
        );
        assert_eq!(
            demote_target_rank(3, 2, 4, false),
            Err(GuildRefusal::RankTooHigh)
        );
        assert_eq!(
            demote_target_rank(1, 3, 4, true),
            Err(GuildRefusal::TargetIsSelf)
        );
    }

    #[test]
    fn remove_refuses_the_leader_and_a_target_at_or_above_the_actor() {
        assert_eq!(remove_gate(2, 3), Ok(()));
        assert_eq!(remove_gate(2, 0), Err(GuildRefusal::LeaderCannotLeave));
        assert_eq!(remove_gate(2, 2), Err(GuildRefusal::RankTooHigh));
        assert_eq!(remove_gate(3, 2), Err(GuildRefusal::RankTooHigh));
    }

    #[test]
    fn leave_disbands_only_a_lone_leader() {
        assert_eq!(leave_outcome(true, 1), Ok(LeaveOutcome::Disband));
        assert_eq!(leave_outcome(true, 2), Err(GuildRefusal::LeaderCannotLeave));
        assert_eq!(leave_outcome(false, 5), Ok(LeaveOutcome::RemoveSelf));
    }
}
