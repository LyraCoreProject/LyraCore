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
