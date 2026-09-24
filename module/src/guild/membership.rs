//! Membership lifecycle: invite, accept, decline, leave, remove, promote, demote, pass leadership
//! and disband. The Gates and their order are cmangos-classic's (`cm:GuildHandler.cpp`); the cores
//! here are what [`super::realm_guild_op`] dispatches to.

use lyracore_shared::guild::{event_kind, has_right, rights, GuildRefusal, LEADER_RANK};
use spacetimedb::{table, ReducerContext, Table, Timestamp};

use super::{
    add_member, game_guild, game_guild_member, game_guild_rank, lowest_rank, member, push_event,
    GuildAcceptRequest, GuildInviteRequest,
};

/// The Guild Rank id `SetLeader` gives the old Guild Leader: the default Officer rank (`cm:Guild.h:37`).
const OFFICER_RANK: u32 = 1;

/// A pending offer for one Character to join one Guild. At most one per target: a repeated invite
/// to the same target is a Refusal, never a replacement. Reaped on `INVITE_TTL_MICROS`
/// (`module/src/gc.rs`), like `game_group_invite`.
#[table(accessor = game_guild_invite, index(accessor = by_guild, btree(columns = [guild_id])))]
pub struct GuildInvite {
    #[primary_key]
    pub target_guid: u64,
    pub guild_id: u32,
    pub inviter_guid: u64,
    pub created_at: Timestamp,
}

/// `Invite` (`cm:GuildHandler.cpp:66-131`): a member on `actor_guid`'s side offers `request.target_guid`
/// a place in its Guild. An ignored actor gets a silent success and no row, matching mangos'
/// "OK result but not send invite".
pub fn invite(
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

/// `Accept` (`cm:GuildHandler.cpp:192-211`): the actor joins the Guild that invited it, at the
/// lowest Guild Rank. A stale, missing or cross-team invite is silent, exactly like mangos.
pub fn accept(
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

/// `Decline` (`cm:GuildHandler.cpp:214-238`): the actor refuses its pending Guild Invite. The
/// inviter hears about it through SMSG_GUILD_DECLINE, addressed by this Guild Event.
pub fn decline(
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

/// Which of Leave's two outcomes a member gets, pure so the branch is unit-testable without a
/// `ReducerContext`. Mirrors `cm:GuildHandler.cpp:383-404`.
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

/// `Leave` (`cm:GuildHandler.cpp:383-404`): an ordinary member departs; a lone Guild Leader
/// disbands the Guild instead. A Guild Leader with company must pass leadership first.
pub fn leave(ctx: &ReducerContext, actor_guid: u64) -> Result<(), GuildRefusal> {
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
            ctx.db
                .game_guild_member()
                .character_guid()
                .delete(actor_guid);
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

/// Remove's rank Gate (`cm:GuildHandler.cpp:146-177`): the Guild Leader cannot be removed, and the
/// actor can only remove someone below its own Guild Rank.
fn remove_gate(actor_rank: u32, target_rank: u32) -> Result<(), GuildRefusal> {
    if target_rank == LEADER_RANK {
        return Err(GuildRefusal::LeaderCannotLeave);
    }
    if actor_rank >= target_rank {
        return Err(GuildRefusal::RankTooHigh);
    }
    Ok(())
}

/// `Remove` (`cm:GuildHandler.cpp:136-190`): the actor expels `target_guid` from its own Guild.
pub fn remove(ctx: &ReducerContext, actor_guid: u64, target_guid: u64) -> Result<(), GuildRefusal> {
    let actor = member(ctx, actor_guid).ok_or(GuildRefusal::NotInGuild)?;
    let actor_rights = super::rank_rights(ctx, actor.guild_id, actor.rank_id);
    if !has_right(actor_rights, rights::REMOVE) {
        return Err(GuildRefusal::NoPermission);
    }
    let target = member(ctx, target_guid)
        .filter(|row| row.guild_id == actor.guild_id)
        .ok_or(GuildRefusal::TargetNotInGuild)?;
    remove_gate(actor.rank_id, target.rank_id)?;
    ctx.db
        .game_guild_member()
        .character_guid()
        .delete(target_guid);
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

/// Promote's rank Gate (`cm:GuildHandler.cpp:279-311`): the actor can only promote to a Guild Rank
/// below its own plus one. Answers the Guild Rank the promotion lands on.
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

/// `Promote` (`cm:GuildHandler.cpp:269-320`): `target_guid` moves one Guild Rank up (its rank id
/// decreases by one).
pub fn promote(
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
    let target = ctx.db.game_guild_member().character_guid().update(target);
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

/// Demote's rank Gate (`cm:GuildHandler.cpp:332-372`): the actor can only demote someone below its
/// own Guild Rank, and never past the Guild's lowest. Answers the Guild Rank the demotion lands on.
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

/// `Demote` (`cm:GuildHandler.cpp:322-380`): `target_guid` moves one Guild Rank down (its rank id
/// increases by one).
pub fn demote(ctx: &ReducerContext, actor_guid: u64, target_guid: u64) -> Result<(), GuildRefusal> {
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
    let target = ctx.db.game_guild_member().character_guid().update(target);
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

/// `SetLeader` (`cm:GuildHandler.cpp:441-486`): the Guild Leader passes leadership to `target_guid`.
/// Naming itself is a no-op: mangos' own code demotes the leader to Officer here through a shared
/// path (`cm:GuildHandler.cpp:482-483`), which this Gate declines to reproduce.
pub fn set_leader(
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
    ctx.db.game_guild_member().character_guid().update(target);
    let mut old_leader = actor;
    old_leader.rank_id = OFFICER_RANK;
    ctx.db
        .game_guild_member()
        .character_guid()
        .update(old_leader);
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

/// `Disband` (`cm:GuildHandler.cpp:424-435`): the Guild Leader dissolves its own Guild.
pub fn disband(ctx: &ReducerContext, actor_guid: u64) -> Result<(), GuildRefusal> {
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

/// The mechanics both `Disband` and a lone Guild Leader's `Leave` share (`cm:Guild.cpp:695-711`).
/// DISBANDED goes out addressed, one row per member, before any row is deleted: the generic
/// broadcast reads membership when its relay job runs, which is too late once the members are gone.
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
        ctx.db.game_guild_member().character_guid().delete(guid);
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

/// The name of one Guild Rank, empty for an unknown rank (`cm:Guild.cpp:660-666`'s rule, read for
/// names rather than rights).
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
