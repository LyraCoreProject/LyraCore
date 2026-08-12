//! The race → team mapping, in the one place both halves of the server can reach it.
//!
//! It lived in `module/src/graveyard.rs` (a corpse releases to a graveyard its own faction serves)
//! and stayed there while every faction question was asked inside a reducer. Mail asks the same
//! question in the GATEWAY — realm-core holds no characters, so "may these two write to each
//! other" is answered before any reducer runs — and a second copy of this table over there is the
//! failure mode worth avoiding: two mappings agree until somebody adds a race to one of them.

/// cmangos team-faction ids, as `game_graveyard_zone.faction` carries them (0 = both factions serve
/// a zone; 469 = Alliance-only; 67 = Horde-only).
pub const TEAM_ALLIANCE: u32 = 469;
pub const TEAM_HORDE: u32 = 67;

/// The team a `game_character.race` byte belongs to.
///
/// Alliance races: Human(1)/Dwarf(3)/NightElf(4)/Gnome(7)/Draenei(11 — a TBC-era id some DBC builds
/// still carry a placeholder row for); every other race byte — including an unrecognized one —
/// defaults to ALLIANCE: the only content this sandbox has ever imported is Alliance-side
/// (Elwynn/Westfall), so failing toward Alliance keeps the common path correct. A real Horde launch
/// needs this list extended (and Horde-side content imported) before it matters.
pub fn team_for_race(race: u8) -> u32 {
    match race {
        2 | 5 | 6 | 8 => TEAM_HORDE, // Orc / Undead / Tauren / Troll
        _ => TEAM_ALLIANCE,
    }
}

/// Do two race bytes belong to the same team? The mail faction gate, and the shape any other
/// cross-character gate should ask in rather than comparing two [`team_for_race`] calls itself.
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
