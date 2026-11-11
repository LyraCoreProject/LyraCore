pub const FIXTURE_RESERVED_ID_FLOOR: u32 = 5_090_000;

pub const FIXTURE_RESERVED_ID_CEIL: u32 = 5_099_999;

pub const CURATED_RESERVED_ID_FLOOR: u32 = 1_000_000;

pub const CURATED_RESERVED_ID_CEIL: u32 = 5_200_499;

pub const RESERVED_ID_FLOOR: u32 = lower(FIXTURE_RESERVED_ID_FLOOR, CURATED_RESERVED_ID_FLOOR);

pub const RESERVED_ID_CEIL: u32 = higher(FIXTURE_RESERVED_ID_CEIL, CURATED_RESERVED_ID_CEIL);

const fn higher(a: u32, b: u32) -> u32 {
    if a > b {
        a
    } else {
        b
    }
}

const fn lower(a: u32, b: u32) -> u32 {
    if a < b {
        a
    } else {
        b
    }
}

const _: () = assert!(FIXTURE_RESERVED_ID_FLOOR <= FIXTURE_RESERVED_ID_CEIL);
const _: () = assert!(CURATED_RESERVED_ID_FLOOR <= CURATED_RESERVED_ID_CEIL);
const _: () = assert!(RESERVED_ID_FLOOR <= RESERVED_ID_CEIL);

pub const PACKAGE_SCRIPT_ID_FLOOR: u32 = 100_000;
pub const PACKAGE_SCRIPT_ID_CEIL: u32 = 999_999;

pub const PACKAGE_SPELL_ID_FLOOR: u32 = 6_000_000;
pub const PACKAGE_SPELL_ID_CEIL: u32 = 6_999_999;

pub const PACKAGE_ITEM_ID_FLOOR: u32 = 7_000_000;
pub const PACKAGE_ITEM_ID_CEIL: u32 = 7_999_999;

pub const PACKAGE_QUEST_ID_FLOOR: u32 = 8_000_000;
pub const PACKAGE_QUEST_ID_CEIL: u32 = 8_999_999;

pub const PACKAGE_LOOT_ID_FLOOR: u64 = 9_000_000;
pub const PACKAGE_LOOT_ID_CEIL: u64 = 9_999_999;

pub const PACKAGE_CAST_ID_FLOOR: u64 = 10_000_000;
pub const PACKAGE_CAST_ID_CEIL: u64 = 10_999_999;

pub const PACKAGE_TRAINER_ID_FLOOR: u64 = 11_000_000;
pub const PACKAGE_TRAINER_ID_CEIL: u64 = 11_999_999;

pub const PACKAGE_GOSSIP_ID_FLOOR: u64 = 12_000_000;
pub const PACKAGE_GOSSIP_ID_CEIL: u64 = 12_999_999;

pub const PACKAGE_GLOBALS_ID_FLOOR: u64 = 13_000_000;
pub const PACKAGE_GLOBALS_ID_CEIL: u64 = 13_999_999;

pub const PACKAGE_SPELLMETA_ID_FLOOR: u64 = 14_000_000;
pub const PACKAGE_SPELLMETA_ID_CEIL: u64 = 14_999_999;

pub const PACKAGE_CREATURE_ID_FLOOR: u32 = 15_000_000;
pub const PACKAGE_CREATURE_ID_CEIL: u32 = 15_999_999;

pub const PACKAGE_GAMEOBJECT_ID_FLOOR: u32 = 16_000_000;
pub const PACKAGE_GAMEOBJECT_ID_CEIL: u32 = 16_999_999;

pub const PACKAGE_CREATURE_AI_ID_FLOOR: u64 = 17_000_000;
pub const PACKAGE_CREATURE_AI_ID_CEIL: u64 = 17_999_999;

pub const FIXTURE_SPELL_ID_FLOOR: u32 = 50_000;
pub const FIXTURE_SPELL_ID_CEIL: u32 = 50_999;

