//! Artifact fragments the tests in this module tree share.
//!
//! The JSON here is written by hand, the way a Datascript generator would emit it, so the tests
//! exercise the parser rather than a builder that could drift with it. A family that lands adds its
//! own fragments beside these.

use lyracore_package_delta::{Column, FieldType, FieldValue};

use super::{ApplyPlan, ARTIFACT_SEPARATOR};

pub(super) const HASH_A: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
pub(super) const PACKAGE_SPELL: u32 = 6_000_001;
pub(super) const REAL_SPELL: u32 = 133;

pub(super) const WHOLE_SPELL_ROW: &str = r#"{
    "name": { "type": "string", "value": "Kindled Bolt" },
    "power_type": { "type": "u8", "value": 0 },
    "cost": { "type": "u32", "value": 30 },
    "cast_time_ms": { "type": "u32", "value": 2500 },
    "gcd_ms": { "type": "u32", "value": 1500 },
    "cooldown_ms": { "type": "u32", "value": 8000 },
    "range_yd": { "type": "u32", "value": 30 },
    "duration_ms": { "type": "u32", "value": 0 },
    "school_mask": { "type": "u8", "value": 4 },
    "dispel_type": { "type": "u8", "value": 1 },
    "mechanic": { "type": "u8", "value": 0 },
    "max_stacks": { "type": "u8", "value": 1 },
    "aura_interrupt": { "type": "u16", "value": 3 },
    "attributes": { "type": "u32", "value": 0 },
    "spell_level": { "type": "u8", "value": 10 },
    "max_level": { "type": "u8", "value": 60 },
    "is_negative": { "type": "bool", "value": true },
    "cast_flags": { "type": "u32", "value": 0 },
    "stances": { "type": "u8", "value": 0 },
    "family_name": { "type": "u8", "value": 3 },
    "family_flags": { "type": "u64", "value": "18446744073709551615" },
    "proc_flags": { "type": "u32", "value": 0 },
    "proc_chance": { "type": "u8", "value": 0 },
    "proc_charges": { "type": "u8", "value": 0 }
}"#;

pub(super) const WHOLE_EFFECT_ROW: &str = r#"{
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

