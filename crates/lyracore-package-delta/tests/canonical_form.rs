//! Canonical serialization: equivalent artifacts must produce identical bytes.

mod common;

use common::{artifact, effect_claim, one_spell_update, spell_claim, HASH_A, PACKAGE_SPELL};
use lyracore_package_delta::{canonicalize, PackageDelta};

#[test]
fn member_order_whitespace_and_number_spelling_do_not_change_the_bytes() {
    // The same two claims, written two ways: reordered members, different indentation, a padded
    // unsigned 64-bit string, and a float spelled with a trailing zero.
    let spelled_one = format!(
        r#"{{
            "version": 1,
            "package": "example.pkg",
            "source_hash": "{HASH_A}",
            "claims": [
                {{
                    "table": "game_spell",
                    "key": {{ "spell_id": 133 }},
                    "operation": "update",
                    "fields": {{
                        "cooldown_ms": {{ "type": "u32", "value": 1500 }},
                        "family_flags": {{ "type": "u64", "value": "42" }}
                    }}
                }},
                {{
                    "table": "game_spell_effect",
                    "key": {{ "spell_id": 133, "effect_index": 0 }},
                    "operation": "update",
                    "fields": {{ "per_level": {{ "type": "f32", "value": 1.5 }} }}
                }}
            ]
        }}"#
    );
    let spelled_two = format!(
        r#"{{"claims":[{{"fields":{{"per_level":{{"value":1.50,"type":"f32"}}}},"operation":"update","key":{{"effect_index":0,"spell_id":133}},"table":"game_spell_effect"}},{{"operation":"update","fields":{{"family_flags":{{"type":"u64","value":"0042"}},"cooldown_ms":{{"value":1500,"type":"u32"}}}},"key":{{"spell_id":133}},"table":"game_spell"}}],"source_hash":"{HASH_A}","package":"example.pkg","version":1}}"#
    );

    let one = canonicalize(&spelled_one).expect("first spelling parses");
    let two = canonicalize(&spelled_two).expect("second spelling parses");

    assert_eq!(one, two);
}

/// The bytes are frozen, not merely self-consistent.
///
/// A shard records `game_package_import.artifact_hash` as the BLAKE3 digest of these bytes, and a
/// Package's Build Identity records the artifact's own hash. Both are stored on live realms, so a
/// canonical form that moved — even in a way that still round-trips — would invalidate every stored
/// digest and force a realm-wide reapply. This case pins one artifact that exercises every value
/// type, both key shapes and the claim ordering, so any such move fails here first.
#[test]
fn a_frozen_artifact_still_canonicalizes_to_the_same_bytes() {
    let written = format!(
        r#"{{
            "package": "example.frozen",
            "version": 1,
            "claims": [
                {{
                    "table": "game_spell_effect",
                    "operation": "update",
                    "key": {{ "effect_index": 0, "spell_id": 133 }},
                    "fields": {{
                        "per_level": {{ "type": "f32", "value": 1.50 }},
                        "base_points": {{ "type": "i32", "value": -120 }}
                    }}
                }},
                {{
                    "table": "game_spell",
                    "operation": "update",
                    "key": {{ "spell_id": 133 }},
                    "fields": {{
                        "power_type": {{ "type": "u8", "value": 0 }},
                        "name": {{ "type": "string", "value": "Kindled\tBolt" }},
                        "is_negative": {{ "type": "bool", "value": true }},
                        "family_flags": {{ "type": "u64", "value": "18446744073709551615" }},
                        "cooldown_ms": {{ "type": "u32", "value": 1500 }},
                        "aura_interrupt": {{ "type": "u16", "value": 3 }}
                    }}
                }}
            ],
            "source_hash": "{HASH_A}"
        }}"#
    );

    let canonical = canonicalize(&written).expect("the artifact parses");

    assert_eq!(
        canonical,
        format!(
            r#"{{"version":1,"package":"example.frozen","source_hash":"{HASH_A}","claims":[{{"table":"game_spell","key":{{"spell_id":133}},"operation":"update","fields":{{"aura_interrupt":{{"type":"u16","value":3}},"cooldown_ms":{{"type":"u32","value":1500}},"family_flags":{{"type":"u64","value":"18446744073709551615"}},"is_negative":{{"type":"bool","value":true}},"name":{{"type":"string","value":"Kindled\tBolt"}},"power_type":{{"type":"u8","value":0}}}}}},{{"table":"game_spell_effect","key":{{"spell_id":133,"effect_index":0}},"operation":"update","fields":{{"base_points":{{"type":"i32","value":-120}},"per_level":{{"type":"f32","value":1.5}}}}}}]}}"#
        )
    );
}

