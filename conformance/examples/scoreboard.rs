use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, ensure, Context, Result};
use lyracore_conformance::{Behavior, FeatureLedger, RealClient};

#[path = "scoreboard/counts.rs"]
mod counts;
#[path = "scoreboard/evidence.rs"]
mod evidence;
use evidence::{Report, State};

fn requests(behaviors: &[&Behavior]) -> Result<BTreeSet<u16>> {
    behaviors
        .iter()
        .flat_map(|behavior| &behavior.requests)
        .map(|opcode| u16::from_str_radix(&opcode[2..], 16).map_err(Into::into))
        .collect()
}

fn state(
    report: Option<&Report>,
    behavior: &Behavior,
    head: &str,
    revision: &str,
) -> Result<State> {
    match report {
        Some(report) => report.state(behavior, head, revision),
        None if behavior.scenarios.is_empty() => Ok(State::NoScenario),
        None => Ok(State::NotRun),
    }
}

fn provenance(path: &Path, report: &Report) {
    println!(
        "{}: {}, core {}, harness {}, captured {}{}",
        report.tier,
        path.display(),
        report.core_commit,
        report.harness_revision,
        report.recorded_at_unix_ms,
        if report.working_tree_dirty {
            ", uncommitted changes (*)"
        } else {
            ""
        }
    );
}

fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("Core root")?;
    let mut tier1_path = root.join(".lyracore/conformance/tier1.json");
    let mut explicit_tier1 = false;
    let mut audit_path = None;
    let mut arguments = std::env::args().skip(1);
    while let Some(option) = arguments.next() {
        match option.as_str() {
            "--tier1" => {
                tier1_path = arguments.next().context("--tier1 needs a path")?.into();
                explicit_tier1 = true;
            }
            "--audit" => {
                audit_path = Some(PathBuf::from(
                    arguments.next().context("--audit needs a path")?,
                ));
            }
            "--help" => {
                println!("scoreboard [--tier1 RESULT.json] [--audit REPORT.json]");
                return Ok(());
            }
            _ => bail!("unknown option {option}"),
        }
    }
    let output = Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "HEAD"])
        .output()?;
    ensure!(output.status.success(), "git rev-parse HEAD failed");
    let head = String::from_utf8(output.stdout)?;
    let head = head.trim();
    let pin = std::fs::read_to_string(root.join(".wire-harness-rev"))?;
    let revision = pin
        .lines()
        .filter(|line| !line.starts_with('#'))
        .flat_map(str::split_whitespace)
        .nth(1)
        .context("harness SHA")?;
    let ledger = FeatureLedger::load(&root.join("conformance/features"))?;
    let catalogue = wire_client::catalogue::ENTRIES
        .iter()
        .map(|entry| wire_client::catalogue::get(entry.id))
        .collect::<Result<Vec<_>>>()?;
    ledger.validate_scenarios(catalogue.iter().map(|golden| {
        (
            golden.scenario().id.as_str(),
            golden.scenario().behavior.as_str(),
        )
    }))?;
    let counts = counts::read(root)?;
    let tier1 = if explicit_tier1 || tier1_path.exists() {
        Some(Report::load(&tier1_path, "tier1")?)
    } else {
        None
    };
    let audit = match audit_path {
        Some(path) => {
            let report = Report::load(&path, "conformance_audit")?;
            Some((path, report))
        }
        None => evidence::latest_audit(&root.join("conformance/reports"))?,
    };
    if let Some(report) = &tier1 {
        provenance(&tier1_path, report);
    } else {
        println!("tier1: no result");
    }
    if let Some((path, report)) = &audit {
        provenance(path, report);
    } else {
        println!("conformance_audit: no report");
    }
    let audit = audit.as_ref().map(|(_, report)| report);
    let mut systems = BTreeMap::<&str, Vec<&Behavior>>::new();
    for behavior in ledger.behaviors() {
        systems.entry(&behavior.system).or_default().push(behavior);
    }
    let all: Vec<_> = ledger.behaviors().iter().collect();
    let claimed = requests(&all)?;
    println!("\nClient opcodes claimed: {}/{} ClientOpcodeMessage variants; {} claims outside that enum.", claimed.intersection(&counts.variants).count(), counts.variants.len(), claimed.difference(&counts.variants).count());
    println!(
        "Unavailable requests: {} total, {} claimed by the ledger.",
        counts.unavailable.len(),
        claimed.intersection(&counts.unavailable).count()
    );
    println!("\n| System | Behaviors | Tier 1 pass | Audit pass | Real client waiting | Claimed opcodes | Unavailable |\n| --- | ---: | ---: | ---: | ---: | ---: | ---: |");
    for (system, behaviors) in &systems {
        let claimed = requests(behaviors)?;
        let tier1_pass = behaviors
            .iter()
            .map(|behavior| state(tier1.as_ref(), behavior, head, revision))
            .collect::<Result<Vec<_>>>()?
            .iter()
            .filter(|state| state.is_match())
            .count();
        let audit_pass = behaviors
            .iter()
            .map(|behavior| state(audit, behavior, head, revision))
            .collect::<Result<Vec<_>>>()?
            .iter()
            .filter(|state| state.is_match())
            .count();
        let waiting = behaviors
            .iter()
            .filter(|behavior| matches!(behavior.real_client, RealClient::Unverified {}))
            .count();
        println!(
            "| {system} | {} | {tier1_pass} | {audit_pass} | {waiting} | {} | {} |",
            behaviors.len(),
            claimed.len(),
            claimed.intersection(&counts.unavailable).count()
        );
    }
    println!("\n| System / Behavior | Tier 1 | Audit | Real client | Claimed opcodes | Unavailable |\n| --- | --- | --- | --- | ---: | ---: |");
    for (system, behaviors) in systems {
        for behavior in behaviors {
            let claimed = requests(&[behavior])?;
            let real_client = match &behavior.real_client {
                RealClient::Unverified {} => "waiting".into(),
                RealClient::Verified { date, .. } => format!("verified {date}"),
            };
            println!(
                "| {system} / {} | {} | {} | {real_client} | {} | {} |",
                behavior.id,
                state(tier1.as_ref(), behavior, head, revision)?,
                state(audit, behavior, head, revision)?,
                claimed.len(),
                claimed.intersection(&counts.unavailable).count()
            );
        }
    }
    println!("\nOpcode claims and source counts do not establish conformance. Other required rungs remain in the ledger.");
    Ok(())
}

#[test]
fn shared_opcode_claims_count_once() {
    let ledger = FeatureLedger::from_sources([(
        "connection/connection_identity.toml",
        include_str!("../features/connection/connection_identity.toml"),
    )])
    .unwrap();
    let behavior = &ledger.behaviors()[0];
    assert_eq!(
        requests(&[behavior, behavior]).unwrap(),
        [0x0050, 0x01dc].into()
    );
}
