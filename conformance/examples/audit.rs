use std::collections::BTreeMap;
use std::fmt::Write;
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
use evidence::{Audit, BehaviorAudit, Changes, Report, ScenarioResult, Summary};

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
            let candidate = Trace::parse(&std::fs::read(
                directory.join(format!("{}-{adapter}.json", entry.id)),
            )?)?;
            ensure!(
                matches!(&candidate.header.origin, Origin::Candidate { adapter: recorded } if recorded == adapter),
                "wrong candidate origin"
            );
            compare::compare(&golden, &candidate, &[])
        };
        let Origin::Golden { provenance } = &golden.trace().header.origin else {
            bail!("expected Golden Trace");
        };
        ensure!(
            provenance == &run.reference,
            "Reference Server provenance differs from the Golden Trace"
        );
        report.scenarios.push(ScenarioResult {
            id: entry.id.into(),
            behavior: golden.scenario().behavior.clone(),
            scenario_sha256: golden.trace().header.scenario_sha256.clone(),
            comparison: compare("lyracore")?,
            reference_comparison: Some(compare("vmangos")?),
        });
    }
    let mut behaviors = BTreeMap::new();
    for scenario in &report.scenarios {
        let candidate = report.summary(&scenario.behavior, false);
        let changes = previous
            .map(|old| Changes::between(&old.summary(&scenario.behavior, false), &candidate));
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

fn markdown(report: &Report, date: &str) -> Result<String> {
    let audit = report.audit.as_ref().context("audit details")?;
    let mut text = format!("# Conformance Audit, {date}\n\nEvery scenario in the pinned catalogue ran over TCP on a fresh local Native Host and on vmangos. Both candidates were compared with their Golden Traces.\n\nCore `{}`, dirty tree `{}`. Harness `{}`. Capture time `{}` Unix ms.\n\nContent: {}.\n\nNative Host SHA-256 `{}`. Module SHA-256 `{}`.\n\nReference Server `{}`, binary `{}`, archive SHA-256 `{}`, source `{}`, database `{}`. Its provenance matches both Golden Traces.\n\n",
        report.core_commit, report.working_tree_dirty, report.harness_revision, report.recorded_at_unix_ms,
        audit.content, String::from(audit.native_host_sha256.clone()), String::from(audit.module_sha256.clone()),
        audit.reference.server, audit.reference.binary_release, String::from(audit.reference.binary_sha256.clone()),
        audit.reference.source_commit, audit.reference.database_commit);
    text.push_str("| Behavior | LyraCore rule / data signatures | LyraCore rule / data records | Reference rule / data signatures | Reference rule / data records |\n| --- | ---: | ---: | ---: | ---: |\n");
    for (behavior, result) in &audit.behaviors {
        writeln!(
            text,
            "| {behavior} | {} / {} | {} / {} | {} / {} | {} / {} |",
            result.candidate.rule.signatures.len(),
            result.candidate.data.signatures.len(),
            result.candidate.rule.raw_records,
            result.candidate.data.raw_records,
            result.reference.rule.signatures.len(),
            result.reference.data.signatures.len(),
            result.reference.rule.raw_records,
            result.reference.data.raw_records
        )?;
    }
    let stable = audit
        .behaviors
        .values()
        .all(|result| result.reference == Summary::default());
    writeln!(text, "\nReference stability: {}. Data differences do not fail conformance. Taxi gameplay and its durable CI skip are unchanged. No Deliberate Deviation was added.\n",
        if stable { "both comparisons have zero rule and data records" } else { "the Reference Server differs; this audit cannot establish a stable reference" })?;
    if let Some(previous) = &audit.previous_report {
        writeln!(text, "Previous report: `{previous}`. Fixed and regressed sets compare signatures, excluding record multiplicity, packet indices and values.\n")?;
    } else {
        text.push_str("This is the first machine-readable Conformance Audit. Fixed and regressed signatures are not established. The earlier T2d and T2g notes used different harness revisions and are not audit baselines.\n\n");
    }
    for (behavior, result) in &audit.behaviors {
        writeln!(text, "## {behavior}\n")?;
        for (server, summary) in [
            ("LyraCore", &result.candidate),
            ("Reference Server", &result.reference),
        ] {
            writeln!(text, "{server}: {summary}.\n")?;
            if summary == &Summary::default() {
                continue;
            }
            text.push_str("| Class | Rule | Checkpoint | Stream | Opcode | Field path | Raw records |\n| --- | --- | --- | --- | --- | --- | ---: |\n");
            for (class, counts) in [("rule", &summary.rule), ("data", &summary.data)] {
                for entry in &counts.signatures {
                    let signature = &entry.signature;
                    writeln!(
                        text,
                        "| {class} | `{}` | `{}` | `{}` | {} | `{}` | {} |",
                        signature.rule,
                        signature.checkpoint,
                        serde_json::to_string(&signature.stream)?,
                        signature
                            .opcode
                            .map(|opcode| format!("0x{opcode:04X}"))
                            .unwrap_or_else(|| "none".into()),
                        signature.field_path.join("."),
                        entry.raw_records
                    )?;
                }
            }
            text.push('\n');
        }
        if let Some(changes) = &result.changes {
            for (class, changed) in [("rule", &changes.rule), ("data", &changes.data)] {
                writeln!(
                    text,
                    "{class}: {} fixed, {} regressed signatures.\n",
                    changed.fixed.len(),
                    changed.regressed.len()
                )?;
                for (state, signatures) in
                    [("fixed", &changed.fixed), ("regressed", &changed.regressed)]
                {
                    for signature in signatures {
                        writeln!(
                            text,
                            "- {class} {state}: `{}`",
                            serde_json::to_string(signature)?
                        )?;
                    }
                }
                text.push('\n');
            }
        }
    }
    writeln!(text, "Reproduce with `./conformance/audit --host /path/to/spacetimedb-standalone --date {date}` after preparing the pinned Reference Server. See [the audit command](../README.md#conformance-audit). The command stops both local servers on exit. Candidate traces and credentials stay in scratch; this report and [{date}.json]({date}.json) contain only provenance, counts, signatures and scrubbed difference values. No real-client check was performed.")?;
    Ok(text)
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [mode, directory] if mode == "export" => export(Path::new(directory)),
        [mode, directory, output, date] if mode == "report" => {
            let root = Path::new(env!("CARGO_MANIFEST_DIR"));
            let ledger = FeatureLedger::load(&root.join("features"))?;
            let previous = evidence::latest_audit(&root.join("reports"), &ledger)?;
            let mut report = compare_run(Path::new(directory), &ledger, previous.as_ref().map(|(_, report)| report))?;
            if let Some((path, _)) = &previous {
                report.audit.as_mut().unwrap().previous_report = Some(path.file_name().context("report filename")?.to_string_lossy().into());
            }
            report.validate_changes(&root.join("reports"), &ledger)?;
            let head = std::process::Command::new("git").args(["rev-parse", "HEAD"]).current_dir(root).output()?;
            ensure!(head.status.success(), "git rev-parse failed");
            ensure!(evidence::same_core(root.parent().context("core root")?, &report.core_commit,
                String::from_utf8(head.stdout)?.trim())?, "core changed after capture");
            let output = PathBuf::from(output);
            std::fs::create_dir_all(&output)?;
            let json = output.join(format!("{date}.json"));
            let md = output.join(format!("{date}.md"));
            ensure!(!json.exists() && !md.exists(), "report already exists; use a new date or archive the old report");
            let rendered = markdown(&report, date)?;
            std::fs::write(&json, serde_json::to_vec_pretty(&report)?)?;
            std::fs::write(&md, &rendered)?;
            println!("{rendered}");
            Ok(())
        }
        _ => bail!("audit export DIRECTORY | report DIRECTORY OUTPUT_DIRECTORY DATE; use conformance/audit for a full run"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn audit_requires_both_candidates_for_every_catalogue_scenario() -> Result<()> {
        let directory = PathBuf::from(std::env::var_os("HOME").context("HOME")?)
            .join(".cache/lyracore-scratch/conformance")
            .join(format!(
                "audit-test-{}-{}",
                std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
            ));
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
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }
}
