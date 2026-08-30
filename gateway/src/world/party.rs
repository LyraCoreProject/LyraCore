//! Realm-wide party state — the GROUP slice of realm-core's social & economy plane, part of the
//! elastic world-sharding design.
//!
//! # What moved, and why here
//!
//! `game_group` / `game_group_member` / `game_group_invite` are authoritative on **realm-core**.
//! None of the three is coupled to space — the spec's partition rule — and all three silently became
//! shard-local the moment a second database existed: `group_invite` resolved its target inside the
//! CALLING database, so a player inside Deadmines could not invite one standing in Elwynn, and the
//! escrowed transfer's interim membership mirror was a snapshot taken at `begin_transfer`, so a party
//! SPLIT across the boundary never saw itself. Both were observed live (2026-07-25), not theorised.
//!
//! This module is the ROUTING half of the fix and the only place that decides which database a party
//! op runs against. Everything in it is generic over [`WorldStore`], so the decisions execute under
//! test against the same in-memory shard topology the cross-database transfer uses — the seam the
//! transfer-transport test harness built. The database-specific halves are thin: `Coordinator`'s
//! trait impl (which database a handle names) and the module's `realm_group_op` (the rules, unchanged).
//!
//! # The three planes
//!
//! 1. **Realm-core** owns membership. Ops go there through the operator-gated `realm_group_op`, and
//!    it is where invites, rosters and the leadership/disband rules live. One authority, so an invite
//!    across a shard boundary is not a special case — it is the only case.
//! 2. **Each world shard** keeps a MIRROR of the roster, refreshed by [`sync_mirrors`] after every op
//!    and by [`on_world_entry`] when a character arrives. It exists because ~fifty in-world reads
//!    (kill-XP split, quest credit, loot rules, `/p` chat, the party's dungeon binding) resolve
//!    membership locally on the hot path and must not become cross-database calls. Same relationship
//!    `game_account`/`game_session` have had with realm-core from the start.
//! 3. **A single-database gateway** has no realm-core to route to ([`WorldStore::realm_store`]
//!    answers `None`), so every op takes the pre-realm-core path: the player's own connection, the
//!    player-facing reducer, the shard's own tables. Byte-identical, and pinned by
//!    `an_unsharded_gateway_runs_every_party_op_on_the_players_own_shard`.
//!
//! # What did NOT move
//!
//! `/say`-range chat, `/yell` and targeted emotes stay on the world shards as AOI-scoped events:
//! they ARE spatial, which is the same rule that moved membership off them. Party (`/p`) chat still
//! rides the shard's own `game_group_event` relay against the local mirror — non-proximity chat is a
//! later slice of realm-core's social & economy work, and this one deliberately does not touch it.

use anyhow::Result;

use super::{send, Outbound, SessionTx, WorldStore};
use crate::codec;
use lyracore_shared::group::realm_op;
use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;

/// One party, as the database that holds it sees it. Read from realm-core it is the authority; read
/// from a world shard it is that shard's mirror. Names and online flags are deliberately NOT in it —
/// realm-core has no character rows to resolve either from, so they are filled at render time from
/// the shards ([`render_list`]).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct GroupRoster {
    pub group_id: u64,
    pub leader_guid: u64,
    pub loot_method: u8,
    pub loot_threshold: u8,
    pub master_looter_guid: u64,
    /// Member guids in join order (member-row id), which is the order leadership succeeds in.
    pub members: Vec<u64>,
}

impl GroupRoster {
    /// The empty roster for a group that no longer exists — what [`sync_mirrors`] pushes to make a
    /// shard forget a disbanded party. `members` empty is the disband signal `sync_group_mirror`
    /// reads, so the other fields are irrelevant and left at zero.
    pub fn disbanded(group_id: u64) -> Self {
        Self {
            group_id,
            ..Default::default()
        }
    }
}

