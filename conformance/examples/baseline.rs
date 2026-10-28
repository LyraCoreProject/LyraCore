#[path = "../support/ratchet.rs"]
mod ratchet;

use std::path::Path;

use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use wire_client::{catalogue, compare::Comparison, trace::Sha256Digest};

#[derive(Deserialize)]
struct Tier1Result {
    tier: String,
    core_commit: String,
    harness_revision: String,
    scenarios: Vec<ScenarioResult>,
}

#[derive(Deserialize)]
struct ScenarioResult {
    id: String,
    behavior: String,
    scenario_sha256: Sha256Digest,
    comparison: Comparison,
}

fn baseline_from_result(result: Tier1Result, current_commit: &str) -> Result<ratchet::Baseline> {
    ensure!(result.tier == "tier1", "expected a tier 1 result");
    ensure!(
        result.core_commit == current_commit,
        "tier 1 result is from another commit"
    );
    let pin = include_str!("../../.wire-harness-rev")
        .split_whitespace()
        .last()
        .context("harness pin")?;
    ensure!(
        result.harness_revision == pin,
        "tier 1 result uses another harness revision"
    );
    let mut scenarios = Vec::new();
    for entry in result.scenarios {
        let golden = catalogue::get(&entry.id)?;
        ensure!(
            entry.behavior == golden.scenario().behavior,
            "{} names another Behavior",
            entry.id
        );
        ensure!(
            entry.scenario_sha256 == golden.trace().header.scenario_sha256,
            "{} uses another scenario revision",
            entry.id
        );
        scenarios.push(ratchet::ScenarioBaseline::from_comparison(
            &entry.id,
            &entry.behavior,
            &entry.comparison,
        ));
    }
    scenarios.sort_by(|left, right| left.scenario.cmp(&right.scenario));
    let mut expected: Vec<_> = catalogue::ENTRIES.iter().map(|entry| entry.id).collect();
    expected.sort_unstable();
    let actual: Vec<_> = scenarios
        .iter()
        .map(|entry| entry.scenario.as_str())
        .collect();
    ensure!(
        actual == expected,
        "tier 1 result must contain every catalogue scenario exactly once: {actual:?}"
    );
    Ok(ratchet::Baseline {
        schema_version: 1,
        scenarios,
    })
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    ensure!(
        args.next().as_deref() == Some("regenerate") && args.next().is_none(),
        "usage: baseline regenerate"
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("core root")?;
    let result: Tier1Result = serde_json::from_slice(&std::fs::read(
        root.join(".lyracore/conformance/tier1.json"),
    )?)?;
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()?;
    ensure!(output.status.success(), "git rev-parse HEAD failed");
    let commit = String::from_utf8(output.stdout)?;
    let baseline = baseline_from_result(result, commit.trim())?;
    let path = root.join("conformance/tier1-baseline.json");
    std::fs::write(&path, baseline.canonical_bytes()?)?;
    let saved = ratchet::Baseline::read(&path)?;
    for scenario in &baseline.scenarios {
        saved.check(scenario)?;
    }
    println!("wrote {}", path.display());
    Ok(())
}
