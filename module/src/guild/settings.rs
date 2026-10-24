use super::{
    game_guild, game_guild_member, game_guild_rank, member, push_event, rank_rights, GuildMember,
    GuildNoteEdit, GuildRank, GuildRankEdit,
};
use lyracore_shared::guild::{
    event_kind, fits_length, has_right, rights, GuildRefusal, LEADER_RANK, MAX_INFO, MAX_MOTD,
    MAX_NOTE, MAX_RANKS, MAX_RANK_NAME, MIN_RANKS,
};
use spacetimedb::{ReducerContext, Table};

pub(super) fn set_motd(
    ctx: &ReducerContext,
    actor_guid: u64,
    text: &str,
) -> Result<(), GuildRefusal> {
    let actor = member(ctx, actor_guid).ok_or(GuildRefusal::NotInGuild)?;
    if !has_right(
        rank_rights(ctx, actor.guild_id, actor.rank_id),
        rights::SETMOTD,
    ) {
        return Err(GuildRefusal::NoPermission);
    }
    if !fits_length(text, MAX_MOTD) {
        return Err(GuildRefusal::TooLong);
    }
    let mut guild = ctx
        .db
        .game_guild()
        .guild_id()
        .find(actor.guild_id)
        .ok_or(GuildRefusal::NotInGuild)?;
    guild.motd = text.to_string();
    ctx.db.game_guild().guild_id().update(guild);
    push_event(
        ctx,
        actor.guild_id,
        0,
        event_kind::MOTD,
        0,
        0,
        vec![text.to_string()],
    );
    Ok(())
}

pub(super) fn set_info(
    ctx: &ReducerContext,
    actor_guid: u64,
    text: &str,
) -> Result<(), GuildRefusal> {
    let actor = member(ctx, actor_guid).ok_or(GuildRefusal::NotInGuild)?;
    if !has_right(
        rank_rights(ctx, actor.guild_id, actor.rank_id),
        rights::MODIFY_GUILD_INFO,
    ) {
        return Err(GuildRefusal::NoPermission);
    }
    if !fits_length(text, MAX_INFO) {
        return Err(GuildRefusal::TooLong);
    }
    let mut guild = ctx
        .db
        .game_guild()
        .guild_id()
        .find(actor.guild_id)
        .ok_or(GuildRefusal::NotInGuild)?;
    guild.info = text.to_string();
    ctx.db.game_guild().guild_id().update(guild);
    Ok(())
}

enum NoteKind {
    Public,
    Officer,
}

impl NoteKind {
    fn right(&self) -> u32 {
        match self {
            Self::Public => rights::EPNOTE,
            Self::Officer => rights::EOFFNOTE,
        }
    }

    fn apply(&self, target: &mut GuildMember, text: &str) {
        match self {
            Self::Public => target.public_note = text.to_string(),
            Self::Officer => target.officer_note = text.to_string(),
        }
    }
}

fn set_note(
    ctx: &ReducerContext,
    actor_guid: u64,
    edit: GuildNoteEdit,
    kind: NoteKind,
) -> Result<(), GuildRefusal> {
    let actor = member(ctx, actor_guid).ok_or(GuildRefusal::NotInGuild)?;
    if !has_right(
        rank_rights(ctx, actor.guild_id, actor.rank_id),
        kind.right(),
    ) {
        return Err(GuildRefusal::NoPermission);
    }
    let mut target = member(ctx, edit.target_guid).ok_or(GuildRefusal::TargetNotInGuild)?;
    if target.guild_id != actor.guild_id {
        return Err(GuildRefusal::TargetNotInGuild);
    }
    if !fits_length(&edit.text, MAX_NOTE) {
        return Err(GuildRefusal::TooLong);
    }
    kind.apply(&mut target, &edit.text);
    target.update(ctx);
    push_event(
        ctx,
        actor.guild_id,
        actor_guid,
        event_kind::ROSTER_TO_ACTOR,
        0,
        0,
        Vec::new(),
    );
    Ok(())
}

pub(super) fn set_public_note(
    ctx: &ReducerContext,
    actor_guid: u64,
    edit: GuildNoteEdit,
) -> Result<(), GuildRefusal> {
    set_note(ctx, actor_guid, edit, NoteKind::Public)
}

