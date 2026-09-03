//! Artifact fragments the behaviour tests share.
//!
//! The JSON here is written by hand, the way a Datascript generator would emit it, so the tests
//! exercise the parser rather than a builder that could drift with it.

#![allow(dead_code)]

/// A source digest, in the shape the importer's BLAKE3 hashes already take.
pub const HASH_A: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A second, different source digest.
pub const HASH_B: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

/// A spell inside the Package spell range, safe to insert.
pub const PACKAGE_SPELL: u32 = 6_000_001;

/// A real vanilla spell (Fireball), safe to update and never safe to insert.
pub const REAL_SPELL: u32 = 133;

/// Every claimable `game_spell` column, so an `insert` carries the whole row.
pub const WHOLE_SPELL_ROW: &str = r#"{
    "name": { "type": "string", "value": "Kindled Bolt" },
    "power_type": { "type": "u8", "value": 0 },
    "cost": { "type": "u32", "value": 30 },
    "cast_time_ms": { "type": "u32", "value": 2500 },
    "gcd_ms": { "type": "u32", "value": 1500 },
    "cooldown_ms": { "type": "u32", "value": 0 },
    "range_yd": { "type": "u32", "value": 30 },
    "duration_ms": { "type": "u32", "value": 0 },
    "school_mask": { "type": "u8", "value": 4 },
    "dispel_type": { "type": "u8", "value": 1 },
    "mechanic": { "type": "u8", "value": 0 },
    "max_stacks": { "type": "u8", "value": 1 },
    "aura_interrupt": { "type": "u16", "value": 0 },
    "attributes": { "type": "u32", "value": 0 },
    "spell_level": { "type": "u8", "value": 10 },
    "max_level": { "type": "u8", "value": 60 },
    "is_negative": { "type": "bool", "value": true },
    "cast_flags": { "type": "u32", "value": 0 },
    "stances": { "type": "u8", "value": 0 },
    "family_name": { "type": "u8", "value": 3 },
    "family_flags": { "type": "u64", "value": "1" },
    "proc_flags": { "type": "u32", "value": 0 },
    "proc_chance": { "type": "u8", "value": 0 },
    "proc_charges": { "type": "u8", "value": 0 }
}"#;

/// Every claimable `game_spell_effect` column, so an `insert` carries the whole row.
pub const WHOLE_EFFECT_ROW: &str = r#"{
    "kind": { "type": "u8", "value": 2 },
    "base_points": { "type": "i32", "value": 120 },
    "die_sides": { "type": "i32", "value": 10 },
    "per_level": { "type": "f32", "value": 1.5 },
    "period_ms": { "type": "u32", "value": 0 },
    "target": { "type": "u8", "value": 1 },
    "radius_yd": { "type": "f32", "value": 0.0 },
    "chain_targets": { "type": "u8", "value": 0 },
    "trigger_spell": { "type": "u32", "value": 0 },
    "effect_mechanic": { "type": "u8", "value": 0 },
    "p0": { "type": "i32", "value": 0 },
    "p0_kind": { "type": "u8", "value": 255 },
    "p1": { "type": "i32", "value": 0 },
    "script_id": { "type": "u32", "value": 0 },
    "enters_combat": { "type": "bool", "value": false }
}"#;

