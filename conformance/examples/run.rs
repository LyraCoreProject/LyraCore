#[path = "../support/ratchet.rs"]
mod ratchet;

use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};
use lyracore_conformance::{FeatureLedger, SourceReference};
use serde::{Deserialize, Serialize};
use wire_client::catalogue;
use wire_client::compare::{Comparison, DifferenceClass};
use wire_client::scenario::Stream;
use wire_client::trace::{RecordedField, Sha256Digest};

#[derive(Deserialize)]
struct Tier1Result {
    schema_version: u32,
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

#[derive(Serialize)]
struct RunOutput<'a> {
    schema_version: u32,
    scenario: &'a str,
    behavior: &'a str,
    vmangos: &'a SourceReference,
    conforms: bool,
    baseline: BaselineOutput,
    groups: Vec<Group>,
}

#[derive(Serialize)]
struct BaselineOutput {
    matches: bool,
    regeneration: ratchet::BaselineChanges,
}

#[derive(Serialize)]
struct Group {
    classification: &'static str,
    signature: ratchet::Signature,
    records: Vec<Record>,
}

#[derive(Serialize)]
struct Record {
    packet_index: Option<usize>,
    reason: String,
    expected: Option<RecordedField>,
    actual: Option<RecordedField>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum GroupClass {
    Rule,
    Data,
}

impl GroupClass {
    fn name(self) -> &'static str {
        match self {
            Self::Rule => "rule",
            Self::Data => "data",
        }
    }
}

fn assemble<'a>(
    scenario: &'a str,
    behavior: &'a str,
    vmangos: &'a SourceReference,
    harness_revision: &str,
    scenario_sha256: &Sha256Digest,
    comparison: &Comparison,
    baseline: &ratchet::Baseline,
) -> Result<RunOutput<'a>> {
    let current = ratchet::ScenarioBaseline::from_comparison(
        scenario,
        behavior,
        harness_revision,
        scenario_sha256,
        comparison,
    );
    let regeneration = baseline.changes(&current)?;
    let matches = baseline.check(&current).is_ok();
    let mut grouped = BTreeMap::<(GroupClass, ratchet::Signature), Vec<Record>>::new();
    for difference in &comparison.differences {
        let class = match difference.classification {
            DifferenceClass::Rule => GroupClass::Rule,
            DifferenceClass::Data => GroupClass::Data,
        };
        grouped
            .entry((class, difference.into()))
            .or_default()
            .push(Record {
                packet_index: difference.packet_index,
                reason: difference.reason.clone(),
                expected: difference.expected.clone(),
                actual: difference.actual.clone(),
            });
    }
    let groups = grouped
        .into_iter()
        .map(|((class, signature), records)| Group {
            classification: class.name(),
            signature,
            records,
        })
        .collect();
    Ok(RunOutput {
        schema_version: 1,
        scenario,
        behavior,
        vmangos,
        conforms: comparison.is_match(),
        baseline: BaselineOutput {
            matches,
            regeneration,
        },
        groups,
    })
}

fn stream_text(stream: &Option<Stream>) -> String {
    match stream {
        Some(Stream::Opening) => "opening".into(),
        Some(Stream::Reply(id)) => format!("reply/{id}"),
        Some(Stream::Broadcast(id)) => format!("broadcast/{id}"),
        Some(Stream::Unexpected) => "unexpected".into(),
        Some(Stream::Malformed) => "malformed".into(),
        None => "-".into(),
    }
}

fn signature_text(signature: &ratchet::Signature) -> String {
    let opcode = signature
        .opcode
        .map_or("-".into(), |opcode| format!("0x{opcode:04X}"));
    let path = if signature.field_path.is_empty() {
        "-".into()
    } else {
        signature.field_path.join(".")
    };
    format!(
        "{} / {} / {} / {} / {}",
        signature.rule,
        signature.checkpoint,
        stream_text(&signature.stream),
        opcode,
        path
    )
}

