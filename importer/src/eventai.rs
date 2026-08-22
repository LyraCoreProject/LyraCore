//! CMaNGOS EventAI import boundary.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::{bt, field, parse_table, sql_text, world_guid};

pub(crate) const SOURCE_PROFILE_NAME: &str = "cmangos-classic-z2815";
pub(crate) const LOADER_CONTRACT: &str = "creature-ai-scripts-24-columns-ordered-by-id-v1";
const SOURCE_PROFILE_JSON: &str = include_str!("../manifests/cmangos-classic-z2815.json");
const DEFINITION_REVISION_DOMAIN: &[u8] = b"lyracore-eventai-definition-v1";
const DEFINITION_BATCH_BYTES: usize = 64 * 1024;

const EVENT_TIMER_IN_COMBAT: u32 = 0;
const EVENT_HP: u32 = 2;
const EVENT_AGGRO: u32 = 4;
const EVENT_DEATH: u32 = 6;
const EVENT_RANGE: u32 = 9;
const EVENT_SPAWNED: u32 = 11;
const EVENT_FRIENDLY_HP: u32 = 14;

const ACTION_TEXT: u32 = 1;
const ACTION_TEXT_NEW: u32 = 54;
const ACTION_EMOTE: u32 = 5;
const ACTION_CAST: u32 = 11;
const ACTION_SPAWN: u32 = 12;
const ACTION_THREAT_SINGLE: u32 = 13;
const ACTION_QUEST_EVENT: u32 = 15;
const ACTION_CAST_EVENT: u32 = 16;
const ACTION_SET_UNIT_FIELD: u32 = 17;
const ACTION_SET_UNIT_FLAG: u32 = 18;
const ACTION_REMOVE_UNIT_FLAG: u32 = 19;
const ACTION_SET_PHASE: u32 = 22;
const ACTION_FLEE_FOR_ASSIST: u32 = 25;
const ACTION_REMOVE_AURA: u32 = 28;
const ACTION_RANGED_MOVEMENT: u32 = 29;
const ACTION_SUMMON_ID: u32 = 32;
const ACTION_KILLED_MONSTER: u32 = 33;
const ACTION_SET_INSTANCE_DATA_GUID: u32 = 35;
const ACTION_CALL_FOR_HELP: u32 = 39;
const ACTION_THROW_AI_EVENT: u32 = 45;
const ACTION_START_RELAY: u32 = 53;
const ACTION_ATTACK_START: u32 = 55;
const ACTION_SET_FACING: u32 = 59;
const ACTION_END: u16 = 65;

const TARGET_SELF: u32 = 0;
const TARGET_HOSTILE: u32 = 1;
const TARGET_HOSTILE_SECOND: u32 = 2;
const TARGET_HOSTILE_RANDOM: u32 = 4;
const TARGET_ACTION_INVOKER: u32 = 6;
const TARGET_HOSTILE_RANDOM_PLAYER: u32 = 8;
const TARGET_EVENT_SPECIFIC: u32 = 12;
const TARGET_NEAREST_AOE: u32 = 17;
const TARGET_HOSTILE_FARTHEST: u32 = 18;

const FLAG_REPEATABLE: u32 = 0x01;
const FLAG_RANDOM_ACTION: u32 = 0x20;
const FLAG_COMBAT_ACTION: u32 = 0x400;
const SUPPORTED_FLAGS: u32 = FLAG_REPEATABLE | FLAG_RANDOM_ACTION | FLAG_COMBAT_ACTION;

const CAST_INTERRUPT_PREVIOUS: u32 = 0x01;
const CAST_TRIGGERED: u32 = 0x02;
const CAST_FORCE_CAST: u32 = 0x04;
const CAST_AURA_NOT_PRESENT: u32 = 0x20;
const CAST_PLAYER_ONLY: u32 = 0x200;
const CAST_TARGET_CASTING: u32 = 0x800;
const SUPPORTED_CAST_FLAGS: u32 = CAST_INTERRUPT_PREVIOUS
    | CAST_TRIGGERED
    | CAST_FORCE_CAST
    | CAST_AURA_NOT_PRESENT
    | CAST_PLAYER_ONLY
    | CAST_TARGET_CASTING;

const FIXTURE_ID_FIRST: u32 = 5_099_000;
const FIXTURE_ID_LAST: u32 = 5_099_999;

#[derive(Default)]
pub(crate) struct EventAiPlan {
    pub(crate) definition_rows: Vec<String>,
    pub(crate) definition_batches: Vec<String>,
    pub(crate) broadcast_rows: Vec<String>,
    pub(crate) summon_rows: Vec<String>,
    pub(crate) forced_template_entries: BTreeSet<u64>,
    coverage: Coverage,
}

impl EventAiPlan {
    pub(crate) fn definition_count(&self) -> u64 {
        self.definition_rows.len() as u64
    }

    pub(crate) fn instruction_count(&self) -> u64 {
        self.coverage.emitted_instructions
    }

    pub(crate) fn row_count(&self) -> u64 {
        (self.definition_rows.len() + self.broadcast_rows.len() + self.summon_rows.len()) as u64
    }

    pub(crate) fn compatibility_manifest(
        &self,
        profile: &SourceProfile,
        observed_sql_sha256: &str,
        observed_loader_contract: &str,
    ) -> CompatibilityManifest {
        CompatibilityManifest::build(
            profile,
            observed_sql_sha256,
            observed_loader_contract,
            &self.coverage,
            self.definition_rows.len(),
            self.broadcast_rows.len(),
            self.summon_rows.len(),
        )
    }

    #[cfg(test)]
    pub(crate) fn dropped(&self, reason: &str, value: u64) -> u64 {
        if let Some(rules) = self
            .coverage
            .dropped_rule_values
            .get(&(reason.to_string(), value))
        {
            return rules.len() as u64;
        }
        let direct = self
            .coverage
            .dropped_values
            .get(&(reason.to_string(), value))
            .copied()
            .unwrap_or(0);
        if direct != 0 || reason != "unsupported_action" {
            return direct;
        }
        self.coverage
            .groups
            .iter()
            .filter(|(key, _)| {
                key.dimension == "action"
                    && key.raw_value == value.to_string()
                    && key.reason == "invalid_action_to_none"
            })
            .flat_map(|(_, result)| result.source_rule_ids.iter())
            .copied()
            .collect::<BTreeSet<_>>()
            .len() as u64
    }

    #[cfg(test)]
    pub(crate) fn event_counts(&self, value: u64) -> (u64, u64, u64, u64) {
        self.coverage.counts(
            value,
            &self.coverage.event,
            &self.coverage.accepted_event,
            &self.coverage.emitted_event,
        )
    }

    #[cfg(test)]
    pub(crate) fn action_counts(&self, value: u64) -> (u64, u64, u64, u64) {
        self.coverage.counts(
            value,
            &self.coverage.action,
            &self.coverage.accepted_action,
            &self.coverage.emitted_action,
        )
    }
}

#[derive(Clone)]
pub(crate) struct SourceProfile {
    pub(crate) name: String,
    pub(crate) cmangos_commit: String,
    pub(crate) classic_db_commit: String,
    pub(crate) sql_sha256: String,
    pub(crate) loader_contract: String,
    pub(crate) source_rule_count: u64,
    pub(crate) source_guid_rule_count: u64,
    approvals: ApprovalRules,
    expected_source_census: BTreeMap<String, BTreeMap<u64, u64>>,
}

#[derive(Clone, Default)]
struct ApprovalRules {
    classifications: BTreeSet<String>,
    events: BTreeSet<u64>,
    actions: BTreeSet<u64>,
    targets: BTreeSet<u64>,
    event_flag_bits: u32,
    cast_flag_bits: u32,
    dependencies: BTreeSet<String>,
    reasons: BTreeSet<String>,
    normalizations: BTreeSet<(String, String, String)>,
}