pub const FIXTURE_CREATURE_ID_FLOOR: u32 = 51_000;
pub const FIXTURE_CREATURE_ID_CEIL: u32 = 51_999;

/// One Package Identifier Range; see docs/package-api.md#package-identifier-ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PackageRange {
    Script,
    Spell,
    Item,
    Quest,
    Loot,
    Cast,
    Trainer,
    Gossip,
    Globals,
    SpellMetadata,
    Creature,
    Gameobject,
    EventAi,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Band {
    range: PackageRange,
    floor: u64,
    ceil: u64,
    fixture_cluster: Option<(u64, u64)>,
    pub(crate) row: &'static str,
    pub(crate) name: &'static str,
    pub(crate) inserted: &'static str,
    pub(crate) zero_is_not: &'static str,
}

const fn band(range: PackageRange, floor: u64, ceil: u64, row: &'static str) -> Band {
    Band {
        range,
        floor,
        ceil,
        fixture_cluster: None,
        row,
        name: row,
        inserted: row,
        zero_is_not: "a row",
    }
}

const BANDS: [Band; 13] = [
    band(
        PackageRange::Script,
        PACKAGE_SCRIPT_ID_FLOOR as u64,
        PACKAGE_SCRIPT_ID_CEIL as u64,
        "script",
    ),
    Band {
        fixture_cluster: Some((FIXTURE_SPELL_ID_FLOOR as u64, FIXTURE_SPELL_ID_CEIL as u64)),
        zero_is_not: "a spell",
        ..band(
            PackageRange::Spell,
            PACKAGE_SPELL_ID_FLOOR as u64,
            PACKAGE_SPELL_ID_CEIL as u64,
            "spell",
        )
    },
    Band {
        zero_is_not: "an item",
        ..band(
            PackageRange::Item,
            PACKAGE_ITEM_ID_FLOOR as u64,
            PACKAGE_ITEM_ID_CEIL as u64,
            "item",
        )
    },
    Band {
        zero_is_not: "a quest",
        ..band(
            PackageRange::Quest,
            PACKAGE_QUEST_ID_FLOOR as u64,
            PACKAGE_QUEST_ID_CEIL as u64,
            "quest",
        )
    },
    Band {
        name: "loot",
        ..band(
            PackageRange::Loot,
            PACKAGE_LOOT_ID_FLOOR,
            PACKAGE_LOOT_ID_CEIL,
            "loot row",
        )
    },
    Band {
        name: "cast",
        ..band(
            PackageRange::Cast,
            PACKAGE_CAST_ID_FLOOR,
            PACKAGE_CAST_ID_CEIL,
            "cast row",
        )
    },
    Band {
        name: "trainer",
        ..band(
            PackageRange::Trainer,
            PACKAGE_TRAINER_ID_FLOOR,
            PACKAGE_TRAINER_ID_CEIL,
            "trainer row",
        )
    },
    Band {
        name: "gossip",
        ..band(
            PackageRange::Gossip,
            PACKAGE_GOSSIP_ID_FLOOR,
            PACKAGE_GOSSIP_ID_CEIL,
            "gossip row",
        )
    },
    Band {
        name: "globals",
        ..band(
            PackageRange::Globals,
            PACKAGE_GLOBALS_ID_FLOOR,
            PACKAGE_GLOBALS_ID_CEIL,
            "globals row",
        )
    },
    Band {
        name: "spell metadata",
        ..band(
            PackageRange::SpellMetadata,
            PACKAGE_SPELLMETA_ID_FLOOR,
            PACKAGE_SPELLMETA_ID_CEIL,
            "spell metadata row",
        )
    },
    Band {
        fixture_cluster: Some((
            FIXTURE_CREATURE_ID_FLOOR as u64,
            FIXTURE_CREATURE_ID_CEIL as u64,
        )),
        name: "creature",
        inserted: "creature template or spawn",
        ..band(
            PackageRange::Creature,
            PACKAGE_CREATURE_ID_FLOOR as u64,
            PACKAGE_CREATURE_ID_CEIL as u64,
            "creature row",
        )
    },
    Band {
        name: "gameobject",
        inserted: "gameobject template, trap or spawn",
        ..band(
            PackageRange::Gameobject,
            PACKAGE_GAMEOBJECT_ID_FLOOR as u64,
            PACKAGE_GAMEOBJECT_ID_CEIL as u64,
            "gameobject row",
        )
    },
    Band {
        name: "EventAI",
        inserted: "broadcast text, summon placement or quest event requirement",
        ..band(
            PackageRange::EventAi,
            PACKAGE_CREATURE_AI_ID_FLOOR,
            PACKAGE_CREATURE_AI_ID_CEIL,
            "creature-ai row",
        )
    },
];

