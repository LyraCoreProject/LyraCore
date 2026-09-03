//! The gossip family's identifier policy. Five of its six tables follow the loot shape (one band,
//! checked against each row's own key). `game_gossip_menu` is the sixth: its key names a creature
//! template, so it permits no insert at all, the `game_creature_cast` shape.

mod common;

use common::{
    gossip_menu_claim, gossip_option_claim, npc_text_claim, npc_text_slot_claim, PACKAGE_GOSSIP,
    REAL_GOSSIP_MENU, REAL_NPC_TEXT, WHOLE_GOSSIP_OPTION_ROW, WHOLE_NPC_TEXT_ROW,
    WHOLE_NPC_TEXT_SLOT_ROW,
};
use lyracore_package_delta::{
    is_fixture_reserved_gossip_id, is_package_gossip_id, is_package_trainer_id, DeltaError,
    PackageDelta, PrimaryKey, Table, PACKAGE_GLOBALS_ID_FLOOR, PACKAGE_GOSSIP_ID_CEIL,
    PACKAGE_GOSSIP_ID_FLOOR,
};

fn artifact(package: &str, claims: &str) -> String {
    format!(
        r#"{{"version":1,"package":"{package}","source_hash":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","claims":[{claims}]}}"#
    )
}

const A_TEXT: &str = r#"{"text":{"type":"string","value":"Well met."}}"#;
const A_TEXT_ID: &str = r#"{"text_id":{"type":"u32","value":12000001}}"#;

#[test]
fn an_npc_text_key_is_the_text_id_itself() {
    let key = PrimaryKey::npc_text(12_000_001).expect("a Package greeting is claimable");

    assert_eq!(key.row_id(), 12_000_001);
    assert_eq!(key.table(), Table::NpcText);
}

#[test]
fn a_package_may_insert_a_gossip_row_inside_the_package_range() {
    for id in [PACKAGE_GOSSIP_ID_FLOOR, 12_500_000, PACKAGE_GOSSIP_ID_CEIL] {
        let json = artifact(
            "example.pkg",
            &npc_text_claim(id as u32, "insert", WHOLE_NPC_TEXT_ROW),
        );

        PackageDelta::parse(&json).unwrap_or_else(|e| panic!("npc text {id} must insert: {e}"));
    }
}

#[test]
fn an_insert_outside_the_package_range_is_refused() {
    for id in [
        1u64,
        PACKAGE_GOSSIP_ID_FLOOR - 1,
        PACKAGE_GOSSIP_ID_CEIL + 1,
    ] {
        let json = artifact(
            "example.pkg",
            &npc_text_claim(id as u32, "insert", WHOLE_NPC_TEXT_ROW),
        );

        assert_eq!(
            PackageDelta::parse(&json).expect_err("the insert must be refused"),
            DeltaError::GossipIdNotClientSafe { id },
            "npc text {id}"
        );
    }
}

/// Every insertable gossip table shares one band, so one out-of-range identifier is refused the
/// same way whichever of them names it.
#[test]
fn one_band_covers_every_insertable_gossip_table() {
    let out_of_band = PACKAGE_GOSSIP_ID_FLOOR - 1;
    let claims = [
        npc_text_claim(out_of_band as u32, "insert", WHOLE_NPC_TEXT_ROW),
        npc_text_slot_claim(out_of_band, "insert", WHOLE_NPC_TEXT_SLOT_ROW),
        gossip_option_claim(out_of_band as u32, "insert", WHOLE_GOSSIP_OPTION_ROW),
    ];

    for claim in claims {
        assert_eq!(
            PackageDelta::parse(&artifact("example.pkg", &claim))
                .expect_err("the insert must be refused"),
            DeltaError::GossipIdNotClientSafe { id: out_of_band },
            "{claim}"
        );
    }
}

#[test]
fn an_update_may_name_a_real_imported_npc_text_row() {
    let json = artifact(
        "example.pkg",
        &npc_text_claim(REAL_NPC_TEXT, "update", A_TEXT),
    );

    let delta = PackageDelta::parse(&json).expect("a real greeting is updatable");

    assert_eq!(delta.claims()[0].key().row_id(), u64::from(REAL_NPC_TEXT));
}

