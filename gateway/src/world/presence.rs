//! Realm Presence — the Gateway's realm-wide read of one Character: in world, session online,
//! race, class, level, zone and Away Status, from whichever World Shard holds it. Guild rosters,
//! friends, `/who`, whisper and Member Stats all read it here, instead of each resolving its own
//! realm-wide Character read the way `party.rs` and `handlers/member_stats.rs` used to.
//!
//! Existence alone (a durable `game_character` row) never needs proof of absence: it is a positive
//! signal, read best-effort from whichever connected Shard answers first. A NEGATIVE claim —
//! `Whereabouts::Offline`, or [`of`] answering `None` for a guid with no row anywhere — needs every
//! configured World Shard to vouch that it genuinely has nothing, because a Shard whose Coordinator
//! subscription has gone stale could be hiding the Character. [`WorldStore::every_shard_vouches_for_absence`]
//! is that gate, and [`of`] never returns either without it succeeding first.

use anyhow::Result;

use super::WorldStore;
use crate::codec;

/// One Character's Realm Presence, as whichever World Shard holds it reports it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RealmPresence {
    pub guid: u64,
    pub name: String,
    pub race: u8,
    pub class: u8,
    /// From the live entity when [`Whereabouts::InWorld`] (current); otherwise the durable
    /// `game_character` row, which only `persist_entity` refreshes (logout, cross-map teleport,
    /// Transfer) — stale by however long the Character has been away.
    pub level: u8,
    pub zone_id: u32,
    /// `game_character.online`, the session flag. Bots never set it. A frozen pre-Transfer copy
    /// keeps this `true` with no live entity anywhere (`begin_transfer` persists with
    /// `set_offline: false`) — read [`Whereabouts`] for whether the Character is actually here,
    /// never this flag alone.
    pub session_online: bool,
    pub whereabouts: Whereabouts,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Whereabouts {
    /// A live `game_world_entity` on some World Shard. Bots count.
    InWorld {
        away: AwayStatus,
        entity: codec::MemberEntity,
        /// [`WorldStore::shard_name`] of the Shard the live entity answered from — Member Stats'
        /// own aura and pet reads key off it.
        shard_name: String,
    },
    /// No live entity, but the Character is between two places: a pending Transfer (Realm-core's
    /// own signal), a bot's Transfer Intent, or a Shard's own Character row reading online with no
    /// entity there (a map-change loading screen, or the frozen source copy of a human Transfer).
    /// Reporting either offline would flicker the party frame and Member Stats mid-crossing.
    InTransit,
    /// No live entity anywhere, not between places, and every configured World Shard vouches for
    /// the absence.
    Offline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AwayStatus {
    None,
    Afk,
    Dnd,
}

/// A live entity's Away Status from its `PLAYER_FLAGS`, with
/// [`lyracore_shared::chat::chat_tag_for`] semantics: DND wins when both bits are set.
pub(crate) fn away_from_player_flags(player_flags: u32) -> AwayStatus {
    use lyracore_shared::chat::chat_tag;
    match lyracore_shared::chat::chat_tag_for(player_flags) {
        chat_tag::DND => AwayStatus::Dnd,
        chat_tag::AFK => AwayStatus::Afk,
        _ => AwayStatus::None,
    }
}

/// A durable `game_character` row's identity and session flag, from one Shard — [`of`]'s existence
/// signal, before it knows whether the Character is in world, in transit, or offline.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CharacterIdentity {
    pub guid: u64,
    pub name: String,
    pub race: u8,
    pub class: u8,
    pub level: u8,
    pub zone_id: u32,
    pub session_online: bool,
}

impl CharacterIdentity {
    /// A guid with no known durable row — [`of`]'s fallback when a positive signal (a live entity,
    /// or a between-places match) fires for a Character no connected Shard's `game_character`
    /// happened to name, such as a bot whose Transfer Intent exists before its row does.
    fn unknown(guid: u64) -> Self {
        Self {
            guid,
            name: String::new(),
            race: 0,
            class: 0,
            level: 0,
            zone_id: 0,
            session_online: false,
        }
    }