const fn bands_are_sound() -> bool {
    let reserved_floor = RESERVED_ID_FLOOR as u64;
    let reserved_ceil = RESERVED_ID_CEIL as u64;
    let fixture_floor = FIXTURE_RESERVED_ID_FLOOR as u64;
    let mut i = 0;
    while i < BANDS.len() {
        let a = BANDS[i];
        if a.range as usize != i || a.floor == 0 || a.floor > a.ceil {
            return false;
        }
        if a.ceil >= reserved_floor && a.floor <= reserved_ceil {
            return false;
        }
        if let Some((floor, ceil)) = a.fixture_cluster {
            if floor > ceil || ceil >= fixture_floor {
                return false;
            }
        }
        let mut j = i + 1;
        while j < BANDS.len() {
            let b = BANDS[j];
            if a.floor <= b.ceil && b.floor <= a.ceil {
                return false;
            }
            j += 1;
        }
        i += 1;
    }
    true
}

const _: () = assert!(bands_are_sound());

impl PackageRange {
    pub(crate) const fn band(self) -> &'static Band {
        &BANDS[self as usize]
    }

    #[must_use]
    pub const fn floor(self) -> u64 {
        self.band().floor
    }

    #[must_use]
    pub const fn ceil(self) -> u64 {
        self.band().ceil
    }

    #[must_use]
    pub const fn contains(self, id: u64) -> bool {
        id >= self.floor() && id <= self.ceil()
    }

    /// The range's own fixture cluster, beside the project-wide fixture band every range shares.
    pub(crate) const fn fixture_cluster(self) -> Option<(u64, u64)> {
        self.band().fixture_cluster
    }

    #[must_use]
    pub const fn is_fixture_reserved(self, id: u64) -> bool {
        if let Some((floor, ceil)) = self.fixture_cluster() {
            if id >= floor && id <= ceil {
                return true;
            }
        }
        is_fixture_reserved_id(id)
    }
}

pub(crate) const fn is_fixture_reserved_id(id: u64) -> bool {
    id >= FIXTURE_RESERVED_ID_FLOOR as u64 && id <= FIXTURE_RESERVED_ID_CEIL as u64
}

#[must_use]
pub const fn is_package_script_id(script_id: u32) -> bool {
    PackageRange::Script.contains(script_id as u64)
}

#[must_use]
pub const fn is_package_spell_id(spell_id: u32) -> bool {
    PackageRange::Spell.contains(spell_id as u64)
}

#[must_use]
pub const fn is_package_item_id(entry: u32) -> bool {
    PackageRange::Item.contains(entry as u64)
}

#[must_use]
pub const fn is_package_quest_id(entry: u32) -> bool {
    PackageRange::Quest.contains(entry as u64)
}

#[must_use]
pub const fn is_package_loot_id(id: u64) -> bool {
    PackageRange::Loot.contains(id)
}

#[must_use]
pub const fn is_package_cast_id(id: u64) -> bool {
    PackageRange::Cast.contains(id)
}

#[must_use]
pub const fn is_package_trainer_id(id: u64) -> bool {
    PackageRange::Trainer.contains(id)
}

