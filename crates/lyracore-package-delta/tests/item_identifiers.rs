//! The item identifier policy: which rows a Package may claim. Mirrors `spell_identifiers.rs`,
//! items' worked-example counterpart — items have one table, so there is no packed child key to
//! test here.

mod common;

use common::{artifact, item_claim, WHOLE_ITEM_ROW};
use lyracore_package_delta::{
    is_fixture_reserved_item_id, is_package_item_id, DeltaError, PackageDelta, PrimaryKey,
    PACKAGE_ITEM_ID_CEIL, PACKAGE_ITEM_ID_FLOOR,
};

const A_BUY_PRICE: &str = r#"{"buy_price":{"type":"u32","value":100}}"#;

#[test]
fn an_item_header_key_is_the_entry_itself() {
    let key = PrimaryKey::item(7_000_001).expect("a Package item is claimable");

    assert_eq!(key.row_id(), 7_000_001);
}

#[test]
fn a_package_may_insert_an_item_inside_the_package_range() {
    for entry in [PACKAGE_ITEM_ID_FLOOR, 7_500_000, PACKAGE_ITEM_ID_CEIL] {
        let json = artifact("example.pkg", &item_claim(entry, "insert", WHOLE_ITEM_ROW));

        PackageDelta::parse(&json).unwrap_or_else(|e| panic!("item {entry} must insert: {e}"));
    }
}

#[test]
fn an_insert_outside_the_package_range_is_refused() {
    // A real client item, the value just below the floor, and the value just above the ceiling.
    for entry in [25, PACKAGE_ITEM_ID_FLOOR - 1, PACKAGE_ITEM_ID_CEIL + 1] {
        let json = artifact("example.pkg", &item_claim(entry, "insert", WHOLE_ITEM_ROW));

        assert_eq!(
            PackageDelta::parse(&json).expect_err("the insert must be refused"),
            DeltaError::ItemIdNotClientSafe { entry },
            "item {entry}"
        );
    }
}

#[test]
fn an_update_may_name_a_real_client_item() {
    // Tuning a real item is the common case, and the reason the range applies to inserts alone.
    let json = artifact("example.pkg", &item_claim(25, "update", A_BUY_PRICE));

    let delta = PackageDelta::parse(&json).expect("a real item is updatable");

    assert_eq!(delta.claims()[0].key().row_id(), 25);
}

#[test]
fn no_operation_may_claim_the_project_wide_fixture_reserved_band() {
    // Items have no family-specific fixture cluster the way spells do — the shared band is the
    // whole check. `5_090_050` is `FIXTURE_BLADE` (`module/src/seed/fixtures.rs`).
    for entry in [5_090_000, 5_090_050, 5_099_999] {
        let update = artifact("example.pkg", &item_claim(entry, "update", A_BUY_PRICE));
        assert_eq!(
            PackageDelta::parse(&update).expect_err("the update must be refused"),
            DeltaError::ItemIdFixtureReserved { entry },
            "update on fixture item {entry}"
        );

        let insert = artifact("example.pkg", &item_claim(entry, "insert", WHOLE_ITEM_ROW));
        assert_eq!(
            PackageDelta::parse(&insert).expect_err("the insert must be refused"),
            DeltaError::ItemIdFixtureReserved { entry },
            "insert on fixture item {entry}"
        );
    }
}

#[test]
fn the_bands_next_to_the_fixture_cluster_stay_claimable() {
    for entry in [5_089_999, 5_100_000] {
        let json = artifact("example.pkg", &item_claim(entry, "update", A_BUY_PRICE));

        PackageDelta::parse(&json)
            .unwrap_or_else(|e| panic!("item {entry} must stay updatable: {e}"));
    }
}

#[test]
fn entry_zero_is_not_a_row() {
    let json = artifact("example.pkg", &item_claim(0, "update", A_BUY_PRICE));

    assert!(matches!(
        PackageDelta::parse(&json).expect_err("the claim must be refused"),
        DeltaError::MalformedKey { .. }
    ));
}

#[test]
fn the_package_range_is_disjoint_from_every_reserved_identifier() {
    for entry in [5_090_000, 5_090_050, 5_099_999] {
        assert!(
            !is_package_item_id(entry),
            "reserved item {entry} must not be insertable by a Package"
        );
    }

    for entry in [PACKAGE_ITEM_ID_FLOOR, PACKAGE_ITEM_ID_CEIL] {
        assert!(
            !is_fixture_reserved_item_id(entry),
            "Package item {entry} must not be fixture-reserved"
        );
    }
}

#[test]
fn an_incomplete_item_insert_is_refused() {
    let json = artifact(
        "example.pkg",
        &item_claim(PACKAGE_ITEM_ID_FLOOR, "insert", A_BUY_PRICE),
    );

    match PackageDelta::parse(&json).expect_err("the insert must be refused") {
        DeltaError::IncompleteInsert { table, missing } => {
            assert_eq!(table, lyracore_package_delta::Table::Item);
            // Every claimable column except the one the claim carried.
            assert_eq!(missing.len(), 56);
            assert!(missing.contains(&"name".to_owned()));
            assert!(!missing.contains(&"buy_price".to_owned()));
        }
        other => panic!("expected an incomplete insert, got {other}"),
    }
}

#[test]
fn an_unknown_item_column_is_refused() {
    let json = artifact(
        "example.pkg",
        &item_claim(25, "update", r#"{"weight":{"type":"u32","value":1}}"#),
    );

    assert_eq!(
        PackageDelta::parse(&json).expect_err("the claim must be refused"),
        DeltaError::UnknownField {
            table: lyracore_package_delta::Table::Item,
            field: "weight".to_owned()
        }
    );
}
