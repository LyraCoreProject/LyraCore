//! What one Package Delta reports about its own size, which is what an applier records as
//! provenance and what a dry-run check prints.

mod common;

use common::{
    artifact, effect_claim, item_claim, spell_claim, PACKAGE_ITEM, PACKAGE_SPELL, REAL_SPELL,
    WHOLE_EFFECT_ROW, WHOLE_ITEM_ROW, WHOLE_SPELL_ROW,
};
use lyracore_package_delta::PackageDelta;

fn delta(claims: &str) -> PackageDelta {
    PackageDelta::parse(&artifact("example.counted", claims)).expect("artifact parses")
}

#[test]
fn a_package_counts_its_updates_and_its_inserts_apart() {
    let claims = [
        spell_claim(
            REAL_SPELL,
            "update",
            r#"{"cooldown_ms":{"type":"u32","value":1500}}"#,
        ),
        effect_claim(
            REAL_SPELL,
            0,
            "update",
            r#"{"base_points":{"type":"i32","value":120}}"#,
        ),
        spell_claim(PACKAGE_SPELL, "insert", WHOLE_SPELL_ROW),
        effect_claim(PACKAGE_SPELL, 0, "insert", WHOLE_EFFECT_ROW),
        effect_claim(PACKAGE_SPELL, 1, "insert", WHOLE_EFFECT_ROW),
    ]
    .join(",");

    let counts = delta(&claims).claim_counts();

    assert_eq!(counts.updated_rows, 2);
    assert_eq!(counts.inserted_spells, 1);
    assert_eq!(counts.inserted_effects, 2);
    // `inserted_rows` is the family-generic total: the spell writer fills it too, as the sum of
    // its two legacy counts.
    assert_eq!(counts.inserted_rows, 3);
}

#[test]
fn a_package_that_only_tunes_existing_rows_inserts_nothing() {
    let counts = delta(&spell_claim(
        REAL_SPELL,
        "update",
        r#"{"gcd_ms":{"type":"u32","value":1000}}"#,
    ))
    .claim_counts();

    assert_eq!(counts.updated_rows, 1);
    assert_eq!(counts.inserted_spells, 0);
    assert_eq!(counts.inserted_effects, 0);
    assert_eq!(counts.inserted_rows, 0);
}

/// The items family has no legacy pair of its own — every insert counts through `inserted_rows`
/// alone, and `inserted_spells`/`inserted_effects` stay at zero.
#[test]
fn an_item_package_counts_its_inserts_through_the_family_generic_total_only() {
    let claims = [
        item_claim(25, "update", r#"{"buy_price":{"type":"u32","value":100}}"#),
        item_claim(PACKAGE_ITEM, "insert", WHOLE_ITEM_ROW),
    ]
    .join(",");

    let counts = delta(&claims).claim_counts();

    assert_eq!(counts.updated_rows, 1);
    assert_eq!(counts.inserted_rows, 1);
    assert_eq!(counts.inserted_spells, 0);
    assert_eq!(counts.inserted_effects, 0);
}
