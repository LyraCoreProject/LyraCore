use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};
use lyracore_conformance::{Behavior, DataStream, DataValueType, FeatureLedger};
use serde::Deserialize;
use wire_client::compare::{Comparison, Difference};
use wire_client::scenario::Stream;
use wire_client::trace::{RecordedField, Sha256Digest};

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScenarioResult {
    id: String,
    behavior: String,
    scenario_sha256: Sha256Digest,
    comparison: Comparison,
    #[serde(default)]
    data_differences: Vec<Difference>,
}

impl ScenarioResult {
    fn counts(&self, behavior: &Behavior, tier: &str) -> Result<(usize, usize)> {
        let mut rules = 0;
        let mut data = 0;
        for (claimed_data, differences) in [
            (false, &self.comparison.differences),
            (true, &self.data_differences),
        ] {
            for difference in differences {
                let derived_data = tier == "conformance_audit"
                    && is_data_difference(behavior, &self.id, difference);
                ensure!(
                    claimed_data == derived_data,
                    "scenario {}: cannot re-derive {} classification for {} {:?} ({})",
                    self.id,
                    if claimed_data { "data" } else { "rule" },
                    difference.checkpoint,
                    difference.field_path,
                    difference.rule
                );
                if derived_data {
                    data += 1;
                } else {
                    rules += 1;
                }
            }
        }
        Ok((rules, data))
    }
}

fn is_data_difference(behavior: &Behavior, scenario: &str, difference: &Difference) -> bool {
    if difference.rule != "exact_field"
        || !difference.deviations.is_empty()
        || difference.packet_index.is_none()
    {
        return false;
    }
    behavior.data_fields.iter().any(|field| {
        field.scenario == scenario
            && field.checkpoint == difference.checkpoint
            && Some(field.opcode) == difference.opcode
            && field.field_path == difference.field_path
            && matches!((&field.stream, &difference.stream),
                (DataStream::Reply(expected), Some(Stream::Reply(actual)))
                | (DataStream::Broadcast(expected), Some(Stream::Broadcast(actual))) if expected == actual)
            && [difference.expected.as_ref(), difference.actual.as_ref()]
                .into_iter()
                .all(|value| matches!((field.value_type, value),
                    (DataValueType::Unsigned, Some(RecordedField::Unsigned(_)))
                    | (DataValueType::Signed, Some(RecordedField::Signed(_)))
                    | (DataValueType::Float, Some(RecordedField::Float(_)))
                    | (DataValueType::TextSha256, Some(RecordedField::TextSha256(_)))))
    })
}

impl Report {
    fn parse(bytes: &[u8], tier: &str, ledger: &FeatureLedger) -> Result<Self> {
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
            let behavior = ledger
                .behaviors()
                .iter()
                .find(|behavior| behavior.id == scenario.behavior)
                .context("result names an unknown Behavior")?;
            ensure!(
                behavior.scenarios.contains(&scenario.id),
                "result scenario is not listed by its Behavior"
            );
            scenario.counts(behavior, tier)?;
        }
        Ok(report)
    }

    pub fn load(path: &Path, tier: &str, ledger: &FeatureLedger) -> Result<Self> {
        Self::parse(&std::fs::read(path)?, tier, ledger)
            .with_context(|| format!("read {}", path.display()))
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
            let (rule_count, data_count) = result.counts(behavior, &self.tier)?;
            rules += rule_count;
            data += data_count;
        }
        if rules > 0 {
            return Ok(State::Different { rules, data });
        }
        if missing {
            return Ok(State::NotRun);
        }
        if self.working_tree_dirty {
            return Ok(State::Unverified { data });
        }
        Ok(State::Match { data })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum State {
    NoScenario,
    NotRun,
    Stale,
    Different { rules: usize, data: usize },
    Unverified { data: usize },
    Match { data: usize },
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
            Self::Match { data } | Self::Unverified { data } => {
                f.write_str(if self.is_match() {
                    "pass"
                } else {
                    "unverified (dirty tree)"
                })?;
                if *data > 0 {
                    write!(f, "; {data} data")?;
                }
                Ok(())
            }
        }
    }
}

