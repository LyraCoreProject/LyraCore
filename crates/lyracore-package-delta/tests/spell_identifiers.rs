//! The spell identifier policy: which rows a Package may claim, and how a child effect key is
//! derived from the spell it belongs to.

mod common;

use common::{artifact, effect_claim, spell_claim, WHOLE_EFFECT_ROW, WHOLE_SPELL_ROW};
use lyracore_package_delta::{
    is_fixture_reserved_spell_id, is_package_spell_id, DeltaError, PackageDelta, PrimaryKey,
    MAX_SPELL_EFFECT_INDEX, PACKAGE_SPELL_ID_CEIL, PACKAGE_SPELL_ID_FLOOR,
};

/// Every fixture spell the seed and the fixtures module park in the reserved cluster. A Package that
/// overwrote one of these would turn the Module's own tests into a report about the Package.
const FIXTURE_SPELLS: [u32; 6] = [50_000, 50_072, 50_110, 50_137, 50_310, 50_311];

const A_COOLDOWN: &str = r#"{"cooldown_ms":{"type":"u32","value":1500}}"#;
const A_BASE_POINT: &str = r#"{"base_points":{"type":"i32","value":7}}"#;

#[test]
fn a_spell_effect_key_is_the_spell_shifted_left_by_two_plus_the_effect_index() {
    // Worked by hand from the formula the Module's `SpellEffect.id` column already uses:
    // 133 * 4 = 532, plus the effect index.
    let expected = [(0_u8, 532_u64), (1, 533), (2, 534)];

    for (effect_index, row_id) in expected {
        let key =
            PrimaryKey::spell_effect(133, effect_index).expect("a real spell has this effect");
        assert_eq!(key.row_id(), row_id);
    }
}

#[test]
fn a_package_spell_effect_key_stays_inside_a_u64() {
    // 6000001 * 4 = 24000004, plus effect index 2.
    let key = PrimaryKey::spell_effect(6_000_001, 2).expect("a Package spell has this effect");

    assert_eq!(key.row_id(), 24_000_006);
}

#[test]
fn a_spell_header_key_is_the_spell_itself() {
    let key = PrimaryKey::spell(6_000_001).expect("a Package spell is claimable");

    assert_eq!(key.row_id(), 6_000_001);
}

#[test]
fn a_package_may_insert_a_spell_inside_the_package_range() {
    for spell_id in [PACKAGE_SPELL_ID_FLOOR, 6_500_000, PACKAGE_SPELL_ID_CEIL] {
        let json = artifact(
            "example.pkg",
            &spell_claim(spell_id, "insert", WHOLE_SPELL_ROW),
        );

        PackageDelta::parse(&json).unwrap_or_else(|e| panic!("spell {spell_id} must insert: {e}"));
    }
}

#[test]
fn an_insert_outside_the_package_range_is_refused() {
    // A real client spell, a curated trainer identifier, the value just below the floor, and the
    // value just above the ceiling.
    for spell_id in [
        133,
        5_200_000,
        PACKAGE_SPELL_ID_FLOOR - 1,
        PACKAGE_SPELL_ID_CEIL + 1,
    ] {
        let json = artifact(
            "example.pkg",
            &spell_claim(spell_id, "insert", WHOLE_SPELL_ROW),
        );

        assert_eq!(
            PackageDelta::parse(&json).expect_err("the insert must be refused"),
            DeltaError::SpellIdNotClientSafe { spell_id },
            "spell {spell_id}"
        );
    }
}

#[test]
fn an_inserted_effect_obeys_the_same_range_as_its_spell() {
    let json = artifact(
        "example.pkg",
        &effect_claim(133, 0, "insert", WHOLE_EFFECT_ROW),
    );

    assert_eq!(
        PackageDelta::parse(&json).expect_err("the insert must be refused"),
        DeltaError::SpellIdNotClientSafe { spell_id: 133 }
    );
}

#[test]
fn an_update_may_name_a_real_client_spell() {
    // Tuning a real spell is the common case, and the reason the range applies to inserts alone: a
    // Package that could not touch spell 133 could not change Fireball.
    let json = artifact("example.pkg", &spell_claim(133, "update", A_COOLDOWN));

    let delta = PackageDelta::parse(&json).expect("a real spell is updatable");

    assert_eq!(delta.claims()[0].key().spell_id(), 133);
}

