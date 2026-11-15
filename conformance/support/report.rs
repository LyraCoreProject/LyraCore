// Includers declare `mod ratchet` and bring `Violation` into scope beside this module.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use wire_client::compare::Comparison;
use wire_client::trace::{Provenance, Sha256Digest};

use super::ratchet::Signature;
use super::Violation;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct SchemaVersion;

impl TryFrom<u32> for SchemaVersion {
    type Error = String;

    fn try_from(version: u32) -> std::result::Result<Self, String> {
        if version == 2 {
            Ok(Self)
        } else {
            Err(format!("unsupported conformance result schema {version}"))
        }
    }
}

impl From<SchemaVersion> for u32 {
    fn from(_: SchemaVersion) -> Self {
        2
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub schema_version: SchemaVersion,
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
    pub properties: Option<Vec<Violation>>,
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

impl Report {
    /// Fails unless this is `tier` evidence from `core_commit` at harness pin `harness_revision`.
    pub fn ensure_current(
        &self,
        tier: &str,
        core_commit: &str,
        harness_revision: &str,
    ) -> Result<()> {
        ensure!(
            self.tier == tier,
            "expected {tier} evidence, found {}",
            self.tier
        );
        ensure!(
            self.core_commit == core_commit,
            "{tier} evidence is from another commit"
        );
        ensure!(
            self.harness_revision == harness_revision,
            "{tier} evidence uses another harness revision"
        );
        Ok(())
    }
}