    /// Finish into a [`RealmPresence`] that keeps this identity's own level and zone — the
    /// `InTransit` and `Offline` cases, which have no live entity to read fresher ones from.
    fn stationary(self, whereabouts: Whereabouts) -> RealmPresence {
        RealmPresence {
            guid: self.guid,
            name: self.name,
            race: self.race,
            class: self.class,
            level: self.level,
            zone_id: self.zone_id,
            session_online: self.session_online,
            whereabouts,
        }
    }

    /// Finish into a [`RealmPresence`] with level and zone read fresh from a live entity.
    fn in_world(self, shard_name: String, entity: codec::MemberEntity) -> RealmPresence {
        let level = u8::try_from(entity.level).unwrap_or(u8::MAX);
        let zone_id = entity.zone_id;
        let away = away_from_player_flags(entity.player_flags);
        RealmPresence {
            guid: self.guid,
            name: self.name,
            race: self.race,
            class: self.class,
            level,
            zone_id,
            session_online: self.session_online,
            whereabouts: Whereabouts::InWorld {
                away,
                entity,
                shard_name,
            },
        }
    }
}

/// `guid`'s Realm Presence from whichever connected Shard holds it.
///
/// A live entity on any connected Shard wins outright, whatever that Shard's own health — a
/// stale-but-connected cache still answers from its last-known state, same as `entity_in_world`.
/// Failing that, a pending Transfer (Realm-core's own signal, or a Shard's between-places match)
/// answers `InTransit`. Only once neither fires does the answer become a negative claim —
/// `Offline`, or `None` for a guid no Shard ever named — and a negative claim needs every
/// configured Shard to vouch first (see the module doc).
pub(crate) fn of<St: WorldStore + ?Sized>(store: &St, guid: u64) -> Result<Option<RealmPresence>> {
    let identity = character_identity_anywhere(store, guid)?;

    if let Some((shard_name, entity)) = live_entity_anywhere(store, guid) {
        let identity = identity.unwrap_or_else(|| CharacterIdentity::unknown(guid));
        return Ok(Some(identity.in_world(shard_name, entity)));
    }
    if realm_transfer_pending(store, guid)? {
        let identity = identity.unwrap_or_else(|| CharacterIdentity::unknown(guid));
        return Ok(Some(identity.stationary(Whereabouts::InTransit)));
    }

    // Both `InTransit` (via a Shard's own between-places match) and `Offline` are negative claims
    // from here on: no configured Shard may be unreachable, or a stale cache could be hiding the
    // Character.
    store.every_shard_vouches_for_absence()?;
    let between_places = store.character_in_transit(guid)
        || store
            .world_stores()
            .iter()
            .any(|shard| shard.character_in_transit(guid));

    Ok(match (identity, between_places) {
        (Some(identity), true) => Some(identity.stationary(Whereabouts::InTransit)),
        (Some(identity), false) => Some(identity.stationary(Whereabouts::Offline)),
        (None, true) => Some(CharacterIdentity::unknown(guid).stationary(Whereabouts::InTransit)),
        (None, false) => None,
    })
}

/// [`CharacterIdentity`] from whichever connected Shard answers first: this handle, then every
/// `world_stores()` peer. Best-effort — a hit is a positive signal that needs no health proof.
///
/// `pub(crate)` because it is also the best-effort race read a caller reaches for when it must
/// NEVER end a World Session or a Relay pump over an unreachable Shard, but a wrong guessed race
/// would be worse than answering nothing: `world::social::resolve_add_contact` (a friend add's
/// Enemy Gate) and `stdb::world_view::claim_edge_outcome` (the Account Claim Relay's OFFLINE
/// edge) both read it in place of the full, health-checked [`of`].
pub(crate) fn character_identity_anywhere<St: WorldStore + ?Sized>(
    store: &St,
    guid: u64,
) -> Result<Option<CharacterIdentity>> {
    if let Some(row) = store.character_identity(guid)? {
        return Ok(Some(row));
    }
    for shard in store.world_stores() {
        if let Some(row) = shard.character_identity(guid)? {
            return Ok(Some(row));
        }
    }
    Ok(None)
}

