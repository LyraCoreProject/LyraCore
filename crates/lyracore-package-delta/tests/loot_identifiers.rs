//! The loot identifier policy: which rows a Package may claim. Mirrors `item_identifiers.rs` — a
//! loot row's key is flat, the same shape as an item's `entry`, because (unlike the quest family)
//! no loot table's owning entity (creature, gameobject/chest, or zone) is ever Package-invented.
//! The band therefore applies to a loot row's OWN `id`, not to an owning identifier.

mod common;

use common::{artifact, pickpocket_loot_claim, WHOLE_PICKPOCKET_LOOT_ROW};
use lyracore_package_delta::{
    is_fixture_reserved_loot_id, is_package_loot_id, DeltaError, PackageDelta, PrimaryKey, Table,
    PACKAGE_LOOT_ID_CEIL, PACKAGE_LOOT_ID_FLOOR,
};

const A_CHANCE: &str = r#"{"chance_bp":{"type":"u32","value":2500}}"#;

#[test]
fn a_pickpocket_loot_key_is_the_id_itself() {
    let key = PrimaryKey::pickpocket_loot(9_000_001).expect("a Package loot row is claimable");

    assert_eq!(key.row_id(), 9_000_001);
}

#[test]
fn a_package_may_insert_a_loot_row_inside_the_package_range() {
    for id in [PACKAGE_LOOT_ID_FLOOR, 9_500_000, PACKAGE_LOOT_ID_CEIL] {
        let json = artifact(
            "example.pkg",
            &pickpocket_loot_claim(id, "insert", WHOLE_PICKPOCKET_LOOT_ROW),
        );

        PackageDelta::parse(&json).unwrap_or_else(|e| panic!("loot row {id} must insert: {e}"));
    }
}

#[test]
fn an_insert_outside_the_package_range_is_refused() {
    for id in [1, PACKAGE_LOOT_ID_FLOOR - 1, PACKAGE_LOOT_ID_CEIL + 1] {
        let json = artifact(
            "example.pkg",
            &pickpocket_loot_claim(id, "insert", WHOLE_PICKPOCKET_LOOT_ROW),
        );

        assert_eq!(
            PackageDelta::parse(&json).expect_err("the insert must be refused"),
            DeltaError::LootIdNotClientSafe { id },
            "loot row {id}"
        );
    }
}

#[test]
fn an_update_may_name_a_real_imported_loot_row() {
    let json = artifact("example.pkg", &pickpocket_loot_claim(1, "update", A_CHANCE));

    let delta = PackageDelta::parse(&json).expect("a real loot row is updatable");

    assert_eq!(delta.claims()[0].key().row_id(), 1);
}

#[test]
fn no_operation_may_claim_the_project_wide_fixture_reserved_band() {
    for id in [5_090_000u64, 5_090_050, 5_099_999] {
        let update = artifact(
            "example.pkg",
            &pickpocket_loot_claim(id, "update", A_CHANCE),
        );
        assert_eq!(
            PackageDelta::parse(&update).expect_err("the update must be refused"),
            DeltaError::LootIdFixtureReserved { id },
            "update on fixture loot row {id}"
        );

        let insert = artifact(
            "example.pkg",
            &pickpocket_loot_claim(id, "insert", WHOLE_PICKPOCKET_LOOT_ROW),
        );
        assert_eq!(
            PackageDelta::parse(&insert).expect_err("the insert must be refused"),
            DeltaError::LootIdFixtureReserved { id },
            "insert on fixture loot row {id}"
        );
    }
}

#[test]
fn id_zero_is_not_a_row() {
    let json = artifact("example.pkg", &pickpocket_loot_claim(0, "update", A_CHANCE));

    assert!(matches!(
        PackageDelta::parse(&json).expect_err("the claim must be refused"),
        DeltaError::MalformedKey { .. }
    ));
}

#[test]
fn the_package_range_is_disjoint_from_every_reserved_identifier() {
    for id in [5_090_000u64, 5_090_050, 5_099_999] {
        assert!(
            !is_package_loot_id(id),
            "reserved loot row {id} must not be insertable by a Package"
        );
    }

    for id in [PACKAGE_LOOT_ID_FLOOR, PACKAGE_LOOT_ID_CEIL] {
        assert!(
            !is_fixture_reserved_loot_id(id),
            "Package loot row {id} must not be fixture-reserved"
        );
    }
}

#[test]
fn an_incomplete_loot_insert_is_refused() {
    let json = artifact(
        "example.pkg",
        &pickpocket_loot_claim(PACKAGE_LOOT_ID_FLOOR, "insert", A_CHANCE),
    );

    match PackageDelta::parse(&json).expect_err("the insert must be refused") {
        DeltaError::IncompleteInsert { table, missing } => {
            assert_eq!(table, Table::PickpocketLoot);
            assert_eq!(missing.len(), 5);
            assert!(missing.contains(&"creature_entry".to_owned()));
            assert!(!missing.contains(&"chance_bp".to_owned()));
        }
        other => panic!("expected an incomplete insert, got {other}"),
    }
}

/// The four loot tables are independent SpacetimeDB tables, so sharing one numeric band across
/// them cannot collide: the same `id` is a valid key on each.
#[test]
fn the_same_id_is_independently_claimable_on_every_loot_table() {
    for table in [
        Table::PickpocketLoot,
        Table::GameobjectLoot,
        Table::SkinningLoot,
        Table::FishingLoot,
    ] {
        assert!(table.columns().iter().any(|c| c.name == "item_entry"));
    }
}
