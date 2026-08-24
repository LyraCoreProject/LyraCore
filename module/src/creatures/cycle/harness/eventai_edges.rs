//! Spawn and death EventAI scenarios.
use super::*;
use crate::creatures::eventai::*;
use crate::creatures::presentation::NpcFlagsProjection;

const CREATURE: u64 = 8_101;
const TARGET: u64 = 8_102;
const ENTRY: u32 = 911;

fn point(x: f32) -> Point {
    Point { x, y: 0.0, z: 0.0 }
}

fn scenario() -> Scenario {
    Scenario::new(1_000_000)
        .creature(CREATURE, point(0.0))
        .entry(CREATURE, ENTRY)
        .player(TARGET, point(2.0))
        .at_war(BEASTS, ALLIANCE)
}

#[expect(
    clippy::too_many_arguments,
    reason = "edge test rows keep each authored value at the call site"
)]
fn row(
    row_id: u64,
    rule_id: u64,
    order: u8,
    event_type: u8,
    action_type: u8,
    chance_pct: u8,
    phases: u32,
    repeat_policy: u8,
    action_params: [u32; 3],
) -> CreatureAiEvent {
    CreatureAiEvent {
        id: row_id,
        creature_entry: ENTRY,
        event_type,
        action_type,
        text: String::new(),
        spell_id: 0,
        initial_min_ms: 0,
        initial_max_ms: 0,
        repeat_min_ms: 0,
        repeat_max_ms: 0,
        source_rule_id: rule_id,
        action_order: order,
        creature_guid: 0,
        chance_pct,
        allowed_phase_mask: phases,
        source_flags: 0,
        repeat_policy,
        event_param_1: 0,
        event_param_2: 0,
        event_param_3: 0,
        event_param_4: 0,
        event_param_5: 0,
        event_param_6: 0,
        action_param_1: action_params[0],
        action_param_2: action_params[1],
        action_param_3: action_params[2],
        target_policy: TARGET_CURRENT,
        cast_options: 0,
    }
}

fn edge(scenario: &mut Scenario, kind: EventKind, assisted: bool) {
    evaluate(
        scenario,
        EventAiRequest::Edge(EventContext {
            invoker_guid: Some(TARGET),
            beneficiary_guid: Some(TARGET),
            current_target_guid: Some(TARGET),
            assisted,
            engaged: true,
            ..EventContext::empty(kind, CREATURE, 1_000)
        }),
    );
}

fn reset_lifecycle(scenario: &Scenario) {
    scenario.eventai_rule_state.borrow_mut().remove(&CREATURE);
    scenario
        .eventai_creature_state
        .borrow_mut()
        .remove(&CREATURE);
}

#[test]
fn direct_aggro_speaks_once_and_assisted_aggro_keeps_casts() {
    let mut direct = scenario()
        .eventai_broadcast(1, "direct", 0)
        .eventai_row(row(
            1,
            1,
            0,
            EVENT_ON_AGGRO,
            ACTION_SAY,
            100,
            1,
            REPEAT,
            [1, 0, 0],
        ));

    edge(&mut direct, EventKind::OnAggro, false);
    edge(&mut direct, EventKind::OnAggro, false);

    assert_eq!(
        direct.eventai_speech(),
        vec![(CREATURE, 0, "direct".to_string())]
    );

    let mut assisted = scenario()
        .eventai_broadcast(2, "quiet", 0)
        .eventai_row(row(
            2,
            2,
            0,
            EVENT_ON_AGGRO,
            ACTION_SAY,
            100,
            1,
            REPEAT_ONCE,
            [2, 0, 0],
        ))
        .eventai_row(row(
            3,
            3,
            0,
            EVENT_ON_AGGRO,
            ACTION_CAST,
            100,
            1,
            REPEAT_ONCE,
            [3, 0, 0],
        ));

    edge(&mut assisted, EventKind::OnAggro, true);

    assert!(assisted.eventai_speech().is_empty());
    assert_eq!(assisted.casts(), vec![(CREATURE, 3, TARGET)]);
}

