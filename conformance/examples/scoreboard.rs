use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, ensure, Context, Result};
use lyracore_conformance::{Behavior, FeatureLedger, RealClient, ScopeVerification};
use serde::Serialize;
use serde_json::{json, Value};

#[path = "scoreboard/counts.rs"]
mod counts;
#[path = "scoreboard/data.rs"]
mod data;
#[path = "scoreboard/evidence.rs"]
mod evidence;
#[path = "scoreboard/spells.rs"]
mod spells;
use evidence::{Report, State};

#[derive(Serialize)]
struct OpenWork {
    behavior: String,
    system: String,
    kind: &'static str,
    reproduce_with: Option<&'static str>,
    next_command: String,
    prerequisites: Vec<&'static str>,
    dependents: usize,
    open_signatures_or_gaps: usize,
    claimed_opcodes: usize,
}

fn unused_audit_name(reports: &Path, date: &str) -> String {
    for number in 0.. {
        let name = if number == 0 {
            date.to_owned()
        } else {
            format!("{date}-c{number}")
        };
        if ["md", "json", "traces"]
            .iter()
            .all(|extension| !reports.join(format!("{name}.{extension}")).exists())
        {
            return name;
        }
    }
    unreachable!()
}

fn audit_command() -> Result<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("date").arg("+%F").output()?;
    ensure!(output.status.success(), "date failed");
    let date = String::from_utf8(output.stdout)?;
    let name = unused_audit_name(&root.join("reports"), date.trim());
    Ok(format!(
        "./conformance/audit --date {name} --client-data /path/to/client/Data"
    ))
}

struct ScoreboardData<'a> {
    ledger: &'a FeatureLedger,
    counts: &'a counts::Counts,
    spells: &'a spells::SpellCounts,
    tier1: Option<(&'a Path, &'a Report)>,
    audit: Option<(&'a Path, &'a Report)>,
    tier1_head: &'a str,
    audit_head: &'a str,
    revision: &'a str,
    work: &'a [OpenWork],
}

fn open_work(
    ledger: &FeatureLedger,
    audit: Option<&Report>,
    tier1: Option<&Report>,
    tier1_head: &str,
    audit_head: &str,
    revision: &str,
) -> Result<Vec<OpenWork>> {
    let baseline = serde_json::from_str(include_str!("../tier1-baseline.json"))?;
    open_work_from(
        &baseline, ledger, audit, tier1, tier1_head, audit_head, revision,
    )
}

