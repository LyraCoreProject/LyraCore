//! Append-only Stat Kind codes used by item catalogue authors and the importer.

pub const UNKNOWN: u8 = 0;
pub const STRENGTH: u8 = 1;
pub const AGILITY: u8 = 2;
pub const STAMINA: u8 = 3;
pub const INTELLECT: u8 = 4;
pub const SPIRIT: u8 = 5;
pub const HEALTH: u8 = 6;
pub const MANA: u8 = 7;
pub const HOLY_RESISTANCE: u8 = 8;
pub const FIRE_RESISTANCE: u8 = 9;
pub const NATURE_RESISTANCE: u8 = 10;
pub const FROST_RESISTANCE: u8 = 11;
pub const SHADOW_RESISTANCE: u8 = 12;
pub const ARCANE_RESISTANCE: u8 = 13;
pub const ARMOR: u8 = 14;
pub const WEAPON_DAMAGE: u8 = 15;
pub const SPELL_POWER: u8 = 16;
pub const HEALING_POWER: u8 = 17;
pub const MANA_PER_FIVE: u8 = 18;
pub const HEALTH_PER_FIVE: u8 = 19;
pub const CRIT: u8 = 20;
pub const HIT: u8 = 21;
pub const DEFENSE: u8 = 22;
pub const DODGE: u8 = 23;
pub const PARRY: u8 = 24;
pub const BLOCK: u8 = 25;

/// Authored compatibility entries, separate from client enchantment IDs.
/// Existing stored items and synthetic enchanting spells use these meanings.
pub const COMPATIBILITY_ENCHANTMENTS: [(u32, u8, i32); 2] =
    [(7745, STRENGTH, 3), (7748, STAMINA, 3)];

/// Client enchantment ID for +3 Strength. It is the ID the Enchant Bracer - Lesser Strength spell
/// applies, so players read it as the Enchanting result. Random Properties own 70 for the same text.
const CLIENT_STRENGTH_3: u32 = 823;
/// Client enchantment ID for +3 Stamina. It is the ID the Lesser Stamina Enchanting spells apply.
/// Random Properties own 73 for the same text.
const CLIENT_STAMINA_3: u32 = 724;

/// The enchantment ID the client resolves for a stored enchantment ID. The client catalogue does
/// not hold the compatibility entries, so each maps to a client entry with the same Stat Kind and
/// amount. Every other ID, including 0, passes through. Stored IDs never change.
pub fn client_enchantment_id(stored: u32) -> u32 {
    match stored {
        7745 => CLIENT_STRENGTH_3,
        7748 => CLIENT_STAMINA_3,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatibility_entries_map_to_client_enchantments_with_the_same_text() {
        assert_eq!(client_enchantment_id(7745), 823);
        assert_eq!(client_enchantment_id(7748), 724);
    }

    #[test]
    fn other_ids_pass_through() {
        for id in [0, 7, 2564, 7746, 823, 724] {
            assert_eq!(client_enchantment_id(id), id);
        }
    }

    #[test]
    fn every_compatibility_entry_has_a_client_id() {
        for (id, _, _) in COMPATIBILITY_ENCHANTMENTS {
            assert_ne!(client_enchantment_id(id), id, "compatibility id {id}");
        }
    }
}