#[test]
fn on_death_direct_and_triggered_self_casts_carry_dead_creature_admission() {
    let cast = |spell_id, start_mode| {
        CreatureInstruction::Cast(CastInstruction {
            spell_id,
            target: InstructionTarget::SelfActor,
            interrupt_previous: false,
            start_mode,
            caster_role: SpellCasterRole::Actor,
            target_role: SpellTargetRole::Selected,
            aura_absent: false,
            character_only: false,
            target_must_be_casting: false,
            main_spell: false,
            distance_after_start: false,
        })
    };
    let rule = EventAiRule {
        source_rule_id: 22_5202,
        event: EventCondition::OnDeath(DeathCondition {
            predicate: EventPredicate::Always,
        }),
        chance_pct: 100,
        allowed_phases: PhaseSet { bits: u32::MAX },
        recurrence: RecurrencePolicy::Once,
        selection: InstructionSelection::All,
        execution: ExecutionPolicy::Ordinary,
        posture: PostureAdmission::Any,
        instructions: vec![
            cast(9_144, SpellStartMode::Direct),
            cast(9_145, SpellStartMode::Triggered),
        ],
    };
    let definition = EventAiDefinition {
        subject: EventAiSubject::Entry(ENTRY),
        revision: normalized_revision(EventAiSubject::Entry(ENTRY), std::slice::from_ref(&rule)),
        rules: vec![rule],
    };
    let mut scenario = scenario()
        .slain(CREATURE)
        .eventai_native_definition(definition);

    evaluate(
        &mut scenario,
        EventAiRequest::Edge(EventContext {
            invoker_guid: Some(TARGET),
            invoker_is_player: Some(true),
            beneficiary_guid: Some(TARGET),
            current_target_guid: Some(TARGET),
            ..EventContext::empty(EventKind::OnDeath, CREATURE, 1_000)
        }),
    );

    assert_eq!(
        scenario.eventai_spell_starts.borrow().as_slice(),
        &[
            (
                SpellStartMode::Direct,
                SpellCastTarget::Unit(CREATURE),
                false,
                SpellCasterAdmission::DeadCreatureCallback,
            ),
            (
                SpellStartMode::Triggered,
                SpellCastTarget::Unit(CREATURE),
                false,
                SpellCasterAdmission::DeadCreatureCallback,
            ),
        ]
    );
}

#[test]
fn spell_hit_callback_can_cast_after_the_creature_dies_from_the_hit() {
    let cast = |spell_id, start_mode, target| {
        CreatureInstruction::Cast(CastInstruction {
            spell_id,
            target,
            interrupt_previous: false,
            start_mode,
            caster_role: SpellCasterRole::Actor,
            target_role: match target {
                InstructionTarget::NoExplicitSpellTarget => SpellTargetRole::None,
                _ => SpellTargetRole::Selected,
            },
            aura_absent: false,
            character_only: false,
            target_must_be_casting: false,
            main_spell: false,
            distance_after_start: false,
        })
    };
    let rule = EventAiRule {
        source_rule_id: 113_3802,
        event: EventCondition::OnSpellHit(SpellEventCondition {
            spell_id: 0,
            school_mask: 0,
        }),
        chance_pct: 100,
        allowed_phases: PhaseSet { bits: u32::MAX },
        recurrence: RecurrencePolicy::RepeatOnEvent,
        selection: InstructionSelection::All,
        execution: ExecutionPolicy::Ordinary,
        posture: PostureAdmission::Any,
        instructions: vec![
            cast(
                22_947,
                SpellStartMode::Direct,
                InstructionTarget::NoExplicitSpellTarget,
            ),
            cast(
                22_948,
                SpellStartMode::Triggered,
                InstructionTarget::SelfActor,
            ),
        ],
    };
    let definition = EventAiDefinition {
        subject: EventAiSubject::Entry(ENTRY),
        revision: normalized_revision(EventAiSubject::Entry(ENTRY), std::slice::from_ref(&rule)),
        rules: vec![rule],
    };
    let mut scenario = scenario()
        .slain(CREATURE)
        .eventai_native_definition(definition);

    evaluate(
        &mut scenario,
        EventAiRequest::Edge(EventContext {
            invoker_guid: Some(TARGET),
            invoker_is_player: Some(true),
            spell_id: Some(1),
            ..EventContext::empty(EventKind::OnSpellHit, CREATURE, 1_000)
        }),
    );

    assert_eq!(
        scenario.eventai_spell_starts.borrow().as_slice(),
        &[
            (
                SpellStartMode::Direct,
                SpellCastTarget::None,
                false,
                SpellCasterAdmission::DeadCreatureCallback,
            ),
            (
                SpellStartMode::Triggered,
                SpellCastTarget::Unit(CREATURE),
                false,
                SpellCasterAdmission::DeadCreatureCallback,
            ),
        ]
    );
}

