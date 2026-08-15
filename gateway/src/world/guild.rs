//! Realm-wide guild state — the ROUTING half, and the only place that decides which database a
//! guild op runs against.
//!
//! Guild membership is authoritative on **realm-core**, for the reason party membership is: nothing
//! about a guild is coupled to space, so a shard-local guild would silently split the moment a
//! second database existed. Everything here is generic over [`WorldStore`], so the decisions execute
//! under test against the same in-memory multi-database topology the cross-database transfer uses.
//!
//! [`WorldStore::realm_store`] answering `None` is not "no realm-core configured" — it is "this
//! gateway runs against ONE database", which already is the authority. That arm calls the
//! player-facing reducer on the player's own shard and reads the player's own shard, byte-identical
//! to the sharded path from the client's side. `an_unsharded_gateway_runs_every_guild_op_on_the_\
//! players_own_shard` pins it.
//!
//! **There is no roster mirror**, and that is a deliberate difference from `world::party`. Party
//! needed one because ~fifty in-world reads resolve membership on the hot path; guild has none. All
//! a world shard needs is the character's OWN guild id and rank, which ride two scalar columns on
//! `game_character` — pushed by [`push_membership`] after a membership change and by
//! [`on_world_entry`] when a character arrives.

use anyhow::Result;

use super::WorldStore;
use lyracore_shared::guild::realm_op;

/// One guild, as the database that holds it sees it — authoritative on realm-core, and on a
/// single-database gateway simply that database's own rows.
///
/// `rank_names` carries exactly `lyracore_shared::guild::GUILD_RANK_COUNT` entries in rank order,
/// because `SMSG_GUILD_QUERY_RESPONSE` renders a fixed `[String; 10]` from them.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct GuildView {
    pub guild_id: u64,
    pub name: String,
    pub master_guid: u64,
    pub motd: String,
    pub info_text: String,
    /// Unix-epoch micros the guild was founded. Split into day/month/year at packet build.
    pub created_micros: i64,
    pub member_count: u32,
    pub rank_names: Vec<String>,
}

/// One guild op, in the client's own vocabulary. Packing the arguments into `realm_guild_op`'s
/// slots happens once, in [`run`], against the shared [`lyracore_shared::guild::realm_op`] contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    /// `CMSG_GUILD_CREATE`, name already validated by nobody — the module owns that gate.
    Create(String),
}

/// Run one guild op for the session that owns `self_guid`.
///
/// Unsharded → the player's own connection calls the player-facing reducer on the player's own
/// shard, and nothing else happens.
///
/// Sharded → realm-core runs the op, then the acting character's own guild columns are pushed onto
/// every connected world shard. The push is best-effort BY DESIGN (see [`push_membership`]); the
/// op's own result is not.
pub(crate) fn run<St: WorldStore + ?Sized>(
    store: &St,
    account_id: u64,
    self_guid: u64,
    op: Op,
) -> Result<()> {
    let Some(realm) = store.realm_store() else {
        return match op {
            Op::Create(name) => store.create_guild(account_id, self_guid, &name),
        };
    };
    let (code, target, arg_a, text) = match op {
        Op::Create(name) => (realm_op::CREATE, 0, 0, name),
    };
    realm.realm_guild_op(code, self_guid, target, arg_a, text)?;
    push_membership(store, realm.as_ref(), self_guid);
    Ok(())
}

/// Push `self_guid`'s own guild id and rank, as the authority has them, onto every connected world
/// shard.
///
/// **Best-effort, deliberately.** The authoritative write has already committed on realm-core by
/// the time this runs, so a failed push must not turn a guild op that DID happen into an error the
/// client renders as a failure. The cost of a miss is bounded and self-healing: that shard's
/// `SMSG_CHAR_ENUM` and guild unit fields read the previous pair until the next op or the next
/// world entry re-pushes it, and nothing else on a shard reads guild state at all.
pub(crate) fn push_membership<St: WorldStore + ?Sized>(
    store: &St,
    realm: &dyn WorldStore,
    self_guid: u64,
) {
    let (guild_id, rank) = match realm.guild_membership(self_guid) {
        Ok(m) => m.unwrap_or((0, 0)),
        Err(e) => {
            log::warn!(
                "guild: could not read realm-core membership for {self_guid} ({e:#}) — the shards \
                 keep their previous guild columns until the next op or world entry"
            );
            return;
        }
    };
    for shard in store.world_stores() {
        if let Err(e) = shard.sync_guild_membership(self_guid, guild_id, rank) {
            log::warn!(
                "guild: could not push guild {guild_id} onto shard {} for {self_guid} ({e:#}) — \
                 that shard's guild columns stay stale until the next op or world entry",
                shard.shard_name()
            );
        }
    }
}

/// World entry (login, and every cross-shard arrival): put the guild the player is actually in onto
/// the shard they just entered.
///
/// Unsharded → returns immediately, before any read: the shard's own tables already are the
/// authority and the login path is unchanged.
pub(crate) fn on_world_entry<St: WorldStore + ?Sized>(store: &St, self_guid: u64) -> Result<()> {
    let Some(realm) = store.realm_store() else {
        return Ok(());
    };
    let (guild_id, rank) = realm.guild_membership(self_guid)?.unwrap_or((0, 0));
    store.sync_guild_membership(self_guid, guild_id, rank)
}

/// The guild `guild_id` names, read from the authority — realm-core when there is one, this
/// handle's own database when there is not.
pub(crate) fn view<St: WorldStore + ?Sized>(
    store: &St,
    guild_id: u64,
) -> Result<Option<GuildView>> {
    match store.realm_store() {
        Some(realm) => realm.guild_snapshot(guild_id),
        None => store.guild_snapshot(guild_id),
    }
}

/// The guild `character_guid` belongs to, read from the authority. `None` = guildless.
///
/// Deliberately NOT the character's own `game_character.guild_id` column: that column is the
/// shard's cached copy, and a membership change made while the player stood on another shard may
/// not have reached it yet. The authority always has.
pub(crate) fn guild_of<St: WorldStore + ?Sized>(
    store: &St,
    character_guid: u64,
) -> Result<Option<u64>> {
    let membership = match store.realm_store() {
        Some(realm) => realm.guild_membership(character_guid)?,
        None => store.guild_membership(character_guid)?,
    };
    Ok(membership.map(|(guild_id, _rank)| guild_id))
}