pub(crate) fn source_profile(name: &str) -> Result<SourceProfile, String> {
    if name != SOURCE_PROFILE_NAME {
        return Err(format!("unknown EventAI source profile: {name}"));
    }
    let root: serde_json::Value = serde_json::from_str(SOURCE_PROFILE_JSON)
        .map_err(|error| format!("parse EventAI source profile: {error}"))?;
    if root
        .get("format_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        return Err("EventAI source profile format_version must be 1".to_string());
    }
    let profile = root
        .get("profile")
        .and_then(serde_json::Value::as_object)
        .ok_or("EventAI source profile has no profile object")?;
    let approved = root
        .get("approved_results")
        .and_then(serde_json::Value::as_object)
        .ok_or("EventAI source profile has no approved_results object")?;
    let string = |key: &str| -> Result<String, String> {
        profile
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("EventAI source profile field is not a string: {key}"))
    };
    let count = |key: &str| -> Result<u64, String> {
        profile
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| format!("EventAI source profile field is not a count: {key}"))
    };
    let strings = |key: &str| -> Result<BTreeSet<String>, String> {
        approved
            .get(key)
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| format!("EventAI approval field is not an array: {key}"))?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("EventAI approval value is not a string: {key}"))
            })
            .collect()
    };
    let numbers = |key: &str| -> Result<BTreeSet<u64>, String> {
        approved
            .get(key)
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| format!("EventAI approval field is not an array: {key}"))?
            .iter()
            .map(|value| {
                value
                    .as_u64()
                    .ok_or_else(|| format!("EventAI approval value is not an integer: {key}"))
            })
            .collect()
    };
    let bit_mask = |key: &str| -> Result<u32, String> {
        numbers(key)?.into_iter().try_fold(0u32, |mask, bit| {
            let bit = u32::try_from(bit)
                .map_err(|_| format!("EventAI approval bit does not fit u32: {bit}"))?;
            if !bit.is_power_of_two() {
                return Err(format!("EventAI approval value is not one bit: {bit:#x}"));
            }
            Ok(mask | bit)
        })
    };
    let normalizations = approved
        .get("normalizations")
        .and_then(serde_json::Value::as_array)
        .ok_or("EventAI approval field is not an array: normalizations")?
        .iter()
        .map(|value| {
            let value = value
                .as_object()
                .ok_or("EventAI normalization approval is not an object")?;
            let field = |key: &str| {
                value
                    .get(key)
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| {
                        format!("EventAI normalization approval field is not a string: {key}")
                    })
            };
            Ok((field("dimension")?, field("raw_value")?, field("reason")?))
        })
        .collect::<Result<BTreeSet<_>, String>>()?;
    let expected_source_census = root
        .get("expected_source_census")
        .and_then(serde_json::Value::as_object)
        .map(|dimensions| {
            dimensions
                .iter()
                .map(|(dimension, values)| {
                    let values = values
                        .as_object()
                        .ok_or_else(|| format!("source census is not an object: {dimension}"))?
                        .iter()
                        .map(|(value, count)| {
                            let value = value.parse::<u64>().map_err(|_| {
                                format!("source census value is not an integer: {value}")
                            })?;
                            let count = count.as_u64().ok_or_else(|| {
                                format!(
                                    "source census count is not an integer: {dimension}/{value}"
                                )
                            })?;
                            Ok((value, count))
                        })
                        .collect::<Result<BTreeMap<_, _>, String>>()?;
                    Ok((dimension.clone(), values))
                })
                .collect::<Result<BTreeMap<_, _>, String>>()
        })
        .transpose()?
        .unwrap_or_default();
    let source_profile = SourceProfile {
        name: string("name")?,
        cmangos_commit: string("cmangos_commit")?,
        classic_db_commit: string("classic_db_commit")?,
        sql_sha256: string("sql_sha256")?,
        loader_contract: string("loader_contract")?,
        source_rule_count: count("source_rule_count")?,
        source_guid_rule_count: count("source_guid_rule_count")?,
        approvals: ApprovalRules {
            classifications: strings("classifications")?,
            events: numbers("events")?,
            actions: numbers("actions")?,
            targets: numbers("targets")?,
            event_flag_bits: bit_mask("event_flag_bits")?,
            cast_flag_bits: bit_mask("cast_flag_bits")?,
            dependencies: strings("dependencies")?,
            reasons: strings("reasons")?,
            normalizations,
        },
        expected_source_census,
    };
    if source_profile.name != name {
        return Err(format!(
            "EventAI source profile name mismatch: requested={name} stored={}",
            source_profile.name
        ));
    }
    if source_profile.loader_contract != LOADER_CONTRACT {
        return Err(format!(
            "EventAI loader contract mismatch: code={LOADER_CONTRACT} profile={}",
            source_profile.loader_contract
        ));
    }
    source_profile.check_census()?;
    Ok(source_profile)
}

#[derive(Clone, Debug)]
pub(crate) struct CompatibilityManifest {
    rendered: String,
    findings: Vec<String>,
}

impl CompatibilityManifest {
    pub(crate) fn render(&self) -> &str {
        &self.rendered
    }

    pub(crate) fn is_apply_ready(&self) -> bool {
        self.findings.is_empty()
    }

    pub(crate) fn require_apply_ready(&self) -> Result<(), String> {
        if self.is_apply_ready() {
            Ok(())
        } else {
            Err(format!(
                "EventAI compatibility manifest is not approved: {}",
                self.findings.join("; ")
            ))
        }
    }

    fn build(
        profile: &SourceProfile,
        observed_sql_sha256: &str,
        observed_loader_contract: &str,
        coverage: &Coverage,
        definition_count: usize,
        broadcast_count: usize,
        summon_count: usize,
    ) -> Self {
        let mut findings = Vec::new();
        if observed_sql_sha256 != profile.sql_sha256 {
            findings.push(format!(
                "sql_sha256 expected={} observed={observed_sql_sha256}",
                profile.sql_sha256
            ));
        }
        if observed_loader_contract != profile.loader_contract {
            findings.push(format!(
                "loader_contract expected={} observed={observed_loader_contract}",
                profile.loader_contract
            ));
        }
        if coverage.total_rules != profile.source_rule_count {
            findings.push(format!(
                "source_rule_count expected={} observed={}",
                profile.source_rule_count, coverage.total_rules
            ));
        }
        if coverage.source_guid_rules != profile.source_guid_rule_count {
            findings.push(format!(
                "source_guid_rule_count expected={} observed={}",
                profile.source_guid_rule_count, coverage.source_guid_rules
            ));
        }
        let classified_rules = coverage.classified_rules();
        if classified_rules != coverage.total_rules {
            findings.push(format!(
                "source_classification_count expected={} observed={classified_rules}",
                coverage.total_rules
            ));
        }
        for (dimension, expected) in &profile.expected_source_census {
            let observed = coverage.source_census(dimension);
            if &observed != expected {
                findings.push(format!(
                    "source_{dimension}_census expected={} observed={}",
                    render_census(expected),
                    render_census(&observed)
                ));
            }
        }
        for (key, value) in &coverage.groups {
            if !profile.approves(key) {
                findings.push(format!(
                    "unapproved {} raw={} classification={} reason={} rules={}",
                    key.dimension,
                    key.raw_value,
                    key.classification,
                    key.reason,
                    render_rule_ids(&value.source_rule_ids),
                ));
            }
        }
        let unapproved_result_groups = coverage
            .groups
            .keys()
            .filter(|key| !profile.approves(key))
            .count();
        findings.sort();
        findings.dedup();

        let groups = coverage
            .groups
            .iter()
            .map(|(key, value)| {
                serde_json::json!({
                    "dimension": key.dimension,
                    "raw_value": key.raw_value,
                    "classification": key.classification,
                    "reason": key.reason,
                    "occurrences": value.occurrences,
                    "source_rule_ids": value.source_rule_ids,
                    "dependency_paths": value.dependency_paths,
                })
            })
            .collect::<Vec<_>>();
        let rendered = serde_json::to_string_pretty(&serde_json::json!({
            "format_version": 1,
            "profile": {
                "name": profile.name,
                "cmangos_commit": profile.cmangos_commit,
                "classic_db_commit": profile.classic_db_commit,
                "expected_sql_sha256": profile.sql_sha256,
                "observed_sql_sha256": observed_sql_sha256,
                "expected_loader_contract": profile.loader_contract,
                "observed_loader_contract": observed_loader_contract,
            },
            "counts": {
                "source_rules": coverage.total_rules,
                "source_guid_rules": coverage.source_guid_rules,
                "classified_rules": classified_rules,
                "unclassified_rules": coverage.total_rules.saturating_sub(classified_rules),
                "emitted_rules": coverage.emitted_rules,
                "emitted_instructions": coverage.emitted_instructions,
                "normalized_rules": coverage.normalized_rules,
                "excluded_rules": coverage.excluded_rules,
                "dropped_rules": coverage.dropped_rules,
                "unapproved_result_groups": unapproved_result_groups,
                "definitions": definition_count,
                "broadcast_texts": broadcast_count,
                "summon_locations": summon_count,
            },
            "source_census": coverage.source_census_json(),
            "results": groups,
            "apply_ready": findings.is_empty(),
            "findings": findings,
        }))
        .expect("the compatibility manifest contains JSON values");
        Self { rendered, findings }
    }
}

impl SourceProfile {
    fn check_census(&self) -> Result<(), String> {
        let required = ["events", "actions", "targets", "event_flags", "cast_flags"];
        for dimension in required {
            if !self.expected_source_census.contains_key(dimension) {
                return Err(format!(
                    "EventAI source profile has no {dimension} source census"
                ));
            }
        }
        for dimension in ["events", "event_flags"] {
            let count = self.expected_source_census[dimension].values().sum::<u64>();
            if count != self.source_rule_count {
                return Err(format!(
                    "EventAI {dimension} census count mismatch: profile={} census={count}",
                    self.source_rule_count
                ));
            }
        }
        let actions = &self.expected_source_census["actions"];
        let cast_count = actions.get(&u64::from(ACTION_CAST)).copied().unwrap_or(0);
        let cast_flag_count = self.expected_source_census["cast_flags"]
            .values()
            .sum::<u64>();
        if cast_count != cast_flag_count {
            return Err(format!(
                "EventAI cast flag census count mismatch: actions={cast_count} flags={cast_flag_count}"
            ));
        }
        let target_count = actions
            .iter()
            .filter(|(action, _)| {
                u32::try_from(**action)
                    .ok()
                    .and_then(source_target_parameter)
                    .is_some()
            })
            .map(|(_, count)| *count)
            .sum::<u64>();
        let observed_target_count = self.expected_source_census["targets"].values().sum::<u64>();
        if target_count != observed_target_count {
            return Err(format!(
                "EventAI target census count mismatch: actions={target_count} targets={observed_target_count}"
            ));
        }
        Ok(())
    }

    fn approves(&self, key: &GroupKey) -> bool {
        if key.classification == "excluded" && key.reason == "outside_world_import_scope" {
            return true;
        }
        if matches!(
            key.reason.as_str(),
            "chance_capped" | "invalid_action_to_none"
        ) {
            return key.classification == "normalized"
                && self.approvals.classifications.contains(&key.classification)
                && self.approvals.reasons.contains(&key.reason)
                && self.approvals.normalizations.contains(&(
                    key.dimension.clone(),
                    key.raw_value.clone(),
                    key.reason.clone(),
                ));
        }
        if !self.approvals.classifications.contains(&key.classification)
            || !self.approvals.reasons.contains(&key.reason)
        {
            return false;
        }
        match key.dimension.as_str() {
            "event" => key
                .raw_value
                .parse()
                .is_ok_and(|value| self.approvals.events.contains(&value)),
            "action" => key
                .raw_value
                .parse()
                .is_ok_and(|value| self.approvals.actions.contains(&value)),
            "target" => key
                .raw_value
                .parse()
                .is_ok_and(|value| self.approvals.targets.contains(&value)),
            "event_flag" => mask_has_no_residual(&key.raw_value, self.approvals.event_flag_bits),
            "cast_flag" => mask_has_no_residual(&key.raw_value, self.approvals.cast_flag_bits),
            "dependency" => self.approvals.dependencies.contains(&key.raw_value),
            "rule" => true,
            _ => false,
        }
    }
}

