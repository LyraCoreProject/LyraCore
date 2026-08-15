//! Guild system: the durable guild, its ten ranks, its roster, pending invites and the
//! per-recipient event relay.
//!
//! Shape mirrors `group`: the GATEWAY owns protocol and resolves typed names to guids, and the
//! reducers here own the rules. Guild state is authoritative on **realm-core** — nothing about a
//! guild is coupled to space — so the gateway drives it through the operator-gated
//! [`realm_guild_op`], and a single-database deployment calls [`create_guild`] on its one database
//! instead. Both run the same cores, so the two planes cannot drift.
//!
//! **No roster mirror.** Party needed one because ~fifty in-world reads resolve membership on the
//! hot path; guild has none. What a world shard needs is the character's OWN guild id and rank, and
//! those ride two scalar columns on `game_character`, stamped here when the membership changes on
//! this database and pushed by the gateway ([`sync_guild_membership`]) when it changed on another.
//!
//! Rank permissions are out of scope: `GuildRank.rights` is written with the vanilla defaults and
//! never consulted. The only authority check in the system is "are you the guild master".

use spacetimedb::{reducer, table, Identity, ReducerContext, Table, Timestamp};

use crate::game_character;

// The rank seed, the name rules, the event kinds, the realm-op tags and the classified error
// strings are the SHARED wire contract: `lyracore_shared::guild` is the one definition both crates
// import, so a reword or a renumber is a cross-crate compile error rather than a runtime drift.
use lyracore_shared::guild::{
    err as guild_err, event_kind, valid_guild_name, DEFAULT_RANK_NAMES, DEFAULT_RANK_RIGHTS,
    GUILD_JOIN_RANK, GUILD_MASTER_RANK,
};

/// A guild. `name` is unique realm-wide, which one index buys because realm-core is one database.
/// Public + no RLS (a guild's name and MOTD are world-visible, like `game_group`). [entity]
#[table(accessor = game_guild, public)]
pub struct Guild {
    #[primary_key]
    #[auto_inc]
    pub guild_id: u64,
    #[unique]
    pub name: String,
    pub master_guid: u64,
    pub motd: String,
    pub info_text: String,
    pub created_at: Timestamp,
}

/// One member row per character in a guild. `character_guid` is unique across the table — a
/// character is in at most one guild — enforced by [`guild_of`] at every write, the same way
/// `game_group_member` enforces its own. [entity]
#[table(
    accessor = game_guild_member,
    public,
    index(accessor = by_guild, btree(columns = [guild_id])),
    index(accessor = by_character, btree(columns = [character_guid]))
)]
pub struct GuildMember {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub guild_id: u64,
    pub character_guid: u64,
    pub rank_index: u32,
    pub public_note: String,
    pub officer_note: String,
    pub joined_at: Timestamp,
}

// Character-owned sweep: a deleted character leaves its guild through the same path a voluntary
// leave uses, never a bare row delete, so master succession and the last-member disband both run.
crate::character_owned!(delete, fn sweep_delete_game_guild_member(ctx, character_guid) {
    remove_member(ctx, character_guid);
});
// CROSS-DATABASE transport: membership is REALM-CORE state, so carrying it in the export blob would
// race the authority — the blob could only carry what the character had when it stepped into the
// portal. The gateway re-pushes the authority's answer at world entry instead.
crate::character_owned!(not_transported, fn sweep_transfer_game_guild_member());

/// One rank row. **Exactly [`GUILD_RANK_COUNT`] per guild, always** — seeded at creation and never
/// added to or removed from in this slice, because `SMSG_GUILD_QUERY_RESPONSE.rank_names` is a
/// fixed `[String; 10]` and a short guild cannot be rendered. `rights` carries the vanilla defaults
/// and is never read server-side. [entity]
#[table(accessor = game_guild_rank, public, index(accessor = by_guild, btree(columns = [guild_id])))]
pub struct GuildRank {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub guild_id: u64,
    pub rank_index: u32,
    pub name: String,
    pub rights: u32,
}

/// A pending guild invite: at most one per target (a newer invite replaces it). Consumed by
/// accept/decline; a never-answered invite is reaped on the shared 2-minute invite TTL
/// (`gc.rs`), long enough to answer the dialog and short enough not to leak. Private — only the
/// module reads it. [entity]
#[table(accessor = game_guild_invite, index(accessor = by_target, btree(columns = [target_guid])))]
pub struct GuildInvite {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub target_guid: u64,
    pub inviter_guid: u64,
    pub guild_id: u64,
    pub created_at: Timestamp,
}

