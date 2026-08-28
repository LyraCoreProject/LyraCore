//! Several Packages' scripts, together: what merges (nothing) and what collides.
//!
//! A Package Delta's tracer merges columns because a row has a base import behind it. A Runtime
//! Script has none, so there is nothing to merge and identity is the only thing to trace.

use lyracore_package_delta::{trace_scripts, ScriptArtifact, ScriptConflict};

const HASH_A: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn artifact(package: &str, scripts: &[(u32, &str, &str)]) -> ScriptArtifact {
    let scripts: Vec<String> = scripts
        .iter()
        .map(|(script_id, name, event)| {
            format!(
                r#"{{"script_id":{script_id},"name":"{name}","event":"{event}","priority":0,"enabled":true,"source":"grant_xp(event.actor, 1)"}}"#
            )
        })
        .collect();
    ScriptArtifact::parse(&format!(
        r#"{{"kind":"script","version":1,"package":"{package}","source_hash":"{HASH_A}","scripts":[{}]}}"#,
        scripts.join(",")
    ))
    .expect("artifact parses")
}

#[test]
fn packages_shipping_different_scripts_all_reach_the_plan() {
    let trace = trace_scripts(&[
        artifact("example.alpha", &[(100_001, "alpha.greet", "on_login")]),
        artifact("example.zeta", &[(100_002, "zeta.farewell", "on_logout")]),
    ]);

    assert!(trace.is_clear());
    assert_eq!(trace.scripts().len(), 2);
}

/// Several Packages may bind to ONE event without disagreeing. That is the normal case, and
/// `priority` is what orders them — not a conflict to resolve.
#[test]
fn two_packages_binding_to_one_event_is_not_a_conflict() {
    let trace = trace_scripts(&[
        artifact("example.alpha", &[(100_001, "alpha.greet", "on_login")]),
        artifact("example.zeta", &[(100_002, "zeta.greet", "on_login")]),
    ]);

    assert!(trace.is_clear());
    assert_eq!(trace.scripts().len(), 2);
}

#[test]
fn the_plan_is_ordered_by_identifier_whatever_order_the_packages_arrived_in() {
    let forwards = trace_scripts(&[
        artifact("example.alpha", &[(100_009, "alpha.a", "on_login")]),
        artifact("example.zeta", &[(100_002, "zeta.z", "on_login")]),
    ]);
    let backwards = trace_scripts(&[
        artifact("example.zeta", &[(100_002, "zeta.z", "on_login")]),
        artifact("example.alpha", &[(100_009, "alpha.a", "on_login")]),
    ]);

    let ids = |trace: &lyracore_package_delta::ScriptTrace| -> Vec<u32> {
        trace
            .scripts()
            .iter()
            .map(|t| t.script.script_id())
            .collect()
    };
    assert_eq!(ids(&forwards), [100_002, 100_009]);
    assert_eq!(ids(&forwards), ids(&backwards));
}

#[test]
fn every_traced_script_carries_the_package_that_ships_it() {
    let trace = trace_scripts(&[
        artifact("example.alpha", &[(100_001, "alpha.greet", "on_login")]),
        artifact("example.zeta", &[(100_002, "zeta.farewell", "on_logout")]),
    ]);

    let owners: Vec<&str> = trace
        .scripts()
        .iter()
        .map(|traced| traced.package.as_str())
        .collect();
    assert_eq!(owners, ["example.alpha", "example.zeta"]);
    assert!(trace
        .scripts()
        .iter()
        .all(|traced| traced.source_hash.as_str() == HASH_A));
}

#[test]
fn two_packages_shipping_one_identifier_is_a_conflict_naming_both() {
    let trace = trace_scripts(&[
        artifact("example.alpha", &[(100_001, "alpha.greet", "on_login")]),
        artifact("example.zeta", &[(100_001, "zeta.greet", "on_login")]),
    ]);

    assert!(!trace.is_clear());
    assert_eq!(trace.conflicts().len(), 1);
    let report = trace.conflicts()[0].to_string();
    assert!(report.contains("example.alpha"), "{report}");
    assert!(report.contains("example.zeta"), "{report}");
    assert!(matches!(
        trace.conflicts()[0],
        ScriptConflict::Id {
            script_id: 100_001,
            ..
        }
    ));
}

#[test]
fn two_packages_shipping_one_name_is_a_conflict_naming_both() {
    let trace = trace_scripts(&[
        artifact("example.alpha", &[(100_001, "shared.greet", "on_login")]),
        artifact("example.zeta", &[(100_002, "shared.greet", "on_login")]),
    ]);

    assert!(!trace.is_clear());
    let report = trace.conflicts()[0].to_string();
    assert!(report.contains("shared.greet"), "{report}");
    assert!(report.contains("example.alpha"), "{report}");
    assert!(report.contains("example.zeta"), "{report}");
    assert!(matches!(trace.conflicts()[0], ScriptConflict::Name { .. }));
}

/// The operator fixing a broken Package Inventory wants the whole list in one pass, not one
/// refusal per re-run.
#[test]
fn every_disagreement_is_reported_rather_than_the_first() {
    let trace = trace_scripts(&[
        artifact(
            "example.alpha",
            &[
                (100_001, "alpha.a", "on_login"),
                (100_002, "alpha.b", "on_kill"),
            ],
        ),
        artifact(
            "example.zeta",
            &[
                (100_001, "zeta.a", "on_login"),
                (100_002, "zeta.b", "on_kill"),
            ],
        ),
    ]);

    assert_eq!(trace.conflicts().len(), 2);
}

/// One Package's own scripts never collide with each other here: the artifact parser already
/// refused that, so a conflict in a trace is always BETWEEN Packages.
#[test]
fn one_package_alone_can_never_produce_a_conflict() {
    let trace = trace_scripts(&[artifact(
        "example.alpha",
        &[
            (100_001, "alpha.a", "on_login"),
            (100_002, "alpha.b", "on_kill"),
        ],
    )]);

    assert!(trace.is_clear());
}

#[test]
fn no_packages_at_all_is_a_clear_and_empty_plan() {
    let trace = trace_scripts(&[]);

    assert!(trace.is_clear());
    assert!(trace.scripts().is_empty());
}
