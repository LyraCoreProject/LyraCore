mod common;

use common::{
    artifact, form_claim, form_spell_claim, spell_form_claim, PACKAGE_FORM, PACKAGE_SPELL,
    REAL_FORM, REAL_SPELL, WHOLE_FORM_ROW,
};
use lyracore_package_delta::ids::PackageRange;
use lyracore_package_delta::{
    canonicalize, is_package_form_id, packed_form_spell_id, packed_spell_form_id, DeltaError,
    PackageDelta, PrimaryKey, Table, SPELL_FAMILY,
};

const A_POWER_RULE: &str = r#"{"power_on_entry":{"type":"u8","value":0}}"#;

#[test]
fn a_package_may_insert_a_form_and_its_rows_inside_the_package_form_range() {
    for form_id in [18_000_000u32, 18_500_000, 18_999_999] {
        let claims = [
            form_claim(form_id, "insert", WHOLE_FORM_ROW),
            form_spell_claim(form_id, PACKAGE_SPELL, "insert"),
            spell_form_claim(REAL_SPELL, form_id, "insert", false),
        ]
        .join(",");

        let delta = PackageDelta::parse(&artifact("example.aspect", &claims))
            .unwrap_or_else(|e| panic!("Form {form_id} must insert: {e}"));

        assert_eq!(delta.claim_counts().inserted_rows, 3);
        assert!(delta
            .claims()
            .iter()
            .all(|claim| claim.table().family() == SPELL_FAMILY));
    }
}

#[test]
fn a_form_insert_outside_the_package_form_range_is_refused() {
    for form_id in [1u32, REAL_FORM, 32, 6_000_001, 17_999_999, 19_000_000] {
        let refused = [
            form_claim(form_id, "insert", WHOLE_FORM_ROW),
            form_spell_claim(form_id, PACKAGE_SPELL, "insert"),
            spell_form_claim(PACKAGE_SPELL, form_id, "insert", false),
        ];
        for claim in refused {
            assert_eq!(
                PackageDelta::parse(&artifact("example.aspect", &claim))
                    .expect_err("the insert must be refused"),
                DeltaError::IdOutsidePackageRange {
                    range: PackageRange::Form,
                    id: u64::from(form_id)
                },
                "{claim}"
            );
        }
    }
}

#[test]
fn an_imported_form_is_tunable_but_its_passives_are_the_imports() {
    let update = artifact(
        "example.tuner",
        &form_claim(REAL_FORM, "update", A_POWER_RULE),
    );
    let delta = PackageDelta::parse(&update).expect("an imported Form is updatable");
    assert_eq!(delta.claims()[0].key().row_id(), u64::from(REAL_FORM));

    let passive = artifact(
        "example.tuner",
        &form_spell_claim(REAL_FORM, PACKAGE_SPELL, "insert"),
    );
    assert!(matches!(
        PackageDelta::parse(&passive),
        Err(DeltaError::IdOutsidePackageRange {
            range: PackageRange::Form,
            ..
        })
    ));
}

#[test]
fn a_passive_names_no_column_so_it_cannot_be_updated() {
    let json = artifact(
        "example.aspect",
        &form_spell_claim(PACKAGE_FORM, PACKAGE_SPELL, "update"),
    );

    assert_eq!(
        PackageDelta::parse(&json).expect_err("the update must be refused"),
        DeltaError::EmptyUpdate {
            table: Table::FormSpell
        }
    );
}

#[test]
fn form_zero_is_no_form() {
    let json = artifact("example.aspect", &form_claim(0, "update", A_POWER_RULE));

    assert!(matches!(
        PackageDelta::parse(&json),
        Err(DeltaError::MalformedKey { table: Table::Form, ref detail }) if detail.contains("a Form")
    ));
}

#[test]
fn form_row_keys_pack_into_the_durable_ids() {
    let passive = PrimaryKey::form_spell(PACKAGE_FORM, PACKAGE_SPELL).expect("claimable");
    let form_set = PrimaryKey::spell_form(PACKAGE_SPELL, PACKAGE_FORM).expect("claimable");

    assert_eq!(
        passive.row_id(),
        packed_form_spell_id(PACKAGE_FORM, PACKAGE_SPELL)
    );
    assert_eq!(
        form_set.row_id(),
        packed_spell_form_id(PACKAGE_SPELL, PACKAGE_FORM)
    );
    assert_eq!(passive.form_id(), PACKAGE_FORM);
    assert_eq!(form_set.form_id(), PACKAGE_FORM);
    assert!(is_package_form_id(PACKAGE_FORM));
    assert!(!is_package_form_id(REAL_FORM));
}

#[test]
fn a_form_artifact_is_canonical_after_one_pass() {
    let claims = [
        form_claim(PACKAGE_FORM, "insert", WHOLE_FORM_ROW),
        form_spell_claim(PACKAGE_FORM, PACKAGE_SPELL, "insert"),
        spell_form_claim(PACKAGE_SPELL, PACKAGE_FORM, "insert", true),
    ]
    .join(",");

    let once = canonicalize(&artifact("example.aspect", &claims)).expect("it parses");

    assert_eq!(canonicalize(&once).expect("it parses again"), once);
    assert!(once.contains(r#""key":{"spell_id":6000001,"form_id":18000001}"#));
}