#[test]
fn the_canonical_form_reads_back_as_itself() {
    let source = one_spell_update(
        "example.pkg",
        133,
        r#"{"cooldown_ms":{"type":"u32","value":1500}}"#,
    );

    let once = canonicalize(&source).expect("artifact parses");
    let twice = canonicalize(&once).expect("canonical form parses");

    assert_eq!(once, twice);
}

#[test]
fn the_canonical_form_has_no_whitespace_and_a_fixed_member_order() {
    let source = one_spell_update(
        "example.pkg",
        133,
        r#"{ "gcd_ms": { "type": "u32", "value": 1500 } }"#,
    );

    let canonical = canonicalize(&source).expect("artifact parses");

    assert_eq!(
        canonical,
        format!(
            r#"{{"version":1,"package":"example.pkg","source_hash":"{HASH_A}","claims":[{{"table":"game_spell","key":{{"spell_id":133}},"operation":"update","fields":{{"gcd_ms":{{"type":"u32","value":1500}}}}}}]}}"#
        )
    );
}

#[test]
fn an_unsigned_64_bit_value_is_written_as_a_decimal_string() {
    let source = one_spell_update(
        "example.pkg",
        133,
        r#"{"family_flags":{"type":"u64","value":"18446744073709551615"}}"#,
    );

    let canonical = canonicalize(&source).expect("artifact parses");

    // The largest u64 survives intact. Through a JSON number it would have come back as
    // 18446744073709551616, with nothing to notice the change.
    assert!(
        canonical.contains(r#""family_flags":{"type":"u64","value":"18446744073709551615"}"#),
        "{canonical}"
    );
}

#[test]
fn a_float_column_always_carries_a_decimal_point() {
    let source = artifact(
        "example.pkg",
        &effect_claim(
            133,
            0,
            "update",
            r#"{"per_level":{"type":"f32","value":2},"radius_yd":{"type":"f32","value":8.00}}"#,
        ),
    );

    let canonical = canonicalize(&source).expect("artifact parses");

    assert!(
        canonical.contains(r#""per_level":{"type":"f32","value":2.0}"#),
        "{canonical}"
    );
    assert!(
        canonical.contains(r#""radius_yd":{"type":"f32","value":8.0}"#),
        "{canonical}"
    );
}

#[test]
fn claims_are_ordered_by_table_then_spell_then_effect_index() {
    let field = r#"{"base_points":{"type":"i32","value":1}}"#;
    let header = r#"{"gcd_ms":{"type":"u32","value":1500}}"#;
    let claims = [
        effect_claim(200, 2, "update", field),
        spell_claim(200, "update", header),
        effect_claim(133, 1, "update", field),
        effect_claim(200, 0, "update", field),
        spell_claim(133, "update", header),
    ]
    .join(",");

    let delta = PackageDelta::parse(&artifact("example.pkg", &claims)).expect("artifact parses");

    let order: Vec<String> = delta
        .claims()
        .iter()
        .map(|claim| format!("{} {}", claim.table(), claim.key()))
        .collect();
    assert_eq!(
        order,
        vec![
            "game_spell {spell_id=133}",
            "game_spell {spell_id=200}",
            "game_spell_effect {spell_id=133, effect_index=1}",
            "game_spell_effect {spell_id=200, effect_index=0}",
            "game_spell_effect {spell_id=200, effect_index=2}",
        ]
    );
}

#[test]
fn a_string_has_one_canonical_escaping() {
    // `bs` is a single backslash. Building the JSON this way keeps the long escapes out of the Rust
    // literal, where they would be a second layer of escaping to reason about.
    let bs = "\\";
    // The input spells a tab and a capital A as long escapes, next to a quote and a backslash.
    let written = format!("{bs}u0009{bs}u0041{bs}u0007{bs}\"{bs}{bs}");
    let source = one_spell_update(
        "example.pkg",
        PACKAGE_SPELL,
        &format!(r#"{{"name":{{"type":"string","value":"{written}"}}}}"#),
    );

    let canonical = canonicalize(&source).expect("artifact parses");

    // Canonical form takes the short escape where JSON has one, leaves a printable character alone,
    // and keeps the long form only for a control character that has no short escape.
    let expected = format!("{bs}tA{bs}u0007{bs}\"{bs}{bs}");
    assert!(
        canonical.contains(&format!(
            r#""name":{{"type":"string","value":"{expected}"}}"#
        )),
        "{canonical}"
    );
}

#[test]
fn a_character_json_does_not_require_escaping_stays_as_itself() {
    let source = one_spell_update(
        "example.pkg",
        PACKAGE_SPELL,
        "{\"name\":{\"type\":\"string\",\"value\":\"Caf\u{e9} Bolt\"}}",
    );

    let canonical = canonicalize(&source).expect("artifact parses");

    // Escaping it would be a second spelling of one artifact, which is what canonical form exists
    // to prevent.
    assert!(canonical.contains("\"Caf\u{e9} Bolt\""), "{canonical}");
}
