//! The gameobjects family's identifier policy and its spatial key.
//!
//! One band covers three tables. The template and the trap share ONE identifier space, because a
//! trap row describes the template of the same entry; the spawn has its own. `game_gameobject` is
//! spatial: its key names the map that routes the claim beside the spawn identifier its durable
//! guid derives from.

mod common;

use common::{
    artifact, gameobject_spawn_claim, gameobject_template_claim, gameobject_trap_claim,
    FOREIGN_MAP, PACKAGE_GAMEOBJECT, REAL_GAMEOBJECT, REAL_GAMEOBJECT_SPAWN, REAL_MAP,
    WHOLE_GAMEOBJECT_SPAWN_ROW, WHOLE_GAMEOBJECT_TEMPLATE_ROW, WHOLE_GAMEOBJECT_TRAP_ROW,
};
use lyracore_package_delta::{
    is_fixture_reserved_gameobject_id, is_package_gameobject_id, packed_gameobject_spawn_guid,
    DeltaError, PackageDelta, PrimaryKey, Table, PACKAGE_GAMEOBJECT_ID_CEIL,
    PACKAGE_GAMEOBJECT_ID_FLOOR,
};

const A_NAME: &str = r#"{"name":{"type":"string","value":"Kindled Cache"}}"#;
const A_POSITION: &str = r#"{"x":{"type":"f32","value":1.5}}"#;

/// The guid is the importer's `go_guid`: the client's GameObject tag and the spawn identifier. No
/// template entry rides in it, unlike a creature's.
#[test]
fn a_spawn_key_derives_the_importers_gameobject_guid() {
    let key = PrimaryKey::gameobject_spawn(REAL_MAP, REAL_GAMEOBJECT_SPAWN)
        .expect("a real spawn is claimable");

    assert_eq!(
        key.row_id(),
        (0xF110u64 << 48) | u64::from(REAL_GAMEOBJECT_SPAWN)
    );
    assert_eq!(
        key.row_id(),
        packed_gameobject_spawn_guid(REAL_GAMEOBJECT_SPAWN)
    );
    assert_eq!(key.table(), Table::GameobjectSpawn);
}

#[test]
fn only_the_spawn_states_a_map() {
    assert_eq!(
        PrimaryKey::gameobject_spawn(FOREIGN_MAP, REAL_GAMEOBJECT_SPAWN)
            .expect("a spawn on any map parses")
            .map_id(),
        Some(FOREIGN_MAP)
    );
    for global in [
        PrimaryKey::gameobject_template(PACKAGE_GAMEOBJECT).expect("a Package template"),
        PrimaryKey::gameobject_trap(PACKAGE_GAMEOBJECT).expect("a Package trap"),
    ] {
        assert_eq!(global.map_id(), None, "{global}");
    }
}

#[test]
fn a_package_may_insert_a_gameobject_row_inside_the_package_range() {
    for id in [
        PACKAGE_GAMEOBJECT_ID_FLOOR,
        16_500_000,
        PACKAGE_GAMEOBJECT_ID_CEIL,
    ] {
        let json = artifact(
            "example.pkg",
            &gameobject_template_claim(id, "insert", WHOLE_GAMEOBJECT_TEMPLATE_ROW),
        );

        PackageDelta::parse(&json).unwrap_or_else(|e| panic!("gameobject {id} must insert: {e}"));
    }
}

#[test]
fn an_insert_outside_the_package_range_is_refused() {
    for id in [
        REAL_GAMEOBJECT,
        PACKAGE_GAMEOBJECT_ID_FLOOR - 1,
        PACKAGE_GAMEOBJECT_ID_CEIL + 1,
    ] {
        let json = artifact(
            "example.pkg",
            &gameobject_template_claim(id, "insert", WHOLE_GAMEOBJECT_TEMPLATE_ROW),
        );

        assert_eq!(
            PackageDelta::parse(&json).expect_err("the insert must be refused"),
            DeltaError::GameobjectIdNotClientSafe { id },
            "gameobject {id}"
        );
    }
}

