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
    /// `CMSG_GUILD_INVITE`, target already resolved realm-wide by [`resolve_target`].
    ///
    /// `actor_name` rides along because realm-core holds no `game_character` rows: the invite
    /// popup names the inviter, and the authority cannot look that name up for itself.
    Invite { target: u64, actor_name: String },
    /// `CMSG_GUILD_ACCEPT`. `actor_name` is what the `Joined` broadcast carries.
    Accept { actor_name: String },
    /// `CMSG_GUILD_DECLINE`. `actor_name` is what the inviter's notification carries.
    Decline { actor_name: String },
    /// `CMSG_GUILD_LEAVE`. Carries the caller's OWN name, because realm-core has no character rows
    /// to resolve one from and the departure notice needs it.
    Leave { actor_name: String },
    /// `CMSG_GUILD_REMOVE` — the guild master removes `target_guid`, already resolved from the
    /// typed name by [`member_by_name`].
    Remove {
        target_guid: u64,
        target_name: String,
    },
    /// `CMSG_GUILD_DISBAND` — the guild master destroys the guild.
    Disband,
    /// `CMSG_GUILD_LEADER` — the guild master hands the guild to `target_guid`.
    Leader {
        target_guid: u64,
        target_name: String,
    },
    /// `CMSG_GUILD_MOTD` — the new MOTD (may be empty; an empty value CLEARS it, D3's acceptance
    /// criterion 4). Master-only, gated module-side.
    SetMotd(String),
    /// `CMSG_GUILD_INFO_TEXT` — same shape and gate as [`Op::SetMotd`], for `Guild.info_text`.
    SetInfoText(String),
    /// `CMSG_GUILD_SET_PUBLIC_NOTE` — `target_guid`'s public note becomes `note`. Module-gated: the
    /// note owner or the guild master.
    SetPublicNote { target_guid: u64, note: String },
    /// `CMSG_GUILD_SET_OFFICER_NOTE` — same shape as [`Op::SetPublicNote`], master-only.
    SetOfficerNote { target_guid: u64, note: String },
}

/// Run one guild op for the session that owns `self_guid`.
///
/// Unsharded → [`Op::Create`] calls the player-facing `create_guild` reducer on the player's own
/// shard, which already is the authority; every other op reaches [`authority`] on that same
/// database. A same-shaped single-database reducer per verb would cost a hand-spliced binding each
/// and buy nothing, because `realm_guild_op` runs correctly against ANY database.
///
/// Sharded → realm-core runs the op, then the guild columns of every character it moved are pushed
/// onto every connected world shard. The push is best-effort BY DESIGN (see [`push_membership`]);
/// the op's own result is not.
pub(crate) fn run<St: WorldStore + ?Sized>(
    store: &St,
    account_id: u64,
    self_guid: u64,
    op: Op,
) -> Result<()> {
    if let (None, Op::Create(name)) = (store.realm_store(), &op) {
        return store.create_guild(account_id, self_guid, name);
    }
    // Read BEFORE the op: a disband leaves no roster to read the ex-members from afterwards, and a
    // shard that never hears about them keeps the dead guild's columns forever.
    let moved = moved_by(store, self_guid, &op)?;
    let (code, target, arg_a, text) = slots(op);
    authority(store, code, self_guid, target, arg_a, text)?;
    if let Some(realm) = store.realm_store() {
        for guid in moved {
            push_membership(store, realm.as_ref(), guid);
        }
    }
    Ok(())
}

/// Call `realm_guild_op` on whichever database is the guild AUTHORITY: realm-core when the gateway
/// is sharded, this handle's own database when it is not.
///
/// The one place that choice is made, so no verb can drift onto the wrong plane.
fn authority<St: WorldStore + ?Sized>(
    store: &St,
    op: u8,
    actor_guid: u64,
    target_guid: u64,
    arg_a: u32,
    text: String,
) -> Result<()> {
    match store.realm_store() {
        Some(realm) => realm.realm_guild_op(op, actor_guid, target_guid, arg_a, text),
        None => store.realm_guild_op(op, actor_guid, target_guid, arg_a, text),
    }
}

/// Resolve `CMSG_GUILD_INVITE`'s typed name to the character it means, across every shard.
///
/// **This is why guild state is on realm-core at all.** Resolving inside the calling database
/// answers "no player named X" for a character who is merely standing on another shard, which is
/// the defect the party slice already hit.
///
/// Every candidate is considered, not the first: character names are unique per DATABASE, not per
/// realm, so the same name can name two people. Being in the world is the disambiguator, and it is
/// also a gate in its own right — an offline character has no client to answer the popup, so an
/// invite to one is `GuildPlayerNotFoundS` exactly as an invite to a stranger is.
fn resolve_target<St: WorldStore + ?Sized>(store: &St, name: &str) -> Result<u64> {
    for candidate in super::party::resolve_all_by_name(store, name)? {
        if super::party::live_anywhere(store, candidate) {
            return Ok(candidate);
        }
    }
    anyhow::bail!(lyracore_shared::guild::err::TARGET_NOT_FOUND)
}