fn open_work_from(
    baseline: &Value,
    ledger: &FeatureLedger,
    audit: Option<&Report>,
    tier1: Option<&Report>,
    tier1_head: &str,
    audit_head: &str,
    revision: &str,
) -> Result<Vec<OpenWork>> {
    let baseline_scenarios = baseline["scenarios"]
        .as_array()
        .context("tier 1 baseline scenarios")?;
    let mut work = Vec::new();
    for behavior in ledger.behaviors() {
        let tier1_state = state(tier1, behavior, tier1_head, revision)?;
        let audit_state = state(audit, behavior, audit_head, revision)?;
        let data_gaps = behavior
            .known_gaps
            .iter()
            .filter(|gap| data::is_data_gap(&gap.path))
            .map(|gap| (&gap.description, &gap.path, &gap.symbol, &gap.evidence))
            .collect::<BTreeSet<_>>();
        if !data_gaps.is_empty() {
            let paths = data_gaps
                .iter()
                .map(|(_, path, _, _)| (*path).clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(" ");
            work.push(OpenWork {
                behavior: behavior.id.clone(),
                system: behavior.system.clone(),
                kind: "data_import",
                reproduce_with: None,
                next_command: format!("rg -n 'unsupported|finding' {paths}"),
                prerequisites: Vec::new(),
                dependents: 0,
                open_signatures_or_gaps: data_gaps.len(),
                claimed_opcodes: 0,
            });
        }
        let distinct_gaps = behavior
            .known_gaps
            .iter()
            .filter(|gap| !data::is_data_gap(&gap.path))
            .map(|gap| (&gap.description, &gap.path, &gap.symbol, &gap.evidence))
            .collect::<BTreeSet<_>>()
            .len();
        if behavior.scope_verification == ScopeVerification::Established
            && ((behavior.scenarios.is_empty() && distinct_gaps == 0 && !data_gaps.is_empty())
                || (!behavior.scenarios.is_empty()
                    && tier1_state.is_match()
                    && audit_state.is_match()
                    && distinct_gaps == 0))
        {
            continue;
        }
        let baseline_signatures = baseline_scenarios
            .iter()
            .filter(|entry| entry["behavior"] == behavior.id)
            .filter_map(|entry| entry["rule"].as_array())
            .flat_map(|signatures| signatures.iter())
            .map(serde_json::to_string)
            .collect::<Result<BTreeSet<_>, _>>()?;
        let baseline_count = baseline_scenarios
            .iter()
            .any(|entry| entry["behavior"] == behavior.id)
            .then_some(baseline_signatures.len());
        let measured = baseline_count.or_else(|| {
            audit
                .and_then(|report| report.audit.as_ref()?.behaviors.get(&behavior.id))
                .map(|evidence| evidence.candidate.rule.signatures.len())
                .or_else(|| {
                    tier1
                        .filter(|report| {
                            report
                                .scenarios
                                .iter()
                                .any(|result| result.behavior == behavior.id)
                        })
                        .map(|report| report.summary(&behavior.id, false).rule.signatures.len())
                })
        });
        let kind = if behavior.scope_verification == ScopeVerification::StockClientUseUnverified {
            "scope_verification"
        } else if behavior.scenarios.is_empty() {
            "scenario_author"
        } else if matches!(
            audit_state,
            State::NotRun | State::Stale | State::Unverified(_) | State::UnstableReference(_)
        ) && (tier1_state.is_match()
            || (!matches!(
                tier1_state,
                State::Different(_) | State::PropertyViolations(..)
            ) && measured == Some(0)
                && distinct_gaps == 0))
        {
            "evidence_refresh"
        } else {
            "implementer"
        };
        let reproduce_with = if kind == "scenario_author" || kind == "scope_verification" {
            None
        } else if kind == "evidence_refresh"
            || (tier1_state.is_match() && matches!(audit_state, State::Different(_)))
        {
            Some("audit")
        } else {
            Some("tier1")
        };
        let next_command = match reproduce_with {
            Some("audit") if kind == "evidence_refresh" => audit_command()?,
            Some("audit") => format!(
                "./conformance/audit --scenario {} --check --client-data /path/to/client/Data",
                behavior.scenarios[0]
            ),
            Some("tier1") => format!("./conformance/run {}", behavior.scenarios[0]),
            _ => format!(
                "cat conformance/features/{}/{}.toml",
                behavior.system, behavior.id
            ),
        };
        let prerequisites = if reproduce_with == Some("audit") {
            vec![
                "Clean core working tree; commit changes before the audit",
                "Existing 1.12.1 client Data directory; replace /path/to/client/Data",
                "Prepared pinned Reference Server and Docker group access; stop the Reference Server before running",
                "Free loopback ports 3000, 3724 and 8085; maintainer access for the disposable Realm",
                "Free loopback Reference Server ports 13724 and 18085",
            ]
        } else {
            Vec::new()
        };
        work.push(OpenWork {
            behavior: behavior.id.clone(),
            system: behavior.system.clone(),
            kind,
            reproduce_with,
            next_command,
            prerequisites,
            dependents: ledger
                .behaviors()
                .iter()
                .filter(|other| other.dependencies.contains(&behavior.id))
                .count(),
            open_signatures_or_gaps: measured.unwrap_or(distinct_gaps),
            claimed_opcodes: requests(&[behavior])?.len(),
        });
    }
    let group = |kind| match kind {
        "scope_verification" => 1,
        "data_import" => 2,
        "evidence_refresh" => 3,
        _ => 0,
    };
    work.sort_by(|a, b| {
        group(a.kind).cmp(&group(b.kind)).then_with(|| {
            b.dependents
                .cmp(&a.dependents)
                .then_with(|| b.open_signatures_or_gaps.cmp(&a.open_signatures_or_gaps))
                .then_with(|| b.claimed_opcodes.cmp(&a.claimed_opcodes))
                .then_with(|| a.behavior.cmp(&b.behavior))
        })
    });
    Ok(work)
}

fn state_json(state: &State) -> Value {
    let (kind, summary) = match state {
        State::NoScenario => ("no_scenario", None),
        State::NotRun => ("not_run", None),
        State::Stale => ("stale", None),
        State::PropertyViolations(_, summary) => ("property_violations", Some(summary)),
        State::Different(summary) => ("different", Some(summary)),
        State::Unverified(summary) => ("unverified", Some(summary)),
        State::UnstableReference(summary) => ("unstable_reference", Some(summary)),
        State::Match(summary) => ("match", Some(summary)),
    };
    json!({"state": kind, "display": state.to_string(), "summary": summary})
}

#[derive(Serialize)]
struct TierSummary {
    evidence: &'static str,
    pass: usize,
    states: BTreeMap<String, usize>,
    scenarios_recorded: usize,
    scenarios_expected: usize,
    rule_signatures: Option<usize>,
    data_signatures: Option<usize>,
}

#[derive(Serialize)]
struct ScoreboardSummary {
    behaviors: usize,
    with_scenario: usize,
    without_scenario: usize,
    tier1: TierSummary,
    audit: TierSummary,
    data_coverage: data::Coverage,
    open_work_by_kind: BTreeMap<&'static str, usize>,
}

fn tier_summary(
    ledger: &FeatureLedger,
    report: Option<&Report>,
    head: &str,
    revision: &str,
) -> Result<TierSummary> {
    let mut states = BTreeMap::new();
    let mut pass = 0;
    let mut rule_signatures = 0;
    let mut data_signatures = 0;
    for behavior in ledger.behaviors() {
        let state = state(report, behavior, head, revision)?;
        pass += usize::from(state.is_match());
        let kind = state_json(&state)["state"]
            .as_str()
            .context("evidence state name")?
            .to_owned();
        *states.entry(kind).or_default() += 1;
        if let Some(report) = report {
            let summary = report.summary(&behavior.id, false);
            rule_signatures += summary.rule.signatures.len();
            data_signatures += summary.data.signatures.len();
        }
    }
    Ok(TierSummary {
        evidence: if report.is_some() {
            "present"
        } else {
            "absent"
        },
        pass,
        states,
        scenarios_recorded: report.map_or(0, |report| report.scenarios.len()),
        scenarios_expected: ledger
            .behaviors()
            .iter()
            .map(|behavior| behavior.scenarios.len())
            .sum(),
        rule_signatures: report.map(|_| rule_signatures),
        data_signatures: report.map(|_| data_signatures),
    })
}

fn scoreboard_summary(data: &ScoreboardData<'_>) -> Result<ScoreboardSummary> {
    let behaviors = data.ledger.behaviors().len();
    let with_scenario = data
        .ledger
        .behaviors()
        .iter()
        .filter(|behavior| !behavior.scenarios.is_empty())
        .count();
    let mut open_work_by_kind = BTreeMap::from([
        ("data_import", 0),
        ("implementer", 0),
        ("scenario_author", 0),
        ("scope_verification", 0),
        ("evidence_refresh", 0),
    ]);
    for item in data.work {
        *open_work_by_kind.entry(item.kind).or_default() += 1;
    }
    Ok(ScoreboardSummary {
        behaviors,
        with_scenario,
        without_scenario: behaviors - with_scenario,
        tier1: tier_summary(
            data.ledger,
            data.tier1.map(|(_, report)| report),
            data.tier1_head,
            data.revision,
        )?,
        audit: tier_summary(
            data.ledger,
            data.audit.map(|(_, report)| report),
            data.audit_head,
            data.revision,
        )?,
        data_coverage: data::coverage(data.ledger)?,
        open_work_by_kind,
    })
}

fn print_summary(data: &ScoreboardData<'_>) -> Result<()> {
    let summary = scoreboard_summary(data)?;
    println!("## Conformance scoreboard\n");
    println!(
        "Behaviors with scenarios: {} / {} ({} without).\n",
        summary.with_scenario, summary.behaviors, summary.without_scenario
    );
    println!("| Tier | Passes | Evidence | Scenarios recorded / expected | Recorded rule signatures | Recorded data signatures |\n| --- | ---: | --- | ---: | ---: | ---: |");
    for (name, tier) in [("Tier 1", &summary.tier1), ("Audit", &summary.audit)] {
        let count = |value: Option<usize>| {
            value.map_or_else(|| "unmeasured".into(), |value| value.to_string())
        };
        println!(
            "| {name} | {} | {} | {} / {} | {} | {} |",
            tier.pass,
            tier.evidence,
            tier.scenarios_recorded,
            tier.scenarios_expected,
            count(tier.rule_signatures),
            count(tier.data_signatures)
        );
    }
    for (name, tier) in [("Tier 1", &summary.tier1), ("Audit", &summary.audit)] {
        println!(
            "\n{name} Behavior states: {}.",
            tier.states
                .iter()
                .map(|(state, count)| format!("{state}: {count}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    println!(
        "\nOpen work: {}.",
        summary
            .open_work_by_kind
            .iter()
            .map(|(kind, count)| format!("{kind}: {count}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "\nData coverage: {} data-import gaps, {} behavior gaps.",
        summary.data_coverage.gaps.data_import, summary.data_coverage.gaps.behavior
    );
    println!(
        "\n| System | Source | Behavior gaps | Data-import gaps |\n| --- | --- | ---: | ---: |"
    );
    for (system, coverage) in &summary.data_coverage.systems {
        println!(
            "| {system} | Total | {} | {} |",
            coverage.gaps.behavior, coverage.gaps.data_import
        );
        for (source, gaps) in &coverage.by_source {
            println!(
                "| {system} | {source} | {} | {} |",
                gaps.behavior, gaps.data_import
            );
        }
    }
    for (system, coverage) in &summary.data_coverage.systems {
        for (path, census) in &coverage.port_census {
            println!("\n{system}: {path} (archived production plan; see report for source identity and scope).");
            for (table, rows) in &census.planned_import_rows_by_table {
                println!("- {table}: {rows} planned import rows");
            }
            if let Some(findings) = census.manifest["findings"].as_array() {
                let mut occurrences = BTreeMap::<&str, u64>::new();
                for finding in findings {
                    let classification = finding["classification"]
                        .as_str()
                        .context("finding classification")?;
                    *occurrences.entry(classification).or_default() += finding["occurrences"]
                        .as_u64()
                        .context("finding occurrences")?;
                }
                for (classification, count) in occurrences {
                    println!("- {classification}: {count} finding occurrences (codes in --json and census)");
                }
            }
        }
    }
    println!("\nPlanned rows and finding occurrences have different units and may overlap; no coverage percentage is inferred.");
    println!("\nSignature totals include stale recorded evidence; absent evidence is unmeasured.");
    Ok(())
}

fn scoreboard_json(data: &ScoreboardData<'_>) -> Result<Value> {
    let ScoreboardData {
        ledger,
        counts,
        spells,
        tier1,
        audit,
        tier1_head,
        audit_head,
        revision,
        work,
    } = data;
    let mut systems = BTreeMap::<&str, Vec<&Behavior>>::new();
    let mut behaviors = BTreeMap::new();
    for behavior in ledger.behaviors() {
        systems.entry(&behavior.system).or_default().push(behavior);
        let claimed = requests(&[behavior])?;
        let real_client = match &behavior.real_client {
            RealClient::Unverified {} => "waiting".to_owned(),
            RealClient::Verified { date, .. } => format!("verified {date}"),
        };
        behaviors.insert(behavior.id.clone(), json!({
            "system": behavior.system, "scenarios": behavior.scenarios,
            "scope_verification": behavior.scope_verification,
            "dependencies": behavior.dependencies, "required_rungs": behavior.required_rungs,
            "tier1": state_json(&state(tier1.map(|(_, report)| report), behavior, tier1_head, revision)?),
            "audit": state_json(&state(audit.map(|(_, report)| report), behavior, audit_head, revision)?),
            "real_client": real_client, "claimed_opcodes": claimed.len(),
            "unavailable": claimed.intersection(&counts.unavailable).count(),
            "known_gaps": behavior.known_gaps.len(),
        }));
    }
    let mut system_rows = BTreeMap::new();
    for (system, entries) in systems {
        let claimed = requests(&entries)?;
        system_rows.insert(system, json!({
            "behaviors": entries.len(),
            "with_scenario": entries.iter().filter(|behavior| !behavior.scenarios.is_empty()).count(),
            "without_scenario": entries.iter().filter(|behavior| behavior.scenarios.is_empty()).count(),
            "tier1_pass": entries.iter().map(|behavior| &behaviors[&behavior.id]["tier1"]["state"]).filter(|state| **state == "match").count(),
            "audit_pass": entries.iter().map(|behavior| &behaviors[&behavior.id]["audit"]["state"]).filter(|state| **state == "match").count(),
            "real_client_waiting": entries.iter().filter(|behavior| matches!(behavior.real_client, RealClient::Unverified {})).count(),
            "claimed_opcodes": claimed.len(),
            "unavailable": claimed.intersection(&counts.unavailable).count(),
        }));
    }
    let all: Vec<_> = ledger.behaviors().iter().collect();
    let claimed = requests(&all)?;
    let unclaimed = unclaimed_requests(ledger, counts)?;
    Ok(json!({
        "schema_version": 1,
        "summary": scoreboard_summary(data)?,
        "tier1_evidence": tier1.map(|(path, report)| json!({"path": path, "report": report})),
        "audit_evidence": audit.map(|(path, report)| json!({"path": path, "report": report})),
        "opcode_coverage": {
            "claimed": claimed.intersection(&counts.variants).count(),
            "out_of_scope": counts.out_of_scope.len(),
            "unclaimed": unclaimed.len(),
            "unclaimed_opcodes": unclaimed.iter().map(|opcode| format!("0x{opcode:04X}")).collect::<Vec<_>>(),
            "total": counts.variants.len(),
            "outside_enum": claimed.difference(&counts.enum_variants).count(),
            "unavailable_total": counts.unavailable.len(),
            "unavailable_claimed": claimed.intersection(&counts.unavailable).count(),
        },
        "systems": system_rows, "behaviors": behaviors, "trainable_spells": spells,
        "open_work": work,
    }))
}

#[cfg(test)]
#[path = "../support/scratch.rs"]
mod scratch;

fn requests(behaviors: &[&Behavior]) -> Result<BTreeSet<u16>> {
    behaviors
        .iter()
        .flat_map(|behavior| &behavior.requests)
        .map(|opcode| u16::from_str_radix(&opcode[2..], 16).map_err(Into::into))
        .collect()
}

fn unclaimed_requests(ledger: &FeatureLedger, counts: &counts::Counts) -> Result<BTreeSet<u16>> {
    let all: Vec<_> = ledger.behaviors().iter().collect();
    let claimed = requests(&all)?;
    ensure!(
        claimed.is_disjoint(&counts.out_of_scope),
        "an out-of-scope opcode is also claimed by a Behavior"
    );
    ensure!(
        claimed.is_subset(&counts.variants),
        "a Behavior claims an opcode outside the build-5875 request inventory"
    );
    Ok(counts
        .variants
        .difference(&claimed)
        .filter(|opcode| !counts.out_of_scope.contains(opcode))
        .copied()
        .collect())
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
    let mut json_output = false;
    let mut summary_output = false;
    let mut arguments = std::env::args().skip(1);
    while let Some(option) = arguments.next() {
        match option.as_str() {
            "--json" => json_output = true,
            "--summary" => summary_output = true,
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
                println!("scoreboard [--json | --summary] [--tier1 RESULT.json] [--audit REPORT.json] [--vmangos mangos.sqlite]");
                return Ok(());
            }
            _ => bail!("unknown option {option}"),
        }
    }
    ensure!(
        !(json_output && summary_output),
        "choose --json or --summary"
    );
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
    let unclaimed = unclaimed_requests(&ledger, &counts)?;
    ensure!(
        unclaimed.is_empty(),
        "unclaimed client requests: {}",
        unclaimed
            .iter()
            .map(|opcode| format!("0x{opcode:04X}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let tier1 = if explicit_tier1 || tier1_path.exists() {
        Some(Report::load(&tier1_path, "tier1", &ledger)?)
    } else {
        None
    };
    let audit_evidence = match audit_path {
        Some(path) => {
            let report = Report::load(&path, "conformance_audit", &ledger)?;
            Some((path, report))
        }
        None => evidence::latest_audit(&root.join("conformance/reports"), None)?,
    };
    if let Some((path, report)) = &audit_evidence {
        report.validate("conformance_audit", &ledger)?;
        report.validate_changes()?;
        report.verify_capture(path)?;
    }
    let audit = audit_evidence.as_ref().map(|(_, report)| report);
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
    let spells = spells::read(root, vmangos_path.as_deref())?;
    let work = open_work(
        &ledger,
        audit,
        tier1.as_ref(),
        tier1_head,
        audit_head,
        revision,
    )?;
    let data = ScoreboardData {
        ledger: &ledger,
        counts: &counts,
        spells: &spells,
        tier1: tier1.as_ref().map(|report| (tier1_path.as_path(), report)),
        audit: audit_evidence
            .as_ref()
            .map(|(path, report)| (path.as_path(), report)),
        tier1_head,
        audit_head,
        revision,
        work: &work,
    };
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&scoreboard_json(&data)?)?
        );
        return Ok(());
    }
    if summary_output {
        return print_summary(&data);
    }
    print_text(&data)
}

fn print_text(data: &ScoreboardData<'_>) -> Result<()> {
    print_summary(data)?;
    let ScoreboardData {
        ledger,
        counts,
        spells,
        tier1_head,
        audit_head,
        revision,
        work,
        ..
    } = data;
    let tier1 = data.tier1.map(|(_, report)| report);
    let audit = data.audit.map(|(_, report)| report);
    if let Some((path, report)) = data.tier1 {
        provenance(path, report);
    } else {
        println!("tier1: no result");
    }
    if let Some((path, report)) = data.audit {
        provenance(path, report);
    } else {
        println!("conformance_audit: no report");
    }
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
    let unclaimed = unclaimed_requests(ledger, counts)?;
    println!("\nClient requests: {} claimed by a Behavior, {} out of scope, {} unclaimed, {} total; {} claims outside ClientOpcodeMessage.", claimed.intersection(&counts.variants).count(), counts.out_of_scope.len(), unclaimed.len(), counts.variants.len(), claimed.difference(&counts.enum_variants).count());
    println!(
        "Unavailable requests: {} total, {} claimed by the ledger.",
        counts.unavailable.len(),
        claimed.intersection(&counts.unavailable).count()
    );
    println!("\n| System | Behaviors | With scenario | Without scenario | Tier 1 pass | Audit pass | Real client waiting | Claimed opcodes | Unavailable |\n| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
    for (system, behaviors) in &systems {
        let claimed = requests(behaviors)?;
        let tier1_pass = behaviors
            .iter()
            .map(|behavior| state(tier1, behavior, tier1_head, revision))
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
            "| {system} | {} | {} | {} | {tier1_pass} | {audit_pass} | {waiting} | {} | {} |",
            behaviors.len(),
            behaviors
                .iter()
                .filter(|behavior| !behavior.scenarios.is_empty())
                .count(),
            behaviors
                .iter()
                .filter(|behavior| behavior.scenarios.is_empty())
                .count(),
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
                state(tier1, behavior, tier1_head, revision)?,
                state(audit, behavior, audit_head, revision)?,
                claimed.len(),
                claimed.intersection(&counts.unavailable).count(),
                behavior.known_gaps.len()
            );
        }
    }
    spells::print(spells);
    println!("\nOpen work: established Behaviors, scope verification, data import, evidence refresh; within each group ranked by dependents, measured rule signatures or distinct behavior gaps (data gaps in their own group), claimed request opcodes, then Behavior id:");
    for (index, item) in work.iter().enumerate() {
        println!(
            "{}. {} / {}: {} (dependents {}, signatures or gaps {}, opcodes {}); reproduce with {}; {}",
            index + 1,
            item.system,
            item.behavior,
            item.kind,
            item.dependents,
            item.open_signatures_or_gaps,
            item.claimed_opcodes,
            item.reproduce_with.unwrap_or("none"),
            item.next_command
        );
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

#[test]
fn protocol_coverage_exposes_unclaimed_requests_and_rejects_overlap() -> Result<()> {
    let ledger = FeatureLedger::from_sources([(
        "connection/connection_identity.toml",
        include_str!("../features/connection/connection_identity.toml"),
    )])?;
    let mut counts = counts::Counts {
        variants: [0x0050, 0x01dc, 0x0036, 0x0002].into(),
        enum_variants: [0x0050, 0x01dc, 0x0036, 0x0002].into(),
        unavailable: BTreeSet::new(),
        out_of_scope: [0x0002].into(),
    };
    assert_eq!(unclaimed_requests(&ledger, &counts)?, [0x0036].into());
    counts.out_of_scope.insert(0x0050);
    assert!(unclaimed_requests(&ledger, &counts).is_err());
    Ok(())
}

#[cfg(test)]
fn synthetic_ledger() -> FeatureLedger {
    let source = |id: &str, dependencies: &str, scenarios: &str, requests: &str| {
        format!(
            r#"
schema_version = 1
id = "{id}"
system = "sample"
scope = "Synthetic Behavior"
requests = {requests}
replies = []
dependencies = {dependencies}
required_rungs = ["tier1", "conformance_audit", "real_client"]
scenarios = {scenarios}
deliberate_deviations = []
prerequisites = []
tests = []
[vmangos]
commit = "d9173eca3ea08dd985c52dabbcc826898d1d0b77"
path = "src/game/Handlers/QueryHandler.cpp"
symbol = "WorldSession::HandleQueryPlayerNameOpcode"
[real_client]
status = "unverified"
"#
        )
    };
    let gap = "\n[[known_gaps]]\ndescription = \"Gap\"\npath = \"module/src/taxi.rs\"\nsymbol = \"test\"\nevidence = \"test\"\n";
    let sources = [
        (
            "sample/root.toml",
            source("root", "[]", "[]", "[\"0x0050\"]"),
        ),
        (
            "sample/branch.toml",
            source(
                "branch",
                "[\"root\"]",
                "[\"branch\"]",
                "[\"0x0050\", \"0x01DC\"]",
            ),
        ),
        (
            "sample/leaf.toml",
            source("leaf", "[\"root\", \"branch\"]", "[]", "[\"0x0050\"]"),
        ),
        (
            "sample/tie.toml",
            format!(
                "{}{gap}{gap}",
                source("tie", "[]", "[]", "[\"0x0050\"]").replace(
                    "replies = []",
                    "replies = []\nscope_verification = \"stock_client_use_unverified\""
                )
            ),
        ),
        (
            "sample/more.toml",
            source("more", "[]", "[]", "[\"0x0050\", \"0x01DC\"]"),
        ),
        ("sample/zoo.toml", source("zoo", "[]", "[]", "[\"0x0050\"]")),
    ];
    FeatureLedger::from_sources(
        sources
            .iter()
            .map(|(path, source)| (*path, source.as_str())),
    )
    .unwrap()
}

#[test]
fn open_work_ranks_synthetic_ledger_deterministically() -> Result<()> {
    let ledger = synthetic_ledger();
    let work = open_work(&ledger, None, None, "head", "head", "pin")?;
    assert_eq!(
        work.iter()
            .map(|item| item.behavior.as_str())
            .collect::<Vec<_>>(),
        ["root", "branch", "more", "leaf", "zoo", "tie"]
    );
    assert_eq!(work[0].dependents, 2);
    assert_eq!(work[1].kind, "implementer");
    assert_eq!(work[1].next_command, "./conformance/run branch");
    assert!(work[1].prerequisites.is_empty());
    assert_eq!(work[2].kind, "scenario_author");
    assert_eq!(work[5].open_signatures_or_gaps, 1);
    Ok(())
}

#[test]
fn unverified_request_scope_follows_established_work_before_evidence_refresh() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ledger = FeatureLedger::load(&root.join("features"))?;
    let work = open_work(&ledger, None, None, "head", "head", "pin")?;
    let position = |id| work.iter().position(|item| item.behavior == id).unwrap();
    assert!(position("direct_trade") < position("debug_motion_controls"));
    assert!(position("cast_and_interrupt") < position("debug_motion_controls"));
    assert!(position("debug_motion_controls") < position("connection_identity"));
    assert_eq!(
        work[position("debug_motion_controls")].kind,
        "scope_verification"
    );
    Ok(())
}

#[test]
fn audit_name_skips_existing_report_parts() -> Result<()> {
    let reports = scratch::Scratch::new("audit-name-test")?;
    for extension in ["json", "md", "traces"] {
        std::fs::write(reports.join(format!("2026-10-28.{extension}")), "")?;
    }
    std::fs::write(reports.join("2026-10-28-c1.json"), "")?;
    assert_eq!(unused_audit_name(&reports, "2026-10-28"), "2026-10-28-c2");
    Ok(())
}

#[cfg(test)]
fn clean_baseline() -> Result<Value> {
    let mut baseline: Value = serde_json::from_str(include_str!("../tier1-baseline.json"))?;
    for scenario in baseline["scenarios"].as_array_mut().unwrap() {
        scenario["rule"] = json!([]);
    }
    Ok(baseline)
}

#[test]
fn clean_baseline_without_report_requests_audit() -> Result<()> {
    let ledger = synthetic_ledger();
    let baseline = json!({"scenarios": [{"behavior": "branch", "rule": []}]});
    let work = open_work_from(&baseline, &ledger, None, None, "head", "head", "pin")?;
    let item = work.iter().find(|item| item.behavior == "branch").unwrap();
    assert_eq!(item.kind, "evidence_refresh");
    assert!(item.next_command.starts_with("./conformance/audit --date "));
    Ok(())
}

#[test]
fn clean_tier1_with_stale_audit_requests_evidence_refresh() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ledger = FeatureLedger::load(&root.join("features"))?;
    let (_, audit) = evidence::latest_audit(&root.join("reports"), None)?.context("audit")?;
    let mut tier1_value = serde_json::to_value(&audit)?;
    tier1_value["tier"] = json!("tier1");
    tier1_value["audit"] = Value::Null;
    for scenario in tier1_value["scenarios"].as_array_mut().unwrap() {
        scenario["comparison"]["differences"] = json!([]);
        scenario["properties"] = json!([]);
    }
    let tier1: Report = serde_json::from_value(tier1_value)?;
    let work = open_work_from(
        &clean_baseline()?,
        &ledger,
        Some(&audit),
        Some(&tier1),
        &tier1.core_commit,
        "stale-head",
        &tier1.harness_revision,
    )?;
    let connection = work
        .iter()
        .find(|item| item.behavior == "connection_identity")
        .unwrap();
    let taxi = work
        .iter()
        .find(|item| item.behavior == "taxi_single_hop")
        .unwrap();
    assert_eq!(connection.kind, "evidence_refresh");
    assert!(connection
        .next_command
        .starts_with("./conformance/audit --date "));
    assert!(connection
        .next_command
        .ends_with(" --client-data /path/to/client/Data"));
    assert_eq!(connection.prerequisites.len(), 5);
    assert!(connection
        .prerequisites
        .iter()
        .any(|item| item.contains("Clean core working tree")));
    assert!(connection
        .prerequisites
        .iter()
        .any(|item| item.contains("13724 and 18085")));
    assert_eq!(taxi.kind, "evidence_refresh");
    assert!(taxi.next_command.starts_with("./conformance/audit --date "));
    assert_eq!(taxi.open_signatures_or_gaps, 0);
    assert!(
        work.iter()
            .position(|item| item.behavior == "taxi_single_hop")
            > work
                .iter()
                .position(|item| item.behavior == "connection_identity")
    );
    let without_tier1 = open_work_from(
        &clean_baseline()?,
        &ledger,
        Some(&audit),
        None,
        &tier1.core_commit,
        "stale-head",
        &tier1.harness_revision,
    )?;
    let connection = without_tier1
        .iter()
        .find(|item| item.behavior == "connection_identity")
        .unwrap();
    assert_eq!(connection.kind, "evidence_refresh");
    assert!(connection
        .next_command
        .starts_with("./conformance/audit --date "));
    let without_audit = open_work_from(
        &clean_baseline()?,
        &ledger,
        None,
        Some(&tier1),
        &tier1.core_commit,
        &tier1.core_commit,
        &tier1.harness_revision,
    )?;
    let taxi = without_audit
        .iter()
        .find(|item| item.behavior == "taxi_single_hop")
        .unwrap();
    assert_eq!(taxi.open_signatures_or_gaps, 0);
    Ok(())
}

#[test]
fn clean_tier1_with_audit_difference_reproduces_with_audit() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ledger = FeatureLedger::load(&root.join("features"))?;
    let (_, mut audit) = evidence::latest_audit(&root.join("reports"), None)?.context("audit")?;
    for scenario in &mut audit.scenarios {
        scenario.scenario_sha256 = wire_client::catalogue::get(&scenario.id)?
            .trace()
            .header
            .scenario_sha256
            .clone();
    }
    let mut tier1_value = serde_json::to_value(&audit)?;
    tier1_value["tier"] = json!("tier1");
    tier1_value["audit"] = Value::Null;
    for scenario in tier1_value["scenarios"].as_array_mut().unwrap() {
        scenario["comparison"]["differences"] = json!([]);
        scenario["properties"] = json!([]);
    }
    let scenarios = tier1_value["scenarios"].as_array_mut().unwrap();
    let taxi = scenarios
        .iter()
        .find(|scenario| scenario["id"] == "taxi_single_hop")
        .unwrap()
        .clone();
    for id in [
        "taxi_single_hop__late_observer",
        "taxi_single_hop__reconnect",
    ] {
        let mut variant = taxi.clone();
        variant["id"] = json!(id);
        variant["scenario_sha256"] = json!(
            wire_client::catalogue::get(id)?
                .trace()
                .header
                .scenario_sha256
        );
        scenarios.push(variant);
    }
    let tier1: Report = serde_json::from_value(tier1_value)?;
    let work = open_work_from(
        &clean_baseline()?,
        &ledger,
        Some(&audit),
        Some(&tier1),
        &tier1.core_commit,
        &audit.core_commit,
        &tier1.harness_revision,
    )?;
    let taxi = work
        .iter()
        .find(|item| item.behavior == "taxi_single_hop")
        .unwrap();
    assert_eq!(taxi.kind, "implementer");
    assert_eq!(taxi.reproduce_with, Some("audit"));
    assert_eq!(
        taxi.next_command,
        "./conformance/audit --scenario taxi_single_hop --check --client-data /path/to/client/Data"
    );
    assert_eq!(taxi.prerequisites.len(), 5);
    assert!(taxi
        .prerequisites
        .iter()
        .any(|item| item.contains("Clean core working tree")));
    assert!(taxi
        .prerequisites
        .iter()
        .any(|item| item.contains("13724 and 18085")));
    assert_eq!(serde_json::to_value(taxi)?["reproduce_with"], "audit");
    Ok(())
}

#[test]
fn json_reports_behavior_evidence_and_scenario_coverage() -> Result<()> {
    let ledger = synthetic_ledger();
    let counts = counts::Counts {
        variants: [0x0050, 0x01dc].into(),
        enum_variants: [0x0050, 0x01dc].into(),
        unavailable: [0x0050].into(),
        out_of_scope: BTreeSet::new(),
    };
    let spells = spells::SpellCounts {
        build: 5875,
        source: None,
        expected: Some("missing".into()),
        classes: Vec::new(),
    };
    let work = open_work(&ledger, None, None, "head", "head", "pin")?;
    let output = scoreboard_json(&ScoreboardData {
        ledger: &ledger,
        counts: &counts,
        spells: &spells,
        tier1: None,
        audit: None,
        tier1_head: "head",
        audit_head: "head",
        revision: "pin",
        work: &work,
    })?;
    assert_eq!(output["summary"]["tier1"]["evidence"], "absent");
    assert_eq!(output["summary"]["tier1"]["pass"], 0);
    assert_eq!(output["summary"]["tier1"]["states"]["not_run"], 1);
    assert_eq!(output["summary"]["tier1"]["rule_signatures"], Value::Null);
    assert_eq!(output["schema_version"], 1);
    assert_eq!(
        output["summary"]["data_coverage"]["gaps"],
        json!({"behavior": 2, "data_import": 0})
    );
    assert_eq!(
        output["summary"]["data_coverage"]["systems"]["sample"]["by_source"]["module/src/taxi.rs"]
            ["behavior"],
        2
    );
    assert_eq!(output["systems"]["sample"]["with_scenario"], 1);
    assert_eq!(output["systems"]["sample"]["without_scenario"], 5);
    assert_eq!(output["behaviors"]["branch"]["tier1"]["state"], "not_run");
    assert_eq!(output["behaviors"]["root"]["tier1"]["state"], "no_scenario");
    assert_eq!(
        output["behaviors"]["branch"]["required_rungs"],
        json!(["tier1", "conformance_audit", "real_client"])
    );
    assert_eq!(output["opcode_coverage"]["claimed"], 2);
    assert_eq!(output["opcode_coverage"]["out_of_scope"], 0);
    assert_eq!(output["opcode_coverage"]["unclaimed"], 0);
    assert_eq!(output["open_work"].as_array().unwrap().len(), 6);
    assert_eq!(output["open_work"][0]["reproduce_with"], Value::Null);
    assert_eq!(output["open_work"][1]["reproduce_with"], "tier1");
    assert!(output["audit_evidence"].is_null());
    Ok(())
}

#[cfg(test)]
fn measured_connection() -> Result<(FeatureLedger, Report)> {
    let ledger = FeatureLedger::from_sources([
        (
            "connection/connection_identity.toml",
            include_str!("../features/connection/connection_identity.toml"),
        ),
        (
            "taxi/taxi_single_hop.toml",
            include_str!("../features/taxi/taxi_single_hop.toml"),
        ),
    ])?;
    let golden = wire_client::catalogue::get("connection_identity")?;
    let report = serde_json::from_value(json!({
        "schema_version": 2, "tier": "tier1", "core_commit": "head",
        "working_tree_dirty": false, "harness_revision": "pin", "recorded_at_unix_ms": 1,
        "scenarios": [{
            "id": "connection_identity", "behavior": "connection_identity",
            "scenario_sha256": golden.trace().header.scenario_sha256,
            "comparison": {"differences": []}, "properties": [],
        }],
    }))?;
    Ok((ledger, report))
}

#[test]
fn summary_separates_partial_evidence_from_measured_passes() -> Result<()> {
    let (ledger, report) = measured_connection()?;
    let summary = tier_summary(&ledger, Some(&report), "head", "pin")?;
    assert_eq!(summary.evidence, "present");
    assert_eq!(summary.pass, 1);
    assert_eq!(summary.scenarios_recorded, 1);
    assert_eq!(summary.scenarios_expected, 4);
    assert_eq!(summary.states["match"], 1);
    assert_eq!(summary.states["not_run"], 1);
    assert_eq!(summary.rule_signatures, Some(0));
    assert_eq!(summary.data_signatures, Some(0));
    Ok(())
}

#[test]
fn summary_keeps_recorded_signatures_when_evidence_is_stale() -> Result<()> {
    let (ledger, mut report) = measured_connection()?;
    let difference = json!({
        "classification": "rule", "character": null, "checkpoint": "reply", "stream": null,
        "packet_index": null, "field_path": ["value"], "opcode": 1, "deviations": [],
        "rule": "field_value", "reason": "different value", "expected": null, "actual": null,
    });
    let mut data_difference = difference.clone();
    data_difference["classification"] = json!("data");
    report.scenarios[0].comparison.differences =
        serde_json::from_value(json!([difference, difference, data_difference,]))?;
    let summary = tier_summary(&ledger, Some(&report), "head", "pin")?;
    assert_eq!(summary.pass, 0);
    assert_eq!(summary.states["different"], 1);
    assert_eq!(summary.rule_signatures, Some(1));
    assert_eq!(summary.data_signatures, Some(1));
    let stale = tier_summary(&ledger, Some(&report), "new-head", "pin")?;
    assert_eq!(stale.pass, 0);
    assert_eq!(stale.states["stale"], 2);
    assert_eq!(stale.rule_signatures, Some(1));
    assert_eq!(stale.data_signatures, Some(1));
    Ok(())
}

#[test]
fn json_keeps_committed_audit_evidence() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ledger = FeatureLedger::load(&root.join("features"))?;
    let (path, report) = evidence::latest_audit(&root.join("reports"), None)?.context("audit")?;
    let all: Vec<_> = ledger.behaviors().iter().collect();
    let counts = counts::Counts {
        variants: requests(&all)?,
        enum_variants: BTreeSet::new(),
        unavailable: BTreeSet::new(),
        out_of_scope: BTreeSet::new(),
    };
    let spells = spells::SpellCounts {
        build: 5875,
        source: None,
        expected: Some("missing".into()),
        classes: Vec::new(),
    };
    let output = scoreboard_json(&ScoreboardData {
        ledger: &ledger,
        counts: &counts,
        spells: &spells,
        tier1: None,
        audit: Some((&path, &report)),
        tier1_head: "head",
        audit_head: "head",
        revision: evidence::harness_revision(),
        work: &[],
    })?;
    assert_eq!(
        output["audit_evidence"]["report"]["tier"],
        "conformance_audit"
    );
    assert!(output["audit_evidence"]["report"]["audit"]["behaviors"].is_object());
    assert!(output["audit_evidence"]["report"]["scenarios"].is_array());
    assert_eq!(
        output["audit_evidence"]["report"]["scenarios"]
            .as_array()
            .unwrap()
            .len(),
        report.scenarios.len()
    );
    Ok(())
}

#[test]
fn committed_audit_report_replays() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ledger = FeatureLedger::load(&root.join("features"))?;
    let directory = root.join("reports");
    let (path, report) =
        evidence::latest_audit(&directory, None)?.context("committed audit report")?;
    report.validate("conformance_audit", &ledger)?;
    report.validate_changes()?;
    report.verify_capture(&path)
}

