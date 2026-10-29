pub const FIXTURE_RESERVED_ID_FLOOR: u32 = 5_090_000;

pub const FIXTURE_RESERVED_ID_CEIL: u32 = 5_099_999;

pub const CURATED_RESERVED_ID_CEIL: u32 = 5_200_499;

pub const CURATED_RESERVED_ID_FLOOR: u32 = 1_000_000;

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

#[must_use]
pub const fn packed_spell_effect_id(spell_id: u32, effect_index: u8) -> u64 {
    (spell_id as u64) << 2 | effect_index as u64
}

pub const MAX_SPELL_EFFECT_INDEX: u8 = 2;

pub const PACKAGE_SPELL_ID_FLOOR: u32 = 6_000_000;

pub const PACKAGE_SPELL_ID_CEIL: u32 = 6_999_999;

pub const FIXTURE_SPELL_ID_FLOOR: u32 = 50_000;

pub const FIXTURE_SPELL_ID_CEIL: u32 = 50_999;

const _: () = assert!(FIXTURE_SPELL_ID_FLOOR <= FIXTURE_SPELL_ID_CEIL);
const _: () = assert!(PACKAGE_SPELL_ID_FLOOR <= PACKAGE_SPELL_ID_CEIL);
const _: () = assert!(FIXTURE_SPELL_ID_CEIL < FIXTURE_RESERVED_ID_FLOOR);
const _: () = assert!(RESERVED_ID_CEIL < PACKAGE_SPELL_ID_FLOOR);

#[must_use]
pub const fn is_package_spell_id(spell_id: u32) -> bool {
    spell_id >= PACKAGE_SPELL_ID_FLOOR && spell_id <= PACKAGE_SPELL_ID_CEIL
}

#[must_use]
pub const fn is_fixture_reserved_spell_id(spell_id: u32) -> bool {
    (spell_id >= FIXTURE_SPELL_ID_FLOOR && spell_id <= FIXTURE_SPELL_ID_CEIL)
        || (spell_id >= FIXTURE_RESERVED_ID_FLOOR && spell_id <= FIXTURE_RESERVED_ID_CEIL)
}

pub const PACKAGE_ITEM_ID_FLOOR: u32 = 7_000_000;

pub const PACKAGE_ITEM_ID_CEIL: u32 = 7_999_999;

const _: () = assert!(PACKAGE_ITEM_ID_FLOOR <= PACKAGE_ITEM_ID_CEIL);
const _: () = assert!(RESERVED_ID_CEIL < PACKAGE_ITEM_ID_FLOOR);

#[must_use]
pub const fn is_package_item_id(entry: u32) -> bool {
    entry >= PACKAGE_ITEM_ID_FLOOR && entry <= PACKAGE_ITEM_ID_CEIL
}

#[must_use]
pub const fn is_fixture_reserved_item_id(entry: u32) -> bool {
    entry >= FIXTURE_RESERVED_ID_FLOOR && entry <= FIXTURE_RESERVED_ID_CEIL
}

pub const PACKAGE_SCRIPT_ID_FLOOR: u32 = 100_000;

pub const PACKAGE_SCRIPT_ID_CEIL: u32 = 999_999;

const _: () = assert!(PACKAGE_SCRIPT_ID_FLOOR <= PACKAGE_SCRIPT_ID_CEIL);
const _: () = assert!(PACKAGE_SCRIPT_ID_FLOOR > 0);
const _: () = assert!(PACKAGE_SCRIPT_ID_CEIL < RESERVED_ID_FLOOR);

#[must_use]
pub const fn is_package_script_id(script_id: u32) -> bool {
    script_id >= PACKAGE_SCRIPT_ID_FLOOR && script_id <= PACKAGE_SCRIPT_ID_CEIL
}

pub const PACKAGE_QUEST_ID_FLOOR: u32 = 8_000_000;

pub const PACKAGE_QUEST_ID_CEIL: u32 = 8_999_999;

