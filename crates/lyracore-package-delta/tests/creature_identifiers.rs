//! The creatures family's identifier policy and its spatial key.
//!
//! `game_creature_template` follows the item shape: one band, checked against the row's own entry.
//! `game_creature_spawn` is the build's first SPATIAL table: its key names the map that routes the
//! claim as well as the two components its durable guid is derived from.

mod common;

use common::{
    artifact, creature_spawn_claim, creature_template_claim, FOREIGN_MAP, PACKAGE_CREATURE,
    REAL_CREATURE, REAL_CREATURE_SPAWN, REAL_MAP, WHOLE_CREATURE_SPAWN_ROW,
    WHOLE_CREATURE_TEMPLATE_ROW,
};
use lyracore_package_delta::{
    is_fixture_reserved_creature_id, is_package_creature_id, packed_creature_spawn_guid,
    DeltaError, PackageDelta, PrimaryKey, Table, FIXTURE_CREATURE_ID_CEIL,
    FIXTURE_CREATURE_ID_FLOOR, MAX_CREATURE_GUID_COMPONENT, PACKAGE_CREATURE_ID_CEIL,
    PACKAGE_CREATURE_ID_FLOOR,
};

const A_LEVEL: &str = r#"{"level":{"type":"u32","value":12}}"#;
const A_POSITION: &str = r#"{"x":{"type":"f32","value":1.5}}"#;

#[test]
fn a_creature_template_key_is_the_entry_itself() {
    let key = PrimaryKey::creature_template(PACKAGE_CREATURE).expect("a Package creature");

    assert_eq!(key.row_id(), u64::from(PACKAGE_CREATURE));
    assert_eq!(key.table(), Table::CreatureTemplate);
}

/// The guid is the importer's `world_guid`: the client's Unit tag, the template entry, the spawn
/// identifier. Derived from the key so no artifact ever writes one.
#[test]
fn a_spawn_key_derives_the_importers_creature_guid() {
    let key = PrimaryKey::creature_spawn(REAL_MAP, REAL_CREATURE, REAL_CREATURE_SPAWN)
        .expect("a real spawn is claimable");

    assert_eq!(
        key.row_id(),
        (0xF130u64 << 48) | (u64::from(REAL_CREATURE) << 24) | u64::from(REAL_CREATURE_SPAWN)
    );
    assert_eq!(
        key.row_id(),
        packed_creature_spawn_guid(REAL_CREATURE, REAL_CREATURE_SPAWN)
    );
}

/// The map is what routes a spatial claim to a Shard, and it is the only reason it is in the key.
/// Two spawns of one template on two maps are two claims.
#[test]
fn a_spawn_claim_states_the_map_it_routes_by() {
    let key = PrimaryKey::creature_spawn(FOREIGN_MAP, REAL_CREATURE, REAL_CREATURE_SPAWN)
        .expect("a spawn on any map parses");

    assert_eq!(key.map_id(), Some(FOREIGN_MAP));
    assert_eq!(
        PrimaryKey::creature_template(PACKAGE_CREATURE)
            .expect("a Package creature")
            .map_id(),
        None,
        "a template is a global catalogue row"
    );
}

/// The map does not reach the durable guid, so the same spawn on two maps is one durable row. The
/// Module's preflight is what refuses that plan; here it is only stated.
#[test]
fn the_map_stays_out_of_the_derived_guid() {
    let here = PrimaryKey::creature_spawn(REAL_MAP, REAL_CREATURE, REAL_CREATURE_SPAWN)
        .expect("a spawn here");
    let there = PrimaryKey::creature_spawn(FOREIGN_MAP, REAL_CREATURE, REAL_CREATURE_SPAWN)
        .expect("a spawn there");

    assert_ne!(here, there);
    assert_eq!(here.row_id(), there.row_id());
}

