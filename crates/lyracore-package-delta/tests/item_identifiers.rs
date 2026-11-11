mod common;

use common::{artifact, item_claim, WHOLE_ITEM_ROW};
use lyracore_package_delta::ids::{is_fixture_reserved_item_id, PackageRange};
use lyracore_package_delta::{is_package_item_id, DeltaError, PackageDelta, PrimaryKey};

const A_BUY_PRICE: &str = r#"{"buy_price":{"type":"u32","value":100}}"#;

#[test]
fn an_item_header_key_is_the_entry_itself() {
    let key = PrimaryKey::item(7_000_001).expect("a Package item is claimable");

    assert_eq!(key.row_id(), 7_000_001);
}

#[test]
fn a_package_may_insert_an_item_inside_the_package_range() {
    for entry in [7_000_000u32, 7_500_000, 7_999_999u32] {
        let json = artifact("example.pkg", &item_claim(entry, "insert", WHOLE_ITEM_ROW));

        PackageDelta::parse(&json).unwrap_or_else(|e| panic!("item {entry} must insert: {e}"));
    }
}

#[test]
fn an_insert_outside_the_package_range_is_refused() {
    for entry in [25, 6_999_999u32, 8_000_000u32] {
        let json = artifact("example.pkg", &item_claim(entry, "insert", WHOLE_ITEM_ROW));

        assert_eq!(
            PackageDelta::parse(&json).expect_err("the insert must be refused"),
            DeltaError::IdOutsidePackageRange {
                range: PackageRange::Item,
                id: u64::from(entry)
            },
            "item {entry}"
        );
    }
}

#[test]
fn an_update_may_name_a_real_client_item() {
    let json = artifact("example.pkg", &item_claim(25, "update", A_BUY_PRICE));

    let delta = PackageDelta::parse(&json).expect("a real item is updatable");

    assert_eq!(delta.claims()[0].key().row_id(), 25);
}

#[test]
fn no_operation_may_claim_the_project_wide_fixture_reserved_band() {
    for entry in [5_090_000, 5_090_050, 5_099_999] {
        let update = artifact("example.pkg", &item_claim(entry, "update", A_BUY_PRICE));
        assert_eq!(
            PackageDelta::parse(&update).expect_err("the update must be refused"),
            DeltaError::IdFixtureReserved {
                range: PackageRange::Item,
                id: u64::from(entry)
            },
            "update on fixture item {entry}"
        );

        let insert = artifact("example.pkg", &item_claim(entry, "insert", WHOLE_ITEM_ROW));
        assert_eq!(
            PackageDelta::parse(&insert).expect_err("the insert must be refused"),
            DeltaError::IdFixtureReserved {
                range: PackageRange::Item,
                id: u64::from(entry)
            },
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

    for entry in [7_000_000u32, 7_999_999u32] {
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
        &item_claim(7_000_000u32, "insert", A_BUY_PRICE),
    );

    match PackageDelta::parse(&json).expect_err("the insert must be refused") {
        DeltaError::IncompleteInsert { table, missing } => {
            assert_eq!(table, lyracore_package_delta::Table::Item);
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
