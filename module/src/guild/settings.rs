//! MOTD, guild info text, public and officer notes, and Guild Rank edit, add and delete
//! (`cm:GuildHandler.cpp:488-680`). Every op first requires membership; a Refusal leaves every
//! guild row unchanged.

use super::{
    game_guild, game_guild_member, game_guild_rank, member, push_event, rank_rights, GuildMember,
    GuildNoteEdit, GuildRank, GuildRankEdit,
};
use lyracore_shared::guild::{
    event_kind, fits_length, has_right, rights, GuildRefusal, LEADER_RANK, MAX_INFO, MAX_MOTD,
    MAX_NOTE, MAX_RANKS, MAX_RANK_NAME, MIN_RANKS,
};
use spacetimedb::{ReducerContext, Table};

/// CMSG_GUILD_MOTD: the actor's Guild Rank needs SETMOTD. An empty MOTD is allowed
/// (`cm:GuildHandler.cpp:494-497`). Broadcasts MOTD `[text]` (`:511-513`).
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

/// CMSG_GUILD_INFO_TEXT: the actor's Guild Rank needs MODIFY_GUILD_INFO. No event, no reply
/// (`cm:GuildHandler.cpp:693-713`).
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

/// Which note field an edit targets, and the Rank Right that gates it.
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

/// CMSG_GUILD_SET_PUBLIC_NOTE / SET_OFFICER_NOTE, shared: check the editor's right, check the
/// target is a member of the same Guild, check the note length, write the note, then address a
/// fresh roster back to the editor (event `0x50`) so it renders after the commit
/// (`cm:GuildHandler.cpp:526-588`).
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
    ctx.db.game_guild_member().character_guid().update(target);
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

/// The acting Character's Guild, when it is the Guild Leader.
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

/// CMSG_GUILD_RANK: the Guild Leader renames a Guild Rank and sets its rights. An unknown
/// `rank_id` changes nothing and answers success, matching `cm:Guild.cpp:668-688`; rank 0 keeps
/// every right whatever the client sends (`:621-622`). Broadcasts `ROSTER_REFRESH`.
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

/// CMSG_GUILD_ADD_RANK: the Guild Leader appends a new lowest Guild Rank with
/// GCHATLISTEN|GCHATSPEAK, up to [`MAX_RANKS`] (`cm:GuildHandler.cpp:650-657`). Broadcasts
/// `ROSTER_REFRESH`.
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

/// CMSG_GUILD_DEL_RANK: the Guild Leader deletes the lowest Guild Rank, down to [`MIN_RANKS`]
/// (`cm:Guild.cpp:639-650`). Its members move to the new lowest rank in the same transaction, so
/// the Guild Projection relay on `game_guild_member` picks up their new PLAYER_GUILDRANK as an
/// ordinary update; mangos instead leaves them out of range until the client reloads. Broadcasts
/// `ROSTER_REFRESH`.
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
            ctx.db.game_guild_member().character_guid().update(row);
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

// Behavior tests for these reducers (rank add and delete bounds, members moved on delete, rank 0
// rights forced) live in `module/tests/guild_settings.rs`, which runs them as real reducer calls
// against real rows. A pure unit test here could only restate the same constants or expressions
// the reducer already uses, proving nothing about the reducer itself.
