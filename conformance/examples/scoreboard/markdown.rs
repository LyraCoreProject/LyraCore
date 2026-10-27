use std::fmt::Write;

use super::{Report, Summary};
use anyhow::{Context, Result};

pub fn markdown(report: &Report, date: &str) -> Result<String> {
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
        if audit.previous_report.is_some() && result.changes.is_none() {
            text.push_str("Comparison is not possible: the harness revision or scenario hashes differ from the previous report.\n\n");
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
    writeln!(text, "Reproduce with `./conformance/audit --host /path/to/spacetimedb-standalone --date {date}` after preparing the pinned Reference Server. See [the audit command](../README.md#conformance-audit). The command stops both local servers on exit. Scrubbed LyraCore candidates are in [{date}.traces]({date}.traces/). This report is rendered from [{date}.json]({date}.json); Reference Server candidates and credentials stay in scratch. No real-client check was performed.")?;
    Ok(text)
}
