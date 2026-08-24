//! CMaNGOS EventAI import boundary.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::{bt, eventai_presentation, field, go_guid, parse_table, sql_text, world_guid};

pub(crate) const SOURCE_PROFILE_NAME: &str = "cmangos-classic-z2815";
pub(crate) const LOADER_CONTRACT: &str = "creature-ai-scripts-24-columns-ordered-by-id-v1";
const SOURCE_PROFILE_JSON: &str = include_str!("../manifests/cmangos-classic-z2815.json");
const DEFINITION_REVISION_DOMAIN: &[u8] = b"lyracore-eventai-definition-v1";
const DEFINITION_BATCH_BYTES: usize = 64 * 1024;
const RELAY_MAX_DEPTH: usize = 16;
const RELAY_STEP_BUDGET: usize = 4_096;
const RELAY_SCHEDULED_WORK_BUDGET: usize = 2_048;
const RELAY_CATALOGUE_VERSION_PLACEHOLDER: &str = "{relay-catalogue-version}";

const SCRIPT_FLAG_BUDDY_AS_TARGET: u32 = 0x001;
const SCRIPT_FLAG_REVERSE_DIRECTION: u32 = 0x002;
const SCRIPT_FLAG_SOURCE_TARGETS_SELF: u32 = 0x004;
const SCRIPT_FLAG_COMMAND_ADDITIONAL: u32 = 0x008;
const SCRIPT_FLAG_BUDDY_BY_GUID: u32 = 0x010;
const SCRIPT_FLAG_BUDDY_IS_PET: u32 = 0x020;
const SCRIPT_FLAG_BUDDY_IS_DESPAWNED: u32 = 0x040;
const SCRIPT_FLAG_BUDDY_BY_POOL: u32 = 0x080;
const SCRIPT_FLAG_BUDDY_BY_SPAWN_GROUP: u32 = 0x100;
const SCRIPT_FLAG_ALL_ELIGIBLE_BUDDIES: u32 = 0x200;
const SCRIPT_FLAG_BUDDY_BY_GO: u32 = 0x400;
const SCRIPT_FLAG_BUDDY_BY_STRING_ID: u32 = 0x800;
const SCRIPT_FLAG_PARTICIPANTS: u32 = SCRIPT_FLAG_BUDDY_AS_TARGET
    | SCRIPT_FLAG_REVERSE_DIRECTION
    | SCRIPT_FLAG_SOURCE_TARGETS_SELF
    | SCRIPT_FLAG_BUDDY_BY_GUID
    | SCRIPT_FLAG_BUDDY_IS_PET
    | SCRIPT_FLAG_BUDDY_IS_DESPAWNED
    | SCRIPT_FLAG_BUDDY_BY_POOL
    | SCRIPT_FLAG_BUDDY_BY_SPAWN_GROUP
    | SCRIPT_FLAG_ALL_ELIGIBLE_BUDDIES
    | SCRIPT_FLAG_BUDDY_BY_GO
    | SCRIPT_FLAG_BUDDY_BY_STRING_ID;

const EVENT_TIMER_IN_COMBAT: u32 = 0;
const EVENT_TIMER_OUT_OF_COMBAT: u32 = 1;
const EVENT_HP: u32 = 2;
const EVENT_MANA: u32 = 3;
const EVENT_AGGRO: u32 = 4;
const EVENT_KILL: u32 = 5;
const EVENT_DEATH: u32 = 6;
const EVENT_EVADE: u32 = 7;
const EVENT_SPELL_HIT: u32 = 8;
const EVENT_RANGE: u32 = 9;
const EVENT_OOC_LOS: u32 = 10;
const EVENT_SPAWNED: u32 = 11;
const EVENT_TARGET_HP: u32 = 12;
const EVENT_TARGET_CASTING: u32 = 13;
const EVENT_FRIENDLY_HP: u32 = 14;
const EVENT_FRIENDLY_CC: u32 = 15;
const EVENT_FRIENDLY_MISSING_AURA: u32 = 16;
const EVENT_SUMMONED: u32 = 17;
const EVENT_TARGET_MANA: u32 = 18;
const EVENT_HOME: u32 = 21;
const EVENT_RECEIVE_EMOTE: u32 = 22;
const EVENT_AURA: u32 = 23;
const EVENT_TARGET_AURA: u32 = 24;
const EVENT_SUMMONED_DEATH: u32 = 25;
const EVENT_MISSING_AURA: u32 = 27;
const EVENT_TARGET_MISSING_AURA: u32 = 28;
const EVENT_TIMER_GENERIC: u32 = 29;
const EVENT_RECEIVE_AI_EVENT: u32 = 30;
const EVENT_SELECT_ATTACKING: u32 = 32;
const EVENT_FACING: u32 = 33;
const EVENT_SPELL_HIT_TARGET: u32 = 34;
#[cfg(test)]
const EVENT_DEATH_PREVENTED: u32 = 35;
const EVENT_TARGET_NOT_REACHABLE: u32 = 36;

const ACTION_TEXT: u32 = 1;
const ACTION_SOUND: u32 = 4;
const ACTION_TEXT_NEW: u32 = 54;
const ACTION_EMOTE: u32 = 5;
const ACTION_RANDOM_EMOTE: u32 = 10;
const ACTION_CAST: u32 = 11;
const ACTION_SPAWN: u32 = 12;
const ACTION_THREAT_SINGLE: u32 = 13;
const ACTION_THREAT_ALL_PCT: u32 = 14;
const ACTION_QUEST_EVENT: u32 = 15;
const ACTION_COMBAT_MOVEMENT: u32 = 21;
const ACTION_SET_PHASE: u32 = 22;
const ACTION_INCREMENT_PHASE: u32 = 23;
const ACTION_EVADE: u32 = 24;
const ACTION_FLEE_FOR_ASSIST: u32 = 25;
const ACTION_REMOVE_AURA: u32 = 28;
const ACTION_RANGED_MOVEMENT: u32 = 29;
const ACTION_RANDOM_PHASE: u32 = 30;
const ACTION_RANDOM_PHASE_RANGE: u32 = 31;
const ACTION_SUMMON_ID: u32 = 32;
const ACTION_KILLED_MONSTER: u32 = 33;
const ACTION_SET_INSTANCE_DATA: u32 = 34;
const ACTION_SET_INSTANCE_DATA_GUID: u32 = 35;
const ACTION_DIE: u32 = 37;
const ACTION_CALL_FOR_HELP: u32 = 39;
const ACTION_FORCE_DESPAWN: u32 = 41;
const ACTION_SET_DEATH_PREVENTION: u32 = 42;
const ACTION_THROW_AI_EVENT: u32 = 45;
const ACTION_SET_STAND_STATE: u32 = 47;
const ACTION_CHANGE_MOVEMENT: u32 = 48;
const ACTION_SET_REACT_STATE: u32 = 50;
const ACTION_PAUSE_WAYPOINTS: u32 = 51;
const ACTION_START_RELAY: u32 = 53;
const ACTION_ATTACK_START: u32 = 55;
const ACTION_DESPAWN_GUARDIANS: u32 = 56;
const ACTION_SET_FACING: u32 = 59;
const ACTION_SET_RANGED_MODE: u32 = 57;
const ACTION_SET_WALK: u32 = 58;
const ACTION_SET_IMMOBILIZED: u32 = 61;
const ACTION_SET_FOLLOW_MOVEMENT: u32 = 64;
const ACTION_RETREAT: u32 = 65;
const ACTION_END: u16 = 66;

const TARGET_SELF: u32 = 0;
const TARGET_HOSTILE: u32 = 1;
const TARGET_HOSTILE_SECOND: u32 = 2;
const TARGET_HOSTILE_RANDOM: u32 = 4;
const TARGET_HOSTILE_RANDOM_EXCEPT_HIGHEST: u32 = 5;
const TARGET_ACTION_INVOKER: u32 = 6;
const TARGET_BENEFICIARY: u32 = 7;
const TARGET_HOSTILE_RANDOM_PLAYER: u32 = 8;
const TARGET_HOSTILE_RANDOM_PLAYER_EXCEPT_HIGHEST: u32 = 9;
const TARGET_AI_SENDER: u32 = 10;
const TARGET_SPAWNER: u32 = 11;
const TARGET_EVENT_SPECIFIC: u32 = 12;
const TARGET_NO_EXPLICIT: u32 = 15;
const TARGET_HOSTILE_MANA: u32 = 16;
const TARGET_NEAREST_AOE: u32 = 17;
const TARGET_HOSTILE_FARTHEST: u32 = 18;

const FLAG_REPEATABLE: u32 = 0x01;
const FLAG_RANDOM_ACTION: u32 = 0x20;
const FLAG_DEBUG_ONLY: u32 = 0x80;
const FLAG_RANGED_ONLY: u32 = 0x100;
const FLAG_MELEE_ONLY: u32 = 0x200;
const FLAG_COMBAT_ACTION: u32 = 0x400;
const FLAG_CLASSIC_RESERVED: u32 = 0x02 | 0x04 | 0x08 | 0x10;
const SUPPORTED_FLAGS: u32 =
    FLAG_REPEATABLE | FLAG_RANDOM_ACTION | FLAG_RANGED_ONLY | FLAG_MELEE_ONLY | FLAG_COMBAT_ACTION;

const CAST_INTERRUPT_PREVIOUS: u32 = 0x01;
const CAST_TRIGGERED: u32 = 0x02;
const CAST_FORCE_CAST: u32 = 0x04;
const CAST_FORCE_TARGET_SELF: u32 = 0x10;
const CAST_AURA_NOT_PRESENT: u32 = 0x20;
const CAST_IGNORE_UNSELECTABLE: u32 = 0x40;
const CAST_SWITCH_CASTER_TARGET: u32 = 0x80;
const CAST_MAIN_SPELL: u32 = 0x100;
const CAST_PLAYER_ONLY: u32 = 0x200;
const CAST_DISTANCE: u32 = 0x400;
const CAST_TARGET_CASTING: u32 = 0x800;
const SUPPORTED_CAST_FLAGS: u32 = CAST_INTERRUPT_PREVIOUS
    | CAST_TRIGGERED
    | CAST_FORCE_CAST
    | CAST_FORCE_TARGET_SELF
    | CAST_AURA_NOT_PRESENT
    | CAST_IGNORE_UNSELECTABLE
    | CAST_SWITCH_CASTER_TARGET
    | CAST_MAIN_SPELL
    | CAST_PLAYER_ONLY
    | CAST_DISTANCE
    | CAST_TARGET_CASTING;

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
        (self.definition_rows.len()
            + self.broadcast_rows.len()
            + self.summon_rows.len()
            + self.quest_event_entries.len()
            + self.relay_definition_rows.len()) as u64
    }

    pub(crate) fn quest_event_requirement_rows(&self) -> Vec<String> {
        self.quest_event_entries
            .iter()
            .enumerate()
            .map(|(index, quest_entry)| format!("({},{quest_entry})", index + 1))
            .collect()
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
    cast_action_subjects: u64,
    template_schedule_overlaps: u64,
    creature_spell_list_overlaps: u64,
    direct_threat_actions: u64,
    approvals: ApprovalRules,
    expected_source_census: BTreeMap<String, BTreeMap<u64, u64>>,
    expected_threat_percent_census: BTreeMap<i32, u64>,
    expected_ticket_census: BTreeMap<String, u64>,
}

