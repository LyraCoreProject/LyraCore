// Graveyard faction ids: 0 serves both teams, 469 Alliance, 67 Horde.
pub const TEAM_ALLIANCE: u32 = 469;
pub const TEAM_HORDE: u32 = 67;

pub fn team_for_race(race: u8) -> u32 {
    match race {
        2 | 5 | 6 | 8 => TEAM_HORDE,
        _ => TEAM_ALLIANCE,
    }
}

pub fn same_team(race_a: u8, race_b: u8) -> bool {
    team_for_race(race_a) == team_for_race(race_b)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn same_team_pairs_races_by_side() {
        assert!(same_team(1, 3), "Human and Dwarf are one team");
        assert!(same_team(2, 6), "Orc and Tauren are one team");
        assert!(!same_team(1, 2), "Human and Orc are not");
    }
}

pub fn reputation_rank(standing: i32) -> u8 {
    match standing {
        42000.. => 7,
        21000.. => 6,
        9000.. => 5,
        3000.. => 4,
        0.. => 3,
        -3000.. => 2,
        -6000.. => 1,
        _ => 0,
    }
}

/// vmangos `GetIndexFitTo`: the first slot admitting race and class (a zero mask admits all).
pub fn reputation_slot(masks: [(u32, u32); 4], race: u8, class: u8) -> Option<usize> {
    use crate::race_class::{mask_admits, ZeroMask::AdmitsAll};
    masks.into_iter().position(|(race_mask, class_mask)| {
        mask_admits(race_mask, race, AdmitsAll) && mask_admits(class_mask, class, AdmitsAll)
    })
}

pub fn relation_holds(
    asserting: &[u32; 4],
    vetoing: &[u32; 4],
    mask: u32,
    other_faction: u32,
    other_group: u32,
) -> bool {
    if other_faction != 0 {
        if asserting.contains(&other_faction) {
            return true;
        }
        if vetoing.contains(&other_faction) {
            return false;
        }
    }
    mask & other_group != 0
}

#[cfg(test)]
mod rule_tests {
    use super::*;

    // Faction.dbc 1.12 masks for Cenarion Circle (609) and Wintersaber Trainers (589).
    const CENARION_CIRCLE: [(u32, u32); 4] = [(255, 479), (40, 1024), (0, 0), (0, 0)];
    const WINTERSABER_TRAINERS: [(u32, u32); 4] = [(178, 0), (77, 0), (0, 0), (0, 0)];

    #[test]
    fn the_first_slot_that_admits_race_and_class_fits() {
        let (tauren, night_elf, human) = (6, 4, 1);
        let (warrior, druid) = (1, 11);
        assert_eq!(reputation_slot(CENARION_CIRCLE, tauren, druid), Some(1));
        assert_eq!(reputation_slot(CENARION_CIRCLE, night_elf, druid), Some(1));
        assert_eq!(reputation_slot(CENARION_CIRCLE, tauren, warrior), Some(0));
        assert_eq!(
            reputation_slot(WINTERSABER_TRAINERS, tauren, druid),
            Some(0)
        );
        assert_eq!(
            reputation_slot(WINTERSABER_TRAINERS, human, warrior),
            Some(1)
        );
    }

    #[test]
    fn no_slot_fits_when_none_admits_the_character() {
        let (human, warrior) = (1, 1);
        let other_races = [(2, 0), (4, 0), (8, 0), (16, 0)];
        let druids_only = [(1, 1024); 4];
        assert_eq!(reputation_slot(other_races, human, warrior), None);
        assert_eq!(reputation_slot(druids_only, human, warrior), None);
    }

    #[test]
    fn reputation_ranks_change_at_each_vanilla_threshold() {
        for (standing, rank) in [
            (-6000, 1),
            (-3000, 2),
            (0, 3),
            (3000, 4),
            (9000, 5),
            (21000, 6),
            (42000, 7),
        ] {
            assert_eq!(reputation_rank(standing), rank);
            assert_eq!(reputation_rank(standing - 1), rank - 1);
        }
        assert_eq!(reputation_rank(i32::MIN), 0);
        assert_eq!(reputation_rank(i32::MAX), 7);
    }

    #[test]
    fn explicit_hostility_beats_friendship_and_friendship_beats_the_mask() {
        assert!(relation_holds(&[7, 0, 0, 0], &[7, 0, 0, 0], 0, 7, 1));
        assert!(!relation_holds(&[0; 4], &[7, 0, 0, 0], 1, 7, 1));
        assert!(relation_holds(&[0; 4], &[0; 4], 1, 0, 1));
        assert!(!relation_holds(&[0; 4], &[0; 4], 0, 0, 1));
    }
}
