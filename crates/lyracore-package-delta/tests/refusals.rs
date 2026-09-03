//! Everything a Package Delta refuses, and refuses before an applier could act on any of it.

mod common;

use common::{
    artifact, creature_cast_claim, effect_claim, one_spell_update, spell_claim, HASH_A,
    PACKAGE_SPELL, REAL_CREATURE_CAST, REAL_SPELL, WHOLE_EFFECT_ROW, WHOLE_SPELL_ROW,
};
use lyracore_package_delta::{DeltaError, FieldType, PackageDelta, Table};

fn refuse(json: &str) -> DeltaError {
    PackageDelta::parse(json).expect_err("the artifact must be refused")
}

const A_COOLDOWN: &str = r#"{"cooldown_ms":{"type":"u32","value":1500}}"#;

#[test]
fn bytes_that_are_not_json_are_refused() {
    assert!(matches!(refuse("{ not json"), DeltaError::Malformed(_)));
}

#[test]
fn an_unknown_version_is_refused() {
    let json =
        format!(r#"{{"version":2,"package":"example.pkg","source_hash":"{HASH_A}","claims":[]}}"#);

    assert_eq!(
        refuse(&json),
        DeltaError::UnsupportedVersion {
            found: "2".to_owned()
        }
    );
}

#[test]
fn an_unknown_table_is_refused() {
    let claim = r#"{"table":"game_item","key":{"spell_id":133},"operation":"update","fields":{}}"#;

    assert_eq!(
        refuse(&artifact("example.pkg", claim)),
        DeltaError::UnknownTable {
            found: "game_item".to_owned()
        }
    );
}

#[test]
fn an_unknown_column_is_refused() {
    let json = one_spell_update(
        "example.pkg",
        REAL_SPELL,
        r#"{"crit_bonus":{"type":"u32","value":1}}"#,
    );

    assert_eq!(
        refuse(&json),
        DeltaError::UnknownField {
            table: Table::Spell,
            field: "crit_bonus".to_owned()
        }
    );
}

#[test]
fn an_unknown_type_tag_is_refused() {
    let json = one_spell_update(
        "example.pkg",
        REAL_SPELL,
        r#"{"cooldown_ms":{"type":"u128","value":1}}"#,
    );

    assert_eq!(
        refuse(&json),
        DeltaError::UnknownFieldType {
            field: "cooldown_ms".to_owned(),
            found: "u128".to_owned()
        }
    );
}

#[test]
fn a_type_the_column_does_not_take_is_refused() {
    let json = one_spell_update(
        "example.pkg",
        REAL_SPELL,
        r#"{"cooldown_ms":{"type":"u8","value":200}}"#,
    );

    assert_eq!(
        refuse(&json),
        DeltaError::FieldTypeMismatch {
            table: Table::Spell,
            field: "cooldown_ms".to_owned(),
            expected: FieldType::U32,
            found: FieldType::U8
        }
    );
}

#[test]
fn a_value_that_does_not_fit_its_declared_type_is_refused() {
    let json = one_spell_update(
        "example.pkg",
        REAL_SPELL,
        r#"{"power_type":{"type":"u8","value":256}}"#,
    );

    assert!(matches!(
        refuse(&json),
        DeltaError::ValueOutOfRange { ref field, ty: FieldType::U8, .. } if field == "power_type"
    ));
}

#[test]
fn an_unsigned_64_bit_value_written_as_a_json_number_is_refused() {
    // A JSON number cannot carry a u64 without losing precision above 2^53, so the artifact does not
    // accept one even when the value would have fitted.
    let json = one_spell_update(
        "example.pkg",
        REAL_SPELL,
        r#"{"family_flags":{"type":"u64","value":42}}"#,
    );

    assert!(matches!(
        refuse(&json),
        DeltaError::ValueOutOfRange {
            ty: FieldType::U64,
            ..
        }
    ));
}

#[test]
fn a_float_that_does_not_survive_the_artifact_is_refused() {
    let json = artifact(
        "example.pkg",
        &effect_claim(
            REAL_SPELL,
            0,
            "update",
            r#"{"per_level":{"type":"f32","value":1e300}}"#,
        ),
    );

    assert!(matches!(
        refuse(&json),
        DeltaError::ValueOutOfRange {
            ty: FieldType::F32,
            ..
        }
    ));
}

#[test]
fn a_key_missing_a_member_is_refused() {
    let claim =
        r#"{"table":"game_spell_effect","key":{"spell_id":133},"operation":"update","fields":{}}"#;

    assert!(matches!(
        refuse(&artifact("example.pkg", claim)),
        DeltaError::MalformedKey {
            table: Table::SpellEffect,
            ..
        }
    ));
}

#[test]
fn a_key_carrying_a_member_its_table_does_not_have_is_refused() {
    let claim = r#"{"table":"game_spell","key":{"spell_id":133,"effect_index":0},"operation":"update","fields":{}}"#;

    assert!(matches!(
        refuse(&artifact("example.pkg", claim)),
        DeltaError::MalformedKey {
            table: Table::Spell,
            ..
        }
    ));
}

#[test]
fn a_key_member_that_is_not_a_whole_number_is_refused() {
    let claim =
        r#"{"table":"game_spell","key":{"spell_id":"133"},"operation":"update","fields":{}}"#;

    assert!(matches!(
        refuse(&artifact("example.pkg", claim)),
        DeltaError::MalformedKey {
            table: Table::Spell,
            ..
        }
    ));
}

#[test]
fn a_key_that_is_not_an_object_is_refused() {
    let claim = r#"{"table":"game_spell","key":133,"operation":"update","fields":{}}"#;

    assert!(matches!(
        refuse(&artifact("example.pkg", claim)),
        DeltaError::MalformedKey { .. }
    ));
}

#[test]
fn row_deletion_is_refused() {
    let json = artifact("example.pkg", &spell_claim(REAL_SPELL, "delete", "{}"));

    assert_eq!(refuse(&json), DeltaError::DeleteNotSupported);
}

/// `game_creature_cast`'s key names a creature template, which no Package may invent — the worked
/// example of an update-only table. Five more follow it (`game_gossip_menu`, the two stat curves,
/// `game_start_position`, `game_areatrigger_teleport`); see `gossip_identifiers.rs` and
/// `globals_identifiers.rs` for their own coverage.
#[test]
fn an_insert_on_an_update_only_table_is_refused_by_name() {
    let json = artifact(
        "example.pkg",
        &creature_cast_claim(
            REAL_CREATURE_CAST,
            "insert",
            r#"{"spell_id":{"type":"u32","value":200}}"#,
        ),
    );

    assert_eq!(
        refuse(&json),
        DeltaError::InsertNotSupported {
            table: Table::CreatureCast
        }
    );
}

#[test]
fn an_unknown_operation_is_refused() {
    let json = artifact(
        "example.pkg",
        &spell_claim(REAL_SPELL, "upsert", A_COOLDOWN),
    );

    assert_eq!(
        refuse(&json),
        DeltaError::UnknownOperation {
            found: "upsert".to_owned()
        }
    );
}

#[test]
fn an_insert_that_does_not_carry_the_whole_row_is_refused() {
    let json = artifact(
        "example.pkg",
        &spell_claim(PACKAGE_SPELL, "insert", A_COOLDOWN),
    );

    match refuse(&json) {
        DeltaError::IncompleteInsert { table, missing } => {
            assert_eq!(table, Table::Spell);
            // Every claimable column except the one the claim carried.
            assert_eq!(missing.len(), 23);
            assert!(missing.contains(&"name".to_owned()));
            assert!(!missing.contains(&"cooldown_ms".to_owned()));
        }
        other => panic!("expected an incomplete insert, got {other}"),
    }
}

#[test]
fn an_update_that_states_no_change_is_refused() {
    let json = artifact("example.pkg", &spell_claim(REAL_SPELL, "update", "{}"));

    assert_eq!(
        refuse(&json),
        DeltaError::EmptyUpdate {
            table: Table::Spell
        }
    );
}

#[test]
fn claiming_a_primary_key_column_as_a_field_is_refused() {
    let json = one_spell_update(
        "example.pkg",
        REAL_SPELL,
        r#"{"spell_id":{"type":"u32","value":200}}"#,
    );

    assert_eq!(
        refuse(&json),
        DeltaError::KeyColumnClaimed {
            table: Table::Spell,
            field: "spell_id".to_owned()
        }
    );
}

#[test]
fn claiming_the_derived_effect_key_as_a_field_is_refused() {
    let json = artifact(
        "example.pkg",
        &effect_claim(
            REAL_SPELL,
            0,
            "update",
            r#"{"id":{"type":"u64","value":"532"}}"#,
        ),
    );

    assert_eq!(
        refuse(&json),
        DeltaError::KeyColumnClaimed {
            table: Table::SpellEffect,
            field: "id".to_owned()
        }
    );
}

#[test]
fn one_package_claiming_a_row_twice_is_refused() {
    let claims = [
        spell_claim(REAL_SPELL, "update", A_COOLDOWN),
        spell_claim(
            REAL_SPELL,
            "update",
            r#"{"gcd_ms":{"type":"u32","value":1}}"#,
        ),
    ]
    .join(",");

    assert!(matches!(
        refuse(&artifact("example.pkg", &claims)),
        DeltaError::DuplicateClaim {
            table: Table::Spell,
            ..
        }
    ));
}

#[test]
fn an_unexpected_member_is_refused_rather_than_ignored() {
    let json = format!(
        r#"{{"version":1,"package":"example.pkg","source_hash":"{HASH_A}","claims":[],"priority":9}}"#
    );

    assert_eq!(
        refuse(&json),
        DeltaError::UnexpectedMember {
            path: "the artifact root".to_owned(),
            member: "priority".to_owned()
        }
    );
}

#[test]
fn a_missing_member_is_refused() {
    let json = r#"{"version":1,"package":"example.pkg","claims":[]}"#;

    assert_eq!(
        refuse(json),
        DeltaError::MissingMember {
            path: "source_hash".to_owned()
        }
    );
}

#[test]
fn a_package_identity_that_is_not_a_slug_is_refused() {
    let json =
        format!(r#"{{"version":1,"package":"Example Pkg","source_hash":"{HASH_A}","claims":[]}}"#);

    assert_eq!(
        refuse(&json),
        DeltaError::InvalidPackageId {
            found: "Example Pkg".to_owned()
        }
    );
}

#[test]
fn a_source_hash_that_is_not_a_digest_is_refused() {
    let json = r#"{"version":1,"package":"example.pkg","source_hash":"deadbeef","claims":[]}"#;

    assert_eq!(
        refuse(json),
        DeltaError::InvalidSourceHash {
            found: "deadbeef".to_owned()
        }
    );
}

#[test]
fn a_partial_update_and_a_complete_insert_are_both_accepted() {
    let claims = [
        spell_claim(PACKAGE_SPELL, "insert", WHOLE_SPELL_ROW),
        effect_claim(PACKAGE_SPELL, 0, "insert", WHOLE_EFFECT_ROW),
        spell_claim(REAL_SPELL, "update", A_COOLDOWN),
    ]
    .join(",");

    let delta = PackageDelta::parse(&artifact("example.pkg", &claims)).expect("artifact parses");

    assert_eq!(delta.claims().len(), 3);
}
