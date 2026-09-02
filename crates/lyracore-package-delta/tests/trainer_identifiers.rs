//! The trainers family's identifier policy. `game_trainer_spell` follows the loot shape (band
//! checked against the row's own surrogate `id`), the same as `game_creature_spell` in
//! `cast_identifiers.rs`.

mod common;

use common::{
    trainer_spell_claim, PACKAGE_TRAINER_SPELL, REAL_TRAINER_SPELL, WHOLE_TRAINER_SPELL_ROW,
};
use lyracore_package_delta::{
    is_fixture_reserved_trainer_id, is_package_trainer_id, DeltaError, PackageDelta, PrimaryKey,
    Table, PACKAGE_TRAINER_ID_CEIL, PACKAGE_TRAINER_ID_FLOOR,
};

fn artifact(package: &str, claims: &str) -> String {
    format!(
        r#"{{"version":1,"package":"{package}","source_hash":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","claims":[{claims}]}}"#
    )
}

const A_COST: &str = r#"{"cost":{"type":"u32","value":750}}"#;

#[test]
fn a_trainer_spell_key_is_the_id_itself() {
    let key = PrimaryKey::trainer_spell(11_000_001).expect("a Package trainer row is claimable");

    assert_eq!(key.row_id(), 11_000_001);
}

#[test]
fn a_package_may_insert_a_trainer_spell_row_inside_the_package_range() {
    for id in [
        PACKAGE_TRAINER_ID_FLOOR,
        11_500_000,
        PACKAGE_TRAINER_ID_CEIL,
    ] {
        let json = artifact(
            "example.pkg",
            &trainer_spell_claim(id, "insert", WHOLE_TRAINER_SPELL_ROW),
        );

        PackageDelta::parse(&json)
            .unwrap_or_else(|e| panic!("trainer spell row {id} must insert: {e}"));
    }
}

#[test]
fn an_insert_outside_the_package_range_is_refused() {
    for id in [1, PACKAGE_TRAINER_ID_FLOOR - 1, PACKAGE_TRAINER_ID_CEIL + 1] {
        let json = artifact(
            "example.pkg",
            &trainer_spell_claim(id, "insert", WHOLE_TRAINER_SPELL_ROW),
        );

        assert_eq!(
            PackageDelta::parse(&json).expect_err("the insert must be refused"),
            DeltaError::TrainerIdNotClientSafe { id },
            "trainer spell row {id}"
        );
    }
}

#[test]
fn an_update_may_name_a_real_imported_trainer_spell_row() {
    let json = artifact(
        "example.pkg",
        &trainer_spell_claim(REAL_TRAINER_SPELL, "update", A_COST),
    );

    let delta = PackageDelta::parse(&json).expect("a real trainer spell row is updatable");

    assert_eq!(delta.claims()[0].key().row_id(), REAL_TRAINER_SPELL);
}

#[test]
fn no_operation_may_claim_the_project_wide_fixture_reserved_band() {
    for id in [5_090_000u64, 5_090_050, 5_099_999] {
        let update = artifact("example.pkg", &trainer_spell_claim(id, "update", A_COST));
        assert_eq!(
            PackageDelta::parse(&update).expect_err("the update must be refused"),
            DeltaError::TrainerIdFixtureReserved { id },
            "update on fixture trainer spell row {id}"
        );

        let insert = artifact(
            "example.pkg",
            &trainer_spell_claim(id, "insert", WHOLE_TRAINER_SPELL_ROW),
        );
        assert_eq!(
            PackageDelta::parse(&insert).expect_err("the insert must be refused"),
            DeltaError::TrainerIdFixtureReserved { id },
            "insert on fixture trainer spell row {id}"
        );
    }
}

#[test]
fn id_zero_is_not_a_row() {
    let json = artifact("example.pkg", &trainer_spell_claim(0, "update", A_COST));

    assert!(matches!(
        PackageDelta::parse(&json).expect_err("the claim must be refused"),
        DeltaError::MalformedKey { .. }
    ));
}

#[test]
fn the_package_range_is_disjoint_from_every_reserved_identifier() {
    for id in [5_090_000u64, 5_090_050, 5_099_999] {
        assert!(
            !is_package_trainer_id(id),
            "reserved trainer spell row {id} must not be insertable by a Package"
        );
    }

    for id in [PACKAGE_TRAINER_ID_FLOOR, PACKAGE_TRAINER_ID_CEIL] {
        assert!(
            !is_fixture_reserved_trainer_id(id),
            "Package trainer spell row {id} must not be fixture-reserved"
        );
    }
}

#[test]
fn an_incomplete_trainer_spell_insert_is_refused() {
    let json = artifact(
        "example.pkg",
        &trainer_spell_claim(PACKAGE_TRAINER_SPELL, "insert", A_COST),
    );

    match PackageDelta::parse(&json).expect_err("the insert must be refused") {
        DeltaError::IncompleteInsert { table, missing } => {
            assert_eq!(table, Table::TrainerSpell);
            assert_eq!(missing.len(), 5);
            assert!(missing.contains(&"trainer_entry".to_owned()));
            assert!(!missing.contains(&"cost".to_owned()));
        }
        other => panic!("expected an incomplete insert, got {other}"),
    }
}

/// The cast and trainer bands are one decade apart (10M/11M), so an identifier that reads as a
/// valid trainer row must not also read as a cast row.
#[test]
fn the_cast_and_trainer_bands_do_not_overlap() {
    use lyracore_package_delta::is_package_cast_id;

    for id in [PACKAGE_TRAINER_ID_FLOOR, PACKAGE_TRAINER_ID_CEIL] {
        assert!(
            !is_package_cast_id(id),
            "trainer row {id} reads as a cast row"
        );
    }
}
