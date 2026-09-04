//! Item rules shared by the module and gateway.

/// Why the Module refused an item Durable Request. The tag is the whole reducer error text, so
/// neither tier matches on human prose; the detail stays in the Module log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemRefusal {
    /// The named slot holds no item, or the item has no template.
    ItemNotFound,
    /// No free backpack, bag, or bag-equip slot is left to receive the item.
    InventoryFull,
    /// The destination slot is out of range, is not an equipment slot, or is already taken.
    WrongSlot,
    /// The item cannot go on the body: the wrong slot family for its type, or a required level
    /// the Character has not reached.
    CannotEquip,
    /// The Character's class, race, or armor and weapon proficiency does not cover the item.
    NoProficiency,
    /// The item needs a trained skill rank the Character does not hold.
    RequiredSkill,
    /// The item needs a reputation standing the Character has not earned.
    RequiredReputation,
    /// A dead Character cannot act on its inventory.
    PlayerDead,
    /// The action reaches bank space with no banker in range.
    BankUnavailable,
    /// The item carries no on-use spell, sits in the bank, or restores a power the class lacks.
    ItemNotUsable,
    /// The action is blocked for now: a live bandage cooldown, or a cast Gate.
    NotRightNow,
    /// The Module could not act and has no gameplay reason to name.
    Internal,
}

impl ItemRefusal {
    pub const ALL: [Self; 12] = [
        Self::ItemNotFound,
        Self::InventoryFull,
        Self::WrongSlot,
        Self::CannotEquip,
        Self::NoProficiency,
        Self::RequiredSkill,
        Self::RequiredReputation,
        Self::PlayerDead,
        Self::BankUnavailable,
        Self::ItemNotUsable,
        Self::NotRightNow,
        Self::Internal,
    ];

    pub fn as_tag(self) -> &'static str {
        match self {
            Self::ItemNotFound => "item:item_not_found",
            Self::InventoryFull => "item:inventory_full",
            Self::WrongSlot => "item:wrong_slot",
            Self::CannotEquip => "item:cannot_equip",
            Self::NoProficiency => "item:no_proficiency",
            Self::RequiredSkill => "item:required_skill",
            Self::RequiredReputation => "item:required_reputation",
            Self::PlayerDead => "item:player_dead",
            Self::BankUnavailable => "item:bank_unavailable",
            Self::ItemNotUsable => "item:item_not_usable",
            Self::NotRightNow => "item:not_right_now",
            Self::Internal => "item:internal",
        }
    }

    pub fn parse_tag(tag: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|refusal| refusal.as_tag() == tag)
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

/// `item_template.class` values this rule branches on (vanilla `ItemClass`).
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
    pub const DAGGER: u8 = 15;
    pub const THROWN: u8 = 16;
    pub const CROSSBOW: u8 = 18;
    pub const WAND: u8 = 19;
    pub const FISHING_POLE: u8 = 20;
}

/// What a Character may equip: the class weapon table plus the armor tiers it has actually reached.
/// Armor is class base set + the two upgrades a class trainer teaches at level 40 (Plate Mail, and
/// the Hunter/Shaman Mail upgrade), so a level-1 Warrior wears mail but not plate.
///
/// Derive it once, then read it from either side: the Module equip Gate calls [`Proficiency::can_equip`],
/// the Gateway sends [`Proficiency::armor_subclass_mask`] in `SMSG_SET_PROFICIENCY`. One derivation
/// keeps the Refusal and the client's tint from drifting apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Proficiency {
    player_class: u8,
    armor_subclass_mask: u32,
}

