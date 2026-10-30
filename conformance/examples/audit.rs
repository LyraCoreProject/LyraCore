use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};
use lyracore_conformance::FeatureLedger;
use serde::{Deserialize, Serialize};
use wire_client::{
    catalogue, compare,
    scenario::FixtureVerb,
    trace::{Origin, Provenance, Sha256Digest, Trace},
};

#[path = "scoreboard/evidence.rs"]
mod evidence;
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
    content: String,
    reference: Provenance,
}

#[derive(Serialize)]
struct AuditBaseline<'a> {
    schema_version: u32,
    scenarios: Vec<AuditScenarioBaseline<'a>>,
}

#[derive(Serialize)]
struct AuditScenarioBaseline<'a> {
    scenario: &'a str,
    behavior: &'a str,
    harness_revision: &'a str,
    scenario_sha256: &'a Sha256Digest,
    rule: Vec<evidence::Signature>,
    data: Vec<evidence::Signature>,
}

fn export(directory: &Path) -> Result<()> {
    std::fs::create_dir_all(directory)?;
    let mut scenarios = Vec::new();
    for entry in catalogue::ENTRIES {
        let golden = catalogue::get(entry.id)?;
        let fixtures = if golden.scenario().schema_version == 4 {
            golden
                .scenario()
                .characters
                .iter()
                .map(|character| &character.fixtures)
                .collect::<Vec<_>>()
        } else {
            vec![&golden.scenario().fixtures]
        };
        let characters = fixtures
            .into_iter()
            .map(|fixtures| match &fixtures[0] {
                FixtureVerb::Character { name, .. } => Ok(name.clone()),
                _ => bail!("Character fixture required"),
            })
            .collect::<Result<Vec<_>>>()?;
        std::fs::write(directory.join(format!("{}.toml", entry.id)), entry.scenario)?;
        scenarios.push(serde_json::json!({"id": entry.id, "characters": characters}));
    }
    std::fs::write(
        directory.join("catalogue.json"),
        serde_json::to_vec_pretty(&scenarios)?,
    )?;
    Ok(())
}

fn check(directory: &Path, scenario: &str) -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let golden = catalogue::get(scenario)?;
    let (_, committed) = evidence::latest_audit(&root.join("reports"), None)?
        .context("no committed full Conformance Audit report")?;
    let previous = committed
        .scenarios
        .iter()
        .find(|entry| entry.id == scenario)
        .filter(|entry| {
            entry.scenario_sha256 == golden.trace().header.scenario_sha256
                && entry.behavior == golden.scenario().behavior
        });
    let Origin::Golden { provenance } = &golden.trace().header.origin else {
        bail!("expected Golden Trace");
    };
    let candidate = |adapter: &str| -> Result<_> {
        let bytes = std::fs::read(directory.join(format!("{scenario}-{adapter}.json")))?;
        let trace = Trace::parse(&bytes)?;
        ensure!(
            matches!(&trace.header.origin, Origin::Candidate { adapter: recorded } if recorded == adapter),
            "wrong {adapter} candidate origin"
        );
        compare::compare(&golden, &trace, &[])
    };
    let comparison = candidate("lyracore")?;
    let reference = candidate("vmangos")?;
    let head = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()?;
    ensure!(head.status.success(), "git rev-parse HEAD failed");
    let core_commit = String::from_utf8(head.stdout)?.trim().to_owned();
    let result = serde_json::json!({
        "schema_version": 2,
        "tier": "tier1",
        "core_commit": core_commit,
        "harness_revision": evidence::harness_revision(),
        "scenarios": [{
            "id": scenario,
            "behavior": golden.scenario().behavior.clone(),
            "scenario_sha256": golden.trace().header.scenario_sha256.clone(),
            "comparison": comparison,
            "properties": [],
        }],
    });
    std::fs::write(directory.join("result.json"), serde_json::to_vec(&result)?)?;
    std::fs::write(
        directory.join("reference-comparison.json"),
        serde_json::to_vec_pretty(&reference)?,
    )?;
    ensure!(
        reference.is_match(),
        "Reference Server differs from its Golden Trace:\n{}",
        serde_json::to_string_pretty(&reference)?
    );
    let runtime: Provenance =
        serde_json::from_slice(&std::fs::read(directory.join("reference.json"))?)?;
    ensure!(
        &runtime == provenance,
        "Reference Server provenance differs from the Golden Trace"
    );
    let summary = previous.map_or_else(evidence::Summary::default, |entry| {
        evidence::Summary::collect(entry.comparison.differences.iter())
    });
    let unrecorded = Sha256Digest::of(b"not in the latest full audit");
    let baseline = AuditBaseline {
        schema_version: 1,
        scenarios: vec![AuditScenarioBaseline {
            scenario,
            behavior: previous.map_or(golden.scenario().behavior.as_str(), |entry| &entry.behavior),
            harness_revision: &committed.harness_revision,
            scenario_sha256: previous.map_or(&unrecorded, |entry| &entry.scenario_sha256),
            rule: summary
                .rule
                .signatures
                .into_iter()
                .map(|item| item.signature)
                .collect(),
            data: summary
                .data
                .signatures
                .into_iter()
                .map(|item| item.signature)
                .collect(),
        }],
    };
    let mut bytes = serde_json::to_vec_pretty(&baseline)?;
    bytes.push(b'\n');
    std::fs::write(directory.join("baseline.json"), bytes)?;
    Ok(())
}

