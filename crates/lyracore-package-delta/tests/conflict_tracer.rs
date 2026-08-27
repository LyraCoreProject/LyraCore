//! Tracing several Packages together: what merges, what conflicts, and what the report says.

mod common;

use common::{
    artifact, effect_claim, item_claim, spell_claim, PACKAGE_ITEM, PACKAGE_SPELL, REAL_SPELL,
    WHOLE_ITEM_ROW, WHOLE_SPELL_ROW,
};
use lyracore_package_delta::{trace, ClaimConflict, Operation, PackageDelta, Table};

fn delta(package: &str, claims: &str) -> PackageDelta {
    PackageDelta::parse(&artifact(package, claims)).expect("artifact parses")
}

const COOLDOWN_1500: &str = r#"{"cooldown_ms":{"type":"u32","value":1500}}"#;
const COOLDOWN_3000: &str = r#"{"cooldown_ms":{"type":"u32","value":3000}}"#;
const GCD_1000: &str = r#"{"gcd_ms":{"type":"u32","value":1000}}"#;

#[test]
fn two_packages_claiming_different_columns_of_one_row_merge() {
    let first = delta(
        "example.first",
        &spell_claim(REAL_SPELL, "update", COOLDOWN_1500),
    );
    let second = delta(
        "example.second",
        &spell_claim(REAL_SPELL, "update", GCD_1000),
    );

    let traced = trace(&[first, second]);

    assert!(traced.is_clear(), "{:?}", traced.conflicts());
    assert_eq!(traced.rows().len(), 1);
    let row = &traced.rows()[0];
    assert_eq!(row.table(), Table::Spell);
    assert_eq!(row.operation(), Operation::Update);
    assert_eq!(
        row.fields()["cooldown_ms"].claimed_by.as_str(),
        "example.first"
    );
    assert_eq!(row.fields()["gcd_ms"].claimed_by.as_str(), "example.second");
}

#[test]
fn two_packages_claiming_the_same_column_conflict_and_the_report_names_both_and_the_claim() {
    let first = delta(
        "example.first",
        &spell_claim(REAL_SPELL, "update", COOLDOWN_1500),
    );
    let second = delta(
        "example.second",
        &spell_claim(REAL_SPELL, "update", COOLDOWN_3000),
    );

    let traced = trace(&[first, second]);

    assert!(!traced.is_clear());
    assert_eq!(traced.conflicts().len(), 1);
    let report = traced.conflicts()[0].to_string();
    for expected in [
        "example.first",
        "example.second",
        "game_spell",
        "spell_id=133",
        "cooldown_ms",
        "1500",
        "3000",
    ] {
        assert!(
            report.contains(expected),
            "`{expected}` absent from: {report}"
        );
    }
}

#[test]
fn a_same_column_conflict_is_reported_from_either_direction() {
    let build =
        |package: &str, fields: &str| delta(package, &spell_claim(REAL_SPELL, "update", fields));

    let forward = trace(&[
        build("example.first", COOLDOWN_1500),
        build("example.second", COOLDOWN_3000),
    ]);
    let backward = trace(&[
        build("example.second", COOLDOWN_3000),
        build("example.first", COOLDOWN_1500),
    ]);

    assert_eq!(forward.conflicts().len(), 1);
    assert_eq!(backward.conflicts().len(), 1);
}

#[test]
fn two_packages_inserting_the_same_primary_key_conflict() {
    let first = delta(
        "example.first",
        &spell_claim(PACKAGE_SPELL, "insert", WHOLE_SPELL_ROW),
    );
    let second = delta(
        "example.second",
        &spell_claim(PACKAGE_SPELL, "insert", WHOLE_SPELL_ROW),
    );

    let traced = trace(&[first, second]);

    let inserted = traced
        .conflicts()
        .iter()
        .find(|c| matches!(c, ClaimConflict::InsertedRow { .. }))
        .expect("the key collision must be reported");
    let report = inserted.to_string();
    assert!(report.contains("example.first"), "{report}");
    assert!(report.contains("example.second"), "{report}");
    assert!(report.contains("spell_id=6000001"), "{report}");
}

#[test]
fn one_package_inserting_and_another_updating_the_same_row_is_not_a_key_collision() {
    let owner = delta(
        "example.owner",
        &spell_claim(PACKAGE_SPELL, "insert", WHOLE_SPELL_ROW),
    );
    let tuner = delta(
        "example.tuner",
        &spell_claim(PACKAGE_SPELL, "update", GCD_1000),
    );

    let traced = trace(&[owner, tuner]);

    // `gcd_ms` is one of the columns the complete insert carries, so the two Packages do collide on
    // that column. What they must NOT collide on is the primary key.
    assert!(
        !traced
            .conflicts()
            .iter()
            .any(|c| matches!(c, ClaimConflict::InsertedRow { .. })),
        "{:?}",
        traced.conflicts()
    );
    assert_eq!(traced.rows()[0].operation(), Operation::Insert);
}

#[test]
fn a_spell_header_and_its_child_effect_are_different_rows() {
    let header = delta(
        "example.first",
        &spell_claim(REAL_SPELL, "update", COOLDOWN_1500),
    );
    let effect = delta(
        "example.second",
        &effect_claim(
            REAL_SPELL,
            0,
            "update",
            r#"{"base_points":{"type":"i32","value":9}}"#,
        ),
    );

    let traced = trace(&[header, effect]);

    assert!(traced.is_clear(), "{:?}", traced.conflicts());
    assert_eq!(traced.rows().len(), 2);
    // 133 for the header, 133 * 4 + 0 for the effect.
    assert_eq!(traced.rows()[0].row_id(), 133);
    assert_eq!(traced.rows()[1].row_id(), 532);
}

