//! The quest identifier policy: which rows a Package may claim. Mirrors `item_identifiers.rs`, and
//! the packed child keys mirror `spell_identifiers.rs`'s `game_spell_effect` cases.
//!
//! One band covers the whole family: `game_quest_text` and every child table below name their
//! owning quest by `quest_entry`, so a child row is only ever as Package-owned as the quest it
//! belongs to — there is no separate child band to test.

mod common;

use common::{
    artifact, quest_cast_objective_claim, quest_claim, quest_objective_claim,
    quest_reward_choice_claim, quest_text_claim, WHOLE_QUEST_ROW,
};
use lyracore_package_delta::{
    is_fixture_reserved_quest_id, is_package_quest_id, DeltaError, PackageDelta, PrimaryKey, Table,
    PACKAGE_QUEST_ID_CEIL, PACKAGE_QUEST_ID_FLOOR,
};

const A_TITLE: &str = r#"{"title":{"type":"string","value":"A Kindled Errand, Retitled"}}"#;

#[test]
fn a_quest_header_key_is_the_entry_itself() {
    let key = PrimaryKey::quest(8_000_001).expect("a Package quest is claimable");

    assert_eq!(key.row_id(), 8_000_001);
}

#[test]
fn a_package_may_insert_a_quest_inside_the_package_range() {
    for entry in [PACKAGE_QUEST_ID_FLOOR, 8_500_000, PACKAGE_QUEST_ID_CEIL] {
        let json = artifact(
            "example.pkg",
            &quest_claim(entry, "insert", WHOLE_QUEST_ROW),
        );

        PackageDelta::parse(&json).unwrap_or_else(|e| panic!("quest {entry} must insert: {e}"));
    }
}

#[test]
fn an_insert_outside_the_package_range_is_refused() {
    for entry in [1, PACKAGE_QUEST_ID_FLOOR - 1, PACKAGE_QUEST_ID_CEIL + 1] {
        let json = artifact(
            "example.pkg",
            &quest_claim(entry, "insert", WHOLE_QUEST_ROW),
        );

        assert_eq!(
            PackageDelta::parse(&json).expect_err("the insert must be refused"),
            DeltaError::QuestIdNotClientSafe { entry },
            "quest {entry}"
        );
    }
}

#[test]
fn an_update_may_name_a_real_client_quest() {
    let json = artifact("example.pkg", &quest_claim(1, "update", A_TITLE));

    let delta = PackageDelta::parse(&json).expect("a real quest is updatable");

    assert_eq!(delta.claims()[0].key().row_id(), 1);
}

#[test]
fn no_operation_may_claim_the_project_wide_fixture_reserved_band() {
    for entry in [5_090_000, 5_090_050, 5_099_999] {
        let update = artifact("example.pkg", &quest_claim(entry, "update", A_TITLE));
        assert_eq!(
            PackageDelta::parse(&update).expect_err("the update must be refused"),
            DeltaError::QuestIdFixtureReserved { entry },
            "update on fixture quest {entry}"
        );

        let insert = artifact(
            "example.pkg",
            &quest_claim(entry, "insert", WHOLE_QUEST_ROW),
        );
        assert_eq!(
            PackageDelta::parse(&insert).expect_err("the insert must be refused"),
            DeltaError::QuestIdFixtureReserved { entry },
            "insert on fixture quest {entry}"
        );
    }
}

#[test]
fn entry_zero_is_not_a_row() {
    let json = artifact("example.pkg", &quest_claim(0, "update", A_TITLE));

    assert!(matches!(
        PackageDelta::parse(&json).expect_err("the claim must be refused"),
        DeltaError::MalformedKey { .. }
    ));
}

#[test]
fn the_package_range_is_disjoint_from_every_reserved_identifier() {
    for entry in [5_090_000, 5_090_050, 5_099_999] {
        assert!(
            !is_package_quest_id(entry),
            "reserved quest {entry} must not be insertable by a Package"
        );
    }

    for entry in [PACKAGE_QUEST_ID_FLOOR, PACKAGE_QUEST_ID_CEIL] {
        assert!(
            !is_fixture_reserved_quest_id(entry),
            "Package quest {entry} must not be fixture-reserved"
        );
    }
}

#[test]
fn an_incomplete_quest_insert_is_refused() {
    let json = artifact(
        "example.pkg",
        &quest_claim(PACKAGE_QUEST_ID_FLOOR, "insert", A_TITLE),
    );

    match PackageDelta::parse(&json).expect_err("the insert must be refused") {
        DeltaError::IncompleteInsert { table, missing } => {
            assert_eq!(table, Table::Quest);
            assert_eq!(missing.len(), 18);
            assert!(missing.contains(&"reward_money".to_owned()));
            assert!(!missing.contains(&"title".to_owned()));
        }
        other => panic!("expected an incomplete insert, got {other}"),
    }
}