/// One party op, in the client's own vocabulary. The `u8`/`u64` argument packing into
/// `realm_group_op`'s five slots happens once, in [`run`], against the shared
/// [`lyracore_shared::group::realm_op`] contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// `CMSG_GROUP_INVITE`, target already resolved to a guid.
    Invite(u64),
    /// `CMSG_GROUP_ACCEPT`.
    Accept,
    /// `CMSG_GROUP_DECLINE`.
    Decline,
    /// `CMSG_GROUP_DISBAND` — the client's "Leave Party".
    Leave,
    /// `CMSG_GROUP_UNINVITE`, target already resolved to a guid.
    Uninvite(u64),
    /// `CMSG_LOOT_METHOD`.
    LootMethod {
        setting: u8,
        master: u64,
        threshold: u8,
    },
}

/// Resolve a typed player name to a guid ACROSS every connected shard.
///
/// **This is the read that made a cross-shard invite impossible.** `/invite Bob` resolves the typed
/// name against `game_character`, on ONE database — so a Bob who had walked into Deadmines simply
/// did not exist to a player standing in Elwynn, and the invite failed as `BadPlayerName` before any
/// party logic ran (observed live, 2026-07-25). Realm-core cannot answer it either: it owns
/// membership, not characters. The union is the answer, and it is the same shape `characters()`
/// already uses to build the character-select list across shards.
///
/// The session's OWN handle is asked first and `world_stores()` is empty on a single-database
/// gateway, so an unsharded lookup is exactly the one call it always was.
pub(crate) fn resolve_by_name<St: WorldStore + ?Sized>(
    store: &St,
    name: &str,
) -> Result<Option<u64>> {
    if let Some(guid) = store.character_guid_by_name(name)? {
        return Ok(Some(guid));
    }
    for shard in store.world_stores() {
        if let Some(guid) = shard.character_guid_by_name(name)? {
            return Ok(Some(guid));
        }
    }
    Ok(None)
}

/// [`resolve_by_name`] without the first-hit short-circuit: EVERY guid the name resolves to, across
/// every connected shard (found during the whisper slice's own review).
///
/// **Character names are not realm-unique.** The uniqueness constraint `create_character` leans on is
/// a per-database index, so the same name can exist on two shards at once — observed live on
/// 2026-07-25, two characters called `dfsdfsd`, guid 5 on the instances shard and guid 8 on the world
/// shard, after a suite re-created one the single-database gateway could not see. First-hit-wins then
/// resolves a typed name to whichever homonym happens to sit on the SENDER's own shard: two players
/// typing the same name reach two different people, and a private whisper goes to a stranger.
///
/// This cannot be fixed here — a realm-wide name constraint is the fix, and it belongs to the
/// broader realm-core social & economy effort — but a caller that knows which candidate it wants
/// can pick it. [`super::whisper`] wants the one that is ONLINE, which is what `/w` addresses in
/// vanilla. Order is the union's: this handle first, then `ShardMap::shards()` order (default
/// first), so the choice among several ONLINE homonyms is still arbitrary — just no longer
/// silently wrong whenever exactly one of them is live.
///
/// Deduped, because `world_stores()` includes the asking shard (`Coordinator::all_shards` does).
pub(crate) fn resolve_all_by_name<St: WorldStore + ?Sized>(
    store: &St,
    name: &str,
) -> Result<Vec<u64>> {
    let mut guids = Vec::new();
    if let Some(guid) = store.character_guid_by_name(name)? {
        guids.push(guid);
    }
    for shard in store.world_stores() {
        if let Some(guid) = shard.character_guid_by_name(name)? {
            if !guids.contains(&guid) {
                guids.push(guid);
            }
        }
    }
    Ok(guids)
}

/// [`resolve_by_name`]'s existence twin: a character's `(online, level, class, zone)` from whichever
/// shard holds it. Same first-hit-wins union, same unsharded short-circuit.
///
/// The `online` field here is `game_character.online` — the SESSION flag. That is NOT the module's
/// notion of online (see [`live_anywhere`]), so this answers "does this character exist" and nothing
/// else for the invite gate.
pub(crate) fn presence<St: WorldStore + ?Sized>(
    store: &St,
    guid: u64,
) -> Result<Option<(bool, u8, u8, u32)>> {
    if let Some(p) = store.character_presence(guid)? {
        return Ok(Some(p));
    }
    for shard in store.world_stores() {
        if let Some(p) = shard.character_presence(guid)? {
            return Ok(Some(p));
        }
    }
    Ok(None)
}

