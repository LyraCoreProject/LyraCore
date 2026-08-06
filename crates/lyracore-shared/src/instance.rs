//! Which maps are DUNGEONS — the one instancing fact both tiers have to agree on (issue #48).
//!
//! The module needs it to decide whether a portal resolves-or-creates a `game_instance` instead of
//! landing at instance 0 (`module::instance::is_dungeon_map`, work-item 190 / issue #39). The
//! gateway needs the SAME set to answer "when this realm creates a dungeon instance, which database
//! will spawn its population, and does that database host populations at all"
//! (`gateway::config::ShardMap::check_instance_hosting`) — the check that turns issue #48's silent
//! empty dungeons into a startup failure.
//!
//! It lives here rather than in either tier because a *disagreement* is the failure mode: a gateway
//! that thought map 36 was open world would never check map 36's instance hosting, and the operator
//! would be back to eight consecutive dungeons with 0 entities and four bot tests blaming gameplay.
//!
//! # Why a const set and not data
//!
//! The honest source is `Map.dbc`'s instanceable flag, which no importer family loads today (nothing
//! else needs `Map.dbc`), so the set is code — one line per dungeon as content lands. Deadmines is
//! the only imported dungeon (work-item 226).
//!
//! Deliberate simplification: a hardcoded slice. Ceiling — a dungeon whose content is imported but
//! whose id is not added here silently behaves like open world (and its hosting is never checked).
//! Upgrade path is the importer loading `Map.dbc` into a table both tiers read; that costs a new
//! static table and a gateway binding to answer a question with 1 row in it today.

/// The maps that are DUNGEONS — entering an areatrigger portal targeting one of these resolves-or-
/// creates an instance instead of landing at instance 0.
///
/// Every entry MUST also have a `module::instance::entrance_fallback` arm (unit-pinned there) so a
/// reaped-instance login can never strand.
pub const DUNGEON_MAPS: &[u32] = &[36];

/// Is `map_id` a dungeon (instanced) map? See [`DUNGEON_MAPS`]. Pure.
pub fn is_dungeon_map(map_id: u32) -> bool {
    DUNGEON_MAPS.contains(&map_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dungeon_set_holds_deadmines_and_no_open_world_map() {
        assert!(is_dungeon_map(36), "Deadmines is the one imported dungeon");
        assert!(!is_dungeon_map(0), "Eastern Kingdoms is open world");
        assert!(!is_dungeon_map(1), "Kalimdor is open world");
    }

    /// The set is what the gateway's #48 hosting check iterates. An EMPTY set makes that check
    /// vacuous — it would pass for every configuration, including the one that files eight leases
    /// with 0 entities — so "there is at least one dungeon to check" is itself the invariant.
    #[test]
    fn the_dungeon_set_is_never_empty() {
        assert!(
            !DUNGEON_MAPS.is_empty(),
            "DUNGEON_MAPS is empty — the gateway's instance-hosting check (#48) iterates it, so an \
             empty set silently turns that check into a no-op"
        );
        assert!(
            !DUNGEON_MAPS.contains(&0),
            "map 0 is the open world; listing it as a dungeon would route every open-world portal \
             through the instancing path"
        );
    }
}
