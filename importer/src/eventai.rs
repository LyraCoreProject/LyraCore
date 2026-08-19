//! CMaNGOS EventAI import boundary.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::{bt, field, parse_table, sql_text, world_guid};

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
const ACTION_SET_PHASE: u32 = 22;
const ACTION_FLEE_FOR_ASSIST: u32 = 25;
const ACTION_RANGED_MOVEMENT: u32 = 29;
const ACTION_SUMMON_ID: u32 = 32;
const ACTION_CALL_FOR_HELP: u32 = 39;

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

const NATIVE_EVENT_ON_AGGRO: u8 = 0;
const NATIVE_EVENT_TIMED_IN_COMBAT: u8 = 1;
const NATIVE_EVENT_CREATURE_HP: u8 = 2;
const NATIVE_EVENT_ON_DEATH: u8 = 3;
const NATIVE_EVENT_TARGET_RANGE: u8 = 4;
const NATIVE_EVENT_ON_SPAWN: u8 = 5;
const NATIVE_EVENT_FRIENDLY_HP_DEFICIT: u8 = 6;

const NATIVE_ACTION_SAY: u8 = 0;
const NATIVE_ACTION_CAST: u8 = 2;
const NATIVE_ACTION_EMOTE: u8 = 3;
const NATIVE_ACTION_FLEE_FOR_ASSIST: u8 = 4;
const NATIVE_ACTION_CALL_FOR_HELP: u8 = 5;
const NATIVE_ACTION_SET_PHASE: u8 = 6;
const NATIVE_ACTION_SUMMON: u8 = 7;
const NATIVE_ACTION_SET_RANGED_POSTURE: u8 = 8;

const NATIVE_TARGET_CURRENT: u8 = 0;
const NATIVE_TARGET_SELF: u8 = 1;
const NATIVE_TARGET_SECOND_THREAT: u8 = 3;
const NATIVE_TARGET_RANDOM_THREAT: u8 = 4;
const NATIVE_TARGET_INVOKER: u8 = 5;
const NATIVE_TARGET_EVENT: u8 = 6;
const NATIVE_TARGET_RANDOM_THREAT_PLAYER: u8 = 8;
const NATIVE_TARGET_NEAREST_AREA: u8 = 9;
const NATIVE_TARGET_FARTHEST_HOSTILE: u8 = 10;

const NATIVE_SOURCE_COMBAT_ACTION: u32 = 1;
const NATIVE_SOURCE_RANDOM_ACTION: u32 = 1 << 1;
const NATIVE_REPEAT_ONCE: u8 = 0;
const NATIVE_REPEAT: u8 = 1;
const NATIVE_CAST_INTERRUPT_PREVIOUS: u32 = 1;
const NATIVE_CAST_AURA_ABSENT: u32 = 1 << 2;
const NATIVE_CAST_PLAYER_ONLY: u32 = 1 << 3;
const NATIVE_CAST_TARGET_CASTING: u32 = 1 << 4;
const ROW_ID_NAMESPACE: u64 = 0x4000_0000_0000_0000;
const FIXTURE_ID_FIRST: u32 = 5_099_000;
const FIXTURE_ID_LAST: u32 = 5_099_999;

#[derive(Default)]
pub(crate) struct EventAiPlan {
    pub(crate) event_rows: Vec<String>,
    pub(crate) broadcast_rows: Vec<String>,
    pub(crate) summon_rows: Vec<String>,
    pub(crate) forced_template_entries: BTreeSet<u64>,
    coverage: Coverage,
}

impl EventAiPlan {
    pub(crate) fn report(&self) {
        self.coverage
            .report(self.broadcast_rows.len(), self.summon_rows.len());
    }

    pub(crate) fn row_count(&self) -> u64 {
        (self.event_rows.len() + self.broadcast_rows.len() + self.summon_rows.len()) as u64
    }

