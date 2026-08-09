//! Realm-wide whispers (issue #22, WHISPER slice; spec issue #12).
//!
//! # What moved, and why here
//!
//! `/w Bob hello` resolved "Bob" against `game_character` **on the calling database** and read
//! `Bob.online` there too, inside `chat::send_whisper`. On a sharded gateway that is not a whisper
//! that fails to deliver — it is a whisper whose target *does not exist*: a Bob who walked into
//! Deadmines has no character row on the open world's database, so the whisper died as
//! "no player named Bob" before any delivery ran. Identical shape to the cross-boundary invite the
//! group slice fixed, observed live on the same Phase A run (2026-07-25), and the reason #24 lists
//! social as its blocker — whisper is the most-used cross-continent social action there is.
//!
//! This module is the ROUTING half, the same way [`super::party`] is for membership: generic over
//! [`WorldStore`], so every decision executes under test against the in-memory multi-database
//! topology #37/#38 built, and the database-specific halves stay thin (`Coordinator`'s trait impl and
//! the module's `realm_whisper`).
//!
//! # The two planes
//!
//! 1. **Realm-core** carries the whisper. Not because it owns any whisper STATE — a whisper is an
//!    event, not a fact about the world — but because it is the one database both parties can be
//!    addressed on: `game_whisper_event.recipient_guid` is realm-wide, while the bound identity the
//!    shard relay filters on is minted per (account, database) and names nobody elsewhere. Delivery
//!    rides realm-core's coordinator connection, self-filtered per session (the 277/279 law).
//! 2. **A single-database gateway** has no realm-core to route to ([`WorldStore::realm_store`]
//!    answers `None`), so a whisper takes the pre-#22 path verbatim: `send_whisper` on the player's
//!    own connection, name resolution and gates inside the module, delivery over the per-player RLS
//!    relay. Byte-identical, and pinned by
//!    `an_unsharded_gateway_whispers_through_the_players_own_reducer`.
//!
//! # The three gates that had to move, and the one that must NOT be copied from the group slice
//!
//! Realm-core holds no characters, no live entities and no contact rows, so the gateway answers
//! - **does the target exist** — [`super::party::resolve_all_by_name`], the realm-wide name union
//!   (this is the read that was broken). Every candidate, not the first: character names are unique
//!   per DATABASE, not per realm, so the ONLINE gate below doubles as the disambiguator;
//! - **is the SENDER in world** — [`super::party::live_anywhere`], the module's own
//!   `entity_by_owner` miss, unioned across the shards;
//! - **is the TARGET online** — and here the group slice's helper is the WRONG one. `send_whisper`
//!   gates on `game_character.online`, the SESSION flag, not on a live `game_world_entity` row. The
//!   two disagree for every playerbot (spawned straight into the entity table, never runs
//!   `player_login`, so `online == false` for its whole life), which means a whisper to a bot is
//!   refused today and using `live_anywhere` here would silently start ACCEPTING them on a
//!   multi-database gateway only. PR #49's review caught this defect in the mirror direction — the
//!   moved ONLINE gate reading the session flag where the module read the entity — so each gate here
//!   reads the table its own module gate read, and the bot case is pinned in both directions;
//! - **is the sender ignored by the target** — read from wherever the target's contact rows are, and
//!   passed to `realm_whisper` as its verdict. The RULE (drop the incoming line, keep the sender's
//!   echo, tell the sender nothing) stays in the module, on the shared `whisper_rows` core both
//!   planes write through.
//!
//! Every refusal carries the module's own text (`lyracore_shared::whisper`), and the gateway answers all
//! of them with the same `SMSG_CHAT_PLAYER_NOT_FOUND` it always did, so the wire is unchanged.
//!
//! # What did NOT move
//!
//! `/say`, `/yell` and targeted emotes stay shard-local and range-scoped: they are spatial by nature,
//! which is the same partition rule that moved a whisper — which is not — off the shards. `/p` chat
//! also stays, riding the shard's own `game_group_event` relay against the local mirror.

use anyhow::Result;

use super::{party, WorldStore};