/// [`resolve_by_name`] inverted: the character row for `guid` from whichever connected shard holds
/// it. Same first-hit-wins union, same unsharded short-circuit — one cache read when
/// `world_stores()` is empty.
///
/// Two callers, and the second is why this is not private to the party frame: [`render_list`] needs a
/// party member's NAME (realm-core has no character rows to render one from), and `CMSG_NAME_QUERY`
/// needs it for a guid the client has met across a boundary — the whisper slice's case. A whisper
/// from another shard arrives carrying the sender's GUID (the client resolves names itself, via
/// NAME_QUERY), so a shard-local answer there would deliver the line with nobody's name on it.
pub(crate) fn character_anywhere<St: WorldStore + ?Sized>(
    store: &St,
    guid: u64,
) -> Result<Option<codec::CharacterView>> {
    if let Some(c) = store.character_by_guid(guid)? {
        return Ok(Some(c));
    }
    for shard in store.world_stores() {
        if let Some(c) = shard.character_by_guid(guid)? {
            return Ok(Some(c));
        }
    }
    Ok(None)
}

/// Does `guid` have a LIVE ENTITY on any connected shard — the module's own "is the target online"
/// gate (`game_world_entity`), unioned across the boundary.
///
/// NOT `game_character.online`. The two disagree for exactly the case the module's own gate calls
/// out: a **session-less playerbot** is inserted straight into `game_world_entity` by its spawn
/// reducer and never runs `player_login`, so its character row keeps `online = false` for its whole
/// life. Gating the realm-core invite on the session flag would refuse every bot invite on a
/// multi-database gateway while the single-database plane still accepted it — a behaviour change
/// wearing a refactor's clothes, in the one gate this slice moved out of the module.
///
/// Same shape [`render_list`] already uses for the roster's online column, for the same reason.
pub(crate) fn live_anywhere<St: WorldStore + ?Sized>(store: &St, guid: u64) -> bool {
    store.entity_in_world(guid) || store.world_stores().iter().any(|s| s.entity_in_world(guid))
}

/// Is `guid` in the world with **nobody at the keyboard** — i.e. a session-less playerbot?
///
/// It is a live `game_world_entity` AND `game_character.online == false`: the exact pair of facts
/// the group slice had to separate, read together. The module writes them in ONE transaction —
/// `player_login` sets the session flag in the same transaction that inserts the entity, and the
/// logout persist clears it in the same transaction that removes it (`module/src/world.rs`) — while
/// a playerbot is in the split state for its whole life, because `playerbots_spawn` materialises
/// the entity through `build_player_entity` and never runs `player_login`.
///
/// **BOTH READS COME OFF THE SAME DATABASE, and that is the whole correctness argument.** The
/// atomicity above is per-database, so pairing a UNION over the entity tables with a first-hit-wins
/// [`presence`] over `game_character` compares two different shards' answers — and a stale character
/// row on any OTHER connected shard then reads a live, logged-in player as session-less. That is not
/// hypothetical: `init` seeds character guid 1 ("Tester") into EVERY database it is published to, so
/// on the three-database stack a player logged in as guid 1 on `lyracore` also has an
/// `online = false` row sitting on `lyracore-instances` — and an inviter standing inside a dungeon
/// resolves the flag off that copy. Answering an invite for a real player is an impersonation, so the
/// session flag is read on the shard that actually HOLDS the live entity, and nowhere else (caught
/// in the bot-invite fix's own adversarial review). No row there at all ⇒ not session-less: this
/// refuses rather than guesses.
///
/// Why the gateway asks at all: a bot has no client, so nothing answers the group-invite dialog for
/// it. On a SINGLE-database gateway the module answers in-transaction — `invite_core` fires the
/// `on_group_invite` hook and the playerbots package accepts through it — but this slice moved the
/// invite onto REALM-CORE, where `pkg_playerbots_bot` is empty, so that hook is a no-op there and a
/// player's invite to a bot hung until the 2-minute GC (observed live 2026-07-26). The gateway is
/// the only party that can see both databases, so it is the only party that can notice.
///
/// `pkg_playerbots_bot` itself is not an option: it is a PRIVATE package table, so the gateway has no
/// subscription to it (and giving the routing layer a package dependency to answer a question the
/// public tables already answer would be the wrong trade).
///
/// The predicate is "session-less live entity", not "row in the playerbots table", so it also answers
/// TRUE for a character materialised by `debug_spawn_player_entity` — a durable row with a live entity
/// and no session. That is deliberate: such a character has no client either, so nobody else can
/// answer its dialog. It is stated because `debug_reducers` IS published live, so "only playerbots
/// reach this" would be false.
pub(crate) fn session_less_in_world<St: WorldStore + ?Sized>(store: &St, guid: u64) -> bool {
    // The asking handle first (the shard most targets are on), then every other connected one — the
    // same order + short-circuit `presence`/`resolve_by_name` use, except that here finding the
    // ENTITY is what selects whose session flag to trust.
    if store.entity_in_world(guid) {
        return matches!(store.character_presence(guid), Ok(Some((false, ..))));
    }
    for shard in store.world_stores() {
        if shard.entity_in_world(guid) {
            return matches!(shard.character_presence(guid), Ok(Some((false, ..))));
        }
    }
    false
}