// ---- quest text: 1:1 with the header by `quest_entry`, same band ----

#[test]
fn quest_text_shares_the_quest_header_band() {
    let json = artifact(
        "example.pkg",
        &quest_text_claim(
            PACKAGE_QUEST_ID_FLOOR,
            "insert",
            r#"{"details":{"type":"string","value":"d"},"objectives":{"type":"string","value":"o"},"offer_reward_text":{"type":"string","value":"r"},"request_items_text":{"type":"string","value":"i"}}"#,
        ),
    );

    PackageDelta::parse(&json).expect("quest text inside the Package quest range inserts");
}

#[test]
fn quest_text_outside_the_package_range_may_not_be_inserted() {
    let json = artifact(
        "example.pkg",
        &quest_text_claim(
            1,
            "insert",
            r#"{"details":{"type":"string","value":"d"},"objectives":{"type":"string","value":"o"},"offer_reward_text":{"type":"string","value":"r"},"request_items_text":{"type":"string","value":"i"}}"#,
        ),
    );

    assert_eq!(
        PackageDelta::parse(&json).expect_err("the insert must be refused"),
        DeltaError::QuestIdNotClientSafe { entry: 1 }
    );
}

// ---- packed child keys ----

#[test]
fn an_objectives_packed_key_derives_from_the_quest_and_the_slot() {
    let key = PrimaryKey::quest_objective(PACKAGE_QUEST_ID_FLOOR, 2).expect("slot 2 is claimable");

    assert_eq!(key.row_id(), (u64::from(PACKAGE_QUEST_ID_FLOOR) << 8) | 2);
}

#[test]
fn a_cast_objective_and_an_objective_pack_the_same_number_on_different_tables() {
    let objective = PrimaryKey::quest_objective(PACKAGE_QUEST_ID_FLOOR, 1).unwrap();
    let cast = PrimaryKey::quest_cast_objective(PACKAGE_QUEST_ID_FLOOR, 1).unwrap();

    assert_eq!(objective.row_id(), cast.row_id());
    assert_ne!(objective.table(), cast.table());
}

#[test]
fn an_objective_index_past_the_quest_max_is_refused() {
    let json = artifact(
        "example.pkg",
        &quest_objective_claim(
            PACKAGE_QUEST_ID_FLOOR,
            4,
            "insert",
            r#"{"kind":{"type":"u8","value":0},"target_entry":{"type":"u32","value":6},"required_count":{"type":"u32","value":5}}"#,
        ),
    );

    assert_eq!(
        PackageDelta::parse(&json).expect_err("the claim must be refused"),
        DeltaError::QuestObjectiveIndexOutOfRange { obj_index: 4 }
    );
}

#[test]
fn a_cast_objective_shares_the_objective_index_bound() {
    let json = artifact(
        "example.pkg",
        &quest_cast_objective_claim(
            PACKAGE_QUEST_ID_FLOOR,
            4,
            "insert",
            r#"{"spell_id":{"type":"u32","value":133}}"#,
        ),
    );

    assert_eq!(
        PackageDelta::parse(&json).expect_err("the claim must be refused"),
        DeltaError::QuestObjectiveIndexOutOfRange { obj_index: 4 }
    );
}

#[test]
fn a_reward_item_packed_key_derives_from_the_quest_and_the_item() {
    let key = PrimaryKey::quest_reward_item(PACKAGE_QUEST_ID_FLOOR, 25).expect("row is claimable");

    assert_eq!(key.row_id(), (u64::from(PACKAGE_QUEST_ID_FLOOR) << 32) | 25);
}

#[test]
fn a_reward_choice_index_past_six_is_refused() {
    let json = artifact(
        "example.pkg",
        &quest_reward_choice_claim(
            PACKAGE_QUEST_ID_FLOOR,
            6,
            "insert",
            r#"{"item_entry":{"type":"u32","value":25},"count":{"type":"u32","value":1}}"#,
        ),
    );

    assert_eq!(
        PackageDelta::parse(&json).expect_err("the claim must be refused"),
        DeltaError::QuestRewardChoiceIndexOutOfRange { choice_index: 6 }
    );
}

/// The key columns (`quest_entry`, `obj_index`/`item_entry`/`choice_index`) may never also appear
/// in `fields` — the key already names them.
#[test]
fn a_child_table_refuses_its_own_key_column_as_a_field() {
    let json = artifact(
        "example.pkg",
        &quest_objective_claim(
            REAL_QUEST_FOR_UPDATE,
            0,
            "update",
            r#"{"quest_entry":{"type":"u32","value":1}}"#,
        ),
    );

    assert_eq!(
        PackageDelta::parse(&json).expect_err("the claim must be refused"),
        DeltaError::KeyColumnClaimed {
            table: Table::QuestObjective,
            field: "quest_entry".to_owned()
        }
    );
}

const REAL_QUEST_FOR_UPDATE: u32 = 1;
