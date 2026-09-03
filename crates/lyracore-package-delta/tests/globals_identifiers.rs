//! The globals family's identifier policy. Three of its seven tables follow the loot shape (one
//! band, checked against the row's own surrogate key). The other four permit no insert at all:
//! their key names a race, a class, a level or an `AreaTrigger.dbc` trigger id.

mod common;

use common::{
    areatrigger_teleport_claim, class_level_stats_claim, createinfo_spell_claim,
    graveyard_zone_claim, start_position_claim, PACKAGE_GLOBALS, REAL_GRAVEYARD_ZONE,
    WHOLE_CREATEINFO_SPELL_ROW, WHOLE_GRAVEYARD_ZONE_ROW,
};
use lyracore_package_delta::{
    is_fixture_reserved_globals_id, is_package_globals_id, is_package_gossip_id, DeltaError,
    PackageDelta, PrimaryKey, Table, MAX_STATS_LEVEL, PACKAGE_GLOBALS_ID_CEIL,
    PACKAGE_GLOBALS_ID_FLOOR, PACKAGE_SPELLMETA_ID_FLOOR,
};

fn artifact(package: &str, claims: &str) -> String {
    format!(
        r#"{{"version":1,"package":"{package}","source_hash":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","claims":[{claims}]}}"#
    )
}

const A_ZONE: &str = r#"{"zone_id":{"type":"u32","value":40}}"#;
const A_BASE_HEALTH: &str = r#"{"base_health":{"type":"u32","value":120}}"#;
const A_MAP: &str = r#"{"map_id":{"type":"u32","value":0}}"#;
const A_TARGET_MAP: &str = r#"{"target_map":{"type":"u32","value":36}}"#;

#[test]
fn a_package_may_insert_a_globals_row_inside_the_package_range() {
    for row_id in [
        PACKAGE_GLOBALS_ID_FLOOR,
        13_500_000,
        PACKAGE_GLOBALS_ID_CEIL,
    ] {
        let json = artifact(
            "example.pkg",
            &graveyard_zone_claim(row_id, "insert", WHOLE_GRAVEYARD_ZONE_ROW),
        );

        PackageDelta::parse(&json)
            .unwrap_or_else(|e| panic!("graveyard zone {row_id} must insert: {e}"));
    }
}

#[test]
fn an_insert_outside_the_package_range_is_refused() {
    for id in [
        1u64,
        PACKAGE_GLOBALS_ID_FLOOR - 1,
        PACKAGE_GLOBALS_ID_CEIL + 1,
    ] {
        let json = artifact(
            "example.pkg",
            &graveyard_zone_claim(id, "insert", WHOLE_GRAVEYARD_ZONE_ROW),
        );

        assert_eq!(
            PackageDelta::parse(&json).expect_err("the insert must be refused"),
            DeltaError::GlobalsIdNotClientSafe { id },
            "graveyard zone {id}"
        );
    }
}

/// The three insertable globals tables share one band, so one out-of-range identifier is refused
/// the same way whichever of them names it.
#[test]
fn one_band_covers_every_insertable_globals_table() {
    let out_of_band = PACKAGE_GLOBALS_ID_FLOOR - 1;
    let claims = [
        graveyard_zone_claim(out_of_band, "insert", WHOLE_GRAVEYARD_ZONE_ROW),
        createinfo_spell_claim(out_of_band, "insert", WHOLE_CREATEINFO_SPELL_ROW),
    ];

    for claim in claims {
        assert_eq!(
            PackageDelta::parse(&artifact("example.pkg", &claim))
                .expect_err("the insert must be refused"),
            DeltaError::GlobalsIdNotClientSafe { id: out_of_band },
            "{claim}"
        );
    }
}

#[test]
fn an_update_may_name_a_real_imported_graveyard_zone_row() {
    let json = artifact(
        "example.pkg",
        &graveyard_zone_claim(REAL_GRAVEYARD_ZONE, "update", A_ZONE),
    );

    let delta = PackageDelta::parse(&json).expect("a real graveyard zone link is updatable");

    assert_eq!(delta.claims()[0].key().row_id(), REAL_GRAVEYARD_ZONE);
}

