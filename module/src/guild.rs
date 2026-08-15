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
    GUILD_MASTER_RANK,
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

/// Queue one guild notification for `recipient_guid`.
///
/// `other_name` is passed IN rather than read from `game_character`, which is the one place this
/// differs from [`crate::group::push_event`]: realm-core holds no character rows, so the gateway
/// resolves the name and carries it in [`realm_guild_op`]'s `text` slot. The identity is bound where
/// there IS a character row (a single-database gateway) and falls back to the unaddressable
/// placeholder where there is not — the realm-core relay filters on `recipient_guid`.
///
/// The binding read goes through the by-guid transfer chokepoint, so a mid-transfer recipient reads
/// as absent and gets the placeholder identity. That costs nothing: the guid is what addresses the
/// row, and the destination's session picks it up from there.
fn push_guild_event(
    ctx: &ReducerContext,
    recipient_guid: u64,
    kind: u8,
    other_guid: u64,
    other_name: &str,
) {
    let bound = crate::helpers::character_by_guid(ctx, recipient_guid).map(|c| c.owner_identity);
    ctx.db.game_guild_event().insert(GuildEvent {
        id: 0,
        recipient_identity: crate::helpers::event_recipient_identity(bound),
        recipient_guid,
        kind,
        other_guid,
        other_name: other_name.to_string(),
        payload: String::new(),
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
        push_guild_event(
            ctx,
            other.character_guid,
            event_kind::LEFT,
            actor_guid,
            actor_name,
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
        push_guild_event(
            ctx,
            other.character_guid,
            event_kind::REMOVED,
            target_guid,
            target_name,
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
        push_guild_event(
            ctx,
            member.character_guid,
            event_kind::DISBANDED,
            actor_guid,
            "",
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
        push_guild_event(
            ctx,
            member.character_guid,
            event_kind::LEADER_CHANGED,
            target_guid,
            target_name,
        );
    }
    Ok(())
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
        realm_op::LEAVE => leave_guild(ctx, actor_guid, &text),
        realm_op::REMOVE => remove_from_guild(ctx, actor_guid, target_guid, &text),
        realm_op::DISBAND => disband_guild(ctx, actor_guid),
        realm_op::LEADER => set_guild_master(ctx, actor_guid, target_guid, &text),
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
}
