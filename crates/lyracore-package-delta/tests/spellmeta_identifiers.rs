//! The spellmeta family's identifier policy. `game_spell_learn` follows the loot shape on its own
//! band. `game_spell_chain` and `game_spell_proc_event` key on a spell identifier instead, so what
//! a Package may invent there is exactly what it may invent in `game_spell`.

mod common;

use common::{
    spell_chain_claim, spell_learn_claim, spell_proc_event_claim, PACKAGE_SPELL, REAL_SPELL,
    REAL_SPELL_LEARN, WHOLE_SPELL_CHAIN_ROW, WHOLE_SPELL_LEARN_ROW,
};
use lyracore_package_delta::{
    is_fixture_reserved_spellmeta_id, is_package_globals_id, is_package_spellmeta_id, DeltaError,
    PackageDelta, PrimaryKey, Table, PACKAGE_SPELLMETA_ID_CEIL, PACKAGE_SPELLMETA_ID_FLOOR,
    PACKAGE_SPELL_ID_CEIL, PACKAGE_SPELL_ID_FLOOR,
};

fn artifact(package: &str, claims: &str) -> String {
    format!(
        r#"{{"version":1,"package":"{package}","source_hash":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","claims":[{claims}]}}"#
    )
}

const A_RANK: &str = r#"{"rank":{"type":"u8","value":3}}"#;
const A_LEARN: &str = r#"{"learn_spell":{"type":"u32","value":168}}"#;
const AN_ICD: &str = r#"{"icd_ms":{"type":"u32","value":4000}}"#;

#[test]
fn a_spell_learn_key_is_the_id_itself() {
    let key = PrimaryKey::spell_learn(14_000_001).expect("a Package dependent row is claimable");

    assert_eq!(key.row_id(), 14_000_001);
    assert_eq!(key.table(), Table::SpellLearn);
}

#[test]
fn a_package_may_insert_a_spell_learn_row_inside_the_package_range() {
    for id in [
        PACKAGE_SPELLMETA_ID_FLOOR,
        14_500_000,
        PACKAGE_SPELLMETA_ID_CEIL,
    ] {
        let json = artifact(
            "example.pkg",
            &spell_learn_claim(id, "insert", WHOLE_SPELL_LEARN_ROW),
        );

        PackageDelta::parse(&json)
            .unwrap_or_else(|e| panic!("spell learn row {id} must insert: {e}"));
    }
}

#[test]
fn a_spell_learn_insert_outside_the_package_range_is_refused() {
    for id in [
        1u64,
        PACKAGE_SPELLMETA_ID_FLOOR - 1,
        PACKAGE_SPELLMETA_ID_CEIL + 1,
    ] {
        let json = artifact(
            "example.pkg",
            &spell_learn_claim(id, "insert", WHOLE_SPELL_LEARN_ROW),
        );

        assert_eq!(
            PackageDelta::parse(&json).expect_err("the insert must be refused"),
            DeltaError::SpellmetaIdNotClientSafe { id },
            "spell learn row {id}"
        );
    }
}

/// A rank-chain row's key IS a spell identifier, so a Package may only invent one for a spell it
/// could have invented itself. The spellmeta band does not apply here and must not.
#[test]
fn a_spell_chain_insert_takes_the_package_spell_band() {
    for spell_id in [PACKAGE_SPELL_ID_FLOOR, PACKAGE_SPELL_ID_CEIL] {
        let json = artifact(
            "example.pkg",
            &spell_chain_claim(spell_id, "insert", WHOLE_SPELL_CHAIN_ROW),
        );

        PackageDelta::parse(&json).unwrap_or_else(|e| {
            panic!("spell chain for Package spell {spell_id} must insert: {e}")
        });
    }

    let json = artifact(
        "example.pkg",
        &spell_chain_claim(
            PACKAGE_SPELLMETA_ID_FLOOR as u32,
            "insert",
            WHOLE_SPELL_CHAIN_ROW,
        ),
    );
    assert_eq!(
        PackageDelta::parse(&json).expect_err("a spellmeta-band spell is not a spell"),
        DeltaError::SpellIdNotClientSafe {
            spell_id: PACKAGE_SPELLMETA_ID_FLOOR as u32
        }
    );
}

