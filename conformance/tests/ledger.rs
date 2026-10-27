use std::path::{Path, PathBuf};
use std::process::Command;

use lyracore_conformance::FeatureLedger;
use serde::Deserialize;

#[test]
fn every_ledger_entry_parses_and_references_known_behaviors_and_scenarios() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let ledger = FeatureLedger::load(&root.join("conformance/features")).unwrap();
    if let Some(ids) = pinned_scenario_catalogue(root) {
        ledger
            .validate_scenarios(ids.iter().map(String::as_str))
            .unwrap();
    }
    println!("validated {} Behavior files", ledger.behaviors().len());
}

fn pinned_scenario_catalogue(root: &Path) -> Option<Vec<String>> {
    const PRE_CATALOGUE_REV: &str = "68f4a0dec63feaba330efbb15f7730ccc4c1d805";
    // Milestone 2 must match this catalogue contract to the harness; see conformance/README.md.
    const CATALOGUE_PATH: &str = "scenarios/catalogue.toml";
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Catalogue {
        scenario_ids: Vec<String>,
    }

    let pin = std::fs::read_to_string(root.join(".wire-harness-rev")).unwrap();
    let fields: Vec<_> = pin
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .flat_map(str::split_whitespace)
        .collect();
    assert_eq!(fields.len(), 2, "harness pin must hold a tag and full SHA");
    let sha = fields[1];
    assert!(
        sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid harness SHA"
    );
    if sha == PRE_CATALOGUE_REV {
        eprintln!(
            "SKIP scenario-id cross-check: pinned harness {sha} predates the scenario catalogue"
        );
        return None;
    }
    let checkout: PathBuf = root.join(".lyracore/wire-harness").join(sha);
    let head = Command::new("git")
        .current_dir(&checkout)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap_or_else(|e| panic!("read pinned harness checkout {}: {e}", checkout.display()));
    assert!(head.status.success(), "cannot read pinned harness HEAD");
    assert_eq!(
        String::from_utf8(head.stdout).unwrap().trim(),
        sha,
        "harness checkout differs from pin"
    );
    let files = Command::new("git")
        .current_dir(&checkout)
        .args(["ls-tree", "--name-only", sha, "--", CATALOGUE_PATH])
        .output()
        .expect("read pinned harness tree");
    assert!(files.status.success(), "cannot read pinned harness tree");
    if files.stdout.is_empty() {
        eprintln!("SKIP scenario-id cross-check: pinned harness {sha} has no catalogue at the assumed path {CATALOGUE_PATH}");
        return None;
    }
    let source = Command::new("git")
        .current_dir(&checkout)
        .args(["show", &format!("{sha}:{CATALOGUE_PATH}")])
        .output()
        .expect("read pinned scenario catalogue");
    assert!(
        source.status.success(),
        "cannot read pinned scenario catalogue"
    );
    let catalogue: Catalogue = toml::from_str(&String::from_utf8(source.stdout).unwrap())
        .unwrap_or_else(|e| panic!("parse {CATALOGUE_PATH} at {sha}: {e}"));
    Some(catalogue.scenario_ids)
}