#[cfg(test)]
fn copied_audit() -> Result<(scratch::Scratch, PathBuf)> {
    let directory = scratch::Scratch::new("external-audit-test")?;
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("reports/2026-10-28-c5.json");
    let path = directory.join("2026-10-28.json");
    std::fs::copy(&source, &path)?;
    let report = Report::load_structure(&source, "conformance_audit")?;
    std::fs::write(
        path.with_extension("md"),
        evidence::markdown(&report, "2026-10-28")?,
    )?;
    let traces = path.with_extension("traces");
    std::fs::create_dir(&traces)?;
    for entry in std::fs::read_dir(source.with_extension("traces"))? {
        let entry = entry?;
        std::fs::copy(entry.path(), traces.join(entry.file_name()))?;
    }
    Ok((directory, path))
}

#[test]
fn external_report_with_colliding_stem_cannot_clear_predecessor() -> Result<()> {
    let (_directory, path) = copied_audit()?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ledger = FeatureLedger::load(&root.join("features"))?;
    let mut report = Report::load(&path, "conformance_audit", &ledger)?;
    let audit = report.audit.as_mut().context("audit")?;
    audit.previous_report = None;
    for behavior in audit.behaviors.values_mut() {
        behavior.changes = None;
    }
    std::fs::write(&path, serde_json::to_vec_pretty(&report)?)?;
    std::fs::write(
        path.with_extension("md"),
        evidence::markdown(&report, "2026-10-28")?,
    )?;
    let report = Report::load(&path, "conformance_audit", &ledger)?;
    report.verify_capture(&path)?;
    assert!(report.validate_changes().is_err());
    Ok(())
}