/// `game_gossip_menu`'s key names a creature template, which no Package may invent, so the table
/// permits no insert at any identifier — the `game_creature_cast` rule.
#[test]
fn no_gossip_menu_row_may_be_inserted() {
    for entry in [REAL_GOSSIP_MENU, PACKAGE_GOSSIP as u32] {
        let json = artifact(
            "example.pkg",
            &gossip_menu_claim(entry, "insert", A_TEXT_ID),
        );

        assert_eq!(
            PackageDelta::parse(&json).expect_err("the insert must be refused"),
            DeltaError::InsertNotSupported {
                table: Table::GossipMenu
            },
            "gossip menu {entry}"
        );
    }
}

#[test]
fn a_gossip_menu_row_a_creature_already_has_may_be_tuned() {
    let json = artifact(
        "example.pkg",
        &gossip_menu_claim(REAL_GOSSIP_MENU, "update", A_TEXT_ID),
    );

    let delta = PackageDelta::parse(&json).expect("a real creature's menu is updatable");

    assert_eq!(delta.claims()[0].key().table(), Table::GossipMenu);
}

#[test]
fn no_operation_may_claim_the_project_wide_fixture_reserved_band() {
    for id in [5_090_000u64, 5_090_050, 5_099_999] {
        let update = artifact("example.pkg", &npc_text_claim(id as u32, "update", A_TEXT));
        assert_eq!(
            PackageDelta::parse(&update).expect_err("the update must be refused"),
            DeltaError::GossipIdFixtureReserved { id },
            "update on fixture npc text {id}"
        );

        let insert = artifact(
            "example.pkg",
            &npc_text_claim(id as u32, "insert", WHOLE_NPC_TEXT_ROW),
        );
        assert_eq!(
            PackageDelta::parse(&insert).expect_err("the insert must be refused"),
            DeltaError::GossipIdFixtureReserved { id },
            "insert on fixture npc text {id}"
        );
    }
}

#[test]
fn text_id_zero_is_not_a_row() {
    let json = artifact("example.pkg", &npc_text_claim(0, "update", A_TEXT));

    assert!(matches!(
        PackageDelta::parse(&json).expect_err("the claim must be refused"),
        DeltaError::MalformedKey { .. }
    ));
}

#[test]
fn the_package_range_is_disjoint_from_every_reserved_identifier() {
    for id in [5_090_000u64, 5_090_050, 5_099_999] {
        assert!(
            !is_package_gossip_id(id),
            "reserved gossip row {id} must not be insertable by a Package"
        );
    }

    for id in [PACKAGE_GOSSIP_ID_FLOOR, PACKAGE_GOSSIP_ID_CEIL] {
        assert!(
            !is_fixture_reserved_gossip_id(id),
            "Package gossip row {id} must not be fixture-reserved"
        );
    }
}

/// The trainer, gossip and globals bands are one decade apart, so an identifier that reads as a
/// valid gossip row must read as nothing else.
#[test]
fn the_gossip_band_neighbours_do_not_overlap_it() {
    for id in [PACKAGE_GOSSIP_ID_FLOOR, PACKAGE_GOSSIP_ID_CEIL] {
        assert!(
            !is_package_trainer_id(id),
            "gossip row {id} reads as a trainer row"
        );
    }
    assert!(!is_package_gossip_id(PACKAGE_GLOBALS_ID_FLOOR));
}

#[test]
fn an_incomplete_gossip_option_insert_is_refused() {
    let json = artifact(
        "example.pkg",
        &gossip_option_claim(
            PACKAGE_GOSSIP as u32,
            "insert",
            r#"{"entry":{"type":"u32","value":6}}"#,
        ),
    );

    match PackageDelta::parse(&json).expect_err("the insert must be refused") {
        DeltaError::IncompleteInsert { table, missing } => {
            assert_eq!(table, Table::GossipOption);
            assert!(missing.contains(&"text".to_owned()));
            assert!(!missing.contains(&"entry".to_owned()));
        }
        other => panic!("expected an incomplete insert, got {other}"),
    }
}
