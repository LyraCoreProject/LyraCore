use super::CreatureAiEvent;
use crate::creatures::ai::TickScope;

pub(crate) const EVENT_ON_AGGRO: u8 = 0;
pub(crate) const EVENT_TIMED_IN_COMBAT: u8 = 1;
pub(crate) const EVENT_CREATURE_HP: u8 = 2;
pub(crate) const EVENT_ON_DEATH: u8 = 3;
pub(crate) const EVENT_TARGET_RANGE: u8 = 4;
pub(crate) const EVENT_ON_SPAWN: u8 = 5;
pub(crate) const EVENT_FRIENDLY_HP_DEFICIT: u8 = 6;

pub(crate) const ACTION_SAY: u8 = 0;
pub(crate) const ACTION_YELL: u8 = 1;
pub(crate) const ACTION_CAST: u8 = 2;
pub(crate) const ACTION_EMOTE: u8 = 3;
pub(crate) const ACTION_FLEE_FOR_ASSIST: u8 = 4;
pub(crate) const ACTION_CALL_FOR_HELP: u8 = 5;
pub(crate) const ACTION_SET_PHASE: u8 = 6;
pub(crate) const ACTION_SUMMON: u8 = 7;
pub(crate) const ACTION_SET_RANGED_POSTURE: u8 = 8;

pub(crate) const REPEAT_ONCE: u8 = 0;
pub(crate) const REPEAT: u8 = 1;

pub(crate) const TARGET_CURRENT: u8 = 0;
pub(crate) const TARGET_SELF: u8 = 1;
pub(crate) const TARGET_TOP_THREAT: u8 = 2;
pub(crate) const TARGET_SECOND_THREAT: u8 = 3;
pub(crate) const TARGET_RANDOM_THREAT: u8 = 4;
pub(crate) const TARGET_INVOKER: u8 = 5;
pub(crate) const TARGET_EVENT: u8 = 6;
pub(crate) const TARGET_TOP_THREAT_PLAYER: u8 = 7;
pub(crate) const TARGET_RANDOM_THREAT_PLAYER: u8 = 8;
pub(crate) const TARGET_NEAREST_AREA: u8 = 9;
pub(crate) const TARGET_FARTHEST_HOSTILE: u8 = 10;

pub(crate) const SOURCE_FLAG_COMBAT_ACTION: u32 = 1 << 0;
pub(crate) const SOURCE_FLAG_RANDOM_ACTION: u32 = 1 << 1;
const SUPPORTED_SOURCE_FLAGS: u32 = SOURCE_FLAG_COMBAT_ACTION | SOURCE_FLAG_RANDOM_ACTION;

pub(crate) const CAST_INTERRUPT_PREVIOUS: u32 = 1 << 0;
pub(crate) const CAST_TRIGGERED: u32 = 1 << 1;
pub(crate) const CAST_AURA_ABSENT: u32 = 1 << 2;
pub(crate) const CAST_PLAYER_ONLY: u32 = 1 << 3;
pub(crate) const CAST_TARGET_CASTING: u32 = 1 << 4;
const SUPPORTED_CAST_OPTIONS: u32 = CAST_INTERRUPT_PREVIOUS
    | CAST_TRIGGERED
    | CAST_AURA_ABSENT
    | CAST_PLAYER_ONLY
    | CAST_TARGET_CASTING;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EventKind {
    OnAggro,
    TimedInCombat,
    CreatureHp,
    OnDeath,
    TargetRange,
    OnSpawn,
    FriendlyHpDeficit,
}

impl EventKind {
    fn decode(value: u8) -> Option<Self> {
        match value {
            EVENT_ON_AGGRO => Some(Self::OnAggro),
            EVENT_TIMED_IN_COMBAT => Some(Self::TimedInCombat),
            EVENT_CREATURE_HP => Some(Self::CreatureHp),
            EVENT_ON_DEATH => Some(Self::OnDeath),
            EVENT_TARGET_RANGE => Some(Self::TargetRange),
            EVENT_ON_SPAWN => Some(Self::OnSpawn),
            EVENT_FRIENDLY_HP_DEFICIT => Some(Self::FriendlyHpDeficit),
            _ => None,
        }
    }

