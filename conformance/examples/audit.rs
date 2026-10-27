use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use lyracore_conformance::FeatureLedger;
use serde::Deserialize;
use wire_client::{
    catalogue, compare,
    scenario::FixtureVerb,
    trace::{Origin, Provenance, Sha256Digest, Trace},
};

#[path = "scoreboard/evidence.rs"]
mod evidence;
#[path = "../support/taxi_route.rs"]
mod taxi_route;
use evidence::{Audit, BehaviorAudit, CandidateCapture, Report, ScenarioResult};

#[cfg(test)]
#[path = "../support/scratch.rs"]
mod scratch;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Run {
    core_commit: String,
    working_tree_dirty: bool,
    harness_revision: String,
    recorded_at_unix_ms: u64,
    native_host_sha256: Sha256Digest,
    module_sha256: Sha256Digest,
    reference: Provenance,
}

fn export(directory: &Path) -> Result<()> {
    std::fs::create_dir_all(directory)?;
    let mut scenarios = Vec::new();
    for entry in catalogue::ENTRIES {
        let golden = catalogue::get(entry.id)?;
        let FixtureVerb::Character { name, .. } = &golden.scenario().fixtures[0] else {
            bail!("Character fixture required");
        };
        std::fs::write(directory.join(format!("{}.toml", entry.id)), entry.scenario)?;
        scenarios.push(serde_json::json!({"id": entry.id, "character": name}));
    }
    std::fs::write(
        directory.join("catalogue.json"),
        serde_json::to_vec_pretty(&scenarios)?,
    )?;
    std::fs::write(
        directory.join("taxi-fixture.json"),
        serde_json::to_vec(&taxi_route::statements())?,
    )?;
    Ok(())
}