pub(super) fn set_officer_note(
    ctx: &ReducerContext,
    actor_guid: u64,
    edit: GuildNoteEdit,
) -> Result<(), GuildRefusal> {
    set_note(ctx, actor_guid, edit, NoteKind::Officer)
}

fn require_leader(
    ctx: &ReducerContext,
    actor_guid: u64,
    guild_id: u32,
) -> Result<(), GuildRefusal> {
    let guild = ctx
        .db
        .game_guild()
        .guild_id()
        .find(guild_id)
        .ok_or(GuildRefusal::NotInGuild)?;
    if guild.leader_guid != actor_guid {
        return Err(GuildRefusal::NotLeader);
    }
    Ok(())
}

pub(super) fn edit_rank(
    ctx: &ReducerContext,
    actor_guid: u64,
    edit: GuildRankEdit,
) -> Result<(), GuildRefusal> {
    let actor = member(ctx, actor_guid).ok_or(GuildRefusal::NotInGuild)?;
    require_leader(ctx, actor_guid, actor.guild_id)?;
    let Some(mut rank) = ctx
        .db
        .game_guild_rank()
        .by_guild()
        .filter(actor.guild_id)
        .find(|rank| rank.rank_id == edit.rank_id)
    else {
        return Ok(());
    };
    if !fits_length(&edit.name, MAX_RANK_NAME) {
        return Err(GuildRefusal::TooLong);
    }
    rank.name = edit.name;
    rank.rights = if edit.rank_id == LEADER_RANK {
        rights::ALL
    } else {
        edit.rights
    };
    ctx.db.game_guild_rank().id().update(rank);
    push_event(
        ctx,
        actor.guild_id,
        0,
        event_kind::ROSTER_REFRESH,
        0,
        0,
        Vec::new(),
    );
    Ok(())
}

pub(super) fn add_rank(
    ctx: &ReducerContext,
    actor_guid: u64,
    name: &str,
) -> Result<(), GuildRefusal> {
    let actor = member(ctx, actor_guid).ok_or(GuildRefusal::NotInGuild)?;
    require_leader(ctx, actor_guid, actor.guild_id)?;
    let existing = super::ranks(ctx, actor.guild_id);
    if existing.len() >= MAX_RANKS {
        return Err(GuildRefusal::RanksAtLimit);
    }
    if !fits_length(name, MAX_RANK_NAME) {
        return Err(GuildRefusal::TooLong);
    }
    let new_rank_id = u32::try_from(existing.len()).unwrap_or(u32::MAX);
    ctx.db.game_guild_rank().insert(GuildRank {
        id: 0,
        guild_id: actor.guild_id,
        rank_id: new_rank_id,
        name: name.to_string(),
        rights: rights::GCHATLISTEN | rights::GCHATSPEAK,
    });
    push_event(
        ctx,
        actor.guild_id,
        0,
        event_kind::ROSTER_REFRESH,
        0,
        0,
        Vec::new(),
    );
    Ok(())
}

pub(super) fn delete_rank(ctx: &ReducerContext, actor_guid: u64) -> Result<(), GuildRefusal> {
    let actor = member(ctx, actor_guid).ok_or(GuildRefusal::NotInGuild)?;
    require_leader(ctx, actor_guid, actor.guild_id)?;
    let existing = super::ranks(ctx, actor.guild_id);
    if existing.len() <= MIN_RANKS {
        return Err(GuildRefusal::RanksAtLimit);
    }
    let lowest = existing
        .last()
        .expect("more than MIN_RANKS ranks were just checked to exist");
    let (deleted_row_id, deleted_rank_id) = (lowest.id, lowest.rank_id);
    let new_lowest_rank_id = deleted_rank_id - 1;
    for mut row in ctx
        .db
        .game_guild_member()
        .by_guild()
        .filter(actor.guild_id)
        .collect::<Vec<_>>()
    {
        if row.rank_id == deleted_rank_id {
            row.rank_id = new_lowest_rank_id;
            row.update(ctx);
        }
    }
    ctx.db.game_guild_rank().id().delete(deleted_row_id);
    push_event(
        ctx,
        actor.guild_id,
        0,
        event_kind::ROSTER_REFRESH,
        0,
        0,
        Vec::new(),
    );
    Ok(())
}
