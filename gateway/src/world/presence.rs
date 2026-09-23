//! Realm Presence — the Gateway's realm-wide read of one Character: in world, session online,
//! race, class, level, zone and Away Status, from whichever World Shard holds it. Guild rosters,
//! friends, `/who` and whisper all read it here, instead of each resolving its own realm-wide
//! Character reads the way `party.rs` used to.

use anyhow::Result;

use super::WorldStore;
use crate::codec;

/// One Character's Realm Presence, as whichever World Shard holds it reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RealmPresence {
    pub guid: u64,
    pub name: String,
    pub race: u8,
    pub class: u8,
    pub level: u8,
    pub zone_id: u32,
    /// A live `game_world_entity` on some World Shard. Bots count.
    pub in_world: bool,
    /// `game_character.online`, the session flag. Bots never set it.
    pub session_online: bool,
    /// From the live entity's `PLAYER_FLAGS`. `AwayStatus::None` when there is no live entity.
    pub away: AwayStatus,
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

/// `guid`'s Realm Presence from whichever connected Shard holds it: this handle first, then every
/// `world_stores()` handle, first hit wins — the same union [`resolve_by_name`] and its neighbors
/// use. A hit with a live entity beats a hit without one, so a Character caught between the two
/// halves of a Transfer reads as in world.
pub(crate) fn of<St: WorldStore + ?Sized>(store: &St, guid: u64) -> Result<Option<RealmPresence>> {
    let mut fallback = None;
    if let Some(row) = store.presence_row(guid)? {
        if row.in_world {
            return Ok(Some(row));
        }
        fallback = Some(row);
    }
    for shard in store.world_stores() {
        let Some(row) = shard.presence_row(guid)? else {
            continue;
        };
        if row.in_world {
            return Ok(Some(row));
        }
        if fallback.is_none() {
            fallback = Some(row);
        }
    }
    Ok(fallback)
}

/// The union of every connected Shard's in-world players, deduplicated by guid. "Player" is
/// `entry == 0` on `game_world_entity`, [`WorldStore::in_world_players`]'s own rule.
pub(crate) fn in_world_characters<St: WorldStore + ?Sized>(
    store: &St,
) -> Result<Vec<RealmPresence>> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for row in store.in_world_players()? {
        if seen.insert(row.guid) {
            out.push(row);
        }
    }
    for shard in store.world_stores() {
        for row in shard.in_world_players()? {
            if seen.insert(row.guid) {
                out.push(row);
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
/// whole life. [`of`] exposes both, as `in_world` and `session_online`; this is the
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