#[test]
fn on_kill_can_check_a_deleted_non_character_invoker_snapshot() {
    let rule = EventAiRule {
        source_rule_id: 22_5203,
        event: EventCondition::OnKill(KillCondition {
            character_only: false,
        }),
        chance_pct: 100,
        allowed_phases: PhaseSet { bits: u32::MAX },
        recurrence: RecurrencePolicy::RepeatOnEvent,
        selection: InstructionSelection::All,
        execution: ExecutionPolicy::Ordinary,
        posture: PostureAdmission::Any,
        instructions: vec![CreatureInstruction::Speak(SpeakInstruction {
            mode: SpeechMode::Say,
            broadcast_ids: vec![51],
            legacy_text: String::new(),
            target: InstructionTarget::SelfActor,
        })],
    };
    let definition = EventAiDefinition {
        subject: EventAiSubject::Entry(ENTRY),
        revision: normalized_revision(EventAiSubject::Entry(ENTRY), std::slice::from_ref(&rule)),
        rules: vec![rule],
    };
    let deleted_pet = CREATURE + 80;
    let mut scenario = scenario()
        .eventai_broadcast(51, "kill", 0)
        .eventai_native_definition(definition);

    evaluate(
        &mut scenario,
        EventAiRequest::Edge(EventContext {
            invoker_guid: Some(deleted_pet),
            invoker_is_player: Some(false),
            engaged: true,
            ..EventContext::empty(EventKind::OnKill, CREATURE, 1_000)
        }),
    );

    assert_eq!(
        scenario.eventai_speech(),
        vec![(CREATURE, 0, "kill".to_string())]
    );
}

#[test]
fn on_kill_can_cast_at_its_just_killed_invoker() {
    let rule = EventAiRule {
        source_rule_id: 104_7702,
        event: EventCondition::OnKill(KillCondition {
            character_only: false,
        }),
        chance_pct: 100,
        allowed_phases: PhaseSet { bits: u32::MAX },
        recurrence: RecurrencePolicy::RepeatOnEvent,
        selection: InstructionSelection::All,
        execution: ExecutionPolicy::Ordinary,
        posture: PostureAdmission::Any,
        instructions: vec![CreatureInstruction::Cast(CastInstruction {
            spell_id: 17_616,
            target: InstructionTarget::Invoker,
            interrupt_previous: false,
            start_mode: SpellStartMode::Direct,
            caster_role: SpellCasterRole::Actor,
            target_role: SpellTargetRole::Selected,
            aura_absent: false,
            character_only: false,
            target_must_be_casting: false,
            main_spell: false,
            distance_after_start: false,
        })],
    };
    let definition = EventAiDefinition {
        subject: EventAiSubject::Entry(ENTRY),
        revision: normalized_revision(EventAiSubject::Entry(ENTRY), std::slice::from_ref(&rule)),
        rules: vec![rule],
    };
    let mut scenario = scenario()
        .corpse(TARGET)
        .eventai_native_definition(definition);

    evaluate(
        &mut scenario,
        EventAiRequest::Edge(EventContext {
            invoker_guid: Some(TARGET),
            invoker_is_player: Some(true),
            engaged: true,
            ..EventContext::empty(EventKind::OnKill, CREATURE, 1_000)
        }),
    );

    assert_eq!(scenario.casts(), vec![(CREATURE, 17_616, TARGET)]);
}

