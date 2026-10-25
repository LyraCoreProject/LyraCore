// The client classifies creature GUIDs by their high 16 bits.
const HIGHGUID_UNIT: u64 = 0xF130;
const HIGHGUID_PET: u64 = 0xF140;

// Creature-owned guardians use HIGHGUID_PET to avoid colliding with their owner guid.
pub fn pet_guid_for(owner_guid: u64) -> u64 {
    let high = if owner_guid >> 48 == HIGHGUID_UNIT {
        HIGHGUID_PET
    } else {
        HIGHGUID_UNIT
    };
    (high << 48) | (owner_guid & 0x0000_FFFF_FFFF_FFFF)
}

// Unicode scalar limits: CMaNGOS defaults to 2; the 1.12 client caps names at 12.
pub const MIN_PET_NAME: usize = 2;
pub const MAX_PET_NAME: usize = 12;

/// The native Module handler matches this Refusal exactly to send `SMSG_PET_NAME_INVALID`.
pub const PET_NAME_INVALID: &str = "PET_NAME_INVALID";

/// Uses one allowed script; ASCII digits and spaces are refused.
pub fn is_valid_pet_name(name: &str) -> bool {
    let length = name.chars().count();
    (MIN_PET_NAME..=MAX_PET_NAME).contains(&length)
        && crate::guild::Script::ALL
            .into_iter()
            .any(|script| name.chars().all(|c| script.holds(c)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pet_name_is_two_to_twelve_letters_of_one_script() {
        for name in ["Wo", "Wolf", "Fenrisúlfrxx", "Волк", "ウルフ"] {
            assert!(is_valid_pet_name(name), "{name} should be valid");
        }
        for name in [
            "",
            "W",
            "Wolfwolfwolfw",
            "Wolf 2",
            "Wolf2",
            " Wolf",
            "Wo-lf",
            "WКlf", // Cyrillic U+041A amid Latin letters.
        ] {
            assert!(!is_valid_pet_name(name), "{name:?} should be refused");
        }
    }

    #[test]
    fn a_player_owners_pet_keeps_the_unit_namespace() {
        let owner = 7;
        assert_eq!(pet_guid_for(owner) >> 48, HIGHGUID_UNIT);
        assert_eq!(pet_guid_for(owner) & 0x0000_FFFF_FFFF_FFFF, 7);
    }

    #[test]
    fn a_creature_owners_guardian_uses_the_pet_namespace() {
        let owner = (HIGHGUID_UNIT << 48) | 11;
        assert_eq!(pet_guid_for(owner) >> 48, HIGHGUID_PET);
    }
}