#[derive(Clone, Default)]
struct ApprovalRules {
    classifications: BTreeSet<String>,
    events: BTreeSet<u64>,
    actions: BTreeSet<u64>,
    targets: BTreeSet<u64>,
    death_prevention_states: BTreeSet<u64>,
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
    let expected_threat_percent_census = root
        .get("expected_threat_percent_census")
        .and_then(serde_json::Value::as_object)
        .ok_or("EventAI source profile has no threat percent census")?
        .iter()
        .map(|(value, count)| {
            let value = value
                .parse::<i32>()
                .map_err(|_| format!("threat percent census value is not an integer: {value}"))?;
            let count = count
                .as_u64()
                .ok_or_else(|| format!("threat percent census count is not an integer: {value}"))?;
            Ok((value, count))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let expected_ticket_census = root
        .get("expected_ticket_census")
        .and_then(serde_json::Value::as_object)
        .ok_or("EventAI source profile has no expected_ticket_census object")?
        .iter()
        .map(|(name, value)| {
            value
                .as_u64()
                .map(|count| (name.clone(), count))
                .ok_or_else(|| format!("ticket census value is not a count: {name}"))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let source_profile = SourceProfile {
        name: string("name")?,
        cmangos_commit: string("cmangos_commit")?,
        classic_db_commit: string("classic_db_commit")?,
        sql_sha256: string("sql_sha256")?,
        loader_contract: string("loader_contract")?,
        source_rule_count: count("source_rule_count")?,
        source_guid_rule_count: count("source_guid_rule_count")?,
        cast_action_subjects: count("cast_action_subjects")?,
        template_schedule_overlaps: count("template_schedule_overlaps")?,
        creature_spell_list_overlaps: count("creature_spell_list_overlaps")?,
        direct_threat_actions: count("direct_threat_actions")?,
        approvals: ApprovalRules {
            classifications: strings("classifications")?,
            events: numbers("events")?,
            actions: numbers("actions")?,
            targets: numbers("targets")?,
            death_prevention_states: numbers("death_prevention_states")?,
            event_flag_bits: bit_mask("event_flag_bits")?,
            cast_flag_bits: bit_mask("cast_flag_bits")?,
            dependencies: strings("dependencies")?,
            reasons: strings("reasons")?,
            normalizations,
        },
        expected_source_census,
        expected_threat_percent_census,
        expected_ticket_census,
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
        if coverage.cast_action_subjects != profile.cast_action_subjects {
            findings.push(format!(
                "cast_action_subjects expected={} observed={}",
                profile.cast_action_subjects, coverage.cast_action_subjects
            ));
        }
        if coverage.template_schedule_overlaps != profile.template_schedule_overlaps {
            findings.push(format!(
                "template_schedule_overlaps expected={} observed={}",
                profile.template_schedule_overlaps, coverage.template_schedule_overlaps
            ));
        }
        if coverage.creature_spell_list_overlaps != profile.creature_spell_list_overlaps {
            findings.push(format!(
                "creature_spell_list_overlaps expected={} observed={}",
                profile.creature_spell_list_overlaps, coverage.creature_spell_list_overlaps
            ));
        }
        if coverage.direct_threat_actions != profile.direct_threat_actions {
            findings.push(format!(
                "source_direct_threat_actions expected={} observed={}",
                profile.direct_threat_actions, coverage.direct_threat_actions
            ));
        }
        if coverage.threat_percent != profile.expected_threat_percent_census {
            findings.push(format!(
                "source_threat_percent_census expected={} observed={}",
                render_signed_census(&profile.expected_threat_percent_census),
                render_signed_census(&coverage.threat_percent)
            ));
        }
        findings.extend(
            coverage
                .relay_parse_failures
                .iter()
                .map(|failure| format!("relay_parse_failure {failure}")),
        );
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
        for (name, expected) in &profile.expected_ticket_census {
            let observed = coverage.ticket_census.get(name).copied().unwrap_or(0);
            if observed != *expected {
                findings.push(format!(
                    "ticket_{name} expected={expected} observed={observed}"
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
                "cast_action_subjects": coverage.cast_action_subjects,
                "template_schedule_overlaps": coverage.template_schedule_overlaps,
                "creature_spell_list_overlaps": coverage.creature_spell_list_overlaps,
                "direct_threat_actions": coverage.direct_threat_actions,
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
                "ticket": coverage.ticket_census,
                "relay_parse_failures": coverage.relay_parse_failures.len(),
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
        let required = [
            "events",
            "actions",
            "targets",
            "event_flags",
            "cast_flags",
            "presentation_factions",
            "presentation_display_templates",
            "presentation_unit_fields",
            "presentation_set_unit_flags",
            "presentation_remove_unit_flags",
            "presentation_mount_models",
        ];
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
        if let Some(states) = self.expected_source_census.get("death_prevention_states") {
            let actions = actions
                .get(&u64::from(ACTION_SET_DEATH_PREVENTION))
                .copied()
                .unwrap_or(0);
            let state_count = states.values().sum::<u64>();
            if actions != state_count {
                return Err(format!(
                    "EventAI death-prevention census count mismatch: actions={actions} states={state_count}"
                ));
            }
        }
        let percent_threat_actions = actions
            .get(&u64::from(ACTION_THREAT_SINGLE))
            .copied()
            .unwrap_or(0)
            .saturating_sub(self.direct_threat_actions)
            + actions
                .get(&u64::from(ACTION_THREAT_ALL_PCT))
                .copied()
                .unwrap_or(0);
        let observed_percent_actions = self.expected_threat_percent_census.values().sum::<u64>();
        if percent_threat_actions != observed_percent_actions {
            return Err(format!(
                "EventAI threat percent census count mismatch: actions={percent_threat_actions} census={observed_percent_actions}"
            ));
        }
        for required in [
            "instance_actions",
            "instance_begin",
            "instance_fail",
            "instance_complete",
            "instance_special",
            "relay_root_rules",
            "relay_accepted_root_rules",
            "relay_refused_root_rules",
            "relay_definitions",
            "relay_accepted_definitions",
            "relay_refused_definitions",
            "relay_rows",
            "relay_accepted_rows",
            "relay_refused_rows",
            "relay_command_types",
            "relay_random_templates",
            "relay_nested_edges",
            "relay_structural_definitions",
            "relay_structural_rows",
            "relay_emitted_steps",
            "relay_loader_skipped_rows",
            "relay_arrival_edges",
            "relay_arrival_dependency_definitions",
            "relay_arrival_dependency_rows",
        ] {
            if !self.expected_ticket_census.contains_key(required) {
                return Err(format!(
                    "EventAI source profile has no {required} ticket census"
                ));
            }
        }
        Ok(())
    }

    fn approves(&self, key: &GroupKey) -> bool {
        if key.classification == "excluded" && key.reason == "outside_world_import_scope" {
            return true;
        }
        if !self.approvals.classifications.contains(&key.classification)
            || !self.approvals.reasons.contains(&key.reason)
        {
            return false;
        }
        let normalization_reason = self
            .approvals
            .normalizations
            .iter()
            .any(|(_, _, reason)| reason == &key.reason);
        let terminal_rule_normalization = key.dimension == "rule"
            && key.classification == "normalized"
            && key.reason == "normalized";
        if (key.classification == "normalized" || normalization_reason)
            && !terminal_rule_normalization
            && !self.approvals.normalizations.contains(&(
                key.dimension.clone(),
                key.raw_value.clone(),
                key.reason.clone(),
            ))
        {
            return false;
        }
        if key.classification == "normalized"
            && matches!(
                key.reason.as_str(),
                "invalid_action_to_none" | "normalized_random_phase_range"
            )
        {
            return true;
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
            "death_prevention_state" => key
                .raw_value
                .parse()
                .is_ok_and(|value| self.approvals.death_prevention_states.contains(&value)),
            "event_flag" => mask_has_no_residual(&key.raw_value, self.approvals.event_flag_bits),
            "cast_flag" => mask_has_no_residual(&key.raw_value, self.approvals.cast_flag_bits),
            "dependency" => self.approvals.dependencies.contains(&key.raw_value),
            "phase_range" => true,
            "threat_percent" => key
                .raw_value
                .parse()
                .is_ok_and(|value| self.expected_threat_percent_census.contains_key(&value)),
            "unit_flag" => true,
            "rule" => true,
            "relay_command" | "text_template" => true,
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

fn render_signed_census(values: &BTreeMap<i32, u64>) -> String {
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
    cast_action_subjects: u64,
    template_schedule_overlaps: u64,
    creature_spell_list_overlaps: u64,
    emitted_rules: u64,
    emitted_instructions: u64,
    normalized_rules: u64,
    excluded_rules: u64,
    dropped_rules: u64,
    accepted_rules: u64,
    action_rows: u64,
    direct_threat_actions: u64,
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
    death_prevention_states: BTreeMap<u64, u64>,
    threat_percent: BTreeMap<i32, u64>,
    presentation_factions: BTreeMap<u64, u64>,
    presentation_display_templates: BTreeMap<u64, u64>,
    presentation_unit_fields: BTreeMap<u64, u64>,
    presentation_set_unit_flags: BTreeMap<u64, u64>,
    presentation_remove_unit_flags: BTreeMap<u64, u64>,
    presentation_mount_models: BTreeMap<u64, u64>,
    movement_values: BTreeMap<String, u64>,
    movement_dependencies: MovementDependencyCensus,
    reachable_relays: ReachableRelayCensus,
    ticket_census: BTreeMap<String, u64>,
    relay_parse_failures: Vec<String>,
    dropped: BTreeMap<String, u64>,
    dropped_values: BTreeMap<(String, u64), u64>,
    dropped_rule_values: BTreeMap<(String, u64), BTreeSet<u64>>,
    groups: BTreeMap<GroupKey, GroupValue>,
}

#[derive(Clone, Default)]
struct MovementDependencyCensus {
    ranged_mode_actions: u64,
    ranged_mode_subjects: BTreeSet<i32>,
    resolved_main_spell_subjects: BTreeSet<i32>,
    missing_main_spell_subjects: BTreeSet<i32>,
}

#[derive(Clone, Default)]
struct ReachableRelayCensus {
    root_references: u64,
    direct_root_ids: BTreeSet<u32>,
    random_template_ids: BTreeSet<u32>,
    relay_ids: BTreeSet<u32>,
    rows: u64,
    command: BTreeMap<u64, u64>,
    nested_edges: u64,
    movement_values: BTreeMap<String, u64>,
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

    fn source_signed_value(values: &mut BTreeMap<i32, u64>, value: i32) {
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
        self.mark_rule_classification(classification);
        self.result("rule", rule_id, classification, reason, rule_id, None);
    }

    fn mark_rule_classification(&mut self, classification: &str) {
        match classification {
            "emitted" => self.emitted_rules += 1,
            "normalized" => self.normalized_rules += 1,
            "excluded" => self.excluded_rules += 1,
            "dropped" => self.dropped_rules += 1,
            _ => {}
        }
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
            "death_prevention_states" => self.death_prevention_states.clone(),
            "presentation_factions" => self.presentation_factions.clone(),
            "presentation_display_templates" => self.presentation_display_templates.clone(),
            "presentation_unit_fields" => self.presentation_unit_fields.clone(),
            "presentation_set_unit_flags" => self.presentation_set_unit_flags.clone(),
            "presentation_remove_unit_flags" => self.presentation_remove_unit_flags.clone(),
            "presentation_mount_models" => self.presentation_mount_models.clone(),
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
            "death_prevention_states": self.death_prevention_states,
            "threat_percents": self.threat_percent,
            "direct_threat_actions": self.direct_threat_actions,
            "presentation_factions": self.presentation_factions,
            "presentation_display_templates": self.presentation_display_templates,
            "presentation_unit_fields": self.presentation_unit_fields,
            "presentation_set_unit_flags": self.presentation_set_unit_flags,
            "presentation_remove_unit_flags": self.presentation_remove_unit_flags,
            "presentation_mount_models": self.presentation_mount_models,
            "movement_values": self.movement_values,
            "movement_dependencies": {
                "ranged_mode_actions": self.movement_dependencies.ranged_mode_actions,
                "ranged_mode_subjects": self.movement_dependencies.ranged_mode_subjects,
                "resolved_main_spell_subjects": self
                    .movement_dependencies
                    .resolved_main_spell_subjects,
                "missing_main_spell_subjects": self
                    .movement_dependencies
                    .missing_main_spell_subjects,
            },
            "reachable_relays": {
                "root_references": self.reachable_relays.root_references,
                "direct_root_ids": self.reachable_relays.direct_root_ids,
                "random_template_ids": self.reachable_relays.random_template_ids,
                "relay_ids": self.reachable_relays.relay_ids,
                "relay_count": self.reachable_relays.relay_ids.len(),
                "rows": self.reachable_relays.rows,
                "commands": self.reachable_relays.command,
                "raw_command_ids": self.reachable_relays.command.len(),
                "recognized_command_types": self.reachable_relays.command
                    .keys()
                    .filter(|command| **command != 45)
                    .count(),
                "unclassified_command_values": {},
                "command_type_classifications": {},
                "nested_edges": self.reachable_relays.nested_edges,
                "movement_rows": self.reachable_relays.movement_values.values().sum::<u64>(),
                "movement_value_columns": [
                    "command", "datalong", "datalong2", "datalong3", "buddy_entry",
                    "search_radius", "data_flags", "dataint", "dataint2", "dataint3",
                    "dataint4", "datafloat", "x", "y", "z", "o", "speed",
                    "condition_id"
                ],
                "movement_values": self.reachable_relays.movement_values,
            },
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

#[derive(Clone, Default)]
struct RelaySource {
    definitions: BTreeMap<u32, Vec<RawRelayStep>>,
    random_templates: BTreeMap<u32, Vec<RelayTemplateChoice>>,
    string_templates: BTreeMap<u32, Vec<RelayTemplateChoice>>,
    broadcast_ids: HashSet<u32>,
    spell_ids: HashSet<u32>,
    item_entries: HashSet<u32>,
    creature_entries: HashSet<u32>,
    gameobject_entries: HashSet<u32>,
    gameobject_trap_spells: HashMap<u32, u32>,
    creature_spawns: HashMap<u32, u32>,
    gameobject_spawns: HashMap<u32, u32>,
    path_ids: HashSet<u32>,
    pool_ids: HashSet<u32>,
    gossip_menu_ids: HashSet<u32>,
    world_state_ids: HashSet<u32>,
    parse_failures: Vec<String>,
}

#[derive(Clone)]
struct RelayTemplateChoice {
    target_id: u32,
    chance: u32,
}

#[derive(Clone)]
struct RawRelayStep {
    offset_ms: u32,
    priority: u32,
    source_order: u32,
    command: u32,
    datalong: u32,
    datalong2: u32,
    datalong3: u32,
    buddy_entry: u32,
    search_radius: u32,
    flags: u32,
    dataints: [i32; 4],
    x: f32,
    y: f32,
    z: f32,
    orientation: f32,
    speed: f32,
    condition_id: u32,
}

impl RelaySource {
    fn parse(dump: &str) -> Self {
        let mut source = Self::default();
        source.broadcast_ids.extend(
            parse_table(dump, "broadcast_text")
                .into_iter()
                .filter_map(|row| source_u32(field(&row, 0))),
        );
        source.broadcast_ids.extend(
            parse_table(dump, "script_texts")
                .into_iter()
                .filter_map(|row| field(&row, 0).parse::<i32>().ok())
                .map(|id| id as u32),
        );
        source.spell_ids.extend(
            parse_table(dump, "spell_template")
                .into_iter()
                .filter_map(|row| source_u32(field(&row, 0))),
        );
        source.item_entries.extend(
            parse_table(dump, "item_template")
                .into_iter()
                .filter_map(|row| source_u32(field(&row, 0))),
        );
        source.creature_entries.extend(
            parse_table(dump, "creature_template")
                .into_iter()
                .filter_map(|row| source_u32(field(&row, 0))),
        );
        for row in parse_table(dump, "gameobject_template") {
            let Some(entry) = source_u32(field(&row, 0)) else {
                continue;
            };
            source.gameobject_entries.insert(entry);
            if source_u32(field(&row, 1)) == Some(6) {
                if let Some(spell_id) = source_u32(field(&row, 11)).filter(|id| *id != 0) {
                    source.gameobject_trap_spells.insert(entry, spell_id);
                }
            }
        }
        source.creature_spawns.extend(
            parse_table(dump, "creature")
                .into_iter()
                .filter_map(|row| Some((source_u32(field(&row, 0))?, source_u32(field(&row, 1))?))),
        );
        source.gameobject_spawns.extend(
            parse_table(dump, "gameobject")
                .into_iter()
                .filter_map(|row| Some((source_u32(field(&row, 0))?, source_u32(field(&row, 1))?))),
        );
        source.path_ids.extend(
            parse_table(dump, "creature_movement_template")
                .into_iter()
                .filter_map(|row| source_u32(field(&row, 0))),
        );
        source.path_ids.extend(
            parse_table(dump, "waypoint_path")
                .into_iter()
                .filter_map(|row| source_u32(field(&row, 0))),
        );
        source.pool_ids.extend(
            parse_table(dump, "pool_template")
                .into_iter()
                .filter_map(|row| source_u32(field(&row, 0))),
        );
        source.gossip_menu_ids.extend(
            parse_table(dump, "gossip_menu")
                .into_iter()
                .filter_map(|row| source_u32(field(&row, 0))),
        );
        source.world_state_ids.extend(
            parse_table(dump, "worldstate_name")
                .into_iter()
                .filter_map(|row| source_u32(field(&row, 0))),
        );
        for (row_number, row) in parse_table(dump, "dbscripts_on_relay")
            .into_iter()
            .enumerate()
        {
            if row.len() < 22 {
                source.parse_failures.push(format!(
                    "dbscripts_on_relay row {row_number} has {} fields, expected 22",
                    row.len()
                ));
                continue;
            }
            let parsed = (|| -> Result<(u32, RawRelayStep), String> {
                let u32_at = |index: usize, name: &str| {
                    field(&row, index).parse::<u32>().map_err(|_| {
                        format!(
                            "dbscripts_on_relay row {row_number} field {name} is not u32: {}",
                            field(&row, index)
                        )
                    })
                };
                let i32_at = |index: usize, name: &str| {
                    field(&row, index).parse::<i32>().map_err(|_| {
                        format!(
                            "dbscripts_on_relay row {row_number} field {name} is not i32: {}",
                            field(&row, index)
                        )
                    })
                };
                let f32_at = |index: usize, name: &str| {
                    field(&row, index).parse::<f32>().map_err(|_| {
                        format!(
                            "dbscripts_on_relay row {row_number} field {name} is not f32: {}",
                            field(&row, index)
                        )
                    })
                };
                let relay_id = u32_at(0, "id")?;
                f32_at(14, "datafloat")?;
                let speed = f32_at(19, "speed")?;
                Ok((
                    relay_id,
                    RawRelayStep {
                        offset_ms: u32_at(1, "delay")?,
                        priority: u32_at(2, "priority")?,
                        source_order: row_number as u32,
                        command: u32_at(3, "command")?,
                        datalong: u32_at(4, "datalong")?,
                        datalong2: u32_at(5, "datalong2")?,
                        datalong3: u32_at(6, "datalong3")?,
                        buddy_entry: u32_at(7, "buddy_entry")?,
                        search_radius: u32_at(8, "search_radius")?,
                        flags: u32_at(9, "data_flags")?,
                        dataints: [
                            i32_at(10, "dataint")?,
                            i32_at(11, "dataint2")?,
                            i32_at(12, "dataint3")?,
                            i32_at(13, "dataint4")?,
                        ],
                        x: f32_at(15, "x")?,
                        y: f32_at(16, "y")?,
                        z: f32_at(17, "z")?,
                        orientation: f32_at(18, "o")?,
                        speed,
                        condition_id: u32_at(20, "condition_id")?,
                    },
                ))
            })();
            match parsed {
                Ok((relay_id, step)) => {
                    source.definitions.entry(relay_id).or_default().push(step);
                }
                Err(error) => source.parse_failures.push(error),
            }
        }
        for (row_number, row) in parse_table(dump, "dbscript_random_templates")
            .into_iter()
            .enumerate()
        {
            if row.len() < 4 {
                source.parse_failures.push(format!(
                    "dbscript_random_templates row {row_number} has {} fields, expected at least 4",
                    row.len()
                ));
                continue;
            }
            let parsed = (|| -> Result<(u32, u32, i32, u32), String> {
                let template_id = field(&row, 0).parse::<u32>().map_err(|_| {
                    format!("dbscript_random_templates row {row_number} has invalid id")
                })?;
                let template_type = field(&row, 1).parse::<u32>().map_err(|_| {
                    format!("dbscript_random_templates row {row_number} has invalid type")
                })?;
                let target_id = field(&row, 2).parse::<i32>().map_err(|_| {
                    format!("dbscript_random_templates row {row_number} has invalid target_id")
                })?;
                let chance = field(&row, 3).parse::<u32>().map_err(|_| {
                    format!("dbscript_random_templates row {row_number} has invalid chance")
                })?;
                Ok((template_id, template_type, target_id, chance))
            })();
            let (template_id, template_type, target_id, chance) = match parsed {
                Ok(values) => values,
                Err(error) => {
                    source.parse_failures.push(error);
                    continue;
                }
            };
            if !matches!(template_type, 0 | 1) {
                continue;
            }
            if target_id > 0 {
                let templates = if template_type == 0 {
                    &mut source.string_templates
                } else {
                    &mut source.random_templates
                };
                templates
                    .entry(template_id)
                    .or_default()
                    .push(RelayTemplateChoice {
                        target_id: target_id as u32,
                        chance,
                    });
            } else if target_id < 0 {
                source.parse_failures.push(format!(
                    "dbscript_random_templates row {row_number} has nonpositive target {target_id}"
                ));
            }
        }
        for steps in source.definitions.values_mut() {
            steps.sort_by_key(|step| (step.offset_ms, step.priority, step.source_order));
        }
        source
    }

    fn ticket_census(&self, rules: &[RawRule]) -> BTreeMap<String, u64> {
        let instance_states = rules
            .iter()
            .flat_map(|rule| rule.actions)
            .filter(|action| action[0] == ACTION_SET_INSTANCE_DATA)
            .map(|action| action[2])
            .collect::<Vec<_>>();
        let relay_actions = rules
            .iter()
            .flat_map(|rule| rule.actions)
            .filter(|action| action[0] == ACTION_START_RELAY)
            .collect::<Vec<_>>();
        let mut roots = BTreeSet::new();
        let mut templates = BTreeSet::new();
        for action in &relay_actions {
            if (action[1] as i32) < 0 {
                templates.insert((action[1] as i32).unsigned_abs());
            }
            if let Ok(choices) = self.root_choices(action[1]) {
                roots.extend(choices);
            }
        }
        let structural_closure = self.structural_closure(&roots);
        let structural_rows = structural_closure
            .iter()
            .filter_map(|id| self.definitions.get(id))
            .flatten()
            .collect::<Vec<_>>();
        let closure = self.closure(&roots);
        let rows = closure
            .iter()
            .filter_map(|id| self.definitions.get(id))
            .flatten()
            .collect::<Vec<_>>();
        let mut accepted_root_rules = 0_u64;
        let mut accepted_root_ids = BTreeSet::new();
        for rule in rules {
            for (slot, action) in rule.actions.iter().enumerate() {
                if action[0] != ACTION_START_RELAY {
                    continue;
                }
                if let Ok(roots) = self.gate_action(action[1], rule.id, slot) {
                    accepted_root_rules += 1;
                    accepted_root_ids.extend(roots);
                }
            }
        }
        let accepted_closure = self.closure(&accepted_root_ids);
        let accepted_rows = accepted_closure
            .iter()
            .filter_map(|id| self.definitions.get(id))
            .map(Vec::len)
            .sum::<usize>();
        let loader_skipped_rows = rows.iter().filter(|row| row.is_loader_skipped()).count();
        let command_types = rows
            .iter()
            .filter(|row| row.command != 45)
            .map(|row| row.command)
            .collect::<BTreeSet<_>>();
        BTreeMap::from([
            ("instance_actions".to_string(), instance_states.len() as u64),
            (
                "instance_begin".to_string(),
                instance_states.iter().filter(|state| **state == 1).count() as u64,
            ),
            (
                "instance_fail".to_string(),
                instance_states.iter().filter(|state| **state == 2).count() as u64,
            ),
            (
                "instance_complete".to_string(),
                instance_states.iter().filter(|state| **state == 3).count() as u64,
            ),
            (
                "instance_special".to_string(),
                instance_states.iter().filter(|state| **state == 4).count() as u64,
            ),
            ("relay_root_rules".to_string(), relay_actions.len() as u64),
            ("relay_accepted_root_rules".to_string(), accepted_root_rules),
            (
                "relay_refused_root_rules".to_string(),
                relay_actions.len() as u64 - accepted_root_rules,
            ),
            ("relay_definitions".to_string(), closure.len() as u64),
            (
                "relay_structural_definitions".to_string(),
                structural_closure.len() as u64,
            ),
            (
                "relay_accepted_definitions".to_string(),
                accepted_closure.len() as u64,
            ),
            (
                "relay_refused_definitions".to_string(),
                (closure.len() - accepted_closure.len()) as u64,
            ),
            ("relay_rows".to_string(), rows.len() as u64),
            (
                "relay_structural_rows".to_string(),
                structural_rows.len() as u64,
            ),
            (
                "relay_emitted_steps".to_string(),
                (rows.len() - loader_skipped_rows) as u64,
            ),
            (
                "relay_loader_skipped_rows".to_string(),
                loader_skipped_rows as u64,
            ),
            ("relay_accepted_rows".to_string(), accepted_rows as u64),
            (
                "relay_refused_rows".to_string(),
                (rows.len() - accepted_rows) as u64,
            ),
            (
                "relay_command_types".to_string(),
                command_types.len() as u64,
            ),
            ("relay_random_templates".to_string(), templates.len() as u64),
            (
                "relay_nested_edges".to_string(),
                rows.iter().filter(|row| row.command == 45).count() as u64,
            ),
            (
                "relay_arrival_edges".to_string(),
                rows.iter()
                    .filter(|row| row.arrival_relay_id().is_some())
                    .count() as u64,
            ),
            (
                "relay_arrival_dependency_definitions".to_string(),
                closure.len().saturating_sub(structural_closure.len()) as u64,
            ),
            (
                "relay_arrival_dependency_rows".to_string(),
                rows.len().saturating_sub(structural_rows.len()) as u64,
            ),
        ])
    }

    fn root_choices(&self, raw_relay: u32) -> Result<Vec<u32>, MappingFailure> {
        let signed = raw_relay as i32;
        if signed >= 0 {
            return Ok(vec![raw_relay]);
        }
        let template_id = signed.unsigned_abs();
        let choices = self.random_templates.get(&template_id).ok_or_else(|| {
            MappingFailure::dependency(
                "relay_random_template",
                u64::from(template_id),
                "missing",
                format!("relay_random_template:{template_id}"),
            )
        })?;
        uniform_template_targets(choices).map_err(|reason| {
            MappingFailure::dependency(
                "relay_random_template",
                u64::from(template_id),
                reason,
                format!("relay_random_template:{template_id}"),
            )
        })
    }

    fn gate_action(
        &self,
        raw_relay: u32,
        rule_id: u64,
        slot: usize,
    ) -> Result<Vec<u32>, Vec<MappingFailure>> {
        let roots = self.root_choices(raw_relay).map_err(|mut failure| {
            failure.dependency_path = Some(format!(
                "rule:{rule_id} -> action:{slot} -> {}",
                failure
                    .dependency_path
                    .as_deref()
                    .unwrap_or("relay_random_template")
            ));
            vec![failure]
        })?;
        let mut failures = Vec::new();
        for root in &roots {
            if let Err(failure) = self.gate_root(*root, rule_id, slot) {
                failures.push(failure);
            }
        }
        if failures.is_empty() {
            Ok(roots)
        } else {
            Err(failures)
        }
    }

    fn action_dependencies(
        &self,
        raw_relay: u32,
        roots: &[u32],
        rule_id: u64,
        slot: usize,
    ) -> Vec<Dependency> {
        fn visit(
            source: &RelaySource,
            relay_id: u32,
            prefix: &str,
            dependencies: &mut Vec<Dependency>,
        ) {
            let relay_path = format!("{prefix} -> relay:{relay_id}");
            dependencies.push(Dependency {
                kind: "relay_definition",
                path: relay_path.clone(),
            });
            let rows = source
                .definitions
                .get(&relay_id)
                .expect("validated relay dependency exists");
            for row in rows.iter().filter(|row| !row.is_loader_skipped()) {
                let location = format!("{relay_path} -> row:{}", row.source_order);
                for dependency in row
                    .resolved_dependencies(source)
                    .expect("validated relay leaf dependencies exist")
                {
                    dependencies.push(Dependency {
                        kind: dependency.kind,
                        path: format!("{location} -> {}:{}", dependency.kind, dependency.raw_value),
                    });
                }
                if let Some(arrival_relay) = row.arrival_relay_id() {
                    dependencies.push(Dependency {
                        kind: "relay_arrival",
                        path: format!("{location} -> arrival-relay:{arrival_relay}"),
                    });
                }
                for target in row
                    .execution_targets(source)
                    .expect("validated relay execution dependencies exist")
                {
                    visit(source, target, &location, dependencies);
                }
            }
        }

        let action_path = format!("rule:{rule_id} -> action:{slot}");
        let mut dependencies = Vec::new();
        let signed_relay = raw_relay as i32;
        if signed_relay < 0 {
            let template_id = signed_relay.unsigned_abs();
            dependencies.push(Dependency {
                kind: "relay_random_template",
                path: format!("{action_path} -> relay_random_template:{template_id}"),
            });
        }
        for root in roots {
            visit(self, *root, &action_path, &mut dependencies);
        }
        dependencies
    }

    fn gate_root(&self, root: u32, rule_id: u64, slot: usize) -> Result<(), MappingFailure> {
        fn visit(
            source: &RelaySource,
            relay_id: u32,
            path: &mut Vec<u32>,
            steps: &mut usize,
            scheduled: &mut usize,
            rule_id: u64,
            slot: usize,
        ) -> Result<(), MappingFailure> {
            let relay_path = |path: &[u32]| {
                path.iter()
                    .map(|id| format!("relay:{id}"))
                    .collect::<Vec<_>>()
                    .join(" -> ")
            };
            if path.contains(&relay_id) {
                path.push(relay_id);
                return Err(MappingFailure::dependency(
                    "relay_definition",
                    u64::from(relay_id),
                    "cycle",
                    format!("rule:{rule_id} -> action:{slot} -> {}", relay_path(path)),
                ));
            }
            if path.len() >= RELAY_MAX_DEPTH {
                return Err(MappingFailure::dependency(
                    "relay_definition",
                    u64::from(relay_id),
                    format!("depth_exceeds_{RELAY_MAX_DEPTH}"),
                    format!(
                        "rule:{rule_id} -> action:{slot} -> {} -> relay:{relay_id}",
                        relay_path(path)
                    ),
                ));
            }
            let rows = source.definitions.get(&relay_id).ok_or_else(|| {
                let mut missing_path = path.clone();
                missing_path.push(relay_id);
                MappingFailure::dependency(
                    "relay_definition",
                    u64::from(relay_id),
                    "missing",
                    format!(
                        "rule:{rule_id} -> action:{slot} -> {}",
                        relay_path(&missing_path)
                    ),
                )
            })?;
            path.push(relay_id);
            for row in rows {
                if row.is_loader_skipped() {
                    continue;
                }
                let location = format!(
                    "rule:{rule_id} -> action:{slot} -> {} -> row:{}",
                    relay_path(path),
                    row.source_order,
                );
                if row.condition_id != 0 {
                    return Err(MappingFailure::dependency(
                        "relay_definition",
                        u64::from(relay_id),
                        format!("unsupported_condition_{}", row.condition_id),
                        location,
                    ));
                }
                row.resolved_dependencies(source)
                    .map_err(|failure| failure.mapping_failure(&location))?;
                row.gate_participants(source).map_err(|reason| {
                    MappingFailure::dependency(
                        "relay_participant",
                        u64::from(row.flags),
                        reason,
                        format!("{location} -> participant"),
                    )
                })?;
                if !row.has_gameplay_authority() {
                    return Err(MappingFailure::dependency(
                        "relay_definition",
                        u64::from(relay_id),
                        format!("no_gameplay_authority_for_command_{}", row.command),
                        location,
                    ));
                }
                row.encode_instruction_with_source(source)
                    .map_err(|reason| {
                        MappingFailure::dependency(
                            "relay_definition",
                            u64::from(relay_id),
                            reason,
                            location.clone(),
                        )
                    })?;
                *steps += 1;
                *scheduled += usize::from(row.offset_ms != 0);
                if *steps > RELAY_STEP_BUDGET {
                    return Err(MappingFailure::dependency(
                        "relay_definition",
                        u64::from(relay_id),
                        format!("step_budget_exceeds_{RELAY_STEP_BUDGET}"),
                        location,
                    ));
                }
                if *scheduled > RELAY_SCHEDULED_WORK_BUDGET {
                    return Err(MappingFailure::dependency(
                        "relay_definition",
                        u64::from(relay_id),
                        format!("scheduled_work_budget_exceeds_{RELAY_SCHEDULED_WORK_BUDGET}"),
                        location,
                    ));
                }
                for target in row.execution_targets(source).map_err(|failure| {
                    failure.mapping_failure(&format!("{location} -> relay_dependency"))
                })? {
                    visit(source, target, path, steps, scheduled, rule_id, slot)?;
                }
            }
            path.pop();
            Ok(())
        }

        let mut path = Vec::new();
        let mut steps = 0;
        let mut scheduled = 0;
        visit(
            self,
            root,
            &mut path,
            &mut steps,
            &mut scheduled,
            rule_id,
            slot,
        )
    }

    fn structural_closure(&self, roots: &BTreeSet<u32>) -> BTreeSet<u32> {
        fn add(source: &RelaySource, id: u32, closure: &mut BTreeSet<u32>) {
            if !closure.insert(id) {
                return;
            }
            if let Some(rows) = source.definitions.get(&id) {
                for row in rows.iter().filter(|row| row.command == 45) {
                    if let Ok(targets) = row.nested_targets(source) {
                        for target in targets {
                            add(source, target, closure);
                        }
                    }
                }
            }
        }
        let mut closure = BTreeSet::new();
        for root in roots {
            add(self, *root, &mut closure);
        }
        closure
    }

    fn closure(&self, roots: &BTreeSet<u32>) -> BTreeSet<u32> {
        fn add(source: &RelaySource, id: u32, closure: &mut BTreeSet<u32>) {
            if !closure.insert(id) {
                return;
            }
            if let Some(rows) = source.definitions.get(&id) {
                for row in rows {
                    if let Ok(targets) = row.execution_targets(source) {
                        for target in targets {
                            add(source, target, closure);
                        }
                    }
                }
            }
        }
        let mut closure = BTreeSet::new();
        for root in roots {
            add(self, *root, &mut closure);
        }
        closure
    }

    fn encode_closure(&self, roots: &BTreeSet<u32>) -> Vec<String> {
        self.closure(roots)
            .into_iter()
            .filter_map(|relay_id| {
                let steps = self.definitions.get(&relay_id)?;
                let encoded_steps = steps
                    .iter()
                    .filter(|step| !step.is_loader_skipped())
                    .map(|step| step.encode(self))
                    .collect::<Result<Vec<_>, _>>()
                    // Every relay in this closure passed `gate_root` (each instruction encodes) and
                    // `gate_participants` (each subject resolves), which together cover both
                    // failure arms of `encode`. A panic here means a closure reached encoding
                    // without passing one of those two Gates.
                    .expect("relay closure passed both relay Gates before encoding");
                let steps = encoded_steps.join("~");
                let material = format!("{relay_id}@parallel@map-or-instance@{steps}");
                let mut hasher = blake3::Hasher::new();
                hasher.update(b"lyracore-relay-definition-v1");
                hasher.update(material.as_bytes());
                let version =
                    u64::from_le_bytes(hasher.finalize().as_bytes()[..8].try_into().unwrap());
                Some(format!(
                    "{relay_id}@{version}@parallel@map-or-instance@{steps}"
                ))
            })
            .collect()
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

    fn closure_broadcast_ids(&self, roots: &BTreeSet<u32>) -> BTreeSet<u32> {
        self.closure(roots)
            .into_iter()
            .filter_map(|relay_id| self.definitions.get(&relay_id))
            .flatten()
            .filter(|step| step.command == 0)
            .flat_map(|step| step.talk_ids(self).unwrap_or_default())
            .collect()
    }
}

fn uniform_template_targets(choices: &[RelayTemplateChoice]) -> Result<Vec<u32>, String> {
    if choices.is_empty() {
        return Err("empty".to_string());
    }
    if choices.iter().any(|choice| choice.chance != 0) {
        return Err("weighted_choices_are_not_supported".to_string());
    }
    Ok(choices.iter().map(|choice| choice.target_id).collect())
}

#[derive(Debug)]
struct RelayDependencyFailure {
    kind: &'static str,
    raw_value: u64,
    reason: String,
}

#[derive(Clone, Copy)]
struct ResolvedRelayDependency {
    kind: &'static str,
    raw_value: u64,
}

impl RelayDependencyFailure {
    fn missing(kind: &'static str, raw_value: impl Into<u64>) -> Self {
        Self {
            kind,
            raw_value: raw_value.into(),
            reason: "missing".to_string(),
        }
    }

    fn mapping_failure(self, location: &str) -> MappingFailure {
        MappingFailure::dependency(
            self.kind,
            self.raw_value,
            self.reason,
            format!("{location} -> {}:{}", self.kind, self.raw_value),
        )
    }
}

impl RawRelayStep {
    fn is_loader_skipped(&self) -> bool {
        self.command == 1 && self.datalong == 0
    }

    fn arrival_relay_id(&self) -> Option<u32> {
        (self.command == 37)
            .then(|| u32::try_from(self.dataints[1]).ok().filter(|id| *id != 0))
            .flatten()
    }

    fn execution_targets(&self, source: &RelaySource) -> Result<Vec<u32>, RelayDependencyFailure> {
        if self.command == 45 {
            return self.nested_targets(source);
        }
        if self.command != 37 || self.dataints[1] == 0 {
            return Ok(Vec::new());
        }
        let relay_id = u32::try_from(self.dataints[1]).map_err(|_| RelayDependencyFailure {
            kind: "relay_definition",
            raw_value: self.dataints[1].unsigned_abs() as u64,
            reason: "negative_arrival_relay".to_string(),
        })?;
        Ok(vec![relay_id])
    }

    fn has_gameplay_authority(&self) -> bool {
        matches!(
            self.command,
            0 | 1
                | 3
                | 10
                | 13
                | 15
                | 18
                | 20
                | 21
                | 22
                | 25
                | 26
                | 28
                | 29
                | 31
                | 32
                | 35
                | 36
                | 37
                | 40
                | 42
                | 44
                | 45
                | 48
                | 52
                | 53
        )
    }

    fn buddy_is_gameobject(&self) -> bool {
        matches!(self.command, 13 | 40)
            || matches!(self.command, 31 | 36 | 37) && self.flags & SCRIPT_FLAG_BUDDY_BY_GO != 0
    }

    fn gate_participants(&self, source: &RelaySource) -> Result<(), String> {
        let additional_allowed = matches!(self.command, 3 | 10 | 15 | 20 | 31 | 36 | 37);
        if self.flags & SCRIPT_FLAG_COMMAND_ADDITIONAL != 0 && !additional_allowed {
            return Err(format!(
                "command {} does not define additional flag semantics",
                self.command
            ));
        }
        let residual = self.flags & !(SCRIPT_FLAG_PARTICIPANTS | SCRIPT_FLAG_COMMAND_ADDITIONAL);
        if residual != 0 {
            return Err(format!("unsupported participant flag bits {residual:#x}"));
        }
        let unsupported_modes = self.flags
            & (SCRIPT_FLAG_BUDDY_IS_PET
                | SCRIPT_FLAG_BUDDY_IS_DESPAWNED
                | SCRIPT_FLAG_BUDDY_BY_POOL
                | SCRIPT_FLAG_BUDDY_BY_SPAWN_GROUP
                | SCRIPT_FLAG_BUDDY_BY_STRING_ID);
        if unsupported_modes != 0 {
            return Err(format!(
                "unsupported buddy lookup mode {unsupported_modes:#x}"
            ));
        }
        let lookup_bits = self.flags
            & (SCRIPT_FLAG_BUDDY_AS_TARGET
                | SCRIPT_FLAG_BUDDY_BY_GUID
                | SCRIPT_FLAG_ALL_ELIGIBLE_BUDDIES);
        if self.buddy_entry == 0 && lookup_bits != 0 {
            return Err("buddy lookup flags require a buddy entry".to_string());
        }
        if self.buddy_entry == 0 && self.flags & SCRIPT_FLAG_BUDDY_BY_GO != 0 && self.command != 31
        {
            return Err("gameobject buddy flag requires a buddy entry".to_string());
        }
        if self.buddy_entry != 0
            && self.flags & SCRIPT_FLAG_BUDDY_BY_GO != 0
            && !matches!(self.command, 13 | 36 | 37 | 40)
        {
            return Err(format!(
                "command {} cannot resolve a gameobject buddy",
                self.command
            ));
        }
        if self.flags & SCRIPT_FLAG_ALL_ELIGIBLE_BUDDIES != 0
            && (self.flags & SCRIPT_FLAG_BUDDY_BY_GUID != 0 || self.buddy_is_gameobject())
        {
            return Err("all-buddy lookup only supports nearby creatures".to_string());
        }
        if self.command == 13
            && (self.buddy_entry == 0 || self.flags & SCRIPT_FLAG_BUDDY_AS_TARGET == 0)
        {
            return Err("activate-object requires a gameobject target".to_string());
        }
        if self.flags & SCRIPT_FLAG_BUDDY_BY_GUID != 0 {
            if self.buddy_is_gameobject() {
                if source.gameobject_spawns.get(&self.search_radius) != Some(&self.buddy_entry) {
                    return Err(format!(
                        "gameobject DB guid {} has no live GUID mapping",
                        self.search_radius
                    ));
                }
            } else if source.creature_spawns.get(&self.search_radius) != Some(&self.buddy_entry) {
                return Err(format!(
                    "creature DB guid {} does not map to entry {}",
                    self.search_radius, self.buddy_entry
                ));
            }
        }
        Ok(())
    }

    fn resolved_dependencies(
        &self,
        source: &RelaySource,
    ) -> Result<Vec<ResolvedRelayDependency>, RelayDependencyFailure> {
        let mut dependencies = Vec::new();
        if self.buddy_entry != 0 {
            let is_gameobject = self.buddy_is_gameobject();
            let (kind, exists) = if is_gameobject {
                (
                    "gameobject_template",
                    source.gameobject_entries.contains(&self.buddy_entry),
                )
            } else {
                (
                    "creature_template",
                    source.creature_entries.contains(&self.buddy_entry),
                )
            };
            if !exists {
                return Err(RelayDependencyFailure::missing(kind, self.buddy_entry));
            }
            dependencies.push(ResolvedRelayDependency {
                kind,
                raw_value: u64::from(self.buddy_entry),
            });
            if self.flags & SCRIPT_FLAG_BUDDY_BY_GUID != 0 {
                let (kind, entry) = if is_gameobject {
                    (
                        "gameobject_spawn",
                        source.gameobject_spawns.get(&self.search_radius),
                    )
                } else {
                    (
                        "creature_spawn",
                        source.creature_spawns.get(&self.search_radius),
                    )
                };
                match entry {
                    None => {
                        return Err(RelayDependencyFailure::missing(kind, self.search_radius));
                    }
                    Some(entry) if *entry != self.buddy_entry => {
                        return Err(RelayDependencyFailure {
                            kind,
                            raw_value: u64::from(self.search_radius),
                            reason: format!("entry_{}_does_not_match_{}", entry, self.buddy_entry),
                        });
                    }
                    Some(_) => {}
                }
                dependencies.push(ResolvedRelayDependency {
                    kind,
                    raw_value: u64::from(self.search_radius),
                });
            }
        }
        match self.command {
            0 => {
                if self.datalong != 0 {
                    dependencies.push(ResolvedRelayDependency {
                        kind: "relay_string_template",
                        raw_value: u64::from(self.datalong),
                    });
                }
                for id in self.talk_ids(source)? {
                    if !source.broadcast_ids.contains(&id) {
                        return Err(RelayDependencyFailure::missing("broadcast_text", id));
                    }
                    dependencies.push(ResolvedRelayDependency {
                        kind: "broadcast_text",
                        raw_value: u64::from(id),
                    });
                }
            }
            3 if self.datalong != 0 => {
                if !source.definitions.contains_key(&self.datalong) {
                    return Err(RelayDependencyFailure::missing(
                        "relay_definition",
                        self.datalong,
                    ));
                }
                dependencies.push(ResolvedRelayDependency {
                    kind: "relay_definition",
                    raw_value: u64::from(self.datalong),
                });
            }
            10 => {
                if !source.creature_entries.contains(&self.datalong) {
                    return Err(RelayDependencyFailure::missing(
                        "creature_template",
                        self.datalong,
                    ));
                }
                dependencies.push(ResolvedRelayDependency {
                    kind: "creature_template",
                    raw_value: u64::from(self.datalong),
                });
                if self.datalong3 != 0 {
                    if !source.path_ids.contains(&self.datalong3) {
                        return Err(RelayDependencyFailure::missing(
                            "creature_movement_template",
                            self.datalong3,
                        ));
                    }
                    dependencies.push(ResolvedRelayDependency {
                        kind: "creature_movement_template",
                        raw_value: u64::from(self.datalong3),
                    });
                }
            }
            13 => {
                let Some(spell_id) = source.gameobject_trap_spells.get(&self.buddy_entry) else {
                    return Err(RelayDependencyFailure::missing(
                        "gameobject_trap",
                        self.buddy_entry,
                    ));
                };
                dependencies.push(ResolvedRelayDependency {
                    kind: "gameobject_trap",
                    raw_value: u64::from(self.buddy_entry),
                });
                if !source.spell_ids.contains(spell_id) {
                    return Err(RelayDependencyFailure::missing("spell_template", *spell_id));
                }
                dependencies.push(ResolvedRelayDependency {
                    kind: "spell_template",
                    raw_value: u64::from(*spell_id),
                });
            }
            15 => {
                for spell_id in self.random_ids(self.datalong)? {
                    if !source.spell_ids.contains(&spell_id) {
                        return Err(RelayDependencyFailure::missing("spell_template", spell_id));
                    }
                    dependencies.push(ResolvedRelayDependency {
                        kind: "spell_template",
                        raw_value: u64::from(spell_id),
                    });
                }
            }
            20 if self.datalong == 2 && self.datalong2 != 0 => {
                if !source.path_ids.contains(&self.datalong2) {
                    return Err(RelayDependencyFailure::missing(
                        "creature_movement_template",
                        self.datalong2,
                    ));
                }
                dependencies.push(ResolvedRelayDependency {
                    kind: "creature_movement_template",
                    raw_value: u64::from(self.datalong2),
                });
            }
            31 => {
                if self.datalong != 0 {
                    let (kind, exists) = if self.flags & SCRIPT_FLAG_BUDDY_BY_GO != 0 {
                        (
                            "gameobject_template",
                            source.gameobject_entries.contains(&self.datalong),
                        )
                    } else {
                        (
                            "creature_template",
                            source.creature_entries.contains(&self.datalong),
                        )
                    };
                    if !exists {
                        return Err(RelayDependencyFailure::missing(kind, self.datalong));
                    }
                    dependencies.push(ResolvedRelayDependency {
                        kind,
                        raw_value: u64::from(self.datalong),
                    });
                }
                if self.datalong3 != 0 {
                    if !source.pool_ids.contains(&self.datalong3) {
                        return Err(RelayDependencyFailure::missing(
                            "pool_template",
                            self.datalong3,
                        ));
                    }
                    dependencies.push(ResolvedRelayDependency {
                        kind: "pool_template",
                        raw_value: u64::from(self.datalong3),
                    });
                }
            }
            37 => {
                for relay_id in self.execution_targets(source)? {
                    if !source.definitions.contains_key(&relay_id) {
                        return Err(RelayDependencyFailure::missing(
                            "relay_definition",
                            relay_id,
                        ));
                    }
                    dependencies.push(ResolvedRelayDependency {
                        kind: "relay_definition",
                        raw_value: u64::from(relay_id),
                    });
                }
            }
            42 => {
                for raw_entry in self.dataints[..3]
                    .iter()
                    .copied()
                    .filter(|entry| *entry != 0)
                {
                    let entry = u32::try_from(raw_entry).map_err(|_| RelayDependencyFailure {
                        kind: "item_template",
                        raw_value: raw_entry.unsigned_abs() as u64,
                        reason: "negative_id".to_string(),
                    })?;
                    if !source.item_entries.contains(&entry) {
                        return Err(RelayDependencyFailure::missing("item_template", entry));
                    }
                    dependencies.push(ResolvedRelayDependency {
                        kind: "item_template",
                        raw_value: u64::from(entry),
                    });
                }
            }
            44 => {
                if !source.creature_entries.contains(&self.datalong) {
                    return Err(RelayDependencyFailure::missing(
                        "creature_template",
                        self.datalong,
                    ));
                }
                dependencies.push(ResolvedRelayDependency {
                    kind: "creature_template",
                    raw_value: u64::from(self.datalong),
                });
            }
            45 => {
                self.nested_targets(source)?;
                if self.datalong2 != 0 {
                    dependencies.push(ResolvedRelayDependency {
                        kind: "relay_random_template",
                        raw_value: u64::from(self.datalong2),
                    });
                }
            }
            52 => {
                if !source.gossip_menu_ids.contains(&self.datalong) {
                    return Err(RelayDependencyFailure::missing(
                        "gossip_menu",
                        self.datalong,
                    ));
                }
                dependencies.push(ResolvedRelayDependency {
                    kind: "gossip_menu",
                    raw_value: u64::from(self.datalong),
                });
            }
            53 => {
                let world_state_id =
                    u32::try_from(self.dataints[0]).map_err(|_| RelayDependencyFailure {
                        kind: "world_state",
                        raw_value: self.dataints[0].unsigned_abs() as u64,
                        reason: "negative_id".to_string(),
                    })?;
                if !source.world_state_ids.contains(&world_state_id) {
                    return Err(RelayDependencyFailure::missing(
                        "world_state",
                        world_state_id,
                    ));
                }
                dependencies.push(ResolvedRelayDependency {
                    kind: "world_state",
                    raw_value: u64::from(world_state_id),
                });
            }
            _ => {}
        }
        Ok(dependencies)
    }

    fn talk_ids(&self, source: &RelaySource) -> Result<Vec<u32>, RelayDependencyFailure> {
        if self.datalong == 0 {
            return self.random_ids(0);
        }
        let choices = source.string_templates.get(&self.datalong).ok_or_else(|| {
            RelayDependencyFailure::missing("relay_string_template", self.datalong)
        })?;
        uniform_template_targets(choices).map_err(|reason| RelayDependencyFailure {
            kind: "relay_string_template",
            raw_value: u64::from(self.datalong),
            reason,
        })
    }

    fn random_ids(&self, primary: u32) -> Result<Vec<u32>, RelayDependencyFailure> {
        let mut ids = Vec::new();
        if primary != 0 {
            ids.push(primary);
        }
        for value in self.dataints {
            if value == 0 {
                break;
            }
            let id = u32::try_from(value).map_err(|_| RelayDependencyFailure {
                kind: "relay_definition",
                raw_value: value.unsigned_abs() as u64,
                reason: "negative_random_choice".to_string(),
            })?;
            ids.push(id);
        }
        if ids.is_empty() {
            return Err(RelayDependencyFailure {
                kind: "relay_definition",
                raw_value: u64::from(self.source_order),
                reason: "empty_random_choice".to_string(),
            });
        }
        Ok(ids)
    }

    fn nested_targets(&self, source: &RelaySource) -> Result<Vec<u32>, RelayDependencyFailure> {
        if self.datalong2 == 0 {
            if self.datalong == 0 {
                return Err(RelayDependencyFailure::missing("relay_definition", 0_u64));
            }
            return Ok(vec![self.datalong]);
        }
        let choices = source
            .random_templates
            .get(&self.datalong2)
            .ok_or_else(|| {
                RelayDependencyFailure::missing("relay_random_template", self.datalong2)
            })?;
        uniform_template_targets(choices).map_err(|reason| RelayDependencyFailure {
            kind: "relay_random_template",
            raw_value: u64::from(self.datalong2),
            reason,
        })
    }

    fn encode(&self, source: &RelaySource) -> Result<String, String> {
        Ok(format!(
            "{},{},{},{},{}",
            self.offset_ms,
            self.priority,
            self.source_order,
            self.encode_subject(source)?,
            self.encode_instruction_with_source(source)?
        ))
    }

    fn encode_subject(&self, source_data: &RelaySource) -> Result<String, String> {
        let buddy = if self.buddy_entry == 0 {
            None
        } else if self.flags & SCRIPT_FLAG_ALL_ELIGIBLE_BUDDIES != 0 {
            Some(format!(
                "all-nearby-creatures:{}:{}",
                self.buddy_entry, self.search_radius
            ))
        } else if self.flags & SCRIPT_FLAG_BUDDY_BY_GUID != 0 {
            if self.buddy_is_gameobject() {
                Some(format!(
                    "gameobject-guid:{}",
                    go_guid(u64::from(self.search_radius))
                ))
            } else {
                let entry = source_data
                    .creature_spawns
                    .get(&self.search_radius)
                    .copied()
                    .ok_or_else(|| format!("creature spawn {} is missing", self.search_radius))?;
                Some(format!(
                    "creature-guid:{}",
                    world_guid(u64::from(entry), u64::from(self.search_radius))
                ))
            }
        } else if self.buddy_is_gameobject() {
            Some(format!(
                "nearby-gameobject:{}:{}",
                self.buddy_entry, self.search_radius
            ))
        } else {
            Some(format!(
                "nearby-creature:{}:{}",
                self.buddy_entry, self.search_radius
            ))
        };
        let mut source = "source".to_string();
        let mut target = "selected".to_string();
        if let Some(buddy) = buddy {
            if self.flags & SCRIPT_FLAG_BUDDY_AS_TARGET != 0 {
                target = buddy;
            } else {
                source = buddy;
            }
        }
        if self.flags & SCRIPT_FLAG_REVERSE_DIRECTION != 0 {
            std::mem::swap(&mut source, &mut target);
        }
        if self.flags & SCRIPT_FLAG_SOURCE_TARGETS_SELF != 0 {
            target.clone_from(&source);
        }
        Ok(format!("{source}>{target}"))
    }

    fn encode_instruction_with_source(&self, source: &RelaySource) -> Result<String, String> {
        if self.command == 0 {
            if self.datalong3 != 0 {
                return Err("unsupported talk parameters".to_string());
            }
            let ids = self
                .talk_ids(source)
                .map_err(|failure| failure.reason)?
                .into_iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>();
            return Ok(format!("talk:{}", ids.join(".")));
        }
        if self.command == 1 {
            if self.datalong3 != 0 {
                return Err("unsupported emote parameters".to_string());
            }
            let ids = self
                .random_ids(self.datalong)
                .map_err(|failure| failure.reason)?
                .into_iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>();
            return Ok(format!("emote:{}", ids.join(".")));
        }
        if self.command == 15 {
            if self.datalong3 != 0 {
                return Err("unsupported cast parameters".to_string());
            }
            let ids = self
                .random_ids(self.datalong)
                .map_err(|failure| failure.reason)?
                .into_iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>();
            let cast = match self.datalong2 {
                0 => "direct",
                1 => "triggered",
                value => return Err(format!("unsupported cast flags {value:#x}")),
            };
            if self.flags & SCRIPT_FLAG_COMMAND_ADDITIONAL != 0 {
                return Err("cast without a target has no gameplay authority".to_string());
            }
            return Ok(format!("cast-spell:{}:{cast}", ids.join(".")));
        }
        if self.command == 45 {
            let targets = self
                .nested_targets(source)
                .map_err(|failure| failure.reason)?
                .into_iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>();
            if targets.len() != 1 {
                return Err("nested relay random choice has no runtime owner".to_string());
            }
            return Ok(format!("start-relay:{}", targets.join(".")));
        }
        self.encode_instruction()
    }

    fn encode_instruction(&self) -> Result<String, String> {
        let bool_value = |value: u32| u8::from(value != 0);
        let forced = |value: i32| match value {
            0 => Ok("inherit"),
            1 => Ok("walk"),
            2 => Ok("run"),
            _ => Err(format!("unsupported forced movement {value}")),
        };
        let change = |value: u32| match value {
            0 => Ok("remove"),
            1 => Ok("add"),
            2 => Ok("toggle"),
            _ => Err(format!("invalid flag change {value}")),
        };
        match self.command {
            0 => Err("talk encoding requires relay templates".to_string()),
            1 => Err("emote encoding requires random choices".to_string()),
            3 => {
                if self.datalong != 0
                    || self.datalong2 != 0
                    || self.dataints != [0; 4]
                    || self.speed != 0.0
                    || self.flags & SCRIPT_FLAG_COMMAND_ADDITIONAL != 0
                {
                    return Err("unsupported move-point parameters".to_string());
                }
                let forced = forced(self.datalong3 as i32)?;
                if self.x == 0.0 && self.y == 0.0 && self.z == 0.0 {
                    if forced != "inherit" {
                        return Err("facing-only move cannot force movement".to_string());
                    }
                    Ok(format!("face-orientation:{}", self.orientation))
                } else if self.x == 0.0 && self.y == 0.0 {
                    Err("relative-height move has no gameplay authority".to_string())
                } else {
                    Ok(format!(
                        "move-point:{forced}:{}:{}:{}:{}",
                        self.x, self.y, self.z, self.orientation
                    ))
                }
            }
            10 => {
                if self.datalong3 != 0
                    || !matches!(self.dataints[0], 0 | 1)
                    || self.dataints[1..] != [0; 3]
                    || self.speed != 0.0
                {
                    return Err("unsupported spawn-creature parameters".to_string());
                }
                Ok(format!(
                    "spawn-creature:{}:{}:{}:{}:{}:{}:{}:{}",
                    self.datalong,
                    self.datalong2,
                    bool_value(self.flags & SCRIPT_FLAG_COMMAND_ADDITIONAL),
                    self.dataints[0],
                    self.x,
                    self.y,
                    self.z,
                    self.orientation
                ))
            }
            13 if self.datalong == 0
                && self.datalong2 == 0
                && self.datalong3 == 0
                && self.dataints == [0; 4] =>
            {
                Ok("activate-object:use".to_string())
            }
            13 => Err("unsupported activate-object parameters".to_string()),
            15 => Err("cast-spell encoding requires random choices".to_string()),
            18 if self.datalong == 0
                && self.datalong2 == 0
                && self.datalong3 == 0
                && self.dataints == [0; 4] =>
            {
                Ok("despawn-source:0".to_string())
            }
            18 => Err("unsupported despawn parameters".to_string()),
            20 => {
                if self.datalong3 != 0 || self.dataints[1..] != [0; 3] || self.speed != 0.0 {
                    return Err("unsupported movement parameters".to_string());
                }
                let forced = forced(self.dataints[0])?;
                match self.datalong {
                    0 if self.datalong2 == 0
                        && self.flags & SCRIPT_FLAG_COMMAND_ADDITIONAL == 0 =>
                    {
                        Ok(format!("set-movement:stationary:{forced}"))
                    }
                    1 => Ok(format!(
                        "set-movement:{}:{}:{forced}",
                        if self.flags & SCRIPT_FLAG_COMMAND_ADDITIONAL != 0 {
                            "random-current"
                        } else {
                            "random-home"
                        },
                        self.datalong2
                    )),
                    2 if self.flags & SCRIPT_FLAG_COMMAND_ADDITIONAL == 0 => {
                        Ok(format!("set-movement:patrol:{}:{forced}", self.datalong2))
                    }
                    _ => Err("unsupported movement kind or origin".to_string()),
                }
            }
            21 if matches!(self.datalong, 0 | 1)
                && self.datalong2 == 0
                && self.datalong3 == 0
                && self.dataints == [0; 4] =>
            {
                Ok(format!("set-active:{}", bool_value(self.datalong)))
            }
            21 => Err("unsupported active-state parameters".to_string()),
            22 => {
                let lifetime = match self.datalong2 {
                    0 => "permanent",
                    36 => "combat-stop-or-respawn",
                    value => return Err(format!("unsupported faction flags {value:#x}")),
                };
                Ok(format!("set-faction:{}:{lifetime}", self.datalong))
            }
            25 if matches!(self.datalong, 0 | 1) => {
                Ok(format!("set-run:{}", bool_value(self.datalong)))
            }
            25 => Err(format!("invalid run state {}", self.datalong)),
            26 if self.datalong == 0
                && self.datalong2 == 0
                && self.datalong3 == 0
                && self.dataints == [0; 4] =>
            {
                Ok("attack-start".to_string())
            }
            26 => Err("unsupported attack-start parameters".to_string()),
            28 if u8::try_from(self.datalong).is_ok()
                && self.datalong2 == 0
                && self.datalong3 == 0
                && self.dataints == [0; 4] =>
            {
                Ok(format!("set-stand-state:{}", self.datalong))
            }
            28 => Err("unsupported stand-state parameters".to_string()),
            29 => Ok(format!(
                "modify-npc-flags:{}:{}",
                self.datalong,
                change(self.datalong2)?
            )),
            31 => {
                if self.datalong == 0 || self.datalong3 != 0 || self.dataints != [0; 4] {
                    return Err("unsupported conditional termination parameters".to_string());
                }
                let subject = if self.flags & SCRIPT_FLAG_BUDDY_BY_GO != 0 {
                    format!("nearby-gameobject:{}:{}", self.datalong, self.datalong2)
                } else {
                    format!("nearby-creature:{}:{}", self.datalong, self.datalong2)
                };
                Ok(format!(
                    "terminate-if-{}:{subject}",
                    if self.flags & SCRIPT_FLAG_COMMAND_ADDITIONAL != 0 {
                        "present"
                    } else {
                        "missing"
                    }
                ))
            }
            32 if matches!(self.datalong, 0 | 1)
                && self.datalong2 == 0
                && self.datalong3 == 0
                && self.dataints == [0; 4] =>
            {
                Ok(format!("pause-waypoints:{}", bool_value(self.datalong)))
            }
            32 => Err("unsupported waypoint pause parameters".to_string()),
            35 if self.datalong3 == 0 && self.dataints == [0; 4] => Ok(format!(
                "send-ai-event:{}:{}",
                ai_event_name(self.datalong)
                    .ok_or_else(|| format!("unknown AI event {}", self.datalong))?,
                self.datalong2
            )),
            35 => Err("unsupported AI event payload".to_string()),
            36 if self.datalong2 == 0
                && self.datalong3 == 0
                && self.dataints == [0; 4]
                && self.flags & SCRIPT_FLAG_COMMAND_ADDITIONAL == 0 =>
            {
                Ok(format!(
                    "set-facing:{}",
                    if self.datalong == 0 {
                        "target"
                    } else {
                        "reset"
                    }
                ))
            }
            36 => Err("unsupported facing parameters".to_string()),
            37 => {
                if self.dataints[2..] != [0; 2]
                    || self.speed != 0.0
                    || self.flags & SCRIPT_FLAG_COMMAND_ADDITIONAL != 0
                {
                    return Err("unsupported dynamic movement parameters".to_string());
                }
                let arrival_relay = u32::try_from(self.dataints[1])
                    .map_err(|_| "negative arrival relay".to_string())?;
                Ok(format!(
                    "move-dynamic:{}:{}:{}:{}:{}",
                    self.datalong2,
                    self.datalong,
                    self.datalong3,
                    forced(self.dataints[0])?,
                    arrival_relay
                ))
            }
            40 if self.datalong == 0
                && self.datalong2 == 0
                && self.datalong3 == 0
                && self.dataints == [0; 4] =>
            {
                Ok("despawn-gameobject:0".to_string())
            }
            40 => Err("unsupported gameobject despawn parameters".to_string()),
            42 if matches!(self.datalong, 0 | 1)
                && self.datalong2 == 0
                && self.datalong3 == 0
                && self.dataints[3] == 0 =>
            {
                Ok(format!(
                    "set-equipment:{}:{}:{}:{}",
                    bool_value(self.datalong),
                    self.dataints[0],
                    self.dataints[1],
                    self.dataints[2]
                ))
            }
            42 => Err("unsupported equipment parameters".to_string()),
            44 if self.datalong != 0
                && self.datalong2 == 0
                && self.datalong3 == 0
                && self.dataints == [0; 4] =>
            {
                Ok(format!("update-creature-template:{}", self.datalong))
            }
            44 => Err("unsupported creature-template update parameters".to_string()),
            45 => Err("nested relay encoding requires relay templates".to_string()),
            48 if matches!(self.datalong, 0x100 | 0x200) => Ok(format!(
                "modify-unit-flags:{}:{}",
                self.datalong,
                change(self.datalong2)?
            )),
            48 => Err(format!("unsupported unit flag {:#x}", self.datalong)),
            52 if self.datalong != 0
                && self.datalong2 == 0
                && self.datalong3 == 0
                && self.dataints == [0; 4] =>
            {
                Ok(format!("set-gossip-menu:{}", self.datalong))
            }
            52 => Err("unsupported gossip-menu parameters".to_string()),
            53 if self.datalong == 0
                && self.datalong2 == 0
                && self.datalong3 == 0
                && self.dataints[2..] == [0; 2] =>
            {
                Ok(format!(
                    "set-world-state:{}:{}",
                    self.dataints[0], self.dataints[1]
                ))
            }
            53 => Err("unsupported world-state parameters".to_string()),
            command => Err(format!("unsupported command {command}")),
        }
    }

    // Relay Gates and encoding are kept together so accepted source rows cannot diverge from
    // the wire form sent to the Module.
}

fn parse_source_maps(dump: &str) -> HashMap<u32, BTreeSet<u32>> {
    let mut maps = HashMap::new();
    for row in parse_table(dump, "creature") {
        let (Some(entry), Some(map_id)) = (source_u32(field(&row, 1)), source_u32(field(&row, 2)))
        else {
            continue;
        };
        maps.entry(entry)
            .or_insert_with(BTreeSet::new)
            .insert(map_id);
    }
    maps
}

#[derive(Clone, Copy)]
pub(super) struct SourceNormalization {
    pub(super) dimension: &'static str,
    pub(super) raw_value: u64,
    pub(super) reason: &'static str,
}

#[derive(Clone, Copy)]
enum SourceEventPredicate {
    Alliance,
    Horde,
    QuestTaken(u32),
}

impl SourceEventPredicate {
    fn encode(self) -> String {
        match self {
            Self::Alliance => "alliance".to_string(),
            Self::Horde => "horde".to_string(),
            Self::QuestTaken(quest_entry) => format!("quest-taken.{quest_entry}"),
        }
    }
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
    threat_percent: Option<i32>,
    texts: Vec<u32>,
    summon_entry: Option<u64>,
    summon_location: Option<u32>,
    dependencies: Vec<Dependency>,
    normalizations: Vec<SourceNormalization>,
}

pub(super) struct Dependency {
    pub(super) kind: &'static str,
    pub(super) path: String,
}

#[derive(Debug)]
pub(super) struct MappingFailure {
    dimension: &'static str,
    raw_value: u64,
    reason: String,
    dependency_path: Option<String>,
    dependency_kind: Option<&'static str>,
}

impl MappingFailure {
    pub(super) fn source(
        dimension: &'static str,
        raw_value: u64,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            dimension,
            raw_value,
            reason: reason.into(),
            dependency_path: None,
            dependency_kind: None,
        }
    }

    pub(super) fn dependency(
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
    event_predicates: BTreeMap<u32, SourceEventPredicate>,
    quest_entries: HashSet<u32>,
    rules: Vec<RawRule>,
    source_maps: HashMap<u32, BTreeSet<u32>>,
    relays: RelaySource,
    coverage: Coverage,
}

pub(crate) fn parse(dump: &str) -> EventAiSource {
    let mut coverage = Coverage::default();
    let mut broadcasts = parse_broadcasts(dump, &mut coverage);
    broadcasts.extend(parse_legacy_texts(dump, &broadcasts, &mut coverage));
    let summon_locations = parse_summons(dump, &mut coverage);
    let event_predicates = parse_event_predicates(dump);
    let quest_rows = parse_table(dump, "quest_template");
    let quest_entries = quest_rows
        .iter()
        .filter_map(|row| source_u32(field(row, 0)))
        .collect();
    let mut rules = parse_rules(dump, &mut coverage);
    rules.sort_by_key(|rule| rule.id);
    coverage.movement_dependencies = movement_dependency_census(&rules);
    coverage.reachable_relays = reachable_relay_census(dump, &rules);
    let relays = RelaySource::parse(dump);
    coverage.ticket_census = relays.ticket_census(&rules);
    coverage.relay_parse_failures = relays.parse_failures.clone();
    let (cast_action_subjects, template_schedule_overlaps, creature_spell_list_overlaps) =
        source_overlap_census(dump, &rules);
    coverage.cast_action_subjects = cast_action_subjects;
    coverage.template_schedule_overlaps = template_schedule_overlaps;
    coverage.creature_spell_list_overlaps = creature_spell_list_overlaps;
    EventAiSource {
        broadcasts,
        summon_locations,
        event_predicates,
        quest_entries,
        rules,
        source_maps: parse_source_maps(dump),
        relays,
        coverage,
    }
}

fn movement_dependency_census(rules: &[RawRule]) -> MovementDependencyCensus {
    let main_spell_subjects = rules
        .iter()
        .filter(|rule| {
            rule.actions
                .iter()
                .any(|action| action[0] == ACTION_CAST && action[3] & CAST_MAIN_SPELL != 0)
        })
        .map(|rule| rule.subject)
        .collect::<BTreeSet<_>>();
    let mut census = MovementDependencyCensus::default();
    for rule in rules {
        for _ in rule
            .actions
            .iter()
            .filter(|action| action[0] == ACTION_SET_RANGED_MODE)
        {
            census.ranged_mode_actions += 1;
            census.ranged_mode_subjects.insert(rule.subject);
        }
    }
    census.resolved_main_spell_subjects = census
        .ranged_mode_subjects
        .intersection(&main_spell_subjects)
        .copied()
        .collect();
    census.missing_main_spell_subjects = census
        .ranged_mode_subjects
        .difference(&main_spell_subjects)
        .copied()
        .collect();
    census
}

fn reachable_relay_census(dump: &str, rules: &[RawRule]) -> ReachableRelayCensus {
    const RANDOM_TEMPLATE_SCRIPT: u32 = 1;
    const COMMAND_MOVE_TO: u32 = 3;
    const COMMAND_MOVEMENT: u32 = 20;
    const COMMAND_SET_RUN: u32 = 25;
    const COMMAND_ATTACK_START: u32 = 26;
    const COMMAND_PAUSE_WAYPOINTS: u32 = 32;
    const COMMAND_SET_FACING: u32 = 36;
    const COMMAND_MOVE_DYNAMIC: u32 = 37;
    const COMMAND_START_RELAY: u32 = 45;

    let random_templates = parse_table(dump, "dbscript_random_templates")
        .into_iter()
        .filter_map(|row| {
            let template_id = source_u32(field(&row, 0))?;
            let kind = source_u32(field(&row, 1))?;
            let relay_id = field(&row, 2).parse::<i64>().ok()?;
            (kind == RANDOM_TEMPLATE_SCRIPT && relay_id > 0)
                .then_some((template_id, relay_id as u32))
        })
        .fold(
            BTreeMap::<u32, BTreeSet<u32>>::new(),
            |mut by_template, value| {
                by_template.entry(value.0).or_default().insert(value.1);
                by_template
            },
        );
    let relay_rows = parse_table(dump, "dbscripts_on_relay")
        .into_iter()
        .filter_map(|row| Some((source_u32(field(&row, 0))?, row)))
        .fold(
            BTreeMap::<u32, Vec<Vec<String>>>::new(),
            |mut by_id, value| {
                by_id.entry(value.0).or_default().push(value.1);
                by_id
            },
        );

    let mut census = ReachableRelayCensus::default();
    let mut pending = BTreeSet::new();
    let include_template =
        |template_id: u32, pending: &mut BTreeSet<u32>, census: &mut ReachableRelayCensus| {
            census.random_template_ids.insert(template_id);
            if let Some(relay_ids) = random_templates.get(&template_id) {
                pending.extend(relay_ids.iter().copied());
            }
        };
    for action in rules
        .iter()
        .flat_map(|rule| rule.actions)
        .filter(|action| action[0] == ACTION_START_RELAY)
    {
        census.root_references += 1;
        let relay = action[1] as i32;
        if relay < 0 {
            include_template(relay.unsigned_abs(), &mut pending, &mut census);
        } else if relay > 0 {
            census.direct_root_ids.insert(relay as u32);
            pending.insert(relay as u32);
        }
    }

    while let Some(relay_id) = pending.pop_first() {
        if !census.relay_ids.insert(relay_id) {
            continue;
        }
        let Some(rows) = relay_rows.get(&relay_id) else {
            continue;
        };
        for row in rows {
            census.rows += 1;
            let Some(command) = source_u32(field(row, 3)) else {
                continue;
            };
            *census.command.entry(u64::from(command)).or_default() += 1;
            if matches!(
                command,
                COMMAND_MOVE_TO
                    | COMMAND_MOVEMENT
                    | COMMAND_SET_RUN
                    | COMMAND_ATTACK_START
                    | COMMAND_PAUSE_WAYPOINTS
                    | COMMAND_SET_FACING
                    | COMMAND_MOVE_DYNAMIC
            ) {
                let raw = [
                    3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
                ]
                .into_iter()
                .map(|index| field(row, index))
                .collect::<Vec<_>>()
                .join(":");
                *census.movement_values.entry(raw).or_default() += 1;
            }
            if command == COMMAND_START_RELAY {
                census.nested_edges += 1;
                let direct = source_u32(field(row, 4)).unwrap_or(0);
                let template = source_u32(field(row, 5)).unwrap_or(0);
                if direct != 0 {
                    pending.insert(direct);
                } else if template != 0 {
                    include_template(template, &mut pending, &mut census);
                }
            }
        }
    }
    census
}

fn source_overlap_census(dump: &str, rules: &[RawRule]) -> (u64, u64, u64) {
    let template_rows = crate::parse_table(dump, "creature_template");
    let schedule_rows = crate::parse_table(dump, "creature_template_spells");
    let template_schedule_entries: HashSet<u64> = schedule_rows
        .into_iter()
        .filter_map(|row| row.first()?.parse().ok())
        .collect();
    let creature_spell_list_entries: HashSet<u64> = template_rows
        .into_iter()
        .filter(|row| {
            row.get(80)
                .and_then(|value| value.parse::<i64>().ok())
                .is_some_and(|spell_list| spell_list > 0)
        })
        .filter_map(|row| row.first()?.parse().ok())
        .collect();
    let cast_action_entries: HashSet<u64> = rules
        .iter()
        .filter(|rule| rule.actions.iter().any(|action| action[0] == ACTION_CAST))
        .filter_map(|rule| (rule.subject > 0).then_some(rule.subject as u64))
        .collect();
    (
        cast_action_entries.len() as u64,
        cast_action_entries
            .intersection(&template_schedule_entries)
            .count() as u64,
        cast_action_entries
            .intersection(&creature_spell_list_entries)
            .count() as u64,
    )
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
        let mut accepted_relay_roots = BTreeSet::new();

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
            if rule.actions.iter().any(|action| action[0] == ACTION_SOUND) {
                plan.coverage
                    .classify_rule(rule.id, "excluded", "unsupported_sound_playback");
                record_rule_dimensions(
                    &mut plan.coverage,
                    rule,
                    "excluded",
                    "unsupported_sound_playback",
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
            let mut failures = Vec::new();
            let event = match map_event(rule, &self.event_predicates) {
                Ok(event) => Some(event),
                Err(failure) => {
                    failures.push(failure);
                    None
                }
            };
            if let Some((condition_id, quest_entry)) =
                event_quest_dependency(rule, &self.event_predicates)
            {
                if !self.quest_entries.contains(&quest_entry) {
                    failures.push(MappingFailure::dependency(
                        "quest_template",
                        u64::from(quest_entry),
                        "missing",
                        format!(
                            "rule:{} -> conditions:{condition_id} -> quest_template:{quest_entry}",
                            rule.id
                        ),
                    ));
                }
            }
            if rule.chance == 0 {
                failures.push(MappingFailure::source(
                    "rule",
                    rule.chance as u64,
                    "invalid_chance",
                ));
            }
            let flags = effective_event_flags(rule);
            let flag_residual = flags & !SUPPORTED_FLAGS;
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
            if flags & FLAG_RANGED_ONLY != 0 && flags & FLAG_MELEE_ONLY != 0 {
                failures.push(MappingFailure::source(
                    "event_flag",
                    rule.flags as u64,
                    "conflicting_posture_flags",
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
                    rule.subject,
                    rule.id,
                    rule.subject,
                    slot,
                    broadcasts,
                    summon_locations,
                    &self.quest_entries,
                    importable_templates,
                    &self.source_maps,
                    &self.relays,
                ) {
                    Ok(action) => actions.push(action),
                    Err(mut action_failures) => failures.append(&mut action_failures),
                }
            }
            for action in &actions {
                if let Some(target) = action.raw_target {
                    if !event_provides_target_context(rule.event, target) {
                        failures.push(MappingFailure::source(
                            "target",
                            u64::from(target),
                            format!("target_context_unavailable_for_event_{}", rule.event),
                        ));
                    }
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
            let recurrence = if flags & FLAG_REPEATABLE != 0 {
                match repeat_window(rule) {
                    Some((min, max)) if min <= max => format!("repeat:{min}:{max}"),
                    None if repeats_on_each_event(rule.event) => "repeat-event".to_string(),
                    _ => {
                        failures.push(MappingFailure::source(
                            "event",
                            rule.event as u64,
                            "invalid_repeat_window",
                        ));
                        "once".to_string()
                    }
                }
            } else {
                "once".to_string()
            };
            if !failures.is_empty() {
                record_dropped_rule(&mut plan.coverage, rule, failures);
                continue;
            }
            if rule.flags & FLAG_DEBUG_ONLY != 0 {
                plan.coverage
                    .classify_rule(rule.id, "excluded", "debug_only_rule");
                record_rule_dimensions(&mut plan.coverage, rule, "excluded", "debug_only_rule");
                continue;
            }
            let selection = if flags & FLAG_RANDOM_ACTION != 0 {
                "random"
            } else {
                "all"
            };
            let execution = if flags & FLAG_COMBAT_ACTION != 0 {
                "combat"
            } else {
                "ordinary"
            };
            let posture = if flags & FLAG_RANGED_ONLY != 0 {
                "ranged-only"
            } else if flags & FLAG_MELEE_ONLY != 0 {
                "melee-only"
            } else {
                "any-posture"
            };
            let instructions = actions
                .iter()
                .map(|action| action.encoded.as_str())
                .collect::<Vec<_>>()
                .join("+");
            let encoded_rule = format!(
                "{},{},{},{},{},{},{},{},{}",
                rule.id,
                event,
                rule.chance,
                allowed_phase_mask,
                recurrence,
                selection,
                execution,
                posture,
                instructions,
            );
            definitions.entry(subject.0).or_default().push(encoded_rule);
            for action in &rule.actions {
                if action[0] == ACTION_START_RELAY {
                    if let Ok(roots) = self.relays.root_choices(action[1]) {
                        accepted_relay_roots.extend(roots);
                    }
                }
            }

            let has_action_normalization = actions
                .iter()
                .any(|action| !action.normalizations.is_empty());
            let normalized = !rule.normalizations.is_empty() || has_action_normalization;
            if normalized {
                plan.coverage
                    .classify_rule(rule.id, "normalized", "normalized");
            } else {
                plan.coverage.classify_rule(rule.id, "emitted", "emitted");
            }
            let classification = "emitted";
            let reason = "emitted";
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
            for normalization in actions
                .iter()
                .flat_map(|action| action.normalizations.iter())
            {
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
            if let Some(condition_id) = event_condition_id(rule) {
                plan.coverage.result(
                    "dependency",
                    "event_condition",
                    classification,
                    "resolved",
                    rule.id,
                    Some(format!("rule:{} -> conditions:{condition_id}", rule.id)),
                );
                if let Some((_, quest_entry)) = event_quest_dependency(rule, &self.event_predicates)
                {
                    plan.coverage.result(
                        "dependency",
                        "quest_template",
                        classification,
                        "resolved",
                        rule.id,
                        Some(format!(
                            "rule:{} -> conditions:{condition_id} -> quest_template:{quest_entry}",
                            rule.id
                        )),
                    );
                }
            }
            for action in actions {
                if action.raw_kind == ACTION_QUEST_EVENT {
                    let quest_entry = action
                        .encoded
                        .strip_prefix("quest-event:")
                        .and_then(|encoded| encoded.split_once(':').map(|(entry, _)| entry))
                        .and_then(|entry| entry.parse().ok())
                        .expect("a mapped quest event has a numeric quest entry");
                    plan.quest_event_entries.insert(quest_entry);
                }
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
                if action.raw_kind == ACTION_SET_DEATH_PREVENTION {
                    let state = match action.encoded.as_str() {
                        "lethal-floor:off" => 0,
                        "lethal-floor:on" => 1,
                        _ => unreachable!("the native death-prevention encoding is exhaustive"),
                    };
                    plan.coverage.result(
                        "death_prevention_state",
                        state,
                        classification,
                        reason,
                        rule.id,
                        None,
                    );
                }
                if let Some(target) = action.raw_target {
                    Coverage::source_value(&mut plan.coverage.target, target as u64);
                    plan.coverage
                        .result("target", target, classification, reason, rule.id, None);
                }
                if let Some(flags) = action.raw_cast_flags {
                    plan.coverage
                        .result("cast_flag", flags, classification, reason, rule.id, None);
                }
                if let Some(percent) = action.threat_percent {
                    plan.coverage.result(
                        "threat_percent",
                        percent,
                        classification,
                        reason,
                        rule.id,
                        None,
                    );
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
                if action.texts.iter().any(|id| {
                    self.broadcasts
                        .get(id)
                        .is_some_and(|text| text.chat_type == 2)
                }) {
                    *plan
                        .coverage
                        .ticket_census
                        .entry("text_emote_dependency_occurrences".to_string())
                        .or_default() += 1;
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

        plan.relay_definition_rows = self.relays.encode_closure(&accepted_relay_roots);
        let relay_catalogue_version =
            RelaySource::encoded_catalogue_version(&plan.relay_definition_rows);
        for rules in definitions.values_mut() {
            for rule in rules {
                *rule = rule.replace(
                    RELAY_CATALOGUE_VERSION_PLACEHOLDER,
                    &relay_catalogue_version.to_string(),
                );
            }
        }
        plan.definition_rows = definitions
            .into_iter()
            .map(|(subject, rules)| encode_definition(subject, &rules))
            .collect();
        plan.definition_batches = pack_definition_batches(&plan.definition_rows);
        used_texts.extend(self.relays.closure_broadcast_ids(&accepted_relay_roots));
        plan.relay_definition_batches = vec![plan.relay_definition_rows.join("\n")];

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

fn event_provides_target_context(event: u32, target: u32) -> bool {
    match target {
        TARGET_ACTION_INVOKER | TARGET_BENEFICIARY => matches!(
            event,
            EVENT_AGGRO
                | EVENT_KILL
                | EVENT_DEATH
                | EVENT_SPELL_HIT
                | EVENT_OOC_LOS
                | EVENT_RECEIVE_EMOTE
                | EVENT_RECEIVE_AI_EVENT
                | EVENT_SUMMONED
                | EVENT_SUMMONED_DEATH
                | EVENT_SPELL_HIT_TARGET
        ),
        TARGET_AI_SENDER => event == EVENT_RECEIVE_AI_EVENT,
        TARGET_EVENT_SPECIFIC => matches!(
            event,
            EVENT_FRIENDLY_HP
                | EVENT_FRIENDLY_CC
                | EVENT_FRIENDLY_MISSING_AURA
                | EVENT_SELECT_ATTACKING
        ),
        _ => true,
    }
}

fn event_condition_id(rule: &RawRule) -> Option<u32> {
    let id = match rule.event {
        EVENT_DEATH => rule.params[0],
        EVENT_OOC_LOS => rule.params[5],
        EVENT_RECEIVE_EMOTE => rule.params[1],
        _ => 0,
    };
    (id != 0).then_some(id)
}

fn event_quest_dependency(
    rule: &RawRule,
    predicates: &BTreeMap<u32, SourceEventPredicate>,
) -> Option<(u32, u32)> {
    let condition_id = event_condition_id(rule)?;
    let SourceEventPredicate::QuestTaken(quest_entry) = predicates.get(&condition_id)? else {
        return None;
    };
    Some((condition_id, *quest_entry))
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
        if action[0] == ACTION_SET_DEATH_PREVENTION {
            coverage.result(
                "death_prevention_state",
                action[1],
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
            if flags & FLAG_CLASSIC_RESERVED != 0 {
                normalizations.push(SourceNormalization {
                    dimension: "event_flag",
                    raw_value: u64::from(flags),
                    reason: "normalized_reserved_event_flag",
                });
            }
            if event == EVENT_RECEIVE_EMOTE && flags & FLAG_REPEATABLE == 0 {
                normalizations.push(SourceNormalization {
                    dimension: "event_flag",
                    raw_value: u64::from(flags),
                    reason: "receive_emote_repeatable",
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
                record_presentation_source_census(coverage, *action);
                if movement_action_kind(kind) {
                    *coverage
                        .movement_values
                        .entry(format!(
                            "{}:{}:{}:{}",
                            kind, action[1], action[2], action[3]
                        ))
                        .or_default() += 1;
                }
                if let Some(parameter) = source_target_parameter(kind) {
                    Coverage::source_value(&mut coverage.source_target, action[parameter] as u64);
                }
                if kind == ACTION_CAST {
                    Coverage::source_value(&mut coverage.cast_flags, action[3] as u64);
                }
                if kind == ACTION_SET_DEATH_PREVENTION {
                    Coverage::source_value(&mut coverage.death_prevention_states, action[1] as u64);
                }
                if kind == ACTION_THREAT_SINGLE {
                    if action[3] == 0 {
                        Coverage::source_signed_value(
                            &mut coverage.threat_percent,
                            action[1] as i32,
                        );
                    } else {
                        coverage.direct_threat_actions += 1;
                    }
                }
                if kind == ACTION_THREAT_ALL_PCT {
                    Coverage::source_signed_value(&mut coverage.threat_percent, action[1] as i32);
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

fn record_presentation_source_census(coverage: &mut Coverage, action: [u32; 4]) {
    match action[0] {
        eventai_presentation::ACTION_SET_FACTION => {
            Coverage::source_value(&mut coverage.presentation_factions, u64::from(action[1]));
        }
        eventai_presentation::ACTION_MORPH_TO_ENTRY_OR_MODEL
        | eventai_presentation::ACTION_UPDATE_TEMPLATE => {
            Coverage::source_value(
                &mut coverage.presentation_display_templates,
                u64::from(action[1]),
            );
        }
        eventai_presentation::ACTION_SET_UNIT_FIELD => {
            Coverage::source_value(&mut coverage.presentation_unit_fields, u64::from(action[1]));
        }
        eventai_presentation::ACTION_SET_UNIT_FLAG => {
            Coverage::source_value(
                &mut coverage.presentation_set_unit_flags,
                u64::from(action[1]),
            );
        }
        eventai_presentation::ACTION_REMOVE_UNIT_FLAG => {
            Coverage::source_value(
                &mut coverage.presentation_remove_unit_flags,
                u64::from(action[1]),
            );
        }
        eventai_presentation::ACTION_MOUNT_TO_ENTRY_OR_MODEL => {
            Coverage::source_value(
                &mut coverage.presentation_mount_models,
                u64::from(action[2]),
            );
        }
        _ => {}
    }
}

fn effective_event_flags(rule: &RawRule) -> u32 {
    let mut flags = rule.flags & !FLAG_CLASSIC_RESERVED & !FLAG_DEBUG_ONLY;
    if rule.event == EVENT_RECEIVE_EMOTE {
        flags |= FLAG_REPEATABLE;
    }
    flags
}

fn source_target_parameter(action: u32) -> Option<usize> {
    if let Some(parameter) = eventai_presentation::source_target_parameter(action) {
        return Some(parameter);
    }
    match action {
        ACTION_REMOVE_AURA | ACTION_ATTACK_START | ACTION_SET_FACING => Some(1),
        ACTION_CAST
        | ACTION_SPAWN
        | ACTION_THREAT_SINGLE
        | ACTION_QUEST_EVENT
        | ACTION_SUMMON_ID
        | ACTION_KILLED_MONSTER
        | ACTION_SET_INSTANCE_DATA_GUID
        | ACTION_START_RELAY
        | ACTION_TEXT_NEW => Some(2),
        eventai_presentation::ACTION_SET_UNIT_FIELD | ACTION_THROW_AI_EVENT => Some(3),
        _ => None,
    }
}

fn movement_action_kind(action: u32) -> bool {
    matches!(
        action,
        ACTION_COMBAT_MOVEMENT
            | ACTION_EVADE
            | ACTION_RANGED_MOVEMENT
            | ACTION_CHANGE_MOVEMENT
            | ACTION_PAUSE_WAYPOINTS
            | ACTION_SET_RANGED_MODE
            | ACTION_SET_WALK
            | ACTION_SET_FACING
            | ACTION_SET_IMMOBILIZED
            | ACTION_SET_FOLLOW_MOVEMENT
            | ACTION_RETREAT
    )
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

fn parse_event_predicates(dump: &str) -> BTreeMap<u32, SourceEventPredicate> {
    parse_table(dump, "conditions")
        .into_iter()
        .filter_map(|row| {
            let id = source_u32(field(&row, 0))?;
            let kind = field(&row, 1).parse::<i32>().ok()?;
            let value = source_u32(field(&row, 2))?;
            let second = source_u32(field(&row, 3))?;
            let predicate = match (kind, value, second) {
                (6, 469, 0) => SourceEventPredicate::Alliance,
                (6, 67, 0) => SourceEventPredicate::Horde,
                (9, quest_entry, 0) => SourceEventPredicate::QuestTaken(quest_entry),
                _ => return None,
            };
            Some((id, predicate))
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

fn map_event(
    rule: &RawRule,
    predicates: &BTreeMap<u32, SourceEventPredicate>,
) -> Result<String, MappingFailure> {
    let params = rule.params;
    let percentage = |event: &str| {
        (params[0] <= 100 && params[1] <= params[0])
            .then(|| format!("{event}:{}:{}", params[1], params[0]))
    };
    match rule.event {
        EVENT_TIMER_IN_COMBAT if params[0] <= params[1] => {
            Some(format!("timer-combat:{}:{}", params[0], params[1]))
        }
        EVENT_TIMER_IN_COMBAT => None,
        EVENT_TIMER_OUT_OF_COMBAT if params[0] <= params[1] => {
            Some(format!("timer-ooc:{}:{}", params[0], params[1]))
        }
        EVENT_TIMER_OUT_OF_COMBAT => None,
        EVENT_HP if params[4] <= 1 => {
            percentage("health").map(|event| format!("{event}:{}", params[4]))
        }
        EVENT_HP => None,
        EVENT_MANA => percentage("power"),
        EVENT_AGGRO => Some("aggro".to_string()),
        EVENT_KILL if params[2] <= 1 => Some(format!("kill:{}", params[2])),
        EVENT_KILL => None,
        EVENT_DEATH => Some(format!(
            "death:{}",
            event_predicate(rule, params[0], predicates)?
        )),
        EVENT_EVADE => Some("evade".to_string()),
        EVENT_SPELL_HIT => Some(format!("spell-hit:{}:{}", params[0], params[1])),
        EVENT_RANGE if params[0] <= params[1] => Some(format!("range:{}:{}", params[0], params[1])),
        EVENT_RANGE => None,
        EVENT_OOC_LOS if params[0] <= 1 && params[4] <= 1 => Some(format!(
            "ooc-los:{}:{}:{}:{}",
            params[0],
            params[1],
            params[4],
            event_predicate(rule, params[5], predicates)?
        )),
        EVENT_OOC_LOS => None,
        EVENT_SPAWNED => match params[0] {
            0 => Some("spawn:always".to_string()),
            1 => Some(format!("spawn:map:{}", params[1])),
            2 => Some(format!("spawn:zone-or-area:{}", params[1])),
            _ => None,
        },
        EVENT_TARGET_HP => percentage("target-health"),
        EVENT_TARGET_CASTING => Some("target-casting".to_string()),
        EVENT_FRIENDLY_HP if params[1] > 0 && params[4] <= 1 => Some(format!(
            "friendly-health:{}:{}:{}",
            params[0], params[1], params[4]
        )),
        EVENT_FRIENDLY_HP => None,
        EVENT_FRIENDLY_CC if params[1] > 0 => Some(format!("friendly-cc:{}", params[1])),
        EVENT_FRIENDLY_CC => None,
        EVENT_FRIENDLY_MISSING_AURA if params[1] > 0 && params[4] <= 2 => {
            let selection = match params[4] {
                0 => "nearby-engaged",
                1 => "match-actor-combat",
                2 => "any-while-disengaged",
                _ => unreachable!("the event guard admits only source modes 0..=2"),
            };
            Some(format!(
                "friendly-missing-aura:{}:{}:{selection}",
                params[0], params[1]
            ))
        }
        EVENT_FRIENDLY_MISSING_AURA => None,
        EVENT_SUMMONED => Some(format!("summoned:{}", params[0])),
        EVENT_TARGET_MANA => percentage("target-power"),
        EVENT_HOME => Some("home".to_string()),
        EVENT_RECEIVE_EMOTE => Some(format!(
            "receive-emote:{}:{}",
            params[0],
            event_predicate(rule, params[1], predicates)?
        )),
        EVENT_AURA => Some(format!("aura:{}:{}", params[0], params[1])),
        EVENT_TARGET_AURA => Some(format!("target-aura:{}:{}", params[0], params[1])),
        EVENT_SUMMONED_DEATH => Some(format!("summoned-death:{}", params[0])),
        EVENT_MISSING_AURA => Some(format!("missing-aura:{}:{}", params[0], params[1])),
        EVENT_TARGET_MISSING_AURA => {
            Some(format!("target-missing-aura:{}:{}", params[0], params[1]))
        }
        EVENT_TIMER_GENERIC if params[0] <= params[1] => {
            Some(format!("timer-generic:{}:{}", params[0], params[1]))
        }
        EVENT_TIMER_GENERIC => None,
        EVENT_RECEIVE_AI_EVENT => {
            ai_event_name(params[0]).map(|event| format!("ai-event:{event}:{}", params[1]))
        }
        EVENT_SELECT_ATTACKING if params[0] <= params[1] => {
            Some(format!("select-attacking:{}:{}", params[0], params[1]))
        }
        EVENT_SELECT_ATTACKING => None,
        EVENT_FACING if params[0] <= 1 => Some(format!("facing:{}", u8::from(params[0] == 0))),
        EVENT_FACING => None,
        EVENT_SPELL_HIT_TARGET => Some(format!("spell-hit-target:{}:{}", params[0], params[1])),
        EVENT_TARGET_NOT_REACHABLE => Some("target-not-reachable".to_string()),
        _ => {
            return Err(MappingFailure::source(
                "event",
                rule.event as u64,
                "unsupported_event",
            ));
        }
    }
    .ok_or_else(|| MappingFailure::source("event", rule.event as u64, "invalid_event_parameters"))
}

fn event_predicate(
    rule: &RawRule,
    condition_id: u32,
    predicates: &BTreeMap<u32, SourceEventPredicate>,
) -> Result<String, MappingFailure> {
    if condition_id == 0 {
        return Ok("always".to_string());
    }
    predicates
        .get(&condition_id)
        .copied()
        .map(SourceEventPredicate::encode)
        .ok_or_else(|| {
            MappingFailure::dependency(
                "event_condition",
                u64::from(condition_id),
                "unsupported_or_missing",
                format!("rule:{} -> conditions:{condition_id}", rule.id),
            )
        })
}

fn ai_event_name(value: u32) -> Option<&'static str> {
    [
        "just-died",
        "critical-health",
        "lost-health",
        "lost-some-health",
        "got-full-health",
        "custom-a",
        "custom-b",
        "crowd-controlled",
        "custom-c",
        "custom-d",
        "custom-e",
        "custom-f",
    ]
    .get(value as usize)
    .copied()
}

fn repeat_window(rule: &RawRule) -> Option<(u32, u32)> {
    let p = rule.params;
    match rule.event {
        EVENT_TIMER_IN_COMBAT
        | EVENT_TIMER_OUT_OF_COMBAT
        | EVENT_HP
        | EVENT_MANA
        | EVENT_SPELL_HIT
        | EVENT_RANGE
        | EVENT_OOC_LOS
        | EVENT_TARGET_HP
        | EVENT_FRIENDLY_HP
        | EVENT_FRIENDLY_CC
        | EVENT_FRIENDLY_MISSING_AURA
        | EVENT_TARGET_MANA
        | EVENT_AURA
        | EVENT_TARGET_AURA
        | EVENT_MISSING_AURA
        | EVENT_TARGET_MISSING_AURA
        | EVENT_TIMER_GENERIC
        | EVENT_SELECT_ATTACKING
        | EVENT_FACING
        | EVENT_SPELL_HIT_TARGET => Some((p[2], p[3])),
        EVENT_KILL | EVENT_TARGET_CASTING => Some((p[0], p[1])),
        EVENT_SUMMONED | EVENT_SUMMONED_DEATH => Some((p[1], p[2])),
        _ => None,
    }
}

fn repeats_on_each_event(event: u32) -> bool {
    matches!(
        event,
        EVENT_SPAWNED | EVENT_RECEIVE_EMOTE | EVENT_RECEIVE_AI_EVENT | EVENT_TARGET_NOT_REACHABLE
    )
}

fn map_action(
    action: [u32; 4],
    raw_subject: i32,
    rule_id: u64,
    subject: i32,
    slot: usize,
    broadcasts: &BTreeMap<u32, Broadcast>,
    summon_locations: &BTreeMap<u32, SummonLocation>,
    quest_entries: &HashSet<u32>,
    importable_templates: &HashSet<u64>,
    source_maps: &HashMap<u32, BTreeSet<u32>>,
    relays: &RelaySource,
) -> Result<NativeAction, Vec<MappingFailure>> {
    let kind = action[0];
    if let Some(mapped) =
        eventai_presentation::map_action(action, rule_id, subject, slot, importable_templates)
    {
        return mapped.map(|mapped| NativeAction {
            encoded: mapped.encoded,
            raw_kind: kind,
            raw_target: mapped.raw_target,
            raw_cast_flags: None,
            threat_percent: None,
            texts: Vec::new(),
            summon_entry: None,
            summon_location: None,
            dependencies: mapped.dependencies,
            normalizations: mapped.normalizations,
        });
    }
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
                threat_percent: None,
                dependencies: text_dependencies(rule_id, slot, &texts),
                texts,
                summon_entry: None,
                summon_location: None,
                normalizations: Vec::new(),
            })
        }
        ACTION_TEXT_NEW => {
            let target = map_target(action[2]).map_err(|failure| vec![failure])?;
            if action[3] != 0 {
                let template_id = action[3];
                let Some(choices) = relays.string_templates.get(&template_id) else {
                    return Ok(NativeAction {
                        encoded: format!("no-effect:missing-text-template:{template_id}"),
                        raw_kind: kind,
                        raw_target: Some(action[2]),
                        raw_cast_flags: None,
                        threat_percent: None,
                        dependencies: Vec::new(),
                        texts: Vec::new(),
                        summon_entry: None,
                        summon_location: None,
                        normalizations: vec![SourceNormalization {
                            dimension: "text_template",
                            raw_value: u64::from(template_id),
                            reason: "source_runtime_missing_text_template_no_effect",
                        }],
                    });
                };
                let texts = uniform_template_targets(choices).map_err(|reason| {
                    vec![MappingFailure::dependency(
                        "eventai_text_template",
                        u64::from(template_id),
                        reason,
                        format!(
                            "rule:{rule_id} -> action:{slot} -> dbscript_random_templates:{template_id}"
                        ),
                    )]
                })?;
                let mut failures = Vec::new();
                for id in &texts {
                    if !broadcasts.contains_key(id) {
                        failures.push(MappingFailure::dependency(
                            "broadcast_text",
                            u64::from(*id),
                            "missing",
                            format!(
                                "rule:{rule_id} -> action:{slot} -> dbscript_random_templates:{template_id} -> broadcast_text:{id}"
                            ),
                        ));
                    }
                }
                let mode = speech_mode(&texts, broadcasts, rule_id, slot, &mut failures);
                if !failures.is_empty() {
                    return Err(failures);
                }
                let mut dependencies = vec![Dependency {
                    kind: "eventai_text_template",
                    path: format!(
                        "rule:{rule_id} -> action:{slot} -> dbscript_random_templates:{template_id}"
                    ),
                }];
                dependencies.extend(texts.iter().map(|id| Dependency {
                    kind: "broadcast_text",
                    path: format!(
                        "rule:{rule_id} -> action:{slot} -> dbscript_random_templates:{template_id} -> broadcast_text:{id}"
                    ),
                }));
                return Ok(NativeAction {
                    encoded: format!(
                        "speak:{mode}:{target}:{}",
                        texts
                            .iter()
                            .map(u32::to_string)
                            .collect::<Vec<_>>()
                            .join(".")
                    ),
                    raw_kind: kind,
                    raw_target: Some(action[2]),
                    raw_cast_flags: None,
                    threat_percent: None,
                    dependencies,
                    texts,
                    summon_entry: None,
                    summon_location: None,
                    normalizations: Vec::new(),
                });
            }
            if action[1] == 0 || !broadcasts.contains_key(&action[1]) {
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
            if !failures.is_empty() {
                return Err(failures);
            }
            Ok(NativeAction {
                encoded: format!("speak:{mode}:{target}:{}", action[1]),
                raw_kind: kind,
                raw_target: Some(action[2]),
                raw_cast_flags: None,
                threat_percent: None,
                dependencies: text_dependencies(rule_id, slot, &[action[1]]),
                texts: vec![action[1]],
                summon_entry: None,
                summon_location: None,
                normalizations: Vec::new(),
            })
        }
        ACTION_EMOTE => Ok(NativeAction {
            encoded: format!("emote:{}:self", action[1]),
            raw_kind: kind,
            raw_target: None,
            raw_cast_flags: None,
            threat_percent: None,
            dependencies: Vec::new(),
            texts: Vec::new(),
            summon_entry: None,
            summon_location: None,
            normalizations: Vec::new(),
        }),
        ACTION_RANDOM_EMOTE => Ok(NativeAction {
            encoded: format!(
                "random-emote:{}.{}.{}",
                action[1] as i32, action[2] as i32, action[3] as i32
            ),
            raw_kind: kind,
            raw_target: None,
            raw_cast_flags: None,
            threat_percent: None,
            dependencies: Vec::new(),
            texts: Vec::new(),
            summon_entry: None,
            summon_location: None,
            normalizations: Vec::new(),
        }),
        ACTION_CAST => {
            if action[1] == 0 {
                return Err(vec![MappingFailure::source(
                    "action",
                    u64::from(action[1]),
                    "invalid_spell",
                )]);
            }
            if !relays.spell_ids.contains(&action[1]) {
                return Err(vec![MappingFailure::dependency(
                    "spell_template",
                    u64::from(action[1]),
                    "missing",
                    format!(
                        "rule:{rule_id} -> action:{slot} -> spell_template:{}",
                        action[1]
                    ),
                )]);
            }
            let (target, options, normalizations) =
                map_cast_target_and_flags(action[2], action[3])?;
            Ok(NativeAction {
                encoded: format!("cast:{}:{target}:{options}", action[1]),
                raw_kind: kind,
                raw_target: Some(action[2]),
                raw_cast_flags: Some(action[3]),
                threat_percent: None,
                dependencies: vec![Dependency {
                    kind: "spell_template",
                    path: format!(
                        "rule:{rule_id} -> action:{slot} -> spell_template:{}",
                        action[1]
                    ),
                }],
                texts: Vec::new(),
                summon_entry: None,
                summon_location: None,
                normalizations,
            })
        }
        ACTION_THREAT_SINGLE if action[3] == 0 => {
            let percent = map_threat_percent(action[1])?;
            let target = map_target(action[2]).map_err(|failure| vec![failure])?;
            Ok(NativeAction {
                encoded: format!("threat-selected:{percent}:{target}"),
                raw_kind: kind,
                raw_target: Some(action[2]),
                raw_cast_flags: None,
                threat_percent: Some(percent),
                dependencies: Vec::new(),
                texts: Vec::new(),
                summon_entry: None,
                summon_location: None,
                normalizations: Vec::new(),
            })
        }
        ACTION_THREAT_SINGLE => Err(vec![MappingFailure::source(
            "threat_mode",
            u64::from(action[3]),
            "unsupported_direct_threat",
        )]),
        ACTION_THREAT_ALL_PCT => {
            let percent = map_threat_percent(action[1])?;
            Ok(NativeAction {
                encoded: format!("threat-all:{percent}"),
                raw_kind: kind,
                raw_target: None,
                raw_cast_flags: None,
                threat_percent: Some(percent),
                dependencies: Vec::new(),
                texts: Vec::new(),
                summon_entry: None,
                summon_location: None,
                normalizations: Vec::new(),
            })
        }
        ACTION_SET_PHASE if action[1] < 32 => Ok(NativeAction {
            encoded: format!("phase:{}", action[1]),
            raw_kind: kind,
            raw_target: None,
            raw_cast_flags: None,
            threat_percent: None,
            dependencies: Vec::new(),
            texts: Vec::new(),
            summon_entry: None,
            summon_location: None,
            normalizations: Vec::new(),
        }),
        ACTION_INCREMENT_PHASE if action[1] != 0 => Ok(NativeAction {
            encoded: format!("phase-inc:{}", action[1] as i32),
            raw_kind: kind,
            raw_target: None,
            raw_cast_flags: None,
            threat_percent: None,
            dependencies: Vec::new(),
            texts: Vec::new(),
            summon_entry: None,
            summon_location: None,
            normalizations: Vec::new(),
        }),
        ACTION_RANDOM_PHASE if action[1..].iter().all(|phase| *phase < 32) => Ok(NativeAction {
            encoded: format!("phase-random:{}.{}.{}", action[1], action[2], action[3]),
            raw_kind: kind,
            raw_target: None,
            raw_cast_flags: None,
            threat_percent: None,
            dependencies: Vec::new(),
            texts: Vec::new(),
            summon_entry: None,
            summon_location: None,
            normalizations: Vec::new(),
        }),
        ACTION_RANDOM_PHASE_RANGE if action[1] != action[2] && action[1] < 32 && action[2] < 32 => {
            let (min, max, normalizations) = if action[1] < action[2] {
                (action[1], action[2], Vec::new())
            } else {
                (
                    action[2],
                    action[1],
                    vec![SourceNormalization {
                        dimension: "phase_range",
                        raw_value: (u64::from(action[1]) << 32) | u64::from(action[2]),
                        reason: "normalized_random_phase_range",
                    }],
                )
            };
            Ok(NativeAction {
                encoded: format!("phase-range:{min}:{max}"),
                raw_kind: kind,
                raw_target: None,
                raw_cast_flags: None,
                threat_percent: None,
                dependencies: Vec::new(),
                texts: Vec::new(),
                summon_entry: None,
                summon_location: None,
                normalizations,
            })
        }
        ACTION_FLEE_FOR_ASSIST => Ok(NativeAction {
            encoded: "flee".to_string(),
            raw_kind: kind,
            raw_target: None,
            raw_cast_flags: None,
            threat_percent: None,
            dependencies: Vec::new(),
            texts: Vec::new(),
            summon_entry: None,
            summon_location: None,
            normalizations: Vec::new(),
        }),
        ACTION_CALL_FOR_HELP => Ok(NativeAction {
            encoded: format!("help:{}", action[1]),
            raw_kind: kind,
            raw_target: None,
            raw_cast_flags: None,
            threat_percent: None,
            dependencies: Vec::new(),
            texts: Vec::new(),
            summon_entry: None,
            summon_location: None,
            normalizations: Vec::new(),
        }),
        ACTION_SPAWN => {
            let entry = u64::from(action[1]);
            let mut failures = Vec::new();
            if entry == 0 || !importable_templates.contains(&entry) {
                failures.push(MappingFailure::dependency(
                    "summon_creature",
                    entry,
                    "missing",
                    format!("rule:{rule_id} -> action:{slot} -> creature_template:{entry}"),
                ));
            }
            if action[3] == 0 {
                failures.push(MappingFailure::source(
                    "action",
                    u64::from(kind),
                    "invalid_spawn_lifetime",
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
                    "spawn-at-actor:{}:{}:{}",
                    action[1],
                    target.expect("a mapped spawn has a target"),
                    action[3]
                ),
                raw_kind: kind,
                raw_target: Some(action[2]),
                raw_cast_flags: None,
                threat_percent: None,
                dependencies: vec![Dependency {
                    kind: "summon_creature",
                    path: format!("rule:{rule_id} -> action:{slot} -> creature_template:{entry}"),
                }],
                texts: Vec::new(),
                summon_entry: Some(entry),
                summon_location: None,
                normalizations: Vec::new(),
            })
        }
        ACTION_REMOVE_AURA if action[2] != 0 && action[3] == 0 => {
            let target = map_target(action[1]).map_err(|failure| vec![failure])?;
            if !relays.spell_ids.contains(&action[2]) {
                return Err(vec![MappingFailure::dependency(
                    "spell_template",
                    u64::from(action[2]),
                    "missing",
                    format!(
                        "rule:{rule_id} -> action:{slot} -> spell_template:{}",
                        action[2]
                    ),
                )]);
            }
            Ok(NativeAction {
                encoded: format!("remove-aura:{}:{target}", action[2]),
                raw_kind: kind,
                raw_target: Some(action[1]),
                raw_cast_flags: None,
                threat_percent: None,
                dependencies: vec![Dependency {
                    kind: "spell_template",
                    path: format!(
                        "rule:{rule_id} -> action:{slot} -> spell_template:{}",
                        action[2]
                    ),
                }],
                texts: Vec::new(),
                summon_entry: None,
                summon_location: None,
                normalizations: Vec::new(),
            })
        }
        ACTION_COMBAT_MOVEMENT if action[2] == 0 && action[3] == 0 => Ok(movement_action(
            kind,
            format!("combat-movement:{}", action[1] != 0),
            None,
        )),
        ACTION_EVADE if action[2] == 0 && action[3] == 0 => Ok(movement_action(
            kind,
            format!("evade:{}", action[1] != 0),
            None,
        )),
        ACTION_RANGED_MOVEMENT => Ok(NativeAction {
            encoded: format!("posture:{}:{}", action[1], action[2] as i32),
            raw_kind: kind,
            raw_target: None,
            raw_cast_flags: None,
            threat_percent: None,
            dependencies: Vec::new(),
            texts: Vec::new(),
            summon_entry: None,
            summon_location: None,
            normalizations: Vec::new(),
        }),
        ACTION_CHANGE_MOVEMENT if action[3] == 0 => {
            let encoded = match action[1] {
                0 if action[2] == 0 => "idle:stationary".to_string(),
                1 => format!("idle:random-current:{}", action[2]),
                2 => format!("idle:patrol:{}", action[2]),
                movement_type => {
                    return Err(vec![MappingFailure::source(
                        "movement_type",
                        u64::from(movement_type),
                        "unsupported_movement_type",
                    )]);
                }
            };
            Ok(movement_action(kind, encoded, None))
        }
        ACTION_CHANGE_MOVEMENT => Err(vec![MappingFailure::source(
            "movement_flag",
            u64::from(action[3]),
            format!("unsupported_movement_flag_{:#x}", action[3]),
        )]),
        ACTION_PAUSE_WAYPOINTS if action[2] == 0 && action[3] == 0 => Ok(movement_action(
            kind,
            format!("patrol-paused:{}", action[1] != 0),
            None,
        )),
        ACTION_SET_RANGED_MODE if action[1] <= 4 && action[3] == 0 => {
            let mode = match action[1] {
                0 => "none",
                1 => "full-caster",
                2 => "proximity",
                3 => "no-melee",
                4 => "distancer",
                _ => unreachable!(),
            };
            Ok(movement_action(
                kind,
                format!("ranged-mode:{mode}:{}", action[2]),
                None,
            ))
        }
        ACTION_SET_WALK if action[1] <= 3 && action[2] == 0 && action[3] == 0 => {
            let mode = match action[1] {
                0 => "run-default",
                1 => "walk-default",
                2 => "run-chase",
                3 => "walk-chase",
                _ => unreachable!(),
            };
            Ok(movement_action(kind, format!("walking:{mode}"), None))
        }
        ACTION_SET_FACING if action[2] <= 1 && action[3] == 0 => {
            let target = map_target(action[1]).map_err(|failure| vec![failure])?;
            Ok(movement_action(
                kind,
                format!("facing:{target}:{}", action[2] != 0),
                Some(action[1]),
            ))
        }
        ACTION_SET_IMMOBILIZED if action[3] == 0 => Ok(movement_action(
            kind,
            format!("immobilized:{}:{}", action[1] != 0, action[2] != 0),
            None,
        )),
        ACTION_SET_FOLLOW_MOVEMENT if action[2] == 0 && action[3] == 0 => Ok(movement_action(
            kind,
            format!("follow-movement:{}", action[1] != 0),
            None,
        )),
        ACTION_RETREAT => Err(vec![MappingFailure::source(
            "action",
            u64::from(kind),
            "unsupported_retreat",
        )]),
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
                threat_percent: None,
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
                normalizations: Vec::new(),
            })
        }
        ACTION_DIE if action[1..] == [0, 0, 0] => Ok(NativeAction {
            encoded: "force-death".to_string(),
            raw_kind: kind,
            raw_target: None,
            raw_cast_flags: None,
            threat_percent: None,
            dependencies: Vec::new(),
            texts: Vec::new(),
            summon_entry: None,
            summon_location: None,
            normalizations: Vec::new(),
        }),
        ACTION_FORCE_DESPAWN if action[2..] == [0, 0] => Ok(NativeAction {
            encoded: format!("force-despawn:{}", action[1]),
            raw_kind: kind,
            raw_target: None,
            raw_cast_flags: None,
            threat_percent: None,
            dependencies: Vec::new(),
            texts: Vec::new(),
            summon_entry: None,
            summon_location: None,
            normalizations: Vec::new(),
        }),
        ACTION_THROW_AI_EVENT if action[1] <= 11 => {
            let target = map_target(action[3]).map_err(|failure| vec![failure])?;
            Ok(NativeAction {
                encoded: format!(
                    "throw-ai-event:{}:{}:{target}",
                    ai_event_name(action[1]).expect("the bounded AI event kind has a name"),
                    action[2]
                ),
                raw_kind: kind,
                raw_target: Some(action[3]),
                raw_cast_flags: None,
                threat_percent: None,
                dependencies: Vec::new(),
                texts: Vec::new(),
                summon_entry: None,
                summon_location: None,
                normalizations: Vec::new(),
            })
        }
        ACTION_SET_STAND_STATE if u8::try_from(action[1]).is_ok() && action[2..] == [0, 0] => {
            Ok(NativeAction {
                encoded: format!("set-stand-state:{}", action[1]),
                raw_kind: kind,
                raw_target: None,
                raw_cast_flags: None,
                threat_percent: None,
                dependencies: Vec::new(),
                texts: Vec::new(),
                summon_entry: None,
                summon_location: None,
                normalizations: Vec::new(),
            })
        }
        ACTION_SET_REACT_STATE if action[2..] == [0, 0] && action[1] <= 2 => Ok(NativeAction {
            encoded: format!(
                "set-react-state:{}",
                match action[1] {
                    0 => "passive",
                    1 => "defensive",
                    2 => "aggressive",
                    _ => unreachable!("the react state is bounded"),
                }
            ),
            raw_kind: kind,
            raw_target: None,
            raw_cast_flags: None,
            threat_percent: None,
            dependencies: Vec::new(),
            texts: Vec::new(),
            summon_entry: None,
            summon_location: None,
            normalizations: Vec::new(),
        }),
        ACTION_DESPAWN_GUARDIANS if action[2..] == [0, 0] => {
            let entry = u64::from(action[1]);
            if entry != 0 && !importable_templates.contains(&entry) {
                return Err(vec![MappingFailure::dependency(
                    "creature_template",
                    entry,
                    "missing",
                    format!("rule:{rule_id} -> action:{slot} -> creature_template:{entry}"),
                )]);
            }
            Ok(NativeAction {
                encoded: format!("remove-guardians:{}", action[1]),
                raw_kind: kind,
                raw_target: None,
                raw_cast_flags: None,
                threat_percent: None,
                dependencies: (entry != 0)
                    .then(|| Dependency {
                        kind: "creature_template",
                        path: format!(
                            "rule:{rule_id} -> action:{slot} -> creature_template:{entry}"
                        ),
                    })
                    .into_iter()
                    .collect(),
                texts: Vec::new(),
                summon_entry: None,
                summon_location: None,
                normalizations: Vec::new(),
            })
        }
        ACTION_SET_DEATH_PREVENTION if action[1] <= 1 && action[2..] == [0, 0] => {
            Ok(NativeAction {
                encoded: format!("lethal-floor:{}", if action[1] == 0 { "off" } else { "on" }),
                raw_kind: kind,
                raw_target: None,
                raw_cast_flags: None,
                threat_percent: None,
                dependencies: Vec::new(),
                texts: Vec::new(),
                summon_entry: None,
                summon_location: None,
                normalizations: Vec::new(),
            })
        }
        ACTION_SET_INSTANCE_DATA => {
            let entry = u32::try_from(raw_subject).map_err(|_| {
                vec![MappingFailure::dependency(
                    "encounter_binding",
                    raw_subject.unsigned_abs() as u64,
                    "guid_subject_has_no_binding",
                    format!("rule:{rule_id} -> action:{slot} -> encounter_binding"),
                )]
            })?;
            let (map_id, binding) = encounter_binding(entry, action[1]).ok_or_else(|| {
                vec![MappingFailure::dependency(
                    "encounter_binding",
                    u64::from(action[1]),
                    "missing",
                    format!(
                        "rule:{rule_id} -> action:{slot} -> creature_template:{entry} -> encounter_field:{}",
                        action[1]
                    ),
                )]
            })?;
            if source_maps
                .get(&entry)
                .is_some_and(|maps| maps.iter().any(|source_map| *source_map != map_id))
            {
                return Err(vec![MappingFailure::dependency(
                    "encounter_binding",
                    u64::from(map_id),
                    "cross_map",
                    format!(
                        "rule:{rule_id} -> action:{slot} -> creature_template:{entry} -> map:{map_id} -> encounter_binding:{binding}"
                    ),
                )]);
            }
            if action[3] != 0 {
                return Err(vec![MappingFailure::source(
                    "action",
                    u64::from(action[3]),
                    "invalid_encounter_action_parameters",
                )]);
            }
            let signal = encounter_signal(binding, action[2]).ok_or_else(|| {
                vec![MappingFailure::source(
                    "action",
                    u64::from(action[2]),
                    "unsupported_encounter_signal",
                )]
            })?;
            Ok(NativeAction {
                encoded: format!("notify-encounter:{binding}:{signal}"),
                raw_kind: kind,
                raw_target: None,
                raw_cast_flags: None,
                threat_percent: None,
                dependencies: vec![Dependency {
                    kind: "encounter_binding",
                    path: format!(
                        "rule:{rule_id} -> action:{slot} -> map:{map_id} -> encounter_binding:{binding} -> package_handler:{signal}"
                    ),
                }],
                texts: Vec::new(),
                summon_entry: None,
                summon_location: None,
                normalizations: Vec::new(),
            })
        }
        ACTION_START_RELAY => {
            let roots = relays.gate_action(action[1], rule_id, slot)?;
            let target = map_target(action[2]).map_err(|failure| vec![failure])?;
            let root_set = roots.iter().copied().collect::<BTreeSet<_>>();
            let closure = relays.closure(&root_set);
            let dependencies = relays.action_dependencies(action[1], &roots, rule_id, slot);
            let normalizations = closure
                .iter()
                .filter_map(|relay_id| relays.definitions.get(relay_id))
                .flatten()
                .any(RawRelayStep::is_loader_skipped)
                .then_some(SourceNormalization {
                    dimension: "relay_command",
                    raw_value: 1,
                    reason: "source_loader_skipped_empty_emote",
                })
                .into_iter()
                .collect();
            Ok(NativeAction {
                encoded: format!(
                    "start-relay:{}:{target}:{RELAY_CATALOGUE_VERSION_PLACEHOLDER}",
                    roots
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(".")
                ),
                raw_kind: kind,
                raw_target: Some(action[2]),
                raw_cast_flags: None,
                threat_percent: None,
                dependencies,
                texts: Vec::new(),
                summon_entry: None,
                summon_location: None,
                normalizations,
            })
        }
        ACTION_DIE => Err(vec![MappingFailure::source(
            "action",
            ACTION_DIE as u64,
            "invalid_force_death_parameters",
        )]),
        ACTION_SET_DEATH_PREVENTION => Err(vec![MappingFailure::source(
            "death_prevention_state",
            u64::from(action[1]),
            "invalid_death_prevention_state",
        )]),
        ACTION_QUEST_EVENT => {
            let recipient: &'static str = match (action[2], action[3]) {
                (TARGET_ACTION_INVOKER, 0) => Ok("selected-character"),
                (TARGET_BENEFICIARY, 0) => Ok("invoker-beneficiary"),
                (TARGET_ACTION_INVOKER, 1) => Ok("eligible-group"),
                (_, reward_group) if reward_group > 1 => Err(MappingFailure::source(
                    "action",
                    u64::from(reward_group),
                    "invalid_quest_reward_group",
                )),
                _ => Err(MappingFailure::source(
                    "target",
                    u64::from(action[2]),
                    "unsupported_quest_recipient_policy",
                )),
            }
            .map_err(|failure| vec![failure])?;
            if action[1] == 0 {
                return Err(vec![MappingFailure::source("action", 0, "invalid_quest")]);
            }
            if !quest_entries.contains(&action[1]) {
                return Err(vec![MappingFailure::dependency(
                    "quest_template",
                    u64::from(action[1]),
                    "missing",
                    format!(
                        "rule:{rule_id} -> action:{slot} -> quest_template:{}",
                        action[1]
                    ),
                )]);
            }
            Ok(NativeAction {
                encoded: format!("quest-event:{}:{recipient}", action[1]),
                raw_kind: kind,
                raw_target: Some(action[2]),
                raw_cast_flags: None,
                threat_percent: None,
                texts: Vec::new(),
                summon_entry: None,
                summon_location: None,
                dependencies: vec![Dependency {
                    kind: "quest_template",
                    path: format!(
                        "rule:{rule_id} -> action:{slot} -> quest_template:{}",
                        action[1]
                    ),
                }],
                normalizations: Vec::new(),
            })
        }
        ACTION_KILLED_MONSTER => {
            if action[2] != TARGET_ACTION_INVOKER {
                return Err(vec![MappingFailure::source(
                    "target",
                    u64::from(action[2]),
                    "unsupported_quest_recipient_policy",
                )]);
            }
            kill_credit_action(action, rule_id, slot, importable_templates)
        }
        ACTION_SET_PHASE
        | ACTION_INCREMENT_PHASE
        | ACTION_RANDOM_PHASE
        | ACTION_RANDOM_PHASE_RANGE => Err(vec![MappingFailure::source(
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

fn map_threat_percent(raw: u32) -> Result<i32, Vec<MappingFailure>> {
    let percent = raw as i32;
    if !(-100..=100).contains(&percent) {
        return Err(vec![MappingFailure::source(
            "threat_percent",
            u64::from(raw),
            "invalid_threat_percent",
        )]);
    }
    Ok(percent)
}

fn kill_credit_action(
    action: [u32; 4],
    rule_id: u64,
    slot: usize,
    importable_templates: &HashSet<u64>,
) -> Result<NativeAction, Vec<MappingFailure>> {
    let creature_entry = action[1];
    if creature_entry == 0 {
        return Err(vec![MappingFailure::source(
            "action",
            0,
            "invalid_credit_creature",
        )]);
    }
    if !importable_templates.contains(&u64::from(creature_entry)) {
        return Err(vec![MappingFailure::dependency(
            "credit_creature",
            u64::from(creature_entry),
            "missing",
            format!("rule:{rule_id} -> action:{slot} -> creature_template:{creature_entry}"),
        )]);
    }
    Ok(NativeAction {
        encoded: format!("kill-credit:{creature_entry}:tap-group"),
        raw_kind: action[0],
        raw_target: Some(action[2]),
        raw_cast_flags: None,
        threat_percent: None,
        texts: Vec::new(),
        summon_entry: None,
        summon_location: None,
        dependencies: vec![Dependency {
            kind: "credit_creature",
            path: format!("rule:{rule_id} -> action:{slot} -> creature_template:{creature_entry}"),
        }],
        normalizations: Vec::new(),
    })
}

fn movement_action(kind: u32, encoded: String, raw_target: Option<u32>) -> NativeAction {
    NativeAction {
        encoded,
        raw_kind: kind,
        raw_target,
        raw_cast_flags: None,
        threat_percent: None,
        dependencies: Vec::new(),
        texts: Vec::new(),
        summon_entry: None,
        summon_location: None,
        normalizations: Vec::new(),
    }
}

fn encounter_binding(entry: u32, field_id: u32) -> Option<(u32, &'static str)> {
    match (entry, field_id) {
        (4832, 1) => Some((48, "blackfathom-deeps-kelris")),
        (9034..=9040, 4) => Some((230, "blackrock-depths-tomb-of-seven")),
        (11492, 0) => Some((429, "dire-maul-alzzin")),
        (4625, 1) => Some((47, "razorfen-kraul-ward-keepers")),
        (3914, 2) => Some((33, "shadowfang-keep-rethilgore")),
        (4274, 3) => Some((33, "shadowfang-keep-fenrus")),
        (3927, 4) => Some((33, "shadowfang-keep-nandos")),
        (8440, 4) => Some((109, "sunken-temple-avatar")),
        (3671, 0) => Some((43, "wailing-caverns-anacondra")),
        (3669, 1) => Some((43, "wailing-caverns-cobrahn")),
        (3670, 2) => Some((43, "wailing-caverns-pythas")),
        (3673, 3) => Some((43, "wailing-caverns-serpentis")),
        (3654, 5) => Some((43, "wailing-caverns-mutanus")),
        (11391, 5) => Some((309, "zul-gurub-ohgan")),
        _ => None,
    }
}

fn encounter_signal(binding: &str, state: u32) -> Option<&'static str> {
    match state {
        1 => Some("begin"),
        2 => Some("fail"),
        3 => Some("complete"),
        4 if binding == "dire-maul-alzzin" => Some("break-alzzin-crumble-wall"),
        4 if binding == "sunken-temple-avatar" => Some("interrupt-avatar-suppression"),
        4 if binding == "zul-gurub-ohgan" => Some("send-mandokir-downstairs"),
        _ => None,
    }
}

fn map_cast_target_and_flags(
    raw_target: u32,
    raw_flags: u32,
) -> Result<(&'static str, String, Vec<SourceNormalization>), Vec<MappingFailure>> {
    let mut failures = Vec::new();
    let mut normalizations = Vec::new();
    let residual = raw_flags & !SUPPORTED_CAST_FLAGS;
    if residual != 0 {
        failures.push(MappingFailure::source(
            "cast_flag",
            raw_flags as u64,
            format!("unsupported_cast_flag_residual_{residual:#x}"),
        ));
    }
    if raw_flags & CAST_FORCE_CAST != 0 {
        if raw_flags & CAST_TRIGGERED == 0 {
            failures.push(MappingFailure::source(
                "cast_flag",
                raw_flags as u64,
                "force_cast_requires_triggered",
            ));
        } else {
            normalizations.push(SourceNormalization {
                dimension: "cast_flag",
                raw_value: u64::from(raw_flags),
                reason: "normalized_force_cast",
            });
        }
    }
    if raw_flags & CAST_IGNORE_UNSELECTABLE != 0
        && (raw_target != TARGET_NO_EXPLICIT || raw_flags != 0x42)
    {
        failures.push(MappingFailure::source(
            "cast_flag",
            raw_flags as u64,
            "ignore_unselectable_not_profile_normalizable",
        ));
    } else if raw_flags & CAST_IGNORE_UNSELECTABLE != 0 {
        normalizations.push(SourceNormalization {
            dimension: "cast_flag",
            raw_value: u64::from(raw_flags),
            reason: "normalized_no_effect",
        });
    }
    if raw_flags & CAST_FORCE_TARGET_SELF != 0 && raw_flags & CAST_TRIGGERED == 0 {
        normalizations.push(SourceNormalization {
            dimension: "cast_flag",
            raw_value: u64::from(raw_flags),
            reason: "force_target_self_implies_triggered",
        });
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
    let start = if raw_flags & (CAST_TRIGGERED | CAST_FORCE_TARGET_SELF) != 0 {
        "triggered"
    } else {
        "direct"
    };
    let (caster, target_role) = if raw_flags & CAST_FORCE_TARGET_SELF != 0 {
        ("selected", "caster")
    } else if raw_flags & CAST_SWITCH_CASTER_TARGET != 0 {
        ("selected", "actor")
    } else if raw_target == TARGET_NO_EXPLICIT {
        ("actor", "none")
    } else if raw_target == TARGET_NEAREST_AOE {
        ("actor", "caster-area")
    } else {
        ("actor", "selected")
    };
    let options = format!(
        "{}:{start}:{caster}:{target_role}:{}:{}:{}:{}:{}",
        u8::from(raw_flags & CAST_INTERRUPT_PREVIOUS != 0),
        u8::from(raw_flags & CAST_AURA_NOT_PRESENT != 0),
        u8::from(raw_flags & CAST_PLAYER_ONLY != 0),
        u8::from(raw_flags & CAST_TARGET_CASTING != 0),
        u8::from(raw_flags & CAST_MAIN_SPELL != 0),
        u8::from(raw_flags & CAST_DISTANCE != 0),
    );
    Ok((
        target.expect("a cast without mapping failures has a target"),
        options,
        normalizations,
    ))
}

fn map_target(value: u32) -> Result<&'static str, MappingFailure> {
    match value {
        TARGET_SELF => Ok("self"),
        TARGET_HOSTILE => Ok("opponent"),
        TARGET_HOSTILE_SECOND => Ok("second-threat"),
        TARGET_HOSTILE_RANDOM => Ok("random-threat"),
        TARGET_HOSTILE_RANDOM_EXCEPT_HIGHEST => Ok("random-threat-except-highest"),
        TARGET_ACTION_INVOKER => Ok("invoker"),
        TARGET_BENEFICIARY => Ok("beneficiary"),
        TARGET_HOSTILE_RANDOM_PLAYER => Ok("random-threat-character"),
        TARGET_HOSTILE_RANDOM_PLAYER_EXCEPT_HIGHEST => Ok("random-threat-character-except-highest"),
        TARGET_AI_SENDER => Ok("ai-sender"),
        TARGET_SPAWNER => Ok("spawner"),
        TARGET_EVENT_SPECIFIC => Ok("event-subject"),
        TARGET_NO_EXPLICIT => Ok("no-explicit-spell-target"),
        TARGET_HOSTILE_MANA => Ok("random-hostile-mana-user"),
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
        if *mode > 2 {
            failures.push(MappingFailure::dependency(
                "broadcast_text",
                u64::from(*mode),
                "unsupported_chat_type",
                format!("rule:{rule_id} -> action:{slot} -> broadcast_text:chat_type:{mode}"),
            ));
        }
    }
    match ids
        .first()
        .and_then(|id| broadcasts.get(id))
        .map(|text| text.chat_type)
    {
        Some(1) => "yell",
        _ => "say",
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

    fn relay_row(
        id: u32,
        command: u32,
        datalong: u32,
        datalong2: u32,
        position: [f32; 5],
    ) -> String {
        format!(
            "({id},0,0,{command},{datalong},{datalong2},0,0,0,0,0,0,0,0,0,{},{},{},{},{},0,'fixture')",
            position[0], position[1], position[2], position[3], position[4]
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
        profile.cast_action_subjects = plan.coverage.cast_action_subjects;
        profile.template_schedule_overlaps = plan.coverage.template_schedule_overlaps;
        profile.creature_spell_list_overlaps = plan.coverage.creature_spell_list_overlaps;
        profile.direct_threat_actions = plan.coverage.direct_threat_actions;
        profile.expected_source_census.clear();
        profile.expected_threat_percent_census = plan.coverage.threat_percent.clone();
        profile.expected_ticket_census = plan.coverage.ticket_census.clone();
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
        assert_eq!(profile.direct_threat_actions, 0);
        assert_eq!(
            profile.expected_threat_percent_census,
            BTreeMap::from([(-100, 2), (-99, 1), (-75, 1), (-50, 2), (50, 1)])
        );
        assert_eq!(
            profile.expected_source_census["presentation_factions"],
            BTreeMap::from([
                (14, 3),
                (17, 1),
                (35, 6),
                (54, 1),
                (84, 1),
                (104, 4),
                (777, 1),
            ])
        );
        assert_eq!(
            profile.expected_source_census["presentation_mount_models"],
            BTreeMap::from([(0, 2), (207, 1), (2_328, 1), (9_991, 2), (14_337, 1)])
        );
        assert_eq!(
            profile.expected_source_census["actions"].get(&15),
            Some(&10)
        );
        assert_eq!(profile.expected_source_census["actions"].get(&33), Some(&2));
        for action in [16, 26, 27] {
            assert_eq!(profile.expected_source_census["actions"].get(&action), None);
        }
        assert!(source_profile("z2815").is_err());
    }

    #[test]
    fn presentation_actions_emit_only_named_compact_instructions() {
        let mut rows = Vec::new();
        let mut source_rule_id = 1;
        let mut push = |subject, action| {
            rows.push(rule(
                source_rule_id,
                subject,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [action, [0; 4], [0; 4]],
            ));
            source_rule_id += 1;
        };
        for faction in [14, 17, 35, 54, 84, 104, 777] {
            push(100, [2, faction, 0, 0]);
        }
        for template in [
            5_357, 5_358, 5_359, 5_360, 5_361, 9_621, 10_296, 11_284, 13_279, 13_738, 13_739,
            13_740, 13_741, 13_742, 14_603, 14_604, 14_638, 14_639, 14_640,
        ] {
            push(100, [36, template, 0, 0]);
        }
        push(100, [3, 11_284, 0, 0]);
        for action in [
            [17, 147, 0, 0],
            [17, 147, 3, 0],
            [17, 37, 0, 0],
            [17, 23, 0, 0],
        ] {
            push(100, action);
        }
        for action in [
            [18, 256, 0, 0],
            [18, 512, 0, 0],
            [18, 768, 0, 0],
            [18, 33_554_432, 0, 0],
        ] {
            push(100, action);
        }
        for action in [
            [19, 2, 0, 0],
            [19, 256, 0, 0],
            [19, 512, 0, 0],
            [19, 768, 0, 0],
        ] {
            push(100, action);
        }
        for action in [
            [43, 0, 0, 0],
            [43, 0, 207, 0],
            [43, 0, 2_328, 0],
            [43, 0, 9_991, 0],
            [43, 0, 14_337, 0],
        ] {
            push(100, action);
        }
        rows.push(rule(
            1_534_108,
            -155_940,
            EVENT_AGGRO,
            100,
            0,
            [0; 6],
            [[18, 832, 0, 0], [0; 4], [0; 4]],
        ));

        let source = parse(&dump(&rows));
        let (entries, mut guids, mut templates) = scope();
        guids.insert(155_940, 100);
        templates.extend([
            5_357, 5_358, 5_359, 5_360, 5_361, 9_621, 10_296, 11_284, 13_279, 13_738, 13_739,
            13_740, 13_741, 13_742, 14_603, 14_604, 14_638, 14_639, 14_640,
        ]);
        let plan = source.assemble(&entries, &guids, &templates);
        let profile = fixture_profile(&plan);
        let manifest = plan.compatibility_manifest(&profile, "fixture", LOADER_CONTRACT);
        assert!(manifest.is_apply_ready(), "{}", manifest.render());

        let definitions = plan.definition_rows.join("\n");
        for named in [
            "faction:777",
            "display-template:11284",
            "npc-flags:clear",
            "npc-flags:gossip-and-quest",
            "virtual-main-hand:clear",
            "mana:empty",
            "unit-flags:set:immune-to-players-and-creatures",
            "unit-flags:set:not-selectable",
            "unit-flags:set:rajaxx-spawn-protection",
            "unit-flags:clear:not-attackable",
            "unit-flags:clear:immune-to-players-and-creatures",
            "creature-mount:twilight-marauder",
        ] {
            assert!(
                definitions.contains(named),
                "missing {named}: {definitions}"
            );
        }
        assert_eq!(
            plan.coverage.source_census("presentation_unit_fields"),
            BTreeMap::from([(23, 1), (37, 1), (147, 2)])
        );
        assert_eq!(
            plan.coverage.source_census("presentation_set_unit_flags"),
            BTreeMap::from([(256, 1), (512, 1), (768, 1), (832, 1), (33_554_432, 1)])
        );
    }

    #[test]
    fn presentation_action_parameters_outside_the_pinned_census_drop_the_rule() {
        let source = parse(&dump(&[rule(
            10,
            100,
            EVENT_AGGRO,
            100,
            0,
            [0; 6],
            [[17, 999, 0, 0], [0; 4], [0; 4]],
        )]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        let profile = fixture_profile(&plan);
        let manifest = plan.compatibility_manifest(&profile, "fixture", LOADER_CONTRACT);
        let rendered = assert_refusal(
            &manifest,
            "unsupported_presentation_action_parameters_999_0_0",
        );
        assert_result(
            &rendered,
            "presentation_action",
            "17",
            "dropped",
            "unsupported_presentation_action_parameters_999_0_0",
        );
    }

    #[test]
    fn presentation_unit_actions_refuse_nonself_targets() {
        for action in [[17, 23, 0, 1], [18, 256, 1, 0], [19, 2, 1, 0]] {
            let source = parse(&dump(&[rule(
                10,
                100,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [action, [0; 4], [0; 4]],
            )]));
            let (entries, guids, templates) = scope();
            let plan = source.assemble(&entries, &guids, &templates);
            let profile = fixture_profile(&plan);
            let manifest = plan.compatibility_manifest(&profile, "fixture", LOADER_CONTRACT);

            let action_parameters = format!(
                "unsupported_presentation_action_parameters_{}_{}_{}",
                action[1], action[2], action[3]
            );
            let rendered = assert_refusal(&manifest, &action_parameters);
            assert_result(
                &rendered,
                "presentation_action",
                &action[0].to_string(),
                "dropped",
                &action_parameters,
            );
        }
    }

    #[test]
    fn rajaxx_client_projection_requires_the_pinned_rule_and_source_guid() {
        let source = parse(&dump(&[rule(
            1_534_108,
            100,
            EVENT_AGGRO,
            100,
            0,
            [0; 6],
            [[18, 832, 0, 0], [0; 4], [0; 4]],
        )]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        let profile = fixture_profile(&plan);
        let manifest = plan.compatibility_manifest(&profile, "fixture", LOADER_CONTRACT);
        let rendered = assert_refusal(
            &manifest,
            "unsupported_presentation_action_parameters_832_0_0",
        );
        assert_result(
            &rendered,
            "presentation_action",
            "18",
            "dropped",
            "unsupported_presentation_action_parameters_832_0_0",
        );
    }

    #[test]
    fn presentation_template_dependencies_remain_explicit() {
        let source = parse(&dump(&[rule(
            10,
            100,
            EVENT_AGGRO,
            100,
            0,
            [0; 6],
            [[3, 11_284, 0, 0], [0; 4], [0; 4]],
        )]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        let profile = fixture_profile(&plan);
        let manifest = plan.compatibility_manifest(&profile, "fixture", LOADER_CONTRACT);
        let rendered = assert_refusal(&manifest, "missing:presentation_template");
        assert_result(
            &rendered,
            "dependency",
            "presentation_template",
            "dropped",
            "missing:presentation_template",
        );
        assert!(manifest
            .render()
            .contains("rule:10 -> action:0 -> creature_template:11284"));
    }

    #[test]
    fn percent_threat_actions_keep_signed_provenance_and_source_order() {
        let source = parse(&dump(&[rule(
            10,
            100,
            EVENT_AGGRO,
            100,
            0,
            [0; 6],
            [
                [ACTION_THREAT_SINGLE as i64, -50, TARGET_HOSTILE as i64, 0],
                [ACTION_THREAT_ALL_PCT as i64, -100, 0, 0],
                [
                    ACTION_THREAT_SINGLE as i64,
                    50,
                    TARGET_HOSTILE_RANDOM_EXCEPT_HIGHEST as i64,
                    0,
                ],
            ],
        )]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        let profile = fixture_profile(&plan);
        let manifest = plan.compatibility_manifest(&profile, "fixture", LOADER_CONTRACT);

        assert!(manifest.is_apply_ready(), "{}", manifest.render());
        assert!(plan.definition_rows[0].contains(
            "threat-selected:-50:opponent+threat-all:-100+threat-selected:50:random-threat-except-highest"
        ));
        assert_eq!(
            plan.action_counts(ACTION_THREAT_SINGLE as u64),
            (2, 2, 0, 2)
        );
        assert_eq!(
            plan.action_counts(ACTION_THREAT_ALL_PCT as u64),
            (1, 1, 0, 1)
        );
        let rendered: serde_json::Value = serde_json::from_str(manifest.render()).unwrap();
        assert_eq!(rendered["source_census"]["threat_percents"]["-100"], 1);
        assert_result(&rendered, "threat_percent", "-50", "emitted", "emitted");
        assert_result(&rendered, "threat_percent", "50", "emitted", "emitted");
    }

    #[test]
    fn direct_threat_is_recognized_but_refuses_without_source_coverage() {
        let source = parse(&dump(&[rule(
            10,
            100,
            EVENT_AGGRO,
            100,
            0,
            [0; 6],
            [
                [ACTION_THREAT_SINGLE as i64, 25, TARGET_HOSTILE as i64, 1],
                [0; 4],
                [0; 4],
            ],
        )]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        let profile = fixture_profile(&plan);
        let manifest = plan.compatibility_manifest(&profile, "fixture", LOADER_CONTRACT);

        assert_eq!(plan.dropped("unsupported_direct_threat", 1), 1);
        let rendered = assert_refusal(&manifest, "unsupported_direct_threat");
        assert_result(
            &rendered,
            "threat_mode",
            "1",
            "dropped",
            "unsupported_direct_threat",
        );
    }

    #[test]
    fn instance_source_states_translate_to_package_owned_signals() {
        assert_eq!(
            encounter_signal("blackfathom-deeps-kelris", 2),
            Some("fail")
        );
        assert_eq!(
            encounter_signal("blackfathom-deeps-kelris", 3),
            Some("complete")
        );
        let broadcasts = BTreeMap::new();
        let summons = BTreeMap::new();
        let quest_entries = HashSet::new();
        let templates = HashSet::new();
        let maps = HashMap::new();
        let relays = RelaySource::default();
        let fail = map_action(
            [ACTION_SET_INSTANCE_DATA, 1, 2, 0],
            4_832,
            483_201,
            4_832,
            0,
            &broadcasts,
            &summons,
            &quest_entries,
            &templates,
            &maps,
            &relays,
        )
        .unwrap();
        let complete = map_action(
            [ACTION_SET_INSTANCE_DATA, 1, 3, 0],
            4_832,
            483_202,
            4_832,
            0,
            &broadcasts,
            &summons,
            &quest_entries,
            &templates,
            &maps,
            &relays,
        )
        .unwrap();
        assert_eq!(
            fail.encoded,
            "notify-encounter:blackfathom-deeps-kelris:fail"
        );
        assert_eq!(
            complete.encoded,
            "notify-encounter:blackfathom-deeps-kelris:complete"
        );
    }

    #[test]
    fn encounter_package_signal_keeps_the_whole_rule() {
        let source = parse(&dump(&[rule(
            10,
            4_832,
            EVENT_AGGRO,
            100,
            0,
            [0; 6],
            [
                [ACTION_EMOTE as i64, 7, 0, 0],
                [ACTION_SET_INSTANCE_DATA as i64, 1, 2, 0],
                [0; 4],
            ],
        )]));
        let entries = HashSet::from([4_832]);
        let plan = source.assemble(&entries, &HashMap::new(), &entries);
        let manifest =
            plan.compatibility_manifest(&fixture_profile(&plan), "fixture", LOADER_CONTRACT);

        assert_eq!(plan.definition_rows.len(), 1);
        assert_eq!(plan.coverage.dropped_rules, 0);
        assert!(plan.definition_rows[0]
            .contains("emote:7:self+notify-encounter:blackfathom-deeps-kelris:fail"));
        assert!(manifest.render().contains(
            "rule:10 -> action:1 -> map:48 -> encounter_binding:blackfathom-deeps-kelris -> package_handler:fail"
        ));
    }

    #[test]
    fn cross_map_encounter_binding_is_refused_before_package_resolution() {
        let maps = HashMap::from([(4_832, BTreeSet::from([33]))]);
        let result = map_action(
            [ACTION_SET_INSTANCE_DATA, 1, 2, 0],
            4_832,
            10,
            4_832,
            1,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &HashSet::new(),
            &HashSet::new(),
            &maps,
            &RelaySource::default(),
        );
        let Err(failures) = result else {
            panic!("cross-map encounter binding was accepted");
        };
        assert_eq!(failures[0].reason, "cross_map:encounter_binding");
        assert_eq!(
            failures[0].dependency_path.as_deref(),
            Some(
                "rule:10 -> action:1 -> creature_template:4832 -> map:48 -> encounter_binding:blackfathom-deeps-kelris"
            )
        );
    }

    fn relay_step(command: u32, datalong: u32, offset_ms: u32) -> RawRelayStep {
        RawRelayStep {
            offset_ms,
            priority: 0,
            source_order: 0,
            command,
            datalong,
            datalong2: 0,
            datalong3: 0,
            buddy_entry: 0,
            search_radius: 0,
            flags: 0,
            dataints: [0; 4],
            x: 0.0,
            y: 0.0,
            z: 0.0,
            orientation: 0.0,
            speed: 0.0,
            condition_id: 0,
        }
    }

    #[test]
    fn relay_participants_apply_buddy_reverse_and_self_flags_in_source_order() {
        let creature_guid = world_guid(6_251, 99);
        let gameobject_guid = go_guid(77);
        let mut source = RelaySource::default();
        source.creature_entries.insert(6_251);
        source.creature_spawns.insert(99, 6_251);
        source.gameobject_entries.insert(180_391);
        source.gameobject_spawns.insert(77, 180_391);

        let mut creature = relay_step(1, 5, 0);
        creature.buddy_entry = 6_251;
        creature.search_radius = 99;
        creature.flags = SCRIPT_FLAG_BUDDY_AS_TARGET | SCRIPT_FLAG_BUDDY_BY_GUID;
        creature.gate_participants(&source).unwrap();
        assert_eq!(
            creature.encode_subject(&source).unwrap(),
            format!("source>creature-guid:{creature_guid}")
        );

        creature.flags |= SCRIPT_FLAG_REVERSE_DIRECTION | SCRIPT_FLAG_SOURCE_TARGETS_SELF;
        assert_eq!(
            creature.encode_subject(&source).unwrap(),
            format!("creature-guid:{creature_guid}>creature-guid:{creature_guid}")
        );

        let mut gameobject = relay_step(13, 0, 0);
        gameobject.buddy_entry = 180_391;
        gameobject.search_radius = 77;
        gameobject.flags =
            SCRIPT_FLAG_BUDDY_AS_TARGET | SCRIPT_FLAG_BUDDY_BY_GUID | SCRIPT_FLAG_BUDDY_BY_GO;
        gameobject.gate_participants(&source).unwrap();
        assert_eq!(
            gameobject.encode_subject(&source).unwrap(),
            format!("source>gameobject-guid:{gameobject_guid}")
        );

        let mut all = relay_step(35, 5, 0);
        all.buddy_entry = 6_251;
        all.search_radius = 50;
        all.flags = SCRIPT_FLAG_ALL_ELIGIBLE_BUDDIES;
        assert_eq!(
            all.encode_subject(&source).unwrap(),
            "all-nearby-creatures:6251:50>selected"
        );
    }

    #[test]
    fn relay_commands_keep_overloaded_fields_and_refuse_unowned_variants() {
        let mut movement = relay_step(20, 2, 0);
        movement.datalong2 = 7;
        movement.dataints[0] = 2;
        assert_eq!(
            movement.encode_instruction().unwrap(),
            "set-movement:patrol:7:run"
        );

        let mut random_home = relay_step(20, 1, 0);
        random_home.datalong2 = 20;
        random_home.dataints[0] = 1;
        assert_eq!(
            random_home.encode_instruction().unwrap(),
            "set-movement:random-home:20:walk"
        );
        random_home.flags = SCRIPT_FLAG_COMMAND_ADDITIONAL;
        assert_eq!(
            random_home.encode_instruction().unwrap(),
            "set-movement:random-current:20:walk"
        );

        let mut terminate = relay_step(31, 177_784, 0);
        terminate.datalong2 = 40;
        terminate.flags = SCRIPT_FLAG_BUDDY_BY_GO | SCRIPT_FLAG_COMMAND_ADDITIONAL;
        assert_eq!(
            terminate.encode_instruction().unwrap(),
            "terminate-if-present:nearby-gameobject:177784:40"
        );

        assert_eq!(
            relay_step(36, 0, 0).encode_instruction().unwrap(),
            "set-facing:target"
        );
        assert_eq!(
            relay_step(36, 1, 0).encode_instruction().unwrap(),
            "set-facing:reset"
        );

        let mut world_state = relay_step(53, 0, 0);
        world_state.dataints = [19_990, 1, 0, 0];
        assert_eq!(
            world_state.encode_instruction().unwrap(),
            "set-world-state:19990:1"
        );
        assert!(world_state.has_gameplay_authority());

        let mut dynamic = relay_step(37, 0, 0);
        dynamic.dataints = [2, 20, 0, 0];
        assert_eq!(
            dynamic.encode_instruction().unwrap(),
            "move-dynamic:0:0:0:run:20"
        );
    }

    #[test]
    fn relay_graph_refuses_missing_dependencies_cycles_and_expanded_work() {
        let mut missing = RelaySource::default();
        missing.definitions.insert(1, vec![relay_step(45, 2, 0)]);
        let Err(failures) = missing.gate_action(1, 10, 0) else {
            panic!("missing relay dependency was accepted");
        };
        assert_eq!(failures[0].reason, "missing:relay_definition");
        assert_eq!(
            failures[0].dependency_path.as_deref(),
            Some("rule:10 -> action:0 -> relay:1 -> relay:2")
        );

        let mut cycle = RelaySource::default();
        cycle.definitions.insert(1, vec![relay_step(45, 2, 0)]);
        cycle.definitions.insert(2, vec![relay_step(45, 1, 0)]);
        let Err(failures) = cycle.gate_action(1, 10, 0) else {
            panic!("relay cycle was accepted");
        };
        assert_eq!(failures[0].reason, "cycle:relay_definition");
        assert_eq!(
            failures[0].dependency_path.as_deref(),
            Some("rule:10 -> action:0 -> relay:1 -> relay:2 -> relay:1")
        );

        let mut expanded = RelaySource::default();
        expanded.definitions.insert(
            1,
            vec![
                relay_step(45, 2, 0),
                relay_step(45, 2, 0),
                relay_step(45, 2, 0),
            ],
        );
        expanded
            .definitions
            .insert(2, vec![relay_step(1, 1, 0); 2_048]);
        let Err(failures) = expanded.gate_action(1, 10, 0) else {
            panic!("expanded relay work was accepted");
        };
        assert_eq!(
            failures[0].reason,
            "step_budget_exceeds_4096:relay_definition"
        );
    }

    #[test]
    fn missing_relay_dependency_drops_the_whole_rule_with_its_path() {
        let source = parse(&dump(&[rule(
            10,
            100,
            EVENT_AGGRO,
            100,
            0,
            [0; 6],
            [
                [ACTION_EMOTE as i64, 7, 0, 0],
                [ACTION_START_RELAY as i64, 404, 0, 0],
                [0; 4],
            ],
        )]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        let manifest =
            plan.compatibility_manifest(&fixture_profile(&plan), "fixture", LOADER_CONTRACT);

        assert!(plan.definition_rows.is_empty());
        assert_eq!(plan.coverage.dropped_rules, 1);
        let rendered = assert_refusal(&manifest, "missing:relay_definition");
        assert!(rendered
            .to_string()
            .contains("rule:10 -> action:1 -> relay:404"));
    }

    #[test]
    fn relay_parser_reports_malformed_fields_but_accepts_no_action_random_choices() {
        let source = RelaySource::parse(
            "INSERT INTO `dbscripts_on_relay` VALUES \
             (1,0,0,'bad',0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,''); \
             INSERT INTO `dbscript_random_templates` VALUES (4072,1,0,20,'No Action');",
        );
        assert_eq!(source.parse_failures.len(), 1);
        assert!(source.parse_failures[0].contains("field command is not u32"));
        assert!(source.random_templates.is_empty());
    }

    #[test]
    fn relay_talk_random_template_resolves_equal_choices_before_encoding() {
        let mut source = RelaySource::default();
        let talk = relay_step(0, 901, 0);
        source.broadcast_ids.insert(900);
        source.string_templates.insert(
            901,
            vec![RelayTemplateChoice {
                target_id: 900,
                chance: 0,
            }],
        );
        source.definitions.insert(1, vec![talk]);

        assert_eq!(source.gate_action(1, 10, 0).unwrap(), vec![1]);
        assert!(source.encode_closure(&BTreeSet::from([1]))[0].contains("talk:900"));
    }

    #[test]
    fn accepted_relay_talk_adds_its_text_to_the_load_plan() {
        let sql = format!(
            "INSERT INTO `broadcast_text` VALUES \
             (901,'Relay only','Relay only',0,0,0,0,0,0,0,0,0,0,0,0,0,0); \
             INSERT INTO `creature_ai_scripts` VALUES {}; \
             INSERT INTO `dbscripts_on_relay` VALUES \
             (1,0,0,0,0,0,0,0,0,0,901,0,0,0,0,0,0,0,0,0,0,'');",
            rule(
                10,
                100,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [[ACTION_START_RELAY as i64, 1, 0, 0], [0; 4], [0; 4]],
            )
        );
        let source = parse(&sql);
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);

        assert_eq!(plan.relay_definition_rows.len(), 1);
        assert_eq!(plan.broadcast_rows.len(), 1);
        assert!(plan.broadcast_rows[0].starts_with("(901,"));
        let relay = &plan.relay_definition_rows[0];
        let mut relay_fields = relay.split('@');
        let relay_id = relay_fields.next().unwrap().parse::<u32>().unwrap();
        let relay_version = relay_fields.next().unwrap().parse::<u64>().unwrap();
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"lyracore-relay-catalogue-v1");
        hasher.update(&relay_id.to_le_bytes());
        hasher.update(&relay_version.to_le_bytes());
        let catalogue_version =
            u64::from_le_bytes(hasher.finalize().as_bytes()[..8].try_into().unwrap());
        assert_ne!(catalogue_version, 0);
        assert!(
            plan.definition_rows[0].contains(&format!("start-relay:1:self:{catalogue_version}"))
        );
    }

    #[test]
    fn relay_empty_primary_emote_is_loader_skipped_without_a_runtime_step() {
        let sql = format!(
            "INSERT INTO `creature_ai_scripts` VALUES {}; \
             INSERT INTO `dbscripts_on_relay` VALUES \
             (1,0,0,1,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,'loader skips'), \
             (1,1,0,25,1,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,'run');",
            rule(
                10,
                100,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [[ACTION_START_RELAY as i64, 1, 0, 0], [0; 4], [0; 4]],
            )
        );
        let source = parse(&sql);
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        assert_eq!(plan.relay_definition_rows.len(), 1);
        assert!(plan.relay_definition_rows[0].contains("set-run:1"));
        assert!(!plan.relay_definition_rows[0].contains("emote:"));
        assert_eq!(plan.coverage.ticket_census["relay_rows"], 2);
        assert_eq!(plan.coverage.ticket_census["relay_emitted_steps"], 1);
        assert_eq!(plan.coverage.ticket_census["relay_loader_skipped_rows"], 1);

        let manifest =
            plan.compatibility_manifest(&fixture_profile(&plan), "fixture", LOADER_CONTRACT);
        assert!(manifest.is_apply_ready(), "{}", manifest.render());
        assert!(manifest
            .render()
            .contains("source_loader_skipped_empty_emote"));
    }

    #[test]
    fn text_new_uses_type_zero_string_templates_and_preserves_linked_choice_modes() {
        let sql = format!(
            "INSERT INTO `broadcast_text` VALUES \
             (7133,'First','First',1,0,0,0,0,0,0,0,0,0,0,0,0,0), \
             (7134,'Second','Second',2,0,0,0,0,0,0,0,0,0,0,0,0,0); \
             INSERT INTO `dbscript_random_templates` VALUES \
             (35,0,7133,0,'string one'),(35,0,7134,0,'string two'), \
             (36,1,9000,0,'relay'); \
             INSERT INTO `creature_ai_scripts` VALUES {};",
            rule(
                10,
                100,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [[ACTION_TEXT_NEW as i64, 0, 0, 35], [0; 4], [0; 4]],
            )
        );
        let source = parse(&sql);
        assert!(source.relays.string_templates.contains_key(&35));
        assert!(!source.relays.random_templates.contains_key(&35));
        assert!(source.relays.random_templates.contains_key(&36));

        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        assert!(plan.definition_rows[0].contains("speak:yell:self:7133.7134"));
        assert_eq!(plan.broadcast_rows.len(), 2);
        assert!(plan
            .compatibility_manifest(&fixture_profile(&plan), "fixture", LOADER_CONTRACT)
            .is_apply_ready());
    }

    #[test]
    fn missing_text_template_keeps_the_action_slot_as_a_typed_no_effect() {
        let source = parse(&dump(&[rule(
            10,
            100,
            EVENT_AGGRO,
            100,
            0,
            [0; 6],
            [[ACTION_TEXT_NEW as i64, 0, 0, 999], [0; 4], [0; 4]],
        )]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        assert!(plan.definition_rows[0].contains("no-effect:missing-text-template:999"));

        let mut profile = fixture_profile(&plan);
        profile.approvals.normalizations.insert((
            "text_template".to_string(),
            "999".to_string(),
            "source_runtime_missing_text_template_no_effect".to_string(),
        ));
        let manifest = plan.compatibility_manifest(&profile, "fixture", LOADER_CONTRACT);
        assert!(manifest.is_apply_ready(), "{}", manifest.render());
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
        assert!(entry.ends_with(
            "@10,aggro,100,4294967295,once,random,combat,any-posture,emote:7:self+help:12+phase:3"
        ));
        let guid = world_guid(100, 2);
        assert!(plan
            .definition_rows
            .iter()
            .any(|row| row.starts_with(&format!("guid:{guid}@"))
                && row.ends_with(
                    "@11,death:always,75,4294967295,once,all,ordinary,any-posture,flee"
                )));
        assert_eq!(plan.instruction_count(), 4);
        assert_eq!(
            plan.definition_batches.join("\n"),
            plan.definition_rows.join("\n")
        );

        let again = source.assemble(&entries, &guids, &templates);
        assert_eq!(plan.definition_rows, again.definition_rows);
    }

    #[test]
    fn canonical_direct_actions_use_named_runtime_instructions() {
        let mut relays = RelaySource::default();
        relays.spell_ids.insert(8_909);
        let templates = HashSet::from([6_911]);
        let cases = [
            (
                [ACTION_RANDOM_EMOTE, u32::MAX, 0, 18],
                "random-emote:-1.0.18",
            ),
            (
                [ACTION_SPAWN, 6_911, TARGET_HOSTILE, 10_000],
                "spawn-at-actor:6911:opponent:10000",
            ),
            (
                [ACTION_REMOVE_AURA, TARGET_SELF, 8_909, 0],
                "remove-aura:8909:self",
            ),
            ([ACTION_FORCE_DESPAWN, 3_000, 0, 0], "force-despawn:3000"),
            (
                [ACTION_THROW_AI_EVENT, 5, 50, TARGET_SELF],
                "throw-ai-event:custom-a:50:self",
            ),
            ([ACTION_SET_STAND_STATE, 7, 0, 0], "set-stand-state:7"),
            (
                [ACTION_SET_REACT_STATE, 2, 0, 0],
                "set-react-state:aggressive",
            ),
            ([ACTION_DESPAWN_GUARDIANS, 0, 0, 0], "remove-guardians:0"),
        ];
        for (slot, (action, expected)) in cases.into_iter().enumerate() {
            let mapped = map_action(
                action,
                100,
                10,
                100,
                slot,
                &BTreeMap::new(),
                &BTreeMap::new(),
                &HashSet::new(),
                &templates,
                &HashMap::new(),
                &relays,
            )
            .unwrap();
            assert_eq!(mapped.encoded, expected);
        }
    }

    #[test]
    fn sound_exclusion_drops_the_whole_rule_instead_of_emitting_siblings() {
        let source = parse(&dump(&[rule(
            10,
            100,
            EVENT_AGGRO,
            100,
            0,
            [0; 6],
            [
                [ACTION_SOUND as i64, 1_018, 0, 0],
                [ACTION_EMOTE as i64, 15, 0, 0],
                [0; 4],
            ],
        )]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        assert!(plan.definition_rows.is_empty());
        assert_eq!(plan.coverage.excluded_rules, 1);
        assert_eq!(plan.coverage.dropped_rules, 0);
        let manifest =
            plan.compatibility_manifest(&fixture_profile(&plan), "fixture", LOADER_CONTRACT);
        assert!(manifest.is_apply_ready(), "{}", manifest.render());
        assert!(manifest.render().contains("unsupported_sound_playback"));
    }

    #[test]
    fn pinned_quest_credit_actions_map_to_typed_recipient_policies() {
        let source = parse(&format!(
            "INSERT INTO `quest_template` VALUES (77); {}",
            dump(&[rule(
                10,
                100,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [
                    [
                        ACTION_QUEST_EVENT as i64,
                        77,
                        TARGET_ACTION_INVOKER as i64,
                        0
                    ],
                    [
                        ACTION_KILLED_MONSTER as i64,
                        100,
                        TARGET_ACTION_INVOKER as i64,
                        0
                    ],
                    [0; 4],
                ],
            ),])
        ));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);

        assert_eq!(plan.action_counts(ACTION_QUEST_EVENT as u64), (1, 1, 0, 1));
        assert_eq!(
            plan.action_counts(ACTION_KILLED_MONSTER as u64),
            (1, 1, 0, 1)
        );
        assert!(
            plan.definition_rows
                .iter()
                .any(|row| row
                    .contains("quest-event:77:selected-character+kill-credit:100:tap-group"))
        );
        assert_eq!(plan.quest_event_requirement_rows(), vec!["(1,77)"]);
    }

    #[test]
    fn zero_occurrence_quest_credit_actions_are_refused() {
        let source = parse(&dump(&[
            rule(
                10,
                100,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [[16, 100, 123, TARGET_ACTION_INVOKER as i64], [0; 4], [0; 4]],
            ),
            rule(
                11,
                100,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [[26, 77, 0, 0], [0; 4], [0; 4]],
            ),
            rule(
                12,
                100,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [[27, 100, 123, 0], [0; 4], [0; 4]],
            ),
        ]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);

        for action in [16, 26, 27] {
            assert_eq!(plan.action_counts(action), (1, 0, 1, 0));
            assert_eq!(plan.dropped("unsupported_action", action), 1);
        }
        assert!(plan.definition_rows.is_empty());
    }

    #[test]
    fn killed_monster_requires_the_imported_creature_template() {
        let source = parse(&dump(&[rule(
            12,
            100,
            EVENT_AGGRO,
            100,
            0,
            [0; 6],
            [
                [
                    ACTION_KILLED_MONSTER as i64,
                    999,
                    TARGET_ACTION_INVOKER as i64,
                    0,
                ],
                [0; 4],
                [0; 4],
            ],
        )]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        let manifest =
            plan.compatibility_manifest(&fixture_profile(&plan), "fixture", LOADER_CONTRACT);
        let rendered = assert_refusal(&manifest, "missing:credit_creature");
        assert_result(
            &rendered,
            "dependency",
            "credit_creature",
            "dropped",
            "missing:credit_creature",
        );
        assert!(rendered["results"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|result| result["dependency_paths"].as_array().unwrap())
            .any(|path| path == "rule:12 -> action:0 -> creature_template:999"));
    }

    #[test]
    fn quest_credit_missing_quest_or_recipient_context_drops_the_whole_rule() {
        let missing_quest = parse(&dump(&[rule(
            12,
            100,
            EVENT_AGGRO,
            100,
            0,
            [0; 6],
            [
                [
                    ACTION_QUEST_EVENT as i64,
                    77,
                    TARGET_ACTION_INVOKER as i64,
                    0,
                ],
                [0; 4],
                [0; 4],
            ],
        )]));
        let missing_context = parse(&format!(
            "INSERT INTO `quest_template` VALUES (77); {}",
            dump(&[rule(
                13,
                100,
                EVENT_SPAWNED,
                100,
                0,
                [0; 6],
                [
                    [
                        ACTION_QUEST_EVENT as i64,
                        77,
                        TARGET_ACTION_INVOKER as i64,
                        0
                    ],
                    [0; 4],
                    [0; 4]
                ],
            )])
        ));
        let (entries, guids, templates) = scope();

        let missing_plan = missing_quest.assemble(&entries, &guids, &templates);
        let missing_manifest = missing_plan.compatibility_manifest(
            &fixture_profile(&missing_plan),
            "fixture",
            LOADER_CONTRACT,
        );
        let rendered = assert_refusal(&missing_manifest, "missing:quest_template");
        assert_result(
            &rendered,
            "dependency",
            "quest_template",
            "dropped",
            "missing:quest_template",
        );
        assert!(rendered["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|result| result["dependency_paths"]
                .as_array()
                .unwrap()
                .iter()
                .any(|path| path == "rule:12 -> action:0 -> quest_template:77")));

        let context_plan = missing_context.assemble(&entries, &guids, &templates);
        let context_manifest = context_plan.compatibility_manifest(
            &fixture_profile(&context_plan),
            "fixture",
            LOADER_CONTRACT,
        );
        assert_refusal(&context_manifest, "target_context_unavailable_for_event_11");
    }

    #[test]
    fn definitions_emit_the_loader_posture_vocabulary() {
        let source = parse(&dump(&[
            rule(
                10,
                100,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [[ACTION_EMOTE as i64, 1, 0, 0], [0; 4], [0; 4]],
            ),
            rule(
                11,
                100,
                EVENT_AGGRO,
                100,
                FLAG_RANGED_ONLY,
                [0; 6],
                [[ACTION_EMOTE as i64, 2, 0, 0], [0; 4], [0; 4]],
            ),
            rule(
                12,
                100,
                EVENT_AGGRO,
                100,
                FLAG_MELEE_ONLY,
                [0; 6],
                [[ACTION_EMOTE as i64, 3, 0, 0], [0; 4], [0; 4]],
            ),
        ]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        let definition = &plan.definition_rows[0];

        for posture in ["any-posture", "ranged-only", "melee-only"] {
            assert!(definition.contains(posture), "{definition}");
        }
    }

    #[test]
    fn death_actions_emit_named_combat_requests_and_refuse_unknown_state() {
        let source = parse(&dump(&[
            rule(
                10,
                100,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [
                    [ACTION_SET_DEATH_PREVENTION as i64, 1, 0, 0],
                    [ACTION_SET_DEATH_PREVENTION as i64, 0, 0, 0],
                    [ACTION_DIE as i64, 0, 0, 0],
                ],
            ),
            rule(
                11,
                100,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [
                    [ACTION_SET_DEATH_PREVENTION as i64, 2, 0, 0],
                    [0; 4],
                    [0; 4],
                ],
            ),
        ]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);

        assert!(plan.definition_rows[0].ends_with("lethal-floor:on+lethal-floor:off+force-death"));
        assert_eq!(plan.dropped("invalid_death_prevention_state", 2), 1);
        let manifest =
            plan.compatibility_manifest(&fixture_profile(&plan), "fixture", LOADER_CONTRACT);
        let rendered: serde_json::Value = serde_json::from_str(manifest.render()).unwrap();
        assert_result(
            &rendered,
            "death_prevention_state",
            "0",
            "emitted",
            "emitted",
        );
        assert_result(
            &rendered,
            "death_prevention_state",
            "1",
            "emitted",
            "emitted",
        );
        assert_result(
            &rendered,
            "death_prevention_state",
            "2",
            "dropped",
            "invalid_death_prevention_state",
        );
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
    fn direct_cast_requires_and_records_its_spell_template() {
        let rule = rule(
            10,
            100,
            EVENT_AGGRO,
            100,
            0,
            [0; 6],
            [
                [ACTION_CAST as i64, 123, TARGET_SELF as i64, 0],
                [0; 4],
                [0; 4],
            ],
        );
        let (entries, guids, templates) = scope();
        let missing =
            parse(&dump(std::slice::from_ref(&rule))).assemble(&entries, &guids, &templates);
        let missing_manifest =
            missing.compatibility_manifest(&fixture_profile(&missing), "fixture", LOADER_CONTRACT);

        assert!(missing.definition_rows.is_empty());
        assert!(missing_manifest
            .render()
            .contains("rule:10 -> action:0 -> spell_template:123"));
        assert_refusal(&missing_manifest, "missing:spell_template");

        let complete = parse(&format!(
            "INSERT INTO `spell_template` VALUES (123); {}",
            dump(&[rule])
        ))
        .assemble(&entries, &guids, &templates);
        let complete_manifest = complete.compatibility_manifest(
            &fixture_profile(&complete),
            "fixture",
            LOADER_CONTRACT,
        );

        assert!(
            complete_manifest.is_apply_ready(),
            "{}",
            complete_manifest.render()
        );
        assert!(complete.definition_rows[0].contains("cast:123:self:"));
        assert!(complete_manifest
            .render()
            .contains("rule:10 -> action:0 -> spell_template:123"));
    }

    #[test]
    fn successful_relay_records_resolved_leaf_dependency_paths() {
        let mut source = parse(&dump(&[rule(
            10,
            100,
            EVENT_AGGRO,
            100,
            0,
            [0; 6],
            [
                [ACTION_START_RELAY as i64, 1, TARGET_SELF as i64, 0],
                [0; 4],
                [0; 4],
            ],
        )]));
        let mut relays = RelaySource::default();
        relays.broadcast_ids.insert(900);
        relays.spell_ids.extend([123, 124]);
        relays.item_entries.insert(2_196);
        relays.creature_entries.insert(456);
        relays.creature_spawns.insert(99, 456);
        relays.gameobject_entries.insert(789);
        relays.gameobject_spawns.insert(77, 789);
        relays.gameobject_trap_spells.insert(789, 124);
        relays.path_ids.insert(88);
        relays.gossip_menu_ids.insert(99);
        relays.world_state_ids.insert(555);
        relays.string_templates.insert(
            70,
            vec![RelayTemplateChoice {
                target_id: 900,
                chance: 0,
            }],
        );
        relays.random_templates.insert(
            71,
            vec![RelayTemplateChoice {
                target_id: 2,
                chance: 0,
            }],
        );

        let mut talk = relay_step(0, 70, 0);
        talk.source_order = 1;
        let mut cast = relay_step(15, 123, 0);
        cast.source_order = 2;
        let mut patrol = relay_step(20, 2, 0);
        patrol.datalong2 = 88;
        patrol.source_order = 3;
        let mut gossip = relay_step(52, 99, 0);
        gossip.source_order = 4;
        let mut world_state = relay_step(53, 0, 0);
        world_state.dataints = [555, 1, 0, 0];
        world_state.source_order = 5;
        let mut buddy = relay_step(1, 7, 0);
        buddy.buddy_entry = 456;
        buddy.search_radius = 99;
        buddy.flags = SCRIPT_FLAG_BUDDY_BY_GUID;
        buddy.source_order = 6;
        let mut equipment = relay_step(42, 0, 0);
        equipment.dataints = [2_196, 0, 0, 0];
        equipment.source_order = 7;
        let mut nested = relay_step(45, 0, 0);
        nested.datalong2 = 71;
        nested.source_order = 8;
        relays.definitions.insert(
            1,
            vec![
                talk,
                cast,
                patrol,
                gossip,
                world_state,
                buddy,
                equipment,
                nested,
            ],
        );

        let mut activate = relay_step(13, 0, 0);
        activate.buddy_entry = 789;
        activate.search_radius = 77;
        activate.flags =
            SCRIPT_FLAG_BUDDY_AS_TARGET | SCRIPT_FLAG_BUDDY_BY_GUID | SCRIPT_FLAG_BUDDY_BY_GO;
        activate.source_order = 9;
        relays.definitions.insert(2, vec![activate]);
        source.relays = relays;

        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        let manifest =
            plan.compatibility_manifest(&fixture_profile(&plan), "fixture", LOADER_CONTRACT);
        let rendered: serde_json::Value = serde_json::from_str(manifest.render()).unwrap();
        let dependency_kinds = rendered["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|result| result["dimension"] == "dependency")
            .map(|result| result["raw_value"].as_str().unwrap())
            .collect::<BTreeSet<_>>();

        assert!(manifest.is_apply_ready(), "{}", manifest.render());
        assert!(dependency_kinds.is_superset(&BTreeSet::from([
            "broadcast_text",
            "creature_movement_template",
            "creature_spawn",
            "creature_template",
            "gameobject_spawn",
            "gameobject_template",
            "gameobject_trap",
            "gossip_menu",
            "item_template",
            "relay_definition",
            "relay_random_template",
            "relay_string_template",
            "spell_template",
            "world_state",
        ])));
        assert!(manifest.render().contains(
            "rule:10 -> action:0 -> relay:1 -> row:8 -> relay:2 -> row:9 -> gameobject_trap:789"
        ));
    }

    #[test]
    fn every_assembled_rule_has_one_terminal_result() {
        let source = parse(&dump(&[
            rule(
                10,
                100,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [[ACTION_EMOTE as i64, 7, 0, 0], [0; 4], [0; 4]],
            ),
            rule(
                11,
                100,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [[999, 0, 0, 0], [ACTION_EMOTE as i64, 8, 0, 0], [0; 4]],
            ),
            rule(
                12,
                100,
                EVENT_AGGRO,
                100,
                FLAG_DEBUG_ONLY,
                [0; 6],
                [[ACTION_EMOTE as i64, 9, 0, 0], [0; 4], [0; 4]],
            ),
            rule(
                13,
                100,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [
                    [ACTION_EMOTE as i64, 10, 0, 0],
                    [ACTION_TEXT as i64, 901, 0, 0],
                    [0; 4],
                ],
            ),
        ]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        let manifest =
            plan.compatibility_manifest(&fixture_profile(&plan), "fixture", LOADER_CONTRACT);
        let rendered: serde_json::Value = serde_json::from_str(manifest.render()).unwrap();
        let terminals = rendered["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|result| result["dimension"] == "rule")
            .map(|result| {
                (
                    result["raw_value"].as_str().unwrap(),
                    result["classification"].as_str().unwrap(),
                    result["reason"].as_str().unwrap(),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            terminals,
            vec![
                ("10", "emitted", "emitted"),
                ("11", "normalized", "normalized"),
                ("12", "excluded", "debug_only_rule"),
                ("13", "dropped", "missing:broadcast_text"),
            ]
        );
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
    fn quest_predicates_require_the_condition_and_quest_dependency_chain() {
        let death = rule(
            54_4102,
            100,
            EVENT_DEATH,
            100,
            0,
            [100, 0, 0, 0, 0, 0],
            [[ACTION_EMOTE as i64, 7, 0, 0], [0; 4], [0; 4]],
        );
        let missing_quest_dump = format!(
            "{} INSERT INTO `conditions` VALUES (100,9,7734,0);",
            dump(std::slice::from_ref(&death))
        );
        let source = parse(&missing_quest_dump);
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        let profile = fixture_profile(&plan);
        let manifest = plan.compatibility_manifest(&profile, "fixture", LOADER_CONTRACT);
        let rendered = assert_refusal(&manifest, "missing:quest_template");
        assert_result(
            &rendered,
            "dependency",
            "quest_template",
            "dropped",
            "missing:quest_template",
        );
        assert!(manifest
            .render()
            .contains("rule:544102 -> conditions:100 -> quest_template:7734"));

        let complete_dump =
            format!("{missing_quest_dump} INSERT INTO `quest_template` VALUES (7734);");
        let source = parse(&complete_dump);
        let plan = source.assemble(&entries, &guids, &templates);
        let profile = fixture_profile(&plan);
        let manifest = plan.compatibility_manifest(&profile, "fixture", LOADER_CONTRACT);

        assert!(manifest.is_apply_ready(), "{}", manifest.render());
        assert!(plan.definition_rows[0].contains("death:quest-taken.7734"));
        assert!(manifest
            .render()
            .contains("rule:544102 -> conditions:100 -> quest_template:7734"));
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
            0x40,
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
            .contains("unsupported_event_flag_residual_0x40"));
    }

    #[test]
    fn debug_exclusion_records_raw_dimensions_without_normalization_results() {
        let source = parse(&dump(&[rule(
            10,
            100,
            EVENT_AGGRO,
            100,
            FLAG_DEBUG_ONLY | 0x02,
            [0; 6],
            [[ACTION_EMOTE as i64, 7, 0, 0], [0; 4], [0; 4]],
        )]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        let profile = fixture_profile(&plan);
        let manifest = plan.compatibility_manifest(&profile, "fixture", LOADER_CONTRACT);
        let rendered: serde_json::Value = serde_json::from_str(manifest.render()).unwrap();

        assert!(manifest.is_apply_ready(), "{}", manifest.render());
        assert_result(
            &rendered,
            "event_flag",
            "130",
            "excluded",
            "debug_only_rule",
        );
        assert!(!rendered["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|result| { result["reason"] == "normalized_reserved_event_flag" }));
    }

    #[test]
    fn debug_exclusion_cannot_hide_an_unknown_event_flag() {
        let source = parse(&dump(&[rule(
            10,
            100,
            EVENT_AGGRO,
            100,
            FLAG_DEBUG_ONLY | 0x40,
            [0; 6],
            [[ACTION_EMOTE as i64, 7, 0, 0], [0; 4], [0; 4]],
        )]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        let profile = fixture_profile(&plan);
        let manifest = plan.compatibility_manifest(&profile, "fixture", LOADER_CONTRACT);

        assert_refusal(&manifest, "unsupported_event_flag_residual_0x40");
    }

    #[test]
    fn outside_scope_exclusion_cannot_bypass_an_unapproved_normalization_tuple() {
        let source = parse(&dump(&[rule(
            10,
            101,
            EVENT_AGGRO,
            100,
            0x04,
            [0; 6],
            [[ACTION_EMOTE as i64, 7, 0, 0], [0; 4], [0; 4]],
        )]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        let profile = fixture_profile(&plan);
        let manifest = plan.compatibility_manifest(&profile, "fixture", LOADER_CONTRACT);

        assert_refusal(&manifest, "normalized_reserved_event_flag");
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
            "@{},aggro,100,4294967295,once,all,ordinary,any-posture,emote:7:self",
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
            "20,timer-combat:4294967295:4294967295,100,4294967295,once,all,ordinary,any-posture,emote:4294967295:self"
        ));
    }

    #[test]
    fn movement_rows_emit_named_intents_and_inventory_the_exact_raw_values() {
        let source = parse(&dump(&[
            rule(
                100,
                100,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [
                    [ACTION_COMBAT_MOVEMENT as i64, 0, 0, 0],
                    [ACTION_EVADE as i64, 1, 0, 0],
                    [ACTION_RANGED_MOVEMENT as i64, 10, 150, 0],
                ],
            ),
            rule(
                101,
                100,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [
                    [ACTION_CHANGE_MOVEMENT as i64, 1, 15, 0],
                    [ACTION_CHANGE_MOVEMENT as i64, 2, 0, 0],
                    [ACTION_PAUSE_WAYPOINTS as i64, 0, 0, 0],
                ],
            ),
            rule(
                102,
                100,
                EVENT_SPAWNED,
                100,
                0,
                [0; 6],
                [
                    [ACTION_SET_RANGED_MODE as i64, 2, 35, 0],
                    [ACTION_SET_WALK as i64, 1, 0, 0],
                    [ACTION_SET_FACING as i64, TARGET_SPAWNER as i64, 0, 0],
                ],
            ),
            rule(
                103,
                100,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [
                    [ACTION_SET_IMMOBILIZED as i64, 1, 0, 0],
                    [ACTION_SET_FOLLOW_MOVEMENT as i64, 0, 0, 0],
                    [0; 4],
                ],
            ),
        ]));
        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        let definitions = plan.definition_rows.join("\n");

        for encoded in [
            "combat-movement:false",
            "evade:true",
            "posture:10:150",
            "idle:random-current:15",
            "idle:patrol:0",
            "patrol-paused:false",
            "ranged-mode:proximity:35",
            "walking:walk-default",
            "facing:spawner:false",
            "immobilized:true:false",
            "follow-movement:false",
        ] {
            assert!(definitions.contains(encoded), "missing `{encoded}`");
        }

        let profile = fixture_profile(&plan);
        let manifest = plan.compatibility_manifest(&profile, "fixture", LOADER_CONTRACT);
        let rendered: serde_json::Value = serde_json::from_str(manifest.render()).unwrap();
        for raw in [
            "21:0:0:0",
            "24:1:0:0",
            "29:10:150:0",
            "48:1:15:0",
            "48:2:0:0",
            "51:0:0:0",
            "57:2:35:0",
            "58:1:0:0",
            "59:11:0:0",
            "61:1:0:0",
            "64:0:0:0",
        ] {
            assert_eq!(rendered["source_census"]["movement_values"][raw], 1);
        }

        let unsupported = parse(&dump(&[
            rule(
                110,
                100,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [[ACTION_CHANGE_MOVEMENT as i64, 2, 0, 1], [0; 4], [0; 4]],
            ),
            rule(
                111,
                100,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [[ACTION_CHANGE_MOVEMENT as i64, 4, 0, 0], [0; 4], [0; 4]],
            ),
            rule(
                112,
                100,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [[ACTION_CHANGE_MOVEMENT as i64, 2, 7, 2], [0; 4], [0; 4]],
            ),
            rule(
                113,
                100,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [[ACTION_CHANGE_MOVEMENT as i64, 3, 7, 0], [0; 4], [0; 4]],
            ),
        ]))
        .assemble(&entries, &guids, &templates);
        assert_eq!(unsupported.dropped("unsupported_movement_flag_0x1", 1), 1);
        assert_eq!(unsupported.dropped("unsupported_movement_flag_0x2", 2), 1);
        assert_eq!(unsupported.dropped("unsupported_movement_type", 3), 1);
        assert_eq!(unsupported.dropped("unsupported_movement_type", 4), 1);
    }

    #[test]
    fn reachable_relay_census_expands_templates_and_nested_edges_without_mapping_commands() {
        let source_rows = dump(&[
            rule(
                120,
                100,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [[ACTION_START_RELAY as i64, 100, 0, 0], [0; 4], [0; 4]],
            ),
            rule(
                121,
                100,
                EVENT_AGGRO,
                100,
                0,
                [0; 6],
                [[ACTION_START_RELAY as i64, -7, 0, 0], [0; 4], [0; 4]],
            ),
        ]);
        let relay_rows = [
            relay_row(100, 3, 11, 12, [1.0, 2.0, 3.0, 4.0, 5.0]),
            relay_row(100, 45, 300, 0, [0.0; 5]),
            relay_row(100, 53, 8, 9, [0.0; 5]),
            relay_row(200, 20, 1, 2, [0.0; 5]),
            relay_row(201, 25, 1, 0, [0.0; 5]),
            relay_row(300, 36, 6, 0, [0.0; 5]),
        ];
        let source = parse(&format!(
            "{source_rows} INSERT INTO `dbscript_random_templates` VALUES \
             (7,1,200,50,'first'),(7,1,201,50,'second'); \
             INSERT INTO `dbscripts_on_relay` VALUES {};",
            relay_rows.join(",")
        ));
        let relays = &source.coverage.reachable_relays;

        assert_eq!(relays.root_references, 2);
        assert_eq!(relays.direct_root_ids, BTreeSet::from([100]));
        assert_eq!(relays.random_template_ids, BTreeSet::from([7]));
        assert_eq!(relays.relay_ids, BTreeSet::from([100, 200, 201, 300]));
        assert_eq!(relays.rows, 6);
        assert_eq!(relays.nested_edges, 1);
        assert_eq!(relays.command.len(), 6);
        assert_eq!(relays.command.get(&53), Some(&1));
        assert_eq!(relays.movement_values.values().sum::<u64>(), 4);
        assert_eq!(
            relays
                .movement_values
                .get("3:11:12:0:0:0:0:0:0:0:0:0:1:2:3:4:5:0"),
            Some(&1)
        );

        let (entries, guids, templates) = scope();
        let plan = source.assemble(&entries, &guids, &templates);
        let profile = fixture_profile(&plan);
        let manifest = plan.compatibility_manifest(&profile, "fixture", LOADER_CONTRACT);
        let rendered: serde_json::Value = serde_json::from_str(manifest.render()).unwrap();
        assert_eq!(
            rendered["source_census"]["reachable_relays"]["raw_command_ids"],
            6
        );
        assert_eq!(
            rendered["source_census"]["reachable_relays"]["recognized_command_types"],
            5
        );
        assert_eq!(
            rendered["source_census"]["reachable_relays"]["unclassified_command_values"],
            serde_json::json!({})
        );
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
        let exclude_caster_spells = [3_477, 4_961, 7_154, 7_638, 11_014, 13_903];
        let exclude_caster_actions: Vec<(u64, u32)> = source
            .rules
            .iter()
            .filter(|rule| rule.event == EVENT_FRIENDLY_HP)
            .flat_map(|rule| {
                rule.actions
                    .iter()
                    .filter(|action| {
                        action[0] == ACTION_CAST
                            && action[2] == TARGET_EVENT_SPECIFIC
                            && exclude_caster_spells.contains(&action[1])
                    })
                    .map(|action| (rule.id, action[1]))
            })
            .collect();
        assert_eq!(exclude_caster_actions.len(), 14);
        assert_eq!(
            exclude_caster_actions
                .iter()
                .map(|(_, spell_id)| *spell_id)
                .collect::<BTreeSet<_>>(),
            exclude_caster_spells.into_iter().collect()
        );
        assert_eq!(source.coverage.total_rules, 10_843);
        assert_eq!(source.coverage.source_guid_rules, 39);
        let text_template_references = source
            .rules
            .iter()
            .flat_map(|rule| rule.actions)
            .filter(|action| action[0] == ACTION_TEXT_NEW && action[3] != 0)
            .map(|action| action[3])
            .collect::<Vec<_>>();
        assert_eq!(text_template_references.len(), 109);
        assert_eq!(source.relays.string_templates.len(), 57);
        assert!(text_template_references
            .iter()
            .all(|template_id| source.relays.string_templates.contains_key(template_id)));
        assert_eq!(
            source
                .broadcasts
                .values()
                .filter(|text| text.chat_type == 2)
                .count(),
            314
        );
        assert_eq!(
            uniform_template_targets(&source.relays.string_templates[&35]).unwrap(),
            vec![7_133, 7_134]
        );
        assert!(!source.relays.random_templates.contains_key(&35));
        let relays = &source.coverage.reachable_relays;
        assert_eq!(relays.root_references, 141);
        assert_eq!(relays.direct_root_ids.len(), 109);
        assert_eq!(relays.random_template_ids, BTreeSet::from([33, 39, 20_055]));
        assert_eq!(relays.relay_ids.len(), 120);
        assert_eq!(relays.rows, 447);
        assert_eq!(relays.command.values().sum::<u64>(), 447);
        assert_eq!(relays.command.len(), 26);
        assert_eq!(
            relays
                .command
                .keys()
                .filter(|command| **command != 45)
                .count(),
            25
        );
        assert_eq!(
            relays
                .command
                .keys()
                .copied()
                .filter(|command| *command != 45)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                0, 1, 3, 10, 13, 15, 18, 20, 21, 22, 25, 26, 28, 29, 31, 32, 35, 36, 37, 40, 42,
                44, 48, 52, 53,
            ])
        );
        assert_eq!(relays.command.get(&53), Some(&2));
        assert_eq!(relays.nested_edges, 1);
        for (command, count) in [
            (3, 104),
            (20, 11),
            (25, 14),
            (26, 3),
            (32, 16),
            (36, 20),
            (37, 2),
        ] {
            assert_eq!(relays.command.get(&command), Some(&count));
        }
        assert_eq!(relays.movement_values.values().sum::<u64>(), 170);
        let movement_dependencies = &source.coverage.movement_dependencies;
        assert_eq!(movement_dependencies.ranged_mode_actions, 701);
        assert_eq!(movement_dependencies.ranged_mode_subjects.len(), 686);
        assert_eq!(
            movement_dependencies.resolved_main_spell_subjects.len(),
            660
        );
        assert_eq!(
            movement_dependencies.missing_main_spell_subjects,
            BTreeSet::from([
                589, 595, 619, 815, 891, 1_013, 1_487, 1_489, 1_490, 1_539, 2_534, 2_638, 3_272,
                3_502, 4_624, 6_222, 6_223, 6_224, 9_451, 9_460, 10_411, 11_190, 12_856, 15_634,
                16_096, 16_379,
            ])
        );
        assert_eq!(
            source.rules.len(),
            10_843,
            "loader-valid rows lost during parsing: {:?}",
            source.coverage.dropped_values
        );
        let normalization_census = source
            .rules
            .iter()
            .flat_map(|rule| &rule.normalizations)
            .fold(BTreeMap::new(), |mut counts, normalization| {
                *counts
                    .entry((
                        normalization.dimension,
                        normalization.raw_value,
                        normalization.reason,
                    ))
                    .or_insert(0) += 1;
                counts
            });
        assert_eq!(
            normalization_census,
            BTreeMap::from([
                (("event_flag", 2, "normalized_reserved_event_flag"), 2),
                (("event_flag", 1027, "normalized_reserved_event_flag"), 3),
                (("event_flag", 1029, "normalized_reserved_event_flag"), 2),
                (("event_flag", 1031, "normalized_reserved_event_flag"), 3),
                (("event_flag", 1205, "normalized_reserved_event_flag"), 1),
            ])
        );
        assert_eq!(source.coverage.template_schedule_overlaps, 2_108);
        assert_eq!(source.coverage.creature_spell_list_overlaps, 38);
        assert_eq!(source.coverage.cast_action_subjects, 3_556);
        let death_prevention_states = source
            .rules
            .iter()
            .flat_map(|rule| rule.actions)
            .filter(|action| action[0] == ACTION_SET_DEATH_PREVENTION)
            .fold(BTreeMap::new(), |mut counts, action| {
                *counts.entry(action[1]).or_insert(0) += 1;
                counts
            });
        assert_eq!(death_prevention_states, BTreeMap::from([(0, 9), (1, 21)]));
        assert_eq!(
            source
                .rules
                .iter()
                .flat_map(|rule| rule.actions)
                .filter(|action| action[0] == ACTION_DIE)
                .count(),
            11
        );
        assert_eq!(
            source
                .rules
                .iter()
                .filter(|rule| rule.event == EVENT_DEATH_PREVENTED)
                .count(),
            0
        );
        let profile = source_profile(SOURCE_PROFILE_NAME).unwrap();
        assert_eq!(source.coverage.direct_threat_actions, 0);
        assert_eq!(
            source.coverage.threat_percent,
            profile.expected_threat_percent_census
        );
        for (dimension, expected) in profile.expected_source_census {
            assert_eq!(
                source.coverage.source_census(&dimension),
                expected,
                "{dimension}"
            );
        }
        assert_eq!(
            source.coverage.ticket_census,
            profile.expected_ticket_census
        );

        let mut encounter_instructions = Vec::new();
        for rule in &source.rules {
            for (slot, action) in rule.actions.iter().enumerate() {
                if action[0] != ACTION_SET_INSTANCE_DATA {
                    continue;
                }
                encounter_instructions.push(
                    map_action(
                        *action,
                        rule.subject,
                        rule.id,
                        rule.subject,
                        slot,
                        &source.broadcasts,
                        &source.summon_locations,
                        &source.quest_entries,
                        &HashSet::new(),
                        &source.source_maps,
                        &source.relays,
                    )
                    .unwrap()
                    .encoded,
                );
            }
        }
        assert_eq!(encounter_instructions.len(), 37);
        assert_eq!(
            encounter_instructions
                .iter()
                .filter(|instruction| instruction.ends_with(":begin"))
                .count(),
            8
        );
        assert_eq!(
            encounter_instructions
                .iter()
                .filter(|instruction| instruction.ends_with(":fail"))
                .count(),
            15
        );
        assert_eq!(
            encounter_instructions
                .iter()
                .filter(|instruction| instruction.ends_with(":complete"))
                .count(),
            11
        );
        assert_eq!(
            encounter_instructions
                .iter()
                .filter(|instruction| {
                    instruction.ends_with(":break-alzzin-crumble-wall")
                        || instruction.ends_with(":interrupt-avatar-suppression")
                        || instruction.ends_with(":send-mandokir-downstairs")
                })
                .count(),
            3
        );
        assert_eq!(
            encounter_instructions
                .iter()
                .filter_map(|instruction| instruction.split(':').nth(1))
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "blackfathom-deeps-kelris",
                "blackrock-depths-tomb-of-seven",
                "dire-maul-alzzin",
                "razorfen-kraul-ward-keepers",
                "shadowfang-keep-fenrus",
                "shadowfang-keep-nandos",
                "shadowfang-keep-rethilgore",
                "sunken-temple-avatar",
                "wailing-caverns-anacondra",
                "wailing-caverns-cobrahn",
                "wailing-caverns-mutanus",
                "wailing-caverns-pythas",
                "wailing-caverns-serpentis",
                "zul-gurub-ohgan",
            ])
        );

        let mut accepted_relay_actions = 0;
        let mut relay_failures = BTreeMap::new();
        for rule in &source.rules {
            for (slot, action) in rule.actions.iter().enumerate() {
                if action[0] != ACTION_START_RELAY {
                    continue;
                }
                match source.relays.gate_action(action[1], rule.id, slot) {
                    Ok(_) => accepted_relay_actions += 1,
                    Err(failures) => {
                        *relay_failures
                            .entry(failures[0].reason.clone())
                            .or_insert(0) += 1;
                    }
                }
            }
        }
        assert_eq!(accepted_relay_actions, 141);
        assert_eq!(relay_failures, BTreeMap::new());

        let root_ids = source
            .rules
            .iter()
            .flat_map(|rule| rule.actions)
            .filter(|action| action[0] == ACTION_START_RELAY)
            .flat_map(|action| source.relays.root_choices(action[1]).unwrap_or_default())
            .collect::<BTreeSet<_>>();
        let relay_rows = source
            .relays
            .closure(&root_ids)
            .into_iter()
            .flat_map(|id| source.relays.definitions.get(&id).into_iter().flatten())
            .collect::<Vec<_>>();
        assert_eq!(root_ids.len(), 119);
        assert_eq!(source.relays.closure(&root_ids).len(), 121);
        assert_eq!(relay_rows.len(), 451);
        assert_eq!(
            relay_rows
                .iter()
                .filter(|row| row.is_loader_skipped())
                .count(),
            2
        );
        let unowned_rows = relay_rows
            .iter()
            .filter(|row| !row.has_gameplay_authority())
            .fold(BTreeMap::new(), |mut counts, row| {
                *counts.entry(row.command).or_insert(0) += 1;
                counts
            });
        assert_eq!(unowned_rows, BTreeMap::new());
        let refused_owned_rows = relay_rows
            .iter()
            .filter(|row| !row.is_loader_skipped())
            .filter(|row| row.has_gameplay_authority())
            .filter_map(|row| row.encode_instruction_with_source(&source.relays).err())
            .fold(BTreeMap::new(), |mut counts, reason| {
                *counts.entry(reason).or_insert(0) += 1;
                counts
            });
        assert_eq!(refused_owned_rows, BTreeMap::new());
        let emitted_rows = relay_rows
            .iter()
            .filter(|row| !row.is_loader_skipped())
            .map(|row| row.encode_instruction_with_source(&source.relays).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(emitted_rows.len(), 449);
        assert!(emitted_rows
            .iter()
            .any(|instruction| instruction == "set-movement:random-home:20:walk"));
    }
}