/// `CMSG_GUILD_INVITE`: resolve the typed name realm-wide, then run the invite on the authority.
pub(crate) fn invite<St: WorldStore + ?Sized>(
    store: &St,
    account_id: u64,
    self_guid: u64,
    name: &str,
) -> Result<()> {
    let target = resolve_target(store, name)?;
    let op = Op::Invite {
        target,
        actor_name: own_name(store, self_guid)?,
    };
    run(store, account_id, self_guid, op)
}

/// `CMSG_GUILD_ACCEPT` / `CMSG_GUILD_DECLINE`: answer the pending invite as the acting character.
pub(crate) fn answer_invite<St: WorldStore + ?Sized>(
    store: &St,
    account_id: u64,
    self_guid: u64,
    accept: bool,
) -> Result<()> {
    let actor_name = own_name(store, self_guid)?;
    let op = if accept {
        Op::Accept { actor_name }
    } else {
        Op::Decline { actor_name }
    };
    run(store, account_id, self_guid, op)
}

/// Tell the rest of `self_guid`'s guild that they signed on (`online`) or off.
///
/// Deliberately NOT an [`Op`] through [`run`]: presence changes no membership, so the guild-column
/// push `run` performs after every op would be a realm read and a write to every shard for nothing.
/// The authority's own broadcast is the whole effect.
///
/// Best-effort throughout. This runs on the login and logout paths, and neither may fail over a
/// notification: a missed `SignedOn` costs one chat line, a failed login costs the session.
pub(crate) fn broadcast_presence<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: u64,
    online: bool,
) {
    match guild_of(store, self_guid) {
        Ok(Some(_)) => {}
        // Guildless: nobody to tell, and no reason to spend a reducer call finding that out again.
        Ok(None) => return,
        Err(e) => {
            log::warn!(
                "guild: could not read membership for {self_guid} ({e:#}) — no presence broadcast"
            );
            return;
        }
    }
    let arg_a = if online {
        realm_op::PRESENCE_ON
    } else {
        realm_op::PRESENCE_OFF
    };
    let name = own_name(store, self_guid).unwrap_or_default();
    if let Err(e) = authority(store, realm_op::PRESENCE, self_guid, 0, arg_a, name) {
        log::warn!(
            "guild: could not broadcast {} for {self_guid} ({e:#}) — their guild sees no status line",
            if online { "sign-on" } else { "sign-off" }
        );
    }
}

/// Pack one op into `realm_guild_op`'s frozen argument slots, against the shared
/// [`lyracore_shared::guild::realm_op`] contract. The single packing site: both planes call it, so
/// the two cannot drift.
fn slots(op: Op) -> (u8, u64, u32, String) {
    match op {
        Op::Create(name) => (realm_op::CREATE, 0, 0, name),
        Op::Invite {
            target,
            actor_name: name,
        } => (realm_op::INVITE, target, 0, name),
        Op::Accept { actor_name: name } => (realm_op::ANSWER, 0, realm_op::ANSWER_ACCEPT, name),
        Op::Decline { actor_name: name } => (realm_op::ANSWER, 0, realm_op::ANSWER_DECLINE, name),
        Op::Leave { actor_name } => (realm_op::LEAVE, 0, 0, actor_name),
        Op::Remove {
            target_guid,
            target_name,
        } => (realm_op::REMOVE, target_guid, 0, target_name),
        Op::Disband => (realm_op::DISBAND, 0, 0, String::new()),
        Op::Leader {
            target_guid,
            target_name,
        } => (realm_op::LEADER, target_guid, 0, target_name),
        Op::SetMotd(text) => (realm_op::SET_MOTD, 0, 0, text),
        Op::SetInfoText(text) => (realm_op::SET_INFO_TEXT, 0, 0, text),
        Op::SetPublicNote { target_guid, note } => {
            (realm_op::SET_PUBLIC_NOTE, target_guid, 0, note)
        }
        Op::SetOfficerNote { target_guid, note } => {
            (realm_op::SET_OFFICER_NOTE, target_guid, 0, note)
        }
    }
}