#[test]
fn a_package_may_insert_a_creature_row_inside_the_package_range() {
    for id in [
        PACKAGE_CREATURE_ID_FLOOR,
        15_500_000,
        PACKAGE_CREATURE_ID_CEIL,
    ] {
        let json = artifact(
            "example.pkg",
            &creature_template_claim(id, "insert", WHOLE_CREATURE_TEMPLATE_ROW),
        );

        PackageDelta::parse(&json).unwrap_or_else(|e| panic!("creature {id} must insert: {e}"));
    }
}

#[test]
fn an_insert_outside_the_package_range_is_refused() {
    for id in [
        REAL_CREATURE,
        PACKAGE_CREATURE_ID_FLOOR - 1,
        PACKAGE_CREATURE_ID_CEIL + 1,
    ] {
        let json = artifact(
            "example.pkg",
            &creature_template_claim(id, "insert", WHOLE_CREATURE_TEMPLATE_ROW),
        );

        assert_eq!(
            PackageDelta::parse(&json).expect_err("the insert must be refused"),
            DeltaError::CreatureIdNotClientSafe { id },
            "creature {id}"
        );
    }
}

/// One band covers both insertable tables: the template's own entry and the spawn's own spawn
/// identifier. The template a spawn NAMES is the other side of the relation and takes no band.
#[test]
fn one_band_covers_the_template_and_the_spawn_identifier() {
    let out_of_band = PACKAGE_CREATURE_ID_FLOOR - 1;

    assert_eq!(
        PackageDelta::parse(&artifact(
            "example.pkg",
            &creature_spawn_claim(
                REAL_MAP,
                PACKAGE_CREATURE,
                out_of_band,
                "insert",
                WHOLE_CREATURE_SPAWN_ROW
            ),
        ))
        .expect_err("the insert must be refused"),
        DeltaError::CreatureIdNotClientSafe { id: out_of_band }
    );

    PackageDelta::parse(&artifact(
        "example.pkg",
        &creature_spawn_claim(
            REAL_MAP,
            REAL_CREATURE,
            PACKAGE_CREATURE,
            "insert",
            WHOLE_CREATURE_SPAWN_ROW,
        ),
    ))
    .expect("a Package spawn of a real creature is the ordinary case");
}

#[test]
fn an_update_may_name_a_real_imported_creature() {
    PackageDelta::parse(&artifact(
        "example.pkg",
        &creature_template_claim(REAL_CREATURE, "update", A_LEVEL),
    ))
    .expect("tuning a real creature is the point");

    PackageDelta::parse(&artifact(
        "example.pkg",
        &creature_spawn_claim(
            REAL_MAP,
            REAL_CREATURE,
            REAL_CREATURE_SPAWN,
            "update",
            A_POSITION,
        ),
    ))
    .expect("moving a real spawn is the point");
}

#[test]
fn a_fixture_reserved_creature_is_refused_under_every_operation() {
    for operation in ["insert", "update"] {
        let fields = if operation == "insert" {
            WHOLE_CREATURE_TEMPLATE_ROW
        } else {
            A_LEVEL
        };

        assert_eq!(
            PackageDelta::parse(&artifact(
                "example.pkg",
                &creature_template_claim(5_090_001, operation, fields),
            ))
            .expect_err("the claim must be refused"),
            DeltaError::CreatureIdFixtureReserved { id: 5_090_001 },
            "{operation}"
        );
    }
}

/// The Module seeds its creature fixtures at `51xxx` and asserts against them, so a Package tuning
/// one would turn those tests into a report about the Package.
#[test]
fn a_fixture_cluster_creature_template_is_refused_under_every_operation() {
    for id in [FIXTURE_CREATURE_ID_FLOOR, 51_001, FIXTURE_CREATURE_ID_CEIL] {
        assert!(is_fixture_reserved_creature_id(id), "creature {id}");

        assert_eq!(
            PackageDelta::parse(&artifact(
                "example.pkg",
                &creature_template_claim(id, "update", A_LEVEL),
            ))
            .expect_err("the claim must be refused"),
            DeltaError::CreatureIdFixtureReserved { id }
        );
    }
}