#[test]
fn a_chance_miss_costs_the_opportunity_and_not_the_rule() {
    let mut scenario = scenario()
        .eventai_broadcast(4, "late", 0)
        .eventai_row(row(
            4,
            4,
            0,
            EVENT_ON_SPAWN,
            ACTION_SAY,
            50,
            1,
            REPEAT,
            [4, 0, 0],
        ))
        .rolls([50, 49]);

    edge(&mut scenario, EventKind::OnSpawn, false);

    assert!(scenario.eventai_speech().is_empty());
    assert!(scenario
        .eventai_state(CREATURE, 4)
        .is_some_and(|state| state.consumed));

    edge(&mut scenario, EventKind::OnSpawn, false);

    assert!(scenario.eventai_speech().is_empty());
}

#[test]
fn an_inverted_repeat_window_is_refused_instead_of_rolled() {
    let mut inverted = row(
        20,
        20,
        0,
        EVENT_CREATURE_HP,
        ACTION_SAY,
        100,
        1,
        REPEAT,
        [20, 0, 0],
    );
    inverted.event_param_2 = 100;
    inverted.event_param_3 = 5_000;
    inverted.event_param_4 = 4_000;
    let mut scenario = scenario()
        .eventai_broadcast(20, "hurt", 0)
        .eventai_row(inverted);

    edge(&mut scenario, EventKind::CreatureHp, false);

    assert!(scenario.eventai_speech().is_empty());
    assert_eq!(
        scenario
            .eventai_diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.kind)
            .collect::<Vec<_>>(),
        vec![DiagnosticKind::InvalidWindow]
    );
}

#[test]
fn a_dead_creature_says_nothing() {
    let mut scenario = scenario()
        .slain(CREATURE)
        .eventai_broadcast(21, "silent", 0)
        .eventai_row(row(
            21,
            21,
            0,
            EVENT_ON_AGGRO,
            ACTION_SAY,
            100,
            1,
            REPEAT_ONCE,
            [21, 0, 0],
        ));

    edge(&mut scenario, EventKind::OnAggro, false);

    assert!(scenario.eventai_speech().is_empty());
}

#[test]
fn an_overlong_line_is_capped_like_any_other_chat() {
    let mut scenario = scenario()
        .eventai_broadcast(22, &"a".repeat(400), 0)
        .eventai_row(row(
            22,
            22,
            0,
            EVENT_ON_AGGRO,
            ACTION_SAY,
            100,
            1,
            REPEAT_ONCE,
            [22, 0, 0],
        ));

    edge(&mut scenario, EventKind::OnAggro, false);

    // Vanilla caps client chat input around 255 characters; the stored row is bounded to match.
    assert_eq!(
        scenario
            .eventai_speech()
            .first()
            .map(|(_, _, line)| line.chars().count()),
        Some(255)
    );
}

#[test]
fn a_repeatable_condition_keeps_its_repeat_policy() {
    let mut repeated = row(
        10,
        10,
        0,
        EVENT_CREATURE_HP,
        ACTION_SAY,
        100,
        1,
        REPEAT,
        [10, 0, 0],
    );
    repeated.event_param_2 = 100;
    let mut scenario = scenario()
        .eventai_broadcast(10, "again", 0)
        .eventai_row(repeated);

    edge(&mut scenario, EventKind::CreatureHp, false);
    edge(&mut scenario, EventKind::CreatureHp, false);

    assert_eq!(
        scenario.eventai_speech(),
        vec![
            (CREATURE, 0, "again".to_string()),
            (CREATURE, 0, "again".to_string()),
        ]
    );
    assert!(!scenario.eventai_state(CREATURE, 10).unwrap().consumed);
}

#[test]
fn spawn_and_respawn_each_get_one_new_lifecycle() {
    let mut scenario = scenario().eventai_broadcast(5, "awake", 0).eventai_row(row(
        5,
        5,
        0,
        EVENT_ON_SPAWN,
        ACTION_SAY,
        100,
        1,
        REPEAT_ONCE,
        [5, 0, 0],
    ));

    edge(&mut scenario, EventKind::OnSpawn, false);
    edge(&mut scenario, EventKind::OnSpawn, false);
    reset_lifecycle(&scenario);
    edge(&mut scenario, EventKind::OnSpawn, false);

    assert_eq!(
        scenario.eventai_speech(),
        vec![
            (CREATURE, 0, "awake".to_string()),
            (CREATURE, 0, "awake".to_string()),
        ]
    );
}

