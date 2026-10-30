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
// A Chance on Hit enchantment: its spell may cast when the weapon strikes; it adds no stat.
pub const COMBAT_SPELL: u8 = 26;

pub fn enchant_stat(
    kind: u8,
    school_mask: u32,
    effects: impl IntoIterator<Item = (u8, i32, u32)>,
) -> i32 {
    effects
        .into_iter()
        .filter(|&(effect_kind, _, mask)| {
            effect_kind != UNKNOWN && effect_kind == kind && (mask == 0 || mask & school_mask != 0)
        })
        .fold(0i32, |sum, (_, amount, _)| sum.saturating_add(amount))
}

pub fn property_stat(enchantments: [u32; 3], mut enchant_stat: impl FnMut(u32) -> i32) -> i32 {
    enchantments
        .into_iter()
        .map(&mut enchant_stat)
        .fold(0i32, i32::saturating_add)
}

pub fn item_stat(template: i32, enchantment: i32, property: i32) -> i32 {
    template
        .saturating_add(enchantment)
        .saturating_add(property)
}

pub fn equipped_stat(items: impl IntoIterator<Item = (u8, u32, u32, i32)>) -> i32 {
    items
        .into_iter()
        .filter(|&(slot, max_durability, durability, _)| {
            crate::item::grants_equipped_stats(slot, max_durability, durability)
        })
        .fold(0i32, |sum, (_, _, _, amount)| sum.saturating_add(amount))
}

// Compatibility tuples are stored id, client id, Stat Kind, amount; build 5875 lacks stored ids.
pub const COMPATIBILITY_ENCHANTMENTS: [(u32, u32, u8, i32); 2] =
    [(7745, 823, STRENGTH, 3), (7748, 724, STAMINA, 3)];

pub fn client_enchantment_id(stored: u32) -> u32 {
    COMPATIBILITY_ENCHANTMENTS
        .iter()
        .find(|&&(id, ..)| id == stored)
        .map_or(stored, |&(_, client, ..)| client)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enchantment_effects_require_the_stat_kind_and_school() {
        let effects = [
            (SPELL_POWER, 7, 0),
            (SPELL_POWER, 11, 4),
            (SPELL_POWER, 13, 16),
            (ARMOR, 100, 0),
            (UNKNOWN, 100, 0),
        ];
        assert_eq!(enchant_stat(SPELL_POWER, 4, effects), 18);
        assert_eq!(enchant_stat(UNKNOWN, u32::MAX, effects), 0);
    }

    #[test]
    fn contributions_saturate_in_template_enchantment_property_order() {
        let enchant = enchant_stat(
            ARMOR,
            u32::MAX,
            [(ARMOR, i32::MAX, 0), (ARMOR, 10, 0), (ARMOR, -5, 0)],
        );
        assert_eq!(enchant, i32::MAX - 5);
        let property = property_stat([1, 2, 3], |id| match id {
            1 => i32::MAX,
            2 => 10,
            3 => -30,
            _ => 0,
        });
        assert_eq!(property, i32::MAX - 30);
        assert_eq!(item_stat(i32::MAX, 10, -50), i32::MAX - 50);
        assert_eq!(
            equipped_stat([
                (4, 100, 1, i32::MAX),
                (5, 100, 0, -100),
                (23, 0, 0, -100),
                (18, 0, 0, 10),
                (14, 0, 0, -5)
            ]),
            i32::MAX - 5
        );
    }

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
}
