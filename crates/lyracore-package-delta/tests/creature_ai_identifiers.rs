//! The creature-ai family's identifier policy and its global keys.
//!
//! One band covers three tables that share nothing else: a broadcast text, a summon placement and
//! a quest event requirement are three independent identifier spaces. None of the three names a
//! map, so every claim in this family is global and reaches every Shard.

mod common;

use common::{
    artifact, broadcast_text_claim, quest_event_requirement_claim, summon_claim,
    PACKAGE_CREATURE_AI, REAL_BROADCAST_TEXT, WHOLE_BROADCAST_TEXT_ROW,
    WHOLE_QUEST_EVENT_REQUIREMENT_ROW, WHOLE_SUMMON_ROW,
};
use lyracore_package_delta::{
    is_fixture_reserved_creature_ai_id, is_package_creature_ai_id, DeltaError, PackageDelta,
    PrimaryKey, Table, PACKAGE_CREATURE_AI_ID_CEIL, PACKAGE_CREATURE_AI_ID_FLOOR,
};

const A_LINE: &str = r#"{"male_text":{"type":"string","value":"You will burn."}}"#;

#[test]
fn every_creature_ai_key_is_its_own_row_identifier() {
    let text = PrimaryKey::creature_ai_broadcast_text(PACKAGE_CREATURE_AI).expect("a Package text");
    let summon = PrimaryKey::creature_ai_summon(PACKAGE_CREATURE_AI).expect("a Package summon");
    let requirement = PrimaryKey::quest_event_requirement(u64::from(PACKAGE_CREATURE_AI))
        .expect("a Package requirement");

    assert_eq!(text.row_id(), u64::from(PACKAGE_CREATURE_AI));
    assert_eq!(text.table(), Table::CreatureAiBroadcastText);
    assert_eq!(summon.row_id(), u64::from(PACKAGE_CREATURE_AI));
    assert_eq!(summon.table(), Table::CreatureAiSummon);
    assert_eq!(requirement.row_id(), u64::from(PACKAGE_CREATURE_AI));
    assert_eq!(requirement.table(), Table::QuestEventRequirement);
}

/// The `creature-ai` base import writes these three tables with global SQL, so a claim on one
/// reaches every Shard. Nothing in the family carries a map to route by.
#[test]
fn no_creature_ai_claim_states_a_map() {
    for key in [
        PrimaryKey::creature_ai_broadcast_text(PACKAGE_CREATURE_AI).expect("a Package text"),
        PrimaryKey::creature_ai_summon(PACKAGE_CREATURE_AI).expect("a Package summon"),
        PrimaryKey::quest_event_requirement(u64::from(PACKAGE_CREATURE_AI))
            .expect("a Package requirement"),
    ] {
        assert_eq!(key.map_id(), None, "{key}");
    }
}

#[test]
fn a_package_may_insert_a_creature_ai_row_inside_the_package_range() {
    for id in [
        PACKAGE_CREATURE_AI_ID_FLOOR,
        17_500_000,
        PACKAGE_CREATURE_AI_ID_CEIL,
    ] {
        let json = artifact(
            "example.pkg",
            &broadcast_text_claim(id as u32, "insert", WHOLE_BROADCAST_TEXT_ROW),
        );

        PackageDelta::parse(&json)
            .unwrap_or_else(|e| panic!("broadcast text {id} must insert: {e}"));
    }
}

#[test]
fn an_insert_outside_the_package_range_is_refused() {
    for id in [
        u64::from(REAL_BROADCAST_TEXT),
        PACKAGE_CREATURE_AI_ID_FLOOR - 1,
        PACKAGE_CREATURE_AI_ID_CEIL + 1,
    ] {
        let json = artifact(
            "example.pkg",
            &broadcast_text_claim(id as u32, "insert", WHOLE_BROADCAST_TEXT_ROW),
        );

        assert_eq!(
            PackageDelta::parse(&json).expect_err("the insert must be refused"),
            DeltaError::CreatureAiIdNotClientSafe { id },
            "broadcast text {id}"
        );
    }
}

/// Every insertable creature-ai table shares one band, so one out-of-range identifier is refused
/// the same way whichever of them names it.
#[test]
fn one_band_covers_every_insertable_creature_ai_table() {
    let out_of_band = PACKAGE_CREATURE_AI_ID_FLOOR - 1;
    let claims = [
        broadcast_text_claim(out_of_band as u32, "insert", WHOLE_BROADCAST_TEXT_ROW),
        summon_claim(out_of_band as u32, "insert", WHOLE_SUMMON_ROW),
        quest_event_requirement_claim(out_of_band, "insert", WHOLE_QUEST_EVENT_REQUIREMENT_ROW),
    ];

    for claim in claims {
        assert_eq!(
            PackageDelta::parse(&artifact("example.pkg", &claim))
                .expect_err("the insert must be refused"),
            DeltaError::CreatureAiIdNotClientSafe { id: out_of_band },
            "{claim}"
        );
    }
}

#[test]
fn an_update_may_name_a_real_imported_broadcast_text() {
    PackageDelta::parse(&artifact(
        "example.pkg",
        &broadcast_text_claim(REAL_BROADCAST_TEXT, "update", A_LINE),
    ))
    .expect("retuning an imported line is the point");
}

#[test]
fn a_fixture_reserved_creature_ai_row_is_refused_under_every_operation() {
    for operation in ["insert", "update"] {
        let fields = if operation == "insert" {
            WHOLE_BROADCAST_TEXT_ROW
        } else {
            A_LINE
        };

        assert_eq!(
            PackageDelta::parse(&artifact(
                "example.pkg",
                &broadcast_text_claim(5_090_001, operation, fields),
            ))
            .expect_err("the claim must be refused"),
            DeltaError::CreatureAiIdFixtureReserved { id: 5_090_001 },
            "{operation}"
        );
    }
}

#[test]
fn the_package_creature_ai_band_is_its_own() {
    let id = u64::from(PACKAGE_CREATURE_AI);
    assert!(is_package_creature_ai_id(id));
    assert!(!is_package_creature_ai_id(PACKAGE_CREATURE_AI_ID_CEIL + 1));
    assert!(!is_fixture_reserved_creature_ai_id(id));
}

/// Row 0 is not a row. Every creature-ai table keys on a surrogate identifier, so the refusal is
/// the same on all three.
#[test]
fn row_zero_is_not_a_creature_ai_row() {
    for claim in [
        broadcast_text_claim(0, "update", A_LINE),
        summon_claim(0, "update", r#"{"lifetime_ms":{"type":"u32","value":1}}"#),
        quest_event_requirement_claim(0, "update", r#"{"quest_entry":{"type":"u32","value":1}}"#),
    ] {
        let refusal = PackageDelta::parse(&artifact("example.pkg", &claim))
            .expect_err("the claim must be refused")
            .to_string();

        assert!(refusal.contains("`id` 0 is not a row"), "{refusal}");
    }
}

/// The claim carries the row identifier in its key, so naming it again in `fields` would let a
/// Package state it twice and disagree with itself.
#[test]
fn a_creature_ai_claim_may_not_restate_its_key() {
    assert_eq!(
        PackageDelta::parse(&artifact(
            "example.pkg",
            &broadcast_text_claim(
                REAL_BROADCAST_TEXT,
                "update",
                r#"{"id":{"type":"u32","value":1}}"#
            ),
        ))
        .expect_err("the claim must be refused"),
        DeltaError::KeyColumnClaimed {
            table: Table::CreatureAiBroadcastText,
            field: "id".to_owned()
        }
    );
}