#[test]
fn a_spawn_phase_change_gates_later_rules_in_source_order() {
    let mut scenario = scenario()
        .eventai_broadcast(11, "phase two", 0)
        .eventai_row(row(
            11,
            11,
            0,
            EVENT_ON_SPAWN,
            ACTION_SET_PHASE,
            100,
            1,
            REPEAT_ONCE,
            [2, 0, 0],
        ))
        .eventai_row(row(
            12,
            12,
            0,
            EVENT_ON_SPAWN,
            ACTION_SAY,
            100,
            1 << 2,
            REPEAT_ONCE,
            [11, 0, 0],
        ));

    edge(&mut scenario, EventKind::OnSpawn, false);

    assert_eq!(
        scenario.eventai_speech(),
        vec![(CREATURE, 0, "phase two".to_string())]
    );
}

#[test]
fn combat_exit_keeps_spawn_rules_consumed_in_the_same_lifecycle() {
    let mut scenario = scenario()
        .eventai_broadcast(13, "once", 0)
        .eventai_row(row(
            13,
            13,
            0,
            EVENT_ON_SPAWN,
            ACTION_SET_PHASE,
            100,
            1,
            REPEAT_ONCE,
            [1, 0, 0],
        ))
        .eventai_row(row(
            14,
            14,
            0,
            EVENT_ON_SPAWN,
            ACTION_SAY,
            100,
            1 << 1,
            REPEAT_ONCE,
            [13, 0, 0],
        ));

    edge(&mut scenario, EventKind::OnSpawn, false);
    EngageSink::leave_combat(&mut scenario, CREATURE);
    edge(&mut scenario, EventKind::OnSpawn, false);

    assert_eq!(
        scenario.eventai_speech(),
        vec![(CREATURE, 0, "once".to_string())]
    );
}

#[test]
fn death_runs_before_its_lifecycle_cleanup() {
    let mut scenario = scenario()
        .eventai_broadcast(6, "gone", 0)
        .eventai_row(row(
            6,
            6,
            0,
            EVENT_ON_AGGRO,
            ACTION_SET_PHASE,
            100,
            1,
            REPEAT_ONCE,
            [1, 0, 0],
        ))
        .eventai_row(row(
            7,
            7,
            0,
            EVENT_ON_DEATH,
            ACTION_SAY,
            100,
            1 << 1,
            REPEAT_ONCE,
            [6, 0, 0],
        ));

    edge(&mut scenario, EventKind::OnAggro, false);
    edge(&mut scenario, EventKind::OnDeath, false);
    reset_lifecycle(&scenario);

    assert_eq!(
        scenario.eventai_speech(),
        vec![(CREATURE, 0, "gone".to_string())]
    );
    assert!(scenario
        .eventai_creature_state
        .borrow()
        .get(&CREATURE)
        .is_none());
    assert!(scenario.eventai_state(CREATURE, 6).is_none());
    assert!(scenario.eventai_state(CREATURE, 7).is_none());
}

