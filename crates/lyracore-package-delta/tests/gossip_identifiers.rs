//! The gossip family's identifier policy: `game_npc_text`, `game_npc_text_slot`,
//! `game_gossip_option`, `game_gossip_menu_profile` and `game_gossip_menu_profile_option` follow
//! the loot shape (one band, checked against each table's own primary key — `game_npc_text_slot`
//! is `u64`, the other four are `u32`). `game_gossip_menu` follows no band at all — it is
//! update-only, refused unconditionally on `insert`, the same shape `game_creature_cast` uses.

mod common;

use common::{
    gossip_menu_claim, gossip_menu_profile_claim, gossip_menu_profile_option_claim,
    gossip_option_claim, npc_text_claim, npc_text_slot_claim, PACKAGE_GOSSIP_MENU_PROFILE_OPTION,
    PACKAGE_GOSSIP_OPTION, REAL_GOSSIP_MENU, REAL_GOSSIP_MENU_PROFILE,
    REAL_GOSSIP_MENU_PROFILE_OPTION, REAL_GOSSIP_OPTION, REAL_NPC_TEXT, REAL_NPC_TEXT_SLOT,
    WHOLE_GOSSIP_MENU_PROFILE_OPTION_ROW, WHOLE_GOSSIP_MENU_PROFILE_ROW, WHOLE_GOSSIP_OPTION_ROW,
    WHOLE_NPC_TEXT_ROW, WHOLE_NPC_TEXT_SLOT_ROW,
};
use lyracore_package_delta::{
    is_fixture_reserved_gossip_id, is_package_gossip_id, DeltaError, PackageDelta, PrimaryKey,
    Table, PACKAGE_GOSSIP_ID_CEIL, PACKAGE_GOSSIP_ID_FLOOR,
};

fn artifact(package: &str, claims: &str) -> String {
    format!(
        r#"{{"version":1,"package":"{package}","source_hash":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","claims":[{claims}]}}"#
    )
}

// ---- the five tables sharing the loot-shaped Package Gossip Range ----

/// One inventable table's identity for the parametrized band tests below. `update_field` is a
/// column every table below actually claims — the five tables do not share one column name, so a
/// single constant would misfire on most of them.
struct Inventable {
    table: Table,
    real_id: u64,
    whole_row: &'static str,
    update_field: &'static str,
    claim: fn(u64, &str, &str) -> String,
}

fn inventables() -> Vec<Inventable> {
    vec![
        Inventable {
            table: Table::NpcText,
            real_id: u64::from(REAL_NPC_TEXT),
            whole_row: WHOLE_NPC_TEXT_ROW,
            update_field: r#"{"text":{"type":"string","value":"New words."}}"#,
            claim: |id, op, fields| npc_text_claim(id as u32, op, fields),
        },
        Inventable {
            table: Table::NpcTextSlot,
            real_id: REAL_NPC_TEXT_SLOT,
            whole_row: WHOLE_NPC_TEXT_SLOT_ROW,
            update_field: r#"{"text_id":{"type":"u32","value":1}}"#,
            claim: npc_text_slot_claim,
        },
        Inventable {
            table: Table::GossipOption,
            real_id: u64::from(REAL_GOSSIP_OPTION),
            whole_row: WHOLE_GOSSIP_OPTION_ROW,
            update_field: r#"{"option_index":{"type":"u32","value":1}}"#,
            claim: |id, op, fields| gossip_option_claim(id as u32, op, fields),
        },
        Inventable {
            table: Table::GossipMenuProfile,
            real_id: u64::from(REAL_GOSSIP_MENU_PROFILE),
            whole_row: WHOLE_GOSSIP_MENU_PROFILE_ROW,
            update_field: r#"{"text_id":{"type":"u32","value":1}}"#,
            claim: |id, op, fields| gossip_menu_profile_claim(id as u32, op, fields),
        },
        Inventable {
            table: Table::GossipMenuProfileOption,
            real_id: u64::from(REAL_GOSSIP_MENU_PROFILE_OPTION),
            whole_row: WHOLE_GOSSIP_MENU_PROFILE_OPTION_ROW,
            update_field: r#"{"option_index":{"type":"u32","value":1}}"#,
            claim: |id, op, fields| gossip_menu_profile_option_claim(id as u32, op, fields),
        },
    ]
}

#[test]
fn a_package_may_insert_into_any_inventable_gossip_table_inside_the_package_range() {
    for row in inventables() {
        for id in [
            u64::from(PACKAGE_GOSSIP_ID_FLOOR),
            12_500_000,
            u64::from(PACKAGE_GOSSIP_ID_CEIL),
        ] {
            let json = artifact("example.pkg", &(row.claim)(id, "insert", row.whole_row));

            PackageDelta::parse(&json)
                .unwrap_or_else(|e| panic!("{} row {id} must insert: {e}", row.table));
        }
    }
}

#[test]
fn an_insert_outside_the_package_gossip_range_is_refused() {
    for row in inventables() {
        for id in [
            1,
            u64::from(PACKAGE_GOSSIP_ID_FLOOR) - 1,
            u64::from(PACKAGE_GOSSIP_ID_CEIL) + 1,
        ] {
            let json = artifact("example.pkg", &(row.claim)(id, "insert", row.whole_row));

            assert_eq!(
                PackageDelta::parse(&json).expect_err("the insert must be refused"),
                DeltaError::GossipIdNotClientSafe { id },
                "{} row {id}",
                row.table
            );
        }
    }
}