#[test]
fn external_report_with_canonical_predecessor_link_passes() -> Result<()> {
    let (_directory, path) = copied_audit()?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ledger = FeatureLedger::load(&root.join("features"))?;
    let report = Report::load(&path, "conformance_audit", &ledger)?;
    report.validate_changes()?;
    report.verify_capture(&path)?;
    #[cfg(unix)]
    {
        let alias = _directory.join("alias");
        std::os::unix::fs::symlink(&*_directory, &alias)?;
        let alias_path = alias.join("2026-10-28.json");
        let report = Report::load(&alias_path, "conformance_audit", &ledger)?;
        report.validate_changes()?;
        report.verify_capture(&alias_path)?;
    }
    Ok(())
}

#[test]
fn importer_gaps_have_their_own_work_instead_of_scenario_rank() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ledger = FeatureLedger::load(&root.join("features"))?;
    let work = open_work(&ledger, None, None, "head", "head", "pin")?;
    let creature: Vec<_> = work
        .iter()
        .filter(|item| item.behavior == "creature_world_behavior")
        .collect();
    assert_eq!(creature.len(), 1);
    assert_eq!(creature[0].kind, "data_import");
    assert_eq!(creature[0].open_signatures_or_gaps, 2252);
    assert_eq!(creature[0].reproduce_with, None);
    assert!(creature[0]
        .next_command
        .contains("importer/src/world_import_creatures.rs"));
    let quest: Vec<_> = work
        .iter()
        .filter(|item| item.behavior == "quest_progress")
        .collect();
    assert_eq!(quest.len(), 2);
    assert!(quest
        .iter()
        .any(|item| item.kind == "data_import" && item.open_signatures_or_gaps == 19));
    assert!(quest
        .iter()
        .any(|item| item.kind == "scenario_author" && item.open_signatures_or_gaps == 1));
    assert_ne!(work[0].kind, "data_import");
    Ok(())
}