/// Send one whisper for the session that owns `self_guid`.
///
/// Unsharded → the pre-#22 path, verbatim: `send_whisper` on the player's own connection with the
/// typed name, and every gate inside the module. Nothing else runs — not even the name lookup.
///
/// Sharded → the gates run here, realm-wide, and realm-core carries the whisper. `self_guid` is
/// `None` at character select; that is the module's "whisperer not in world" arm, and it must stay an
/// `Err` because the whisper's sender is the ONE argument `realm_whisper` cannot re-derive (it is the
/// whole authorization of the call — a whisper attributed to another player is impersonation, not a
/// misroute). The caller passes the guid it authenticated for that socket, never a literal.
pub(crate) fn run<St: WorldStore + ?Sized>(
    store: &St,
    account_id: u64,
    self_guid: Option<u64>,
    target_name: &str,
    message: String,
) -> Result<()> {
    let Some(realm) = store.realm_store() else {
        return store.send_whisper(account_id, self_guid.unwrap_or(0), target_name.to_string(), message);
    };
    let Some(sender_guid) = self_guid.filter(|&g| party::live_anywhere(store, g)) else {
        anyhow::bail!(lyracore_shared::whisper::NOT_IN_WORLD);
    };
    // EXISTS: the read that made a cross-shard whisper impossible. The union asks this handle first
    // and then every other connected shard, so an unsharded lookup is the one call it always was and
    // a sharded one finds the Bob standing inside Deadmines.
    //
    // ALL the candidates, not the first: character names are NOT realm-unique (the constraint is a
    // per-database index), so a name can resolve on two shards at once — live, 2026-07-25. See
    // [`party::resolve_all_by_name`].
    let candidates = party::resolve_all_by_name(store, target_name)?;
    if candidates.is_empty() {
        anyhow::bail!(lyracore_shared::whisper::no_player_named(target_name));
    }
    // ONLINE: `game_character.online`, the SESSION flag — `send_whisper`'s own gate, NOT the group
    // slice's `live_anywhere` entity test. See this module's header: they disagree for every
    // playerbot, and switching gates here would make a bot whisperable on a sharded gateway alone.
    //
    // It is also the DISAMBIGUATOR between homonyms: `/w` addresses the character that is logged in,
    // so the gate picks the candidate rather than merely judging one. With a single candidate — every
    // deployment where names really are unique — this is the same one read it was.
    let mut online_target = None;
    for guid in candidates {
        if party::presence(store, guid)?
            .map(|(online, ..)| online)
            .unwrap_or(false)
        {
            online_target = Some(guid);
            break;
        }
    }
    let Some(target_guid) = online_target else {
        anyhow::bail!(lyracore_shared::whisper::player_is_offline(target_name));
    };
    // The target's IGNORE list, read from the shard that holds their contact rows. Realm-core has
    // none, so this verdict travels with the call; the rule it feeds stays in the module.
    let sender_is_ignored = ignored_anywhere(store, target_guid, sender_guid)?;
    realm.realm_whisper(sender_guid, target_guid, message, sender_is_ignored)
}

/// Does `owner_guid` have `other_guid` on their IGNORE list, on whichever shard holds their contact
/// rows (#22, whisper slice)?
///
/// `game_character_contact` is character-owned and travels with the character (`chat.rs`'s transfer
/// sweep), so exactly one connected database has the rows — but which one is not knowable from here,
/// and the answer must be the same either way. The union is the answer, in the same shape
/// [`party::resolve_by_name`] and [`party::live_anywhere`] use, and `world_stores()` is empty on a
/// single-database gateway so this is one cache read there.
///
/// A PEER shard that cannot answer contributes `false`, never `true`. Reading an `Err` as "ignored"
/// would be the invisible catastrophe: an ignored whisper is reported to its sender as a success (that
/// is the vanilla rule), so one unreachable shard would silently swallow the recipient's line for
/// every whisper on the realm while telling nobody. The cost of the other direction is bounded and
/// visible — one whisper reaching someone who blocked the sender, for as long as their shard is down —
/// and that shard cannot be holding a live target anyway (the ONLINE gate would already have refused).
///
/// The session's OWN handle propagates its `Err` instead, because that database is the one the whole
/// session is already reading through: a failure there is not "one shard is down", and every other
/// gateway read on that handle propagates too.
pub(crate) fn ignored_anywhere<St: WorldStore + ?Sized>(
    store: &St,
    owner_guid: u64,
    other_guid: u64,
) -> Result<bool> {
    if store.contact_lists(owner_guid)?.1.contains(&other_guid) {
        return Ok(true);
    }
    Ok(store.world_stores().iter().any(|shard| {
        shard
            .contact_lists(owner_guid)
            .map(|(_, ignored)| ignored.contains(&other_guid))
            .unwrap_or(false)
    }))
}
