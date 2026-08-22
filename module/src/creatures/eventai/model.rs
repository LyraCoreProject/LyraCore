use super::CreatureAiState;
use crate::creatures::ai::TickScope;

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventAiSubject {
    Entry(u32),
    Guid(u64),
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DefinitionRevision {
    pub value: u64,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhaseSet {
    pub bits: u32,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimeWindow {
    pub min_ms: u32,
    pub max_ms: u32,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct CreatureHealthCondition {
    pub min_pct: u8,
    pub max_pct: u8,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct TargetRangeCondition {
    pub min_yd: u32,
    pub max_yd: u32,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct FriendlyHealthDeficitCondition {
    pub missing_health: u32,
    pub radius_yd: u32,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventCondition {
    OnAggro,
    TimedInCombat(TimeWindow),
    CreatureHealth(CreatureHealthCondition),
    OnDeath,
    TargetRange(TargetRangeCondition),
    OnSpawn,
    FriendlyHealthDeficit(FriendlyHealthDeficitCondition),
}

impl EventCondition {
    pub(crate) fn kind(self) -> EventKind {
        match self {
            Self::OnAggro => EventKind::OnAggro,
            Self::TimedInCombat(_) => EventKind::TimedInCombat,
            Self::CreatureHealth(_) => EventKind::CreatureHp,
            Self::OnDeath => EventKind::OnDeath,
            Self::TargetRange(_) => EventKind::TargetRange,
            Self::OnSpawn => EventKind::OnSpawn,
            Self::FriendlyHealthDeficit(_) => EventKind::FriendlyHpDeficit,
        }
    }
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecurrencePolicy {
    Once,
    Repeat(TimeWindow),
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstructionSelection {
    All,
    RandomOne,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionPolicy {
    Ordinary,
    CombatAction,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstructionTarget {
    CurrentOpponent,
    SelfActor,
    HighestThreat,
    SecondThreat,
    RandomThreat,
    Invoker,
    EventSubject,
    HighestThreatCharacter,
    RandomThreatCharacter,
    EligibleCasterArea,
    FarthestHostile,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpeechMode {
    Say,
    Yell,
}

#[derive(spacetimedb::SpacetimeType, Clone, Debug, Eq, PartialEq)]
pub struct SpeakInstruction {
    pub mode: SpeechMode,
    pub broadcast_ids: Vec<u32>,
    pub legacy_text: String,
    pub target: InstructionTarget,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct CastInstruction {
    pub spell_id: u32,
    pub target: InstructionTarget,
    pub interrupt_previous: bool,
    pub triggered: bool,
    pub aura_absent: bool,
    pub character_only: bool,
    pub target_must_be_casting: bool,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct EmoteInstruction {
    pub emote_id: u32,
    pub target: InstructionTarget,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct CallForHelpInstruction {
    pub radius_yd: u32,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct SetPhaseInstruction {
    pub phase: u8,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct SummonInstruction {
    pub creature_entry: u32,
    pub summon_location_id: u32,
    pub target: InstructionTarget,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct RangedPostureInstruction {
    pub distance_yd: u32,
    pub angle_degrees: i32,
}

#[derive(spacetimedb::SpacetimeType, Clone, Debug, Eq, PartialEq)]
pub enum CreatureInstruction {
    Speak(SpeakInstruction),
    Cast(CastInstruction),
    Emote(EmoteInstruction),
    FleeForAssist,
    CallForHelp(CallForHelpInstruction),
    SetPhase(SetPhaseInstruction),
    Summon(SummonInstruction),
    SetRangedPosture(RangedPostureInstruction),
}

#[derive(spacetimedb::SpacetimeType, Clone, Debug, Eq, PartialEq)]
pub struct EventAiRule {
    pub source_rule_id: u64,
    pub event: EventCondition,
    pub chance_pct: u8,
    pub allowed_phases: PhaseSet,
    pub recurrence: RecurrencePolicy,
    pub selection: InstructionSelection,
    pub execution: ExecutionPolicy,
    pub instructions: Vec<CreatureInstruction>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EventAiDefinition {
    pub subject: EventAiSubject,
    pub revision: DefinitionRevision,
    pub rules: Vec<EventAiRule>,
}

impl EventAiDefinition {
    pub(crate) fn empty(creature_guid: u64) -> Self {
        Self {
            subject: EventAiSubject::Guid(creature_guid),
            revision: DefinitionRevision::default(),
            rules: Vec::new(),
        }
    }

    pub(crate) fn compose(creature_guid: u64, mut definitions: Vec<Self>) -> Self {
        definitions.sort_unstable_by_key(|definition| match definition.subject {
            EventAiSubject::Entry(entry) => (0, u64::from(entry)),
            EventAiSubject::Guid(guid) => (1, guid),
        });
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"lyracore-eventai-composition-v1");
        let mut rules = Vec::new();
        for definition in definitions {
            match definition.subject {
                EventAiSubject::Entry(entry) => {
                    hasher.update(&[0]);
                    hasher.update(&entry.to_le_bytes());
                }
                EventAiSubject::Guid(guid) => {
                    hasher.update(&[1]);
                    hasher.update(&guid.to_le_bytes());
                }
            }
            hasher.update(&definition.revision.value.to_le_bytes());
            rules.extend(definition.rules);
        }
        if rules.is_empty() {
            return Self::empty(creature_guid);
        }
        rules.sort_by_key(|rule| rule.source_rule_id);
        let revision = u64::from_le_bytes(
            hasher.finalize().as_bytes()[..8]
                .try_into()
                .expect("a BLAKE3 digest has at least eight bytes"),
        );
        Self {
            subject: EventAiSubject::Guid(creature_guid),
            revision: DefinitionRevision { value: revision },
            rules,
        }
    }
}

pub(crate) fn normalized_revision(
    subject: EventAiSubject,
    rules: &[EventAiRule],
) -> DefinitionRevision {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"lyracore-eventai-native-definition-v1");
    hasher.update(format!("{subject:?}:{rules:?}").as_bytes());
    DefinitionRevision {
        value: u64::from_le_bytes(
            hasher.finalize().as_bytes()[..8]
                .try_into()
                .expect("a BLAKE3 digest has at least eight bytes"),
        ),
    }
}

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
    /// An engaged creature re-evaluates these kinds on every cycle firing, so a rule keyed on one
    /// can carry a repeat window. The edges (aggro, spawn, death) fire once per engagement or
    /// lifecycle: a window stamped on one of them would never be reached again.
    pub(crate) fn recurs(self) -> bool {
        !matches!(self, Self::OnAggro | Self::OnSpawn | Self::OnDeath)
    }
}
pub(crate) enum EventAiRequest<'a> {
    Engaged(&'a TickScope),
    Edge(EventContext),
}

/// One live melee fight the engaged pass evaluates: the creature whose rules run, and the victim
/// its contexts point at.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EngagedFight {
    pub creature_guid: u64,
    pub victim_guid: u64,
}

/// One unit as EventAI conditions and target selection read it, free of any table shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EventAiUnit {
    pub guid: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub map_id: u32,
    pub instance_id: u64,
    pub health: u32,
    pub max_health: u32,
    pub level: u32,
    pub faction_template: u32,
    pub dead: bool,
    pub is_player: bool,
}

/// One imported broadcast text as speech reads it: the line, the chat type it carries, and the
/// animation emote that travels with it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BroadcastLine {
    pub text: String,
    pub chat_type: u8,
    pub language: u8,
    pub emote: u32,
}

/// One authored summon location, with the summon's out-of-combat lifetime.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SummonLocation {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
    pub lifetime_ms: u32,
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
    pub definition_revision: DefinitionRevision,
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
            definition_revision: DefinitionRevision::default(),
        }
    }
}

impl From<CreatureAiState> for CreatureState {
    fn from(row: CreatureAiState) -> Self {
        Self {
            phase: row.phase,
            lifecycle_id: row.lifecycle_id,
            engagement_id: row.engagement_id,
            ranged_distance: row.ranged_distance,
            ranged_angle: row.ranged_angle,
            ranged_posture_active: row.ranged_posture_active,
            definition_revision: DefinitionRevision {
                value: row.definition_revision,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ActionResult {
    Applied,
    Refused,
    Unsupported,
}