/// A live entity from whichever connected Shard answers first, paired with that Shard's name.
fn live_entity_anywhere<St: WorldStore + ?Sized>(
    store: &St,
    guid: u64,
) -> Option<(String, codec::MemberEntity)> {
    if let Some(entity) = store.live_entity(guid) {
        return Some((store.shard_name().to_string(), entity));
    }
    store.world_stores().iter().find_map(|shard| {
        shard
            .live_entity(guid)
            .map(|e| (shard.shard_name().to_string(), e))
    })
}

/// Realm-core's own pending-Transfer signal for `guid`. `false` on an unsharded Gateway, which has
/// no Realm-core to ask (`realm_store()` answers `None`).
fn realm_transfer_pending<St: WorldStore + ?Sized>(store: &St, guid: u64) -> Result<bool> {
    let Some(realm) = store.realm_store() else {
        return Ok(false);
    };
    Ok(realm
        .realm_character_partition(guid)?
        .is_some_and(|partition| partition.transfer_pending))
}

/// The union of every connected Shard's in-world players, deduplicated by guid — `/who`'s ultimate
/// source. Scans each Shard exactly once: `world_stores()` already includes a handle for this
/// Shard's own database once there is more than one connected, so this handle is asked directly
/// only when `world_stores()` is empty (unsharded).
pub(crate) fn in_world_characters<St: WorldStore + ?Sized>(
    store: &St,
) -> Result<Vec<RealmPresence>> {
    let peers = store.world_stores();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    if peers.is_empty() {
        for row in store.in_world_players()? {
            if seen.insert(row.guid) {
                out.push(row);
            }
        }
    } else {
        for shard in &peers {
            for row in shard.in_world_players()? {
                if seen.insert(row.guid) {
                    out.push(row);
                }
            }
        }
    }
    Ok(out)
}

/// Resolve a typed Character name to a guid across every connected Shard: this handle first, then
/// every `world_stores()` handle, first hit wins. `world_stores()` is empty on an unsharded
/// Gateway, so this is the one cache read it always was.
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

/// [`resolve_by_name`] without the first-hit short-circuit: every guid `name` resolves to, across
/// every connected Shard. Character names are unique per Shard, not per Realm, so the same name
/// can name two different Characters at once; a caller that needs one picks among the candidates
/// (see [`super::whisper`]). Deduped, because `world_stores()` includes the asking Shard.
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

/// [`resolve_by_name`] inverted: the Character row for `guid` from whichever connected Shard holds
/// it. Same first-hit-wins union, same unsharded short-circuit.
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

/// Does `guid` have a live entity on any connected Shard — `game_world_entity`, unioned across the
/// boundary. NOT `game_character.online`: a session-less playerbot is inserted straight into
/// `game_world_entity` and never runs `player_login`, so its session flag stays false for its
/// whole life. [`of`] exposes both, as `Whereabouts::InWorld` and `session_online`; this is the
/// `in_world`-only shortcut a caller that does not need the rest of [`RealmPresence`] keeps using.
pub(crate) fn live_anywhere<St: WorldStore + ?Sized>(store: &St, guid: u64) -> bool {
    store.entity_in_world(guid) || store.world_stores().iter().any(|s| s.entity_in_world(guid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn away_from_player_flags_prefers_dnd_and_defaults_none() {
        use lyracore_shared::constants::player_flags;

        assert_eq!(away_from_player_flags(0), AwayStatus::None);
        assert_eq!(away_from_player_flags(player_flags::AFK), AwayStatus::Afk);
        assert_eq!(away_from_player_flags(player_flags::DND), AwayStatus::Dnd);
        assert_eq!(
            away_from_player_flags(player_flags::AFK | player_flags::DND),
            AwayStatus::Dnd,
            "DND wins when both bits are set"
        );
    }
}