#[test]
fn an_update_may_name_a_real_imported_gossip_row() {
    for row in inventables() {
        let json = artifact(
            "example.pkg",
            &(row.claim)(row.real_id, "update", row.update_field),
        );

        let delta = PackageDelta::parse(&json).unwrap_or_else(|e| panic!("{}: {e}", row.table));
        assert_eq!(delta.claims()[0].key().row_id(), row.real_id);
    }
}

#[test]
fn no_operation_may_claim_the_project_wide_fixture_reserved_band_for_any_gossip_table() {
    for row in inventables() {
        for id in [5_090_000u64, 5_090_050, 5_099_999] {
            let update = artifact("example.pkg", &(row.claim)(id, "update", row.update_field));
            assert_eq!(
                PackageDelta::parse(&update).expect_err("the update must be refused"),
                DeltaError::GossipIdFixtureReserved { id },
                "update on fixture {} row {id}",
                row.table
            );

            let insert = artifact("example.pkg", &(row.claim)(id, "insert", row.whole_row));
            assert_eq!(
                PackageDelta::parse(&insert).expect_err("the insert must be refused"),
                DeltaError::GossipIdFixtureReserved { id },
                "insert on fixture {} row {id}",
                row.table
            );
        }
    }
}

#[test]
fn gossip_row_id_zero_is_not_a_row() {
    for row in inventables() {
        let json = artifact("example.pkg", &(row.claim)(0, "update", row.update_field));

        assert!(
            matches!(
                PackageDelta::parse(&json).expect_err("the claim must be refused"),
                DeltaError::MalformedKey { .. }
            ),
            "{}",
            row.table
        );
    }
}

#[test]
fn the_gossip_package_range_is_disjoint_from_every_reserved_identifier() {
    for id in [5_090_000u64, 5_090_050, 5_099_999] {
        assert!(
            !is_package_gossip_id(id),
            "reserved gossip row {id} must not be insertable by a Package"
        );
    }

    for id in [
        u64::from(PACKAGE_GOSSIP_ID_FLOOR),
        u64::from(PACKAGE_GOSSIP_ID_CEIL),
    ] {
        assert!(
            !is_fixture_reserved_gossip_id(id),
            "Package gossip row {id} must not be fixture-reserved"
        );
    }
}

#[test]
fn an_incomplete_gossip_option_insert_is_refused() {
    let json = artifact(
        "example.pkg",
        &gossip_option_claim(
            PACKAGE_GOSSIP_OPTION,
            "insert",
            r#"{"entry":{"type":"u32","value":6}}"#,
        ),
    );

    match PackageDelta::parse(&json).expect_err("the insert must be refused") {
        DeltaError::IncompleteInsert { table, missing } => {
            assert_eq!(table, Table::GossipOption);
            assert_eq!(missing.len(), 8);
            assert!(missing.contains(&"option_index".to_owned()));
            assert!(!missing.contains(&"entry".to_owned()));
        }
        other => panic!("expected an incomplete insert, got {other}"),
    }
}

#[test]
fn a_gossip_menu_profile_option_key_is_the_row_id_itself() {
    let key = PrimaryKey::gossip_menu_profile_option(PACKAGE_GOSSIP_MENU_PROFILE_OPTION)
        .expect("a Package gossip row is claimable");

    assert_eq!(key.row_id(), u64::from(PACKAGE_GOSSIP_MENU_PROFILE_OPTION));
}

// ---- game_gossip_menu: update-only, no band at all ----

#[test]
fn a_gossip_menu_key_is_the_creature_entry_itself() {
    let key = PrimaryKey::gossip_menu(REAL_GOSSIP_MENU).expect("a real creature is claimable");

    assert_eq!(key.row_id(), u64::from(REAL_GOSSIP_MENU));
}

#[test]
fn every_insert_on_game_gossip_menu_is_refused_by_name_whatever_the_identifier() {
    let a_text_id = r#"{"text_id":{"type":"u32","value":1}}"#;
    for entry in [1u32, REAL_GOSSIP_MENU, PACKAGE_GOSSIP_ID_FLOOR] {
        let json = artifact(
            "example.pkg",
            &gossip_menu_claim(entry, "insert", a_text_id),
        );

        assert_eq!(
            PackageDelta::parse(&json).expect_err("every insert is refused"),
            DeltaError::InsertNotSupported {
                table: Table::GossipMenu
            },
            "entry {entry}"
        );
    }
}

#[test]
fn an_update_on_a_real_gossip_menu_entry_is_accepted() {
    let json = artifact(
        "example.pkg",
        &gossip_menu_claim(
            REAL_GOSSIP_MENU,
            "update",
            r#"{"text_id":{"type":"u32","value":1}}"#,
        ),
    );

    let delta = PackageDelta::parse(&json).expect("a real creature's gossip menu is updatable");

    assert_eq!(
        delta.claims()[0].key().row_id(),
        u64::from(REAL_GOSSIP_MENU)
    );
}

#[test]
fn gossip_menu_entry_zero_is_not_a_creature() {
    let json = artifact(
        "example.pkg",
        &gossip_menu_claim(0, "update", r#"{"text_id":{"type":"u32","value":1}}"#),
    );

    assert!(matches!(
        PackageDelta::parse(&json).expect_err("the claim must be refused"),
        DeltaError::MalformedKey {
            table: Table::GossipMenu,
            ..
        }
    ));
}
