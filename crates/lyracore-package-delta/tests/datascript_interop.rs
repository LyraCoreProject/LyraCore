//! The contract between the Datascript authoring library and this crate.
//!
//! The library writes artifacts in TypeScript; this crate reads them in Rust. Three things have to
//! hold across that seam, and each has a test here:
//!
//!  * an artifact the library emits parses, traces clean, and canonicalizes to the SAME bytes it was
//!    written with;
//!  * both sides name the same durable columns, so a schema move cannot leave one behind;
//!  * both sides spell an `f32` identically, which is the only value type where two languages could
//!    plausibly disagree.
//!
//! # Licensing
//!
//! Every fixture here is hand-written and every value in it is invented. `base-snapshot.json` is a
//! synthetic Base Snapshot, and `fire-nova.expected.json` was emitted from it. No client data, and
//! nothing derived from client data, is involved.

use std::collections::BTreeSet;
use std::path::PathBuf;

use lyracore_package_delta::{
    canonicalize, packed_spell_effect_id, trace, DeltaError, Operation, PackageDelta, PrimaryKey,
    Table,
};

fn fixture(name: &str) -> String {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "..",
        "..",
        "datascripts",
        "tests",
        "fixtures",
        name,
    ]
    .iter()
    .collect();
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn authoring_library(name: &str) -> String {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "..",
        "..",
        "datascripts",
        "lib",
        name,
    ]
    .iter()
    .collect();
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn fire_nova() -> PackageDelta {
    PackageDelta::parse(&fixture("fire-nova.expected.json"))
        .expect("the frozen Datascript artifact parses")
}

#[test]
fn an_artifact_the_authoring_library_wrote_is_already_canonical() {
    let written = fixture("fire-nova.expected.json");

    assert_eq!(
        canonicalize(&written).expect("it parses"),
        written,
        "the TypeScript writer and the Rust writer must produce the same bytes"
    );
}

#[test]
fn the_frozen_artifact_traces_without_a_conflict() {
    let traced = trace(&[fire_nova()]);

    assert!(traced.is_clear(), "{:?}", traced.conflicts());
    assert_eq!(traced.rows().len(), 15); // five ranks, each a header and two effects
}

#[test]
fn the_rank_ladder_inserts_five_package_spells_and_their_child_effects() {
    let counts = fire_nova().claim_counts();

    assert_eq!(counts.inserted_spells, 5);
    assert_eq!(counts.inserted_effects, 10);
    assert_eq!(counts.updated_rows, 0);
}

/// The child effects a clone creates follow the packed key rule, derived rather than authored.
#[test]
fn every_child_effect_key_packs_from_its_spell_and_index() {
    for claim in fire_nova().claims() {
        if let PrimaryKey::SpellEffect {
            spell_id,
            effect_index,
        } = claim.key()
        {
            assert_eq!(
                claim.key().row_id(),
                packed_spell_effect_id(spell_id, effect_index)
            );
            assert_eq!(claim.operation(), Operation::Insert);
        }
    }
}

/// Every insert the library emits carries the whole row, because a clone copies every column.
#[test]
fn every_inserted_row_carries_every_column_of_its_table() {
    for claim in fire_nova().claims() {
        let claimed: BTreeSet<&str> = claim.fields().keys().map(String::as_str).collect();
        let expected: BTreeSet<&str> = claim.table().columns().iter().map(|c| c.name).collect();
        assert_eq!(claimed, expected, "{}", claim.key());
    }
}

/// The Datascript catalogue and this crate's catalogue are two hand-maintained lists of one schema.
/// A column added to one and not the other would let an author write a claim the parser refuses, or
/// hide a column an author should be able to set. The scanner is deliberately narrow: it reads the
/// `column:` members of the two exported catalogues and nothing else.
#[test]
fn the_datascript_column_catalogue_names_the_same_columns_as_this_crate() {
    let source = authoring_library("schema.ts");

    for (marker, table) in [
        ("export const SPELL_COLUMNS", Table::Spell),
        ("export const SPELL_EFFECT_COLUMNS", Table::SpellEffect),
    ] {
        let named = durable_columns(&source, marker);
        assert!(
            !named.is_empty(),
            "the scanner found no columns after `{marker}`; it has stopped watching anything"
        );
        let expected: Vec<&str> = table.columns().iter().map(|c| c.name).collect();
        assert_eq!(named, expected, "`{table}` catalogues disagree");
    }
}

