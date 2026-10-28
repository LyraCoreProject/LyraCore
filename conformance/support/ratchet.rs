use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use wire_client::compare::{Comparison, Difference, DifferenceClass};
use wire_client::scenario::Stream;
use wire_client::trace::Sha256Digest;

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

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioBaseline {
    pub scenario: String,
    pub behavior: String,
    pub harness_revision: String,
    pub scenario_sha256: Sha256Digest,
    pub rule: Vec<Signature>,
    pub data: Vec<Signature>,
}

impl ScenarioBaseline {
    pub fn from_comparison(
        scenario: &str,
        behavior: &str,
        harness_revision: &str,
        scenario_sha256: &Sha256Digest,
        comparison: &Comparison,
    ) -> Self {
        let signatures = |class| {
            comparison
                .differences
                .iter()
                .filter(|difference| difference.classification == class)
                .map(Signature::from)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        };
        Self {
            scenario: scenario.into(),
            behavior: behavior.into(),
            harness_revision: harness_revision.into(),
            scenario_sha256: scenario_sha256.clone(),
            rule: signatures(DifferenceClass::Rule),
            data: signatures(DifferenceClass::Data),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Baseline {
    pub schema_version: u32,
    pub scenarios: Vec<ScenarioBaseline>,
}

#[derive(Debug, Serialize)]
pub struct SignatureChanges {
    pub added: Vec<Signature>,
    pub removed: Vec<Signature>,
}

#[derive(Debug, Serialize)]
pub struct BaselineChanges {
    pub rule: SignatureChanges,
    pub data: SignatureChanges,
}

impl BaselineChanges {
    pub fn is_empty(&self) -> bool {
        self.rule.added.is_empty()
            && self.rule.removed.is_empty()
            && self.data.added.is_empty()
            && self.data.removed.is_empty()
    }
}

impl Baseline {
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        Self::parse(&bytes).with_context(|| format!("validate {}", path.display()))
    }

    fn parse(bytes: &[u8]) -> Result<Self> {
        let baseline: Self = serde_json::from_slice(bytes)?;
        ensure!(
            baseline.schema_version == 1,
            "unsupported tier 1 baseline schema"
        );
        ensure!(
            bytes == baseline.canonical_bytes()?,
            "tier 1 baseline is not canonical"
        );
        ensure!(
            baseline
                .scenarios
                .windows(2)
                .all(|pair| pair[0].scenario < pair[1].scenario),
            "tier 1 baseline scenarios must be unique and sorted"
        );
        for entry in &baseline.scenarios {
            for signatures in [&entry.rule, &entry.data] {
                ensure!(
                    signatures.windows(2).all(|pair| pair[0] < pair[1]),
                    "{} baseline signatures must be unique and sorted",
                    entry.scenario
                );
            }
        }
        Ok(baseline)
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    pub fn changes(&self, current: &ScenarioBaseline) -> Result<BaselineChanges> {
        let committed = self
            .scenarios
            .iter()
            .find(|entry| entry.scenario == current.scenario)
            .with_context(|| format!("no tier 1 baseline for {}", current.scenario))?;
        ensure!(
            committed.behavior == current.behavior,
            "Behavior changed for {}",
            current.scenario
        );
        let class_changes = |old: &[Signature], new: &[Signature]| SignatureChanges {
            added: new.iter().filter(|s| !old.contains(s)).cloned().collect(),
            removed: old.iter().filter(|s| !new.contains(s)).cloned().collect(),
        };
        Ok(BaselineChanges {
            rule: class_changes(&committed.rule, &current.rule),
            data: class_changes(&committed.data, &current.data),
        })
    }

    pub fn check(&self, current: &ScenarioBaseline) -> Result<()> {
        let committed = self
            .scenarios
            .iter()
            .find(|entry| entry.scenario == current.scenario)
            .with_context(|| format!("no tier 1 baseline for {}", current.scenario))?;
        ensure!(
            committed.harness_revision == current.harness_revision
                && committed.scenario_sha256 == current.scenario_sha256,
            "{} tier 1 baseline provenance differs; regenerate with ./conformance/baseline",
            current.scenario
        );
        let changes = self.changes(current)?;
        ensure!(
            changes.is_empty(),
            "{} tier 1 baseline differs; rule added {:?}, removed {:?}; data added {:?}, removed {:?}. Regenerate with ./conformance/baseline",
            current.scenario,
            changes.rule.added,
            changes.rule.removed,
            changes.data.added,
            changes.data.removed
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scenario_baseline(
        scenario: &str,
        behavior: &str,
        comparison: &Comparison,
    ) -> ScenarioBaseline {
        ScenarioBaseline::from_comparison(
            scenario,
            behavior,
            "revision",
            &Sha256Digest::of(b"scenario"),
            comparison,
        )
    }

    fn difference(rule: &str) -> Difference {
        Difference {
            classification: DifferenceClass::Rule,
            checkpoint: "reply".into(),
            stream: None,
            packet_index: None,
            field_path: vec!["value".into()],
            opcode: Some(1),
            deviations: vec![],
            rule: rule.into(),
            reason: "reason excluded from baseline".into(),
            expected: None,
            actual: None,
        }
    }

    #[test]
    fn new_signature_fails_until_regeneration() -> Result<()> {
        let original = scenario_baseline(
            "scenario",
            "behavior",
            &Comparison {
                differences: vec![],
            },
        );
        let mut baseline = Baseline {
            schema_version: 1,
            scenarios: vec![original],
        };
        let changed = scenario_baseline(
            "scenario",
            "behavior",
            &Comparison {
                differences: vec![difference("two")],
            },
        );
        assert!(baseline.check(&changed).is_err());
        baseline.scenarios[0] = changed.clone();
        let bytes = baseline.canonical_bytes()?;
        assert!(!String::from_utf8_lossy(&bytes).contains("reason excluded"));
        Baseline::parse(&bytes)?.check(&changed)?;
        Ok(())
    }

    #[test]
    fn removed_signature_fails_until_regeneration() -> Result<()> {
        let original = scenario_baseline(
            "scenario",
            "behavior",
            &Comparison {
                differences: vec![difference("one")],
            },
        );
        let baseline = Baseline {
            schema_version: 1,
            scenarios: vec![original],
        };
        let removed = scenario_baseline(
            "scenario",
            "behavior",
            &Comparison {
                differences: vec![],
            },
        );
        assert!(baseline.check(&removed).is_err());
        Ok(())
    }

    #[test]
    fn data_signature_change_fails() {
        let baseline = Baseline {
            schema_version: 1,
            scenarios: vec![scenario_baseline(
                "scenario",
                "behavior",
                &Comparison {
                    differences: vec![],
                },
            )],
        };
        let mut data_difference = difference("data_field");
        data_difference.classification = DifferenceClass::Data;
        let current = scenario_baseline(
            "scenario",
            "behavior",
            &Comparison {
                differences: vec![data_difference],
            },
        );
        assert_eq!(current.data.len(), 1);
        assert!(baseline.check(&current).is_err());
    }

    #[test]
    fn provenance_change_fails_until_regeneration() -> Result<()> {
        let original = scenario_baseline(
            "scenario",
            "behavior",
            &Comparison {
                differences: vec![difference("same")],
            },
        );
        for changed in [
            ScenarioBaseline {
                harness_revision: "new revision".into(),
                ..original.clone()
            },
            ScenarioBaseline {
                scenario_sha256: Sha256Digest::of(b"changed scenario"),
                ..original.clone()
            },
        ] {
            let mut baseline = Baseline {
                schema_version: 1,
                scenarios: vec![original.clone()],
            };
            let error = baseline.check(&changed).unwrap_err().to_string();
            assert!(error.contains("regenerate with ./conformance/baseline"));
            baseline.scenarios[0] = changed.clone();
            Baseline::parse(&baseline.canonical_bytes()?)?.check(&changed)?;
        }
        Ok(())
    }

    #[test]
    fn hand_edited_baseline_fails() -> Result<()> {
        let current = scenario_baseline(
            "scenario",
            "behavior",
            &Comparison {
                differences: vec![difference("a"), difference("b")],
            },
        );
        let mut baseline = Baseline {
            schema_version: 1,
            scenarios: vec![current.clone()],
        };
        let mut bytes = baseline.canonical_bytes()?;
        bytes.push(b' ');
        assert!(Baseline::parse(&bytes).is_err());
        baseline.scenarios[0].rule[0].rule = "aa".into();
        assert!(Baseline::parse(&baseline.canonical_bytes()?)?
            .check(&current)
            .is_err());
        Ok(())
    }

    #[test]
    fn unsorted_baseline_fails() -> Result<()> {
        let current = scenario_baseline(
            "scenario",
            "behavior",
            &Comparison {
                differences: vec![difference("a"), difference("b")],
            },
        );
        let mut baseline = Baseline {
            schema_version: 1,
            scenarios: vec![current],
        };
        baseline.scenarios[0].rule.reverse();
        assert!(Baseline::parse(&baseline.canonical_bytes()?).is_err());
        Ok(())
    }
}