/// Wraps claims in an artifact envelope, on ONE line. The fixtures above are indented so a reader
/// can follow them; an artifact in a payload never is.
pub(super) fn artifact(package: &str, claims: &str) -> String {
    let claims: String = claims.lines().map(str::trim).collect();
    format!(r#"{{"version":1,"package":"{package}","source_hash":"{HASH_A}","claims":[{claims}]}}"#)
}

pub(super) fn spell_claim(spell_id: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_spell","key":{{"spell_id":{spell_id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

pub(super) fn effect_claim(
    spell_id: u32,
    effect_index: u8,
    operation: &str,
    fields: &str,
) -> String {
    format!(
        r#"{{"table":"game_spell_effect","key":{{"spell_id":{spell_id},"effect_index":{effect_index}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

pub(super) const PACKAGE_ITEM: u32 = 7_000_001;
pub(super) const REAL_ITEM: u32 = 25; // Worn Shortsword — a real imported item template.

pub(super) const WHOLE_ITEM_ROW: &str = r#"{
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

pub(super) fn item_claim(entry: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_item_template","key":{{"entry":{entry}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

pub(super) const PACKAGE_QUEST: u32 = 8_000_001;
pub(super) const REAL_QUEST: u32 = 1; // "Undertaking", the first Elwynn quest, always imported in-box.

pub(super) const WHOLE_QUEST_ROW: &str = r#"{
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

pub(super) fn quest_claim(entry: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_quest_template","key":{{"entry":{entry}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

pub(super) fn quest_objective_claim(
    quest_entry: u32,
    obj_index: u8,
    operation: &str,
    fields: &str,
) -> String {
    format!(
        r#"{{"table":"game_quest_objective","key":{{"quest_entry":{quest_entry},"obj_index":{obj_index}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

pub(super) fn quest_reward_item_claim(
    quest_entry: u32,
    item_entry: u32,
    operation: &str,
    fields: &str,
) -> String {
    format!(
        r#"{{"table":"game_quest_reward_item","key":{{"quest_entry":{quest_entry},"item_entry":{item_entry}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

pub(super) const PACKAGE_LOOT: u64 = 9_000_001;
pub(super) const REAL_PICKPOCKET_LOOT: u64 = 1;

pub(super) const WHOLE_PICKPOCKET_LOOT_ROW: &str = r#"{
    "creature_entry": { "type": "u32", "value": 6 },
    "item_entry": { "type": "u32", "value": 25 },
    "chance_bp": { "type": "u32", "value": 5000 },
    "count": { "type": "u32", "value": 1 },
    "group_id": { "type": "u32", "value": 0 },
    "quest_only": { "type": "bool", "value": false }
}"#;

pub(super) fn pickpocket_loot_claim(id: u64, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_pickpocket_loot","key":{{"id":{id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

pub(super) const PACKAGE_CREATURE_SPELL: u64 = 10_000_001;
pub(super) const REAL_CREATURE_CAST: u32 = 6; // Kobold Vermin — a real imported creature template with a cast row.
pub(super) const REAL_CREATURE_SPELL: u64 = 1;

pub(super) const WHOLE_CREATURE_SPELL_ROW: &str = r#"{
    "creature_entry": { "type": "u32", "value": 6 },
    "spell_id": { "type": "u32", "value": 133 },
    "priority": { "type": "u8", "value": 10 },
    "condition": { "type": "u8", "value": 0 },
    "condition_value": { "type": "u8", "value": 0 }
}"#;

pub(super) fn creature_cast_claim(creature_entry: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_creature_cast","key":{{"creature_entry":{creature_entry}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

pub(super) fn creature_spell_claim(id: u64, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_creature_spell","key":{{"id":{id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

pub(super) const PACKAGE_TRAINER_SPELL: u64 = 11_000_001;
pub(super) const REAL_TRAINER_SPELL: u64 = 1;

pub(super) const WHOLE_TRAINER_SPELL_ROW: &str = r#"{
    "trainer_entry": { "type": "u32", "value": 100 },
    "spell_id": { "type": "u32", "value": 133 },
    "cost": { "type": "u32", "value": 500 },
    "required_level": { "type": "u8", "value": 10 },
    "learn_skill_line": { "type": "u32", "value": 0 },
    "learn_skill_cap": { "type": "u32", "value": 75 }
}"#;

pub(super) fn trainer_spell_claim(id: u64, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_trainer_spell","key":{{"id":{id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

// ---- gossip ----

/// A gossip row inside the Package gossip range, safe to insert. One band covers all five
/// insertable gossip tables.
pub(super) const PACKAGE_GOSSIP: u64 = 12_000_001;

/// A real imported `game_npc_text` row, safe to update and never safe to insert.
pub(super) const REAL_NPC_TEXT: u32 = 1;

pub(super) const WHOLE_NPC_TEXT_ROW: &str = r#"{
    "text": { "type": "string", "value": "The forge is cold, friend." }
}"#;

pub(super) const WHOLE_GOSSIP_OPTION_ROW: &str = r#"{
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

pub(super) fn npc_text_claim(text_id: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_npc_text","key":{{"text_id":{text_id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

pub(super) fn gossip_option_claim(row_id: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_gossip_option","key":{{"row_id":{row_id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

// ---- globals ----

/// A globals row inside the Package globals range, safe to insert.
pub(super) const PACKAGE_GLOBALS: u64 = 13_000_001;

/// A real imported `game_graveyard_zone` row, safe to update and never safe to insert.
pub(super) const REAL_GRAVEYARD_ZONE: u64 = 1;

pub(super) const WHOLE_GRAVEYARD_ZONE_ROW: &str = r#"{
    "safe_loc_id": { "type": "u32", "value": 105 },
    "zone_id": { "type": "u32", "value": 12 },
    "faction": { "type": "u32", "value": 0 }
}"#;

pub(super) fn graveyard_zone_claim(row_id: u64, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_graveyard_zone","key":{{"row_id":{row_id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

pub(super) fn class_level_stats_claim(
    class: u8,
    level: u8,
    operation: &str,
    fields: &str,
) -> String {
    format!(
        r#"{{"table":"game_class_level_stats","key":{{"class":{class},"level":{level}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

// ---- spellmeta ----

/// A `game_spell_learn` row inside the Package spell metadata range, safe to insert.
pub(super) const PACKAGE_SPELL_LEARN: u64 = 14_000_001;

/// A real imported `game_spell_learn` row, safe to update and never safe to insert.
pub(super) const REAL_SPELL_LEARN: u64 = 1;

pub(super) const WHOLE_SPELL_CHAIN_ROW: &str = r#"{
    "prev_spell": { "type": "u32", "value": 133 },
    "first_spell": { "type": "u32", "value": 133 },
    "rank": { "type": "u8", "value": 2 },
    "req_spell": { "type": "u32", "value": 0 }
}"#;

pub(super) const WHOLE_SPELL_LEARN_ROW: &str = r#"{
    "parent_spell": { "type": "u32", "value": 133 },
    "learn_spell": { "type": "u32", "value": 168 }
}"#;

pub(super) fn spell_chain_claim(spell_id: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_spell_chain","key":{{"spell_id":{spell_id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

pub(super) fn spell_learn_claim(id: u64, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_spell_learn","key":{{"id":{id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

// ---- creatures ----

/// A creature identifier inside the Package creature range, safe to insert. One band covers the
/// template and the spawn.
pub(super) const PACKAGE_CREATURE: u32 = 15_000_001;

/// A real imported creature template, safe to update and never safe to insert.
pub(super) const REAL_CREATURE: u32 = 6; // Kobold Vermin.

/// A real imported spawn identifier.
pub(super) const REAL_CREATURE_SPAWN: u32 = 4_242;

/// The map a spatial fixture claim sits on: Eastern Kingdoms.
pub(super) const REAL_MAP: u32 = 0;

pub(super) const WHOLE_CREATURE_TEMPLATE_ROW: &str = r#"{
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

pub(super) fn creature_template_claim(entry: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_creature_template","key":{{"entry":{entry}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

pub(super) fn creature_spawn_claim(
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
pub(super) const PACKAGE_GAMEOBJECT: u32 = 16_000_001;

/// A real imported gameobject spawn identifier.
pub(super) const REAL_GAMEOBJECT_SPAWN: u32 = 7_777;

pub(super) const WHOLE_GAMEOBJECT_TEMPLATE_ROW: &str = r#"{
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

pub(super) fn gameobject_template_claim(entry: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_gameobject_template","key":{{"entry":{entry}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

pub(super) fn gameobject_spawn_claim(
    map_id: u32,
    spawn_id: u32,
    operation: &str,
    fields: &str,
) -> String {
    format!(
        r#"{{"table":"game_gameobject","key":{{"map_id":{map_id},"spawn_id":{spawn_id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

// ---- creature-ai ----

/// A broadcast text inside the Package EventAI range, safe to insert. One band covers the text,
/// the summon placement and the quest event requirement.
pub(super) const PACKAGE_BROADCAST_TEXT: u32 = 17_000_001;

/// A summon placement inside the Package EventAI range.
pub(super) const PACKAGE_SUMMON: u32 = 17_000_002;

/// A quest event requirement inside the Package EventAI range.
pub(super) const PACKAGE_QUEST_EVENT_REQUIREMENT: u64 = 17_000_003;

/// A real imported broadcast text identifier.
pub(super) const REAL_BROADCAST_TEXT: u32 = 900;

pub(super) const WHOLE_BROADCAST_TEXT_ROW: &str = r#"{
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

pub(super) const WHOLE_SUMMON_ROW: &str = r#"{
    "x": { "type": "f32", "value": -8949.95 },
    "y": { "type": "f32", "value": -132.493 },
    "z": { "type": "f32", "value": 83.5312 },
    "orientation": { "type": "f32", "value": 0.0 },
    "lifetime_ms": { "type": "u32", "value": 30000 }
}"#;

pub(super) const WHOLE_QUEST_EVENT_REQUIREMENT_ROW: &str = r#"{
    "quest_entry": { "type": "u32", "value": 8000001 }
}"#;

pub(super) fn broadcast_text_claim(id: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_creature_ai_broadcast_text","key":{{"id":{id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

pub(super) fn summon_claim(id: u32, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_creature_ai_summon","key":{{"id":{id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

pub(super) fn quest_event_requirement_claim(id: u64, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"table":"game_quest_event_requirement","key":{{"id":{id}}},"operation":"{operation}","fields":{fields}}}"#
    )
}

/// A value of the column's declared type, so a test can claim any column without spelling out what
/// it holds.
pub(super) fn some_value(column: Column) -> FieldValue {
    match column.ty {
        FieldType::U8 => FieldValue::U8(7),
        FieldType::U16 => FieldValue::U16(7),
        FieldType::U32 => FieldValue::U32(7),
        FieldType::U64 => FieldValue::U64(7),
        FieldType::I32 => FieldValue::I32(-7),
        FieldType::F32 => FieldValue::F32(7.5),
        FieldType::Bool => FieldValue::Bool(true),
        FieldType::Str => FieldValue::Str("seven".to_owned()),
    }
}

/// Reads a set of artifacts as one payload, the way the reducer does.
pub(super) fn plan(artifacts: &[String]) -> Result<ApplyPlan, String> {
    ApplyPlan::read(&artifacts.join(&ARTIFACT_SEPARATOR.to_string()))
}