/// The `column: "…"` members of one exported catalogue, in declaration order.
fn durable_columns(source: &str, marker: &str) -> Vec<String> {
    let start = source
        .find(marker)
        .unwrap_or_else(|| panic!("`{marker}` is gone from the authoring library's catalogue"));
    let body = &source[start..];
    let body = &body[..body
        .find("\n};")
        .expect("the catalogue is a closed object literal")];

    body.match_indices("column: \"")
        .map(|(at, tag)| {
            let rest = &body[at + tag.len()..];
            rest[..rest.find('"').expect("a closed string")].to_owned()
        })
        .collect()
}

/// The two writers agree on every float, including the ones a shortest-representation algorithm has
/// to think about. The fixture's literals came from Rust; the TypeScript suite asserts the same
/// file, so neither side can drift alone.
#[test]
fn both_writers_spell_a_float_the_same_way() {
    let cases: serde_json::Value =
        serde_json::from_str(&fixture("f32-literals.json")).expect("the float fixture parses");
    let cases = cases.as_array().expect("a list of cases");
    let mut checked = 0;

    for entry in cases {
        let bits = u32::try_from(entry["bits"].as_u64().expect("a bit pattern")).expect("32 bits");
        let expected = entry["literal"].as_str().expect("a literal");
        let claim = format!(
            r#"{{"version":1,"package":"example.floats","source_hash":"{}","claims":[{{"table":"game_spell_effect","key":{{"spell_id":133,"effect_index":0}},"operation":"update","fields":{{"radius_yd":{{"type":"f32","value":{}}}}}}}]}}"#,
            "0".repeat(64),
            f32::from_bits(bits)
        );

        let written = PackageDelta::parse(&claim)
            .expect("a finite f32 claim parses")
            .to_canonical_json();

        assert!(
            written.contains(&format!("\"value\":{expected}")),
            "bits {bits}: expected `{expected}` in {written}"
        );
        checked += 1;
    }

    assert!(
        checked >= 10,
        "the float fixture went missing: {checked} cases"
    );
}

// ---------------------------------------------------------------------------------------------
// Refusal parity: the rules the authoring library mirrors, asserted here on the artifact the
// library would have had to write to break each one.
// ---------------------------------------------------------------------------------------------

fn artifact(table: &str, key: &str, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"version":1,"package":"example.refusals","source_hash":"{}","claims":[{{"table":"{table}","key":{key},"operation":"{operation}","fields":{fields}}}]}}"#,
        "0".repeat(64)
    )
}

#[test]
fn a_fixture_reserved_spell_is_refused_by_the_parser_too() {
    let refusal = PackageDelta::parse(&artifact(
        "game_spell",
        r#"{"spell_id":50000}"#,
        "update",
        r#"{"cooldown_ms":{"type":"u32","value":1500}}"#,
    ))
    .expect_err("a fixture-reserved spell is refused");

    assert_eq!(
        refusal,
        DeltaError::SpellIdFixtureReserved { spell_id: 50_000 }
    );
}

#[test]
fn an_effect_index_above_the_third_slot_is_refused_by_the_parser_too() {
    let refusal = PackageDelta::parse(&artifact(
        "game_spell_effect",
        r#"{"spell_id":133,"effect_index":3}"#,
        "update",
        r#"{"base_points":{"type":"i32","value":1}}"#,
    ))
    .expect_err("a fourth effect slot is refused");

    assert_eq!(
        refusal,
        DeltaError::EffectIndexOutOfRange { effect_index: 3 }
    );
}

#[test]
fn an_insert_that_does_not_carry_the_whole_row_is_refused_by_the_parser_too() {
    let refusal = PackageDelta::parse(&artifact(
        "game_spell",
        r#"{"spell_id":6000100}"#,
        "insert",
        r#"{"cooldown_ms":{"type":"u32","value":1500}}"#,
    ))
    .expect_err("a partial insert is refused");

    match refusal {
        DeltaError::IncompleteInsert { table, missing } => {
            assert_eq!(table, Table::Spell);
            assert!(missing.contains(&"name".to_owned()), "{missing:?}");
            assert!(missing.contains(&"proc_charges".to_owned()), "{missing:?}");
        }
        other => panic!("expected an incomplete insert, got {other:?}"),
    }
}