    #[cfg(test)]
    pub(crate) fn dropped(&self, reason: &str, value: u64) -> u64 {
        self.coverage
            .dropped_values
            .get(&(reason.to_string(), value))
            .copied()
            .unwrap_or(0)
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

#[derive(Clone, Default)]
struct Coverage {
    total_rules: u64,
    accepted_rules: u64,
    action_rows: u64,
    guid_rules: u64,
    event: BTreeMap<u64, u64>,
    action: BTreeMap<u64, u64>,
    accepted_event: BTreeMap<u64, u64>,
    accepted_action: BTreeMap<u64, u64>,
    emitted_event: BTreeMap<u64, u64>,
    emitted_action: BTreeMap<u64, u64>,
    target: BTreeMap<u64, u64>,
    flags: BTreeMap<u64, u64>,
    cast_flags: BTreeMap<u64, u64>,
    unverified_spells: BTreeMap<u64, u64>,
    dropped: BTreeMap<String, u64>,
    dropped_values: BTreeMap<(String, u64), u64>,
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

    fn report(&self, broadcasts: usize, summons: usize) {
        eprintln!(
            "eventai: rules={} accepted={} action_rows={} broadcasts={} summons={} guid_rules={}",
            self.total_rules,
            self.accepted_rules,
            self.action_rows,
            broadcasts,
            summons,
            self.guid_rules,
        );
        for (label, source, accepted, emitted) in [
            (
                "events",
                &self.event,
                &self.accepted_event,
                &self.emitted_event,
            ),
            (
                "actions",
                &self.action,
                &self.accepted_action,
                &self.emitted_action,
            ),
        ] {
            let rendered = source
                .iter()
                .map(|(value, count)| {
                    let accepted = accepted.get(value).copied().unwrap_or(0);
                    let emitted = emitted.get(value).copied().unwrap_or(0);
                    format!(
                        "{value}:source={count}/accepted={accepted}/dropped={}/emitted={emitted}",
                        count.saturating_sub(accepted)
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            eprintln!("eventai: {label} [{rendered}]");
        }
        for (label, values) in [
            ("targets", &self.target),
            ("flags", &self.flags),
            ("cast_flags", &self.cast_flags),
        ] {
            let rendered = values
                .iter()
                .map(|(value, count)| format!("{value}={count}"))
                .collect::<Vec<_>>()
                .join(",");
            eprintln!("eventai: {label} [{rendered}]");
        }
        let spells = self
            .unverified_spells
            .iter()
            .map(|(value, count)| format!("{value}={count}"))
            .collect::<Vec<_>>()
            .join(",");
        eprintln!("eventai: spell_preflight_unavailable [{spells}]");
        let drops = self
            .dropped_values
            .iter()
            .map(|((reason, value), count)| format!("{reason}/{value}={count}"))
            .collect::<Vec<_>>()
            .join(",");
        eprintln!("eventai: drops [{drops}]");
    }
}

struct RawRule {
    id: u64,
    subject: i64,
    event: u32,
    inverse_phase_mask: u32,
    chance: u32,
    flags: u32,
    params: [u32; 6],
    actions: [[u32; 4]; 3],
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

/// A row names its creature by template entry or by spawn guid, never both. The Module reads the
/// pair and refuses a row that sets neither or sets both.
enum Subject {
    Entry(u32),
    Guid(u64),
}

impl Subject {
    fn columns(&self) -> (u32, u64) {
        match *self {
            Self::Entry(entry) => (entry, 0),
            Self::Guid(guid) => (0, guid),
        }
    }
}

struct NativeAction {
    kind: u8,
    params: [u32; 3],
    target: u8,
    cast_options: u32,
    legacy_text: &'static str,
    texts: Vec<u32>,
    summon_entry: Option<u64>,
    summon_location: Option<u32>,
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

        for rule in &self.rules {
            let Some(subject) = resolve_subject(
                rule,
                imported_entries,
                imported_guid_entries,
                importable_templates,
                &mut plan.coverage,
            ) else {
                continue;
            };
            let (creature_entry, creature_guid) = subject.columns();
            let Some((event, params)) = map_event(rule, &mut plan.coverage) else {
                continue;
            };
            if rule.chance == 0 || rule.chance > 100 {
                plan.coverage.drop("invalid_chance", rule.chance as u64);
                continue;
            }
            if rule.flags & !SUPPORTED_FLAGS != 0 {
                plan.coverage.drop("unsupported_flag", rule.flags as u64);
                continue;
            }
            let allowed_phase_mask = !rule.inverse_phase_mask;
            if allowed_phase_mask == 0 {
                plan.coverage
                    .drop("empty_phase_mask", rule.inverse_phase_mask as u64);
                continue;
            }
            let mut actions = Vec::new();
            let mut failed = false;
            for (slot, action) in rule.actions.iter().enumerate() {
                let raw_action = action[0];
                if raw_action == 0 {
                    continue;
                }
                match map_action(
                    *action,
                    broadcasts,
                    summon_locations,
                    importable_templates,
                    &mut plan.coverage,
                ) {
                    Some(action) => actions.push((slot as u8, raw_action, action)),
                    None => {
                        failed = true;
                        break;
                    }
                }
            }
            if failed {
                continue;
            }
            if actions.is_empty() {
                plan.coverage.drop("empty_rule", rule.id);
                continue;
            }
            let action_count = actions.len() as u64;
            if rule.id == 0 || rule.id > ((ROW_ID_NAMESPACE - 1) >> 2) {
                plan.coverage.drop("invalid_rule_id", rule.id);
                continue;
            }
            let rule_prefix = ROW_ID_NAMESPACE | (rule.id << 2);
            let source_flags = if rule.flags & FLAG_COMBAT_ACTION != 0 {
                NATIVE_SOURCE_COMBAT_ACTION
            } else {
                0
            } | if rule.flags & FLAG_RANDOM_ACTION != 0 {
                NATIVE_SOURCE_RANDOM_ACTION
            } else {
                0
            };
            let repeat = if rule.flags & FLAG_REPEATABLE != 0 {
                NATIVE_REPEAT
            } else {
                NATIVE_REPEAT_ONCE
            };
            Coverage::source_value(&mut plan.coverage.accepted_event, rule.event as u64);
            *plan
                .coverage
                .emitted_event
                .entry(rule.event as u64)
                .or_default() += action_count;
            for (_, raw_action, _) in &actions {
                Coverage::source_value(&mut plan.coverage.accepted_action, *raw_action as u64);
                Coverage::source_value(&mut plan.coverage.emitted_action, *raw_action as u64);
            }
            for (order, _, action) in actions {
                let row_id = rule_prefix | u64::from(order);
                used_texts.extend(action.texts.iter().copied());
                if let Some(entry) = action.summon_entry {
                    plan.forced_template_entries.insert(entry);
                }
                if let Some(location) = action.summon_location {
                    used_summons.insert(location);
                }
                plan.event_rows.push(format!(
                    "({row_id},{creature_entry},{event},{kind},{legacy_text},{spell},0,0,0,0,{rule_id},{order},{creature_guid},{chance},{allowed_phase_mask},{source_flags},{repeat},{p1},{p2},{p3},{p4},{p5},{p6},{a1},{a2},{a3},{target},{cast_options})",
                    kind = action.kind,
                    legacy_text = sql_text(action.legacy_text),
                    spell = if action.kind == NATIVE_ACTION_CAST { action.params[0] } else { 0 },
                    rule_id = rule.id,
                    chance = rule.chance,
                    p1 = params[0],
                    p2 = params[1],
                    p3 = params[2],
                    p4 = params[3],
                    p5 = params[4],
                    p6 = params[5],
                    a1 = action.params[0],
                    a2 = action.params[1],
                    a3 = action.params[2],
                    target = action.target,
                    cast_options = action.cast_options,
                ));
            }
            plan.coverage.accepted_rules += 1;
            plan.coverage.action_rows += action_count;
        }

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

fn parse_rules(dump: &str, coverage: &mut Coverage) -> Vec<RawRule> {
    parse_table(dump, "creature_ai_scripts")
        .into_iter()
        .filter_map(|row| {
            coverage.total_rules += 1;
            if row.len() < 24 {
                coverage.drop("malformed_rule", row.len() as u64);
                return None;
            }
            let Some(id) = source_u64(field(&row, 0)) else {
                coverage.drop("invalid_numeric", 0);
                return None;
            };
            let Some(subject) = field(&row, 1).parse().ok() else {
                coverage.drop("invalid_numeric", 1);
                return None;
            };
            let Some(event) = source_u32(field(&row, 2)) else {
                coverage.drop("invalid_numeric", 2);
                return None;
            };
            Coverage::source_value(&mut coverage.event, event as u64);
            let Some(inverse_phase_mask) = source_u32(field(&row, 3)) else {
                coverage.drop("invalid_numeric", 3);
                return None;
            };
            let Some(chance) = source_u32(field(&row, 4)) else {
                coverage.drop("invalid_numeric", 4);
                return None;
            };
            let Some(flags) = source_u32(field(&row, 5)) else {
                coverage.drop("invalid_numeric", 5);
                return None;
            };
            Coverage::source_value(&mut coverage.flags, flags as u64);
            let mut params = [0; 6];
            let mut actions = [[0; 4]; 3];
            for (index, param) in params.iter_mut().enumerate() {
                let Some(value) = source_u32(field(&row, 6 + index)) else {
                    coverage.drop("invalid_numeric", (6 + index) as u64);
                    return None;
                };
                *param = value;
            }
            for (slot, action) in actions.iter_mut().enumerate() {
                let action_column = 12 + slot * 4;
                let Some(kind) = source_u32(field(&row, action_column)) else {
                    coverage.drop("invalid_numeric", action_column as u64);
                    return None;
                };
                action[0] = kind;
                if kind != 0 {
                    Coverage::source_value(&mut coverage.action, kind as u64);
                }
                for (part, value) in action.iter_mut().enumerate().skip(1) {
                    let column = action_column + part;
                    let parsed =
                        if kind == ACTION_TEXT || (kind == ACTION_RANGED_MOVEMENT && part == 2) {
                            source_action_u32(field(&row, column))
                        } else {
                            source_u32(field(&row, column))
                        };
                    let Some(parsed) = parsed else {
                        coverage.drop("invalid_numeric", column as u64);
                        return None;
                    };
                    *value = parsed;
                }
            }
            Some(RawRule {
                id,
                subject,
                event,
                inverse_phase_mask,
                chance,
                flags,
                params,
                actions,
            })
        })
        .collect()
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

fn resolve_subject(
    rule: &RawRule,
    imported_entries: &HashSet<u64>,
    imported_guid_entries: &HashMap<u64, u64>,
    importable_templates: &HashSet<u64>,
    coverage: &mut Coverage,
) -> Option<Subject> {
    if rule.subject > 0 {
        let entry = rule.subject as u64;
        if imported_entries.contains(&entry) && importable_templates.contains(&entry) {
            return u32::try_from(entry).ok().map(Subject::Entry);
        }
        coverage.drop("missing_entry_subject", entry);
        return None;
    }
    if rule.subject >= 0 {
        coverage.drop("invalid_subject", rule.subject as u64);
        return None;
    }
    let source_guid = rule.subject.unsigned_abs();
    let Some(&entry) = imported_guid_entries.get(&source_guid) else {
        coverage.drop("missing_guid_subject", source_guid);
        return None;
    };
    if !importable_templates.contains(&entry) {
        coverage.drop("missing_guid_template", entry);
        return None;
    }
    coverage.guid_rules += 1;
    Some(Subject::Guid(world_guid(entry, source_guid)))
}

fn map_event(rule: &RawRule, coverage: &mut Coverage) -> Option<(u8, [u32; 6])> {
    let params = rule.params;
    let valid_timer = |params: [u32; 6]| params[0] <= params[1] && params[2] <= params[3];
    match rule.event {
        EVENT_TIMER_IN_COMBAT if valid_timer(params) => {
            Some((NATIVE_EVENT_TIMED_IN_COMBAT, params))
        }
        EVENT_HP if params[0] <= 100 && params[1] <= params[0] && params[2] <= params[3] => Some((
            NATIVE_EVENT_CREATURE_HP,
            [
                params[1], params[0], params[2], params[3], params[4], params[5],
            ],
        )),
        EVENT_AGGRO if params == [0; 6] => Some((NATIVE_EVENT_ON_AGGRO, params)),
        EVENT_DEATH if params == [0; 6] => Some((NATIVE_EVENT_ON_DEATH, params)),
        EVENT_RANGE if valid_timer(params) => Some((NATIVE_EVENT_TARGET_RANGE, params)),
        EVENT_SPAWNED if params[0] == 0 && params[1..] == [0; 5] => {
            Some((NATIVE_EVENT_ON_SPAWN, params))
        }
        EVENT_FRIENDLY_HP if params[1] > 0 && params[2] <= params[3] => {
            Some((NATIVE_EVENT_FRIENDLY_HP_DEFICIT, params))
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
            coverage.drop("invalid_event_parameters", value as u64);
            None
        }
        value => {
            coverage.drop("unsupported_event", value as u64);
            None
        }
    }
}

fn map_action(
    action: [u32; 4],
    broadcasts: &BTreeMap<u32, Broadcast>,
    summon_locations: &BTreeMap<u32, SummonLocation>,
    importable_templates: &HashSet<u64>,
    coverage: &mut Coverage,
) -> Option<NativeAction> {
    let kind = action[0];
    match kind {
        ACTION_TEXT => {
            let texts = action[1..]
                .iter()
                .copied()
                .filter(|id| *id != 0)
                .collect::<Vec<_>>();
            if texts.is_empty() || texts.iter().any(|id| !broadcasts.contains_key(id)) {
                let reason = if texts.iter().any(|id| (*id as i32) < 0) {
                    "unsupported_legacy_text"
                } else {
                    "missing_broadcast_text"
                };
                coverage.drop(reason, action[1] as u64);
                return None;
            }
            if !supported_texts(&texts, broadcasts, coverage) {
                return None;
            }
            Some(NativeAction {
                kind: NATIVE_ACTION_SAY,
                params: [action[1], action[2], action[3]],
                target: NATIVE_TARGET_SELF,
                cast_options: 0,
                legacy_text: "",
                texts,
                summon_entry: None,
                summon_location: None,
            })
        }
        ACTION_TEXT_NEW => {
            if action[3] != 0 {
                coverage.drop("unsupported_text_template", action[3] as u64);
                return None;
            }
            if !broadcasts.contains_key(&action[1]) {
                coverage.drop("missing_broadcast_text", action[1] as u64);
                return None;
            }
            if !supported_texts(&[action[1]], broadcasts, coverage) {
                return None;
            }
            let target = map_target(action[2], coverage)?;
            Some(NativeAction {
                kind: NATIVE_ACTION_SAY,
                params: [action[1], 0, 0],
                target,
                cast_options: 0,
                legacy_text: "",
                texts: vec![action[1]],
                summon_entry: None,
                summon_location: None,
            })
        }
        ACTION_EMOTE => Some(NativeAction {
            kind: NATIVE_ACTION_EMOTE,
            params: [action[1], action[2], action[3]],
            target: NATIVE_TARGET_SELF,
            cast_options: 0,
            legacy_text: "",
            texts: Vec::new(),
            summon_entry: None,
            summon_location: None,
        }),
        ACTION_CAST => {
            if action[1] == 0 {
                coverage.drop("invalid_spell", 0);
                return None;
            }
            Coverage::source_value(&mut coverage.unverified_spells, action[1] as u64);
            let (target, cast_options) = map_cast_target_and_flags(action[2], action[3], coverage)?;
            Some(NativeAction {
                kind: NATIVE_ACTION_CAST,
                params: [action[1], 0, 0],
                target,
                cast_options,
                legacy_text: "",
                texts: Vec::new(),
                summon_entry: None,
                summon_location: None,
            })
        }
        ACTION_SET_PHASE if action[1] < 32 => Some(NativeAction {
            kind: NATIVE_ACTION_SET_PHASE,
            params: [action[1], 0, 0],
            target: NATIVE_TARGET_SELF,
            cast_options: 0,
            legacy_text: "",
            texts: Vec::new(),
            summon_entry: None,
            summon_location: None,
        }),
        ACTION_FLEE_FOR_ASSIST => Some(NativeAction {
            kind: NATIVE_ACTION_FLEE_FOR_ASSIST,
            params: [0; 3],
            target: NATIVE_TARGET_SELF,
            cast_options: 0,
            legacy_text: "",
            texts: Vec::new(),
            summon_entry: None,
            summon_location: None,
        }),
        ACTION_CALL_FOR_HELP => Some(NativeAction {
            kind: NATIVE_ACTION_CALL_FOR_HELP,
            params: [action[1], 0, 0],
            target: NATIVE_TARGET_SELF,
            cast_options: 0,
            legacy_text: "",
            texts: Vec::new(),
            summon_entry: None,
            summon_location: None,
        }),
        ACTION_RANGED_MOVEMENT => Some(NativeAction {
            kind: NATIVE_ACTION_SET_RANGED_POSTURE,
            params: [action[1], action[2], 0],
            target: NATIVE_TARGET_CURRENT,
            cast_options: 0,
            legacy_text: "",
            texts: Vec::new(),
            summon_entry: None,
            summon_location: None,
        }),
        ACTION_SUMMON_ID => {
            let entry = action[1] as u64;
            if !importable_templates.contains(&entry) {
                coverage.drop("missing_summon_creature", entry);
                return None;
            }
            if !summon_locations.contains_key(&action[3]) {
                coverage.drop("missing_summon_location", action[3] as u64);
                return None;
            }
            let target = map_target(action[2], coverage)?;
            Some(NativeAction {
                kind: NATIVE_ACTION_SUMMON,
                params: [action[1], action[2], action[3]],
                target,
                cast_options: 0,
                legacy_text: "",
                texts: Vec::new(),
                summon_entry: Some(entry),
                summon_location: Some(action[3]),
            })
        }
        ACTION_SET_PHASE => {
            coverage.drop("invalid_phase", action[1] as u64);
            None
        }
        value => {
            coverage.drop("unsupported_action", value as u64);
            None
        }
    }
}

fn map_cast_target_and_flags(
    raw_target: u32,
    raw_flags: u32,
    coverage: &mut Coverage,
) -> Option<(u8, u32)> {
    Coverage::source_value(&mut coverage.cast_flags, raw_flags as u64);
    if raw_flags & !SUPPORTED_CAST_FLAGS != 0 {
        coverage.drop("unsupported_cast_flag", raw_flags as u64);
        return None;
    }
    if raw_flags & CAST_TRIGGERED != 0 {
        coverage.drop("unsupported_triggered_cast", raw_flags as u64);
        return None;
    }
    if raw_flags & CAST_FORCE_CAST != 0 {
        coverage.drop("unsupported_force_cast", raw_flags as u64);
        return None;
    }
    let target = map_target(raw_target, coverage)?;
    let mut options = 0;
    if raw_flags & CAST_INTERRUPT_PREVIOUS != 0 {
        options |= NATIVE_CAST_INTERRUPT_PREVIOUS;
    }
    if raw_flags & CAST_AURA_NOT_PRESENT != 0 {
        options |= NATIVE_CAST_AURA_ABSENT;
    }
    if raw_flags & CAST_PLAYER_ONLY != 0 {
        options |= NATIVE_CAST_PLAYER_ONLY;
    }
    if raw_flags & CAST_TARGET_CASTING != 0 {
        options |= NATIVE_CAST_TARGET_CASTING;
    }
    Some((target, options))
}

fn map_target(value: u32, coverage: &mut Coverage) -> Option<u8> {
    Coverage::source_value(&mut coverage.target, value as u64);
    match value {
        TARGET_SELF => Some(NATIVE_TARGET_SELF),
        TARGET_HOSTILE => Some(NATIVE_TARGET_CURRENT),
        TARGET_HOSTILE_SECOND => Some(NATIVE_TARGET_SECOND_THREAT),
        TARGET_HOSTILE_RANDOM => Some(NATIVE_TARGET_RANDOM_THREAT),
        TARGET_ACTION_INVOKER => Some(NATIVE_TARGET_INVOKER),
        TARGET_HOSTILE_RANDOM_PLAYER => Some(NATIVE_TARGET_RANDOM_THREAT_PLAYER),
        TARGET_EVENT_SPECIFIC => Some(NATIVE_TARGET_EVENT),
        TARGET_NEAREST_AOE => Some(NATIVE_TARGET_NEAREST_AREA),
        TARGET_HOSTILE_FARTHEST => Some(NATIVE_TARGET_FARTHEST_HOSTILE),
        other => {
            coverage.drop("unsupported_target", other as u64);
            None
        }
    }
}

fn source_u32(value: &str) -> Option<u32> {
    value.parse().ok()
}

fn source_u64(value: &str) -> Option<u64> {
    value.parse().ok()
}

fn source_action_u32(value: &str) -> Option<u32> {
    value.parse::<u32>().ok().or_else(|| {
        value
            .parse::<i32>()
            .ok()
            .filter(|value| *value < 0)
            .map(|value| value as u32)
    })
}

fn fixture_id(id: u32) -> bool {
    (FIXTURE_ID_FIRST..=FIXTURE_ID_LAST).contains(&id)
}

fn supported_texts(
    ids: &[u32],
    broadcasts: &BTreeMap<u32, Broadcast>,
    coverage: &mut Coverage,
) -> bool {
    let mut valid = true;
    for id in ids {
        let Some(text) = broadcasts.get(id) else {
            continue;
        };
        if text.chat_type > 1 {
            coverage.drop("unsupported_chat_type", text.chat_type as u64);
            valid = false;
        }
    }
    valid && ids.iter().all(|id| broadcasts.contains_key(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `EVENTAI_*` out of the world-import verifier's supported-native-values manifest, as the set of
    /// numbers a `0|1|2`-style shell alternation lists.
    fn verifier_values(key: &str) -> Vec<u32> {
        verifier_manifest(key)
            .split('|')
            .map(|value| value.parse().expect("a numeric native value"))
            .collect()
    }

    fn verifier_mask(key: &str) -> u32 {
        verifier_manifest(key).parse().expect("a numeric mask")
    }

    fn verifier_manifest(key: &str) -> String {
        let script = include_str!("../scripts/import-world.sh");
        let manifest = script
            .split_once("# SUPPORTED NATIVE VALUES BEGIN")
            .and_then(|(_, rest)| rest.split_once("# SUPPORTED NATIVE VALUES END"))
            .expect("the verifier's supported-native-values manifest")
            .0;
        manifest
            .lines()
            .find_map(|line| line.trim().strip_prefix(&format!("{key}=")))
            .unwrap_or_else(|| panic!("{key} is missing from the verifier manifest"))
            .trim_matches('\'')
            .to_string()
    }

    /// The world import verifies the loaded catalogue with its own allowlist of native values, in
    /// shell. A value this importer emits that the verifier then reports as unsupported fails only
    /// at import time, against real data, long after the mapping was written.
    #[test]
    fn eventai_verifier_accepts_every_native_value_the_importer_emits() {
        for (key, emitted) in [
            (
                "EVENTAI_EVENT_TYPES",
                vec![
                    NATIVE_EVENT_ON_AGGRO,
                    NATIVE_EVENT_TIMED_IN_COMBAT,
                    NATIVE_EVENT_CREATURE_HP,
                    NATIVE_EVENT_ON_DEATH,
                    NATIVE_EVENT_TARGET_RANGE,
                    NATIVE_EVENT_ON_SPAWN,
                    NATIVE_EVENT_FRIENDLY_HP_DEFICIT,
                ],
            ),
            (
                "EVENTAI_ACTION_TYPES",
                vec![
                    NATIVE_ACTION_SAY,
                    NATIVE_ACTION_CAST,
                    NATIVE_ACTION_EMOTE,
                    NATIVE_ACTION_FLEE_FOR_ASSIST,
                    NATIVE_ACTION_CALL_FOR_HELP,
                    NATIVE_ACTION_SET_PHASE,
                    NATIVE_ACTION_SUMMON,
                    NATIVE_ACTION_SET_RANGED_POSTURE,
                ],
            ),
            (
                "EVENTAI_TARGET_POLICIES",
                vec![
                    NATIVE_TARGET_CURRENT,
                    NATIVE_TARGET_SELF,
                    NATIVE_TARGET_SECOND_THREAT,
                    NATIVE_TARGET_RANDOM_THREAT,
                    NATIVE_TARGET_INVOKER,
                    NATIVE_TARGET_EVENT,
                    NATIVE_TARGET_RANDOM_THREAT_PLAYER,
                    NATIVE_TARGET_NEAREST_AREA,
                    NATIVE_TARGET_FARTHEST_HOSTILE,
                ],
            ),
            (
                "EVENTAI_REPEAT_POLICIES",
                vec![NATIVE_REPEAT_ONCE, NATIVE_REPEAT],
            ),
        ] {
            let accepted = verifier_values(key);
            for value in emitted {
                assert!(
                    accepted.contains(&u32::from(value)),
                    "{key} rejects native value {value} the importer emits: {accepted:?}"
                );
            }
        }

        let source_flags = NATIVE_SOURCE_COMBAT_ACTION | NATIVE_SOURCE_RANDOM_ACTION;
        assert_eq!(
            source_flags & !verifier_mask("EVENTAI_SOURCE_FLAGS_MASK"),
            0,
            "the verifier rejects a source policy the importer emits"
        );
        let cast_options = NATIVE_CAST_INTERRUPT_PREVIOUS
            | NATIVE_CAST_AURA_ABSENT
            | NATIVE_CAST_PLAYER_ONLY
            | NATIVE_CAST_TARGET_CASTING;
        assert_eq!(
            cast_options & !verifier_mask("EVENTAI_CAST_OPTIONS_MASK"),
            0,
            "the verifier rejects a cast option the importer emits"
        );
        // The Module's supported cast options also carry triggered casts. This importer drops them,
        // but a reserved fixture may load one, and the verifier reads the durable table.
        const MODULE_CAST_TRIGGERED: u32 = 1 << 1;
        assert_eq!(
            verifier_mask("EVENTAI_CAST_OPTIONS_MASK") & MODULE_CAST_TRIGGERED,
            MODULE_CAST_TRIGGERED,
            "the verifier rejects a triggered cast the Module supports"
        );
    }
}
