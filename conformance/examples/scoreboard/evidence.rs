use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};
use lyracore_conformance::Behavior;
use serde::Deserialize;
use wire_client::compare::{Comparison, Difference};
use wire_client::trace::Sha256Digest;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    schema_version: u32,
    pub tier: String,
    pub core_commit: String,
    pub working_tree_dirty: bool,
    pub harness_revision: String,
    pub recorded_at_unix_ms: u64,
    scenarios: Vec<ScenarioResult>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScenarioResult {
    id: String,
    behavior: String,
    scenario_sha256: Sha256Digest,
    comparison: Comparison,
    #[serde(default)]
    data_differences: Vec<Difference>,
}

impl Report {
    fn parse(bytes: &[u8], tier: &str) -> Result<Self> {
        let report: Self = serde_json::from_slice(bytes)?;
        ensure!(
            report.schema_version == 1 && report.tier == tier,
            "wrong evidence schema or tier"
        );
        for sha in [&report.core_commit, &report.harness_revision] {
            ensure!(
                sha.len() == 40 && sha.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "invalid evidence revision"
            );
        }
        ensure!(report.recorded_at_unix_ms > 0, "missing evidence time");
        let mut ids = BTreeSet::new();
        for scenario in &report.scenarios {
            ensure!(
                wire_client::scenario::identifier(&scenario.id) && ids.insert(&scenario.id),
                "invalid or duplicate result scenario id"
            );
            ensure!(
                wire_client::scenario::identifier(&scenario.behavior),
                "invalid result Behavior"
            );
            ensure!(
                tier != "tier1" || scenario.data_differences.is_empty(),
                "tier 1 cannot classify data differences"
            );
        }
        Ok(report)
    }

    pub fn load(path: &Path, tier: &str) -> Result<Self> {
        Self::parse(&std::fs::read(path)?, tier).with_context(|| format!("read {}", path.display()))
    }

