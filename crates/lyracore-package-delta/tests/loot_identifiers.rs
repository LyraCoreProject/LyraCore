mod common;

use common::{artifact, pickpocket_loot_claim, WHOLE_PICKPOCKET_LOOT_ROW};
use lyracore_package_delta::ids::{is_fixture_reserved_loot_id, PackageRange};
use lyracore_package_delta::{
    is_package_loot_id, DeltaError, FieldValue, PackageDelta, PrimaryKey, Table,
};

const A_CHANCE: &str = r#"{"chance_bp":{"type":"u32","value":2500}}"#;

#[test]
fn a_pickpocket_loot_key_is_the_id_itself() {
    let key = PrimaryKey::pickpocket_loot(9_000_001).expect("a Package loot row is claimable");

    assert_eq!(key.row_id(), 9_000_001);
}

#[test]
fn a_package_may_insert_a_loot_row_inside_the_package_range() {
    for id in [9_000_000u64, 9_500_000, 9_999_999u64] {
        let json = artifact(
            "example.pkg",
            &pickpocket_loot_claim(id, "insert", WHOLE_PICKPOCKET_LOOT_ROW),
        );

        PackageDelta::parse(&json).unwrap_or_else(|e| panic!("loot row {id} must insert: {e}"));
    }
}

#[test]
fn an_insert_outside_the_package_range_is_refused() {
    for id in [1, 8_999_999u64, 10_000_000u64] {
        let json = artifact(
            "example.pkg",
            &pickpocket_loot_claim(id, "insert", WHOLE_PICKPOCKET_LOOT_ROW),
        );

        assert_eq!(
            PackageDelta::parse(&json).expect_err("the insert must be refused"),
            DeltaError::IdOutsidePackageRange {
                range: PackageRange::Loot,
                id
            },
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
            DeltaError::IdFixtureReserved {
                range: PackageRange::Loot,
                id
            },
            "update on fixture loot row {id}"
        );

        let insert = artifact(
            "example.pkg",
            &pickpocket_loot_claim(id, "insert", WHOLE_PICKPOCKET_LOOT_ROW),
        );
        assert_eq!(
            PackageDelta::parse(&insert).expect_err("the insert must be refused"),
            DeltaError::IdFixtureReserved {
                range: PackageRange::Loot,
                id
            },
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

    for id in [9_000_000u64, 9_999_999u64] {
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
        &pickpocket_loot_claim(9_000_000u64, "insert", A_CHANCE),
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

#[test]
fn a_loot_insert_may_claim_a_stack_minimum() {
    let ranged = r#"{
        "creature_entry": { "type": "u32", "value": 6 },
        "item_entry": { "type": "u32", "value": 25 },
        "chance_bp": { "type": "u32", "value": 5000 },
        "min_count": { "type": "u32", "value": 1 },
        "count": { "type": "u32", "value": 3 },
        "group_id": { "type": "u32", "value": 0 },
        "quest_only": { "type": "bool", "value": false }
    }"#;
    let json = artifact(
        "example.pkg",
        &pickpocket_loot_claim(9_000_000, "insert", ranged),
    );

    let delta = PackageDelta::parse(&json).expect("a loot row may claim min_count");

    assert_eq!(delta.claims()[0].fields()["min_count"], FieldValue::U32(1));
}

#[test]
fn every_loot_table_takes_an_insert_without_a_stack_minimum() {
    for (table, owner, quest_only) in [
        ("game_pickpocket_loot", "creature_entry", true),
        ("game_gameobject_loot", "loot_id", true),
        ("game_skinning_loot", "skin_loot_id", false),
        ("game_fishing_loot", "zone_id", false),
    ] {
        let quest_only = if quest_only {
            r#","quest_only":{"type":"bool","value":false}"#
        } else {
            ""
        };
        let json = artifact(
            "example.pkg",
            &format!(
                r#"{{"table":"{table}","key":{{"id":9000000}},"operation":"insert","fields":{{"{owner}":{{"type":"u32","value":6}},"item_entry":{{"type":"u32","value":25}},"chance_bp":{{"type":"u32","value":5000}},"count":{{"type":"u32","value":1}},"group_id":{{"type":"u32","value":0}}{quest_only}}}}}"#
            ),
        );

        let delta = PackageDelta::parse(&json)
            .unwrap_or_else(|e| panic!("`{table}` must insert without min_count: {e}"));

        assert!(
            !delta.claims()[0].fields().contains_key("min_count"),
            "{table}"
        );
    }
}