impl Proficiency {
    /// `knows_plate` is the passive spell 750, `knows_mail_upgrade` the passive 8737
    /// (`constants::armor_proficiency`). A flag set for a class that cannot train that upgrade adds
    /// nothing — a Mage holding 750 still wears cloth. An unknown class gets misc only.
    pub const fn derive(player_class: u8, knows_plate: bool, knows_mail_upgrade: bool) -> Self {
        use armor_subclass::*;

        let cloth = bit(MISC) | bit(CLOTH);
        let mut mask = match player_class {
            1 | 2 => cloth | bit(LEATHER) | bit(MAIL) | bit(SHIELD), // Warrior, Paladin
            3 | 4 | 11 => cloth | bit(LEATHER),                      // Hunter, Rogue, Druid
            7 => cloth | bit(LEATHER) | bit(SHIELD),                 // Shaman
            5 | 8 | 9 => cloth,                                      // Priest, Mage, Warlock
            _ => bit(MISC), // misc (rings, cloaks, trinkets) is never gated for anyone
        };
        if knows_plate && matches!(player_class, 1 | 2) {
            mask |= bit(PLATE);
        }
        if knows_mail_upgrade && matches!(player_class, 3 | 7) {
            mask |= bit(MAIL);
        }
        Self {
            player_class,
            armor_subclass_mask: mask,
        }
    }

    /// Derive from a Character's spellbook: `knows` answers whether a spell id is in it. The Module
    /// equip Gate asks its `game_player_spell` index, the Gateway asks the learned-spell list it
    /// already gathered for the login batch. Which passives mean which tier is named HERE and only
    /// here, so neither side can read a different spell than the other.
    pub fn from_spellbook(player_class: u8, knows: impl Fn(u32) -> bool) -> Self {
        use crate::constants::armor_proficiency::{MAIL_PASSIVE_SPELL_ID, PLATE_PASSIVE_SPELL_ID};

        Self::derive(
            player_class,
            knows(PLATE_PASSIVE_SPELL_ID),
            knows(MAIL_PASSIVE_SPELL_ID),
        )
    }

    /// Every armor tier the class can ever reach, both upgrades included. The auction "usable"
    /// filter shows a browser what its class could wear; the equip Gate must use
    /// [`Self::from_spellbook`] with the Character's own spellbook instead.
    pub const fn class_ceiling(player_class: u8) -> Self {
        Self::derive(player_class, true, true)
    }

    /// Armor subclasses as a `1 << subclass` bitmask — the `SMSG_SET_PROFICIENCY` payload shape.
    pub const fn armor_subclass_mask(self) -> u32 {
        self.armor_subclass_mask
    }

    /// Whether this Character may equip an item of `item_class`/`item_subclass`. Item classes other
    /// than armor and weapon carry no proficiency restriction.
    pub const fn can_equip(self, item_class: u8, item_subclass: u8) -> bool {
        match item_class {
            item_class::ARMOR => self.allows_armor(item_subclass),
            item_class::WEAPON => weapon_proficiency(self.player_class, item_subclass),
            _ => true,
        }
    }

    /// Whether one armor subclass is covered. A subclass past the mask's width cannot be
    /// represented, so it fails closed.
    pub const fn allows_armor(self, subclass: u8) -> bool {
        subclass < u32::BITS as u8 && self.armor_subclass_mask & bit(subclass) != 0
    }
}

const fn bit(subclass: u8) -> u32 {
    1 << subclass
}