fn mask_has_no_residual(raw_value: &str, approved_bits: u32) -> bool {
    raw_value
        .parse::<u32>()
        .is_ok_and(|mask| mask & !approved_bits == 0)
}

fn render_census(values: &BTreeMap<u64, u64>) -> String {
    values
        .iter()
        .map(|(value, count)| format!("{value}={count}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn render_rule_ids(values: &BTreeSet<u64>) -> String {
    values
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

#[derive(Clone, Default)]
struct Coverage {
    total_rules: u64,
    source_guid_rules: u64,
    emitted_rules: u64,
    emitted_instructions: u64,
    normalized_rules: u64,
    excluded_rules: u64,
    dropped_rules: u64,
    accepted_rules: u64,
    action_rows: u64,
    event: BTreeMap<u64, u64>,
    action: BTreeMap<u64, u64>,
    accepted_event: BTreeMap<u64, u64>,
    accepted_action: BTreeMap<u64, u64>,
    emitted_event: BTreeMap<u64, u64>,
    emitted_action: BTreeMap<u64, u64>,
    target: BTreeMap<u64, u64>,
    source_target: BTreeMap<u64, u64>,
    flags: BTreeMap<u64, u64>,
    cast_flags: BTreeMap<u64, u64>,
    dropped: BTreeMap<String, u64>,
    dropped_values: BTreeMap<(String, u64), u64>,
    dropped_rule_values: BTreeMap<(String, u64), BTreeSet<u64>>,
    groups: BTreeMap<GroupKey, GroupValue>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct GroupKey {
    dimension: String,
    raw_value: String,
    classification: String,
    reason: String,
}

#[derive(Clone, Debug, Default)]
struct GroupValue {
    occurrences: u64,
    source_rule_ids: BTreeSet<u64>,
    dependency_paths: BTreeSet<String>,
}

impl Coverage {
    fn source_value(values: &mut BTreeMap<u64, u64>, value: u64) {
        *values.entry(value).or_default() += 1;
    }

    fn drop(&mut self, reason: &str, value: u64) {
        *self.dropped.entry(reason.to_string()).or_default() += 1;
        *self
            .dropped_values
            .entry((reason.to_string(), value))
            .or_default() += 1;
        let legacy_reason = legacy_drop_reason(reason);
        if let Some(legacy_reason) = legacy_reason {
            *self
                .dropped_values
                .entry((legacy_reason.to_string(), value))
                .or_default() += 1;
        }
    }

    fn drop_for_rule(&mut self, reason: &str, value: u64, rule_id: u64) {
        self.drop(reason, value);
        self.dropped_rule_values
            .entry((reason.to_string(), value))
            .or_default()
            .insert(rule_id);
        if let Some(legacy_reason) = legacy_drop_reason(reason) {
            self.dropped_rule_values
                .entry((legacy_reason.to_string(), value))
                .or_default()
                .insert(rule_id);
        }
    }

    fn result(
        &mut self,
        dimension: &str,
        raw_value: impl ToString,
        classification: &str,
        reason: &str,
        source_rule_id: u64,
        dependency_path: Option<String>,
    ) {
        let result = self
            .groups
            .entry(GroupKey {
                dimension: dimension.to_string(),
                raw_value: raw_value.to_string(),
                classification: classification.to_string(),
                reason: reason.to_string(),
            })
            .or_default();
        result.occurrences += 1;
        if source_rule_id != 0 {
            result.source_rule_ids.insert(source_rule_id);
        }
        if let Some(path) = dependency_path {
            result.dependency_paths.insert(path);
        }
    }

    fn classify_rule(&mut self, rule_id: u64, classification: &str, reason: &str) {
        match classification {
            "emitted" => self.emitted_rules += 1,
            "normalized" => self.normalized_rules += 1,
            "excluded" => self.excluded_rules += 1,
            "dropped" => self.dropped_rules += 1,
            _ => {}
        }
        self.result("rule", rule_id, classification, reason, rule_id, None);
    }

    fn classified_rules(&self) -> u64 {
        self.emitted_rules + self.normalized_rules + self.excluded_rules + self.dropped_rules
    }

    fn source_census(&self, dimension: &str) -> BTreeMap<u64, u64> {
        match dimension {
            "events" => self.event.clone(),
            "actions" => self.action.clone(),
            "targets" => self.source_target.clone(),
            "event_flags" => self.flags.clone(),
            "cast_flags" => self.cast_flags.clone(),
            _ => BTreeMap::new(),
        }
    }

    fn source_census_json(&self) -> serde_json::Value {
        serde_json::json!({
            "events": self.event,
            "actions": self.action,
            "targets": self.source_target,
            "event_flags": self.flags,
            "cast_flags": self.cast_flags,
        })
    }

    #[cfg(test)]
    fn counts(
        &self,
        value: u64,
        source: &BTreeMap<u64, u64>,
        accepted: &BTreeMap<u64, u64>,
        emitted: &BTreeMap<u64, u64>,
    ) -> (u64, u64, u64, u64) {
        let source = source.get(&value).copied().unwrap_or(0);
        let accepted = accepted.get(&value).copied().unwrap_or(0);
        (
            source,
            accepted,
            source.saturating_sub(accepted),
            emitted.get(&value).copied().unwrap_or(0),
        )
    }
}

fn legacy_drop_reason(reason: &str) -> Option<&'static str> {
    if reason.starts_with("unsupported_event_flag_residual_") {
        Some("unsupported_flag")
    } else if reason == "missing:broadcast_text" {
        Some("missing_broadcast_text")
    } else if reason == "missing:summon_creature" {
        Some("missing_summon_creature")
    } else if reason == "missing:summon_location" {
        Some("missing_summon_location")
    } else if reason.starts_with("unsupported_text_template_") {
        Some("unsupported_text_template")
    } else if reason == "unsupported_chat_type:broadcast_text" {
        Some("unsupported_chat_type")
    } else {
        None
    }
}

struct RawRule {
    id: u64,
    subject: i32,
    event: u32,
    inverse_phase_mask: u32,
    chance: u8,
    flags: u32,
    params: [u32; 6],
    actions: [[u32; 4]; 3],
    normalizations: Vec<SourceNormalization>,
}

struct SourceNormalization {
    dimension: &'static str,
    raw_value: u64,
    reason: &'static str,
}

#[derive(Clone)]
struct Broadcast {
    male: String,
    female: String,
    chat_type: u8,
    language: u8,
    emotes: [(u32, u32); 3],
}

struct SummonLocation {
    x: f32,
    y: f32,
    z: f32,
    orientation: f32,
    lifetime_ms: u32,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Subject {
    Entry(u32),
    Guid(u64),
}

impl Subject {
    fn encode(self) -> String {
        match self {
            Self::Entry(entry) => format!("entry:{entry}"),
            Self::Guid(guid) => format!("guid:{guid}"),
        }
    }
}

struct NativeAction {
    encoded: String,
    raw_kind: u32,
    raw_target: Option<u32>,
    raw_cast_flags: Option<u32>,
    texts: Vec<u32>,
    summon_entry: Option<u64>,
    summon_location: Option<u32>,
    dependencies: Vec<Dependency>,
}

struct Dependency {
    kind: &'static str,
    path: String,
}

struct MappingFailure {
    dimension: &'static str,
    raw_value: u64,
    reason: String,
    dependency_path: Option<String>,
    dependency_kind: Option<&'static str>,
}

impl MappingFailure {
    fn source(dimension: &'static str, raw_value: u64, reason: impl Into<String>) -> Self {
        Self {
            dimension,
            raw_value,
            reason: reason.into(),
            dependency_path: None,
            dependency_kind: None,
        }
    }

    fn dependency(
        kind: &'static str,
        raw_value: u64,
        reason: impl Into<String>,
        path: String,
    ) -> Self {
        Self {
            dimension: "dependency",
            raw_value,
            reason: format!("{}:{kind}", reason.into()),
            dependency_path: Some(path),
            dependency_kind: Some(kind),
        }
    }
}

/// The dump's EventAI tables, parsed once. Only rule assembly depends on the World Import Scope, and
/// the scope grows to a fixpoint while summoned templates pull in more entries, so the caller
/// re-runs [`EventAiSource::assemble`] against the grown scope rather than re-reading the dump.
pub(crate) struct EventAiSource {
    broadcasts: BTreeMap<u32, Broadcast>,
    summon_locations: BTreeMap<u32, SummonLocation>,
    rules: Vec<RawRule>,
    coverage: Coverage,
}

pub(crate) fn parse(dump: &str) -> EventAiSource {
    let mut coverage = Coverage::default();
    let mut broadcasts = parse_broadcasts(dump, &mut coverage);
    broadcasts.extend(parse_legacy_texts(dump, &broadcasts, &mut coverage));
    let summon_locations = parse_summons(dump, &mut coverage);
    let mut rules = parse_rules(dump, &mut coverage);
    rules.sort_by_key(|rule| rule.id);
    EventAiSource {
        broadcasts,
        summon_locations,
        rules,
        coverage,
    }
}

impl EventAiSource {
    pub(crate) fn assemble(
        &self,
        imported_entries: &HashSet<u64>,
        imported_guid_entries: &HashMap<u64, u64>,
        importable_templates: &HashSet<u64>,
    ) -> EventAiPlan {
        let broadcasts = &self.broadcasts;
        let summon_locations = &self.summon_locations;
        let mut plan = EventAiPlan {
            coverage: self.coverage.clone(),
            ..EventAiPlan::default()
        };
        let mut used_texts = BTreeSet::new();
        let mut used_summons = BTreeSet::new();
        let mut definitions: BTreeMap<Subject, Vec<String>> = BTreeMap::new();

        for rule in &self.rules {
            let subject = match resolve_subject(
                rule,
                imported_entries,
                imported_guid_entries,
                importable_templates,
            ) {
                SubjectResolution::Included {
                    subject,
                    dependencies,
                } => (subject, dependencies),
                SubjectResolution::Excluded => {
                    plan.coverage
                        .classify_rule(rule.id, "excluded", "outside_world_import_scope");
                    record_rule_dimensions(
                        &mut plan.coverage,
                        rule,
                        "excluded",
                        "outside_world_import_scope",
                    );
                    for normalization in &rule.normalizations {
                        plan.coverage.result(
                            normalization.dimension,
                            normalization.raw_value,
                            "excluded",
                            normalization.reason,
                            rule.id,
                            None,
                        );
                    }
                    continue;
                }
                SubjectResolution::Failed(failure) => {
                    record_dropped_rule(&mut plan.coverage, rule, vec![failure]);
                    continue;
                }
            };
            let mut failures = Vec::new();
            let event = match map_event(rule) {
                Ok(event) => Some(event),
                Err(failure) => {
                    failures.push(failure);
                    None
                }
            };
            if rule.chance == 0 {
                failures.push(MappingFailure::source(
                    "rule",
                    rule.chance as u64,
                    "invalid_chance",
                ));
            }
            let flag_residual = rule.flags & !SUPPORTED_FLAGS;
            if flag_residual != 0 {
                failures.push(MappingFailure::source(
                    "event_flag",
                    rule.flags as u64,
                    format!("unsupported_event_flag_residual_{flag_residual:#x}"),
                ));
            }
            let allowed_phase_mask = !rule.inverse_phase_mask;
            if allowed_phase_mask == 0 {
                failures.push(MappingFailure::source(
                    "rule",
                    rule.inverse_phase_mask as u64,
                    "empty_phase_mask",
                ));
            }
            let mut actions = Vec::new();
            for (slot, action) in rule.actions.iter().enumerate() {
                let raw_action = action[0];
                if raw_action == 0 {
                    continue;
                }
                match map_action(
                    *action,
                    rule.id,
                    slot,
                    broadcasts,
                    summon_locations,
                    importable_templates,
                ) {
                    Ok(action) => actions.push(action),
                    Err(mut action_failures) => failures.append(&mut action_failures),
                }
            }
            if actions.is_empty() {
                failures.push(MappingFailure::source("rule", rule.id, "empty_rule"));
            }
            if rule.id == 0 {
                failures.push(MappingFailure::source("rule", rule.id, "invalid_rule_id"));
            }
            if !failures.is_empty() {
                record_dropped_rule(&mut plan.coverage, rule, failures);
                continue;
            }
            let event = event.expect("a rule without mapping failures has an event");
            let recurrence = if rule.flags & FLAG_REPEATABLE != 0 {
                format!("repeat:{}:{}", rule.params[2], rule.params[3])
            } else {
                "once".to_string()
            };
            let selection = if rule.flags & FLAG_RANDOM_ACTION != 0 {
                "random"
            } else {
                "all"
            };
            let execution = if rule.flags & FLAG_COMBAT_ACTION != 0 {
                "combat"
            } else {
                "ordinary"
            };
            let instructions = actions
                .iter()
                .map(|action| action.encoded.as_str())
                .collect::<Vec<_>>()
                .join("+");
            let encoded_rule = format!(
                "{},{},{},{},{},{},{},{}",
                rule.id,
                event,
                rule.chance,
                allowed_phase_mask,
                recurrence,
                selection,
                execution,
                instructions,
            );
            definitions.entry(subject.0).or_default().push(encoded_rule);

            let (classification, reason) = if rule.normalizations.is_empty() {
                ("emitted", "emitted")
            } else {
                ("normalized", "source_loader_normalization")
            };
            plan.coverage.classify_rule(rule.id, classification, reason);
            plan.coverage
                .result("event", rule.event, classification, reason, rule.id, None);
            plan.coverage.result(
                "event_flag",
                rule.flags,
                classification,
                reason,
                rule.id,
                None,
            );
            for normalization in &rule.normalizations {
                plan.coverage.result(
                    normalization.dimension,
                    normalization.raw_value,
                    "normalized",
                    normalization.reason,
                    rule.id,
                    None,
                );
            }
            Coverage::source_value(&mut plan.coverage.accepted_event, rule.event as u64);
            *plan
                .coverage
                .emitted_event
                .entry(rule.event as u64)
                .or_default() += actions.len() as u64;
            for dependency in subject.1 {
                plan.coverage.result(
                    "dependency",
                    dependency.kind,
                    classification,
                    "resolved",
                    rule.id,
                    Some(dependency.path),
                );
            }
            for action in actions {
                Coverage::source_value(&mut plan.coverage.accepted_action, action.raw_kind as u64);
                Coverage::source_value(&mut plan.coverage.emitted_action, action.raw_kind as u64);
                plan.coverage.result(
                    "action",
                    action.raw_kind,
                    classification,
                    reason,
                    rule.id,
                    None,
                );
                if let Some(target) = action.raw_target {
                    Coverage::source_value(&mut plan.coverage.target, target as u64);
                    plan.coverage
                        .result("target", target, classification, reason, rule.id, None);
                }
                if let Some(flags) = action.raw_cast_flags {
                    plan.coverage
                        .result("cast_flag", flags, classification, reason, rule.id, None);
                }
                for dependency in action.dependencies {
                    plan.coverage.result(
                        "dependency",
                        dependency.kind,
                        classification,
                        "resolved",
                        rule.id,
                        Some(dependency.path),
                    );
                }
                used_texts.extend(action.texts.iter().copied());
                if let Some(entry) = action.summon_entry {
                    plan.forced_template_entries.insert(entry);
                }
                if let Some(location) = action.summon_location {
                    used_summons.insert(location);
                }
            }
            plan.coverage.accepted_rules += 1;
            plan.coverage.action_rows +=
                rule.actions.iter().filter(|action| action[0] != 0).count() as u64;
            plan.coverage.emitted_instructions +=
                rule.actions.iter().filter(|action| action[0] != 0).count() as u64;
        }

        plan.definition_rows = definitions
            .into_iter()
            .map(|(subject, rules)| encode_definition(subject, &rules))
            .collect();
        plan.definition_batches = pack_definition_batches(&plan.definition_rows);

        for id in used_texts {
            let Some(text) = broadcasts.get(&id) else {
                continue;
            };
            plan.broadcast_rows.push(format!(
                "({id},{male},{female},{chat_type},{language},{delay1},{emote1},{delay2},{emote2},{delay3},{emote3})",
                male = sql_text(&text.male),
                female = sql_text(&text.female),
                chat_type = text.chat_type,
                language = text.language,
                delay1 = text.emotes[0].0,
                emote1 = text.emotes[0].1,
                delay2 = text.emotes[1].0,
                emote2 = text.emotes[1].1,
                delay3 = text.emotes[2].0,
                emote3 = text.emotes[2].1,
            ));
        }
        for id in used_summons {
            let Some(location) = summon_locations.get(&id) else {
                continue;
            };
            plan.summon_rows.push(format!(
                "({id},{},{},{},{},{})",
                location.x, location.y, location.z, location.orientation, location.lifetime_ms
            ));
        }
        plan
    }
}

fn encode_definition(subject: Subject, rules: &[String]) -> String {
    let subject = subject.encode();
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

fn record_rule_dimensions(
    coverage: &mut Coverage,
    rule: &RawRule,
    classification: &str,
    reason: &str,
) {
    coverage.result("event", rule.event, classification, reason, rule.id, None);
    coverage.result(
        "event_flag",
        rule.flags,
        classification,
        reason,
        rule.id,
        None,
    );
    for action in rule.actions.iter().filter(|action| action[0] != 0) {
        coverage.result("action", action[0], classification, reason, rule.id, None);
        if let Some(parameter) = source_target_parameter(action[0]) {
            coverage.result(
                "target",
                action[parameter],
                classification,
                reason,
                rule.id,
                None,
            );
        }
        if action[0] == ACTION_CAST {
            coverage.result(
                "cast_flag",
                action[3],
                classification,
                reason,
                rule.id,
                None,
            );
        }
    }
}

fn record_dropped_rule(coverage: &mut Coverage, rule: &RawRule, failures: Vec<MappingFailure>) {
    let reasons = failures
        .iter()
        .map(|failure| failure.reason.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join("+");
    coverage.classify_rule(rule.id, "dropped", &reasons);
    record_rule_dimensions(coverage, rule, "dropped", "whole_rule_dropped");
    for normalization in &rule.normalizations {
        coverage.result(
            normalization.dimension,
            normalization.raw_value,
            "dropped",
            normalization.reason,
            rule.id,
            None,
        );
    }
    for failure in failures {
        coverage.drop_for_rule(&failure.reason, failure.raw_value, rule.id);
        coverage.result(
            failure.dimension,
            failure
                .dependency_kind
                .map(str::to_string)
                .unwrap_or_else(|| failure.raw_value.to_string()),
            "dropped",
            &failure.reason,
            rule.id,
            failure.dependency_path,
        );
    }
}

fn parse_rules(dump: &str, coverage: &mut Coverage) -> Vec<RawRule> {
    parse_table(dump, "creature_ai_scripts")
        .into_iter()
        .filter_map(|row| {
            coverage.total_rules += 1;
            if row.len() < 24 {
                coverage.drop("malformed_rule", row.len() as u64);
                return None;
            }
            let Some(id) = source_u32(field(&row, 0)) else {
                coverage.drop("invalid_numeric", 0);
                return None;
            };
            let Some(subject) = field(&row, 1).parse::<i32>().ok() else {
                coverage.drop("invalid_numeric", 1);
                return None;
            };
            if subject < 0 {
                coverage.source_guid_rules += 1;
            }
            let Some(event) = source_loader_u32(field(&row, 2)) else {
                coverage.drop("invalid_numeric", 2);
                return None;
            };
            Coverage::source_value(&mut coverage.event, event as u64);
            let Some(inverse_phase_mask) = source_loader_u32(field(&row, 3)) else {
                coverage.drop("invalid_numeric", 3);
                return None;
            };
            let Some(raw_chance) = source_u8(field(&row, 4)) else {
                coverage.drop("invalid_numeric", 4);
                return None;
            };
            let chance = raw_chance.min(100);
            let Some(flags) = source_loader_u32(field(&row, 5)) else {
                coverage.drop("invalid_numeric", 5);
                return None;
            };
            Coverage::source_value(&mut coverage.flags, flags as u64);
            let mut params = [0; 6];
            let mut actions = [[0; 4]; 3];
            let mut normalizations = Vec::new();
            if raw_chance > 100 {
                normalizations.push(SourceNormalization {
                    dimension: "rule",
                    raw_value: u64::from(raw_chance),
                    reason: "chance_capped",
                });
            }
            for (index, param) in params.iter_mut().enumerate() {
                let Some(value) = source_loader_u32(field(&row, 6 + index)) else {
                    coverage.drop("invalid_numeric", (6 + index) as u64);
                    return None;
                };
                *param = value;
            }
            for (slot, action) in actions.iter_mut().enumerate() {
                let action_column = 12 + slot * 4;
                let Some(kind) = source_loader_u16(field(&row, action_column)) else {
                    coverage.drop("invalid_numeric", action_column as u64);
                    return None;
                };
                if kind >= ACTION_END {
                    Coverage::source_value(&mut coverage.action, u64::from(kind));
                    normalizations.push(SourceNormalization {
                        dimension: "action",
                        raw_value: u64::from(kind),
                        reason: "invalid_action_to_none",
                    });
                    continue;
                }
                let kind = u32::from(kind);
                action[0] = kind;
                if kind != 0 {
                    Coverage::source_value(&mut coverage.action, kind as u64);
                }
                for (part, value) in action.iter_mut().enumerate().skip(1) {
                    let column = action_column + part;
                    let parsed = source_loader_u32(field(&row, column));
                    let Some(parsed) = parsed else {
                        coverage.drop("invalid_numeric", column as u64);
                        return None;
                    };
                    *value = parsed;
                }
                if let Some(parameter) = source_target_parameter(kind) {
                    Coverage::source_value(&mut coverage.source_target, action[parameter] as u64);
                }
                if kind == ACTION_CAST {
                    Coverage::source_value(&mut coverage.cast_flags, action[3] as u64);
                }
            }
            Some(RawRule {
                id: u64::from(id),
                subject,
                event,
                inverse_phase_mask,
                chance,
                flags,
                params,
                actions,
                normalizations,
            })
        })
        .collect()
}

fn source_target_parameter(action: u32) -> Option<usize> {
    match action {
        ACTION_REMOVE_AURA | ACTION_ATTACK_START | ACTION_SET_FACING => Some(1),
        ACTION_CAST
        | ACTION_SPAWN
        | ACTION_THREAT_SINGLE
        | ACTION_QUEST_EVENT
        | ACTION_SET_UNIT_FLAG
        | ACTION_REMOVE_UNIT_FLAG
        | ACTION_SUMMON_ID
        | ACTION_KILLED_MONSTER
        | ACTION_SET_INSTANCE_DATA_GUID
        | ACTION_START_RELAY
        | ACTION_TEXT_NEW => Some(2),
        ACTION_CAST_EVENT | ACTION_SET_UNIT_FIELD | ACTION_THROW_AI_EVENT => Some(3),
        _ => None,
    }
}

fn parse_broadcasts(dump: &str, coverage: &mut Coverage) -> BTreeMap<u32, Broadcast> {
    parse_table(dump, "broadcast_text")
        .into_iter()
        .filter_map(|row| {
            let Some(id) = source_u32(field(&row, bt::ID)) else {
                coverage.drop("malformed_broadcast_text", 0);
                return None;
            };
            if fixture_id(id) {
                coverage.drop("reserved_catalogue_id", id as u64);
                return None;
            }
            let Some(chat_type) =
                source_u32(field(&row, bt::CHAT_TYPE)).and_then(|value| u8::try_from(value).ok())
            else {
                coverage.drop("malformed_broadcast_text", id as u64);
                return None;
            };
            let Some(language) =
                source_u32(field(&row, bt::LANGUAGE)).and_then(|value| u8::try_from(value).ok())
            else {
                coverage.drop("malformed_broadcast_text", id as u64);
                return None;
            };
            if row.len() < 12 {
                coverage.drop("malformed_broadcast_text", id as u64);
                return None;
            }
            let (emote_start, delay_start) = if row.len() >= 17 { (10, 13) } else { (6, 9) };
            let emotes = std::array::from_fn(|index| {
                (
                    source_u32(field(&row, delay_start + index)).unwrap_or(0),
                    source_u32(field(&row, emote_start + index)).unwrap_or(0),
                )
            });
            Some((
                id,
                Broadcast {
                    male: field(&row, bt::TEXT).to_string(),
                    female: field(&row, bt::TEXT1).to_string(),
                    chat_type,
                    language,
                    emotes,
                },
            ))
        })
        .collect()
}

fn parse_legacy_texts(
    dump: &str,
    broadcasts: &BTreeMap<u32, Broadcast>,
    coverage: &mut Coverage,
) -> BTreeMap<u32, Broadcast> {
    parse_table(dump, "script_texts")
        .into_iter()
        .filter_map(|row| {
            let Some(id) = field(&row, 0).parse::<i64>().ok() else {
                coverage.drop("malformed_legacy_text", 0);
                return None;
            };
            if row.len() < 15 {
                coverage.drop("malformed_legacy_text", id.unsigned_abs());
                return None;
            }
            let broadcast_id = source_u32(field(&row, 14)).unwrap_or(0);
            let text = broadcasts.get(&broadcast_id).cloned().unwrap_or(Broadcast {
                male: field(&row, 1).to_string(),
                female: field(&row, 1).to_string(),
                chat_type: source_u32(field(&row, 11))
                    .and_then(|value| u8::try_from(value).ok())
                    .unwrap_or(0),
                language: source_u32(field(&row, 12))
                    .and_then(|value| u8::try_from(value).ok())
                    .unwrap_or(0),
                emotes: [
                    (0, source_u32(field(&row, 13)).unwrap_or(0)),
                    (0, 0),
                    (0, 0),
                ],
            });
            (id < 0).then_some((id as u32, text))
        })
        .collect()
}

fn parse_summons(dump: &str, coverage: &mut Coverage) -> BTreeMap<u32, SummonLocation> {
    parse_table(dump, "creature_ai_summons")
        .into_iter()
        .filter_map(|row| {
            let Some(id) = source_u32(field(&row, 0)) else {
                coverage.drop("malformed_summon_location", 0);
                return None;
            };
            if fixture_id(id) {
                coverage.drop("reserved_catalogue_id", id as u64);
                return None;
            }
            let parsed = || {
                Some((
                    field(&row, 1).parse::<f32>().ok()?,
                    field(&row, 2).parse::<f32>().ok()?,
                    field(&row, 3).parse::<f32>().ok()?,
                    field(&row, 4).parse::<f32>().ok()?,
                    source_u32(field(&row, 5))?.checked_mul(1_000)?,
                ))
            };
            let Some((x, y, z, orientation, lifetime_ms)) = parsed() else {
                coverage.drop("malformed_summon_location", id as u64);
                return None;
            };
            if !(x.is_finite() && y.is_finite() && z.is_finite() && orientation.is_finite()) {
                coverage.drop("malformed_summon_location", id as u64);
                return None;
            }
            Some((
                id,
                SummonLocation {
                    x,
                    y,
                    z,
                    orientation,
                    lifetime_ms,
                },
            ))
        })
        .collect()
}

enum SubjectResolution {
    Included {
        subject: Subject,
        dependencies: Vec<Dependency>,
    },
    Excluded,
    Failed(MappingFailure),
}

fn resolve_subject(
    rule: &RawRule,
    imported_entries: &HashSet<u64>,
    imported_guid_entries: &HashMap<u64, u64>,
    importable_templates: &HashSet<u64>,
) -> SubjectResolution {
    if rule.subject > 0 {
        let entry = rule.subject as u64;
        if !imported_entries.contains(&entry) {
            return SubjectResolution::Excluded;
        }
        if !importable_templates.contains(&entry) {
            return SubjectResolution::Failed(MappingFailure::dependency(
                "creature_subject",
                entry,
                "missing",
                format!("rule:{} -> creature_template:{entry}", rule.id),
            ));
        }
        let Ok(entry) = u32::try_from(entry) else {
            return SubjectResolution::Failed(MappingFailure::source(
                "dependency",
                entry,
                "creature_subject_out_of_range",
            ));
        };
        return SubjectResolution::Included {
            subject: Subject::Entry(entry),
            dependencies: vec![Dependency {
                kind: "creature_subject",
                path: format!("rule:{} -> creature_template:{entry}", rule.id),
            }],
        };
    }
    if rule.subject >= 0 {
        return SubjectResolution::Failed(MappingFailure::source(
            "dependency",
            rule.subject as u64,
            "invalid_subject",
        ));
    }
    let source_guid = u64::from(rule.subject.unsigned_abs());
    let Some(&entry) = imported_guid_entries.get(&source_guid) else {
        return SubjectResolution::Excluded;
    };
    if !importable_templates.contains(&entry) {
        return SubjectResolution::Failed(MappingFailure::dependency(
            "source_guid_subject",
            source_guid,
            "missing_template",
            format!(
                "rule:{} -> creature:{source_guid} -> creature_template:{entry}",
                rule.id
            ),
        ));
    }
    let guid = world_guid(entry, source_guid);
    SubjectResolution::Included {
        subject: Subject::Guid(guid),
        dependencies: vec![
            Dependency {
                kind: "source_guid_subject",
                path: format!("rule:{} -> creature:{source_guid} -> guid:{guid}", rule.id),
            },
            Dependency {
                kind: "creature_subject",
                path: format!(
                    "rule:{} -> creature:{source_guid} -> creature_template:{entry}",
                    rule.id
                ),
            },
        ],
    }
}

fn map_event(rule: &RawRule) -> Result<String, MappingFailure> {
    let params = rule.params;
    let valid_timer = |params: [u32; 6]| params[0] <= params[1] && params[2] <= params[3];
    match rule.event {
        EVENT_TIMER_IN_COMBAT if valid_timer(params) => {
            Ok(format!("timer:{}:{}", params[0], params[1]))
        }
        EVENT_HP if params[0] <= 100 && params[1] <= params[0] && params[2] <= params[3] => {
            Ok(format!("health:{}:{}", params[1], params[0]))
        }
        EVENT_AGGRO if params == [0; 6] => Ok("aggro".to_string()),
        EVENT_DEATH if params == [0; 6] => Ok("death".to_string()),
        EVENT_RANGE if valid_timer(params) => Ok(format!("range:{}:{}", params[0], params[1])),
        EVENT_SPAWNED if params[0] == 0 && params[1..] == [0; 5] => Ok("spawn".to_string()),
        EVENT_FRIENDLY_HP if params[1] > 0 && params[2] <= params[3] => {
            Ok(format!("friendly-health:{}:{}", params[0], params[1]))
        }
        value
            if matches!(
                value,
                EVENT_TIMER_IN_COMBAT
                    | EVENT_HP
                    | EVENT_AGGRO
                    | EVENT_DEATH
                    | EVENT_RANGE
                    | EVENT_SPAWNED
                    | EVENT_FRIENDLY_HP
            ) =>
        {
            Err(MappingFailure::source(
                "event",
                value as u64,
                "invalid_event_parameters",
            ))
        }
        value => Err(MappingFailure::source(
            "event",
            value as u64,
            "unsupported_event",
        )),
    }
}

fn map_action(
    action: [u32; 4],
    rule_id: u64,
    slot: usize,
    broadcasts: &BTreeMap<u32, Broadcast>,
    summon_locations: &BTreeMap<u32, SummonLocation>,
    importable_templates: &HashSet<u64>,
) -> Result<NativeAction, Vec<MappingFailure>> {
    let kind = action[0];
    match kind {
        ACTION_TEXT => {
            let texts = action[1..]
                .iter()
                .copied()
                .filter(|id| *id != 0)
                .collect::<Vec<_>>();
            let mut failures = Vec::new();
            if texts.is_empty() {
                failures.push(MappingFailure::source("action", kind as u64, "empty_text"));
            }
            for id in &texts {
                if !broadcasts.contains_key(id) {
                    failures.push(MappingFailure::dependency(
                        "broadcast_text",
                        u64::from(*id),
                        if (*id as i32) < 0 {
                            "unsupported_legacy"
                        } else {
                            "missing"
                        },
                        format!("rule:{rule_id} -> action:{slot} -> broadcast_text:{id}"),
                    ));
                }
            }
            let mode = speech_mode(&texts, broadcasts, rule_id, slot, &mut failures);
            if !failures.is_empty() {
                return Err(failures);
            }
            Ok(NativeAction {
                encoded: format!(
                    "speak:{mode}:self:{}",
                    texts
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(".")
                ),
                raw_kind: kind,
                raw_target: None,
                raw_cast_flags: None,
                dependencies: text_dependencies(rule_id, slot, &texts),
                texts,
                summon_entry: None,
                summon_location: None,
            })
        }
        ACTION_TEXT_NEW => {
            if action[3] != 0 {
                return Err(vec![MappingFailure::source(
                    "action",
                    u64::from(action[3]),
                    format!("unsupported_text_template_{}", action[3]),
                )]);
            }
            if !broadcasts.contains_key(&action[1]) {
                return Err(vec![MappingFailure::dependency(
                    "broadcast_text",
                    u64::from(action[1]),
                    "missing",
                    format!(
                        "rule:{rule_id} -> action:{slot} -> broadcast_text:{}",
                        action[1]
                    ),
                )]);
            }
            let mut failures = Vec::new();
            let mode = speech_mode(&[action[1]], broadcasts, rule_id, slot, &mut failures);
            let target = map_target(action[2]).map_err(|failure| vec![failure])?;
            if !failures.is_empty() {
                return Err(failures);
            }
            Ok(NativeAction {
                encoded: format!("speak:{mode}:{target}:{}", action[1]),
                raw_kind: kind,
                raw_target: Some(action[2]),
                raw_cast_flags: None,
                dependencies: text_dependencies(rule_id, slot, &[action[1]]),
                texts: vec![action[1]],
                summon_entry: None,
                summon_location: None,
            })
        }
        ACTION_EMOTE => Ok(NativeAction {
            encoded: format!("emote:{}:self", action[1]),
            raw_kind: kind,
            raw_target: None,
            raw_cast_flags: None,
            dependencies: Vec::new(),
            texts: Vec::new(),
            summon_entry: None,
            summon_location: None,
        }),
        ACTION_CAST => {
            if action[1] == 0 {
                return Err(vec![MappingFailure::source(
                    "action",
                    u64::from(action[1]),
                    "invalid_spell",
                )]);
            }
            let (target, options) = map_cast_target_and_flags(action[2], action[3])?;
            Ok(NativeAction {
                encoded: format!("cast:{}:{target}:{options}", action[1]),
                raw_kind: kind,
                raw_target: Some(action[2]),
                raw_cast_flags: Some(action[3]),
                dependencies: Vec::new(),
                texts: Vec::new(),
                summon_entry: None,
                summon_location: None,
            })
        }
        ACTION_SET_PHASE if action[1] < 32 => Ok(NativeAction {
            encoded: format!("phase:{}", action[1]),
            raw_kind: kind,
            raw_target: None,
            raw_cast_flags: None,
            dependencies: Vec::new(),
            texts: Vec::new(),
            summon_entry: None,
            summon_location: None,
        }),
        ACTION_FLEE_FOR_ASSIST => Ok(NativeAction {
            encoded: "flee".to_string(),
            raw_kind: kind,
            raw_target: None,
            raw_cast_flags: None,
            dependencies: Vec::new(),
            texts: Vec::new(),
            summon_entry: None,
            summon_location: None,
        }),
        ACTION_CALL_FOR_HELP => Ok(NativeAction {
            encoded: format!("help:{}", action[1]),
            raw_kind: kind,
            raw_target: None,
            raw_cast_flags: None,
            dependencies: Vec::new(),
            texts: Vec::new(),
            summon_entry: None,
            summon_location: None,
        }),
        ACTION_RANGED_MOVEMENT => Ok(NativeAction {
            encoded: format!("posture:{}:{}", action[1], action[2] as i32),
            raw_kind: kind,
            raw_target: None,
            raw_cast_flags: None,
            dependencies: Vec::new(),
            texts: Vec::new(),
            summon_entry: None,
            summon_location: None,
        }),
        ACTION_SUMMON_ID => {
            let entry = action[1] as u64;
            let mut failures = Vec::new();
            if !importable_templates.contains(&entry) {
                failures.push(MappingFailure::dependency(
                    "summon_creature",
                    entry,
                    "missing",
                    format!("rule:{rule_id} -> action:{slot} -> creature_template:{entry}"),
                ));
            }
            if !summon_locations.contains_key(&action[3]) {
                failures.push(MappingFailure::dependency(
                    "summon_location",
                    u64::from(action[3]),
                    "missing",
                    format!(
                        "rule:{rule_id} -> action:{slot} -> creature_ai_summons:{}",
                        action[3]
                    ),
                ));
            }
            let target = match map_target(action[2]) {
                Ok(target) => Some(target),
                Err(failure) => {
                    failures.push(failure);
                    None
                }
            };
            if !failures.is_empty() {
                return Err(failures);
            }
            Ok(NativeAction {
                encoded: format!(
                    "summon:{}:{}:{}",
                    action[1],
                    action[3],
                    target.expect("a summon without mapping failures has a target")
                ),
                raw_kind: kind,
                raw_target: Some(action[2]),
                raw_cast_flags: None,
                texts: Vec::new(),
                summon_entry: Some(entry),
                summon_location: Some(action[3]),
                dependencies: vec![
                    Dependency {
                        kind: "summon_creature",
                        path: format!(
                            "rule:{rule_id} -> action:{slot} -> creature_template:{entry}"
                        ),
                    },
                    Dependency {
                        kind: "summon_location",
                        path: format!(
                            "rule:{rule_id} -> action:{slot} -> creature_ai_summons:{}",
                            action[3]
                        ),
                    },
                ],
            })
        }
        ACTION_SET_PHASE => Err(vec![MappingFailure::source(
            "action",
            u64::from(action[1]),
            format!("invalid_phase_{}", action[1]),
        )]),
        value => Err(vec![MappingFailure::source(
            "action",
            value as u64,
            "unsupported_action",
        )]),
    }
}

fn map_cast_target_and_flags(
    raw_target: u32,
    raw_flags: u32,
) -> Result<(&'static str, String), Vec<MappingFailure>> {
    let mut failures = Vec::new();
    let residual = raw_flags & !SUPPORTED_CAST_FLAGS;
    if residual != 0 {
        failures.push(MappingFailure::source(
            "cast_flag",
            raw_flags as u64,
            format!("unsupported_cast_flag_residual_{residual:#x}"),
        ));
    }
    if raw_flags & CAST_TRIGGERED != 0 {
        failures.push(MappingFailure::source(
            "cast_flag",
            raw_flags as u64,
            "unsupported_triggered_cast",
        ));
    }
    if raw_flags & CAST_FORCE_CAST != 0 {
        failures.push(MappingFailure::source(
            "cast_flag",
            raw_flags as u64,
            "unsupported_force_cast",
        ));
    }
    let target = match map_target(raw_target) {
        Ok(target) => Some(target),
        Err(failure) => {
            failures.push(failure);
            None
        }
    };
    if !failures.is_empty() {
        return Err(failures);
    }
    let options = format!(
        "{}:0:{}:{}:{}",
        u8::from(raw_flags & CAST_INTERRUPT_PREVIOUS != 0),
        u8::from(raw_flags & CAST_AURA_NOT_PRESENT != 0),
        u8::from(raw_flags & CAST_PLAYER_ONLY != 0),
        u8::from(raw_flags & CAST_TARGET_CASTING != 0),
    );
    Ok((
        target.expect("a cast without mapping failures has a target"),
        options,
    ))
}

fn map_target(value: u32) -> Result<&'static str, MappingFailure> {
    match value {
        TARGET_SELF => Ok("self"),
        TARGET_HOSTILE => Ok("opponent"),
        TARGET_HOSTILE_SECOND => Ok("second-threat"),
        TARGET_HOSTILE_RANDOM => Ok("random-threat"),
        TARGET_ACTION_INVOKER => Ok("invoker"),
        TARGET_HOSTILE_RANDOM_PLAYER => Ok("random-threat-character"),
        TARGET_EVENT_SPECIFIC => Ok("event-subject"),
        TARGET_NEAREST_AOE => Ok("eligible-caster-area"),
        TARGET_HOSTILE_FARTHEST => Ok("farthest-hostile"),
        other => Err(MappingFailure::source(
            "target",
            other as u64,
            "unsupported_target",
        )),
    }
}

fn speech_mode(
    ids: &[u32],
    broadcasts: &BTreeMap<u32, Broadcast>,
    rule_id: u64,
    slot: usize,
    failures: &mut Vec<MappingFailure>,
) -> &'static str {
    let modes = ids
        .iter()
        .filter_map(|id| broadcasts.get(id).map(|text| text.chat_type))
        .collect::<BTreeSet<_>>();
    for mode in &modes {
        if *mode > 1 {
            failures.push(MappingFailure::dependency(
                "broadcast_text",
                u64::from(*mode),
                "unsupported_chat_type",
                format!("rule:{rule_id} -> action:{slot} -> broadcast_text:chat_type:{mode}"),
            ));
        }
    }
    if modes.len() > 1 {
        failures.push(MappingFailure::source(
            "action",
            ACTION_TEXT as u64,
            "mixed_speech_mode",
        ));
    }
    if modes.first().copied() == Some(1) {
        "yell"
    } else {
        "say"
    }
}

fn text_dependencies(rule_id: u64, slot: usize, ids: &[u32]) -> Vec<Dependency> {
    ids.iter()
        .map(|id| Dependency {
            kind: "broadcast_text",
            path: format!("rule:{rule_id} -> action:{slot} -> broadcast_text:{id}"),
        })
        .collect()
}

fn source_u32(value: &str) -> Option<u32> {
    value.parse().ok()
}

fn source_u8(value: &str) -> Option<u8> {
    value.parse::<i64>().ok().map(|value| value as u8)
}

fn source_loader_u16(value: &str) -> Option<u16> {
    value.parse::<i64>().ok().map(|value| value as u16)
}

fn source_loader_u32(value: &str) -> Option<u32> {
    value.parse::<i64>().ok().map(|value| value as u32)
}

fn fixture_id(id: u32) -> bool {
    (FIXTURE_ID_FIRST..=FIXTURE_ID_LAST).contains(&id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(
        id: impl ToString,
        subject: impl ToString,
        event: impl ToString,
        chance: impl ToString,
        flags: impl ToString,
        params: [i64; 6],
        actions: [[i64; 4]; 3],
    ) -> String {
        let mut fields = vec![
            id.to_string(),
            subject.to_string(),
            event.to_string(),
            "0".to_string(),
            chance.to_string(),
            flags.to_string(),
        ];
        fields.extend(params.map(|value| value.to_string()));
        for action in actions {
            fields.extend(action.map(|value| value.to_string()));
        }
        format!("({})", fields.join(","))
    }

    fn dump(rules: &[String]) -> String {
        format!(
            "INSERT INTO `broadcast_text` VALUES \
             (900,'Hello','Hello',0,0,0,0,0,0,0,0,0,0,0,0,0,0); \
             INSERT INTO `creature_ai_scripts` VALUES {};",
            rules.join(",")
        )
    }

    fn scope() -> (HashSet<u64>, HashMap<u64, u64>, HashSet<u64>) {
        (
            [100].into_iter().collect(),
            [(2, 100)].into_iter().collect(),
            [100].into_iter().collect(),
        )
    }

    fn fixture_profile(plan: &EventAiPlan) -> SourceProfile {
        let mut profile = source_profile(SOURCE_PROFILE_NAME).unwrap();
        profile.sql_sha256 = "fixture".to_string();
        profile.source_rule_count = plan.coverage.total_rules;
        profile.source_guid_rule_count = plan.coverage.source_guid_rules;
        profile.expected_source_census.clear();
        profile
    }

    fn assert_refusal(
        manifest: &CompatibilityManifest,
        finding_fragment: &str,
    ) -> serde_json::Value {
        assert!(!manifest.is_apply_ready(), "{}", manifest.render());
        assert!(manifest
            .require_apply_ready()
            .unwrap_err()
            .contains(finding_fragment));
        let rendered: serde_json::Value = serde_json::from_str(manifest.render()).unwrap();
        assert_eq!(rendered["apply_ready"], false);
        assert!(rendered["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding.as_str().unwrap().contains(finding_fragment)));
        rendered
    }

    fn assert_result(
        manifest: &serde_json::Value,
        dimension: &str,
        raw_value: &str,
        classification: &str,
        reason: &str,
    ) {
        assert!(manifest["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|result| {
                result["dimension"] == dimension
                    && result["raw_value"] == raw_value
                    && result["classification"] == classification
                    && result["reason"] == reason
            }));
    }

    #[test]
    fn source_profile_pins_product_identity_and_loader_contract() {
        let profile = source_profile(SOURCE_PROFILE_NAME).unwrap();
        assert_eq!(
            profile.cmangos_commit,
            "ca0775fe352fb67a0e82dc6051f0563187d569a5"
        );
        assert_eq!(
            profile.classic_db_commit,
            "cd0c426a3b2ff56dd518bf009025299468e60fdb"
        );
        assert_eq!(
            profile.sql_sha256,
            "d2083bcd2670451279cbf93af138eadae04c6d183a4cd0ff0357047e4a565de6"
        );
        assert_eq!(profile.loader_contract, LOADER_CONTRACT);
        assert_eq!(profile.source_rule_count, 10_843);
        assert_eq!(profile.source_guid_rule_count, 39);
        assert!(source_profile("z2815").is_err());
    }

    #[test]
    fn definitions_keep_subjects_rules_and_instruction_order() {
        let source = parse(&dump(&[
            rule(
                10,
                100,
                EVENT_AGGRO,
                100,
                FLAG_RANDOM_ACTION | FLAG_COMBAT_ACTION,
                [0; 6],
                [
                    [ACTION_EMOTE as i64, 7, 0, 0],
                    [ACTION_CALL_FOR_HELP as i64, 12, 0, 0],
                    [ACTION_SET_PHASE as i64, 3, 0, 0],
                ],
            ),
            rule(
                11,
                -2,
                EVENT_DEATH,
                75,
                0,
                [0; 6],
                [[ACTION_FLEE_FOR_ASSIST as i64, 0, 0, 0], [0; 4], [0; 4]],
            ),
        ]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        assert_eq!(plan.definition_rows.len(), 2);
        let entry = plan
            .definition_rows
            .iter()
            .find(|row| row.starts_with("entry:100@"))
            .unwrap();
        assert!(entry
            .ends_with("@10,aggro,100,4294967295,once,random,combat,emote:7:self+help:12+phase:3"));
        let guid = world_guid(100, 2);
        assert!(plan
            .definition_rows
            .iter()
            .any(|row| row.starts_with(&format!("guid:{guid}@"))
                && row.ends_with("@11,death,75,4294967295,once,all,ordinary,flee")));
        assert_eq!(plan.instruction_count(), 4);
        assert_eq!(
            plan.definition_batches.join("\n"),
            plan.definition_rows.join("\n")
        );

        let again = source.assemble(&entries, &guids, &templates);
        assert_eq!(plan.definition_rows, again.definition_rows);
    }

    #[test]
    fn manifest_reports_resolved_dependency_paths() {
        let source = parse(&dump(&[rule(
            10,
            100,
            EVENT_AGGRO,
            100,
            0,
            [0; 6],
            [[ACTION_TEXT as i64, 900, 0, 0], [0; 4], [0; 4]],
        )]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        let profile = fixture_profile(&plan);
        let manifest = plan.compatibility_manifest(&profile, "fixture", LOADER_CONTRACT);
        assert!(manifest.is_apply_ready(), "{}", manifest.render());
        assert!(manifest
            .render()
            .contains("rule:10 -> action:0 -> broadcast_text:900"));
        assert!(manifest
            .render()
            .contains("\"unapproved_result_groups\": 0"));
    }

    #[test]
    fn manifest_refuses_a_wrong_sql_hash_and_retains_the_finding() {
        let source = parse(&dump(&[rule(
            10,
            100,
            EVENT_AGGRO,
            100,
            0,
            [0; 6],
            [[ACTION_EMOTE as i64, 7, 0, 0], [0; 4], [0; 4]],
        )]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        let profile = fixture_profile(&plan);
        let manifest = plan.compatibility_manifest(&profile, "wrong-hash", LOADER_CONTRACT);
        let rendered = assert_refusal(&manifest, "sql_sha256");
        assert_eq!(rendered["profile"]["observed_sql_sha256"], "wrong-hash");
    }

    #[test]
    fn manifest_refuses_a_wrong_loader_contract_and_retains_the_finding() {
        let source = parse(&dump(&[rule(
            10,
            100,
            EVENT_AGGRO,
            100,
            0,
            [0; 6],
            [[ACTION_EMOTE as i64, 7, 0, 0], [0; 4], [0; 4]],
        )]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        let profile = fixture_profile(&plan);
        let manifest = plan.compatibility_manifest(&profile, "fixture", "other-loader");
        let rendered = assert_refusal(&manifest, "loader_contract");
        assert_eq!(
            rendered["profile"]["observed_loader_contract"],
            "other-loader"
        );
    }

    #[test]
    fn manifest_refuses_wrong_source_counts_and_census_and_retains_both_findings() {
        let source = parse(&dump(&[rule(
            10,
            100,
            EVENT_AGGRO,
            100,
            0,
            [0; 6],
            [[ACTION_EMOTE as i64, 7, 0, 0], [0; 4], [0; 4]],
        )]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        let mut profile = fixture_profile(&plan);
        profile.source_rule_count += 1;
        profile
            .expected_source_census
            .insert("events".to_string(), [(EVENT_AGGRO as u64, 2)].into());
        let manifest = plan.compatibility_manifest(&profile, "fixture", LOADER_CONTRACT);
        let rendered = assert_refusal(&manifest, "source_rule_count");
        assert!(rendered["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding.as_str().unwrap().contains("source_events_census")));
        assert_eq!(rendered["source_census"]["events"]["4"], 1);
    }

    #[test]
    fn manifest_refuses_a_new_normalized_raw_value_and_retains_its_group() {
        let source = parse(&dump(&[rule(
            10,
            100,
            EVENT_AGGRO,
            100,
            0,
            [0; 6],
            [[999, 0, 0, 0], [ACTION_EMOTE as i64, 7, 0, 0], [0; 4]],
        )]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        let profile = fixture_profile(&plan);
        let manifest = plan.compatibility_manifest(&profile, "fixture", LOADER_CONTRACT);
        let rendered = assert_refusal(&manifest, "raw=999");
        assert_result(
            &rendered,
            "action",
            "999",
            "normalized",
            "invalid_action_to_none",
        );
    }

    #[test]
    fn manifest_refuses_a_missing_dependency_and_retains_its_full_path() {
        let source = parse(&dump(&[rule(
            10,
            100,
            EVENT_AGGRO,
            100,
            0,
            [0; 6],
            [[ACTION_TEXT as i64, 901, 0, 0], [0; 4], [0; 4]],
        )]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        let profile = fixture_profile(&plan);
        let manifest = plan.compatibility_manifest(&profile, "fixture", LOADER_CONTRACT);
        let rendered = assert_refusal(&manifest, "missing:broadcast_text");
        assert_result(
            &rendered,
            "dependency",
            "broadcast_text",
            "dropped",
            "missing:broadcast_text",
        );
        assert!(manifest
            .render()
            .contains("rule:10 -> action:0 -> broadcast_text:901"));
    }

    #[test]
    fn manifest_refuses_an_unapproved_classification_and_retains_its_group() {
        let source = parse(&dump(&[rule(
            10,
            100,
            EVENT_AGGRO,
            100,
            0,
            [0; 6],
            [[ACTION_EMOTE as i64, 7, 0, 0], [0; 4], [0; 4]],
        )]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        let mut profile = fixture_profile(&plan);
        profile.approvals.classifications.remove("emitted");
        let manifest = plan.compatibility_manifest(&profile, "fixture", LOADER_CONTRACT);
        let rendered = assert_refusal(&manifest, "classification=emitted");
        assert_result(&rendered, "rule", "10", "emitted", "emitted");
    }

    #[test]
    fn manifest_refuses_source_rows_without_a_terminal_classification() {
        let source = parse(&dump(&["(10,100)".to_string()]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        let profile = fixture_profile(&plan);
        let manifest = plan.compatibility_manifest(&profile, "fixture", LOADER_CONTRACT);
        let rendered = assert_refusal(&manifest, "source_classification_count");
        assert_eq!(rendered["counts"]["classified_rules"], 0);
        assert_eq!(rendered["counts"]["unclassified_rules"], 1);
    }

    #[test]
    fn flag_approvals_use_bits_and_report_the_residual_mask() {
        let source = parse(&dump(&[rule(
            10,
            100,
            EVENT_AGGRO,
            100,
            0x02,
            [0; 6],
            [[ACTION_EMOTE as i64, 7, 0, 0], [0; 4], [0; 4]],
        )]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        let profile = fixture_profile(&plan);
        let manifest = plan.compatibility_manifest(&profile, "fixture", LOADER_CONTRACT);
        assert!(!manifest.is_apply_ready());
        assert!(manifest
            .render()
            .contains("unsupported_event_flag_residual_0x2"));
    }

    #[test]
    fn source_loader_contract_constrains_ids_subjects_chance_and_actions() {
        let source = parse(&dump(&[
            rule(
                u32::MAX,
                100,
                EVENT_AGGRO,
                101,
                0,
                [0; 6],
                [
                    [ACTION_END as i64, 99, 99, 99],
                    [ACTION_EMOTE as i64, 7, 0, 0],
                    [0; 4],
                ],
            ),
            rule(
                u64::from(u32::MAX) + 1,
                100,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [[ACTION_EMOTE as i64, 8, 0, 0], [0; 4], [0; 4]],
            ),
            rule(
                12,
                i64::from(i32::MAX) + 1,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [[ACTION_EMOTE as i64, 9, 0, 0], [0; 4], [0; 4]],
            ),
        ]));
        assert_eq!(source.rules.len(), 1);
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        assert_eq!(plan.definition_rows.len(), 1);
        assert!(plan.definition_rows[0].contains(&format!(
            "@{},aggro,100,4294967295,once,all,ordinary,emote:7:self",
            u32::MAX
        )));
        assert_eq!(plan.coverage.normalized_rules, 1);
    }

    #[test]
    fn loader_uint32_signed_values_are_bit_preserved_and_classified() {
        let source = parse(&dump(&[
            rule(
                20,
                100,
                EVENT_TIMER_IN_COMBAT,
                100,
                0,
                [-1, -1, 0, 0, 0, 0],
                [[ACTION_EMOTE as i64, -1, 0, 0], [0; 4], [0; 4]],
            ),
            rule(
                21,
                100,
                -1,
                100,
                -1,
                [0; 6],
                [[ACTION_EMOTE as i64, 8, 0, 0], [0; 4], [0; 4]],
            ),
            rule(
                22,
                100,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [[-1, 0, 0, 0], [ACTION_EMOTE as i64, 9, 0, 0], [0; 4]],
            ),
        ]));
        assert_eq!(source.rules.len(), 3);
        assert_eq!(source.coverage.event.get(&(u32::MAX as u64)), Some(&1));
        assert_eq!(source.coverage.flags.get(&(u32::MAX as u64)), Some(&1));
        assert_eq!(source.coverage.action.get(&(u16::MAX as u64)), Some(&1));

        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        assert_eq!(plan.coverage.classified_rules(), 3);
        assert_eq!(plan.coverage.emitted_rules, 1);
        assert_eq!(plan.coverage.normalized_rules, 1);
        assert_eq!(plan.coverage.dropped_rules, 1);
        assert!(plan.definition_rows[0].contains(
            "20,timer:4294967295:4294967295,100,4294967295,once,all,ordinary,emote:4294967295:self"
        ));
    }

    #[test]
    #[ignore = "requires LYRACORE_CLASSIC_DB_SQL pointing at the pinned decompressed SQL or gzip"]
    fn pinned_full_dump_source_census_matches_the_profile() {
        use std::io::Read;

        let path = std::env::var("LYRACORE_CLASSIC_DB_SQL").unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let bytes = if path.ends_with(".gz") {
            let mut decoded = Vec::new();
            flate2::read::GzDecoder::new(bytes.as_slice())
                .read_to_end(&mut decoded)
                .unwrap();
            decoded
        } else {
            bytes
        };
        assert_eq!(
            crate::sha256_hex(&bytes).unwrap(),
            "d2083bcd2670451279cbf93af138eadae04c6d183a4cd0ff0357047e4a565de6"
        );
        let source = parse(std::str::from_utf8(&bytes).unwrap());
        assert_eq!(source.coverage.total_rules, 10_843);
        assert_eq!(source.coverage.source_guid_rules, 39);
        assert_eq!(
            source.rules.len(),
            10_843,
            "loader-valid rows lost during parsing: {:?}",
            source.coverage.dropped_values
        );
        assert!(source
            .rules
            .iter()
            .all(|rule| rule.normalizations.is_empty()));
        let profile = source_profile(SOURCE_PROFILE_NAME).unwrap();
        for (dimension, expected) in profile.expected_source_census {
            assert_eq!(
                source.coverage.source_census(&dimension),
                expected,
                "{dimension}"
            );
        }
    }
}
