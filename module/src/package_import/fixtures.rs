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