#[test]
fn two_effects_of_one_spell_are_different_rows() {
    let field = r#"{"base_points":{"type":"i32","value":9}}"#;
    let first = delta(
        "example.first",
        &effect_claim(REAL_SPELL, 0, "update", field),
    );
    let second = delta(
        "example.second",
        &effect_claim(REAL_SPELL, 1, "update", field),
    );

    let traced = trace(&[first, second]);

    assert!(traced.is_clear(), "{:?}", traced.conflicts());
    assert_eq!(traced.rows().len(), 2);
}

#[test]
fn the_trace_reports_every_conflict_not_only_the_first() {
    let both =
        r#"{"cooldown_ms":{"type":"u32","value":1500},"gcd_ms":{"type":"u32","value":1000}}"#;
    let clash =
        r#"{"cooldown_ms":{"type":"u32","value":3000},"gcd_ms":{"type":"u32","value":2000}}"#;
    let first = delta("example.first", &spell_claim(REAL_SPELL, "update", both));
    let second = delta("example.second", &spell_claim(REAL_SPELL, "update", clash));

    let traced = trace(&[first, second]);

    assert_eq!(traced.conflicts().len(), 2);
    let columns: Vec<String> = traced
        .conflicts()
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    assert!(columns[0].contains("cooldown_ms"), "{columns:?}");
    assert!(columns[1].contains("gcd_ms"), "{columns:?}");
}

#[test]
fn the_conflict_report_reads_the_same_way_for_the_same_input() {
    let build = || {
        let clash =
            r#"{"cooldown_ms":{"type":"u32","value":3000},"gcd_ms":{"type":"u32","value":2000}}"#;
        let both =
            r#"{"cooldown_ms":{"type":"u32","value":1500},"gcd_ms":{"type":"u32","value":1000}}"#;
        vec![
            delta(
                "example.first",
                &[
                    spell_claim(200, "update", both),
                    spell_claim(REAL_SPELL, "update", both),
                ]
                .join(","),
            ),
            delta(
                "example.second",
                &[
                    spell_claim(REAL_SPELL, "update", clash),
                    spell_claim(200, "update", clash),
                ]
                .join(","),
            ),
        ]
    };

    let render = |deltas: &[PackageDelta]| {
        trace(deltas)
            .conflicts()
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    };

    assert_eq!(render(&build()), render(&build()));
    // Sorted by row, then by column: spell 133 before spell 200, cooldown before gcd.
    let report = render(&build());
    let lines: Vec<&str> = report.lines().collect();
    assert_eq!(lines.len(), 4);
    assert!(
        lines[0].contains("spell_id=133") && lines[0].contains("cooldown_ms"),
        "{report}"
    );
    assert!(
        lines[1].contains("spell_id=133") && lines[1].contains("gcd_ms"),
        "{report}"
    );
    assert!(
        lines[2].contains("spell_id=200") && lines[2].contains("cooldown_ms"),
        "{report}"
    );
    assert!(
        lines[3].contains("spell_id=200") && lines[3].contains("gcd_ms"),
        "{report}"
    );
}

#[test]
fn claims_on_different_spells_never_conflict() {
    let first = delta(
        "example.first",
        &spell_claim(REAL_SPELL, "update", COOLDOWN_1500),
    );
    let second = delta("example.second", &spell_claim(200, "update", COOLDOWN_3000));

    let traced = trace(&[first, second]);

    assert!(traced.is_clear(), "{:?}", traced.conflicts());
    assert_eq!(traced.rows().len(), 2);
}

#[test]
fn tracing_nothing_is_clear() {
    let traced = trace(&[]);

    assert!(traced.is_clear());
    assert!(traced.rows().is_empty());
}

/// The item family traces the same way the spell family does: two Packages tuning different
/// columns of one item merge, cleanly.
#[test]
fn two_packages_claiming_different_columns_of_one_item_merge() {
    let first = delta(
        "example.first",
        &item_claim(25, "update", r#"{"buy_price":{"type":"u32","value":100}}"#),
    );
    let second = delta(
        "example.second",
        &item_claim(25, "update", r#"{"sell_price":{"type":"u32","value":25}}"#),
    );

    let traced = trace(&[first, second]);

    assert!(traced.is_clear(), "{:?}", traced.conflicts());
    assert_eq!(traced.rows().len(), 1);
    assert_eq!(traced.rows()[0].table(), Table::Item);
}

/// Two Packages inventing the same item entry conflict the same way two Packages inventing the
/// same spell do.
#[test]
fn two_packages_inserting_the_same_item_entry_conflict() {
    let first = delta(
        "example.first",
        &item_claim(PACKAGE_ITEM, "insert", WHOLE_ITEM_ROW),
    );
    let second = delta(
        "example.second",
        &item_claim(PACKAGE_ITEM, "insert", WHOLE_ITEM_ROW),
    );

    let traced = trace(&[first, second]);

    let inserted = traced
        .conflicts()
        .iter()
        .find(|c| matches!(c, ClaimConflict::InsertedRow { .. }))
        .expect("the key collision must be reported");
    let report = inserted.to_string();
    assert!(report.contains("example.first"), "{report}");
    assert!(report.contains("example.second"), "{report}");
    assert!(report.contains("entry=7000001"), "{report}");
}