pub fn latest_audit(directory: &Path, ledger: &FeatureLedger) -> Result<Option<(PathBuf, Report)>> {
    if !directory.exists() {
        return Ok(None);
    }
    let mut latest: Option<(PathBuf, Report)> = None;
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            let report = Report::load(&path, "conformance_audit", ledger)?;
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
    use serde_json::{json, Value};

    fn example() -> (FeatureLedger, Value) {
        let ledger = FeatureLedger::from_sources([(
            "connection/connection_identity.toml",
            include_str!("../../features/connection/connection_identity.toml"),
        )])
        .unwrap();
        let golden = wire_client::catalogue::get("connection_identity").unwrap();
        (
            ledger,
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

    fn parse(value: &Value, ledger: &FeatureLedger) -> Result<Report> {
        Report::parse(
            &serde_json::to_vec(value)?,
            value["tier"].as_str().unwrap(),
            ledger,
        )
    }

    fn state(value: &Value, ledger: &FeatureLedger) -> State {
        parse(value, ledger)
            .unwrap()
            .state(&ledger.behaviors()[0], &"1".repeat(40), &"2".repeat(40))
            .unwrap()
    }

    fn audit_example() -> (FeatureLedger, Value, Value) {
        let source = format!(
            "{}\n{}",
            include_str!("../../features/taxi/taxi_single_hop.toml"),
            r#"
[[data_fields]]
scenario = "taxi_single_hop"
checkpoint = "flight"
stream = { kind = "reply", id = "flight" }
opcode = 199
field_path = ["position", "x"]
value_type = "float"
reason = "The destination coordinate comes from the node catalogue."
source = { commit = "d9173eca3ea08dd985c52dabbcc826898d1d0b77", path = "src/game/Handlers/TaxiHandler.cpp", symbol = "WorldSession::HandleActivateTaxiOpcode" }
"#
        );
        let ledger = FeatureLedger::from_sources([
            ("taxi/taxi_single_hop.toml", source.as_str()),
            (
                "connection/connection_identity.toml",
                include_str!("../../features/connection/connection_identity.toml"),
            ),
        ])
        .unwrap();
        let (_, mut value) = example();
        value["tier"] = json!("conformance_audit");
        value["scenarios"][0]["id"] = json!("taxi_single_hop");
        value["scenarios"][0]["behavior"] = json!("taxi_single_hop");
        value["scenarios"][0]["scenario_sha256"] = json!(
            wire_client::catalogue::get("taxi_single_hop")
                .unwrap()
                .trace()
                .header
                .scenario_sha256
        );
        let difference = json!({
            "checkpoint": "flight", "stream": {"kind":"reply", "id":"flight"},
            "packet_index": 6, "field_path": ["position", "x"], "opcode": 199,
            "deviations": [], "rule": "exact_field", "reason": "field differs",
            "expected": {"type":"float", "value":1.0},
            "actual": {"type":"float", "value":2.0}
        });
        (ledger, value, difference)
    }

    #[test]
    fn evidence_requires_current_revisions_and_every_scenario() {
        let (ledger, mut value) = example();
        assert_eq!(state(&value, &ledger), State::Match { data: 0 });
        for field in ["core_commit", "harness_revision"] {
            let mut stale = value.clone();
            stale[field] = json!("3".repeat(40));
            assert_eq!(state(&stale, &ledger), State::Stale);
        }
        value["scenarios"] = json!([]);
        assert_eq!(state(&value, &ledger), State::NotRun);
    }

    #[test]
    fn dirty_matches_are_unverified_and_do_not_count_as_passes() {
        let (ledger, mut value) = example();
        value["working_tree_dirty"] = json!(true);
        for tier in ["tier1", "conformance_audit"] {
            value["tier"] = json!(tier);
            let state = state(&value, &ledger);
            assert_eq!(state, State::Unverified { data: 0 });
            assert!(!state.is_match());
            assert_eq!(state.to_string(), "unverified (dirty tree)");
        }
    }

    #[test]
    fn audit_classification_comes_from_the_sourced_ledger_field() {
        let (ledger, mut value, difference) = audit_example();
        value["scenarios"][0]["data_differences"] = json!([difference]);
        let behavior = ledger
            .behaviors()
            .iter()
            .find(|b| b.id == "taxi_single_hop")
            .unwrap();
        assert_eq!(
            parse(&value, &ledger)
                .unwrap()
                .state(behavior, &"1".repeat(40), &"2".repeat(40))
                .unwrap(),
            State::Match { data: 1 }
        );
        let undeclared = FeatureLedger::from_sources([
            (
                "taxi/taxi_single_hop.toml",
                include_str!("../../features/taxi/taxi_single_hop.toml"),
            ),
            (
                "connection/connection_identity.toml",
                include_str!("../../features/connection/connection_identity.toml"),
            ),
        ])
        .unwrap();
        assert!(parse(&value, &undeclared)
            .unwrap_err()
            .to_string()
            .contains("cannot re-derive data"));
        value["scenarios"][0]["data_differences"] = json!([]);
        value["scenarios"][0]["comparison"]["differences"] = json!([difference]);
        assert!(parse(&value, &ledger)
            .unwrap_err()
            .to_string()
            .contains("cannot re-derive rule"));
        value["tier"] = json!("tier1");
        assert_eq!(
            parse(&value, &ledger)
                .unwrap()
                .state(behavior, &"1".repeat(40), &"2".repeat(40))
                .unwrap(),
            State::Different { rules: 1, data: 0 }
        );
        value["scenarios"][0]["comparison"]["differences"] = json!([]);
        value["scenarios"][0]["data_differences"] = json!([difference]);
        assert!(parse(&value, &ledger).is_err());
    }

    #[test]
    fn rule_failures_cannot_be_relabelled_as_data() {
        let (ledger, value, difference) = audit_example();
        for (field, replacement) in [
            ("checkpoint", json!("another_checkpoint")),
            ("opcode", json!(221)),
            ("stream", json!({"kind":"broadcast", "id":"flight"})),
            ("stream", json!({"kind":"reply", "id":"another_reply"})),
            ("stream", Value::Null),
            ("packet_index", Value::Null),
            ("field_path", json!(["position", "y"])),
            ("field_path", json!(["position"])),
            ("rule", json!("packet_order")),
            ("rule", json!("packet_count")),
            ("rule", json!("field_shape")),
            ("rule", json!("guid_bijection")),
            ("rule", json!("landing_delivery")),
            ("expected", Value::Null),
            ("actual", Value::Null),
            ("actual", json!({"type":"unsigned", "value":2})),
            ("actual", json!({"type":"list", "value":[]})),
            (
                "deviations",
                json!([{"name":"chosen", "reason":"reason", "source":"source"}]),
            ),
        ] {
            let mut report = value.clone();
            let mut changed = difference.clone();
            changed[field] = replacement;
            report["scenarios"][0]["comparison"]["differences"] = json!([changed]);
            assert!(parse(&report, &ledger).is_ok(), "rule rejected for {field}");
            report["scenarios"][0]["comparison"]["differences"] = json!([]);
            report["scenarios"][0]["data_differences"] = json!([changed]);
            assert!(
                parse(&report, &ledger)
                    .unwrap_err()
                    .to_string()
                    .contains("cannot re-derive data"),
                "accepted {field}"
            );
        }
    }

    #[test]
    fn known_gaps_neither_excuse_rules_nor_change_passes() {
        let (_, mut value) = example();
        let source = format!(
            "{}\n{}",
            include_str!("../../features/connection/connection_identity.toml"),
            r#"
[[known_gaps]]
description = "Missing reply."
path = "module/src/native/mod.rs"
symbol = "native_world_receive"
evidence = "missing_packet record"
"#
        );
        let ledger =
            FeatureLedger::from_sources([("connection/connection_identity.toml", source.as_str())])
                .unwrap();
        assert!(state(&value, &ledger).is_match());
        value["tier"] = json!("conformance_audit");
        let (_, _, difference) = audit_example();
        value["scenarios"][0]["comparison"]["differences"] = json!([difference]);
        assert_eq!(
            state(&value, &ledger),
            State::Different { rules: 1, data: 0 }
        );
        value["scenarios"][0]["comparison"]["differences"] = json!([]);
        value["scenarios"][0]["data_differences"] = json!([difference]);
        assert!(parse(&value, &ledger).is_err());
    }

    #[test]
    fn changed_scenario_hashes_are_stale_and_malformed_results_fail() {
        let (ledger, value) = example();
        let mut stale = value.clone();
        stale["scenarios"][0]["scenario_sha256"] = json!("0".repeat(64));
        assert_eq!(state(&stale, &ledger), State::Stale);
        for (field, replacement) in [
            ("comparison", json!({})),
            ("behavior", json!("wrong_behavior")),
            ("id", json!("unlisted_scenario")),
        ] {
            let mut malformed = value.clone();
            malformed["scenarios"][0][field] = replacement;
            assert!(parse(&malformed, &ledger).is_err());
        }
        let mut duplicate = value.clone();
        duplicate["scenarios"]
            .as_array_mut()
            .unwrap()
            .push(value["scenarios"][0].clone());
        assert!(parse(&duplicate, &ledger).is_err());
    }

    #[test]
    fn extra_unlisted_results_cannot_hide_unchecked_data_classification() {
        let (ledger, mut value) = example();
        value["tier"] = json!("conformance_audit");
        let mut unlisted = value["scenarios"][0].clone();
        unlisted["id"] = json!("unlisted");
        value["scenarios"].as_array_mut().unwrap().push(unlisted);
        assert!(parse(&value, &ledger).is_err());
    }

    #[test]
    fn latest_audit_uses_capture_time_and_rejects_malformed_evidence() {
        let directory = PathBuf::from(std::env::var_os("HOME").unwrap())
            .join(".cache/lyracore-scratch/conformance")
            .join(format!("scoreboard-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let (ledger, mut value) = example();
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
            latest_audit(&directory, &ledger)
                .unwrap()
                .unwrap()
                .0
                .file_name()
                .unwrap(),
            "a.json"
        );
        std::fs::write(directory.join("a.json"), b"{}").unwrap();
        assert!(latest_audit(&directory, &ledger).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