/// Answer the pending invite of a session-less playerbot, on the database that holds it.
///
/// Cadence: none — this runs SYNCHRONOUSLY inside the invite op, so there is no new scheduled tick,
/// no poll, and no staggering to tune. (The original bug report asked for a staggered window on the
/// playerbots goal tick; a poll there would have read the shard's own `game_group_invite`, which on
/// a sharded deployment is never written — the invite lives on realm-core.)
///
/// The bot acts as ITSELF: `realm_group_op`'s `actor` slot carries the bot's own guid for both the
/// accept and the decline, never the inviter's — the impersonation hazard this slice already hit once.
///
/// A refusal DECLINES rather than walking away. Every accept gate in the module (`already in a group`,
/// `group full`, `inviter no longer leads`) returns `Err`, which rolls its transaction back and leaves
/// the invite row standing — so ignoring the refusal would leave the dialog hanging, which is
/// indistinguishable from the bug this fixes. The decline consumes the invite and pushes
/// `SMSG_GROUP_DECLINE` at the inviter.
///
/// Best-effort, like the mirror push: the player's invite has already committed on the authority, and
/// a bot that could not join must not turn the player's own invite into an error.
///
// Note: no bot-side policy is consulted, because there is none to consult — the bot-to-bot path
// (`brain.rs`'s `playerbots_auto_accept`) accepts any invite to any bot, and the party-cap /
// one-pending-invite gates it relies on live in `accept_invite_for`, which runs here too.
fn answer_for_session_less<St: WorldStore + ?Sized>(store: &St, realm: &dyn WorldStore, guid: u64) {
    if !session_less_in_world(store, guid) {
        return; // a real player's own client answers its own dialog
    }
    match realm.realm_group_op(realm_op::ACCEPT, guid, 0, 0, 0) {
        Ok(()) => log::info!("party: session-less {guid} accepted its group invite"),
        Err(e) => {
            log::info!("party: session-less {guid} cannot join ({e:#}) — declining explicitly");
            if let Err(e) = realm.realm_group_op(realm_op::DECLINE, guid, 0, 0, 0) {
                log::warn!(
                    "party: session-less {guid} could neither join nor decline ({e:#}) — the \
                     inviter's dialog stands until realm-core's invite GC reaps it"
                );
            }
        }
    }
}