/// The four update-only tables: each key names something the client fixes, so no identifier makes
/// an insert legal.
#[test]
fn the_update_only_globals_tables_refuse_every_insert() {
    let cases = [
        (
            class_level_stats_claim(1, 10, "insert", A_BASE_HEALTH),
            Table::ClassLevelStats,
        ),
        (
            start_position_claim(1, 1, "insert", A_MAP),
            Table::StartPosition,
        ),
        (
            areatrigger_teleport_claim(1447, "insert", A_TARGET_MAP),
            Table::AreatriggerTeleport,
        ),
    ];

    for (claim, table) in cases {
        assert_eq!(
            PackageDelta::parse(&artifact("example.pkg", &claim))
                .expect_err("the insert must be refused"),
            DeltaError::InsertNotSupported { table },
            "{claim}"
        );
    }
}

#[test]
fn a_stat_curve_row_the_client_already_has_may_be_tuned() {
    let json = artifact(
        "example.pkg",
        &class_level_stats_claim(1, 10, "update", A_BASE_HEALTH),
    );

    let delta = PackageDelta::parse(&json).expect("a real stat curve row is updatable");

    assert_eq!(
        delta.claims()[0].key(),
        PrimaryKey::ClassLevelStats {
            class: 1,
            level: 10
        }
    );
}

/// The packed key the Module reads is derived here, never authored, so a claim that names the same
/// class and level names the same durable row.
#[test]
fn a_stat_curve_key_packs_the_class_and_level_the_module_packs() {
    let key = PrimaryKey::class_level_stats(2, 60).expect("a real class and level are claimable");

    assert_eq!(key.row_id(), (2 << 8) | 60);
}

#[test]
fn a_level_no_stat_curve_carries_is_refused() {
    for level in [0, MAX_STATS_LEVEL + 1] {
        let json = artifact(
            "example.pkg",
            &class_level_stats_claim(1, level, "update", A_BASE_HEALTH),
        );

        assert_eq!(
            PackageDelta::parse(&json).expect_err("the claim must be refused"),
            DeltaError::StatsLevelOutOfRange { level },
            "level {level}"
        );
    }
}

#[test]
fn no_operation_may_claim_the_project_wide_fixture_reserved_band() {
    for id in [5_090_000u64, 5_090_050, 5_099_999] {
        let update = artifact("example.pkg", &graveyard_zone_claim(id, "update", A_ZONE));
        assert_eq!(
            PackageDelta::parse(&update).expect_err("the update must be refused"),
            DeltaError::GlobalsIdFixtureReserved { id },
            "update on fixture graveyard zone {id}"
        );

        let insert = artifact(
            "example.pkg",
            &graveyard_zone_claim(id, "insert", WHOLE_GRAVEYARD_ZONE_ROW),
        );
        assert_eq!(
            PackageDelta::parse(&insert).expect_err("the insert must be refused"),
            DeltaError::GlobalsIdFixtureReserved { id },
            "insert on fixture graveyard zone {id}"
        );
    }
}

#[test]
fn row_id_zero_is_not_a_row() {
    let json = artifact("example.pkg", &graveyard_zone_claim(0, "update", A_ZONE));

    assert!(matches!(
        PackageDelta::parse(&json).expect_err("the claim must be refused"),
        DeltaError::MalformedKey { .. }
    ));
}

#[test]
fn the_package_range_is_disjoint_from_every_reserved_identifier() {
    for id in [5_090_000u64, 5_090_050, 5_099_999] {
        assert!(
            !is_package_globals_id(id),
            "reserved globals row {id} must not be insertable by a Package"
        );
    }

    for id in [PACKAGE_GLOBALS_ID_FLOOR, PACKAGE_GLOBALS_ID_CEIL] {
        assert!(
            !is_fixture_reserved_globals_id(id),
            "Package globals row {id} must not be fixture-reserved"
        );
    }
}

#[test]
fn the_globals_band_neighbours_do_not_overlap_it() {
    for id in [PACKAGE_GLOBALS_ID_FLOOR, PACKAGE_GLOBALS_ID_CEIL] {
        assert!(
            !is_package_gossip_id(id),
            "globals row {id} reads as a gossip row"
        );
    }
    assert!(!is_package_globals_id(PACKAGE_SPELLMETA_ID_FLOOR));
}

#[test]
fn an_incomplete_graveyard_zone_insert_is_refused() {
    let json = artifact(
        "example.pkg",
        &graveyard_zone_claim(PACKAGE_GLOBALS, "insert", A_ZONE),
    );

    match PackageDelta::parse(&json).expect_err("the insert must be refused") {
        DeltaError::IncompleteInsert { table, missing } => {
            assert_eq!(table, Table::GraveyardZone);
            assert!(missing.contains(&"safe_loc_id".to_owned()));
            assert!(!missing.contains(&"zone_id".to_owned()));
        }
        other => panic!("expected an incomplete insert, got {other}"),
    }
}