const _: () = assert!(PACKAGE_QUEST_ID_FLOOR <= PACKAGE_QUEST_ID_CEIL);
const _: () = assert!(RESERVED_ID_CEIL < PACKAGE_QUEST_ID_FLOOR);

#[must_use]
pub const fn is_package_quest_id(entry: u32) -> bool {
    entry >= PACKAGE_QUEST_ID_FLOOR && entry <= PACKAGE_QUEST_ID_CEIL
}

#[must_use]
pub const fn is_fixture_reserved_quest_id(entry: u32) -> bool {
    entry >= FIXTURE_RESERVED_ID_FLOOR && entry <= FIXTURE_RESERVED_ID_CEIL
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

pub const PACKAGE_LOOT_ID_FLOOR: u64 = 9_000_000;

pub const PACKAGE_LOOT_ID_CEIL: u64 = 9_999_999;

const _: () = assert!(PACKAGE_LOOT_ID_FLOOR <= PACKAGE_LOOT_ID_CEIL);
const _: () = assert!((RESERVED_ID_CEIL as u64) < PACKAGE_LOOT_ID_FLOOR);

#[must_use]
pub const fn is_package_loot_id(id: u64) -> bool {
    id >= PACKAGE_LOOT_ID_FLOOR && id <= PACKAGE_LOOT_ID_CEIL
}

#[must_use]
pub const fn is_fixture_reserved_loot_id(id: u64) -> bool {
    id >= FIXTURE_RESERVED_ID_FLOOR as u64 && id <= FIXTURE_RESERVED_ID_CEIL as u64
}

pub const PACKAGE_CAST_ID_FLOOR: u64 = 10_000_000;

pub const PACKAGE_CAST_ID_CEIL: u64 = 10_999_999;

const _: () = assert!(PACKAGE_CAST_ID_FLOOR <= PACKAGE_CAST_ID_CEIL);
const _: () = assert!((RESERVED_ID_CEIL as u64) < PACKAGE_CAST_ID_FLOOR);

#[must_use]
pub const fn is_package_cast_id(id: u64) -> bool {
    id >= PACKAGE_CAST_ID_FLOOR && id <= PACKAGE_CAST_ID_CEIL
}

#[must_use]
pub const fn is_fixture_reserved_cast_id(id: u64) -> bool {
    id >= FIXTURE_RESERVED_ID_FLOOR as u64 && id <= FIXTURE_RESERVED_ID_CEIL as u64
}

pub const PACKAGE_TRAINER_ID_FLOOR: u64 = 11_000_000;

pub const PACKAGE_TRAINER_ID_CEIL: u64 = 11_999_999;

const _: () = assert!(PACKAGE_TRAINER_ID_FLOOR <= PACKAGE_TRAINER_ID_CEIL);
const _: () = assert!((RESERVED_ID_CEIL as u64) < PACKAGE_TRAINER_ID_FLOOR);

#[must_use]
pub const fn is_package_trainer_id(id: u64) -> bool {
    id >= PACKAGE_TRAINER_ID_FLOOR && id <= PACKAGE_TRAINER_ID_CEIL
}

#[must_use]
pub const fn is_fixture_reserved_trainer_id(id: u64) -> bool {
    id >= FIXTURE_RESERVED_ID_FLOOR as u64 && id <= FIXTURE_RESERVED_ID_CEIL as u64
}

pub const PACKAGE_GOSSIP_ID_FLOOR: u64 = 12_000_000;

pub const PACKAGE_GOSSIP_ID_CEIL: u64 = 12_999_999;

const _: () = assert!(PACKAGE_GOSSIP_ID_FLOOR <= PACKAGE_GOSSIP_ID_CEIL);
const _: () = assert!((RESERVED_ID_CEIL as u64) < PACKAGE_GOSSIP_ID_FLOOR);
const _: () = assert!(PACKAGE_TRAINER_ID_CEIL < PACKAGE_GOSSIP_ID_FLOOR);

#[must_use]
pub const fn is_package_gossip_id(id: u64) -> bool {
    id >= PACKAGE_GOSSIP_ID_FLOOR && id <= PACKAGE_GOSSIP_ID_CEIL
}

#[must_use]
pub const fn is_fixture_reserved_gossip_id(id: u64) -> bool {
    id >= FIXTURE_RESERVED_ID_FLOOR as u64 && id <= FIXTURE_RESERVED_ID_CEIL as u64
}

pub const PACKAGE_GLOBALS_ID_FLOOR: u64 = 13_000_000;

pub const PACKAGE_GLOBALS_ID_CEIL: u64 = 13_999_999;

const _: () = assert!(PACKAGE_GLOBALS_ID_FLOOR <= PACKAGE_GLOBALS_ID_CEIL);
const _: () = assert!((RESERVED_ID_CEIL as u64) < PACKAGE_GLOBALS_ID_FLOOR);
const _: () = assert!(PACKAGE_GOSSIP_ID_CEIL < PACKAGE_GLOBALS_ID_FLOOR);

#[must_use]
pub const fn is_package_globals_id(id: u64) -> bool {
    id >= PACKAGE_GLOBALS_ID_FLOOR && id <= PACKAGE_GLOBALS_ID_CEIL
}

#[must_use]
pub const fn is_fixture_reserved_globals_id(id: u64) -> bool {
    id >= FIXTURE_RESERVED_ID_FLOOR as u64 && id <= FIXTURE_RESERVED_ID_CEIL as u64
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

pub const PACKAGE_SPELLMETA_ID_FLOOR: u64 = 14_000_000;

pub const PACKAGE_SPELLMETA_ID_CEIL: u64 = 14_999_999;

const _: () = assert!(PACKAGE_SPELLMETA_ID_FLOOR <= PACKAGE_SPELLMETA_ID_CEIL);
const _: () = assert!((RESERVED_ID_CEIL as u64) < PACKAGE_SPELLMETA_ID_FLOOR);
const _: () = assert!(PACKAGE_GLOBALS_ID_CEIL < PACKAGE_SPELLMETA_ID_FLOOR);

#[must_use]
pub const fn is_package_spellmeta_id(id: u64) -> bool {
    id >= PACKAGE_SPELLMETA_ID_FLOOR && id <= PACKAGE_SPELLMETA_ID_CEIL
}

#[must_use]
pub const fn is_fixture_reserved_spellmeta_id(id: u64) -> bool {
    id >= FIXTURE_RESERVED_ID_FLOOR as u64 && id <= FIXTURE_RESERVED_ID_CEIL as u64
}

// Creature GUIDs allocate 24 bits each to the template entry and spawn identifier.
pub const MAX_CREATURE_GUID_COMPONENT: u32 = 0x00FF_FFFF;

pub const PACKAGE_CREATURE_ID_FLOOR: u32 = 15_000_000;

pub const PACKAGE_CREATURE_ID_CEIL: u32 = 15_999_999;

pub const FIXTURE_CREATURE_ID_FLOOR: u32 = 51_000;

pub const FIXTURE_CREATURE_ID_CEIL: u32 = 51_999;

const _: () = assert!(FIXTURE_CREATURE_ID_FLOOR <= FIXTURE_CREATURE_ID_CEIL);
const _: () = assert!(FIXTURE_CREATURE_ID_CEIL < FIXTURE_RESERVED_ID_FLOOR);
const _: () = assert!(PACKAGE_CREATURE_ID_FLOOR <= PACKAGE_CREATURE_ID_CEIL);
const _: () = assert!(RESERVED_ID_CEIL < PACKAGE_CREATURE_ID_FLOOR);
const _: () = assert!(PACKAGE_SPELLMETA_ID_CEIL < PACKAGE_CREATURE_ID_FLOOR as u64);
const _: () = assert!(PACKAGE_CREATURE_ID_CEIL <= MAX_CREATURE_GUID_COMPONENT);

#[must_use]
pub const fn is_package_creature_id(id: u32) -> bool {
    id >= PACKAGE_CREATURE_ID_FLOOR && id <= PACKAGE_CREATURE_ID_CEIL
}

#[must_use]
pub const fn is_fixture_reserved_creature_id(id: u32) -> bool {
    (id >= FIXTURE_CREATURE_ID_FLOOR && id <= FIXTURE_CREATURE_ID_CEIL)
        || (id >= FIXTURE_RESERVED_ID_FLOOR && id <= FIXTURE_RESERVED_ID_CEIL)
}

#[must_use]
pub const fn is_fixture_reserved_creature_spawn_id(id: u32) -> bool {
    id >= FIXTURE_RESERVED_ID_FLOOR && id <= FIXTURE_RESERVED_ID_CEIL
}

#[must_use]
pub const fn packed_creature_spawn_guid(entry: u32, spawn_id: u32) -> u64 {
    const HIGHGUID_UNIT: u64 = 0xF130;
    (HIGHGUID_UNIT << 48)
        | (((entry & MAX_CREATURE_GUID_COMPONENT) as u64) << 24)
        | (spawn_id & MAX_CREATURE_GUID_COMPONENT) as u64
}

pub const PACKAGE_GAMEOBJECT_ID_FLOOR: u32 = 16_000_000;

pub const PACKAGE_GAMEOBJECT_ID_CEIL: u32 = 16_999_999;

const _: () = assert!(PACKAGE_GAMEOBJECT_ID_FLOOR <= PACKAGE_GAMEOBJECT_ID_CEIL);
const _: () = assert!(RESERVED_ID_CEIL < PACKAGE_GAMEOBJECT_ID_FLOOR);
const _: () = assert!(PACKAGE_CREATURE_ID_CEIL < PACKAGE_GAMEOBJECT_ID_FLOOR);

#[must_use]
pub const fn is_package_gameobject_id(id: u32) -> bool {
    id >= PACKAGE_GAMEOBJECT_ID_FLOOR && id <= PACKAGE_GAMEOBJECT_ID_CEIL
}

#[must_use]
pub const fn is_fixture_reserved_gameobject_id(id: u32) -> bool {
    id >= FIXTURE_RESERVED_ID_FLOOR && id <= FIXTURE_RESERVED_ID_CEIL
}

#[must_use]
pub const fn packed_gameobject_spawn_guid(spawn_id: u32) -> u64 {
    const HIGHGUID_GAMEOBJECT: u64 = 0xF110;
    (HIGHGUID_GAMEOBJECT << 48) | spawn_id as u64
}

pub const PACKAGE_CREATURE_AI_ID_FLOOR: u64 = 17_000_000;

pub const PACKAGE_CREATURE_AI_ID_CEIL: u64 = 17_999_999;

const _: () = assert!(PACKAGE_CREATURE_AI_ID_FLOOR <= PACKAGE_CREATURE_AI_ID_CEIL);
const _: () = assert!((RESERVED_ID_CEIL as u64) < PACKAGE_CREATURE_AI_ID_FLOOR);
const _: () = assert!((PACKAGE_GAMEOBJECT_ID_CEIL as u64) < PACKAGE_CREATURE_AI_ID_FLOOR);
const _: () = assert!(PACKAGE_CREATURE_AI_ID_CEIL <= u32::MAX as u64);

#[must_use]
pub const fn is_package_creature_ai_id(id: u64) -> bool {
    id >= PACKAGE_CREATURE_AI_ID_FLOOR && id <= PACKAGE_CREATURE_AI_ID_CEIL
}

#[must_use]
pub const fn is_fixture_reserved_creature_ai_id(id: u64) -> bool {
    id >= FIXTURE_RESERVED_ID_FLOOR as u64 && id <= FIXTURE_RESERVED_ID_CEIL as u64
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