#[test]
fn combat_end_clears_only_engagement_state() {
    let mut scenario = scenario()
        .eventai_row(row(
            8,
            8,
            0,
            EVENT_ON_AGGRO,
            ACTION_SAY,
            100,
            1,
            REPEAT_ONCE,
            [0, 0, 0],
        ))
        .eventai_row(row(
            9,
            9,
            0,
            EVENT_ON_SPAWN,
            ACTION_SAY,
            100,
            1,
            REPEAT_ONCE,
            [0, 0, 0],
        ));
    scenario.eventai_creature_state.borrow_mut().insert(
        CREATURE,
        CreatureState {
            phase: 3,
            lifecycle_id: 4,
            engagement_id: 5,
            ranged_distance: 20.0,
            ranged_angle: 1.0,
            ranged_posture_active: true,
            definition_revision: DefinitionRevision::default(),
        },
    );
    scenario.eventai_rule_state.borrow_mut().insert(
        CREATURE,
        HashMap::from([
            (
                8,
                RuleState {
                    next_eligible_ms: 1,
                    consumed: true,
                    lifecycle_id: 4,
                    engagement_id: 5,
                    invocation_seed: 0,
                    invocation_started: false,
                    executing: false,
                    invocation_branch: 0,
                    paused_at_ms: 0,
                },
            ),
            (
                9,
                RuleState {
                    next_eligible_ms: 1,
                    consumed: true,
                    lifecycle_id: 4,
                    engagement_id: 5,
                    invocation_seed: 0,
                    invocation_started: false,
                    executing: false,
                    invocation_branch: 0,
                    paused_at_ms: 0,
                },
            ),
        ]),
    );

    EngageSink::leave_combat(&mut scenario, CREATURE);

    assert!(scenario.eventai_state(CREATURE, 8).is_none());
    assert!(scenario.eventai_state(CREATURE, 9).is_some());
    assert_eq!(
        scenario.eventai_creature_state(CREATURE),
        CreatureState {
            phase: 0,
            lifecycle_id: 4,
            engagement_id: 6,
            ranged_distance: 0.0,
            ranged_angle: 0.0,
            ranged_posture_active: false,
            definition_revision: DefinitionRevision::default(),
        }
    );
}

#[test]
fn missing_edge_context_targets_refuse_without_fallback() {
    let mut scenario = scenario();
    for (offset, target_policy) in [
        TARGET_INVOKER,
        TARGET_BENEFICIARY,
        TARGET_AI_SENDER,
        TARGET_SPAWNER,
        TARGET_EVENT,
    ]
    .into_iter()
    .enumerate()
    {
        let mut action = row(
            30 + offset as u64,
            30 + offset as u64,
            0,
            EVENT_ON_AGGRO,
            ACTION_CAST,
            100,
            1,
            REPEAT_ONCE,
            [300 + offset as u32, 0, 0],
        );
        action.target_policy = target_policy;
        scenario = scenario.eventai_row(action);
    }

    evaluate(
        &mut scenario,
        EventAiRequest::Edge(EventContext {
            engaged: true,
            ..EventContext::empty(EventKind::OnAggro, CREATURE, 1_000)
        }),
    );

    assert!(scenario.casts().is_empty());
}

#[test]
fn beneficiary_target_uses_the_invokers_immediate_creature_master() {
    let minion = CREATURE + 50;
    let master = CREATURE + 51;
    let mut action = row(
        40,
        40,
        0,
        EVENT_ON_AGGRO,
        ACTION_CAST,
        100,
        1,
        REPEAT_ONCE,
        [383, 0, 0],
    );
    action.target_policy = TARGET_BENEFICIARY;
    let mut scenario = scenario()
        .creature(master, point(3.0))
        .entry(master, ENTRY + 1)
        .pet(minion, master, point(2.0))
        .eventai_row(action);
    let beneficiary = beneficiary_guid(&scenario, minion);

    evaluate(
        &mut scenario,
        EventAiRequest::Edge(EventContext {
            invoker_guid: Some(minion),
            beneficiary_guid: beneficiary,
            engaged: true,
            ..EventContext::empty(EventKind::OnAggro, CREATURE, 1_000)
        }),
    );

    assert_eq!(beneficiary, Some(master));
    assert_eq!(scenario.casts(), vec![(CREATURE, 383, master)]);
}

