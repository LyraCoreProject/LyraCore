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
