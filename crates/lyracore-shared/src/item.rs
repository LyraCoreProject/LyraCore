pub const fn is_carried_slot(slot: u8) -> bool {
    slot <= 38 || (slot >= 120 && slot <= 191)
}

pub fn carried_quantity(entry: u32, items: impl IntoIterator<Item = (u8, u32, u32)>) -> u32 {
    items
        .into_iter()
        .filter(|&(slot, item_entry, _)| is_carried_slot(slot) && item_entry == entry)
        .map(|(_, _, count)| count)
        .sum()
}

// Launcher subclass pairing is not enforced by the Module.
pub fn loaded_ammo<T>(
    items: impl IntoIterator<Item = T>,
    fields: impl Fn(&T) -> (u8, u32, Option<u8>),
) -> Option<T> {
    items
        .into_iter()
        .filter_map(|item| {
            let (slot, count, class) = fields(&item);
            (is_carried_slot(slot) && count > 0 && class == Some(6)).then_some((slot, item))
        })
        .min_by_key(|(slot, _)| *slot)
        .map(|(_, item)| item)
}

pub const fn grants_equipped_stats(slot: u8, max_durability: u32, durability: u32) -> bool {
    slot <= 18 && !(max_durability > 0 && durability == 0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemRefusal {
    ItemNotFound,
    InventoryFull,
    WrongSlot,
    CannotEquip,
    TwoHandedWeaponEquipped,
    NoProficiency,
    RequiredSkill,
    RequiredReputation,
    PlayerDead,
    BankUnavailable,
    ItemNotUsable,
    NotRightNow,
    Indestructible,
    BagNotEmpty,
    Internal,
}

impl ItemRefusal {
    pub fn as_tag(self) -> &'static str {
        match self {
            Self::ItemNotFound => "item:item_not_found",
            Self::InventoryFull => "item:inventory_full",
            Self::WrongSlot => "item:wrong_slot",
            Self::CannotEquip => "item:cannot_equip",
            Self::TwoHandedWeaponEquipped => "item:two_handed_weapon_equipped",
            Self::NoProficiency => "item:no_proficiency",
            Self::RequiredSkill => "item:required_skill",
            Self::RequiredReputation => "item:required_reputation",
            Self::PlayerDead => "item:player_dead",
            Self::BankUnavailable => "item:bank_unavailable",
            Self::ItemNotUsable => "item:item_not_usable",
            Self::NotRightNow => "item:not_right_now",
            Self::Indestructible => "item:indestructible",
            Self::BagNotEmpty => "item:bag_not_empty",
            Self::Internal => "item:internal",
        }
    }
}

pub mod armor_subclass {
    pub const MISC: u8 = 0;
    pub const CLOTH: u8 = 1;
    pub const LEATHER: u8 = 2;
    pub const MAIL: u8 = 3;
    pub const PLATE: u8 = 4;
    pub const SHIELD: u8 = 6;
}

pub mod item_class {
    pub const WEAPON: u8 = 2;
    pub const ARMOR: u8 = 4;
}

pub mod weapon_subclass {
    pub const AXE_1H: u8 = 0;
    pub const AXE_2H: u8 = 1;
    pub const BOW: u8 = 2;
    pub const GUN: u8 = 3;
    pub const MACE_1H: u8 = 4;
    pub const MACE_2H: u8 = 5;
    pub const POLEARM: u8 = 6;
    pub const SWORD_1H: u8 = 7;
    pub const SWORD_2H: u8 = 8;
    pub const STAFF: u8 = 10;
    pub const FIST: u8 = 13;
    pub const MISC: u8 = 14;
    pub const DAGGER: u8 = 15;
    pub const THROWN: u8 = 16;
    pub const CROSSBOW: u8 = 18;
    pub const WAND: u8 = 19;
    pub const FISHING_POLE: u8 = 20;
}

// Spell.dbc: each spell's Proficiency effect sets one weapon subclass bit.
pub const WEAPON_PROFICIENCY_SPELLS: [(u8, u32); 15] = {
    use weapon_subclass::*;
    [
        (AXE_1H, 196),
        (AXE_2H, 197),
        (BOW, 264),
        (GUN, 266),
        (MACE_1H, 198),
        (MACE_2H, 199),
        (POLEARM, 200),
        (SWORD_1H, 201),
        (SWORD_2H, 202),
        (STAFF, 227),
        (FIST, 15590),
        (DAGGER, 1180),
        (THROWN, 2567),
        (CROSSBOW, 5011),
        (WAND, 5009),
    ]
};

pub fn is_weapon_proficiency_spell(spell_id: u32) -> bool {
    WEAPON_PROFICIENCY_SPELLS
        .iter()
        .any(|&(_, proficiency_spell)| proficiency_spell == spell_id)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Proficiency {
    armor_subclass_mask: u32,
    weapon_subclass_mask: u32,
}

impl Proficiency {
    pub const fn derive(player_class: u8, knows_plate: bool, knows_mail_upgrade: bool) -> Self {
        use armor_subclass::*;

        let cloth = bit(MISC) | bit(CLOTH);
        let mut mask = match player_class {
            1 | 2 => cloth | bit(LEATHER) | bit(MAIL) | bit(SHIELD),
            3 | 4 | 11 => cloth | bit(LEATHER),
            7 => cloth | bit(LEATHER) | bit(SHIELD),
            5 | 8 | 9 => cloth,
            _ => bit(MISC),
        };
        if knows_plate && matches!(player_class, 1 | 2) {
            mask |= bit(PLATE);
        }
        if knows_mail_upgrade && matches!(player_class, 3 | 7) {
            mask |= bit(MAIL);
        }
        Self {
            armor_subclass_mask: mask,
            // Misc weapons need no proficiency. Vanilla also requires Fishing for poles; this does not.
            weapon_subclass_mask: bit(weapon_subclass::MISC) | bit(weapon_subclass::FISHING_POLE),
        }
    }

    pub fn from_spellbook(player_class: u8, knows: impl Fn(u32) -> bool) -> Self {
        use crate::constants::armor_proficiency::{MAIL_PASSIVE_SPELL_ID, PLATE_PASSIVE_SPELL_ID};

        let mut proficiency = Self::derive(
            player_class,
            knows(PLATE_PASSIVE_SPELL_ID),
            knows(MAIL_PASSIVE_SPELL_ID),
        );
        for (subclass, spell_id) in WEAPON_PROFICIENCY_SPELLS {
            if knows(spell_id) {
                proficiency.weapon_subclass_mask |= bit(subclass);
            }
        }
        proficiency
    }

    // SMSG_SET_PROFICIENCY encodes subclasses as 1 << subclass.
    pub const fn armor_subclass_mask(self) -> u32 {
        self.armor_subclass_mask
    }

    pub const fn weapon_subclass_mask(self) -> u32 {
        self.weapon_subclass_mask
    }

    pub const fn can_equip(self, item_class: u8, item_subclass: u8) -> bool {
        match item_class {
            item_class::ARMOR => has_subclass(self.armor_subclass_mask, item_subclass),
            item_class::WEAPON => has_subclass(self.weapon_subclass_mask, item_subclass),
            _ => true,
        }
    }

    pub const fn allows_armor(self, subclass: u8) -> bool {
        has_subclass(self.armor_subclass_mask, subclass)
    }
}

const fn bit(subclass: u8) -> u32 {
    1 << subclass
}

const fn has_subclass(mask: u32, subclass: u8) -> bool {
    subclass < u32::BITS as u8 && mask & bit(subclass) != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    use armor_subclass::*;

    #[test]
    fn carried_quantity_excludes_bank_and_unknown_slots() {
        assert_eq!(
            carried_quantity(
                7,
                [
                    (0, 7, 1),
                    (38, 7, 2),
                    (39, 7, 40),
                    (119, 7, 40),
                    (120, 7, 3),
                    (191, 7, 4),
                    (192, 7, 40),
                    (23, 8, 50)
                ]
            ),
            10
        );
    }

    #[test]
    fn loaded_ammo_uses_the_lowest_carried_nonempty_projectile() {
        let items = [
            (39, 20, Some(6)),
            (120, 5, Some(6)),
            (191, 7, Some(6)),
            (23, 0, Some(6)),
            (24, 8, Some(2)),
            (25, 1, None),
        ];
        assert_eq!(loaded_ammo(items, |item| *item), Some((120, 5, Some(6))));
    }

    const PLAYABLE: [u8; 9] = [1, 2, 3, 4, 5, 7, 8, 9, 11];

    fn armor_set(proficiency: Proficiency) -> Vec<u8> {
        (0..=SHIELD)
            .filter(|&subclass| proficiency.allows_armor(subclass))
            .collect()
    }

    #[test]
    fn untrained_classes_wear_their_vanilla_base_set() {
        let base = |class| armor_set(Proficiency::derive(class, false, false));
        assert_eq!(base(1), vec![MISC, CLOTH, LEATHER, MAIL, SHIELD]);
        assert_eq!(base(2), vec![MISC, CLOTH, LEATHER, MAIL, SHIELD]);
        assert_eq!(base(3), vec![MISC, CLOTH, LEATHER]);
        assert_eq!(base(4), vec![MISC, CLOTH, LEATHER]);
        assert_eq!(base(11), vec![MISC, CLOTH, LEATHER]);
        assert_eq!(base(7), vec![MISC, CLOTH, LEATHER, SHIELD]);
        for caster in [5u8, 8, 9] {
            assert_eq!(
                base(caster),
                vec![MISC, CLOTH],
                "class {caster} wears cloth"
            );
        }
    }

    #[test]
    fn the_plate_upgrade_only_moves_warriors_and_paladins() {
        for class in PLAYABLE {
            let untrained = Proficiency::derive(class, false, false);
            let trained = Proficiency::derive(class, true, false);
            if matches!(class, 1 | 2) {
                assert!(
                    !untrained.allows_armor(PLATE),
                    "class {class} starts unplated"
                );
                assert!(
                    trained.allows_armor(PLATE),
                    "class {class} trains plate at 40"
                );
            } else {
                assert_eq!(
                    armor_set(trained),
                    armor_set(untrained),
                    "class {class} cannot train plate, so the flag must change nothing"
                );
            }
        }
    }

    #[test]
    fn the_mail_upgrade_only_moves_hunters_and_shamans() {
        for class in PLAYABLE {
            let untrained = Proficiency::derive(class, false, false);
            let trained = Proficiency::derive(class, false, true);
            if matches!(class, 3 | 7) {
                assert!(
                    !untrained.allows_armor(MAIL),
                    "class {class} starts in leather"
                );
                assert!(
                    trained.allows_armor(MAIL),
                    "class {class} trains mail at 40"
                );
            } else {
                assert_eq!(
                    armor_set(trained),
                    armor_set(untrained),
                    "class {class} cannot train the mail upgrade"
                );
            }
        }
    }

    #[test]
    fn a_caster_holding_both_passives_still_wears_cloth() {
        let mage = Proficiency::derive(8, true, true);
        assert_eq!(armor_set(mage), vec![MISC, CLOTH]);
        assert!(!mage.can_equip(item_class::ARMOR, PLATE));
        assert!(!mage.can_equip(item_class::ARMOR, MAIL));
    }

    #[test]
    fn unknown_classes_and_subclasses_fail_closed() {
        for unknown in [0u8, 6, 10, 12, 255] {
            let proficiency = Proficiency::derive(unknown, true, true);
            assert_eq!(
                armor_set(proficiency),
                vec![MISC],
                "class {unknown} is not playable and must not wear armor"
            );
        }
        let warrior = Proficiency::derive(1, true, true);
        for subclass in [5u8, 7, 31, 32, 200, 255] {
            assert!(
                !warrior.allows_armor(subclass),
                "armor subclass {subclass} is not a tier and must be refused"
            );
        }
    }

    fn spellbook(player_class: u8, book: &[u32]) -> Proficiency {
        Proficiency::from_spellbook(player_class, |spell| book.contains(&spell))
    }

    fn weapon_set(proficiency: Proficiency) -> Vec<u8> {
        (0..u32::BITS as u8)
            .filter(|&subclass| proficiency.can_equip(item_class::WEAPON, subclass))
            .collect()
    }

    #[test]
    fn the_spellbook_reader_maps_the_two_passives_to_armor() {
        let armor = |class, book: &[u32]| armor_set(spellbook(class, book));
        let warrior = vec![MISC, CLOTH, LEATHER, MAIL, SHIELD];
        let plate_warrior = vec![MISC, CLOTH, LEATHER, MAIL, PLATE, SHIELD];
        assert_eq!(armor(1, &[]), warrior);
        assert_eq!(armor(1, &[750]), plate_warrior);
        assert_eq!(armor(1, &[8737]), warrior, "mail is already a warrior's");
        assert_eq!(armor(1, &[750, 8737]), plate_warrior);
        assert_eq!(armor(2, &[750]), plate_warrior);
        assert_eq!(armor(3, &[]), [MISC, CLOTH, LEATHER]);
        assert_eq!(
            armor(3, &[750]),
            [MISC, CLOTH, LEATHER],
            "no plate for a hunter"
        );
        assert_eq!(armor(3, &[8737]), [MISC, CLOTH, LEATHER, MAIL]);
        assert_eq!(armor(7, &[8737]), [MISC, CLOTH, LEATHER, MAIL, SHIELD]);
        assert_eq!(armor(8, &[750, 8737]), [MISC, CLOTH]);
        assert_eq!(
            armor(1, &[7109, 8738, 674, 201, 202]),
            warrior,
            "only the passives move armor"
        );
    }

    #[test]
    fn a_warrior_needs_two_handed_swords_before_equipping_one() {
        use weapon_subclass::*;

        let human_warrior_kit = [196, 198, 201];
        let new_warrior = spellbook(1, &human_warrior_kit);
        assert!(new_warrior.can_equip(item_class::WEAPON, SWORD_1H));
        assert!(!new_warrior.can_equip(item_class::WEAPON, SWORD_2H));

        let trained = spellbook(1, &[196, 198, 201, 202]);
        assert!(trained.can_equip(item_class::WEAPON, SWORD_2H));
    }

    #[test]
    fn each_weapon_proficiency_spell_opens_only_its_own_subclass() {
        use weapon_subclass::*;

        for (subclass, spell) in [
            (AXE_1H, 196),
            (AXE_2H, 197),
            (BOW, 264),
            (GUN, 266),
            (MACE_1H, 198),
            (MACE_2H, 199),
            (POLEARM, 200),
            (SWORD_1H, 201),
            (SWORD_2H, 202),
            (STAFF, 227),
            (FIST, 15590),
            (DAGGER, 1180),
            (THROWN, 2567),
            (CROSSBOW, 5011),
            (WAND, 5009),
        ] {
            assert!(is_weapon_proficiency_spell(spell));
            let mut expected = vec![subclass, MISC, FISHING_POLE];
            expected.sort_unstable();
            assert_eq!(
                weapon_set(spellbook(1, &[spell])),
                expected,
                "spell {spell}"
            );
        }
        assert!(
            !is_weapon_proficiency_spell(203),
            "Unarmed has no Proficiency effect"
        );
        assert!(!is_weapon_proficiency_spell(750));
    }

    #[test]
    fn an_empty_spellbook_wields_only_misc_weapons_and_fishing_poles() {
        for class in PLAYABLE {
            assert_eq!(
                weapon_set(spellbook(class, &[])),
                vec![weapon_subclass::MISC, weapon_subclass::FISHING_POLE],
                "class {class}"
            );
        }
    }

    #[test]
    fn weapon_proficiency_follows_the_spellbook_not_the_class() {
        assert!(spellbook(8, &[202]).can_equip(item_class::WEAPON, weapon_subclass::SWORD_2H));
        assert!(!spellbook(1, &[]).can_equip(item_class::WEAPON, weapon_subclass::WAND));
    }

    #[test]
    fn weapon_subclasses_without_a_proficiency_spell_fail_closed() {
        let every_spell: Vec<u32> = WEAPON_PROFICIENCY_SPELLS
            .iter()
            .map(|&(_, spell)| spell)
            .collect();
        let master = spellbook(1, &every_spell);
        for subclass in [9u8, 11, 12, 17, 21, 31, 32, 255] {
            assert!(
                !master.can_equip(item_class::WEAPON, subclass),
                "weapon subclass {subclass} has no proficiency spell"
            );
        }
    }

    #[test]
    fn armor_training_never_teaches_a_weapon() {
        for class in PLAYABLE {
            assert_eq!(
                Proficiency::derive(class, true, true).weapon_subclass_mask(),
                Proficiency::derive(class, false, false).weapon_subclass_mask(),
                "class {class}"
            );
        }
    }

    #[test]
    fn the_weapon_wire_mask_agrees_with_the_gate() {
        let warrior = spellbook(1, &[196, 202, 5011]);
        for subclass in 0..u32::BITS as u8 {
            assert_eq!(
                warrior.weapon_subclass_mask() & (1 << subclass) != 0,
                warrior.can_equip(item_class::WEAPON, subclass),
                "weapon subclass {subclass}"
            );
        }
    }
}