#[test]
fn death_quest_predicate_requires_a_character_who_has_taken_the_quest() {
    let quest_entry = 7_734;
    let rule = EventAiRule {
        source_rule_id: 54_4102,
        event: EventCondition::OnDeath(DeathCondition {
            predicate: EventPredicate::QuestTaken(QuestTakenPredicate { quest_entry }),
        }),
        chance_pct: 100,
        allowed_phases: PhaseSet { bits: u32::MAX },
        recurrence: RecurrencePolicy::Once,
        selection: InstructionSelection::All,
        execution: ExecutionPolicy::Ordinary,
        posture: PostureAdmission::Any,
        instructions: vec![CreatureInstruction::Speak(SpeakInstruction {
            mode: SpeechMode::Say,
            broadcast_ids: vec![50],
            legacy_text: String::new(),
            target: InstructionTarget::SelfActor,
        })],
    };
    let definition = EventAiDefinition {
        subject: EventAiSubject::Entry(ENTRY),
        revision: normalized_revision(EventAiSubject::Entry(ENTRY), std::slice::from_ref(&rule)),
        rules: vec![rule],
    };
    let npc_killer = CREATURE + 60;
    let mut scenario = scenario()
        .creature(npc_killer, point(3.0))
        .entry(npc_killer, ENTRY + 2)
        .eventai_broadcast(50, "accepted", 0)
        .eventai_native_definition(definition)
        .eventai_quest_taken(TARGET, quest_entry);

    evaluate(
        &mut scenario,
        EventAiRequest::Edge(EventContext {
            invoker_guid: Some(npc_killer),
            beneficiary_guid: Some(npc_killer),
            ..EventContext::empty(EventKind::OnDeath, CREATURE, 1_000)
        }),
    );
    assert!(scenario.eventai_speech().is_empty());

    evaluate(
        &mut scenario,
        EventAiRequest::Edge(EventContext {
            invoker_guid: Some(TARGET),
            beneficiary_guid: Some(TARGET),
            ..EventContext::empty(EventKind::OnDeath, CREATURE, 1_000)
        }),
    );

    assert_eq!(
        scenario.eventai_speech(),
        vec![(CREATURE, 0, "accepted".to_string())]
    );
}

#[test]
fn a_pending_evade_return_fires_reached_home_once() {
    let rule = EventAiRule {
        source_rule_id: 60,
        event: EventCondition::OnReachedHome,
        chance_pct: 100,
        allowed_phases: PhaseSet { bits: u32::MAX },
        recurrence: RecurrencePolicy::Once,
        selection: InstructionSelection::All,
        execution: ExecutionPolicy::Ordinary,
        posture: PostureAdmission::Any,
        instructions: vec![CreatureInstruction::Speak(SpeakInstruction {
            mode: SpeechMode::Say,
            broadcast_ids: vec![60],
            legacy_text: String::new(),
            target: InstructionTarget::SelfActor,
        })],
    };
    let definition = EventAiDefinition {
        subject: EventAiSubject::Entry(ENTRY),
        revision: normalized_revision(EventAiSubject::Entry(ENTRY), std::slice::from_ref(&rule)),
        rules: vec![rule],
    };
    let mut scenario = Scenario::new(1_000_000)
        .creature(CREATURE, point(0.0))
        .entry(CREATURE, ENTRY)
        .home(CREATURE, point(0.0), false)
        .awake([CREATURE])
        .eventai_broadcast(60, "home", 0)
        .eventai_native_definition(definition);
    scenario
        .eventai_returning_home
        .borrow_mut()
        .insert(CREATURE);

    let tick = scenario.tick(
        true,
        TickScope::CatchAll {
            dedicated: HashSet::new(),
        },
    );
    run_cycle(&mut scenario, tick);
    let tick = scenario.tick(
        true,
        TickScope::CatchAll {
            dedicated: HashSet::new(),
        },
    );
    run_cycle(&mut scenario, tick);

    assert_eq!(
        scenario.eventai_speech(),
        vec![(CREATURE, 0, "home".to_string())]
    );
}

