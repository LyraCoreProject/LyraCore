//! Shared race-to-team mapping for module and gateway faction checks.

/// cmangos team-faction ids, as `game_graveyard_zone.faction` carries them (0 = both factions serve
/// a zone; 469 = Alliance-only; 67 = Horde-only).
pub const TEAM_ALLIANCE: u32 = 469;
pub const TEAM_HORDE: u32 = 67;

/// The team a `game_character.race` byte belongs to.
///
/// Unknown races default to Alliance, matching the currently imported Alliance-only content.
pub fn team_for_race(race: u8) -> u32 {
    match race {
        2 | 5 | 6 | 8 => TEAM_HORDE, // Orc / Undead / Tauren / Troll
        _ => TEAM_ALLIANCE,
    }
}

/// Whether two race bytes belong to the same team.
pub fn same_team(race_a: u8, race_b: u8) -> bool {
    team_for_race(race_a) == team_for_race(race_b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mapping itself, both directions plus the unrecognized-byte default.
    #[test]
    fn every_race_byte_maps_to_a_team_and_an_unknown_one_defaults_alliance() {
        for alliance in [1, 3, 4, 7, 11] {
            assert_eq!(team_for_race(alliance), TEAM_ALLIANCE, "race {alliance}");
        }
        for horde in [2, 5, 6, 8] {
            assert_eq!(team_for_race(horde), TEAM_HORDE, "race {horde}");
        }
        assert_eq!(team_for_race(200), TEAM_ALLIANCE);
    }

    /// The predicate the mail gate reads: same side yes, opposing side no.
    #[test]
    fn same_team_pairs_races_by_side() {
        assert!(same_team(1, 3), "Human and Dwarf are one team");
        assert!(same_team(2, 6), "Orc and Tauren are one team");
        assert!(!same_team(1, 2), "Human and Orc are not");
    }
}