/// Run one party op for the session that owns `self_guid`.
///
/// Unsharded → the pre-realm-core path, verbatim: the player's own connection calls the player-facing
/// reducer on the player's own shard, and nothing else happens.
///
/// Sharded → realm-core runs the op, then every connected world shard's mirror is refreshed. The
/// mirror refresh is best-effort BY DESIGN (see [`sync_mirrors`]); the op's own result is not.
pub(crate) fn run<St: WorldStore + ?Sized>(
    store: &St,
    account_id: u64,
    self_guid: u64,
    op: Op,
) -> Result<()> {
    let Some(realm) = store.realm_store() else {
        return match op {
            Op::Invite(target) => store.group_invite(account_id, self_guid, target),
            Op::Accept => store.group_accept(account_id, self_guid),
            Op::Decline => store.group_decline(account_id, self_guid),
            Op::Leave => store.group_leave(account_id, self_guid),
            Op::Uninvite(target) => store.group_uninvite(account_id, self_guid, target),
            Op::LootMethod {
                setting,
                master,
                threshold,
            } => store.group_loot_method(account_id, self_guid, setting, master, threshold),
        };
    };
    // The two gates realm-core cannot run for itself, because the directory database holds neither
    // characters nor live entities: does the target EXIST, and is it ONLINE. The gateway is the only
    // party that can answer them across a boundary — which is precisely the bug being fixed — and it
    // answers with the module's own error strings so the gateway's `PartyResult` classifier maps
    // them exactly as it maps the shard plane's.
    //
    // Each gate is the module's own read, unioned across the shards — EXISTS is a `game_character`
    // row ([`presence`]), ONLINE is a `game_world_entity` row ([`live_anywhere`]). Reading the
    // session flag for the second would silently refuse every playerbot; see [`live_anywhere`].
    if let Op::Invite(target) = op {
        if presence(store, target)?.is_none() {
            anyhow::bail!("no such player");
        }
        if !live_anywhere(store, target) {
            anyhow::bail!("player not online");
        }
    }
    // The group this character was in BEFORE the op — the only way to reach the party they may have
    // just left (their membership row is gone by the time we look again, but the members still in it
    // need their mirrors updated too).
    let before = realm.group_roster(self_guid)?.map(|r| r.group_id);
    // Found in adversarial review: LEAVE/UNINVITE are the only two ops that can shrink a group
    // below 2 members and reach `remove_member`'s disband branch on realm-core — and that branch
    // force-resolves live loot rolls, which the periodic loot-roll relay may not have promoted yet. A
    // roll staged in the gap between its kill-time creation and its next scheduled promotion is
    // invisible to `remove_member` if a disband lands in that gap ("someone gets kicked right after a
    // kill"), and the relay then promotes an ORPHANED roll onto a group id that no longer exists,
    // which resolves only at the 60s deadline — exactly the fallback the operator's decision
    // rejected, reintroduced in a narrow window. Flushing HERE, synchronously, in-line with the
    // dispatch below (not on the relay's own timer), closes it: see `loot::flush_pending_promotions`.
    if matches!(op, Op::Leave | Op::Uninvite(_)) {
        crate::world::loot::flush_pending_promotions(store, realm.as_ref());
    }
    let (code, target, arg_a, arg_b) = match op {
        Op::Invite(target) => (realm_op::INVITE, target, 0, 0),
        Op::Accept => (realm_op::ACCEPT, 0, 0, 0),
        Op::Decline => (realm_op::DECLINE, 0, 0, 0),
        Op::Leave => (realm_op::LEAVE, 0, 0, 0),
        Op::Uninvite(target) => (realm_op::UNINVITE, target, 0, 0),
        Op::LootMethod {
            setting,
            master,
            threshold,
        } => (realm_op::LOOT_METHOD, master, setting, threshold),
    };
    realm.realm_group_op(code, self_guid, target, arg_a, arg_b)?;
    // Nobody is at the keyboard of a playerbot, so nobody answers its dialog. Done
    // BEFORE the mirror push, so the ONE push that follows already carries the bot as a member —
    // which is what the shard's own party reads (kill-XP split, `/p`, follow-the-leader) need.
    if let Op::Invite(target) = op {
        answer_for_session_less(store, realm.as_ref(), target);
    }
    sync_mirrors(store, realm.as_ref(), self_guid, before);
    Ok(())
}