/// The fixture cluster names TEMPLATE entries. Real imported spawn identifiers run straight
/// through the same numbers, so a spawn takes the project-wide band alone.
#[test]
fn the_fixture_creature_cluster_does_not_reach_spawn_identifiers() {
    PackageDelta::parse(&artifact(
        "example.pkg",
        &creature_spawn_claim(
            REAL_MAP,
            REAL_CREATURE,
            FIXTURE_CREATURE_ID_FLOOR,
            "update",
            A_POSITION,
        ),
    ))
    .expect("a real spawn identifier inside the template cluster is still a real spawn");

    assert_eq!(
        PackageDelta::parse(&artifact(
            "example.pkg",
            &creature_spawn_claim(REAL_MAP, REAL_CREATURE, 5_090_001, "update", A_POSITION),
        ))
        .expect_err("the project-wide band still holds"),
        DeltaError::CreatureIdFixtureReserved { id: 5_090_001 }
    );
}

/// Both halves of the guid are 24 bits wide, so a wider component would wrap into another
/// creature's row instead of addressing its own.
#[test]
fn a_key_component_wider_than_the_guid_field_is_refused() {
    let too_wide = MAX_CREATURE_GUID_COMPONENT + 1;

    let refusal = PackageDelta::parse(&artifact(
        "example.pkg",
        &creature_spawn_claim(REAL_MAP, too_wide, PACKAGE_CREATURE, "update", A_POSITION),
    ))
    .expect_err("the claim must be refused");

    assert!(
        matches!(refusal, DeltaError::MalformedKey { table, ref detail }
            if table == Table::CreatureSpawn && detail.contains("entry")),
        "{refusal}"
    );
}

/// The whole band has to fit both 24-bit fields, or a Package spawn could not be addressed at all.
#[test]
fn the_whole_package_creature_band_fits_the_guid_fields() {
    const { assert!(PACKAGE_CREATURE_ID_CEIL <= MAX_CREATURE_GUID_COMPONENT) };
    assert!(is_package_creature_id(PACKAGE_CREATURE_ID_FLOOR));
    assert!(is_package_creature_id(PACKAGE_CREATURE_ID_CEIL));
    assert!(!is_package_creature_id(PACKAGE_CREATURE_ID_CEIL + 1));
    assert!(!is_fixture_reserved_creature_id(PACKAGE_CREATURE_ID_FLOOR));
}

/// The key already carries the map, the template and the spawn identifier, so a claim that also
/// set them as columns could disagree with itself.
#[test]
fn a_spawn_may_not_claim_its_own_key_columns() {
    for column in ["map_id", "entry", "spawn_id", "guid"] {
        let fields = format!(r#"{{"{column}":{{"type":"u32","value":1}}}}"#);
        let refusal = PackageDelta::parse(&artifact(
            "example.pkg",
            &creature_spawn_claim(
                REAL_MAP,
                REAL_CREATURE,
                REAL_CREATURE_SPAWN,
                "update",
                &fields,
            ),
        ))
        .expect_err("the claim must be refused");

        assert_eq!(
            refusal,
            DeltaError::KeyColumnClaimed {
                table: Table::CreatureSpawn,
                field: column.to_owned()
            }
        );
    }
}

/// A spawn's live state — when it respawns, when its corpse decays, which life is standing — is
/// written by the running Shard, never by an artifact.
#[test]
fn a_spawn_may_not_claim_the_shards_live_state() {
    for column in ["respawn_at", "despawn_at", "life_seq"] {
        let fields = format!(r#"{{"{column}":{{"type":"u32","value":1}}}}"#);
        let refusal = PackageDelta::parse(&artifact(
            "example.pkg",
            &creature_spawn_claim(
                REAL_MAP,
                REAL_CREATURE,
                REAL_CREATURE_SPAWN,
                "update",
                &fields,
            ),
        ))
        .expect_err("the claim must be refused");

        assert_eq!(
            refusal,
            DeltaError::UnknownField {
                table: Table::CreatureSpawn,
                field: column.to_owned()
            }
        );
    }
}