/// Every character whose guild columns `op` may move, read from the authority BEFORE it runs.
///
/// The caller is always in it — a create, a leave and a disband all move the caller's own columns.
/// A kick and a leadership transfer move the target's too, and a DISBAND moves every member's, which
/// is the only case that needs a roster read: criterion "no ex-member keeps a dead guild's id"
/// cannot be met from the caller alone.
fn moved_by<St: WorldStore + ?Sized>(store: &St, self_guid: u64, op: &Op) -> Result<Vec<u64>> {
    let mut moved = vec![self_guid];
    match op {
        // Nothing here moves a SECOND character's guild columns. The caller's own pair is
        // refreshed anyway, which costs one read and keeps the shards honest after an accept.
        Op::Create(_)
        | Op::Invite { .. }
        | Op::Accept { .. }
        | Op::Decline { .. }
        | Op::Leave { .. }
        | Op::SetMotd(_)
        | Op::SetInfoText(_)
        | Op::SetPublicNote { .. }
        | Op::SetOfficerNote { .. } => {}
        Op::Remove { target_guid, .. } | Op::Leader { target_guid, .. } => moved.push(*target_guid),
        // The only op that has to read the roster: every member's columns move, and after the
        // cascade there is no roster left to read them from.
        Op::Disband => {
            if let Some(guild_id) = guild_of(store, self_guid)? {
                let view = match store.realm_store() {
                    Some(realm) => realm.guild_roster_snapshot(guild_id)?,
                    None => store.guild_roster_snapshot(guild_id)?,
                };
                for member in view.into_iter().flat_map(|v| v.members) {
                    if !moved.contains(&member.guid) {
                        moved.push(member.guid);
                    }
                }
            }
        }
    }
    Ok(moved)
}

/// The caller's own character name, from whichever shard holds them.
///
/// Realm-core has no character rows, so a departure notice written there can only carry the name the
/// gateway hands it. Empty when the character cannot be found on any connected shard: a nameless
/// notice is better than a failed leave.
pub(crate) fn own_name<St: WorldStore + ?Sized>(store: &St, self_guid: u64) -> Result<String> {
    Ok(super::party::character_anywhere(store, self_guid)?
        .map(|c| c.name)
        .unwrap_or_default())
}

/// Resolve a typed player NAME to the guid of a member of `self_guid`'s OWN guild.
///
/// Character names are not realm-unique — [`super::party::resolve_all_by_name`] records why — so the
/// guild is the tie-break: among every homonym the realm holds, the one sharing the caller's guild
/// is the one the client meant. A name that matches nobody in the guild refuses exactly as a name
/// that matches a non-member does: from the client's side both are the same typo.
///
/// The module gates on membership again, so this is a resolution step and not the authority.
pub(crate) fn member_by_name<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: u64,
    name: &str,
) -> Result<u64> {
    use lyracore_shared::guild::err;
    let guild_id =
        guild_of(store, self_guid)?.ok_or_else(|| anyhow::anyhow!("{}", err::NOT_IN_GUILD))?;
    for guid in super::party::resolve_all_by_name(store, name)? {
        if guild_of(store, guid)? == Some(guild_id) {
            return Ok(guid);
        }
    }
    Err(anyhow::anyhow!("{}", err::TARGET_NOT_IN_GUILD))
}

/// Push `self_guid`'s own guild id and rank, as the authority has them, onto every connected world
/// shard.
///
/// **Best-effort, deliberately.** The authoritative write has already committed on realm-core by
/// the time this runs, so a failed push must not turn a guild op that DID happen into an error the
/// client renders as a failure. The cost of a miss is bounded and self-healing: that shard's
/// `SMSG_CHAR_ENUM` and guild unit fields read the previous pair until the next op or the next
/// world entry re-pushes it, and nothing else on a shard reads guild state at all.
fn push_membership<St: WorldStore + ?Sized>(
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

// ===========================================================================================
//  Roster
// ===========================================================================================

/// One guild's roster as the AUTHORITY holds it: guids, ranks and notes, plus the guild-wide text
/// and the per-rank rights.
///
/// Nothing a `game_character` row would answer is in here, and that is not an omission — realm-core
/// holds no character rows at all. [`render_roster`] joins this half to the shards' half.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct GuildRosterView {
    pub guild_id: u64,
    pub motd: String,
    pub info_text: String,
    /// One entry per rank row, in rank order — `SMSG_GUILD_ROSTER.rights` verbatim. Written at
    /// creation from the vanilla defaults and never consulted server-side.
    pub rank_rights: Vec<u32>,
    /// Members in join order (member-row id), which is the order the roster renders in.
    pub members: Vec<GuildRosterMember>,
}

/// A roster member as realm-core has it — the whole of what the authority knows about a member.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct GuildRosterMember {
    pub guid: u64,
    pub rank: u32,
    pub public_note: String,
    pub officer_note: String,
}

/// A guild roster with every field `SMSG_GUILD_ROSTER` carries: the authority's half joined to the
/// human-readable half only the shards can answer.
///
/// [`render_roster`] is the only thing that builds one, which is what stops a roster packet being
/// built from realm-core's rows alone — that compiles, and renders a guild of nameless level-zero
/// members standing in `Area::None`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct GuildRoster {
    pub motd: String,
    pub info_text: String,
    pub rank_rights: Vec<u32>,
    pub members: Vec<GuildRosterEntry>,
}

