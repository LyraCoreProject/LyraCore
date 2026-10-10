//! Shared dungeon-map classification and Instance Removal countdown.
//! The Module uses the map set to create dungeon Instances. The Gateway uses it to verify
//! that each dungeon has a Shard that hosts its population.

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

/// The Instance Removal countdown in milliseconds (cm:Player.cpp:17723). The Module schedules the
/// expiry this far out, and the Gateway never shows the client more time than this.
pub const INSTANCE_REMOVAL_MS: u32 = 60_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dungeon_set_holds_deadmines_and_no_open_world_map() {
        assert!(is_dungeon_map(36), "Deadmines is the one imported dungeon");
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
