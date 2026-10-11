//! Shared vanilla dungeon classification and Instance Removal countdown.

/// Vanilla dungeon and raid maps. Battlegrounds use a separate population lifecycle.
/// The client Map.dbc instance types and ClassicDB instance_template identify this set.
pub const DUNGEON_MAPS: &[u32] = &[
    33, 34, 36, 43, 47, 48, 70, 90, 109, 129, 189, 209, 229, 230, 249, 289, 309, 329, 349, 389,
    409, 429, 469, 509, 531, 533,
];

pub fn is_dungeon_map(map_id: u32) -> bool {
    DUNGEON_MAPS.contains(&map_id)
}

/// Source spawns on these maps remain dormant until their instance creates a population.
/// Battleground populations stay dormant until battleground creation is implemented.
/// Shared interiors, including the Deeprun Tram and faction halls, use instance zero.
pub const BATTLEGROUND_MAPS: &[u32] = &[30, 489, 529];

pub fn has_instance_population(map_id: u32) -> bool {
    is_dungeon_map(map_id) || BATTLEGROUND_MAPS.contains(&map_id)
}

/// The Instance Removal countdown in milliseconds (cm:Player.cpp:17723). The Module schedules the
/// expiry this far out, and the Gateway never shows the client more time than this.
pub const INSTANCE_REMOVAL_MS: u32 = 60_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dungeons_include_vanilla_raids_but_exclude_battlegrounds_and_shared_interiors() {
        assert!(is_dungeon_map(36), "Deadmines is instanced");
        for map_id in [33, 229, 249, 309, 409, 469, 509, 531, 533] {
            assert!(is_dungeon_map(map_id));
        }
        for map_id in [0, 1, 369, 449, 450] {
            assert!(!has_instance_population(map_id));
        }
        for map_id in [30, 489, 529] {
            assert!(!is_dungeon_map(map_id));
            assert!(has_instance_population(map_id));
        }
        assert!(!is_dungeon_map(0), "Eastern Kingdoms is open world");
        assert!(!is_dungeon_map(1), "Kalimdor is open world");
    }

    /// The set is what the gateway's hosting check iterates. An EMPTY set makes that check
    /// vacuous — it would pass for every configuration, including the one that files eight leases
    /// with 0 entities — so "there is at least one dungeon to check" is itself the invariant.
    #[test]
    fn the_dungeon_set_is_never_empty() {
        assert!(
            !DUNGEON_MAPS.is_empty(),
            "DUNGEON_MAPS is empty — the gateway's instance-hosting check iterates it, so an \
             empty set silently turns that check into a no-op"
        );
        assert!(
            !DUNGEON_MAPS.contains(&0),
            "map 0 is the open world; listing it as a dungeon would route every open-world portal \
             through the instancing path"
        );
    }
}