/// A rendered roster member: the authority's guid, rank and notes, plus what the shard holding the
/// character answered for. A member no shard has a row for keeps the first half and takes the
/// defaults for the second.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct GuildRosterEntry {
    pub guid: u64,
    pub rank: u32,
    pub public_note: String,
    pub officer_note: String,
    pub name: String,
    pub level: u8,
    pub class: u8,
    pub zone_id: u32,
    pub online: bool,
}

/// The roster of `guild_id`, read from the authority and rendered against the shards. `None` = no
/// such guild.
pub(crate) fn roster<St: WorldStore + ?Sized>(
    store: &St,
    guild_id: u64,
) -> Result<Option<GuildRoster>> {
    let view = match store.realm_store() {
        Some(realm) => realm.guild_roster_snapshot(guild_id)?,
        None => store.guild_roster_snapshot(guild_id)?,
    };
    Ok(view.map(|view| render_roster(store, &view)))
}

/// Fill each member's name, level, class, area and online flag from the shards — the guild twin of
/// [`super::party::render_list`], and for the same reason.
///
/// **The fan-out is the design, not a workaround.** Realm-core owns membership and holds no
/// `game_character` rows, so it cannot say what a member is called, what level they are, or where
/// they stand. Skipping the fan-out still builds a well-formed packet, which is exactly why it has
/// to be here: the guild panel would render every member as a nameless level-zero character in
/// `Area::None`.
///
/// A member no connected shard has a row for keeps its guid, rank and notes and takes the defaults
/// for the rest. It is never dropped: a missing row in the guild panel reads as "they left", which
/// is a worse lie than a blank one.
fn render_roster<St: WorldStore + ?Sized>(
    store: &St,
    view: &GuildRosterView,
) -> GuildRoster {
    GuildRoster {
        motd: view.motd.clone(),
        info_text: view.info_text.clone(),
        rank_rights: view.rank_rights.clone(),
        members: view
            .members
            .iter()
            .map(|m| {
                // This handle first (the viewer's own shard), then every other connected one — a
                // member standing inside an instance is on a database this handle never reads.
                // Empty on a single-database gateway, so the union never leaves it.
                let character = super::party::character_anywhere(store, m.guid)
                    .ok()
                    .flatten();
                GuildRosterEntry {
                    guid: m.guid,
                    rank: m.rank,
                    public_note: m.public_note.clone(),
                    officer_note: m.officer_note.clone(),
                    name: character
                        .as_ref()
                        .map(|c| c.name.clone())
                        .unwrap_or_default(),
                    level: character.as_ref().map_or(0, |c| c.level),
                    class: character.as_ref().map_or(0, |c| c.class),
                    zone_id: character.as_ref().map_or(0, |c| c.zone_id),
                    // The live-entity union, not `game_character.online`: the same flag the party
                    // frame's online column reads, so a session-less playerbot in the guild shows
                    // up as the online member it is.
                    online: super::party::live_anywhere(store, m.guid),
                }
            })
            .collect(),
    }
}

/// Fill the `/who` panel's guild-name column from the AUTHORITY.
///
/// The shards answer for who is online and hand back each character's cached `guild_id`; only
/// realm-core has the guild rows a NAME comes from, so the join happens here — the same split
/// [`render_roster`] makes, for the same reason. A single-database gateway reads its own rows and
/// gets the same answer.
///
/// One read per distinct guild, not per player: a 49-row panel of one guild costs one read. A guild
/// the authority cannot answer for leaves that row's name empty rather than dropping the player.
pub(crate) fn render_who<St: WorldStore + ?Sized>(
    store: &St,
    players: &mut [super::super::codec::WhoPlayerView],
) {
    let mut names: std::collections::HashMap<u64, String> = std::collections::HashMap::new();
    for player in players.iter_mut() {
        if player.guild_id == 0 {
            continue;
        }
        let name = names.entry(player.guild_id).or_insert_with(|| {
            view(store, player.guild_id)
                .ok()
                .flatten()
                .map(|g| g.name)
                .unwrap_or_default()
        });
        player.guild.clone_from(name);
    }
}

/// Send one `/g` line for the session that owns `self_guid` — D4 and D1 made concrete.
///
/// Deliberately NOT an [`Op`] through [`run`]: chat changes no membership, so the guild-column push
/// `run` performs after every op would be a realm read and a write to every shard for nothing.
/// Mirrors `world::whisper::run`, the closest existing precedent for a realm-core relay with no
/// shard mirror behind it.
pub(crate) fn send_chat<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: u64,
    text: String,
) -> Result<()> {
    authority(store, realm_op::GUILD_CHAT, self_guid, 0, 0, text)
}