fn text_output(output: &RunOutput<'_>) -> Result<String> {
    let mut text = String::new();
    writeln!(text, "Scenario: {}", output.scenario)?;
    writeln!(text, "Behavior: {}", output.behavior)?;
    writeln!(
        text,
        "vmangos: {}:{} :: {}",
        output.vmangos.commit, output.vmangos.path, output.vmangos.symbol
    )?;
    writeln!(
        text,
        "Golden Trace: {}",
        if output.conforms {
            "matches"
        } else {
            "differs"
        }
    )?;
    writeln!(
        text,
        "Tier 1 baseline: {}",
        if output.baseline.matches {
            "matches"
        } else {
            "differs"
        }
    )?;
    let changes = &output.baseline.regeneration;
    if changes.is_empty() {
        writeln!(text, "Regeneration: no signature changes")?;
    } else {
        writeln!(text, "Regeneration would change:")?;
        for (class, changes) in [("rule", &changes.rule), ("data", &changes.data)] {
            for signature in &changes.added {
                writeln!(text, "  + {class}: {}", signature_text(signature))?;
            }
            for signature in &changes.removed {
                writeln!(text, "  - {class}: {}", signature_text(signature))?;
            }
        }
    }
    for group in &output.groups {
        writeln!(
            text,
            "\n[{}] {} ({} {})",
            group.classification,
            signature_text(&group.signature),
            group.records.len(),
            if group.records.len() == 1 {
                "record"
            } else {
                "records"
            }
        )?;
        for record in &group.records {
            writeln!(text, "  reason: {}", record.reason)?;
            writeln!(
                text,
                "  expected: {}",
                serde_json::to_string(&record.expected)?
            )?;
            writeln!(text, "  actual: {}", serde_json::to_string(&record.actual)?)?;
        }
    }
    Ok(text)
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let scenario = args.next().context("usage: run SCENARIO [--json]")?;
    let json = match args.next().as_deref() {
        None => false,
        Some("--json") => true,
        _ => anyhow::bail!("usage: run SCENARIO [--json]"),
    };
    ensure!(args.next().is_none(), "usage: run SCENARIO [--json]");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("core root")?;
    let golden = catalogue::get(&scenario)?;
    let ledger = FeatureLedger::load(&root.join("conformance/features"))?;
    let behavior = ledger
        .behaviors()
        .iter()
        .find(|behavior| behavior.id == golden.scenario().behavior)
        .context("scenario Behavior is missing from the Feature Ledger")?;
    ensure!(
        behavior.scenarios.contains(&scenario),
        "Behavior does not list scenario {scenario}"
    );
    let path = std::env::var_os("LYRACORE_CONFORMANCE_RESULT")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join(".lyracore/conformance/tier1.json"));
    let result: Tier1Result = serde_json::from_slice(&std::fs::read(&path)?)?;
    ensure!(
        result.schema_version == 2 && result.tier == "tier1",
        "expected schema 2 tier 1 result"
    );
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()?;
    ensure!(output.status.success(), "git rev-parse HEAD failed");
    ensure!(
        result.core_commit == String::from_utf8(output.stdout)?.trim(),
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
    let [entry] = <[ScenarioResult; 1]>::try_from(result.scenarios)
        .map_err(|_| anyhow::anyhow!("expected one scenario result"))?;
    ensure!(
        entry.id == scenario && entry.behavior == behavior.id,
        "tier 1 result names another scenario or Behavior"
    );
    ensure!(
        entry.scenario_sha256 == golden.trace().header.scenario_sha256,
        "tier 1 result uses another scenario revision"
    );
    let baseline = ratchet::Baseline::read(&root.join("conformance/tier1-baseline.json"))?;
    let output = assemble(
        &scenario,
        &behavior.id,
        &behavior.vmangos,
        &result.harness_revision,
        &entry.scenario_sha256,
        &entry.comparison,
        &baseline,
    )?;
    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        print!("{}", text_output(&output)?);
    }
    if !output.baseline.matches {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wire_client::compare::Difference;

    fn fixture() -> (SourceReference, Comparison, ratchet::Baseline) {
        let source = SourceReference {
            commit: "abcdef".into(),
            path: "src/game/Handlers/TaxiHandler.cpp".into(),
            symbol: "WorldSession::HandleActivateTaxiOpcode".into(),
        };
        let comparison = Comparison {
            differences: vec![Difference {
                classification: DifferenceClass::Rule,
                checkpoint: "flight".into(),
                stream: Some(Stream::Reply("flight".into())),
                packet_index: Some(2),
                field_path: vec!["duration".into()],
                opcode: Some(221),
                deviations: vec![],
                rule: "exact_field".into(),
                reason: "field type and value must match".into(),
                expected: Some(RecordedField::Unsigned(100)),
                actual: Some(RecordedField::Unsigned(99)),
            }],
        };
        let baseline = ratchet::Baseline {
            schema_version: 1,
            scenarios: vec![ratchet::ScenarioBaseline::from_comparison(
                "taxi_single_hop",
                "taxi_single_hop",
                "revision",
                &Sha256Digest::of(b"scenario"),
                &comparison,
            )],
        };
        (source, comparison, baseline)
    }

    #[test]
    fn text_groups_values_with_rule_reason_and_source() -> Result<()> {
        let (source, mut comparison, baseline) = fixture();
        comparison
            .differences
            .push(comparison.differences[0].clone());
        let output = assemble(
            "taxi_single_hop",
            "taxi_single_hop",
            &source,
            "revision",
            &Sha256Digest::of(b"scenario"),
            &comparison,
            &baseline,
        )?;
        let text = text_output(&output)?;
        assert!(text.contains("vmangos: abcdef:src/game/Handlers/TaxiHandler.cpp :: WorldSession::HandleActivateTaxiOpcode"));
        assert!(text.contains(
            "[rule] exact_field / flight / reply/flight / 0x00DD / duration (2 records)"
        ));
        assert!(text.contains("reason: field type and value must match"));
        assert!(text.contains("expected: {\"type\":\"unsigned\",\"value\":100}"));
        assert!(text.contains("actual: {\"type\":\"unsigned\",\"value\":99}"));
        assert!(text.contains("Regeneration: no signature changes"));
        Ok(())
    }

    #[test]
    fn json_has_stable_group_and_regeneration_shape() -> Result<()> {
        let (source, comparison, mut baseline) = fixture();
        baseline.scenarios[0].rule.clear();
        let output = assemble(
            "taxi_single_hop",
            "taxi_single_hop",
            &source,
            "revision",
            &Sha256Digest::of(b"scenario"),
            &comparison,
            &baseline,
        )?;
        let value: serde_json::Value =
            serde_json::from_str(&serde_json::to_string_pretty(&output)?)?;
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["scenario"], "taxi_single_hop");
        assert_eq!(value["behavior"], "taxi_single_hop");
        assert_eq!(
            value["vmangos"]["symbol"],
            "WorldSession::HandleActivateTaxiOpcode"
        );
        assert_eq!(value["conforms"], false);
        assert_eq!(value["baseline"]["matches"], false);
        assert_eq!(
            value["baseline"]["regeneration"]["rule"]["added"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            value["baseline"]["regeneration"]["data"]["added"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        assert_eq!(value["groups"].as_array().unwrap().len(), 1);
        assert_eq!(value["groups"][0]["signature"]["rule"], "exact_field");
        assert_eq!(value["groups"][0]["classification"], "rule");
        assert_eq!(value["groups"][0]["records"][0]["expected"]["value"], 100);
        assert_eq!(value["groups"][0]["records"][0]["actual"]["value"], 99);
        assert_eq!(
            value["groups"][0]["records"][0]["reason"],
            "field type and value must match"
        );
        Ok(())
    }

    #[test]
    fn text_names_the_signature_regeneration_would_add() -> Result<()> {
        let (source, comparison, mut baseline) = fixture();
        baseline.scenarios[0].rule.clear();
        let output = assemble(
            "taxi_single_hop",
            "taxi_single_hop",
            &source,
            "revision",
            &Sha256Digest::of(b"scenario"),
            &comparison,
            &baseline,
        )?;
        let text = text_output(&output)?;
        assert!(text.contains("Tier 1 baseline: differs"));
        assert!(text.contains("Regeneration would change:"));
        assert!(text.contains("+ rule: exact_field / flight / reply/flight / 0x00DD / duration"));
        Ok(())
    }
}
