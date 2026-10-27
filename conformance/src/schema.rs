use std::collections::BTreeSet;
use std::path::{Component, Path};

use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Behavior {
    pub schema_version: u32,
    pub id: String,
    pub system: String,
    pub scope: String,
    pub requests: Vec<String>,
    pub replies: Vec<String>,
    pub dependencies: Vec<String>,
    pub vmangos: SourceReference,
    pub required_rungs: Vec<Rung>,
    pub scenarios: Vec<String>,
    pub deliberate_deviations: Vec<DeliberateDeviation>,
    #[serde(default)]
    pub data_fields: Vec<DataDependentField>,
    pub real_client: RealClient,
    pub prerequisites: Vec<String>,
    pub tests: Vec<TestEvidence>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceReference {
    pub commit: String,
    pub path: String,
    pub symbol: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeliberateDeviation {
    pub id: String,
    pub result: String,
    pub reason: String,
    pub source: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DataDependentField {
    pub scenario: String,
    pub checkpoint: String,
    pub stream: DataStream,
    pub opcode: u16,
    pub field_path: Vec<String>,
    pub value_type: DataValueType,
    pub reason: String,
    pub source: SourceReference,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    content = "id",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum DataStream {
    Reply(String),
    Broadcast(String),
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DataValueType {
    Unsigned,
    Signed,
    Float,
    TextSha256,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum RealClient {
    // Serde ignores extra fields on an internally tagged unit variant.
    Unverified {},
    Verified {
        date: String,
        checked_by: String,
        evidence: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rung {
    Unit,
    Codec,
    DurableModule,
    Tier1,
    ConformanceAudit,
    RealClient,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TestEvidence {
    pub path: String,
    pub function: String,
    pub rung: Rung,
}

impl Behavior {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == 1,
            "unsupported schema_version: {}",
            self.schema_version
        );
        identifier(&self.id)?;
        identifier(&self.system)?;
        nonempty(&self.scope, "scope")?;
        ensure!(
            !self.requests.is_empty() || !self.replies.is_empty(),
            "Behavior has no opcodes"
        );
        for (direction, opcodes) in [("requests", &self.requests), ("replies", &self.replies)] {
            unique(opcodes, direction)?;
            for opcode in opcodes {
                ensure!(opcode.len() == 6 && opcode.starts_with("0x") && opcode[2..].bytes().all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b)),
                    "invalid {direction} opcode {opcode:?}; use 0x followed by four uppercase hexadecimal digits");
            }
        }
        unique(&self.dependencies, "dependencies")?;
        for id in &self.dependencies {
            identifier(id)?;
        }
        self.vmangos.validate()?;
        ensure!(!self.required_rungs.is_empty(), "required_rungs is empty");
        unique(&self.required_rungs, "required_rungs")?;
        unique(&self.scenarios, "scenarios")?;
        for id in &self.scenarios {
            nonempty(id, "scenario id")?;
        }
        let mut deviations = BTreeSet::new();
        for deviation in &self.deliberate_deviations {
            identifier(&deviation.id)?;
            ensure!(
                deviations.insert(&deviation.id),
                "duplicate Deliberate Deviation: {}",
                deviation.id
            );
            nonempty(&deviation.result, "deviation result")?;
            nonempty(&deviation.reason, "deviation reason")?;
            ensure!(
                deviation.source.starts_with("https://")
                    && deviation.source.len() > 8
                    && !deviation.source.contains(char::is_whitespace),
                "deviation source must be an HTTPS URL"
            );
        }
        self.validate_data_fields()?;
        self.real_client.validate()?;
        for prerequisite in &self.prerequisites {
            nonempty(prerequisite, "prerequisite")?;
        }
        let mut tests = BTreeSet::new();
        for test in &self.tests {
            relative_path(&test.path)?;
            identifier(&test.function)?;
            ensure!(
                tests.insert((&test.path, &test.function)),
                "duplicate test evidence: {}::{}",
                test.path,
                test.function
            );
            ensure!(
                matches!(test.rung, Rung::Unit | Rung::Codec | Rung::DurableModule),
                "Rust source evidence must use unit, codec or durable_module"
            );
        }
        Ok(())
    }

    fn validate_data_fields(&self) -> Result<()> {
        let mut fields = BTreeSet::new();
        for field in &self.data_fields {
            ensure!(
                self.scenarios.contains(&field.scenario),
                "data field names an unlisted scenario"
            );
            identifier(&field.checkpoint)?;
            ensure!(
                field.checkpoint != "opening",
                "opening fields are not compared"
            );
            let (DataStream::Reply(stream) | DataStream::Broadcast(stream)) = &field.stream;
            identifier(stream)?;
            ensure!(
                self.replies.contains(&format!("0x{:04X}", field.opcode)),
                "data field names an unclaimed reply opcode"
            );
            ensure!(
                !field.field_path.is_empty(),
                "data field needs an exact field path"
            );
            for part in &field.field_path {
                if part
                    .parse::<usize>()
                    .is_ok_and(|index| index.to_string() == *part)
                {
                    continue;
                }
                identifier(part)?;
            }
            ensure!(
                fields.insert((
                    &field.scenario,
                    &field.checkpoint,
                    &field.stream,
                    field.opcode,
                    &field.field_path
                )),
                "duplicate data field"
            );
            nonempty(&field.reason, "data field reason")?;
            field.source.validate()?;
        }
        Ok(())
    }
}

impl SourceReference {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.commit.len() == 40
                && self.commit.bytes().all(|b| b.is_ascii_hexdigit())
                && self.commit.bytes().any(|b| b != b'0'),
            "vmangos commit must be a full nonzero SHA"
        );
        relative_path(&self.path)?;
        nonempty(&self.symbol, "vmangos symbol")?;
        Ok(())
    }
}

impl RealClient {
    fn validate(&self) -> Result<()> {
        if let RealClient::Verified {
            date,
            checked_by,
            evidence,
        } = self
        {
            validate_date(date)?;
            nonempty(checked_by, "real-client reviewer")?;
            nonempty(evidence, "real-client evidence")?;
        }
        Ok(())
    }
}

fn nonempty(value: &str, name: &str) -> Result<()> {
    ensure!(!value.trim().is_empty(), "{name} is empty");
    Ok(())
}

fn identifier(value: &str) -> Result<()> {
    ensure!(
        value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
            && value
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
        "invalid identifier: {value:?}"
    );
    Ok(())
}

fn unique<T: Ord>(values: &[T], name: &str) -> Result<()> {
    ensure!(
        values.iter().collect::<BTreeSet<_>>().len() == values.len(),
        "duplicate {name}"
    );
    Ok(())
}

fn relative_path(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && !value.contains('\\')
            && Path::new(value)
                .components()
                .all(|part| matches!(part, Component::Normal(_))),
        "expected a relative source path: {value:?}"
    );
    Ok(())
}

fn validate_date(date: &str) -> Result<()> {
    let parts: Vec<_> = date.split('-').collect();
    ensure!(
        parts.len() == 3
            && parts[0].len() == 4
            && parts[1].len() == 2
            && parts[2].len() == 2
            && parts.iter().all(|p| p.bytes().all(|b| b.is_ascii_digit())),
        "real-client date must be YYYY-MM-DD"
    );
    let year: u32 = parts[0].parse()?;
    let month: u32 = parts[1].parse()?;
    let day: u32 = parts[2].parse()?;
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        _ => 0,
    };
    ensure!(
        year > 0 && day > 0 && day <= days,
        "invalid real-client date: {date}"
    );
    Ok(())
}
