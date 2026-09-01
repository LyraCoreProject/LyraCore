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

/// A creature template entry, safe to update on `game_gossip_menu`. This table is update-only, so
/// there is no "safe to insert" identifier at all.
pub const REAL_GOSSIP_MENU: u32 = 6;

/// One `game_gossip_menu` claim.
#[must_use]
pub fn gossip_menu_claim(entry: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_gossip_menu","key":{{"entry":{entry}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// An `game_npc_text` row inside the Package gossip range, safe to insert.
pub const PACKAGE_NPC_TEXT: u32 = 12_000_001;

/// A real imported `game_npc_text` row id, safe to update and never safe to insert.
pub const REAL_NPC_TEXT: u32 = 1;

/// Every claimable `game_npc_text` column, so an `insert` carries the whole row.
pub const WHOLE_NPC_TEXT_ROW: &str =
    r#"{ "text": { "type": "string", "value": "The wilds do not forgive." } }"#;

/// One `game_npc_text` claim.
#[must_use]
pub fn npc_text_claim(text_id: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_npc_text","key":{{"text_id":{text_id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// A `game_npc_text_slot` row inside the Package gossip range, safe to insert.
pub const PACKAGE_NPC_TEXT_SLOT: u64 = 12_000_002;

/// A real imported `game_npc_text_slot` row id, safe to update and never safe to insert.
pub const REAL_NPC_TEXT_SLOT: u64 = 1;

/// Every claimable `game_npc_text_slot` column, so an `insert` carries the whole row.
pub const WHOLE_NPC_TEXT_SLOT_ROW: &str = r#"{
    "text_id": { "type": "u32", "value": 1 },
    "slot_index": { "type": "u8", "value": 0 },
    "text_male": { "type": "string", "value": "The wilds do not forgive." },
    "text_female": { "type": "string", "value": "The wilds do not forgive." },
    "probability": { "type": "f32", "value": 1.0 }
}"#;

/// One `game_npc_text_slot` claim.
#[must_use]
pub fn npc_text_slot_claim(id: u64, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_npc_text_slot","key":{{"id":{id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// A `game_gossip_option` row inside the Package gossip range, safe to insert.
pub const PACKAGE_GOSSIP_OPTION: u32 = 12_000_003;

/// A real imported `game_gossip_option` row id, safe to update and never safe to insert.
pub const REAL_GOSSIP_OPTION: u32 = 1;

/// Every claimable `game_gossip_option` column, so an `insert` carries the whole row.
pub const WHOLE_GOSSIP_OPTION_ROW: &str = r#"{
    "entry": { "type": "u32", "value": 6 },
    "option_index": { "type": "u32", "value": 0 },
    "icon": { "type": "u32", "value": 0 },
    "text": { "type": "string", "value": "Train me." },
    "action": { "type": "u32", "value": 5 },
    "action_menu_id": { "type": "u32", "value": 0 },
    "cond_type": { "type": "u32", "value": 0 },
    "cond_value1": { "type": "u32", "value": 0 },
    "cond_value2": { "type": "u32", "value": 0 }
}"#;

/// One `game_gossip_option` claim.
#[must_use]
pub fn gossip_option_claim(row_id: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_gossip_option","key":{{"row_id":{row_id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// A `game_gossip_menu_profile` row inside the Package gossip range, safe to insert.
pub const PACKAGE_GOSSIP_MENU_PROFILE: u32 = 12_000_004;

/// A real imported `game_gossip_menu_profile` row id, safe to update and never safe to insert.
pub const REAL_GOSSIP_MENU_PROFILE: u32 = 1;

/// Every claimable `game_gossip_menu_profile` column, so an `insert` carries the whole row.
pub const WHOLE_GOSSIP_MENU_PROFILE_ROW: &str = r#"{ "text_id": { "type": "u32", "value": 1 } }"#;

/// One `game_gossip_menu_profile` claim.
#[must_use]
pub fn gossip_menu_profile_claim(menu_id: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_gossip_menu_profile","key":{{"menu_id":{menu_id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// A `game_gossip_menu_profile_option` row inside the Package gossip range, safe to insert.
pub const PACKAGE_GOSSIP_MENU_PROFILE_OPTION: u32 = 12_000_005;

/// A real imported `game_gossip_menu_profile_option` row id, safe to update and never safe to
/// insert.
pub const REAL_GOSSIP_MENU_PROFILE_OPTION: u32 = 1;

/// Every claimable `game_gossip_menu_profile_option` column, so an `insert` carries the whole row.
pub const WHOLE_GOSSIP_MENU_PROFILE_OPTION_ROW: &str = r#"{
    "menu_id": { "type": "u32", "value": 1 },
    "option_index": { "type": "u32", "value": 0 },
    "icon": { "type": "u32", "value": 0 },
    "text": { "type": "string", "value": "Train me." },
    "action": { "type": "u32", "value": 5 },
    "action_menu_id": { "type": "u32", "value": 0 },
    "cond_type": { "type": "u32", "value": 0 },
    "cond_value1": { "type": "u32", "value": 0 },
    "cond_value2": { "type": "u32", "value": 0 }
}"#;

/// One `game_gossip_menu_profile_option` claim.
#[must_use]
pub fn gossip_menu_profile_option_claim(row_id: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_gossip_menu_profile_option","key":{{"row_id":{row_id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}
