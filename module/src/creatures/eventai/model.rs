use super::presentation::CreaturePresentationInstruction;
use super::CreatureAiState;
use crate::creatures::ai::TickScope;
use crate::quest::EventAiQuestCredit;

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
    pub allow_out_of_combat: bool,
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
    pub percent: bool,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct PercentageCondition {
    pub min_pct: u8,
    pub max_pct: u8,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct KillCondition {
    pub character_only: bool,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeathCondition {
    pub predicate: EventPredicate,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpellEventCondition {
    pub spell_id: u32,
    pub school_mask: u32,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutOfCombatSightCondition {
    pub require_non_hostile: bool,
    pub max_range_yd: u32,
    pub character_only: bool,
    pub predicate: EventPredicate,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpawnMapCondition {
    pub map_id: u32,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpawnZoneOrAreaCondition {
    pub zone_or_area_id: u32,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpawnCondition {
    Always,
    Map(SpawnMapCondition),
    ZoneOrArea(SpawnZoneOrAreaCondition),
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct FriendlyCrowdControlCondition {
    pub radius_yd: u32,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct FriendlyMissingAuraCondition {
    pub spell_id: u32,
    pub radius_yd: u32,
    pub selection: FriendlyAuraSelection,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum FriendlyAuraSelection {
    NearbyWhileEngaged,
    MatchActorCombatState,
    AnyWhileDisengaged,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct CreatureEntryCondition {
    pub creature_entry: u32,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReceiveEmoteCondition {
    pub emote_id: u32,
    pub predicate: EventPredicate,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuestTakenPredicate {
    pub quest_entry: u32,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventPredicate {
    Always,
    Alliance,
    Horde,
    QuestTaken(QuestTakenPredicate),
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuraStackCondition {
    pub spell_id: u32,
    pub stacks: u32,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum AiEventKind {
    JustDied,
    CriticalHealth,
    LostHealth,
    LostSomeHealth,
    GotFullHealth,
    CustomA,
    CustomB,
    CrowdControlled,
    CustomC,
    CustomD,
    CustomE,
    CustomF,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReceiveAiEventCondition {
    pub kind: AiEventKind,
    pub sender_entry: u32,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct FacingCondition {
    pub behind: bool,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventCondition {
    TimedInCombat(TimeWindow),
    TimedOutOfCombat(TimeWindow),
    CreatureHealth(CreatureHealthCondition),
    CreaturePower(PercentageCondition),
    OnAggro,
    OnKill(KillCondition),
    OnDeath(DeathCondition),
    OnEvade,
    OnSpellHit(SpellEventCondition),
    TargetRange(TargetRangeCondition),
    OutOfCombatSight(OutOfCombatSightCondition),
    OnSpawn(SpawnCondition),
    TargetHealth(PercentageCondition),
    TargetCasting,
    FriendlyHealthDeficit(FriendlyHealthDeficitCondition),
    FriendlyCrowdControlled(FriendlyCrowdControlCondition),
    FriendlyMissingAura(FriendlyMissingAuraCondition),
    OnSummoned(CreatureEntryCondition),
    TargetPower(PercentageCondition),
    OnReachedHome,
    OnReceiveEmote(ReceiveEmoteCondition),
    CreatureAura(AuraStackCondition),
    TargetAura(AuraStackCondition),
    OnSummonedDeath(CreatureEntryCondition),
    CreatureMissingAura(AuraStackCondition),
    TargetMissingAura(AuraStackCondition),
    TimedGeneric(TimeWindow),
    OnReceiveAiEvent(ReceiveAiEventCondition),
    SelectAttackingTarget(TargetRangeCondition),
    FacingTarget(FacingCondition),
    OnSpellHitTarget(SpellEventCondition),
    TargetNotReachable,
}

impl EventCondition {
    pub(crate) fn kind(self) -> EventKind {
        match self {
            Self::TimedInCombat(_) => EventKind::TimedInCombat,
            Self::TimedOutOfCombat(_) => EventKind::TimedOutOfCombat,
            Self::CreatureHealth(_) => EventKind::CreatureHp,
            Self::CreaturePower(_) => EventKind::CreaturePower,
            Self::OnAggro => EventKind::OnAggro,
            Self::OnKill(_) => EventKind::OnKill,
            Self::OnDeath(_) => EventKind::OnDeath,
            Self::OnEvade => EventKind::OnEvade,
            Self::OnSpellHit(_) => EventKind::OnSpellHit,
            Self::TargetRange(_) => EventKind::TargetRange,
            Self::OutOfCombatSight(_) => EventKind::OutOfCombatSight,
            Self::OnSpawn(_) => EventKind::OnSpawn,
            Self::TargetHealth(_) => EventKind::TargetHp,
            Self::TargetCasting => EventKind::TargetCasting,
            Self::FriendlyHealthDeficit(_) => EventKind::FriendlyHpDeficit,
            Self::FriendlyCrowdControlled(_) => EventKind::FriendlyCrowdControlled,
            Self::FriendlyMissingAura(_) => EventKind::FriendlyMissingAura,
            Self::OnSummoned(_) => EventKind::OnSummoned,
            Self::TargetPower(_) => EventKind::TargetPower,
            Self::OnReachedHome => EventKind::OnReachedHome,
            Self::OnReceiveEmote(_) => EventKind::OnReceiveEmote,
            Self::CreatureAura(_) => EventKind::CreatureAura,
            Self::TargetAura(_) => EventKind::TargetAura,
            Self::OnSummonedDeath(_) => EventKind::OnSummonedDeath,
            Self::CreatureMissingAura(_) => EventKind::CreatureMissingAura,
            Self::TargetMissingAura(_) => EventKind::TargetMissingAura,
            Self::TimedGeneric(_) => EventKind::TimedGeneric,
            Self::OnReceiveAiEvent(_) => EventKind::OnReceiveAiEvent,
            Self::SelectAttackingTarget(_) => EventKind::SelectAttackingTarget,
            Self::FacingTarget(_) => EventKind::FacingTarget,
            Self::OnSpellHitTarget(_) => EventKind::OnSpellHitTarget,
            Self::TargetNotReachable => EventKind::TargetNotReachable,
        }
    }

    pub(crate) fn runs_while_engaged(self) -> bool {
        match self {
            Self::TimedOutOfCombat(_) | Self::OutOfCombatSight(_) => false,
            Self::FriendlyMissingAura(condition) => {
                condition.selection != FriendlyAuraSelection::AnyWhileDisengaged
            }
            _ => self.kind().runs_in_cycle(),
        }
    }
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecurrencePolicy {
    Once,
    Repeat(TimeWindow),
    /// Every distinct edge may fire. There is no authored cooldown window.
    RepeatOnEvent,
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
pub enum PostureAdmission {
    Any,
    RangedOnly,
    MeleeOnly,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstructionTarget {
    CurrentOpponent,
    SelfActor,
    HighestThreat,
    SecondThreat,
    RandomThreat,
    RandomThreatExceptHighest,
    Invoker,
    Beneficiary,
    AiSender,
    Spawner,
    EventSubject,
    HighestThreatCharacter,
    RandomThreatCharacter,
    RandomThreatCharacterExceptHighest,
    NoExplicitSpellTarget,
    RandomHostileManaUser,
    EligibleCasterArea,
    FarthestHostile,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpellStartMode {
    Direct,
    Triggered,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SpellCasterAdmission {
    Living,
    DeadCreatureCallback,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpellCasterRole {
    Actor,
    Selected,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpellTargetRole {
    Selected,
    Actor,
    Caster,
    None,
    CasterArea,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SpellCastTarget {
    Unit(u64),
    None,
    CasterArea,
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
    pub start_mode: SpellStartMode,
    pub caster_role: SpellCasterRole,
    pub target_role: SpellTargetRole,
    pub aura_absent: bool,
    pub character_only: bool,
    pub target_must_be_casting: bool,
    pub main_spell: bool,
    pub distance_after_start: bool,
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
pub struct IncrementPhaseInstruction {
    pub amount: i32,
}

#[derive(spacetimedb::SpacetimeType, Clone, Debug, Eq, PartialEq)]
pub struct RandomPhaseInstruction {
    pub phases: Vec<u8>,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct RandomPhaseRangeInstruction {
    pub min_phase: u8,
    pub max_phase: u8,
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

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct SetLethalDamageFloorInstruction {
    pub enabled: bool,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdleMovementIntent {
    Stationary,
    RandomAroundCurrentPosition(RandomMovementIntent),
    Patrol(PatrolIntent),
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct RandomMovementIntent {
    pub radius_yd: u32,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct PatrolIntent {
    pub path_id: u32,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum WalkingMode {
    RunByDefault,
    WalkByDefault,
    RunWhileChasing,
    WalkWhileChasing,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum RangedMode {
    None,
    FullCaster,
    Proximity,
    NoMelee,
    Distancer,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum MovementOperation {
    ReplaceIdle(IdleMovementIntent),
    SetPatrolPaused(PatrolPause),
    SetCombatMovement(MovementSwitch),
    SetRangedMode(RangedModeInstruction),
    Face(u64),
    ResetFacing,
    SetWalking(WalkingMode),
    SetImmobilized(ImmobilizationInstruction),
    SetFollowMovement(MovementSwitch),
    Evade(EvadeInstruction),
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct PatrolPause {
    pub paused: bool,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct MovementSwitch {
    pub enabled: bool,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScaleSelectedThreatInstruction {
    pub percent: i32,
    pub target: InstructionTarget,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScaleAllThreatInstruction {
    pub percent: i32,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct RangedModeInstruction {
    pub mode: RangedMode,
    pub distance_yd: u32,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImmobilizationInstruction {
    pub enabled: bool,
    pub combat_only: bool,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct EvadeInstruction {
    pub combat_only: bool,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct FacingInstruction {
    pub target: InstructionTarget,
    pub reset: bool,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct NotifyEncounterInstruction {
    pub binding: crate::encounter::EncounterBinding,
    pub signal: crate::encounter::EncounterSignal,
}

#[derive(spacetimedb::SpacetimeType, Clone, Debug, Eq, PartialEq)]
pub struct StartRelayInstruction {
    pub relay_ids: Vec<u32>,
    pub target: InstructionTarget,
}

#[derive(spacetimedb::SpacetimeType, Clone, Debug, Eq, PartialEq)]
pub enum CreatureInstruction {
    Speak(SpeakInstruction),
    Cast(CastInstruction),
    Emote(EmoteInstruction),
    FleeForAssist,
    CallForHelp(CallForHelpInstruction),
    SetPhase(SetPhaseInstruction),
    IncrementPhase(IncrementPhaseInstruction),
    RandomPhase(RandomPhaseInstruction),
    RandomPhaseRange(RandomPhaseRangeInstruction),
    Summon(SummonInstruction),
    SetRangedPosture(RangedPostureInstruction),
    SetLethalDamageFloor(SetLethalDamageFloorInstruction),
    ForceDeath,
    ScaleSelectedThreat(ScaleSelectedThreatInstruction),
    ScaleAllThreat(ScaleAllThreatInstruction),
    Presentation(CreaturePresentationInstruction),
    QuestCredit(EventAiQuestCredit),
    Movement(MovementOperation),
    SetFacing(FacingInstruction),
    NotifyEncounter(NotifyEncounterInstruction),
    StartRelay(StartRelayInstruction),
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
    pub posture: PostureAdmission,
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
    TimedInCombat,
    TimedOutOfCombat,
    CreatureHp,
    CreaturePower,
    OnAggro,
    OnKill,
    OnDeath,
    OnEvade,
    OnSpellHit,
    TargetRange,
    OutOfCombatSight,
    OnSpawn,
    TargetHp,
    TargetCasting,
    FriendlyHpDeficit,
    FriendlyCrowdControlled,
    FriendlyMissingAura,
    OnSummoned,
    TargetPower,
    OnReachedHome,
    OnReceiveEmote,
    CreatureAura,
    TargetAura,
    OnSummonedDeath,
    CreatureMissingAura,
    TargetMissingAura,
    TimedGeneric,
    OnReceiveAiEvent,
    SelectAttackingTarget,
    FacingTarget,
    OnSpellHitTarget,
    TargetNotReachable,
}

impl EventKind {
    /// Whether a new firing of this event may wait behind the previous firing's repeat window.
    /// Aggro, death, evade, spawn, reached-home, and unreachable have no authored cooldown.
    /// `RepeatOnEvent` can still re-arm any of those rules for its next edge.
    pub(crate) fn supports_repeat_cooldown(self) -> bool {
        !matches!(
            self,
            Self::OnAggro
                | Self::OnDeath
                | Self::OnEvade
                | Self::OnSpawn
                | Self::OnReachedHome
                | Self::TargetNotReachable
        )
    }

    pub(crate) fn runs_in_cycle(self) -> bool {
        matches!(
            self,
            Self::TimedInCombat
                | Self::TimedOutOfCombat
                | Self::CreatureHp
                | Self::CreaturePower
                | Self::TargetRange
                | Self::OutOfCombatSight
                | Self::TargetHp
                | Self::TargetCasting
                | Self::FriendlyHpDeficit
                | Self::FriendlyCrowdControlled
                | Self::FriendlyMissingAura
                | Self::TargetPower
                | Self::CreatureAura
                | Self::TargetAura
                | Self::CreatureMissingAura
                | Self::TargetMissingAura
                | Self::TimedGeneric
                | Self::SelectAttackingTarget
                | Self::FacingTarget
        )
    }
}
pub(crate) enum EventAiRequest<'a> {
    Cycle {
        scope: &'a TickScope,
        active: &'a std::collections::HashSet<u64>,
    },
    Edge(EventContext),
}

/// One live melee fight the engaged pass evaluates: the creature whose rules run, and the victim
/// its contexts point at.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EngagedFight {
    pub creature_guid: u64,
    pub victim_guid: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CycleActor {
    pub creature_guid: u64,
    pub current_target_guid: Option<u64>,
    pub engaged: bool,
}

/// One unit as EventAI conditions and target selection read it, free of any table shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EventAiUnit {
    pub guid: u64,
    pub entry: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub map_id: u32,
    pub instance_id: u64,
    pub zone_id: u32,
    pub health: u32,
    pub max_health: u32,
    pub power: u32,
    pub max_power: u32,
    pub power_type: u8,
    pub level: u32,
    pub faction_template: u32,
    pub dead: bool,
    pub is_player: bool,
    pub orientation: f32,
    pub owner_guid: u64,
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
    pub invoker_is_player: Option<bool>,
    pub beneficiary_guid: Option<u64>,
    pub ai_sender_guid: Option<u64>,
    pub spawner_guid: Option<u64>,
    pub event_target_guid: Option<u64>,
    pub current_target_guid: Option<u64>,
    pub spell_id: Option<u32>,
    pub spell_school_mask: u32,
    pub emote_id: Option<u32>,
    pub creature_entry: Option<u32>,
    pub ai_event: Option<AiEventKind>,
    pub engaged: bool,
    pub assisted: bool,
    pub now_ms: u64,
}

impl EventContext {
    pub(crate) fn empty(kind: EventKind, creature_guid: u64, now_ms: u64) -> Self {
        Self {
            kind,
            creature_guid,
            invoker_guid: None,
            invoker_is_player: None,
            beneficiary_guid: None,
            ai_sender_guid: None,
            spawner_guid: None,
            event_target_guid: None,
            current_target_guid: None,
            spell_id: None,
            spell_school_mask: 0,
            emote_id: None,
            creature_entry: None,
            ai_event: None,
            engaged: false,
            assisted: false,
            now_ms,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RuleState {
    pub next_eligible_ms: u64,
    pub consumed: bool,
    pub lifecycle_id: u64,
    pub engagement_id: u64,
    pub invocation_seed: u64,
    pub invocation_started: bool,
    pub executing: bool,
    pub invocation_branch: u32,
    pub paused_at_ms: u64,
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