#[must_use]
pub const fn is_package_gossip_id(id: u64) -> bool {
    PackageRange::Gossip.contains(id)
}

#[must_use]
pub const fn is_package_globals_id(id: u64) -> bool {
    PackageRange::Globals.contains(id)
}

#[must_use]
pub const fn is_package_spellmeta_id(id: u64) -> bool {
    PackageRange::SpellMetadata.contains(id)
}

#[must_use]
pub const fn is_package_creature_id(id: u32) -> bool {
    PackageRange::Creature.contains(id as u64)
}

#[must_use]
pub const fn is_package_gameobject_id(id: u32) -> bool {
    PackageRange::Gameobject.contains(id as u64)
}

#[must_use]
pub const fn is_package_creature_ai_id(id: u64) -> bool {
    PackageRange::EventAi.contains(id)
}

#[must_use]
pub const fn is_fixture_reserved_spell_id(spell_id: u32) -> bool {
    PackageRange::Spell.is_fixture_reserved(spell_id as u64)
}

#[must_use]
pub const fn is_fixture_reserved_item_id(entry: u32) -> bool {
    PackageRange::Item.is_fixture_reserved(entry as u64)
}

#[must_use]
pub const fn is_fixture_reserved_loot_id(id: u64) -> bool {
    PackageRange::Loot.is_fixture_reserved(id)
}

#[must_use]
pub const fn is_fixture_reserved_trainer_id(id: u64) -> bool {
    PackageRange::Trainer.is_fixture_reserved(id)
}

#[must_use]
pub const fn is_fixture_reserved_creature_id(id: u32) -> bool {
    PackageRange::Creature.is_fixture_reserved(id as u64)
}

#[must_use]
pub const fn packed_spell_effect_id(spell_id: u32, effect_index: u8) -> u64 {
    (spell_id as u64) << 2 | effect_index as u64
}

pub const MAX_SPELL_EFFECT_INDEX: u8 = 2;

// The 1.12 initial spellbook packet carries 16-bit spell ids, so a spell a Character can learn
// must stay below 65,536.
pub const OPERATOR_SPELL_ID_FLOOR: u32 = 60_000;

pub const OPERATOR_SPELL_ID_CEIL: u32 = 65_535;

const _: () = assert!(FIXTURE_SPELL_ID_CEIL < OPERATOR_SPELL_ID_FLOOR);
const _: () = assert!(OPERATOR_SPELL_ID_CEIL <= u16::MAX as u32);

#[must_use]
pub const fn is_operator_spell_id(spell_id: u32) -> bool {
    spell_id >= OPERATOR_SPELL_ID_FLOOR && spell_id <= OPERATOR_SPELL_ID_CEIL
}

pub const MAX_QUEST_OBJECTIVE_INDEX: u8 = 3;

pub const MAX_QUEST_REWARD_CHOICE_INDEX: u8 = 5;

#[must_use]
pub const fn packed_quest_objective_id(quest_entry: u32, obj_index: u8) -> u64 {
    (quest_entry as u64) << 8 | obj_index as u64
}

#[must_use]
pub const fn packed_quest_reward_choice_id(quest_entry: u32, choice_index: u8) -> u64 {
    (quest_entry as u64) << 8 | choice_index as u64
}

#[must_use]
pub const fn packed_quest_reward_item_id(quest_entry: u32, item_entry: u32) -> u64 {
    (quest_entry as u64) << 32 | item_entry as u64
}

pub const MAX_STATS_LEVEL: u8 = 60;

#[must_use]
pub const fn packed_class_level_id(class: u8, level: u8) -> u64 {
    ((class as u64) << 8) | level as u64
}

#[must_use]
pub const fn packed_race_class_level_id(race: u8, class: u8, level: u8) -> u64 {
    ((race as u64) << 16) | ((class as u64) << 8) | level as u64
}

#[must_use]
pub const fn packed_race_class_id(race: u8, class: u8) -> u64 {
    ((race as u64) << 8) | class as u64
}