/// Every insertable gameobject table shares one band, so one out-of-range identifier is refused
/// the same way whichever of them names it.
#[test]
fn one_band_covers_every_insertable_gameobject_table() {
    let out_of_band = PACKAGE_GAMEOBJECT_ID_FLOOR - 1;
    let claims = [
        gameobject_template_claim(out_of_band, "insert", WHOLE_GAMEOBJECT_TEMPLATE_ROW),
        gameobject_trap_claim(out_of_band, "insert", WHOLE_GAMEOBJECT_TRAP_ROW),
        gameobject_spawn_claim(REAL_MAP, out_of_band, "insert", WHOLE_GAMEOBJECT_SPAWN_ROW),
    ];

    for claim in claims {
        assert_eq!(
            PackageDelta::parse(&artifact("example.pkg", &claim))
                .expect_err("the insert must be refused"),
            DeltaError::GameobjectIdNotClientSafe { id: out_of_band },
            "{claim}"
        );
    }
}

#[test]
fn an_update_may_name_a_real_imported_gameobject() {
    PackageDelta::parse(&artifact(
        "example.pkg",
        &gameobject_template_claim(REAL_GAMEOBJECT, "update", A_NAME),
    ))
    .expect("tuning a real gameobject is the point");

    PackageDelta::parse(&artifact(
        "example.pkg",
        &gameobject_spawn_claim(REAL_MAP, REAL_GAMEOBJECT_SPAWN, "update", A_POSITION),
    ))
    .expect("moving a real prop is the point");
}

#[test]
fn a_fixture_reserved_gameobject_is_refused_under_every_operation() {
    for operation in ["insert", "update"] {
        let fields = if operation == "insert" {
            WHOLE_GAMEOBJECT_TEMPLATE_ROW
        } else {
            A_NAME
        };

        assert_eq!(
            PackageDelta::parse(&artifact(
                "example.pkg",
                &gameobject_template_claim(5_090_001, operation, fields),
            ))
            .expect_err("the claim must be refused"),
            DeltaError::GameobjectIdFixtureReserved { id: 5_090_001 },
            "{operation}"
        );
    }
}

#[test]
fn the_package_gameobject_band_is_its_own() {
    assert!(is_package_gameobject_id(PACKAGE_GAMEOBJECT));
    assert!(!is_package_gameobject_id(PACKAGE_GAMEOBJECT_ID_CEIL + 1));
    assert!(!is_fixture_reserved_gameobject_id(PACKAGE_GAMEOBJECT));
}

/// The key carries the map and the spawn identifier, and the grid columns are derived from the
/// coordinates in the same write, so neither may be claimed.
#[test]
fn a_gameobject_spawn_may_not_claim_a_key_or_a_derived_column() {
    for column in ["map_id", "spawn_id", "guid"] {
        let fields = format!(r#"{{"{column}":{{"type":"u32","value":1}}}}"#);

        assert_eq!(
            PackageDelta::parse(&artifact(
                "example.pkg",
                &gameobject_spawn_claim(REAL_MAP, REAL_GAMEOBJECT_SPAWN, "update", &fields),
            ))
            .expect_err("the claim must be refused"),
            DeltaError::KeyColumnClaimed {
                table: Table::GameobjectSpawn,
                field: column.to_owned()
            }
        );
    }

    for column in ["grid_x", "grid_y", "cell", "created_at", "instance_id"] {
        let fields = format!(r#"{{"{column}":{{"type":"u32","value":1}}}}"#);

        assert_eq!(
            PackageDelta::parse(&artifact(
                "example.pkg",
                &gameobject_spawn_claim(REAL_MAP, REAL_GAMEOBJECT_SPAWN, "update", &fields),
            ))
            .expect_err("the claim must be refused"),
            DeltaError::UnknownField {
                table: Table::GameobjectSpawn,
                field: column.to_owned()
            }
        );
    }
}
