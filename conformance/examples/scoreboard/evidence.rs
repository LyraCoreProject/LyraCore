use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{ensure, Context, Result};
use lyracore_conformance::{Behavior, FeatureLedger};
use serde::{Deserialize, Serialize};
use wire_client::compare::{Comparison, Difference, DifferenceClass};
use wire_client::scenario::Stream;
use wire_client::trace::{Origin, Provenance, Sha256Digest, Trace};

#[path = "markdown.rs"]
mod rendering;
pub use rendering::markdown;

pub fn harness_revision() -> &'static str {
    include_str!("../../../.wire-harness-rev")
        .split_whitespace()
        .last()
        .expect("harness pin")
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub schema_version: u32,
    pub tier: String,
    pub core_commit: String,
    pub working_tree_dirty: bool,
    pub harness_revision: String,
    pub recorded_at_unix_ms: u64,
    pub scenarios: Vec<ScenarioResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit: Option<Audit>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioResult {
    pub id: String,
    pub behavior: String,
    pub scenario_sha256: Sha256Digest,
    pub comparison: Comparison,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_comparison: Option<Comparison>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<CandidateCapture>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_sha256: Option<Sha256Digest>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateCapture {
    pub sha256: Sha256Digest,
    pub core_commit: String,
    pub native_host_sha256: Sha256Digest,
    pub module_sha256: Sha256Digest,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Audit {
    pub native_host_sha256: Sha256Digest,
    pub module_sha256: Sha256Digest,
    pub content: String,
    pub reference: Provenance,
    pub previous_report: Option<String>,
    pub behaviors: BTreeMap<String, BehaviorAudit>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BehaviorAudit {
    pub candidate: Summary,
    pub reference: Summary,
    pub changes: Option<Changes>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Summary {
    pub rule: ClassSummary,
    pub data: ClassSummary,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClassSummary {
    pub raw_records: usize,
    pub signatures: Vec<SignatureCount>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureCount {
    pub signature: Signature,
    pub raw_records: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Signature {
    pub rule: String,
    pub checkpoint: String,
    pub stream: Option<Stream>,
    pub opcode: Option<u16>,
    pub field_path: Vec<String>,
}

impl From<&Difference> for Signature {
    fn from(difference: &Difference) -> Self {
        Self {
            rule: difference.rule.clone(),
            checkpoint: difference.checkpoint.clone(),
            stream: difference.stream.clone(),
            opcode: difference.opcode,
            field_path: difference.field_path.clone(),
        }
    }
}

impl Summary {
    pub fn collect<'a>(differences: impl Iterator<Item = &'a Difference>) -> Self {
        let mut rules = BTreeMap::<Signature, usize>::new();
        let mut data = BTreeMap::<Signature, usize>::new();
        for difference in differences {
            let counts = match difference.classification {
                DifferenceClass::Rule => &mut rules,
                DifferenceClass::Data => &mut data,
            };
            *counts.entry(difference.into()).or_default() += 1;
        }
        let class = |counts: BTreeMap<Signature, usize>| ClassSummary {
            raw_records: counts.values().sum(),
            signatures: counts
                .into_iter()
                .map(|(signature, raw_records)| SignatureCount {
                    signature,
                    raw_records,
                })
                .collect(),
        };
        Self {
            rule: class(rules),
            data: class(data),
        }
    }
}

impl std::fmt::Display for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} rule / {} data signatures; {} / {} records",
            self.rule.signatures.len(),
            self.data.signatures.len(),
            self.rule.raw_records,
            self.data.raw_records
        )
    }
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Changes {
    pub rule: SignatureChanges,
    pub data: SignatureChanges,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureChanges {
    pub fixed: BTreeSet<Signature>,
    pub regressed: BTreeSet<Signature>,
}

impl Changes {
    pub fn between(previous: &Summary, current: &Summary) -> Self {
        let changes = |before: &ClassSummary, after: &ClassSummary| {
            let keys = |summary: &ClassSummary| {
                summary
                    .signatures
                    .iter()
                    .map(|entry| entry.signature.clone())
                    .collect::<BTreeSet<_>>()
            };
            let before = keys(before);
            let after = keys(after);
            SignatureChanges {
                fixed: before.difference(&after).cloned().collect(),
                regressed: after.difference(&before).cloned().collect(),
            }
        };
        Self {
            rule: changes(&previous.rule, &current.rule),
            data: changes(&previous.data, &current.data),
        }
    }
}

impl Report {
    pub fn parse(bytes: &[u8], tier: &str, ledger: &FeatureLedger) -> Result<Self> {
        let report: Self = serde_json::from_slice(bytes)?;
        report.validate(tier, ledger)?;
        Ok(report)
    }

    fn validate_structure(&self, tier: &str) -> Result<()> {
        ensure!(
            self.schema_version == 2 && self.tier == tier,
            "wrong evidence schema or tier"
        );
        for sha in [&self.core_commit, &self.harness_revision] {
            ensure!(
                sha.len() == 40 && sha.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "invalid evidence revision"
            );
        }
        ensure!(self.recorded_at_unix_ms > 0, "missing evidence time");
        ensure!(
            matches!(tier, "tier1" | "conformance_audit"),
            "unknown evidence tier"
        );
        ensure!(
            (tier == "conformance_audit") == self.audit.is_some(),
            "audit provenance required only for audits"
        );
        let mut ids = BTreeSet::new();
        for scenario in &self.scenarios {
            ensure!(
                wire_client::scenario::identifier(&scenario.id) && ids.insert(&scenario.id),
                "invalid or duplicate result scenario id"
            );
            ensure!(
                wire_client::scenario::identifier(&scenario.behavior),
                "invalid result Behavior id"
            );
            ensure!(
                (tier == "conformance_audit") == scenario.reference_comparison.is_some(),
                "reference comparison required only for audits"
            );
            ensure!(
                (tier == "conformance_audit") == scenario.capture.is_some()
                    && (tier == "conformance_audit") == scenario.reference_sha256.is_some(),
                "candidate provenance and reference digest required only for audits"
            );
            if let (Some(capture), Some(audit)) = (&scenario.capture, &self.audit) {
                ensure!(
                    capture.core_commit == self.core_commit
                        && capture.native_host_sha256 == audit.native_host_sha256
                        && capture.module_sha256 == audit.module_sha256,
                    "candidate provenance differs from the audit run"
                );
            }
        }
        if let Some(audit) = &self.audit {
            audit.reference.validate()?;
            ensure!(
                audit.reference.server == "vmangos",
                "audit reference must be vmangos"
            );
            ensure!(
                !audit.content.trim().is_empty(),
                "missing content provenance"
            );
            let behaviors: BTreeSet<_> = self
                .scenarios
                .iter()
                .map(|scenario| &scenario.behavior)
                .collect();
            ensure!(
                behaviors == audit.behaviors.keys().collect(),
                "audit summary has missing or extra Behaviors"
            );
        }
        Ok(())
    }

    pub fn validate(&self, tier: &str, ledger: &FeatureLedger) -> Result<()> {
        self.validate_structure(tier)?;
        for result in &self.scenarios {
            let behavior = ledger
                .behaviors()
                .iter()
                .find(|b| b.id == result.behavior)
                .context("result names an unknown Behavior")?;
            ensure!(
                behavior.scenarios.contains(&result.id),
                "result scenario is not listed by its Behavior"
            );
            if self.harness_revision == harness_revision() {
                let golden = wire_client::catalogue::get(&result.id)?;
                if result.scenario_sha256 == golden.trace().header.scenario_sha256 {
                    for comparison in
                        std::iter::once(&result.comparison).chain(&result.reference_comparison)
                    {
                        for difference in &comparison.differences {
                            // The comparator uses an empty checkpoint for unmatched rule declarations.
                            ensure!(
                                golden
                                    .scenario()
                                    .steps
                                    .iter()
                                    .any(|s| s.id == difference.checkpoint)
                                    || (difference.checkpoint.is_empty()
                                        && difference.classification == DifferenceClass::Rule
                                        && difference.stream.is_none()
                                        && difference.packet_index.is_none()
                                        && difference.expected.is_none()
                                        && difference.actual.is_none()),
                                "difference names an unknown checkpoint"
                            );
                            if difference.classification == DifferenceClass::Data {
                                ensure!(
                                    golden.scenario().rules.data_fields.iter().any(|rule| Some(
                                        rule.opcode
                                    )
                                        == difference.opcode
                                        && rule.path == difference.field_path
                                        && rule.name == difference.rule),
                                    "data difference has no scenario data_fields declaration"
                                );
                            }
                        }
                    }
                }
            }
        }
        if let Some(audit) = &self.audit {
            for (behavior, summary) in &audit.behaviors {
                ensure!(summary.candidate == self.summary(behavior, false)
                    && summary.reference == self.summary(behavior, true),
                    "{behavior}: cannot re-derive classification, signatures or counts from comparator tags");
                ensure!(
                    summary.changes.is_none() || audit.previous_report.is_some(),
                    "changes require a previous report"
                );
            }
        }
        Ok(())
    }

    fn load_structure(path: &Path, tier: &str) -> Result<Self> {
        let report: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        report.validate_structure(tier)?;
        Ok(report)
    }

    pub fn verify_capture(&self, path: &Path) -> Result<()> {
        let date = path
            .file_stem()
            .and_then(|s| s.to_str())
            .context("report filename")?;
        ensure!(
            std::fs::read_to_string(path.with_extension("md"))? == markdown(self, date)?,
            "Markdown report differs from JSON rendering"
        );
        if self.harness_revision == harness_revision() {
            let ids: BTreeSet<_> = self.scenarios.iter().map(|s| s.id.as_str()).collect();
            ensure!(
                ids == wire_client::catalogue::ENTRIES
                    .iter()
                    .map(|entry| entry.id)
                    .collect(),
                "audit must contain every catalogue scenario"
            );
        }
        for result in &self.scenarios {
            let capture = result.capture.as_ref().context("candidate provenance")?;
            let candidate_path = path
                .with_extension("traces")
                .join(format!("{}-lyracore.json", result.id));
            let bytes = std::fs::read(&candidate_path)
                .with_context(|| format!("read {}", candidate_path.display()))?;
            ensure!(
                Sha256Digest::of(&bytes) == capture.sha256,
                "candidate SHA-256 differs from the audit run"
            );
            if self.harness_revision != harness_revision() {
                continue;
            }
            let golden = wire_client::catalogue::get(&result.id)?;
            ensure!(
                result.scenario_sha256 == golden.trace().header.scenario_sha256,
                "audit scenario hash differs from the pinned catalogue"
            );
            let Origin::Golden { provenance } = &golden.trace().header.origin else {
                anyhow::bail!("expected Golden Trace");
            };
            ensure!(
                self.audit.as_ref().context("audit provenance")?.reference == *provenance,
                "reference provenance differs from the Golden Trace"
            );
            let candidate = Trace::parse(&bytes)?;
            ensure!(
                matches!(&candidate.header.origin, Origin::Candidate { adapter } if adapter == "lyracore"),
                "wrong candidate origin"
            );
            let recomputed = wire_client::compare::compare(&golden, &candidate, &[])?;
            ensure!(
                serde_json::to_value(&result.comparison)? == serde_json::to_value(recomputed)?,
                "{} lyracore: stored comparison differs from candidate replay",
                result.id
            );
        }
        Ok(())
    }

    pub fn changes_since(&self, previous: &Self, behavior: &str) -> Option<Changes> {
        let hashes = |report: &Self| {
            report
                .scenarios
                .iter()
                .filter(|s| s.behavior == behavior)
                .map(|s| (s.id.clone(), s.scenario_sha256.clone()))
                .collect::<BTreeMap<_, _>>()
        };
        let current = hashes(self);
        (self.harness_revision == previous.harness_revision
            && !current.is_empty()
            && current == hashes(previous))
        .then(|| {
            Changes::between(
                &previous.summary(behavior, false),
                &self.summary(behavior, false),
            )
        })
    }

    pub fn load(path: &Path, tier: &str, ledger: &FeatureLedger) -> Result<Self> {
        Self::parse(&std::fs::read(path)?, tier, ledger)
            .with_context(|| format!("read {}", path.display()))
    }

    pub fn summary(&self, behavior: &str, reference: bool) -> Summary {
        Summary::collect(
            self.scenarios
                .iter()
                .filter(|scenario| scenario.behavior == behavior)
                .filter_map(|scenario| {
                    if reference {
                        scenario.reference_comparison.as_ref()
                    } else {
                        Some(&scenario.comparison)
                    }
                })
                .flat_map(|comparison| &comparison.differences),
        )
    }

    pub fn validate_changes(&self, directory: &Path) -> Result<()> {
        let Some(audit) = &self.audit else {
            return Ok(());
        };
        let Some(name) = &audit.previous_report else {
            return Ok(());
        };
        let path = Path::new(name);
        ensure!(
            path.file_name().and_then(|name| name.to_str()) == Some(name.as_str())
                && path.extension().is_some_and(|ext| ext == "json"),
            "previous report must be a JSON filename"
        );
        let previous_path = directory.join(path);
        let previous = Self::load_structure(&previous_path, "conformance_audit")?;
        if previous.harness_revision == harness_revision() {
            previous.verify_capture(&previous_path)?;
        }
        ensure!(
            previous.recorded_at_unix_ms < self.recorded_at_unix_ms,
            "previous report must be earlier"
        );
        for (behavior, result) in &audit.behaviors {
            ensure!(
                result.changes == self.changes_since(&previous, behavior),
                "{behavior}: fixed and regressed signatures disagree with the previous report"
            );
        }
        Ok(())
    }

    pub fn state(&self, behavior: &Behavior, head: &str, revision: &str) -> Result<State> {
        if behavior.scenarios.is_empty() {
            return Ok(State::NoScenario);
        }
        if self.core_commit != head || self.harness_revision != revision {
            return Ok(State::Stale);
        }
        let mut missing = false;
        for id in &behavior.scenarios {
            let Some(result) = self.scenarios.iter().find(|result| result.id == *id) else {
                missing = true;
                continue;
            };
            ensure!(
                result.behavior == behavior.id,
                "result {id} names another Behavior"
            );
            let golden = wire_client::catalogue::get(id)?;
            if result.scenario_sha256 != golden.trace().header.scenario_sha256 {
                return Ok(State::Stale);
            }
        }
        let summary = self.summary(&behavior.id, false);
        if summary.rule.raw_records > 0 {
            return Ok(State::Different(summary));
        }
        if missing {
            return Ok(State::NotRun);
        }
        if self.audit.is_some() && self.summary(&behavior.id, true).rule.raw_records > 0 {
            return Ok(State::UnstableReference(summary));
        }
        if self.working_tree_dirty {
            return Ok(State::Unverified(summary));
        }
        Ok(State::Match(summary))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum State {
    NoScenario,
    NotRun,
    Stale,
    Different(Summary),
    Unverified(Summary),
    UnstableReference(Summary),
    Match(Summary),
}

impl State {
    pub fn is_match(&self) -> bool {
        matches!(self, Self::Match(_))
    }
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoScenario => f.write_str("no scenario"),
            Self::NotRun => f.write_str("not run"),
            Self::Stale => f.write_str("stale"),
            Self::Different(summary) => write!(f, "{summary}"),
            Self::Match(summary) => write!(f, "pass; {summary}"),
            Self::Unverified(summary) => write!(f, "unverified (dirty tree); {summary}"),
            Self::UnstableReference(summary) => write!(f, "reference changed; {summary}"),
        }
    }
}

// A report-only commit cannot change the core that an audit measured.
pub fn same_core(root: &Path, measured: &str, head: &str) -> Result<bool> {
    let ancestor = Command::new("git")
        .current_dir(root)
        .args(["merge-base", "--is-ancestor", measured, head])
        .status()?;
    if !ancestor.success() {
        return Ok(false);
    }
    let diff = Command::new("git")
        .current_dir(root)
        .args([
            "diff",
            "--quiet",
            measured,
            head,
            "--",
            ".",
            ":(exclude)conformance/reports/**",
        ])
        .status()?;
    ensure!(
        matches!(diff.code(), Some(0 | 1)),
        "could not compare core revisions"
    );
    Ok(diff.success())
}

pub fn latest_audit(
    directory: &Path,
    excluded_stem: Option<&str>,
) -> Result<Option<(PathBuf, Report)>> {
    if !directory.exists() {
        return Ok(None);
    }
    let mut latest: Option<(PathBuf, Report)> = None;
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if excluded_stem.is_some_and(|excluded| {
            path.file_stem().and_then(|stem| stem.to_str()) == Some(excluded)
        }) {
            continue;
        }
        if path.extension().is_some_and(|ext| ext == "json") {
            let report = Report::load_structure(&path, "conformance_audit")?;
            if latest.as_ref().is_none_or(|(old_path, old)| {
                (report.recorded_at_unix_ms, &path) > (old.recorded_at_unix_ms, old_path)
            }) {
                latest = Some((path, report));
            }
        }
    }
    Ok(latest)
}

#[cfg(test)]
#[path = "evidence_tests.rs"]
mod tests;