    pub fn state(&self, behavior: &Behavior, head: &str, revision: &str) -> Result<State> {
        if behavior.scenarios.is_empty() {
            return Ok(State::NoScenario);
        }
        if self.core_commit != head || self.harness_revision != revision {
            return Ok(State::Stale);
        }
        let mut rules = 0;
        let mut data = 0;
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
            rules += result.comparison.differences.len();
            data += result.data_differences.len();
        }
        if rules > 0 {
            return Ok(State::Different { rules, data });
        }
        if missing {
            return Ok(State::NotRun);
        }
        Ok(State::Match {
            data,
            dirty: self.working_tree_dirty,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum State {
    NoScenario,
    NotRun,
    Stale,
    Different { rules: usize, data: usize },
    Match { data: usize, dirty: bool },
}

impl State {
    pub fn is_match(&self) -> bool {
        matches!(self, Self::Match { .. })
    }
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoScenario => f.write_str("no scenario"),
            Self::NotRun => f.write_str("not run"),
            Self::Stale => f.write_str("stale"),
            Self::Different { rules, data } => write!(f, "{rules} rule, {data} data"),
            Self::Match { data, dirty } => {
                write!(f, "pass{}", if *dirty { "*" } else { "" })?;
                if *data > 0 {
                    write!(f, "; {data} data")?;
                }
                Ok(())
            }
        }
    }
}

pub fn latest_audit(directory: &Path) -> Result<Option<(PathBuf, Report)>> {
    if !directory.exists() {
        return Ok(None);
    }
    let mut latest: Option<(PathBuf, Report)> = None;
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            let report = Report::load(&path, "conformance_audit")?;
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
mod tests {
    use super::*;
    use serde_json::json;

    fn example() -> (Behavior, serde_json::Value) {
        let ledger = lyracore_conformance::FeatureLedger::from_sources([(
            "connection/connection_identity.toml",
            include_str!("../../features/connection/connection_identity.toml"),
        )])
        .unwrap();
        let behavior = ledger.behaviors().first().unwrap();
        let behavior = serde_json::from_value(serde_json::to_value(behavior).unwrap()).unwrap();
        let golden = wire_client::catalogue::get("connection_identity").unwrap();
        (
            behavior,
            json!({
                "schema_version": 1, "tier": "tier1", "core_commit": "1".repeat(40),
                "harness_revision": "2".repeat(40), "working_tree_dirty": false,
                "recorded_at_unix_ms": 1,
                "scenarios": [{ "id": "connection_identity", "behavior": "connection_identity",
                    "scenario_sha256": golden.trace().header.scenario_sha256,
                    "comparison": { "differences": [] } }],
            }),
        )
    }

    #[test]
    fn evidence_requires_current_revisions_and_every_scenario() {
        let (behavior, mut value) = example();
        let report = Report::parse(&serde_json::to_vec(&value).unwrap(), "tier1").unwrap();
        assert_eq!(
            report
                .state(&behavior, &"1".repeat(40), &"2".repeat(40))
                .unwrap(),
            State::Match {
                data: 0,
                dirty: false
            }
        );
        assert_eq!(
            report
                .state(&behavior, &"3".repeat(40), &"2".repeat(40))
                .unwrap(),
            State::Stale
        );
        assert_eq!(
            report
                .state(&behavior, &"1".repeat(40), &"3".repeat(40))
                .unwrap(),
            State::Stale
        );
        value["scenarios"] = json!([]);
        let report = Report::parse(&serde_json::to_vec(&value).unwrap(), "tier1").unwrap();
        assert_eq!(
            report
                .state(&behavior, &"1".repeat(40), &"2".repeat(40))
                .unwrap(),
            State::NotRun
        );
    }

    #[test]
    fn counted_deviations_remain_differences_and_audit_data_stays_separate() {
        let (behavior, mut value) = example();
        let difference = json!({"checkpoint":"ping", "stream":null, "packet_index":null,
            "field_path":[], "opcode":477, "deviations":[{"name":"chosen", "reason":"reason", "source":"source"}],
            "rule":"exact_field", "reason":"differs", "expected":null, "actual":null});
        value["scenarios"][0]["comparison"]["differences"] = json!([difference]);
        let report = Report::parse(&serde_json::to_vec(&value).unwrap(), "tier1").unwrap();
        assert_eq!(
            report
                .state(&behavior, &"1".repeat(40), &"2".repeat(40))
                .unwrap(),
            State::Different { rules: 1, data: 0 }
        );
        value["tier"] = json!("conformance_audit");
        value["scenarios"][0]["data_differences"] =
            value["scenarios"][0]["comparison"]["differences"].take();
        value["scenarios"][0]["comparison"]["differences"] = json!([]);
        let report =
            Report::parse(&serde_json::to_vec(&value).unwrap(), "conformance_audit").unwrap();
        assert_eq!(
            report
                .state(&behavior, &"1".repeat(40), &"2".repeat(40))
                .unwrap(),
            State::Match {
                data: 1,
                dirty: false
            }
        );
    }

    #[test]
    fn malformed_reports_cannot_look_like_missing_or_passing_evidence() {
        let (_, mut value) = example();
        value["scenarios"][0]["comparison"] = json!({});
        assert!(Report::parse(&serde_json::to_vec(&value).unwrap(), "tier1").is_err());
        let (_, mut value) = example();
        let duplicate = value["scenarios"][0].clone();
        value["scenarios"].as_array_mut().unwrap().push(duplicate);
        assert!(Report::parse(&serde_json::to_vec(&value).unwrap(), "tier1").is_err());
    }
    #[test]
    fn changed_scenario_hashes_are_stale_and_wrong_behaviors_fail() {
        let (behavior, mut value) = example();
        value["scenarios"][0]["scenario_sha256"] = json!("0".repeat(64));
        let report = Report::parse(&serde_json::to_vec(&value).unwrap(), "tier1").unwrap();
        assert_eq!(
            report
                .state(&behavior, &"1".repeat(40), &"2".repeat(40))
                .unwrap(),
            State::Stale
        );
        value["scenarios"][0]["behavior"] = json!("wrong_behavior");
        let report = Report::parse(&serde_json::to_vec(&value).unwrap(), "tier1").unwrap();
        assert!(report
            .state(&behavior, &"1".repeat(40), &"2".repeat(40))
            .is_err());
    }

    #[test]
    fn latest_audit_uses_capture_time_and_reports_malformed_evidence() {
        let directory = PathBuf::from(std::env::var_os("HOME").unwrap())
            .join(".cache/lyracore-scratch/conformance")
            .join(format!("scoreboard-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let (_, mut value) = example();
        value["tier"] = json!("conformance_audit");
        std::fs::write(
            directory.join("z.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        value["recorded_at_unix_ms"] = json!(2);
        std::fs::write(
            directory.join("a.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        assert_eq!(
            latest_audit(&directory)
                .unwrap()
                .unwrap()
                .0
                .file_name()
                .unwrap(),
            "a.json"
        );
        std::fs::write(directory.join("a.json"), b"{}").unwrap();
        assert!(latest_audit(&directory).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
