mod common;

use common::{artifact, spell_follow_up_claim, PACKAGE_SPELL, REAL_SPELL};
use lyracore_package_delta::ids::PackageRange;
use lyracore_package_delta::{DeltaError, PackageDelta, SPELL_FAMILY};

#[test]
fn a_package_spell_may_follow_up_with_any_spell() {
    for follow_up in [REAL_SPELL, PACKAGE_SPELL + 1] {
        for operation in ["insert", "update"] {
            let delta = PackageDelta::parse(&artifact(
                "example.bubble",
                &spell_follow_up_claim(PACKAGE_SPELL, follow_up, operation),
            ))
            .unwrap_or_else(|e| panic!("{operation} {follow_up}: {e}"));
            assert_eq!(delta.claims()[0].table().family(), SPELL_FAMILY);
        }
    }
}

#[test]
fn an_imported_spell_takes_a_package_follow_up_spell() {
    for operation in ["insert", "update"] {
        PackageDelta::parse(&artifact(
            "example.bubble",
            &spell_follow_up_claim(REAL_SPELL, PACKAGE_SPELL, operation),
        ))
        .unwrap_or_else(|e| panic!("{operation}: {e}"));
    }
}

#[test]
fn an_imported_spell_refuses_an_imported_follow_up_spell() {
    for operation in ["insert", "update"] {
        assert_eq!(
            PackageDelta::parse(&artifact(
                "example.bubble",
                &spell_follow_up_claim(REAL_SPELL, 25771, operation),
            ))
            .expect_err("the import owns follow-ups between imported spells"),
            DeltaError::IdOutsidePackageRange {
                range: PackageRange::Spell,
                id: 25771
            },
            "{operation}"
        );
    }
}