const fn weapon_proficiency(player_class: u8, subclass: u8) -> bool {
    use weapon_subclass::*;

    if subclass == FISHING_POLE {
        return true;
    }
    match player_class {
        1 => subclass != WAND,
        2 => matches!(
            subclass,
            AXE_1H | AXE_2H | MACE_1H | MACE_2H | POLEARM | SWORD_1H | SWORD_2H
        ),
        3 => matches!(
            subclass,
            AXE_1H
                | AXE_2H
                | BOW
                | CROSSBOW
                | DAGGER
                | FIST
                | GUN
                | POLEARM
                | STAFF
                | SWORD_1H
                | SWORD_2H
                | THROWN
        ),
        4 => matches!(
            subclass,
            BOW | CROSSBOW | DAGGER | FIST | GUN | MACE_1H | SWORD_1H | THROWN
        ),
        5 => matches!(subclass, DAGGER | MACE_1H | STAFF | WAND),
        7 => matches!(
            subclass,
            AXE_1H | AXE_2H | DAGGER | FIST | MACE_1H | MACE_2H | STAFF
        ),
        8 | 9 => matches!(subclass, DAGGER | STAFF | SWORD_1H | WAND),
        11 => matches!(subclass, DAGGER | FIST | MACE_1H | MACE_2H | STAFF),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use armor_subclass::*;

    /// The nine playable classes (6 and 10 are unused in 1.12).
    const PLAYABLE: [u8; 9] = [1, 2, 3, 4, 5, 7, 8, 9, 11];

    #[test]
    fn every_refusal_tag_round_trips() {
        for refusal in ItemRefusal::ALL {
            assert_eq!(ItemRefusal::parse_tag(refusal.as_tag()), Some(refusal));
        }
        assert_eq!(ItemRefusal::parse_tag("item:"), None);
        assert_eq!(
            ItemRefusal::parse_tag("gw_move_item reducer timed out after 10s"),
            None
        );
    }

    /// What a class can equip with both level-40 upgrades trained — the auction "usable" filter's
    /// question, and the shape the pre-training class table was written in.
    fn ceiling_can_equip(player_class: u8, item_class: u8, item_subclass: u8) -> bool {
        Proficiency::class_ceiling(player_class).can_equip(item_class, item_subclass)
    }

    /// The armor subclasses a derivation admits, in ascending order.
    fn armor_set(proficiency: Proficiency) -> Vec<u8> {
        (0..=SHIELD)
            .filter(|&subclass| proficiency.allows_armor(subclass))
            .collect()
    }

    /// Base sets are class-static: nothing is granted at creation, and no upgrade is trained yet.
    #[test]
    fn untrained_classes_wear_their_vanilla_base_set() {
        let base = |class| armor_set(Proficiency::derive(class, false, false));
        assert_eq!(base(1), vec![MISC, CLOTH, LEATHER, MAIL, SHIELD]); // Warrior
        assert_eq!(base(2), vec![MISC, CLOTH, LEATHER, MAIL, SHIELD]); // Paladin
        assert_eq!(base(3), vec![MISC, CLOTH, LEATHER]); // Hunter
        assert_eq!(base(4), vec![MISC, CLOTH, LEATHER]); // Rogue
        assert_eq!(base(11), vec![MISC, CLOTH, LEATHER]); // Druid
        assert_eq!(base(7), vec![MISC, CLOTH, LEATHER, SHIELD]); // Shaman
        for caster in [5u8, 8, 9] {
            assert_eq!(
                base(caster),
                vec![MISC, CLOTH],
                "class {caster} wears cloth"
            );
        }
    }

    /// Plate is the level-40 Warrior/Paladin purchase, and nobody else's.
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

    /// Mail is the level-40 Hunter/Shaman purchase. Warriors and Paladins already have it.
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

    /// A learned passive is not a wildcard: the class still decides what the spell unlocks.
    #[test]
    fn a_caster_holding_both_passives_still_wears_cloth() {
        let mage = Proficiency::derive(8, true, true);
        assert_eq!(armor_set(mage), vec![MISC, CLOTH]);
        assert!(!mage.can_equip(item_class::ARMOR, PLATE));
        assert!(!mage.can_equip(item_class::ARMOR, MAIL));
    }

    /// Fail closed: a class the model does not know wears accessories only, and a subclass outside
    /// the mask's width is refused rather than shifted out of range.
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

    /// The Gateway sends the mask, the Module answers per subclass. If those two disagree, the
    /// client tints an item usable that the equip Gate then refuses — the exact drift one shared
    /// derivation exists to prevent.
    #[test]
    fn the_wire_mask_agrees_with_the_gate_for_every_class_and_flag() {
        for class in PLAYABLE {
            for knows_plate in [false, true] {
                for knows_mail in [false, true] {
                    let proficiency = Proficiency::derive(class, knows_plate, knows_mail);
                    let mask = proficiency.armor_subclass_mask();
                    for subclass in 0..=SHIELD {
                        assert_eq!(
                            mask & (1 << subclass) != 0,
                            proficiency.allows_armor(subclass),
                            "class {class} ({knows_plate}, {knows_mail}) disagrees on subclass {subclass}"
                        );
                    }
                }
            }
        }
    }

    /// Weapons come from the static class table, untouched by armor training.
    #[test]
    fn armor_training_never_changes_the_weapon_table() {
        for class in PLAYABLE {
            for subclass in 0..=weapon_subclass::FISHING_POLE {
                assert_eq!(
                    Proficiency::derive(class, false, false)
                        .can_equip(item_class::WEAPON, subclass),
                    Proficiency::derive(class, true, true).can_equip(item_class::WEAPON, subclass),
                    "class {class} weapon subclass {subclass}"
                );
            }
        }
    }

    /// The class ceiling is what a Character reaches with everything trained — the auction filter's
    /// question.
    #[test]
    fn the_class_ceiling_is_the_fully_trained_derivation() {
        for class in PLAYABLE {
            assert_eq!(
                Proficiency::class_ceiling(class),
                Proficiency::derive(class, true, true)
            );
        }
        assert!(ceiling_can_equip(1, item_class::ARMOR, PLATE));
        assert!(ceiling_can_equip(3, item_class::ARMOR, MAIL));
    }

    #[test]
    fn representative_class_proficiencies_are_available_to_gateway_reads() {
        assert!(!ceiling_can_equip(8, 4, 4));
        assert!(ceiling_can_equip(1, 4, 4));
        assert!(ceiling_can_equip(8, 2, 19));
        assert!(!ceiling_can_equip(1, 2, 19));
        assert!(ceiling_can_equip(8, 7, 0));
    }

    /// The spellbook reader turns the two trained passives into the two flags, and reads nothing
    /// else. A Character's spellbook holds hundreds of spells; only these two move armor.
    #[test]
    fn the_spellbook_reader_maps_the_two_passives_and_ignores_the_rest() {
        use crate::constants::armor_proficiency::{MAIL_PASSIVE_SPELL_ID, PLATE_PASSIVE_SPELL_ID};

        let from = |class, book: &[u32]| {
            let owned = book.to_vec();
            Proficiency::from_spellbook(class, move |spell| owned.contains(&spell))
        };
        for class in PLAYABLE {
            assert_eq!(from(class, &[]), Proficiency::derive(class, false, false));
            assert_eq!(
                from(class, &[PLATE_PASSIVE_SPELL_ID]),
                Proficiency::derive(class, true, false)
            );
            assert_eq!(
                from(class, &[MAIL_PASSIVE_SPELL_ID]),
                Proficiency::derive(class, false, true)
            );
            assert_eq!(
                from(class, &[PLATE_PASSIVE_SPELL_ID, MAIL_PASSIVE_SPELL_ID]),
                Proficiency::class_ceiling(class)
            );
            // The trainer wrappers are what a Character BUYS; the passive they teach is what counts.
            assert_eq!(
                from(class, &[7109, 8738, 674, 201]),
                Proficiency::derive(class, false, false),
                "class {class}: only the passives move armor"
            );
        }
    }

    /// CROSS-CRATE DERIVATION: the Module's equip Gate and the Gateway's `SMSG_SET_PROFICIENCY`
    /// armor mask must both come from this file, through the same spellbook reader. If either grows
    /// its own class table, or reads a different spell for the same tier, the client tints an item
    /// red that the Gate accepts — or worse, tints one usable that the Gate refuses. No crate can
    /// call both call sites, so this owner pins them by source scan.
    #[test]
    fn both_consumers_derive_their_armor_proficiency_here() {
        let call_sites = [
            (
                "the Module equip Gate",
                include_str!("../../../module/src/items/inventory.rs"),
            ),
            (
                "the Gateway armor mask",
                include_str!("../../../gateway/src/codec/item.rs"),
            ),
        ];
        for (name, src) in call_sites {
            assert!(
                src.contains("Proficiency::from_spellbook("),
                "{name} must derive proficiency from the shared spellbook reader"
            );
            assert!(
                !src.contains("Proficiency::derive("),
                "{name} must not assemble the flags itself — that is how the two sides drift"
            );
            assert!(
                !src.contains("PASSIVE_SPELL_ID"),
                "{name} must not name a proficiency passive; `from_spellbook` owns that mapping"
            );
        }
    }
}