// Creature GUIDs allocate 24 bits each to the template entry and spawn identifier.
pub const MAX_CREATURE_GUID_COMPONENT: u32 = 0x00FF_FFFF;

const _: () = assert!(PACKAGE_CREATURE_ID_CEIL <= MAX_CREATURE_GUID_COMPONENT);
const _: () = assert!(PACKAGE_CREATURE_AI_ID_CEIL <= u32::MAX as u64);

#[must_use]
pub const fn packed_creature_spawn_guid(entry: u32, spawn_id: u32) -> u64 {
    const HIGHGUID_UNIT: u64 = 0xF130;
    (HIGHGUID_UNIT << 48)
        | (((entry & MAX_CREATURE_GUID_COMPONENT) as u64) << 24)
        | (spawn_id & MAX_CREATURE_GUID_COMPONENT) as u64
}

#[must_use]
pub const fn packed_gameobject_spawn_guid(spawn_id: u32) -> u64 {
    const HIGHGUID_GAMEOBJECT: u64 = 0xF110;
    (HIGHGUID_GAMEOBJECT << 48) | spawn_id as u64
}

// Spawn identifiers the Operator Content API allocates; it shares numbers with spell metadata ids.
pub const OPERATOR_SPAWN_ID_FLOOR: u32 = 14_000_000;

pub const OPERATOR_SPAWN_ID_CEIL: u32 = 14_999_999;

const _: () = assert!(OPERATOR_SPAWN_ID_FLOOR <= OPERATOR_SPAWN_ID_CEIL);
const _: () = assert!(RESERVED_ID_CEIL < OPERATOR_SPAWN_ID_FLOOR);
const _: () = assert!(OPERATOR_SPAWN_ID_CEIL < PACKAGE_CREATURE_ID_FLOOR);
const _: () = assert!(OPERATOR_SPAWN_ID_CEIL < PACKAGE_GAMEOBJECT_ID_FLOOR);
const _: () = assert!(OPERATOR_SPAWN_ID_CEIL <= MAX_CREATURE_GUID_COMPONENT);

#[must_use]
pub const fn is_operator_spawn_id(id: u32) -> bool {
    id >= OPERATOR_SPAWN_ID_FLOOR && id <= OPERATOR_SPAWN_ID_CEIL
}

// Template entries are separate from the Operator Spawn Range's spawn identifiers.
pub const OPERATOR_TEMPLATE_ID_FLOOR: u32 = 5_200_500;
pub const OPERATOR_TEMPLATE_ID_CEIL: u32 = 5_999_999;

const _: () = assert!(RESERVED_ID_CEIL < OPERATOR_TEMPLATE_ID_FLOOR);
const _: () = assert!(OPERATOR_TEMPLATE_ID_FLOOR <= OPERATOR_TEMPLATE_ID_CEIL);
const _: () = assert!(OPERATOR_TEMPLATE_ID_CEIL < PACKAGE_SPELL_ID_FLOOR);
const _: () = assert!(OPERATOR_TEMPLATE_ID_CEIL <= MAX_CREATURE_GUID_COMPONENT);

#[must_use]
pub const fn is_operator_template_id(entry: u32) -> bool {
    entry >= OPERATOR_TEMPLATE_ID_FLOOR && entry <= OPERATOR_TEMPLATE_ID_CEIL
}

#[cfg(test)]
mod operator_template_tests {
    use super::*;

    #[test]
    fn operator_templates_exclude_adjacent_ranges() {
        assert!(!is_operator_template_id(CURATED_RESERVED_ID_CEIL));
        assert!(is_operator_template_id(5_200_500));
        assert!(is_operator_template_id(5_999_999));
        assert!(!is_operator_template_id(PACKAGE_SPELL_ID_FLOOR));
        assert!(!is_operator_template_id(FIXTURE_RESERVED_ID_FLOOR));
        assert!(!is_operator_template_id(PACKAGE_ITEM_ID_FLOOR));
    }
}