// Character-owned sweep: a deleted character's pending invites go with it — rows where it is the
// TARGET (indexed) and rows where it is the INVITER (a scan; the table only ever holds currently
// pending invites, so it stays tiny). Delete-only: `GuildInvite` carries no owner identity.
crate::character_owned!(delete, fn sweep_delete_game_guild_invite(ctx, character_guid) {
    let invites = ctx.db.game_guild_invite();
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
// CROSS-DATABASE transport: a pending invite is a 2-minute dialog whose inviter is by definition
// not transferring too. It dies with the source copy, exactly as a decline would.
crate::character_owned!(not_transported, fn sweep_transfer_game_guild_invite());

/// A per-recipient guild notification — the `game_group_event` shape on the guild's own table:
/// public, scoped to the recipient, reaped by the shared event GC. `other_name` is resolved at
/// write time so the gateway never needs a name lookup, and `recipient_guid` is what the realm-core
/// relay filters on (an identity is minted per (account, database), so it names nobody on the
/// directory database). [event]
#[table(accessor = game_guild_event, public, index(accessor = by_recipient, btree(columns = [recipient_identity])))]
pub struct GuildEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub recipient_identity: Identity,
    pub recipient_guid: u64,
    pub kind: u8, // lyracore_shared::guild::event_kind::*
    pub other_guid: u64,
    pub other_name: String,
    pub payload: String,
    pub created_at: Timestamp,
}

// Character-owned sweep: an undelivered notification for a character that no longer exists is dead
// weight the event GC would otherwise carry to its TTL.
crate::character_owned!(delete, fn sweep_delete_game_guild_event(ctx, character_guid) {
    let events = ctx.db.game_guild_event();
    let stale: Vec<u64> = events
        .iter()
        .filter(|e| e.recipient_guid == character_guid)
        .map(|e| e.id)
        .collect();
    for id in stale {
        events.id().delete(id);
    }
});
// CROSS-DATABASE transport: a one-shot relay row with a GC TTL whose durable half is the membership
// on realm-core. Carrying it would replay a stale notification at the destination.
crate::character_owned!(not_transported, fn sweep_transfer_game_guild_event());

// ===========================================================================================
//  Reads
// ===========================================================================================

/// The guild membership row for `character_guid`, if any.
pub(crate) fn guild_of(ctx: &ReducerContext, character_guid: u64) -> Option<GuildMember> {
    ctx.db
        .game_guild_member()
        .by_character()
        .filter(&character_guid)
        .next()
}

/// Every member row of `guild_id`, in join order (member-row id) — the order master succession
/// follows and the order the roster renders in.
pub(crate) fn members_of(ctx: &ReducerContext, guild_id: u64) -> Vec<GuildMember> {
    let mut members: Vec<GuildMember> = ctx
        .db
        .game_guild_member()
        .by_guild()
        .filter(&guild_id)
        .collect();
    members.sort_by_key(|m| m.id);
    members
}

/// The pending invite addressed to `target_guid`, if the dialog is still open. At most one exists:
/// the invite gate refuses a second, and the GC reaps an unanswered one on the shared invite TTL.
pub(crate) fn pending_invite(ctx: &ReducerContext, target_guid: u64) -> Option<GuildInvite> {
    ctx.db
        .game_guild_invite()
        .by_target()
        .filter(&target_guid)
        .next()
}

// ===========================================================================================
//  Writes
// ===========================================================================================

/// Stamp a character's own guild id and rank onto its `game_character` row.
///
/// The whole of the world-shard's guild state (there is no roster mirror). A character row that is
/// not on THIS database is not an error: on realm-core there are none at all, and the gateway
/// pushes the same pair onto the player's own shard through [`sync_guild_membership`].
///
/// Read through the by-guid transfer chokepoint, so a mid-transfer character reads as ABSENT and
/// this is a no-op rather than a write to a source copy the destination has already serialized
/// past. The gateway re-pushes at world entry, which is what makes the refusal free.
fn set_character_guild(ctx: &ReducerContext, character_guid: u64, guild_id: u64, rank: u32) {
    let Some(mut c) = crate::helpers::character_by_guid(ctx, character_guid) else {
        return;
    };
    if c.guild_id == guild_id && c.guild_rank == rank {
        return;
    }
    c.guild_id = guild_id;
    c.guild_rank = rank;
    ctx.db.game_character().guid().update(c);
}

/// The identity-free create core, shared by both planes: validate the name, refuse a founder who is
/// already in a guild, refuse a name the realm already has, then write the guild, its ten ranks and
/// the founder's member row in one transaction.
pub(crate) fn create_guild_for(
    ctx: &ReducerContext,
    founder_guid: u64,
    name: &str,
) -> Result<u64, String> {
    if !valid_guild_name(name) {
        return Err(guild_err::NAME_INVALID.to_string());
    }
    if guild_of(ctx, founder_guid).is_some() {
        return Err(guild_err::ALREADY_IN_GUILD.to_string());
    }
    if ctx.db.game_guild().name().find(name.to_string()).is_some() {
        return Err(guild_err::NAME_TAKEN.to_string());
    }
    let guild = ctx.db.game_guild().insert(Guild {
        guild_id: 0,
        name: name.to_string(),
        master_guid: founder_guid,
        motd: String::new(),
        info_text: String::new(),
        created_at: ctx.timestamp,
    });
    seed_ranks(ctx, guild.guild_id);
    ctx.db.game_guild_member().insert(GuildMember {
        id: 0,
        guild_id: guild.guild_id,
        character_guid: founder_guid,
        rank_index: GUILD_MASTER_RANK,
        public_note: String::new(),
        officer_note: String::new(),
        joined_at: ctx.timestamp,
    });
    set_character_guild(ctx, founder_guid, guild.guild_id, GUILD_MASTER_RANK);
    Ok(guild.guild_id)
}

/// Write a fresh guild's rank rows: exactly [`GUILD_RANK_COUNT`] of them, in rank order, from the
/// shared vanilla seed. The count is the invariant `SMSG_GUILD_QUERY_RESPONSE` renders from.
fn seed_ranks(ctx: &ReducerContext, guild_id: u64) {
    let ranks = ctx.db.game_guild_rank();
    for (index, (name, rights)) in DEFAULT_RANK_NAMES
        .iter()
        .zip(DEFAULT_RANK_RIGHTS.iter())
        .enumerate()
    {
        ranks.insert(GuildRank {
            id: 0,
            guild_id,
            rank_index: index as u32,
            name: (*name).to_string(),
            rights: *rights,
        });
    }
}

/// The single membership-removal core (character delete today; leave, kick and disband when those
/// land). Idempotent for a guid in no guild. Disbands the guild when its last member goes, so a
/// deleted founder never leaves an ownerless guild holding its name.
pub(crate) fn remove_member(ctx: &ReducerContext, character_guid: u64) {
    let Some(m) = guild_of(ctx, character_guid) else {
        return;
    };
    let guild_id = m.guild_id;
    ctx.db.game_guild_member().id().delete(m.id);
    set_character_guild(ctx, character_guid, 0, 0);

    let remaining = members_of(ctx, guild_id);
    if remaining.is_empty() {
        disband(ctx, guild_id);
        return;
    }
    // The master left: the longest-standing remaining member (lowest member-row id) succeeds.
    let Some(mut guild) = ctx.db.game_guild().guild_id().find(guild_id) else {
        return;
    };
    if guild.master_guid == character_guid {
        let heir = remaining[0].character_guid;
        guild.master_guid = heir;
        ctx.db.game_guild().guild_id().update(guild);
        promote_to_master(ctx, heir);
    }
}

/// Move `character_guid` to rank 0 on both the member row and its character row.
fn promote_to_master(ctx: &ReducerContext, character_guid: u64) {
    let Some(mut m) = guild_of(ctx, character_guid) else {
        return;
    };
    m.rank_index = GUILD_MASTER_RANK;
    let guild_id = m.guild_id;
    ctx.db.game_guild_member().id().update(m);
    set_character_guild(ctx, character_guid, guild_id, GUILD_MASTER_RANK);
}

/// Drop a guild and every row that belongs to it: the ranks, any pending invite, and the guild row.
/// Member rows are the caller's business — the last one is what triggers this.
fn disband(ctx: &ReducerContext, guild_id: u64) {
    let ranks = ctx.db.game_guild_rank();
    for r in ranks.by_guild().filter(&guild_id).collect::<Vec<_>>() {
        ranks.id().delete(r.id);
    }
    let invites = ctx.db.game_guild_invite();
    let pending: Vec<u64> = invites
        .iter()
        .filter(|i| i.guild_id == guild_id)
        .map(|i| i.id)
        .collect();
    for id in pending {
        invites.id().delete(id);
    }
    ctx.db.game_guild().guild_id().delete(guild_id);
}

/// Write one guild notification for one recipient — the `game_group_event` relay shape on the
/// guild's own table.
///
/// `other_name` is a PARAMETER, unlike `group::push_event`'s, which looks it up. Realm-core holds
/// no `game_character` rows, so a name it did not receive is a name it cannot know; every caller
/// gets it from the gateway through `realm_guild_op`'s `text` slot. `recipient_identity` still
/// drives the per-player RLS on a world shard and is `Identity::ZERO` where no row binds one.
/// Set a guild's MOTD or its info text — D3's only permission check (guild master), and an empty
/// `text` is a valid write that CLEARS the field rather than a refusal.
///
/// A MOTD change also drops one [`GuildEvent`] (kind
/// [`lyracore_shared::guild::event_kind::MOTD`]) per current member — the durable half of "every
/// online member sees the new MOTD live"; delivery is the gateway relay's job, the same
/// `game_group_event` shape [`push_event`] mirrors. Info text carries no broadcast: T6's own scope
/// keeps it to gating and storage.
fn set_guild_text(
    ctx: &ReducerContext,
    actor_guid: u64,
    text: &str,
    motd: bool,
) -> Result<(), String> {
    let member = guild_of(ctx, actor_guid).ok_or_else(|| guild_err::NOT_IN_GUILD.to_string())?;
    let mut guild = ctx
        .db
        .game_guild()
        .guild_id()
        .find(member.guild_id)
        .ok_or_else(|| guild_err::NOT_IN_GUILD.to_string())?;
    if guild.master_guid != actor_guid {
        return Err(guild_err::NOT_GUILD_MASTER.to_string());
    }
    if motd {
        guild.motd = text.to_string();
    } else {
        guild.info_text = text.to_string();
    }
    let guild_id = guild.guild_id;
    ctx.db.game_guild().guild_id().update(guild);
    if motd {
        for m in members_of(ctx, guild_id) {
            push_event(
                ctx,
                m.character_guid,
                lyracore_shared::guild::event_kind::MOTD,
                actor_guid,
                "",
                text.to_string(),
            );
        }
    }
    Ok(())
}

/// Set a member's public or officer note. The public note is the one thing a plain member may write
/// about themselves — their OWN row only; every other case (another member's public note, anyone's
/// officer note) is master-only (D3). `target_guid` must be a fellow member of the actor's own
/// guild — reusing [`lyracore_shared::guild::err::NOT_IN_GUILD`] for "no such member here" keeps one
/// refusal vocabulary instead of inventing a second.
fn set_member_note(
    ctx: &ReducerContext,
    actor_guid: u64,
    target_guid: u64,
    note: &str,
    officer: bool,
) -> Result<(), String> {
    let actor_member =
        guild_of(ctx, actor_guid).ok_or_else(|| guild_err::NOT_IN_GUILD.to_string())?;
    let guild = ctx
        .db
        .game_guild()
        .guild_id()
        .find(actor_member.guild_id)
        .ok_or_else(|| guild_err::NOT_IN_GUILD.to_string())?;
    let is_master = guild.master_guid == actor_guid;
    let setting_own_public_note = !officer && actor_guid == target_guid;
    if !is_master && !setting_own_public_note {
        return Err(guild_err::NOT_GUILD_MASTER.to_string());
    }
    let Some(mut target) = guild_of(ctx, target_guid) else {
        return Err(guild_err::NOT_IN_GUILD.to_string());
    };
    if target.guild_id != guild.guild_id {
        return Err(guild_err::NOT_IN_GUILD.to_string());
    }
    if officer {
        target.officer_note = note.to_string();
    } else {
        target.public_note = note.to_string();
    }
    ctx.db.game_guild_member().id().update(target);
    Ok(())
}

/// Write one guild notification for one recipient — the `group::push_event` shape on the guild's
/// own table.
///
/// `other_name` is a PARAMETER, unlike `group::push_event`'s, which looks it up. Realm-core holds
/// no `game_character` rows, so a name it did not receive is a name it cannot know; every caller
/// gets it from the gateway through `realm_guild_op`'s `text` slot. `recipient_identity` still
/// drives the per-player RLS on a world shard and is `Identity::ZERO` where no row binds one.
fn push_event(
    ctx: &ReducerContext,
    recipient_guid: u64,
    kind: u8,
    other_guid: u64,
    other_name: &str,
    payload: String,
) {
    let bound = crate::helpers::character_by_guid(ctx, recipient_guid).map(|c| c.owner_identity);
    ctx.db.game_guild_event().insert(GuildEvent {
        id: 0,
        recipient_identity: crate::helpers::event_recipient_identity(bound),
        recipient_guid,
        kind,
        other_guid,
        other_name: other_name.to_string(),
        payload,
        created_at: ctx.timestamp,
    });
}

/// The guild `actor_guid` is the MASTER of. The only permission check in the guild system: rank
/// rights are written at creation and never consulted.
fn master_guild_of(ctx: &ReducerContext, actor_guid: u64) -> Result<u64, String> {
    let member = guild_of(ctx, actor_guid).ok_or(guild_err::NOT_IN_GUILD)?;
    let guild = ctx
        .db
        .game_guild()
        .guild_id()
        .find(member.guild_id)
        .ok_or(guild_err::NOT_IN_GUILD)?;
    if guild.master_guid != actor_guid {
        return Err(guild_err::NOT_GUILD_MASTER.to_string());
    }
    Ok(member.guild_id)
}

/// Move `character_guid` to `rank` on both the member row and its character row.
fn set_member_rank(ctx: &ReducerContext, character_guid: u64, rank: u32) {
    let Some(mut m) = guild_of(ctx, character_guid) else {
        return;
    };
    m.rank_index = rank;
    let guild_id = m.guild_id;
    ctx.db.game_guild_member().id().update(m);
    set_character_guild(ctx, character_guid, guild_id, rank);
}

/// `CMSG_GUILD_LEAVE` — the caller removes themselves.
///
/// **Succession is explicit.** A master with other members left behind is REFUSED: they must hand
/// the guild on with [`set_guild_master`] or destroy it with [`disband_guild`]. Promoting somebody
/// silently is a guild-politics call the realm has no business making. A master who is the LAST
/// member may leave, and [`remove_member`] disbands the guild as the last row goes.
pub(crate) fn leave_guild(
    ctx: &ReducerContext,
    actor_guid: u64,
    actor_name: &str,
) -> Result<(), String> {
    let member = guild_of(ctx, actor_guid).ok_or(guild_err::NOT_IN_GUILD)?;
    let guild_id = member.guild_id;
    let guild = ctx
        .db
        .game_guild()
        .guild_id()
        .find(guild_id)
        .ok_or(guild_err::NOT_IN_GUILD)?;
    let members = members_of(ctx, guild_id);
    if guild.master_guid == actor_guid && members.len() > 1 {
        return Err(guild_err::MASTER_MUST_TRANSFER_OR_DISBAND.to_string());
    }
    // Notified before the row goes, so the roster the notice fans out to is the one that still has
    // the leaver in it — after `remove_member` a last-member leave has no guild left to read.
    for other in members.iter().filter(|o| o.character_guid != actor_guid) {
        push_event(
            ctx,
            other.character_guid,
            event_kind::LEFT,
            actor_guid,
            actor_name,
            String::new(),
        );
    }
    remove_member(ctx, actor_guid);
    Ok(())
}

/// `CMSG_GUILD_REMOVE` — the guild master removes another member.
///
/// A master removing THEMSELVES is refused: that is a leave or a disband, and routing it here would
/// walk straight into `remove_member`'s succession path, promoting somebody by accident.
pub(crate) fn remove_from_guild(
    ctx: &ReducerContext,
    actor_guid: u64,
    target_guid: u64,
    target_name: &str,
) -> Result<(), String> {
    let guild_id = master_guild_of(ctx, actor_guid)?;
    if target_guid == actor_guid {
        return Err(guild_err::CANNOT_REMOVE_SELF.to_string());
    }
    if guild_of(ctx, target_guid).map(|m| m.guild_id) != Some(guild_id) {
        return Err(guild_err::TARGET_NOT_IN_GUILD.to_string());
    }
    for other in members_of(ctx, guild_id)
        .iter()
        .filter(|o| o.character_guid != target_guid)
    {
        push_event(
            ctx,
            other.character_guid,
            event_kind::REMOVED,
            target_guid,
            target_name,
            String::new(),
        );
    }
    remove_member(ctx, target_guid);
    Ok(())
}

/// `CMSG_GUILD_DISBAND` — the guild master destroys the guild.
///
/// The cascade has to leave ZERO rows behind. `GuildMember.character_guid` is unique across the
/// table (enforced by [`guild_of`], not by a constraint), so one orphaned member row is a character
/// that can never join a guild again. Members go first — each one's own guild columns zeroed with
/// it — and [`disband`] then takes the ranks, the pending invites and the guild row.
pub(crate) fn disband_guild(ctx: &ReducerContext, actor_guid: u64) -> Result<(), String> {
    let guild_id = master_guild_of(ctx, actor_guid)?;
    for member in members_of(ctx, guild_id) {
        push_event(
            ctx,
            member.character_guid,
            event_kind::DISBANDED,
            actor_guid,
            "",
            String::new(),
        );
        ctx.db.game_guild_member().id().delete(member.id);
        set_character_guild(ctx, member.character_guid, 0, 0);
    }
    disband(ctx, guild_id);
    Ok(())
}

/// `CMSG_GUILD_LEADER` — the guild master hands the guild to another member.
///
/// The new master takes rank 0 and the old one drops to the second rank, which is what vanilla
/// does: a former master keeps officer standing rather than being demoted to the bottom.
pub(crate) fn set_guild_master(
    ctx: &ReducerContext,
    actor_guid: u64,
    target_guid: u64,
    target_name: &str,
) -> Result<(), String> {
    let guild_id = master_guild_of(ctx, actor_guid)?;
    if target_guid == actor_guid {
        return Ok(()); // handing the guild to yourself changes nothing
    }
    if guild_of(ctx, target_guid).map(|m| m.guild_id) != Some(guild_id) {
        return Err(guild_err::TARGET_NOT_IN_GUILD.to_string());
    }
    let Some(mut guild) = ctx.db.game_guild().guild_id().find(guild_id) else {
        return Err(guild_err::NOT_IN_GUILD.to_string());
    };
    guild.master_guid = target_guid;
    ctx.db.game_guild().guild_id().update(guild);
    promote_to_master(ctx, target_guid);
    set_member_rank(ctx, actor_guid, GUILD_MASTER_RANK + 1);
    for member in members_of(ctx, guild_id) {
        push_event(
            ctx,
            member.character_guid,
            event_kind::LEADER_CHANGED,
            target_guid,
            target_name,
            String::new(),
        );
    }
    Ok(())
}

/// The invite core: gate, write one pending invite, and notify the target.
///
/// The two gates realm-core CAN answer are here; the two it cannot — does the target exist, and is
/// anybody at its keyboard — are the gateway's, because only the gateway sees every shard's
/// characters. `inviter_name` rides in for the same reason: the notification carries it.
pub(crate) fn invite_to_guild(
    ctx: &ReducerContext,
    inviter_guid: u64,
    target_guid: u64,
    inviter_name: &str,
) -> Result<(), String> {
    let membership = guild_of(ctx, inviter_guid).ok_or(guild_err::NOT_IN_GUILD)?;
    let guild = ctx
        .db
        .game_guild()
        .guild_id()
        .find(membership.guild_id)
        .ok_or(guild_err::NOT_IN_GUILD)?;
    if guild.master_guid != inviter_guid {
        return Err(guild_err::NOT_GUILD_MASTER.to_string());
    }
    if guild_of(ctx, target_guid).is_some() {
        return Err(guild_err::TARGET_IN_GUILD.to_string());
    }
    if pending_invite(ctx, target_guid).is_some() {
        return Err(guild_err::ALREADY_INVITED.to_string());
    }
    ctx.db.game_guild_invite().insert(GuildInvite {
        id: 0,
        target_guid,
        inviter_guid,
        guild_id: guild.guild_id,
        created_at: ctx.timestamp,
    });
    push_event(
        ctx,
        target_guid,
        event_kind::INVITE,
        inviter_guid,
        inviter_name,
        guild.name,
    );
    Ok(())
}

/// The answer core: consume the actor's pending invite, then either seat them or tell the inviter.
///
/// Both arms delete the invite row FIRST, so an invite is answerable exactly once however the
/// answer goes. A refusal past that point rolls the whole transaction back — including the delete —
/// which is what keeps a join that could not be seated from silently eating the dialog.
pub(crate) fn answer_invite(
    ctx: &ReducerContext,
    actor_guid: u64,
    accept: bool,
    actor_name: &str,
) -> Result<(), String> {
    let invite = pending_invite(ctx, actor_guid).ok_or(guild_err::NO_PENDING_INVITE)?;
    ctx.db.game_guild_invite().id().delete(invite.id);
    if !accept {
        push_event(
            ctx,
            invite.inviter_guid,
            event_kind::DECLINED,
            actor_guid,
            actor_name,
            String::new(),
        );
        return Ok(());
    }
    // The dialog outlives neither of these: a guild can be disbanded, and the invitee can accept
    // somebody else's invite, while the popup sits on screen.
    if ctx
        .db
        .game_guild()
        .guild_id()
        .find(invite.guild_id)
        .is_none()
    {
        return Err(guild_err::NOT_IN_GUILD.to_string());
    }
    if guild_of(ctx, actor_guid).is_some() {
        return Err(guild_err::ALREADY_IN_GUILD.to_string());
    }
    ctx.db.game_guild_member().insert(GuildMember {
        id: 0,
        guild_id: invite.guild_id,
        character_guid: actor_guid,
        rank_index: GUILD_JOIN_RANK,
        public_note: String::new(),
        officer_note: String::new(),
        joined_at: ctx.timestamp,
    });
    set_character_guild(ctx, actor_guid, invite.guild_id, GUILD_JOIN_RANK);
    // Every member hears it, the new one included: their own client opens the guild panel off the
    // back of this, and the roster read that follows is the same one everyone else's client makes.
    for member in members_of(ctx, invite.guild_id) {
        push_event(
            ctx,
            member.character_guid,
            event_kind::JOINED,
            actor_guid,
            actor_name,
            String::new(),
        );
    }
    Ok(())
}

/// Tell the REST of `actor_guid`'s guild that they signed on or off. A guildless character has
/// nobody to tell, which is a no-op rather than a refusal — the gateway fires this on every world
/// entry and every logout.
pub(crate) fn broadcast_presence(
    ctx: &ReducerContext,
    actor_guid: u64,
    online: bool,
    actor_name: &str,
) {
    let Some(membership) = guild_of(ctx, actor_guid) else {
        return;
    };
    let kind = if online {
        event_kind::SIGNED_ON
    } else {
        event_kind::SIGNED_OFF
    };
    for member in members_of(ctx, membership.guild_id) {
        if member.character_guid == actor_guid {
            continue; // nobody needs to be told they logged in
        }
        push_event(
            ctx,
            member.character_guid,
            kind,
            actor_guid,
            actor_name,
            String::new(),
        );
    }
}

// ===========================================================================================
//  Chat
// ===========================================================================================

/// The guild-chat core (`/g`), driven by [`realm_guild_op`]'s `GUILD_CHAT` arm: deliver `text` to
/// every OTHER member of `sender_guid`'s guild, plus an echo to the sender — vanilla server-echoes
/// the speaker's own guild line back, exactly as party does (`chat::apply_party_chat`). Refuses a
/// caller who is not in a guild with the shared [`guild_err::NOT_IN_GUILD`] string, which the
/// gateway maps to `SMSG_GUILD_COMMAND_RESULT(GuildPlayerNotInGuild)`.
///
/// Every current member gets a row regardless of whether they are online — the SAME construction
/// `chat::apply_party_chat` uses against `game_group_event`. What makes delivery "online members
/// only" (D4) is the per-recipient RLS relay itself: an offline member's session holds no
/// subscription to receive the row, and the shared 1-second event TTL leaves nothing to replay
/// when they next log in.
pub(crate) fn guild_chat_for(
    ctx: &ReducerContext,
    sender_guid: u64,
    text: &str,
) -> Result<(), String> {
    let message =
        crate::chat::normalized_message(text).ok_or_else(|| "empty message".to_string())?;
    let membership =
        guild_of(ctx, sender_guid).ok_or_else(|| guild_err::NOT_IN_GUILD.to_string())?;
    let payload = lyracore_shared::guild::encode_guild_chat(&message);
    let member_guids: Vec<u64> = members_of(ctx, membership.guild_id)
        .into_iter()
        .map(|m| m.character_guid)
        .collect();
    for other in guild_chat_other_recipients(sender_guid, &member_guids) {
        push_guild_chat_event(ctx, other, sender_guid, payload.clone());
    }
    // The echo to the sender (vanilla server-echoes guild lines back to the speaker's own client)
    // — deliberately a SEPARATE push outside the loop above (which excludes the sender by design),
    // not folded into "every member incl. self", for the same reason `apply_party_chat` keeps its
    // own echo separate: the pure recipient-set helper below only ever has to answer "who ELSE".
    push_guild_chat_event(ctx, sender_guid, sender_guid, payload);
    Ok(())
}

/// The OTHER guild members who get [`guild_chat_for`]'s per-recipient event row (every member of
/// `members` except `sender_guid`) — the sender gets a SEPARATE explicit echo row, so this
/// deliberately excludes them. Mirrors `chat::party_chat_other_recipients`. Pure — unit-tested
/// without a `ReducerContext`.
pub(crate) fn guild_chat_other_recipients(sender_guid: u64, members: &[u64]) -> Vec<u64> {
    members
        .iter()
        .copied()
        .filter(|&g| g != sender_guid)
        .collect()
}

/// Insert one `GUILD_CHAT` row addressed to `recipient_guid`.
///
/// `other_name` is left empty on purpose: `SMSG_MESSAGECHAT` with `ChatType::Guild` carries only
/// the speaker's guid, and the client resolves the name itself through `CMSG_NAME_QUERY`. Realm-core
/// could not answer it anyway.
fn push_guild_chat_event(
    ctx: &ReducerContext,
    recipient_guid: u64,
    sender_guid: u64,
    payload: String,
) {
    push_event(
        ctx,
        recipient_guid,
        lyracore_shared::guild::event_kind::GUILD_CHAT,
        sender_guid,
        "",
        payload,
    );
}

// ===========================================================================================
//  Reducers
// ===========================================================================================

/// `CMSG_GUILD_CREATE` on the player's OWN database — the single-database plane, where that one
/// database already is the guild authority and there is nothing to route.
///
/// Operator-gated with the actor named by guid, the `gw_*` verb convention: the gateway holds the
/// operator token and passes the guid it authenticated for that socket.
#[reducer]
pub fn create_guild(ctx: &ReducerContext, actor_guid: u64, name: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::helpers::acting_entity_by_guid(ctx, actor_guid).ok_or("not in world")?;
    create_guild_for(ctx, actor_guid, &name).map(|_| ())
}

/// The realm-wide guild ops, as ONE operator-gated reducer keyed by
/// [`lyracore_shared::guild::realm_op`].
///
/// **Operator-gated, and it has to be**: it takes the acting character's guid as an argument
/// instead of deriving it from `ctx.sender()`, because realm-core has no live entity to derive one
/// from. A client that could call it would act as anybody in the realm. The gateway is the only
/// caller and it holds the coordinator (operator) token.
#[reducer]
pub fn realm_guild_op(
    ctx: &ReducerContext,
    op: u8,
    actor_guid: u64,
    target_guid: u64,
    arg_a: u32,
    text: String,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    use lyracore_shared::guild::realm_op;
    let _ = (target_guid, arg_a); // slots later ops read; CREATE uses `text` alone
    match op {
        realm_op::CREATE => create_guild_for(ctx, actor_guid, &text).map(|_| ()),
        realm_op::INVITE => invite_to_guild(ctx, actor_guid, target_guid, &text),
        realm_op::ANSWER => answer_invite(ctx, actor_guid, arg_a == realm_op::ANSWER_ACCEPT, &text),
        realm_op::PRESENCE => {
            broadcast_presence(ctx, actor_guid, arg_a == realm_op::PRESENCE_ON, &text);
            Ok(())
        }
        realm_op::LEAVE => leave_guild(ctx, actor_guid, &text),
        realm_op::REMOVE => remove_from_guild(ctx, actor_guid, target_guid, &text),
        realm_op::DISBAND => disband_guild(ctx, actor_guid),
        realm_op::LEADER => set_guild_master(ctx, actor_guid, target_guid, &text),
        realm_op::GUILD_CHAT => guild_chat_for(ctx, actor_guid, &text),
        realm_op::SET_MOTD => set_guild_text(ctx, actor_guid, &text, true),
        realm_op::SET_INFO_TEXT => set_guild_text(ctx, actor_guid, &text, false),
        realm_op::SET_PUBLIC_NOTE => set_member_note(ctx, actor_guid, target_guid, &text, false),
        realm_op::SET_OFFICER_NOTE => set_member_note(ctx, actor_guid, target_guid, &text, true),
        other => Err(format!("unknown realm guild op {other}")),
    }
}

/// Push a character's own guild id and rank onto THIS database's `game_character` row.
///
/// The whole of D1's "no roster mirror": realm-core owns the roster, and a world shard needs only
/// the two scalar columns the character carries. The gateway calls this on the player's own shard
/// after a realm-core membership change and at world entry. Mechanical by design — no rules run
/// here, because rules belong to the authority. A character that is not on this database is a
/// no-op, not an error: the gateway pushes to every shard it can reach.
#[reducer]
pub fn sync_guild_membership(
    ctx: &ReducerContext,
    character_guid: u64,
    guild_id: u64,
    guild_rank: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    set_character_guild(ctx, character_guid, guild_id, guild_rank);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_scan::code_of;
    use lyracore_shared::guild::GUILD_RANK_COUNT;

    /// The rank seed is what `SMSG_GUILD_QUERY_RESPONSE`'s fixed `[String; 10]` renders from, and
    /// `seed_ranks` is unreachable without a live node — so the loop's SOURCE is what gets pinned:
    /// it must walk the shared ten-name seed, not a local list that could drift shorter.
    #[test]
    fn rank_seeding_walks_the_shared_ten_name_seed() {
        let body = code_of(include_str!("guild.rs"), "fn seed_ranks(");
        let normalized: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            normalized.contains("for (index, (name, rights)) in DEFAULT_RANK_NAMES")
                && normalized.contains(".zip(DEFAULT_RANK_RIGHTS.iter())"),
            "`seed_ranks` must walk the shared seed pair — a guild with any count other than \
             {GUILD_RANK_COUNT} cannot be rendered into the packet's fixed rank-name array. Body \
             was:\n{body}"
        );
        assert_eq!(DEFAULT_RANK_NAMES.len(), GUILD_RANK_COUNT);
        assert_eq!(DEFAULT_RANK_RIGHTS.len(), GUILD_RANK_COUNT);
    }

    /// The create gate's three refusals are the shared error strings, not ad-hoc prose: the gateway
    /// matches them exactly to pick a `GuildCommandResult`, so a reword here has to be a
    /// compile-visible edit in `lyracore_shared::guild::err`.
    #[test]
    fn the_create_gate_refuses_with_the_shared_error_strings() {
        let body = code_of(include_str!("guild.rs"), "pub(crate) fn create_guild_for(");
        for gate in [
            "guild_err::NAME_INVALID",
            "guild_err::ALREADY_IN_GUILD",
            "guild_err::NAME_TAKEN",
        ] {
            assert!(
                body.contains(gate),
                "`create_guild_for` no longer refuses with `{gate}` — the gateway classifies \
                 SMSG_GUILD_COMMAND_RESULT off these exact strings. Body was:\n{body}"
            );
        }
    }

    /// **The operator gate is the entire authorization of the guild reducers.** Each takes the
    /// acting character's guid (or a whole membership) as an ARGUMENT, so without the gate any
    /// identity that can reach the node founds guilds and rewrites memberships as anybody. Pinned
    /// as the FIRST statement: a gate wrapped in `if false`, bound with `let _ =`, or preceded by
    /// an early return is no gate.
    #[test]
    fn every_guild_reducer_opens_with_the_operator_gate() {
        for f in [
            "pub fn create_guild(",
            "pub fn realm_guild_op(",
            "pub fn sync_guild_membership(",
        ] {
            let body = code_of(include_str!("guild.rs"), f);
            let normalized: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                normalized.starts_with("{ crate::helpers::require_operator(ctx)?;"),
                "`{f}` no longer OPENS with the operator gate. Body was:\n{body}"
            );
        }
    }

    /// The op byte is a wire value the gateway sends and this reducer dispatches on; the shared
    /// contract pins only the NUMBER, so what each number DOES is pinned here.
    #[test]
    fn every_realm_guild_op_byte_dispatches_to_its_own_core() {
        let body = code_of(include_str!("guild.rs"), "pub fn realm_guild_op(");
        let arm = body
            .split("realm_op::CREATE =>")
            .nth(1)
            .unwrap_or_else(|| panic!("`realm_guild_op` no longer dispatches CREATE:\n{body}"));
        let arm: String = arm.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            arm.starts_with("create_guild_for(ctx, actor_guid, &text)"),
            "`realm_op::CREATE` no longer runs the create core. Arm was:\n{arm}"
        );
        for (tag, core) in [
            (
                "realm_op::INVITE =>",
                "invite_to_guild(ctx, actor_guid, target_guid, &text)",
            ),
            (
                "realm_op::ANSWER =>",
                "answer_invite(ctx, actor_guid, arg_a == realm_op::ANSWER_ACCEPT, &text)",
            ),
            (
                "realm_op::PRESENCE =>",
                "{ broadcast_presence(ctx, actor_guid, arg_a == realm_op::PRESENCE_ON, &text);",
            ),
        ] {
            let arm = body.split(tag).nth(1).unwrap_or_else(|| {
                panic!("`realm_guild_op` no longer dispatches `{tag}`:\n{body}")
            });
            let arm: String = arm.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                arm.starts_with(core),
                "`{tag}` no longer runs `{core}`. Arm was:\n{arm}"
            );
        }
    }

    /// The invite gate is the only place the guild system checks authority at all (rank rights are
    /// a deliberate non-goal), and each refusal is a shared error string the gateway matches
    /// exactly to pick a `SMSG_GUILD_COMMAND_RESULT` code.
    #[test]
    fn the_invite_gate_refuses_with_the_shared_error_strings() {
        let body = code_of(include_str!("guild.rs"), "pub(crate) fn invite_to_guild(");
        for gate in [
            "guild_err::NOT_IN_GUILD",
            "guild_err::NOT_GUILD_MASTER",
            "guild_err::TARGET_IN_GUILD",
            "guild_err::ALREADY_INVITED",
        ] {
            assert!(
                body.contains(gate),
                "`invite_to_guild` no longer refuses with `{gate}` — the gateway classifies \
                 SMSG_GUILD_COMMAND_RESULT off these exact strings. Body was:\n{body}"
            );
        }
    }

    /// The invite row is consumed BEFORE either arm runs, which is what makes a dialog answerable
    /// exactly once. Consuming it afterwards would let a double-click join twice.
    #[test]
    fn answering_an_invite_consumes_the_row_before_deciding_anything() {
        let body = code_of(include_str!("guild.rs"), "pub(crate) fn answer_invite(");
        let normalized: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            normalized.starts_with(
                "{ let invite = pending_invite(ctx, actor_guid).ok_or(guild_err::NO_PENDING_INVITE)?; \
                 ctx.db.game_guild_invite().id().delete(invite.id);"
            ),
            "`answer_invite` must take and delete the pending invite first. Body was:\n{body}"
        );
        assert!(
            normalized
                .contains("set_character_guild(ctx, actor_guid, invite.guild_id, GUILD_JOIN_RANK)"),
            "an accepted invite must stamp the new member's own guild columns in the SAME \
             transaction as the member row, or a world shard renders them guildless. Body \
             was:\n{body}"
        );
    }

    /// A never-answered invite is reaped, so accepting a two-minute-old dialog fails exactly like
    /// accepting one that was never sent. Unreachable without a live node — the GC's SOURCE is what
    /// gets pinned.
    #[test]
    fn unanswered_guild_invites_ride_the_shared_invite_ttl() {
        let body = code_of(include_str!("gc.rs"), "pub fn reap_movement_events(");
        let normalized: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            normalized.contains("let t = ctx.db.game_guild_invite();")
                && normalized.contains("INVITE_TTL_MICROS"),
            "`game_guild_invite` is no longer reaped on the shared invite TTL — an unanswered \
             guild dialog would stay answerable forever. Body was:\n{body}"
        );
    }

    /// The teardown ops' own half of the dispatch pin: each byte reaches its own core, with the
    /// argument slots the shared contract documents (`target_guid` = the member acted on, `text` =
    /// the name that goes on the notice).
    #[test]
    fn every_teardown_op_byte_dispatches_to_its_own_core() {
        let body = code_of(include_str!("guild.rs"), "pub fn realm_guild_op(");
        let normalized: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
        for (op, call) in [
            ("realm_op::LEAVE =>", "leave_guild(ctx, actor_guid, &text)"),
            (
                "realm_op::REMOVE =>",
                "remove_from_guild(ctx, actor_guid, target_guid, &text)",
            ),
            ("realm_op::DISBAND =>", "disband_guild(ctx, actor_guid)"),
            (
                "realm_op::LEADER =>",
                "set_guild_master(ctx, actor_guid, target_guid, &text)",
            ),
        ] {
            assert!(
                normalized.contains(&format!("{op} {call}")),
                "`{op}` no longer runs `{call}`. Body was:\n{body}"
            );
        }
    }

    /// **The disband cascade is the failure mode this ticket exists to prevent.**
    /// `GuildMember.character_guid` is unique across the table and enforced in CODE, so one member
    /// row left behind is a character that can never join another guild. The cascade is unreachable
    /// without a live node, so its SHAPE is what gets pinned: every member row deleted, every
    /// member's own guild columns zeroed, then `disband` for the ranks, invites and guild row.
    #[test]
    fn the_disband_cascade_clears_members_columns_ranks_invites_and_the_guild_row() {
        let body = code_of(include_str!("guild.rs"), "pub(crate) fn disband_guild(");
        let normalized: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
        for step in [
            "for member in members_of(ctx, guild_id)",
            "ctx.db.game_guild_member().id().delete(member.id)",
            "set_character_guild(ctx, member.character_guid, 0, 0)",
            "disband(ctx, guild_id)",
        ] {
            assert!(
                normalized.contains(step),
                "`disband_guild` no longer does `{step}` — an orphaned row here locks a character \
                 out of every future guild. Body was:\n{body}"
            );
        }
        // And `disband` itself is what takes the rows a member row does not own.
        let cascade = code_of(include_str!("guild.rs"), "fn disband(");
        for table in ["game_guild_rank()", "game_guild_invite()", "game_guild()"] {
            assert!(
                cascade.contains(table),
                "`disband` no longer clears `{table}`. Body was:\n{cascade}"
            );
        }
    }

    /// A deleted character leaves through the SAME core a voluntary leave uses. A bare row delete
    /// would strip a guild of its master with no succession and no last-member disband.
    #[test]
    fn deleting_a_character_routes_through_the_removal_core() {
        let sweep = code_of(
            include_str!("guild.rs"),
            "fn sweep_delete_game_guild_member(ctx, character_guid)",
        );
        let normalized: String = sweep.split_whitespace().collect::<Vec<_>>().join(" ");
        assert_eq!(
            normalized, "{ remove_member(ctx, character_guid); }",
            "the character-delete sweep must be exactly the removal core. Body was:\n{sweep}"
        );
        // …and the removal core is the one that succeeds a master and disbands a last-member guild.
        let core = code_of(include_str!("guild.rs"), "pub(crate) fn remove_member(");
        for step in ["disband(ctx, guild_id)", "promote_to_master(ctx, heir)"] {
            assert!(
                core.contains(step),
                "`remove_member` no longer does `{step}`. Body was:\n{core}"
            );
        }
    }

    /// Succession is a DECISION, not a gap: the realm refuses a master who would leave members
    /// behind rather than promoting somebody for them.
    #[test]
    fn a_master_with_members_left_behind_is_refused_rather_than_succeeded() {
        let body = code_of(include_str!("guild.rs"), "pub(crate) fn leave_guild(");
        let normalized: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            normalized.contains(
                "if guild.master_guid == actor_guid && members.len() > 1 { return \
                 Err(guild_err::MASTER_MUST_TRANSFER_OR_DISBAND.to_string()); }"
            ),
            "`leave_guild` no longer refuses a master with members remaining. Body was:\n{body}"
        );
    }

    /// The teardown gates refuse with the shared error strings, exactly as the create gate does —
    /// the gateway picks a `GuildCommandResult` by matching them.
    #[test]
    fn the_teardown_gates_refuse_with_the_shared_error_strings() {
        for (f, gates) in [
            (
                "fn master_guild_of(",
                ["guild_err::NOT_IN_GUILD", "guild_err::NOT_GUILD_MASTER"].as_slice(),
            ),
            (
                "pub(crate) fn remove_from_guild(",
                &[
                    "guild_err::CANNOT_REMOVE_SELF",
                    "guild_err::TARGET_NOT_IN_GUILD",
                ],
            ),
            (
                "pub(crate) fn set_guild_master(",
                &["guild_err::TARGET_NOT_IN_GUILD"],
            ),
        ] {
            let body = code_of(include_str!("guild.rs"), f);
            for gate in gates {
                assert!(
                    body.contains(gate),
                    "`{f}` no longer refuses with `{gate}` — the gateway classifies \
                     SMSG_GUILD_COMMAND_RESULT off these exact strings. Body was:\n{body}"
                );
            }
        }
    }

    /// The `GUILD_CHAT` op byte must dispatch to the chat core, the same way `CREATE` dispatches
    /// to `create_guild_for` above.
    #[test]
    fn realm_guild_op_dispatches_guild_chat_to_its_own_core() {
        let body = code_of(include_str!("guild.rs"), "pub fn realm_guild_op(");
        let arm = body
            .split("realm_op::GUILD_CHAT =>")
            .nth(1)
            .unwrap_or_else(|| panic!("`realm_guild_op` no longer dispatches GUILD_CHAT:\n{body}"));
        let arm: String = arm.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            arm.starts_with("guild_chat_for(ctx, actor_guid, &text)"),
            "`realm_op::GUILD_CHAT` no longer runs the chat core. Arm was:\n{arm}"
        );
    }

    // ---- Guild chat (T5) ----

    /// The chat gate's refusal is the shared error string: the gateway maps it to
    /// `SMSG_GUILD_COMMAND_RESULT(GuildPlayerNotInGuild)`.
    #[test]
    fn guild_chat_refuses_a_caller_with_no_guild_with_the_shared_error_string() {
        let body = code_of(include_str!("guild.rs"), "pub(crate) fn guild_chat_for(");
        assert!(
            body.contains("guild_err::NOT_IN_GUILD"),
            "`guild_chat_for` no longer refuses with `guild_err::NOT_IN_GUILD` — the gateway \
             classifies SMSG_GUILD_COMMAND_RESULT off this exact string. Body was:\n{body}"
        );
    }

    /// Mirrors `chat::party_chat_routes_to_every_other_member_and_excludes_the_sender`: the sender
    /// never appears among the OTHER recipients, regardless of where in the roster they sit.
    #[test]
    fn guild_chat_other_recipients_excludes_the_sender_and_preserves_order() {
        let members = [10u64, 20, 30];
        assert_eq!(
            guild_chat_other_recipients(20, &members),
            vec![10, 30],
            "the sender must never appear among the OTHER recipients"
        );
        assert_eq!(guild_chat_other_recipients(2, &[1, 2, 3, 4]), vec![1, 3, 4]);
        assert_eq!(guild_chat_other_recipients(1, &[1, 2, 3]), vec![2, 3]);
        assert_eq!(guild_chat_other_recipients(3, &[1, 2, 3]), vec![1, 2]);
    }

    /// Degenerate case: a guild of one (only the sender). Must not panic or wrongly include them.
    #[test]
    fn guild_chat_other_recipients_is_empty_when_the_sender_is_the_only_member() {
        assert_eq!(guild_chat_other_recipients(7, &[7]), Vec::<u64>::new());
    }

    /// T6's four op bytes each dispatch to their own core, the same way CREATE does above.
    #[test]
    fn every_motd_and_note_realm_guild_op_byte_dispatches_to_its_own_core() {
        let body = code_of(include_str!("guild.rs"), "pub fn realm_guild_op(");
        for (op, core) in [
            (
                "realm_op::SET_MOTD =>",
                "set_guild_text(ctx, actor_guid, &text, true)",
            ),
            (
                "realm_op::SET_INFO_TEXT =>",
                "set_guild_text(ctx, actor_guid, &text, false)",
            ),
            (
                "realm_op::SET_PUBLIC_NOTE =>",
                "set_member_note(ctx, actor_guid, target_guid, &text, false)",
            ),
            (
                "realm_op::SET_OFFICER_NOTE =>",
                "set_member_note(ctx, actor_guid, target_guid, &text, true)",
            ),
        ] {
            let arm = body
                .split(op)
                .nth(1)
                .unwrap_or_else(|| panic!("`realm_guild_op` no longer dispatches {op}:\n{body}"));
            let arm: String = arm.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                arm.starts_with(core),
                "`{op}` no longer runs `{core}`. Arm was:\n{arm}"
            );
        }
    }

    /// D3's only permission check, pinned at the setter that guards the guild's shared text: a
    /// non-master is refused, and — the criterion that reads like a trap — an EMPTY value is never
    /// treated as a second refusal. `set_guild_text` must not gate on the string's own length/emptiness
    /// at all; the master's write always lands, blank or not.
    #[test]
    fn set_guild_text_gates_on_the_master_alone_and_never_on_emptiness() {
        let body = code_of(include_str!("guild.rs"), "fn set_guild_text(");
        assert!(
            body.contains("guild.master_guid != actor_guid")
                && body.contains("guild_err::NOT_GUILD_MASTER"),
            "`set_guild_text` must refuse a non-master setter with the shared NOT_GUILD_MASTER \
             string. Body was:\n{body}"
        );
        assert!(
            !body.contains("is_empty()") && !body.contains(".len() == 0"),
            "`set_guild_text` must not reject an empty value — an empty MOTD/info text is a valid \
             CLEAR, not a refusal (acceptance criterion 4). Body was:\n{body}"
        );
    }

    /// The note gate's one member-writable exception, pinned: a member may set their OWN public
    /// note without being master, but every other combination (someone else's public note, anyone's
    /// officer note) still runs through the master-only refusal.
    #[test]
    fn set_member_note_admits_only_a_members_own_public_note_without_the_master_gate() {
        let body = code_of(include_str!("guild.rs"), "fn set_member_note(");
        let normalized: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            normalized
                .contains("let setting_own_public_note = !officer && actor_guid == target_guid;"),
            "`set_member_note` no longer computes the own-public-note exception the way D3 needs. \
             Body was:\n{body}"
        );
        assert!(
            normalized.contains("if !is_master && !setting_own_public_note {"),
            "`set_member_note` must refuse every case OTHER than the member's own public note \
             unless the actor is master. Body was:\n{body}"
        );
    }
}