#[test]
fn receive_ai_event_keeps_invoker_and_sender_targets_distinct() {
    let sender = CREATURE + 70;
    let invoker = CREATURE + 71;
    let cast = |spell_id, target| {
        CreatureInstruction::Cast(CastInstruction {
            spell_id,
            target,
            interrupt_previous: false,
            start_mode: SpellStartMode::Direct,
            caster_role: SpellCasterRole::Actor,
            target_role: SpellTargetRole::Selected,
            aura_absent: false,
            character_only: false,
            target_must_be_casting: false,
            main_spell: false,
            distance_after_start: false,
        })
    };
    let rule = EventAiRule {
        source_rule_id: 70,
        event: EventCondition::OnReceiveAiEvent(ReceiveAiEventCondition {
            kind: AiEventKind::CustomA,
            sender_entry: ENTRY + 3,
        }),
        chance_pct: 100,
        allowed_phases: PhaseSet { bits: u32::MAX },
        recurrence: RecurrencePolicy::RepeatOnEvent,
        selection: InstructionSelection::All,
        execution: ExecutionPolicy::Ordinary,
        posture: PostureAdmission::Any,
        instructions: vec![
            cast(701, InstructionTarget::Invoker),
            cast(702, InstructionTarget::AiSender),
        ],
    };
    let definition = EventAiDefinition {
        subject: EventAiSubject::Entry(ENTRY),
        revision: normalized_revision(EventAiSubject::Entry(ENTRY), std::slice::from_ref(&rule)),
        rules: vec![rule],
    };
    let mut scenario = scenario()
        .creature(sender, point(3.0))
        .entry(sender, ENTRY + 3)
        .creature(invoker, point(4.0))
        .entry(invoker, ENTRY + 4)
        .eventai_native_definition(definition);

    evaluate(
        &mut scenario,
        EventAiRequest::Edge(EventContext {
            invoker_guid: Some(invoker),
            ai_sender_guid: Some(sender),
            ai_event: Some(AiEventKind::CustomA),
            ..EventContext::empty(EventKind::OnReceiveAiEvent, CREATURE, 1_000)
        }),
    );

    assert_eq!(
        scenario.casts(),
        vec![(CREATURE, 701, invoker), (CREATURE, 702, sender)]
    );
}

#[test]
fn native_presentation_instructions_reach_the_creature_world_seam_in_order() {
    let instructions = vec![
        CreaturePresentationInstruction::SetFaction {
            faction_template: 777,
        },
        CreaturePresentationInstruction::ShowTemplateDisplay {
            template_entry: 11_284,
        },
        CreaturePresentationInstruction::SetCreatureMount {
            mount: CreaturePresentationMount::TwilightMarauder,
        },
        CreaturePresentationInstruction::SetNpcFlags {
            flags: NpcFlagsProjection::Clear,
        },
        CreaturePresentationInstruction::SetNpcFlags {
            flags: NpcFlagsProjection::GossipAndQuest,
        },
        CreaturePresentationInstruction::EmptyMana,
        CreaturePresentationInstruction::ClearVirtualMainHand,
        CreaturePresentationInstruction::SetNotAttackable,
        CreaturePresentationInstruction::ClearNotAttackable,
        CreaturePresentationInstruction::SetImmuneToPlayers,
        CreaturePresentationInstruction::ClearImmuneToPlayers,
        CreaturePresentationInstruction::SetImmuneToCreatures,
        CreaturePresentationInstruction::ClearImmuneToCreatures,
        CreaturePresentationInstruction::SetImmuneToPlayersAndCreatures,
        CreaturePresentationInstruction::ClearImmuneToPlayersAndCreatures,
        CreaturePresentationInstruction::SetNotSelectable,
    ];
    let rule = EventAiRule {
        source_rule_id: 81,
        event: EventCondition::OnAggro,
        chance_pct: 100,
        allowed_phases: PhaseSet { bits: u32::MAX },
        recurrence: RecurrencePolicy::Once,
        selection: InstructionSelection::All,
        execution: ExecutionPolicy::Ordinary,
        posture: PostureAdmission::Any,
        instructions: instructions
            .iter()
            .copied()
            .map(CreatureInstruction::Presentation)
            .collect(),
    };
    let definition = EventAiDefinition {
        subject: EventAiSubject::Entry(ENTRY),
        revision: normalized_revision(EventAiSubject::Entry(ENTRY), std::slice::from_ref(&rule)),
        rules: vec![rule],
    };
    let mut scenario = scenario().eventai_native_definition(definition);

    edge(&mut scenario, EventKind::OnAggro, false);

    let state = scenario.eventai_creature_state(CREATURE);
    let expected: Vec<_> = instructions
        .into_iter()
        .map(|instruction| {
            (
                CREATURE,
                state.lifecycle_id,
                state.definition_revision,
                instruction,
            )
        })
        .collect();
    assert_eq!(
        scenario.eventai_presentation.borrow().as_slice(),
        expected.as_slice()
    );
}
