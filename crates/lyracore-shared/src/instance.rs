pub const DUNGEON_MAPS: &[u32] = &[
    33, 34, 36, 43, 47, 48, 70, 90, 109, 129, 189, 209, 229, 230, 249, 289, 309, 329, 349, 389,
    409, 429, 469, 509, 531, 533,
];

pub fn is_dungeon_map(map_id: u32) -> bool {
    DUNGEON_MAPS.contains(&map_id)
}

pub const BATTLEGROUND_MAPS: &[u32] = &[30, 489, 529];

pub fn has_instance_population(map_id: u32) -> bool {
    is_dungeon_map(map_id) || BATTLEGROUND_MAPS.contains(&map_id)
}

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
}
