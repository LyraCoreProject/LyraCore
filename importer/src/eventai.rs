#[path = "eventai_vmangos.rs"]
pub(crate) mod vmangos;

use std::collections::BTreeSet;

const DEFINITION_REVISION_DOMAIN: &[u8] = b"lyracore-eventai-definition-v1";
const DEFINITION_BATCH_BYTES: usize = 64 * 1024;
const RELAY_MAX_DEPTH: usize = 16;
const RELAY_STEP_BUDGET: usize = 4_096;
const RELAY_SCHEDULED_WORK_BUDGET: usize = 2_048;
const RELAY_CATALOGUE_VERSION_PLACEHOLDER: &str = "{relay-catalogue-version}";
const FIXTURE_ID_FIRST: u32 = 5_099_000;
const FIXTURE_ID_LAST: u32 = 5_099_999;

#[derive(Default)]
pub(crate) struct EventAiPlan {
    pub(crate) definition_rows: Vec<String>,
    pub(crate) definition_batches: Vec<String>,
    pub(crate) broadcast_rows: Vec<String>,
    pub(crate) summon_rows: Vec<String>,
    pub(crate) quest_event_entries: BTreeSet<u32>,
    pub(crate) relay_definition_rows: Vec<String>,
    pub(crate) relay_definition_batches: Vec<String>,
    pub(crate) forced_template_entries: BTreeSet<u64>,
    pub(crate) emitted_instructions: u64,
}

impl EventAiPlan {
    pub(crate) fn definition_count(&self) -> u64 {
        self.definition_rows.len() as u64
    }
    pub(crate) fn instruction_count(&self) -> u64 {
        self.emitted_instructions
    }
    pub(crate) fn row_count(&self) -> u64 {
        (self.definition_rows.len()
            + self.broadcast_rows.len()
            + self.summon_rows.len()
            + self.relay_definition_rows.len()) as u64
    }
}
fn encoded_catalogue_version(rows: &[String]) -> u64 {
    let mut versions = rows
        .iter()
        .map(|row| {
            let mut fields = row.split('@');
            let relay_id = fields
                .next()
                .expect("encoded relay has an id")
                .parse::<u32>()
                .expect("encoded relay id is u32");
            let version = fields
                .next()
                .expect("encoded relay has a version")
                .parse::<u64>()
                .expect("encoded relay version is u64");
            (relay_id, version)
        })
        .collect::<Vec<_>>();
    versions.sort_unstable();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"lyracore-relay-catalogue-v1");
    for (relay_id, version) in versions {
        hasher.update(&relay_id.to_le_bytes());
        hasher.update(&version.to_le_bytes());
    }
    u64::from_le_bytes(hasher.finalize().as_bytes()[..8].try_into().unwrap())
}
fn encode_definition(entry: u32, rules: &[String]) -> String {
    let subject = format!("entry:{entry}");
    let rules = rules.join("~");
    let material = format!("{subject}@{rules}");
    let mut hasher = blake3::Hasher::new();
    hasher.update(DEFINITION_REVISION_DOMAIN);
    hasher.update(material.as_bytes());
    let revision = u64::from_le_bytes(
        hasher.finalize().as_bytes()[..8]
            .try_into()
            .expect("a BLAKE3 digest has at least eight bytes"),
    );
    format!("{subject}@{revision}@{rules}")
}
fn pack_definition_batches(rows: &[String]) -> Vec<String> {
    let mut batches = Vec::new();
    let mut batch = String::new();
    for row in rows {
        let separator = usize::from(!batch.is_empty());
        if !batch.is_empty() && batch.len() + separator + row.len() > DEFINITION_BATCH_BYTES {
            batches.push(std::mem::take(&mut batch));
        }
        if !batch.is_empty() {
            batch.push('\n');
        }
        batch.push_str(row);
    }
    if !batch.is_empty() || batches.is_empty() {
        batches.push(batch);
    }
    batches
}
pub(crate) fn fixture_id(id: u32) -> bool {
    (FIXTURE_ID_FIRST..=FIXTURE_ID_LAST).contains(&id)
}