/// Run a SERVER-DRIVEN invite with no client behind it — a playerbot's serendipity pick, closing
/// the gap the group slice opened: the module used to write this shard's LOCAL
/// `game_group`/`game_group_member` rows directly, which the next `sync_group_mirror` push wiped
/// because realm-core had never heard of them.
///
/// There is no `account_id` here on purpose: a bot has no per-account connection for a reducer to
/// authenticate as, on EITHER topology. So this never takes [`run`]'s unsharded arm (which needs
/// exactly that connection to resolve the acting character via `ctx.sender()`) — it always uses the
/// guid-based `realm_group_op`, targeting realm-core when the gateway is sharded and this database's
/// OWN `game_group`/`game_group_member` otherwise. Unsharded that is still correct, not a special
/// case: with one database, "realm-core" and "the world shard" are the same physical database, so
/// writing there directly cannot diverge from itself — there is no mirror to contradict.
///
/// Otherwise this mirrors [`run`]'s sharded arm: same existence/online gates, same
/// [`answer_for_session_less`] (a bot target still has no client to answer its own dialog — true of
/// the INVITER here too, which is the whole point), same [`sync_mirrors`] push so the shard's own
/// party reads (kill-XP split, `/p`, follow-the-leader) see the new member immediately.
pub(crate) fn run_bot_invite<St: WorldStore>(
    store: &St,
    inviter_guid: u64,
    target_guid: u64,
) -> Result<()> {
    let owned_realm;
    let realm: &dyn WorldStore = match store.realm_store() {
        Some(r) => {
            owned_realm = r;
            owned_realm.as_ref()
        }
        None => store,
    };
    if presence(store, target_guid)?.is_none() {
        anyhow::bail!("no such player");
    }
    if !live_anywhere(store, target_guid) {
        anyhow::bail!("player not online");
    }
    let before = realm.group_roster(inviter_guid)?.map(|r| r.group_id);
    realm.realm_group_op(realm_op::INVITE, inviter_guid, target_guid, 0, 0)?;
    answer_for_session_less(store, realm, target_guid);
    sync_mirrors(store, realm, inviter_guid, before);
    Ok(())
}

/// Push realm-core's roster for every party this op could have touched onto every connected world
/// shard.
///
/// Two groups at most: the one the character is in NOW and the one they were in BEFORE (a leave, a
/// kick, or an accept that moved them). A group that realm-core no longer has is pushed as
/// [`GroupRoster::disbanded`], which is how a shard learns to drop it.
///
/// **Best-effort, deliberately.** The authoritative write has already committed on realm-core by the
/// time this runs, so a failed mirror push must not turn a party op that DID happen into an error
/// the client renders as a failure. The cost of a miss is bounded and self-healing: that shard's
/// local reads (XP split, loot rules) use a stale roster until the next op or the next world entry
/// re-pushes it, and the party FRAME — which is what the player sees — is rendered from realm-core
/// through the relay, not from the mirror.
pub(crate) fn sync_mirrors<St: WorldStore + ?Sized>(
    store: &St,
    realm: &dyn WorldStore,
    self_guid: u64,
    before: Option<u64>,
) {
    let now = match realm.group_roster(self_guid) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("party: could not read the realm-core roster for {self_guid} ({e:#}) — the shard mirrors keep their previous roster until the next op or world entry");
            return;
        }
    };
    let mut touched: Vec<u64> = Vec::new();
    if let Some(r) = &now {
        touched.push(r.group_id);
    }
    if let Some(b) = before {
        if !touched.contains(&b) {
            touched.push(b);
        }
    }
    for group_id in touched {
        let roster = match realm.group_roster_by_id(group_id) {
            Ok(Some(r)) => r,
            // Gone from the authority = disbanded. Push the tombstone rather than skipping, or the
            // shards keep a party that no longer exists and its members stay grouped locally.
            Ok(None) => GroupRoster::disbanded(group_id),
            Err(e) => {
                log::warn!("party: could not read realm-core group {group_id} ({e:#}) — shard mirrors unchanged");
                continue;
            }
        };
        for shard in store.world_stores() {
            if let Err(e) = shard.sync_group_mirror(&roster) {
                log::warn!(
                    "party: could not mirror group {group_id} onto shard {} ({e:#}) — that shard's \
                     in-world party reads stay stale until the next op or world entry",
                    shard.shard_name()
                );
            }
        }
    }
}

/// World entry (login, and every cross-shard arrival): put the party the player is actually in onto
/// the shard they just entered, and re-render their party frame.
///
/// **This is what carries a party across a shard boundary now that the escrowed transfer's blob
/// mirror is gone.** The
/// blob could only carry the membership the character had when it stepped into the portal; this
/// reads the authority at the moment of arrival, so a party formed — or joined, or left — while the
/// player was on the loading screen is what lands.
///
/// Unsharded → returns immediately, before any read: the shard's own tables already are the
/// authority and the login path is unchanged.
pub(crate) fn on_world_entry<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    self_guid: u64,
) -> Result<()> {
    let Some(roster) = sync_arrival_mirror(store, self_guid)? else {
        return Ok(());
    };
    send(tx, Outbound::One(render_list(store, self_guid, &roster)))
}