    /// An engaged creature re-evaluates these kinds on every cycle firing, so a rule keyed on one
    /// can carry a repeat window. The edges (aggro, spawn, death) fire once per engagement or
    /// lifecycle: a window stamped on one of them would never be reached again.
    pub(crate) fn recurs(self) -> bool {
        !matches!(self, Self::OnAggro | Self::OnSpawn | Self::OnDeath)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ActionKind {
    Say,
    Yell,
    Cast,
    Emote,
    FleeForAssist,
    CallForHelp,
    SetPhase,
    Summon,
    SetRangedPosture,
}

impl ActionKind {
    fn decode(value: u8) -> Option<Self> {
        match value {
            ACTION_SAY => Some(Self::Say),
            ACTION_YELL => Some(Self::Yell),
            ACTION_CAST => Some(Self::Cast),
            ACTION_EMOTE => Some(Self::Emote),
            ACTION_FLEE_FOR_ASSIST => Some(Self::FleeForAssist),
            ACTION_CALL_FOR_HELP => Some(Self::CallForHelp),
            ACTION_SET_PHASE => Some(Self::SetPhase),
            ACTION_SUMMON => Some(Self::Summon),
            ACTION_SET_RANGED_POSTURE => Some(Self::SetRangedPosture),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TargetPolicy {
    Current,
    SelfActor,
    TopThreat,
    SecondThreat,
    RandomThreat,
    Invoker,
    EventTarget,
    TopThreatPlayer,
    RandomThreatPlayer,
    NearestArea,
    FarthestHostile,
}

impl TargetPolicy {
    fn decode(value: u8) -> Option<Self> {
        match value {
            TARGET_CURRENT => Some(Self::Current),
            TARGET_SELF => Some(Self::SelfActor),
            TARGET_TOP_THREAT => Some(Self::TopThreat),
            TARGET_SECOND_THREAT => Some(Self::SecondThreat),
            TARGET_RANDOM_THREAT => Some(Self::RandomThreat),
            TARGET_INVOKER => Some(Self::Invoker),
            TARGET_EVENT => Some(Self::EventTarget),
            TARGET_TOP_THREAT_PLAYER => Some(Self::TopThreatPlayer),
            TARGET_RANDOM_THREAT_PLAYER => Some(Self::RandomThreatPlayer),
            TARGET_NEAREST_AREA => Some(Self::NearestArea),
            TARGET_FARTHEST_HOSTILE => Some(Self::FarthestHostile),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CastOptions(u32);

impl CastOptions {
    pub(crate) fn contains(self, bit: u32) -> bool {
        self.0 & bit != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SourcePolicy(u32);

impl SourcePolicy {
    pub(crate) fn contains(self, bit: u32) -> bool {
        self.0 & bit != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RepeatPolicy {
    Once,
    Repeat,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Subject {
    Entry(u32),
    Guid(u64),
}

pub(crate) enum EventAiRequest<'a> {
    Engaged(&'a TickScope),
    Edge(EventContext),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EventContext {
    pub kind: EventKind,
    pub creature_guid: u64,
    pub invoker_guid: Option<u64>,
    pub event_target_guid: Option<u64>,
    pub current_target_guid: Option<u64>,
    pub assisted: bool,
    pub now_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RuleAction {
    pub row_id: u64,
    pub order: u8,
    pub kind: ActionKind,
    pub params: [u32; 3],
    pub target: TargetPolicy,
    pub cast_options: CastOptions,
    pub legacy_text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Rule {
    pub id: u64,
    pub subject: Subject,
    pub event: EventKind,
    pub chance_pct: u8,
    pub allowed_phase_mask: u32,
    pub source_policy: SourcePolicy,
    pub repeat: RepeatPolicy,
    pub event_params: [u32; 6],
    pub actions: Vec<RuleAction>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RuleState {
    pub next_eligible_ms: u64,
    pub consumed: bool,
    pub lifecycle_id: u64,
    pub engagement_id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CreatureState {
    pub phase: u8,
    pub lifecycle_id: u64,
    pub engagement_id: u64,
    pub ranged_distance: f32,
    pub ranged_angle: f32,
    pub ranged_posture_active: bool,
}

impl Default for CreatureState {
    fn default() -> Self {
        Self {
            phase: 0,
            lifecycle_id: 1,
            engagement_id: 1,
            ranged_distance: 0.0,
            ranged_angle: 0.0,
            ranged_posture_active: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DiagnosticKind {
    UnknownEvent,
    UnknownAction,
    UnknownTarget,
    UnknownRepeatPolicy,
    InvalidSubject,
    InvalidChance,
    EmptyPhaseMask,
    UnsupportedSourceFlags,
    UnsupportedCastOptions,
    InvalidWindow,
    InconsistentRule,
    DuplicateActionOrder,
    UnsupportedAction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Diagnostic {
    pub rule_id: u64,
    pub row_id: u64,
    pub kind: DiagnosticKind,
    pub value: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ActionResult {
    Applied,
    Refused,
    Unsupported,
}

impl Rule {
    pub(crate) fn decode(mut rows: Vec<CreatureAiEvent>) -> Result<Self, Diagnostic> {
        rows.sort_unstable_by_key(|row| (row.action_order, row.id));
        let first = rows
            .first()
            .expect("a rule group contains at least one row");
        let legacy = first.source_rule_id == 0;
        let id = effective_rule_id(first);
        let subject = decode_subject(first, id)?;
        let event = EventKind::decode(first.event_type).ok_or(Diagnostic {
            rule_id: id,
            row_id: first.id,
            kind: DiagnosticKind::UnknownEvent,
            value: first.event_type as u64,
        })?;
        let chance_pct = if legacy && first.chance_pct == 0 {
            100
        } else {
            first.chance_pct
        };
        if !(1..=100).contains(&chance_pct) {
            return Err(Diagnostic {
                rule_id: id,
                row_id: first.id,
                kind: DiagnosticKind::InvalidChance,
                value: chance_pct as u64,
            });
        }
        let allowed_phase_mask = if legacy && first.allowed_phase_mask == 0 {
            u32::MAX
        } else {
            first.allowed_phase_mask
        };
        if allowed_phase_mask == 0 {
            return Err(Diagnostic {
                rule_id: id,
                row_id: first.id,
                kind: DiagnosticKind::EmptyPhaseMask,
                value: 0,
            });
        }
        if first.source_flags & !SUPPORTED_SOURCE_FLAGS != 0 {
            return Err(Diagnostic {
                rule_id: id,
                row_id: first.id,
                kind: DiagnosticKind::UnsupportedSourceFlags,
                value: first.source_flags as u64,
            });
        }
        let repeat = if legacy {
            if event == EventKind::TimedInCombat {
                RepeatPolicy::Repeat
            } else {
                RepeatPolicy::Once
            }
        } else {
            match first.repeat_policy {
                REPEAT_ONCE => RepeatPolicy::Once,
                REPEAT => RepeatPolicy::Repeat,
                value => {
                    return Err(Diagnostic {
                        rule_id: id,
                        row_id: first.id,
                        kind: DiagnosticKind::UnknownRepeatPolicy,
                        value: value as u64,
                    });
                }
            }
        };
        let decoded_event_params = if legacy {
            [
                first.initial_min_ms,
                first.initial_max_ms,
                first.repeat_min_ms,
                first.repeat_max_ms,
                0,
                0,
            ]
        } else {
            event_params(first)
        };
        // A timer rule waits its initial window before its first firing; every recurring kind
        // carries its repeat window in params 3 and 4. An inverted window is a bad row, not a
        // fixed cadence: left to the roller it wraps the delay to weeks, so it surfaces here.
        let inverted_initial =
            event == EventKind::TimedInCombat && decoded_event_params[0] > decoded_event_params[1];
        let inverted_repeat = event.recurs() && decoded_event_params[2] > decoded_event_params[3];
        if inverted_initial || inverted_repeat {
            return Err(Diagnostic {
                rule_id: id,
                row_id: first.id,
                kind: DiagnosticKind::InvalidWindow,
                value: 0,
            });
        }

        for row in rows.iter().skip(1) {
            if effective_rule_id(row) != id
                || decode_subject(row, id).ok() != Some(subject)
                || EventKind::decode(row.event_type) != Some(event)
                || row.chance_pct != first.chance_pct
                || row.allowed_phase_mask != first.allowed_phase_mask
                || row.source_flags != first.source_flags
                || row.repeat_policy != first.repeat_policy
                || event_params(row) != event_params(first)
            {
                return Err(Diagnostic {
                    rule_id: id,
                    row_id: row.id,
                    kind: DiagnosticKind::InconsistentRule,
                    value: 0,
                });
            }
        }
        if let Some(pair) = rows
            .windows(2)
            .find(|pair| pair[0].action_order == pair[1].action_order)
        {
            return Err(Diagnostic {
                rule_id: id,
                row_id: pair[1].id,
                kind: DiagnosticKind::DuplicateActionOrder,
                value: pair[1].action_order as u64,
            });
        }

        let source_flags = first.source_flags;
        let mut actions = Vec::with_capacity(rows.len());
        for row in rows {
            let kind = ActionKind::decode(row.action_type).ok_or(Diagnostic {
                rule_id: id,
                row_id: row.id,
                kind: DiagnosticKind::UnknownAction,
                value: row.action_type as u64,
            })?;
            let target = TargetPolicy::decode(row.target_policy).ok_or(Diagnostic {
                rule_id: id,
                row_id: row.id,
                kind: DiagnosticKind::UnknownTarget,
                value: row.target_policy as u64,
            })?;
            if row.cast_options & !SUPPORTED_CAST_OPTIONS != 0 {
                return Err(Diagnostic {
                    rule_id: id,
                    row_id: row.id,
                    kind: DiagnosticKind::UnsupportedCastOptions,
                    value: row.cast_options as u64,
                });
            }
            let mut params = [row.action_param_1, row.action_param_2, row.action_param_3];
            if legacy && kind == ActionKind::Cast {
                params[0] = row.spell_id;
            }
            actions.push(RuleAction {
                row_id: row.id,
                order: row.action_order,
                kind,
                params,
                target,
                cast_options: CastOptions(row.cast_options),
                legacy_text: row.text,
            });
        }
        Ok(Self {
            id,
            subject,
            event,
            chance_pct,
            allowed_phase_mask,
            source_policy: SourcePolicy(source_flags),
            repeat,
            event_params: decoded_event_params,
            actions,
        })
    }
}

pub(crate) fn effective_rule_id(row: &CreatureAiEvent) -> u64 {
    if row.source_rule_id == 0 {
        row.id
    } else {
        row.source_rule_id
    }
}

fn decode_subject(row: &CreatureAiEvent, rule_id: u64) -> Result<Subject, Diagnostic> {
    match (row.creature_entry, row.creature_guid) {
        (entry, 0) if entry != 0 => Ok(Subject::Entry(entry)),
        (0, guid) if guid != 0 => Ok(Subject::Guid(guid)),
        _ => Err(Diagnostic {
            rule_id,
            row_id: row.id,
            kind: DiagnosticKind::InvalidSubject,
            value: row.creature_guid,
        }),
    }
}

fn event_params(row: &CreatureAiEvent) -> [u32; 6] {
    [
        row.event_param_1,
        row.event_param_2,
        row.event_param_3,
        row.event_param_4,
        row.event_param_5,
        row.event_param_6,
    ]
}
