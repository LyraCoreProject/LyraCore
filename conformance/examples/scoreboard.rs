use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, ensure, Context, Result};
use lyracore_conformance::{Behavior, FeatureLedger, RealClient};

#[path = "scoreboard/counts.rs"]
mod counts;
#[path = "scoreboard/evidence.rs"]
mod evidence;
#[path = "scoreboard/spells.rs"]
mod spells;
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
            ", uncommitted changes"
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
    let mut vmangos_path = None;
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
            "--vmangos" => {
                vmangos_path = Some(PathBuf::from(
                    arguments.next().context("--vmangos needs a path")?,
                ));
            }
            "--help" => {
                println!("scoreboard [--tier1 RESULT.json] [--audit REPORT.json] [--vmangos mangos.sqlite]");
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
        Some(Report::load(&tier1_path, "tier1", &ledger)?)
    } else {
        None
    };
    let audit = match audit_path {
        Some(path) => {
            let report = Report::load(&path, "conformance_audit", &ledger)?;
            Some((path, report))
        }
        None => evidence::latest_audit(&root.join("conformance/reports"), None, None)?,
    };
    if let Some(report) = &tier1 {
        provenance(&tier1_path, report);
    } else {
        println!("tier1: no result");
    }
    if let Some((path, report)) = &audit {
        report.validate("conformance_audit", &ledger)?;
        report.validate_changes(path.parent().context("report directory")?)?;
        report.verify_capture(path)?;
        provenance(path, report);
    } else {
        println!("conformance_audit: no report");
    }
    let audit = audit.as_ref().map(|(_, report)| report);
    let tier1_head = match &tier1 {
        Some(report) if evidence::same_core(root, &report.core_commit, head)? => {
            report.core_commit.as_str()
        }
        _ => head,
    };
    let audit_head = match audit {
        Some(report) if evidence::same_core(root, &report.core_commit, head)? => {
            report.core_commit.as_str()
        }
        _ => head,
    };
    if let Some(report) = audit {
        for (behavior, result) in &report.audit.as_ref().context("audit provenance")?.behaviors {
            println!("Reference / {behavior}: {}", result.reference);
            if report.audit.as_ref().unwrap().previous_report.is_some() && result.changes.is_none()
            {
                println!("Changes / {behavior}: comparison is not possible; harness revision or scenario hashes changed");
            }
            if let Some(changes) = &result.changes {
                println!("Changes / {behavior}: {} rule fixed, {} rule regressed; {} data fixed, {} data regressed signatures",
                    changes.rule.fixed.len(), changes.rule.regressed.len(), changes.data.fixed.len(), changes.data.regressed.len());
            }
        }
    }

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
            .map(|behavior| state(tier1.as_ref(), behavior, tier1_head, revision))
            .collect::<Result<Vec<_>>>()?
            .iter()
            .filter(|state| state.is_match())
            .count();
        let audit_pass = behaviors
            .iter()
            .map(|behavior| state(audit, behavior, audit_head, revision))
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
    println!("\n| System / Behavior | Tier 1 | Audit | Real client | Claimed opcodes | Unavailable | Known gaps |\n| --- | --- | --- | --- | ---: | ---: | ---: |");
    for (system, behaviors) in systems {
        for behavior in behaviors {
            let claimed = requests(&[behavior])?;
            let real_client = match &behavior.real_client {
                RealClient::Unverified {} => "waiting".into(),
                RealClient::Verified { date, .. } => format!("verified {date}"),
            };
            println!(
                "| {system} / {} | {} | {} | {real_client} | {} | {} | {} |",
                behavior.id,
                state(tier1.as_ref(), behavior, tier1_head, revision)?,
                state(audit, behavior, audit_head, revision)?,
                claimed.len(),
                claimed.intersection(&counts.unavailable).count(),
                behavior.known_gaps.len()
            );
        }
    }
    spells::print(root, vmangos_path.as_deref())?;
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

#[test]
fn committed_audit_report_replays() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ledger = FeatureLedger::load(&root.join("features"))?;
    let directory = root.join("reports");
    let (path, report) =
        evidence::latest_audit(&directory, None, None)?.context("committed audit report")?;
    report.validate("conformance_audit", &ledger)?;
    report.validate_changes(&directory)?;
    report.verify_capture(&path)
}