/// The mirror half of [`on_world_entry`], without a client: put the party realm-core says
/// `self_guid` is in onto the shard `store` names, and clear a mirror row of a party they have
/// left. Answers the roster it pushed, so the caller can render a frame for a player — and so a
/// session-less arrival (`transfer::run_bot_transfer`) can call the same code and render nothing.
///
/// Unsharded → `Ok(None)` before any read: the shard's own tables already are the authority.
pub(crate) fn sync_arrival_mirror<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: u64,
) -> Result<Option<GroupRoster>> {
    let Some(realm) = store.realm_store() else {
        return Ok(None);
    };
    let roster = realm.group_roster(self_guid)?;
    // A mirror on THIS shard that still has the arriving character in a party realm-core no longer
    // has them in is the ONE staleness "re-syncs on the next op or world entry" does not cover by
    // itself: with no roster to push there was nothing to overwrite it with, and the ops of the party
    // they left never name them again — so the stale membership row survived every arrival, forever.
    // The shard then runs that character's kill-XP split, quest credit, loot rules, `/p` chat and
    // dungeon binding against a party they are not in. Re-push the AUTHORITY's version of the group
    // the mirror thinks they are in (a tombstone if it is gone) — the same repair `sync_mirrors`
    // applies to the group an actor was in BEFORE an op.
    if let Some(stale) = store.group_roster(self_guid)? {
        if roster.as_ref().is_none_or(|r| r.group_id != stale.group_id) {
            let repair = realm
                .group_roster_by_id(stale.group_id)?
                .unwrap_or_else(|| GroupRoster::disbanded(stale.group_id));
            // Best-effort, like every other mirror write: this is a cache repair, and failing the
            // world entry over it would be strictly worse than arriving with a stale roster.
            if let Err(e) = store.sync_group_mirror(&repair) {
                log::warn!(
                    "party: could not clear guid {self_guid} from shard {}'s stale mirror of group \
                     {} ({e:#}) — that shard's party reads for them stay wrong until they join one",
                    store.shard_name(),
                    stale.group_id
                );
            }
        }
    }
    let Some(roster) = roster else {
        return Ok(None);
    };
    // The shard the player just entered. `store` is already the session's home-shard handle (every
    // world-entry caller runs under `on_home_shard!`), so this is the one mirror that must exist
    // before the player takes a single action here.
    store.sync_group_mirror(&roster)?;
    Ok(Some(roster))
}

/// Build `SMSG_GROUP_LIST` for `self_guid` from an authoritative roster, filling each member's NAME
/// and ONLINE flag from the shards.
///
/// The fill is the price of realm-core owning membership: the directory database has no
/// `game_character` rows, so it cannot know what its members are called. The gateway can — it reads
/// every connected shard's cache — and it is the only party that can answer for a member standing on
/// a different database than the viewer. A member whose name will not resolve (a shard that is down)
/// renders with an empty name rather than being dropped from the frame: a missing row in the party
/// UI reads as "they left", which is a worse lie than a blank one.
pub(crate) fn render_list<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: u64,
    roster: &GroupRoster,
) -> ServerOpcodeMessage {
    let shards = store.world_stores();
    let members: Vec<(u64, String, bool)> = roster
        .members
        .iter()
        .map(|&guid| {
            // This handle first (the viewer's own shard, where most of the party is), then every
            // other connected one — the member standing inside an instance is on a database this
            // handle never reads. Empty on a single-database gateway, so the union never leaves it.
            let name = character_anywhere(store, guid)
                .ok()
                .flatten()
                .map(|c| c.name);
            let online =
                store.entity_in_world(guid) || shards.iter().any(|s| s.entity_in_world(guid));
            (guid, name.unwrap_or_default(), online)
        })
        .collect();
    ServerOpcodeMessage::SMSG_GROUP_LIST(Box::new(codec::build_group_list(
        self_guid,
        roster.leader_guid,
        roster.loot_method,
        roster.loot_threshold,
        roster.master_looter_guid,
        &members,
    )))
}
