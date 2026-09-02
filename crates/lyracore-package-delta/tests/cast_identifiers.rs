//! The casts family's identifier policy: `game_creature_spell` follows the loot shape (band
//! checked against the row's own surrogate `id`). `game_creature_cast` follows no band at all —
//! it is update-only, refused unconditionally on `insert` — the worked example of
//! [`DeltaError::InsertNotSupported`].

mod common;

use common::{
    creature_cast_claim, creature_spell_claim, PACKAGE_CREATURE_SPELL, REAL_CREATURE_CAST,
    REAL_CREATURE_SPELL, WHOLE_CREATURE_SPELL_ROW,
};
use lyracore_package_delta::{
    is_fixture_reserved_cast_id, is_package_cast_id, DeltaError, PackageDelta, PrimaryKey, Table,
    PACKAGE_CAST_ID_CEIL, PACKAGE_CAST_ID_FLOOR,
};

fn artifact(package: &str, claims: &str) -> String {
    format!(
        r#"{{"version":1,"package":"{package}","source_hash":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","claims":[{claims}]}}"#
    )
}

const A_PRIORITY: &str = r#"{"priority":{"type":"u8","value":20}}"#;
const A_SPELL_ID: &str = r#"{"spell_id":{"type":"u32","value":200}}"#;

// ---- game_creature_spell: the loot-shaped band ----

#[test]
fn a_creature_spell_key_is_the_id_itself() {
    let key = PrimaryKey::creature_spell(10_000_001).expect("a Package cast row is claimable");

    assert_eq!(key.row_id(), 10_000_001);
}

#[test]
fn a_package_may_insert_a_creature_spell_row_inside_the_package_range() {
    for id in [PACKAGE_CAST_ID_FLOOR, 10_500_000, PACKAGE_CAST_ID_CEIL] {
        let json = artifact(
            "example.pkg",
            &creature_spell_claim(id, "insert", WHOLE_CREATURE_SPELL_ROW),
        );

        PackageDelta::parse(&json)
            .unwrap_or_else(|e| panic!("creature spell row {id} must insert: {e}"));
    }
}

#[test]
fn an_insert_outside_the_package_range_is_refused() {
    for id in [1, PACKAGE_CAST_ID_FLOOR - 1, PACKAGE_CAST_ID_CEIL + 1] {
        let json = artifact(
            "example.pkg",
            &creature_spell_claim(id, "insert", WHOLE_CREATURE_SPELL_ROW),
        );

        assert_eq!(
            PackageDelta::parse(&json).expect_err("the insert must be refused"),
            DeltaError::CastIdNotClientSafe { id },
            "creature spell row {id}"
        );
    }
}

#[test]
fn an_update_may_name_a_real_imported_creature_spell_row() {
    let json = artifact(
        "example.pkg",
        &creature_spell_claim(REAL_CREATURE_SPELL, "update", A_PRIORITY),
    );

    let delta = PackageDelta::parse(&json).expect("a real creature spell row is updatable");

    assert_eq!(delta.claims()[0].key().row_id(), REAL_CREATURE_SPELL);
}

#[test]
fn no_operation_may_claim_the_project_wide_fixture_reserved_band_for_creature_spell() {
    for id in [5_090_000u64, 5_090_050, 5_099_999] {
        let update = artifact(
            "example.pkg",
            &creature_spell_claim(id, "update", A_PRIORITY),
        );
        assert_eq!(
            PackageDelta::parse(&update).expect_err("the update must be refused"),
            DeltaError::CastIdFixtureReserved { id },
            "update on fixture creature spell row {id}"
        );

        let insert = artifact(
            "example.pkg",
            &creature_spell_claim(id, "insert", WHOLE_CREATURE_SPELL_ROW),
        );
        assert_eq!(
            PackageDelta::parse(&insert).expect_err("the insert must be refused"),
            DeltaError::CastIdFixtureReserved { id },
            "insert on fixture creature spell row {id}"
        );
    }
}

#[test]
fn creature_spell_id_zero_is_not_a_row() {
    let json = artifact(
        "example.pkg",
        &creature_spell_claim(0, "update", A_PRIORITY),
    );

    assert!(matches!(
        PackageDelta::parse(&json).expect_err("the claim must be refused"),
        DeltaError::MalformedKey { .. }
    ));
}

#[test]
fn the_cast_package_range_is_disjoint_from_every_reserved_identifier() {
    for id in [5_090_000u64, 5_090_050, 5_099_999] {
        assert!(
            !is_package_cast_id(id),
            "reserved creature spell row {id} must not be insertable by a Package"
        );
    }

    for id in [PACKAGE_CAST_ID_FLOOR, PACKAGE_CAST_ID_CEIL] {
        assert!(
            !is_fixture_reserved_cast_id(id),
            "Package creature spell row {id} must not be fixture-reserved"
        );
    }
}

#[test]
fn an_incomplete_creature_spell_insert_is_refused() {
    let json = artifact(
        "example.pkg",
        &creature_spell_claim(PACKAGE_CREATURE_SPELL, "insert", A_PRIORITY),
    );

    match PackageDelta::parse(&json).expect_err("the insert must be refused") {
        DeltaError::IncompleteInsert { table, missing } => {
            assert_eq!(table, Table::CreatureSpell);
            assert_eq!(missing.len(), 4);
            assert!(missing.contains(&"creature_entry".to_owned()));
            assert!(!missing.contains(&"priority".to_owned()));
        }
        other => panic!("expected an incomplete insert, got {other}"),
    }
}

// ---- game_creature_cast: update-only, no band at all ----

#[test]
fn a_creature_cast_key_is_the_creature_entry_itself() {
    let key = PrimaryKey::creature_cast(REAL_CREATURE_CAST).expect("a real creature is claimable");

    assert_eq!(key.row_id(), u64::from(REAL_CREATURE_CAST));
}

#[test]
fn every_insert_on_game_creature_cast_is_refused_by_name_whatever_the_identifier() {
    for creature_entry in [1u32, REAL_CREATURE_CAST, PACKAGE_CAST_ID_FLOOR as u32] {
        let json = artifact(
            "example.pkg",
            &creature_cast_claim(creature_entry, "insert", A_SPELL_ID),
        );

        assert_eq!(
            PackageDelta::parse(&json).expect_err("every insert is refused"),
            DeltaError::InsertNotSupported {
                table: Table::CreatureCast
            },
            "creature_entry {creature_entry}"
        );
    }
}

#[test]
fn an_update_on_a_real_creature_entry_is_accepted() {
    let json = artifact(
        "example.pkg",
        &creature_cast_claim(REAL_CREATURE_CAST, "update", A_SPELL_ID),
    );

    let delta = PackageDelta::parse(&json).expect("a real creature's cast row is updatable");

    assert_eq!(
        delta.claims()[0].key().row_id(),
        u64::from(REAL_CREATURE_CAST)
    );
}

#[test]
fn creature_cast_entry_zero_is_not_a_creature() {
    let json = artifact("example.pkg", &creature_cast_claim(0, "update", A_SPELL_ID));

    assert!(matches!(
        PackageDelta::parse(&json).expect_err("the claim must be refused"),
        DeltaError::MalformedKey {
            table: Table::CreatureCast,
            ..
        }
    ));
}