#[test]
fn no_operation_may_claim_a_fixture_spell() {
    for spell_id in FIXTURE_SPELLS {
        let update = artifact("example.pkg", &spell_claim(spell_id, "update", A_COOLDOWN));
        assert_eq!(
            PackageDelta::parse(&update).expect_err("the update must be refused"),
            DeltaError::SpellIdFixtureReserved { spell_id },
            "update on fixture spell {spell_id}"
        );

        let insert = artifact(
            "example.pkg",
            &spell_claim(spell_id, "insert", WHOLE_SPELL_ROW),
        );
        assert_eq!(
            PackageDelta::parse(&insert).expect_err("the insert must be refused"),
            DeltaError::SpellIdFixtureReserved { spell_id },
            "insert on fixture spell {spell_id}"
        );
    }
}

#[test]
fn no_operation_may_claim_a_fixture_effect() {
    let json = artifact(
        "example.pkg",
        &effect_claim(50_072, 0, "update", A_BASE_POINT),
    );

    assert_eq!(
        PackageDelta::parse(&json).expect_err("the update must be refused"),
        DeltaError::SpellIdFixtureReserved { spell_id: 50_072 }
    );
}

#[test]
fn no_operation_may_claim_the_project_wide_fixture_reserved_band() {
    for spell_id in [5_090_000, 5_090_100, 5_099_999] {
        let json = artifact("example.pkg", &spell_claim(spell_id, "update", A_COOLDOWN));

        assert_eq!(
            PackageDelta::parse(&json).expect_err("the update must be refused"),
            DeltaError::SpellIdFixtureReserved { spell_id },
            "spell {spell_id}"
        );
    }
}

#[test]
fn the_bands_next_to_the_fixture_clusters_stay_claimable() {
    // The edges matter: one off in either direction and a real spell becomes unclaimable, or a
    // fixture becomes claimable.
    for spell_id in [49_999, 51_000, 5_089_999, 5_100_000] {
        let json = artifact("example.pkg", &spell_claim(spell_id, "update", A_COOLDOWN));

        PackageDelta::parse(&json)
            .unwrap_or_else(|e| panic!("spell {spell_id} must stay updatable: {e}"));
    }
}

#[test]
fn an_effect_index_no_spell_has_is_refused() {
    let json = artifact(
        "example.pkg",
        &effect_claim(133, MAX_SPELL_EFFECT_INDEX + 1, "update", A_BASE_POINT),
    );

    assert_eq!(
        PackageDelta::parse(&json).expect_err("the claim must be refused"),
        DeltaError::EffectIndexOutOfRange {
            effect_index: MAX_SPELL_EFFECT_INDEX + 1
        }
    );
}

#[test]
fn every_effect_index_a_spell_has_is_claimable() {
    for effect_index in 0..=MAX_SPELL_EFFECT_INDEX {
        let json = artifact(
            "example.pkg",
            &effect_claim(133, effect_index, "update", A_BASE_POINT),
        );

        PackageDelta::parse(&json)
            .unwrap_or_else(|e| panic!("effect {effect_index} must be claimable: {e}"));
    }
}

#[test]
fn spell_zero_is_not_a_row() {
    let json = artifact("example.pkg", &spell_claim(0, "update", A_COOLDOWN));

    assert!(matches!(
        PackageDelta::parse(&json).expect_err("the claim must be refused"),
        DeltaError::MalformedKey { .. }
    ));
}

#[test]
fn the_package_range_is_disjoint_from_every_reserved_identifier() {
    for spell_id in FIXTURE_SPELLS
        .into_iter()
        .chain([50_999, 5_090_000, 5_099_999, 5_200_000, 5_200_499])
    {
        assert!(
            !is_package_spell_id(spell_id),
            "reserved spell {spell_id} must not be insertable by a Package"
        );
    }

    for spell_id in [PACKAGE_SPELL_ID_FLOOR, PACKAGE_SPELL_ID_CEIL] {
        assert!(
            !is_fixture_reserved_spell_id(spell_id),
            "Package spell {spell_id} must not be fixture-reserved"
        );
    }
}