fn compare_run(
    directory: &Path,
    ledger: &FeatureLedger,
    previous: Option<&Report>,
) -> Result<Report> {
    let run: Run = serde_json::from_slice(&std::fs::read(directory.join("run.json"))?)?;
    let pin = include_str!("../../.wire-harness-rev")
        .lines()
        .find(|line| !line.starts_with('#'))
        .context("harness pin")?
        .split_whitespace()
        .nth(1)
        .context("harness SHA")?;
    ensure!(
        run.harness_revision == pin,
        "run used another harness revision"
    );
    let mut report = Report {
        schema_version: 2,
        tier: "conformance_audit".into(),
        core_commit: run.core_commit,
        working_tree_dirty: run.working_tree_dirty,
        harness_revision: run.harness_revision,
        recorded_at_unix_ms: run.recorded_at_unix_ms,
        scenarios: Vec::new(),
        audit: None,
    };
    for entry in catalogue::ENTRIES {
        let golden = catalogue::get(entry.id)?;
        let compare = |adapter: &str| -> Result<_> {
            let bytes = std::fs::read(directory.join(format!("{}-{adapter}.json", entry.id)))?;
            let candidate = Trace::parse(&bytes)?;
            ensure!(
                matches!(&candidate.header.origin, Origin::Candidate { adapter: recorded } if recorded == adapter),
                "wrong candidate origin"
            );
            Ok((
                compare::compare(&golden, &candidate, &[])?,
                Sha256Digest::of(&bytes),
            ))
        };
        let Origin::Golden { provenance } = &golden.trace().header.origin else {
            bail!("expected Golden Trace");
        };
        ensure!(
            provenance == &run.reference,
            "Reference Server provenance differs from the Golden Trace"
        );
        let (comparison, candidate_sha256) = compare("lyracore")?;
        let (reference_comparison, reference_sha256) = compare("vmangos")?;
        report.scenarios.push(ScenarioResult {
            id: entry.id.into(),
            behavior: golden.scenario().behavior.clone(),
            scenario_sha256: golden.trace().header.scenario_sha256.clone(),
            comparison,
            reference_comparison: Some(reference_comparison),
            capture: Some(CandidateCapture {
                sha256: candidate_sha256,
                core_commit: report.core_commit.clone(),
                native_host_sha256: run.native_host_sha256.clone(),
                module_sha256: run.module_sha256.clone(),
            }),
            reference_sha256: Some(reference_sha256),
        });
    }
    let mut behaviors = BTreeMap::new();
    for scenario in &report.scenarios {
        let candidate = report.summary(&scenario.behavior, false);
        let changes = previous.and_then(|old| report.changes_since(old, &scenario.behavior));
        behaviors.insert(
            scenario.behavior.clone(),
            BehaviorAudit {
                candidate,
                reference: report.summary(&scenario.behavior, true),
                changes,
            },
        );
    }
    report.audit = Some(Audit {
        native_host_sha256: run.native_host_sha256, module_sha256: run.module_sha256,
        content: "Module seed content and the hand-written taxi route fixture, nodes 2 and 4, path 6, 28 vertices and flight master 352; no world import".into(),
        reference: run.reference, previous_report: previous.map(|old| old.recorded_at_unix_ms.to_string()), behaviors,
    });
    report.validate("conformance_audit", ledger)?;
    for behavior in ledger
        .behaviors()
        .iter()
        .filter(|behavior| !behavior.scenarios.is_empty())
    {
        let state = report.state(behavior, &report.core_commit, &report.harness_revision)?;
        eprintln!(
            "{}: {}; conforming {}",
            behavior.id,
            state,
            state.is_match()
        );
    }

    Ok(report)
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [mode, directory] if mode == "export" => export(Path::new(directory)),
        [mode, directory, output, date] if mode == "report" => {
            let root = Path::new(env!("CARGO_MANIFEST_DIR"));
            let ledger = FeatureLedger::load(&root.join("features"))?;
            let previous = evidence::latest_audit(&root.join("reports"))?
                .filter(|(path, _)| path.file_stem().and_then(|s| s.to_str()) != Some(date));
            let mut report = compare_run(Path::new(directory), &ledger, previous.as_ref().map(|(_, report)| report))?;
            if let Some((path, _)) = &previous {
                report.audit.as_mut().unwrap().previous_report = Some(path.file_name().context("report filename")?.to_string_lossy().into());
            }
            report.validate_changes(&root.join("reports"))?;
            let head = std::process::Command::new("git").args(["rev-parse", "HEAD"]).current_dir(root).output()?;
            ensure!(head.status.success(), "git rev-parse failed");
            ensure!(evidence::same_core(root.parent().context("core root")?, &report.core_commit,
                String::from_utf8(head.stdout)?.trim())?, "core changed after capture");
            let output = PathBuf::from(output);
            std::fs::create_dir_all(&output)?;
            let json = output.join(format!("{date}.json"));
            let md = output.join(format!("{date}.md"));
            ensure!(!json.exists() && !md.exists(), "report already exists; use a new date or archive the old report");
            let rendered = evidence::markdown(&report, date)?;
            let traces = json.with_extension("traces");
            ensure!(!traces.exists(), "candidate directory already exists");
            std::fs::create_dir(&traces)?;
            for entry in catalogue::ENTRIES {
                let name = format!("{}-lyracore.json", entry.id);
                std::fs::copy(Path::new(directory).join(&name), traces.join(name))?;
            }
            std::fs::write(&json, serde_json::to_vec_pretty(&report)?)?;
            std::fs::write(&md, &rendered)?;
            Report::load(&json, "conformance_audit", &ledger)?.verify_capture(&json)?;
            println!("{rendered}");
            Ok(())
        }
        _ => bail!("audit export DIRECTORY | report DIRECTORY OUTPUT_DIRECTORY DATE; use conformance/audit for a full run"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use evidence::Summary;

    #[test]
    fn audit_requires_both_candidates_for_every_catalogue_scenario() -> Result<()> {
        let directory = scratch::Scratch::new("audit-test")?;
        export(&directory)?;
        let ledger = FeatureLedger::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("features"))?;
        let golden = catalogue::get(catalogue::ENTRIES[0].id)?;
        let Origin::Golden { provenance } = &golden.trace().header.origin else {
            bail!("Golden Trace");
        };
        let revision = include_str!("../../.wire-harness-rev")
            .split_whitespace()
            .last()
            .context("revision")?;
        std::fs::write(
            directory.join("run.json"),
            serde_json::to_vec(&serde_json::json!({
                "core_commit": "1".repeat(40), "working_tree_dirty": false,
                "harness_revision": revision, "recorded_at_unix_ms": 2,
                "native_host_sha256": "a".repeat(64), "module_sha256": "b".repeat(64), "reference": provenance,
            }))?,
        )?;
        for entry in catalogue::ENTRIES {
            for adapter in ["lyracore", "vmangos"] {
                let mut candidate: serde_json::Value = serde_json::from_str(entry.trace)?;
                candidate["header"]["origin"] =
                    serde_json::json!({"kind": "candidate", "adapter": adapter});
                std::fs::write(
                    directory.join(format!("{}-{adapter}.json", entry.id)),
                    serde_json::to_vec(&candidate)?,
                )?;
            }
        }
        let report = compare_run(&directory, &ledger, None)?;
        assert_eq!(report.scenarios.len(), catalogue::ENTRIES.len());
        for result in report.audit.as_ref().unwrap().behaviors.values() {
            assert_eq!(result.candidate, Summary::default());
            assert_eq!(result.reference, Summary::default());
            assert!(result.changes.is_none());
        }
        let repeated = compare_run(&directory, &ledger, Some(&report))?;
        for result in repeated.audit.as_ref().unwrap().behaviors.values() {
            let changes = result.changes.as_ref().unwrap();
            assert!(changes.rule.fixed.is_empty() && changes.rule.regressed.is_empty());
            assert!(changes.data.fixed.is_empty() && changes.data.regressed.is_empty());
        }
        std::fs::remove_file(directory.join("taxi_single_hop-vmangos.json"))?;
        assert!(compare_run(&directory, &ledger, None).is_err());
        Ok(())
    }
}