fn compare_run(
    directory: &Path,
    ledger: &FeatureLedger,
    run: Run,
    previous: Option<(&str, &Report)>,
) -> Result<Report> {
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
            properties: None,
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
        let changes = previous.and_then(|(_, old)| report.changes_since(old, &scenario.behavior));
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
        native_host_sha256: run.native_host_sha256,
        module_sha256: run.module_sha256,
        content: run.content,
        reference: run.reference,
        previous_report: previous.map(|(name, _)| name.to_owned()),
        behaviors,
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

fn write_report(directory: &Path, output: &Path, date: &str) -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ledger = FeatureLedger::load(&root.join("features"))?;
    let run: Run = serde_json::from_slice(&std::fs::read(directory.join("run.json"))?)?;
    let history = root.join("reports");
    let previous_name = evidence::expected_predecessor(&history, run.recorded_at_unix_ms)?;
    let previous = previous_name
        .as_ref()
        .map(|name| Report::load_structure(&history.join(name), "conformance_audit"))
        .transpose()?;
    let report = compare_run(
        directory,
        &ledger,
        run,
        previous_name.as_deref().zip(previous.as_ref()),
    )?;
    let head = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()?;
    ensure!(head.status.success(), "git rev-parse failed");
    ensure!(
        evidence::same_core(
            root.parent().context("core root")?,
            &report.core_commit,
            String::from_utf8(head.stdout)?.trim()
        )?,
        "core changed after capture"
    );
    let json = output.join(format!("{date}.json"));
    let md = output.join(format!("{date}.md"));
    report.validate_changes()?;
    std::fs::create_dir_all(output)?;
    ensure!(
        !json.exists() && !md.exists(),
        "report already exists; use a new date or suffix"
    );
    let rendered = evidence::markdown(&report, date)?;
    let traces = json.with_extension("traces");
    ensure!(!traces.exists(), "candidate directory already exists");
    std::fs::create_dir(&traces)?;
    for entry in catalogue::ENTRIES {
        let name = format!("{}-lyracore.json", entry.id);
        std::fs::copy(directory.join(&name), traces.join(name))?;
    }
    std::fs::write(&json, serde_json::to_vec_pretty(&report)?)?;
    std::fs::write(&md, &rendered)?;
    Report::load(&json, "conformance_audit", &ledger)?.verify_capture(&json)?;
    println!("{rendered}");
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [mode, directory] if mode == "export" => export(Path::new(directory)),
        [mode, directory, scenario] if mode == "check" => check(Path::new(directory), scenario),
        [mode, directory, output, date] if mode == "report" => write_report(Path::new(directory), Path::new(output), date),
        _ => bail!("audit export DIRECTORY | check DIRECTORY SCENARIO | report DIRECTORY OUTPUT_DIRECTORY DATE; use conformance/audit"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use evidence::Summary;

    #[test]
    fn scenario_check_uses_committed_audit_signatures() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let directory = scratch::Scratch::new("scenario-check-test")?;
        let (path, report) =
            evidence::latest_audit(&root.join("reports"), None)?.context("committed audit")?;
        let scenario = "taxi_single_hop";
        let golden = catalogue::get(scenario)?;
        let Origin::Golden { provenance } = &golden.trace().header.origin else {
            bail!("Golden Trace");
        };
        std::fs::write(
            directory.join("reference.json"),
            serde_json::to_vec(provenance)?,
        )?;
        let traces = path.with_extension("traces");
        std::fs::copy(
            traces.join(format!("{scenario}-lyracore.json")),
            directory.join(format!("{scenario}-lyracore.json")),
        )?;
        let mut reference = serde_json::to_value(golden.trace())?;
        reference["header"]["origin"] =
            serde_json::json!({"kind": "candidate", "adapter": "vmangos"});
        std::fs::write(
            directory.join(format!("{scenario}-vmangos.json")),
            serde_json::to_vec(&reference)?,
        )?;
        check(&directory, scenario)?;
        let result: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.join("result.json"))?)?;
        let baseline: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.join("baseline.json"))?)?;
        assert_eq!(result["scenarios"][0]["id"], scenario);
        assert_eq!(result["scenarios"][0]["properties"], serde_json::json!([]));
        assert_eq!(
            baseline["scenarios"][0]["harness_revision"],
            report.harness_revision
        );
        assert_eq!(
            baseline["scenarios"][0]["scenario_sha256"],
            String::from(golden.trace().header.scenario_sha256.clone())
        );
        Ok(())
    }

    #[test]
    fn reference_drift_retains_both_comparisons_with_scrubbed_values() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let directory = scratch::Scratch::new("reference-drift")?;
        let scenario = "taxi_single_hop";
        let (path, _) =
            evidence::latest_audit(&root.join("reports"), None)?.context("committed audit")?;
        let bytes = std::fs::read(
            path.with_extension("traces")
                .join(format!("{scenario}-lyracore.json")),
        )?;
        std::fs::write(directory.join(format!("{scenario}-lyracore.json")), &bytes)?;
        let mut reference: serde_json::Value = serde_json::from_slice(&bytes)?;
        reference["header"]["origin"] =
            serde_json::json!({"kind": "candidate", "adapter": "vmangos"});
        std::fs::write(
            directory.join(format!("{scenario}-vmangos.json")),
            serde_json::to_vec(&reference)?,
        )?;
        let error = check(&directory, scenario).unwrap_err().to_string();
        assert!(error.contains("Reference Server differs"));
        for field in ["rule", "checkpoint", "field_path", "expected", "actual"] {
            assert!(error.contains(field), "missing {field}: {error}");
        }
        let candidate: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.join("result.json"))?)?;
        let reference: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.join("reference-comparison.json"))?)?;
        assert_eq!(candidate["scenarios"][0]["comparison"], reference);
        assert!(!directory.join("baseline.json").exists());
        Ok(())
    }

    #[test]
    fn audit_requires_both_candidates_for_every_catalogue_scenario() -> Result<()> {
        let directory = scratch::Scratch::new("audit-test")?;
        export(&directory)?;
        let exported: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.join("catalogue.json"))?)?;
        let visibility = exported
            .as_array()
            .context("exported catalogue")?
            .iter()
            .find(|entry| entry["id"] == "two_character_visibility")
            .context("named Character scenario")?;
        assert_eq!(
            visibility["characters"],
            serde_json::json!(["Watchera", "Arrivera"])
        );
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
                "native_host_sha256": "a".repeat(64), "module_sha256": "b".repeat(64),
                "content": "imported vmangos world", "reference": provenance,
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
        let load_run = || -> Result<Run> {
            Ok(serde_json::from_slice(&std::fs::read(
                directory.join("run.json"),
            )?)?)
        };
        let report = compare_run(&directory, &ledger, load_run()?, None)?;
        assert_eq!(report.scenarios.len(), catalogue::ENTRIES.len());
        for result in report.audit.as_ref().unwrap().behaviors.values() {
            assert_eq!(result.candidate, Summary::default());
            assert_eq!(result.reference, Summary::default());
            assert!(result.changes.is_none());
        }
        let repeated = compare_run(
            &directory,
            &ledger,
            load_run()?,
            Some(("previous.json", &report)),
        )?;
        for result in repeated.audit.as_ref().unwrap().behaviors.values() {
            let changes = result.changes.as_ref().unwrap();
            assert!(changes.rule.fixed.is_empty() && changes.rule.regressed.is_empty());
            assert!(changes.data.fixed.is_empty() && changes.data.regressed.is_empty());
        }
        std::fs::remove_file(directory.join("taxi_single_hop-vmangos.json"))?;
        assert!(compare_run(&directory, &ledger, load_run()?, None).is_err());
        Ok(())
    }

    #[test]
    fn new_report_links_committed_c5_audit() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let committed = Report::load_structure(
            &root.join("reports/2026-10-28-c5.json"),
            "conformance_audit",
        )?;
        let audit = committed.audit.as_ref().context("audit")?;
        let directory = scratch::Scratch::new("new-audit-test")?;
        let input = directory.join("input");
        let output = directory.join("output");
        std::fs::create_dir(&input)?;
        let head = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(root)
            .output()?;
        ensure!(head.status.success(), "git rev-parse failed");
        let core_commit = String::from_utf8(head.stdout)?.trim().to_owned();
        std::fs::write(
            input.join("run.json"),
            serde_json::to_vec(&serde_json::json!({
                "core_commit": core_commit,
                "working_tree_dirty": false,
                "harness_revision": evidence::harness_revision(),
                "recorded_at_unix_ms": committed.recorded_at_unix_ms + 1,
                "native_host_sha256": audit.native_host_sha256,
                "module_sha256": audit.module_sha256,
                "content": audit.content,
                "reference": audit.reference,
            }))?,
        )?;
        for entry in catalogue::ENTRIES {
            let name = format!("{}-lyracore.json", entry.id);
            let recorded = root.join("reports/2026-10-28-c5.traces").join(&name);
            if recorded.exists() {
                std::fs::copy(recorded, input.join(name))?;
            } else {
                let mut candidate: serde_json::Value = serde_json::from_str(entry.trace)?;
                candidate["header"]["origin"] =
                    serde_json::json!({"kind": "candidate", "adapter": "lyracore"});
                std::fs::write(input.join(name), serde_json::to_vec(&candidate)?)?;
            }
            let mut reference: serde_json::Value = serde_json::from_str(entry.trace)?;
            reference["header"]["origin"] =
                serde_json::json!({"kind": "candidate", "adapter": "vmangos"});
            std::fs::write(
                input.join(format!("{}-vmangos.json", entry.id)),
                serde_json::to_vec(&reference)?,
            )?;
        }
        write_report(&input, &output, "2026-10-28-c6")?;
        let path = output.join("2026-10-28-c6.json");
        let ledger = FeatureLedger::load(&root.join("features"))?;
        let report = Report::load(&path, "conformance_audit", &ledger)?;
        assert_eq!(
            report
                .audit
                .as_ref()
                .context("audit")?
                .previous_report
                .as_deref(),
            Some("2026-10-28-c5.json")
        );
        report.validate_changes()?;
        report.verify_capture(&path)
    }
}