/// Wraps claims in an artifact envelope.
#[must_use]
pub fn artifact(package: &str, claims: &str) -> String {
    format!(r#"{{"version":1,"package":"{package}","source_hash":"{HASH_A}","claims":[{claims}]}}"#)
}

/// One `game_spell` claim.
#[must_use]
pub fn spell_claim(spell_id: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_spell","key":{{"spell_id":{spell_id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// One `game_spell_effect` claim.
#[must_use]
pub fn effect_claim(spell_id: u32, effect_index: u8, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_spell_effect","key":{{"spell_id":{spell_id},"effect_index":{effect_index}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// The shortest artifact that parses: one Package, one partial update on one real spell.
#[must_use]
pub fn one_spell_update(package: &str, spell_id: u32, fields: &str) -> String {
    artifact(package, &spell_claim(spell_id, "update", fields))
}

/// An item inside the Package item range, safe to insert.
pub const PACKAGE_ITEM: u32 = 7_000_001;

/// A real vanilla item (Worn Shortsword), safe to update and never safe to insert.
pub const REAL_ITEM: u32 = 25;

/// Every claimable `game_item_template` column, so an `insert` carries the whole row.
pub const WHOLE_ITEM_ROW: &str = r#"{
    "class": { "type": "u8", "value": 2 },
    "subclass": { "type": "u8", "value": 7 },
    "name": { "type": "string", "value": "Kindled Blade" },
    "display_id": { "type": "u32", "value": 1420 },
    "quality": { "type": "u8", "value": 1 },
    "inventory_type": { "type": "u8", "value": 21 },
    "item_level": { "type": "u8", "value": 5 },
    "required_level": { "type": "u8", "value": 3 },
    "max_durability": { "type": "u32", "value": 60 },
    "buy_price": { "type": "u32", "value": 500 },
    "sell_price": { "type": "u32", "value": 125 },
    "max_stack": { "type": "u32", "value": 1 },
    "damage_min": { "type": "f32", "value": 3.0 },
    "damage_max": { "type": "f32", "value": 9.0 },
    "delay_ms": { "type": "u32", "value": 1900 },
    "stat_strength": { "type": "i32", "value": 1 },
    "stat_agility": { "type": "i32", "value": 0 },
    "stat_stamina": { "type": "i32", "value": 0 },
    "stat_intellect": { "type": "i32", "value": 0 },
    "stat_spirit": { "type": "i32", "value": 0 },
    "stat_crit": { "type": "i32", "value": 0 },
    "stat_hit": { "type": "i32", "value": 0 },
    "stat_armor": { "type": "i32", "value": 0 },
    "block_value": { "type": "i32", "value": 0 },
    "restores_power": { "type": "bool", "value": false },
    "spellid_1": { "type": "u32", "value": 0 },
    "spelltrigger_1": { "type": "u8", "value": 0 },
    "spellid_2": { "type": "u32", "value": 0 },
    "spelltrigger_2": { "type": "u8", "value": 0 },
    "container_slots": { "type": "u8", "value": 0 },
    "sheath": { "type": "u8", "value": 3 },
    "bonding": { "type": "u8", "value": 0 },
    "holy_res": { "type": "i32", "value": 0 },
    "fire_res": { "type": "i32", "value": 0 },
    "nature_res": { "type": "i32", "value": 0 },
    "frost_res": { "type": "i32", "value": 0 },
    "shadow_res": { "type": "i32", "value": 0 },
    "arcane_res": { "type": "i32", "value": 0 },
    "spellid_3": { "type": "u32", "value": 0 },
    "spelltrigger_3": { "type": "u8", "value": 0 },
    "spellid_4": { "type": "u32", "value": 0 },
    "spelltrigger_4": { "type": "u8", "value": 0 },
    "spellid_5": { "type": "u32", "value": 0 },
    "spelltrigger_5": { "type": "u8", "value": 0 },
    "required_skill": { "type": "u32", "value": 0 },
    "required_skill_rank": { "type": "u32", "value": 0 },
    "required_reputation_faction": { "type": "u32", "value": 0 },
    "required_reputation_rank": { "type": "u32", "value": 0 },
    "max_count": { "type": "u32", "value": 0 },
    "item_flags": { "type": "u32", "value": 0 },
    "page_text": { "type": "u32", "value": 0 },
    "start_quest": { "type": "u32", "value": 0 },
    "bag_family": { "type": "u32", "value": 0 },
    "buy_count": { "type": "u32", "value": 1 },
    "food_type": { "type": "u8", "value": 0 },
    "allowed_class": { "type": "u32", "value": 1503 },
    "allowed_race": { "type": "u32", "value": 255 }
}"#;

/// One `game_item_template` claim.
#[must_use]
pub fn item_claim(entry: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_item_template","key":{{"entry":{entry}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// The shortest artifact that parses: one Package, one partial update on one real item.
#[must_use]
pub fn one_item_update(package: &str, entry: u32, fields: &str) -> String {
    artifact(package, &item_claim(entry, "update", fields))
}

/// A quest inside the Package quest range, safe to insert.
pub const PACKAGE_QUEST: u32 = 8_000_001;

/// A real vanilla quest entry, safe to update and never safe to insert.
pub const REAL_QUEST: u32 = 1; // "Undertaking", the first Elwynn quest, always imported in-box.

/// Every claimable `game_quest_template` column, so an `insert` carries the whole row.
pub const WHOLE_QUEST_ROW: &str = r#"{
    "min_level": { "type": "u32", "value": 1 },
    "quest_level": { "type": "u32", "value": 3 },
    "title": { "type": "string", "value": "A Kindled Errand" },
    "reward_money": { "type": "u32", "value": 100 },
    "reward_xp": { "type": "u32", "value": 0 },
    "prev_quest_id": { "type": "u32", "value": 0 },
    "required_races": { "type": "u32", "value": 0 },
    "required_classes": { "type": "u32", "value": 0 },
    "zone_or_sort": { "type": "i32", "value": 12 },
    "rew_rep_faction_1": { "type": "u32", "value": 0 },
    "rew_rep_value_1": { "type": "i32", "value": 0 },
    "rew_rep_faction_2": { "type": "u32", "value": 0 },
    "rew_rep_value_2": { "type": "i32", "value": 0 },
    "src_item": { "type": "u32", "value": 0 },
    "src_item_count": { "type": "u32", "value": 0 },
    "repeatable": { "type": "bool", "value": false },
    "next_quest_id": { "type": "u32", "value": 0 },
    "limit_time": { "type": "u32", "value": 0 },
    "reward_money_max_level": { "type": "u32", "value": 0 }
}"#;

/// One `game_quest_template` claim.
#[must_use]
pub fn quest_claim(entry: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_quest_template","key":{{"entry":{entry}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// One `game_quest_text` claim.
#[must_use]
pub fn quest_text_claim(quest_entry: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_quest_text","key":{{"quest_entry":{quest_entry}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// One `game_quest_objective` claim.
#[must_use]
pub fn quest_objective_claim(
    quest_entry: u32,
    obj_index: u8,
    operation: &str,
    fields: &str,
) -> String {
    format!(
        r#"{{"table":"game_quest_objective","key":{{"quest_entry":{quest_entry},"obj_index":{obj_index}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// One `game_quest_cast_objective` claim.
#[must_use]
pub fn quest_cast_objective_claim(
    quest_entry: u32,
    obj_index: u8,
    operation: &str,
    fields: &str,
) -> String {
    format!(
        r#"{{"table":"game_quest_cast_objective","key":{{"quest_entry":{quest_entry},"obj_index":{obj_index}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// One `game_quest_reward_item` claim.
#[must_use]
pub fn quest_reward_item_claim(
    quest_entry: u32,
    item_entry: u32,
    operation: &str,
    fields: &str,
) -> String {
    format!(
        r#"{{"table":"game_quest_reward_item","key":{{"quest_entry":{quest_entry},"item_entry":{item_entry}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// One `game_quest_reward_choice` claim.
#[must_use]
pub fn quest_reward_choice_claim(
    quest_entry: u32,
    choice_index: u8,
    operation: &str,
    fields: &str,
) -> String {
    format!(
        r#"{{"table":"game_quest_reward_choice","key":{{"quest_entry":{quest_entry},"choice_index":{choice_index}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// A loot row inside the Package loot range, safe to insert.
pub const PACKAGE_LOOT: u64 = 9_000_001;

/// A real imported pickpocket-loot row id, safe to update and never safe to insert.
pub const REAL_PICKPOCKET_LOOT: u64 = 1;

/// Every claimable `game_pickpocket_loot` column, so an `insert` carries the whole row.
pub const WHOLE_PICKPOCKET_LOOT_ROW: &str = r#"{
    "creature_entry": { "type": "u32", "value": 6 },
    "item_entry": { "type": "u32", "value": 25 },
    "chance_bp": { "type": "u32", "value": 5000 },
    "count": { "type": "u32", "value": 1 },
    "group_id": { "type": "u32", "value": 0 },
    "quest_only": { "type": "bool", "value": false }
}"#;

/// One `game_pickpocket_loot` claim.
#[must_use]
pub fn pickpocket_loot_claim(id: u64, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_pickpocket_loot","key":{{"id":{id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// A caster creature's template entry, safe to update on `game_creature_cast`. This table is
/// update-only, so there is no "safe to insert" identifier at all.
pub const REAL_CREATURE_CAST: u32 = 6;

/// One `game_creature_cast` claim.
#[must_use]
pub fn creature_cast_claim(creature_entry: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_creature_cast","key":{{"creature_entry":{creature_entry}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// A `game_creature_spell` row inside the Package cast range, safe to insert.
pub const PACKAGE_CREATURE_SPELL: u64 = 10_000_001;

/// A real imported `game_creature_spell` row id, safe to update and never safe to insert.
pub const REAL_CREATURE_SPELL: u64 = 1;

/// Every claimable `game_creature_spell` column, so an `insert` carries the whole row.
pub const WHOLE_CREATURE_SPELL_ROW: &str = r#"{
    "creature_entry": { "type": "u32", "value": 6 },
    "spell_id": { "type": "u32", "value": 133 },
    "priority": { "type": "u8", "value": 10 },
    "condition": { "type": "u8", "value": 0 },
    "condition_value": { "type": "u8", "value": 0 }
}"#;

/// One `game_creature_spell` claim.
#[must_use]
pub fn creature_spell_claim(id: u64, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_creature_spell","key":{{"id":{id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// A `game_trainer_spell` row inside the Package trainer range, safe to insert.
pub const PACKAGE_TRAINER_SPELL: u64 = 11_000_001;

/// A real imported `game_trainer_spell` row id, safe to update and never safe to insert.
pub const REAL_TRAINER_SPELL: u64 = 1;

/// Every claimable `game_trainer_spell` column, so an `insert` carries the whole row. A class
/// offering (`learn_skill_line` 0): `spell_id` names a real `game_spell` row.
pub const WHOLE_TRAINER_SPELL_ROW: &str = r#"{
    "trainer_entry": { "type": "u32", "value": 100 },
    "spell_id": { "type": "u32", "value": 133 },
    "cost": { "type": "u32", "value": 500 },
    "required_level": { "type": "u8", "value": 10 },
    "learn_skill_line": { "type": "u32", "value": 0 },
    "learn_skill_cap": { "type": "u32", "value": 75 }
}"#;

/// One `game_trainer_spell` claim.
#[must_use]
pub fn trainer_spell_claim(id: u64, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_trainer_spell","key":{{"id":{id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

// ---- gossip ----

/// A gossip row inside the Package gossip range, safe to insert. One band covers all five
/// insertable gossip tables, so one constant serves them all.
pub const PACKAGE_GOSSIP: u64 = 12_000_001;

/// A real creature template entry with a gossip menu, safe to update and never safe to insert.
pub const REAL_GOSSIP_MENU: u32 = 6;

/// A real imported `game_npc_text` row, safe to update and never safe to insert.
pub const REAL_NPC_TEXT: u32 = 1;

/// Every claimable `game_npc_text` column, so an `insert` carries the whole row.
pub const WHOLE_NPC_TEXT_ROW: &str = r#"{
    "text": { "type": "string", "value": "The forge is cold, friend." }
}"#;

/// Every claimable `game_npc_text_slot` column, so an `insert` carries the whole row.
pub const WHOLE_NPC_TEXT_SLOT_ROW: &str = r#"{
    "text_id": { "type": "u32", "value": 12000001 },
    "slot_index": { "type": "u8", "value": 0 },
    "text_male": { "type": "string", "value": "The forge is cold, friend." },
    "text_female": { "type": "string", "value": "The forge is cold, friend." },
    "probability": { "type": "f32", "value": 1.0 }
}"#;

/// Every claimable `game_gossip_option` column, so an `insert` carries the whole row.
pub const WHOLE_GOSSIP_OPTION_ROW: &str = r#"{
    "entry": { "type": "u32", "value": 6 },
    "option_index": { "type": "u32", "value": 0 },
    "icon": { "type": "u32", "value": 0 },
    "text": { "type": "string", "value": "Tell me of the forge." },
    "action": { "type": "u32", "value": 1 },
    "action_menu_id": { "type": "u32", "value": 0 },
    "cond_type": { "type": "u32", "value": 0 },
    "cond_value1": { "type": "u32", "value": 0 },
    "cond_value2": { "type": "u32", "value": 0 }
}"#;

/// One `game_gossip_menu` claim.
#[must_use]
pub fn gossip_menu_claim(entry: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_gossip_menu","key":{{"entry":{entry}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// One `game_npc_text` claim.
#[must_use]
pub fn npc_text_claim(text_id: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_npc_text","key":{{"text_id":{text_id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// One `game_npc_text_slot` claim.
#[must_use]
pub fn npc_text_slot_claim(id: u64, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_npc_text_slot","key":{{"id":{id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// One `game_gossip_option` claim.
#[must_use]
pub fn gossip_option_claim(row_id: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_gossip_option","key":{{"row_id":{row_id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

// ---- globals ----

/// A globals row inside the Package globals range, safe to insert.
pub const PACKAGE_GLOBALS: u64 = 13_000_001;

/// A real imported `game_graveyard_zone` row, safe to update and never safe to insert.
pub const REAL_GRAVEYARD_ZONE: u64 = 1;

/// Every claimable `game_graveyard_zone` column, so an `insert` carries the whole row.
pub const WHOLE_GRAVEYARD_ZONE_ROW: &str = r#"{
    "safe_loc_id": { "type": "u32", "value": 105 },
    "zone_id": { "type": "u32", "value": 12 },
    "faction": { "type": "u32", "value": 0 }
}"#;

/// Every claimable `game_createinfo_spell` column, so an `insert` carries the whole row.
pub const WHOLE_CREATEINFO_SPELL_ROW: &str = r#"{
    "race": { "type": "u8", "value": 1 },
    "class": { "type": "u8", "value": 1 },
    "spell_id": { "type": "u32", "value": 133 }
}"#;

/// One `game_class_level_stats` claim.
#[must_use]
pub fn class_level_stats_claim(class: u8, level: u8, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_class_level_stats","key":{{"class":{class},"level":{level}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// One `game_start_position` claim.
#[must_use]
pub fn start_position_claim(race: u8, class: u8, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_start_position","key":{{"race":{race},"class":{class}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// One `game_graveyard_zone` claim.
#[must_use]
pub fn graveyard_zone_claim(row_id: u64, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_graveyard_zone","key":{{"row_id":{row_id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// One `game_createinfo_spell` claim.
#[must_use]
pub fn createinfo_spell_claim(id: u64, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_createinfo_spell","key":{{"id":{id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// One `game_areatrigger_teleport` claim.
#[must_use]
pub fn areatrigger_teleport_claim(trigger_id: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_areatrigger_teleport","key":{{"trigger_id":{trigger_id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

// ---- spellmeta ----

/// A `game_spell_learn` row inside the Package spell metadata range, safe to insert.
pub const PACKAGE_SPELL_LEARN: u64 = 14_000_001;

/// A real imported `game_spell_learn` row, safe to update and never safe to insert.
pub const REAL_SPELL_LEARN: u64 = 1;

/// Every claimable `game_spell_chain` column, so an `insert` carries the whole row.
pub const WHOLE_SPELL_CHAIN_ROW: &str = r#"{
    "prev_spell": { "type": "u32", "value": 133 },
    "first_spell": { "type": "u32", "value": 133 },
    "rank": { "type": "u8", "value": 2 },
    "req_spell": { "type": "u32", "value": 0 }
}"#;

/// Every claimable `game_spell_learn` column, so an `insert` carries the whole row.
pub const WHOLE_SPELL_LEARN_ROW: &str = r#"{
    "parent_spell": { "type": "u32", "value": 133 },
    "learn_spell": { "type": "u32", "value": 168 }
}"#;

/// One `game_spell_chain` claim.
#[must_use]
pub fn spell_chain_claim(spell_id: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_spell_chain","key":{{"spell_id":{spell_id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// One `game_spell_learn` claim.
#[must_use]
pub fn spell_learn_claim(id: u64, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_spell_learn","key":{{"id":{id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// One `game_spell_proc_event` claim.
#[must_use]
pub fn spell_proc_event_claim(spell_id: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_spell_proc_event","key":{{"spell_id":{spell_id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

// ---- creatures ----

/// A creature identifier inside the Package creature range, safe to insert. One band covers the
/// template and the spawn.
pub const PACKAGE_CREATURE: u32 = 15_000_001;

/// A real imported creature template, safe to update and never safe to insert.
pub const REAL_CREATURE: u32 = 6; // Kobold Vermin.

/// A real imported spawn identifier, safe to update and never safe to insert.
pub const REAL_CREATURE_SPAWN: u32 = 4_242;

/// The map a spatial fixture claim sits on: Eastern Kingdoms, which every in-box scope owns.
pub const REAL_MAP: u32 = 0;

/// A map no in-box World Import Scope owns, so a claim on it routes to no Shard in this build.
pub const FOREIGN_MAP: u32 = 571;

/// Every claimable `game_creature_template` column, so an `insert` carries the whole row.
pub const WHOLE_CREATURE_TEMPLATE_ROW: &str = r#"{
    "name": { "type": "string", "value": "Kindled Sentinel" },
    "subname": { "type": "string", "value": "Forge Guard" },
    "display_id": { "type": "u32", "value": 1420 },
    "level": { "type": "u32", "value": 12 },
    "health": { "type": "u32", "value": 300 },
    "faction_template": { "type": "u32", "value": 14 },
    "npc_flags": { "type": "u32", "value": 0 },
    "unit_flags": { "type": "u32", "value": 0 },
    "creature_type": { "type": "u8", "value": 7 },
    "creature_family": { "type": "u8", "value": 0 },
    "type_flags": { "type": "u32", "value": 0 },
    "rank": { "type": "u8", "value": 0 },
    "scale": { "type": "f32", "value": 1.0 },
    "base_attack_time_ms": { "type": "u32", "value": 2000 },
    "money_min": { "type": "u32", "value": 10 },
    "money_max": { "type": "u32", "value": 40 },
    "max_level": { "type": "u32", "value": 13 },
    "max_level_health": { "type": "u32", "value": 340 },
    "aggro_range": { "type": "u32", "value": 15 },
    "damage_min": { "type": "u32", "value": 6 },
    "damage_max": { "type": "u32", "value": 9 },
    "armor": { "type": "u32", "value": 120 },
    "pickpocket_loot_id": { "type": "u32", "value": 0 },
    "skin_loot_id": { "type": "u32", "value": 0 },
    "trainer_type": { "type": "u8", "value": 0 },
    "trainer_class": { "type": "u8", "value": 0 }
}"#;

/// Every claimable `game_creature_spawn` column, so an `insert` carries the whole row.
pub const WHOLE_CREATURE_SPAWN_ROW: &str = r#"{
    "x": { "type": "f32", "value": -8949.95 },
    "y": { "type": "f32", "value": -132.493 },
    "z": { "type": "f32", "value": 83.5312 },
    "orientation": { "type": "f32", "value": 0.0 },
    "movement_type": { "type": "u8", "value": 0 },
    "respawn_secs": { "type": "u32", "value": 300 }
}"#;

/// One `game_creature_template` claim.
#[must_use]
pub fn creature_template_claim(entry: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_creature_template","key":{{"entry":{entry}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// One `game_creature_spawn` claim.
#[must_use]
pub fn creature_spawn_claim(
    map_id: u32,
    entry: u32,
    spawn_id: u32,
    operation: &str,
    fields: &str,
) -> String {
    format!(
        r#"{{"table":"game_creature_spawn","key":{{"map_id":{map_id},"entry":{entry},"spawn_id":{spawn_id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

// ---- gameobjects ----

/// A gameobject identifier inside the Package gameobject range, safe to insert. One band covers
/// the template, the trap and the spawn.
pub const PACKAGE_GAMEOBJECT: u32 = 16_000_001;

/// A real imported gameobject template, safe to update and never safe to insert.
pub const REAL_GAMEOBJECT: u32 = 1_731; // Copper Vein.

/// A real imported gameobject spawn identifier.
pub const REAL_GAMEOBJECT_SPAWN: u32 = 7_777;

/// Every claimable `game_gameobject_template` column, so an `insert` carries the whole row.
pub const WHOLE_GAMEOBJECT_TEMPLATE_ROW: &str = r#"{
    "type_id": { "type": "u8", "value": 3 },
    "display_id": { "type": "u32", "value": 259 },
    "name": { "type": "string", "value": "Kindled Cache" },
    "data0": { "type": "u32", "value": 25 },
    "data1": { "type": "u32", "value": 0 },
    "gather_skill_line": { "type": "u32", "value": 0 },
    "respawn_secs": { "type": "u32", "value": 180 },
    "gather_gray": { "type": "u32", "value": 0 },
    "lock_id": { "type": "u32", "value": 0 },
    "size": { "type": "f32", "value": 1.0 }
}"#;

/// Every claimable `game_gameobject_trap` column, so an `insert` carries the whole row.
pub const WHOLE_GAMEOBJECT_TRAP_ROW: &str = r#"{
    "spell_id": { "type": "u32", "value": 133 },
    "cooldown_secs": { "type": "u32", "value": 4 }
}"#;

/// Every claimable `game_gameobject` column, so an `insert` carries the whole row.
pub const WHOLE_GAMEOBJECT_SPAWN_ROW: &str = r#"{
    "template_entry": { "type": "u32", "value": 16000001 },
    "x": { "type": "f32", "value": -8949.95 },
    "y": { "type": "f32", "value": -132.493 },
    "z": { "type": "f32", "value": 83.5312 },
    "orientation": { "type": "f32", "value": 0.0 },
    "state": { "type": "u8", "value": 0 },
    "rotation_0": { "type": "f32", "value": 0.0 },
    "rotation_1": { "type": "f32", "value": 0.0 },
    "rotation_2": { "type": "f32", "value": 0.0 },
    "rotation_3": { "type": "f32", "value": 0.0 }
}"#;

/// One `game_gameobject_template` claim.
#[must_use]
pub fn gameobject_template_claim(entry: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_gameobject_template","key":{{"entry":{entry}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// One `game_gameobject_trap` claim.
#[must_use]
pub fn gameobject_trap_claim(entry: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_gameobject_trap","key":{{"entry":{entry}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// One `game_gameobject` claim.
#[must_use]
pub fn gameobject_spawn_claim(map_id: u32, spawn_id: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_gameobject","key":{{"map_id":{map_id},"spawn_id":{spawn_id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// A broadcast text, summon placement or quest event requirement inside the Package EventAI range.
pub const PACKAGE_CREATURE_AI: u32 = 17_000_001;

/// A real imported broadcast text identifier.
pub const REAL_BROADCAST_TEXT: u32 = 900;

/// Every claimable `game_creature_ai_broadcast_text` column, so an `insert` carries the whole row.
pub const WHOLE_BROADCAST_TEXT_ROW: &str = r#"{
    "male_text": { "type": "string", "value": "The forge remembers." },
    "female_text": { "type": "string", "value": "The forge remembers." },
    "chat_type": { "type": "u8", "value": 1 },
    "language_id": { "type": "u8", "value": 0 },
    "emote_delay_1_ms": { "type": "u32", "value": 0 },
    "emote_id_1": { "type": "u32", "value": 5 },
    "emote_delay_2_ms": { "type": "u32", "value": 0 },
    "emote_id_2": { "type": "u32", "value": 0 },
    "emote_delay_3_ms": { "type": "u32", "value": 0 },
    "emote_id_3": { "type": "u32", "value": 0 }
}"#;

/// Every claimable `game_creature_ai_summon` column, so an `insert` carries the whole row.
pub const WHOLE_SUMMON_ROW: &str = r#"{
    "x": { "type": "f32", "value": -8949.95 },
    "y": { "type": "f32", "value": -132.493 },
    "z": { "type": "f32", "value": 83.5312 },
    "orientation": { "type": "f32", "value": 0.0 },
    "lifetime_ms": { "type": "u32", "value": 30000 }
}"#;

/// Every claimable `game_quest_event_requirement` column, so an `insert` carries the whole row.
pub const WHOLE_QUEST_EVENT_REQUIREMENT_ROW: &str = r#"{
    "quest_entry": { "type": "u32", "value": 8000001 }
}"#;

/// One `game_creature_ai_broadcast_text` claim.
#[must_use]
pub fn broadcast_text_claim(id: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_creature_ai_broadcast_text","key":{{"id":{id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// One `game_creature_ai_summon` claim.
#[must_use]
pub fn summon_claim(id: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_creature_ai_summon","key":{{"id":{id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// One `game_quest_event_requirement` claim.
#[must_use]
pub fn quest_event_requirement_claim(id: u64, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_quest_event_requirement","key":{{"id":{id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}