/// A proc overlay for a real vanilla spell can be tuned but never invented: the base import owns
/// that row's identifier, so an inserted one could neither be reconciled nor survive a reload.
#[test]
fn a_proc_overlay_for_a_real_spell_is_updatable_but_not_insertable() {
    let update = artifact(
        "example.pkg",
        &spell_proc_event_claim(REAL_SPELL, "update", AN_ICD),
    );
    PackageDelta::parse(&update).expect("a real spell's proc overlay is updatable");

    let insert = artifact(
        "example.pkg",
        &spell_proc_event_claim(
            REAL_SPELL,
            "insert",
            r#"{
                "proc_flags": { "type": "u32", "value": 16 },
                "proc_ex": { "type": "u32", "value": 0 },
                "school_mask": { "type": "u8", "value": 0 },
                "family_name": { "type": "u8", "value": 0 },
                "family_flags": { "type": "u64", "value": "0" },
                "ppm_rate": { "type": "f32", "value": 0.0 },
                "custom_chance": { "type": "u8", "value": 5 },
                "icd_ms": { "type": "u32", "value": 4000 }
            }"#,
        ),
    );
    assert_eq!(
        PackageDelta::parse(&insert).expect_err("the insert must be refused"),
        DeltaError::SpellIdNotClientSafe {
            spell_id: REAL_SPELL
        }
    );
}

#[test]
fn an_update_may_name_a_real_imported_spell_learn_row() {
    let json = artifact(
        "example.pkg",
        &spell_learn_claim(REAL_SPELL_LEARN, "update", A_LEARN),
    );

    let delta = PackageDelta::parse(&json).expect("a real dependent row is updatable");

    assert_eq!(delta.claims()[0].key().row_id(), REAL_SPELL_LEARN);
}

/// A spell-keyed spellmeta row inherits the spell family's fixture cluster too, not only the
/// project-wide band.
#[test]
fn no_operation_may_claim_a_fixture_spells_metadata() {
    for spell_id in [50_000u32, 50_500, 50_999, 5_090_000, 5_099_999] {
        let json = artifact(
            "example.pkg",
            &spell_chain_claim(spell_id, "update", A_RANK),
        );

        assert_eq!(
            PackageDelta::parse(&json).expect_err("the update must be refused"),
            DeltaError::SpellIdFixtureReserved { spell_id },
            "fixture spell {spell_id}"
        );
    }
}

#[test]
fn no_operation_may_claim_the_project_wide_fixture_reserved_band() {
    for id in [5_090_000u64, 5_090_050, 5_099_999] {
        let update = artifact("example.pkg", &spell_learn_claim(id, "update", A_LEARN));
        assert_eq!(
            PackageDelta::parse(&update).expect_err("the update must be refused"),
            DeltaError::SpellmetaIdFixtureReserved { id },
            "update on fixture spell learn row {id}"
        );
    }
}

#[test]
fn id_zero_is_not_a_row() {
    for json in [
        artifact("example.pkg", &spell_learn_claim(0, "update", A_LEARN)),
        artifact("example.pkg", &spell_chain_claim(0, "update", A_RANK)),
    ] {
        assert!(matches!(
            PackageDelta::parse(&json).expect_err("the claim must be refused"),
            DeltaError::MalformedKey { .. }
        ));
    }
}

#[test]
fn the_package_range_is_disjoint_from_every_reserved_identifier() {
    for id in [5_090_000u64, 5_090_050, 5_099_999] {
        assert!(
            !is_package_spellmeta_id(id),
            "reserved spell metadata row {id} must not be insertable by a Package"
        );
    }

    for id in [PACKAGE_SPELLMETA_ID_FLOOR, PACKAGE_SPELLMETA_ID_CEIL] {
        assert!(
            !is_fixture_reserved_spellmeta_id(id),
            "Package spell metadata row {id} must not be fixture-reserved"
        );
        assert!(
            !is_package_globals_id(id),
            "spell metadata row {id} reads as a globals row"
        );
    }
}

#[test]
fn an_incomplete_spell_chain_insert_is_refused() {
    let json = artifact(
        "example.pkg",
        &spell_chain_claim(PACKAGE_SPELL, "insert", A_RANK),
    );

    match PackageDelta::parse(&json).expect_err("the insert must be refused") {
        DeltaError::IncompleteInsert { table, missing } => {
            assert_eq!(table, Table::SpellChain);
            assert_eq!(missing.len(), 3);
            assert!(missing.contains(&"first_spell".to_owned()));
            assert!(!missing.contains(&"rank".to_owned()));
        }
        other => panic!("expected an incomplete insert, got {other}"),
    }
}
