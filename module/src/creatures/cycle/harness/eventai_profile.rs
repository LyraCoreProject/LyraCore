use super::*;
use crate::creatures::eventai::*;

const CREATURE: u64 = 8_401;
const ALLY: u64 = 8_402;
const TARGET: u64 = 8_403;
const ENTRY: u32 = 940;
const GUARDIAN_ENTRY: u32 = 941;
const OTHER_GUARDIAN_ENTRY: u32 = 942;

fn point(x: f32) -> Point {
    Point { x, y: 0.0, z: 0.0 }
}

fn world() -> Scenario {
    Scenario::new(1_000_000)
        .creature(CREATURE, point(0.0))
        .entry(CREATURE, ENTRY)
        .player(TARGET, point(2.0))
        .at_war(BEASTS, ALLIANCE)
}

fn definition(
    subject: EventAiSubject,
    source_rule_id: u64,
    selection: InstructionSelection,
    instructions: Vec<CreatureInstruction>,
) -> EventAiDefinition {
    let rule = EventAiRule {
        source_rule_id,
        event: EventCondition::OnAggro,
        chance_pct: 100,
        allowed_phases: PhaseSet { bits: u32::MAX },
        recurrence: RecurrencePolicy::Once,
        selection,
        execution: ExecutionPolicy::Ordinary,
        posture: PostureAdmission::Any,
        instructions,
    };
    EventAiDefinition {
        subject,
        revision: normalized_revision(subject, std::slice::from_ref(&rule)),
        rules: vec![rule],
    }
}

fn dispatch(scenario: &mut Scenario, creature_guid: u64) {
    evaluate(
        scenario,
        EventAiRequest::Edge(EventContext {
            current_target_guid: Some(TARGET),
            invoker_guid: Some(TARGET),
            engaged: true,
            ..EventContext::empty(EventKind::OnAggro, creature_guid, 1_000)
        }),
    );
}

#[test]
fn profile_actions_reach_their_named_world_owners() {
    let instructions = vec![
        CreatureInstruction::RandomEmote(RandomEmoteInstruction {
            emote_ids: vec![11, 18, 0],
        }),
        CreatureInstruction::SpawnAtActor(SpawnAtActorInstruction {
            creature_entry: GUARDIAN_ENTRY,
            target: InstructionTarget::CurrentOpponent,
            lifetime_ms: 10_000,
        }),
        CreatureInstruction::RemoveAura(RemoveAuraInstruction {
            spell_id: 8_909,
            target: InstructionTarget::SelfActor,
        }),
        CreatureInstruction::ThrowAiEvent(ThrowAiEventInstruction {
            kind: AiEventKind::CustomA,
            radius_yd: 50,
            target: InstructionTarget::SelfActor,
        }),
        CreatureInstruction::SetStandState(SetStandStateInstruction { stand_state: 7 }),
        CreatureInstruction::SetReactState(SetReactStateInstruction {
            state: CreatureReactState::Defensive,
        }),
        CreatureInstruction::MissingTextTemplateNoEffect(MissingTextTemplateNoEffect {
            template_id: 99_999,
        }),
        CreatureInstruction::ForceDespawn(ForceDespawnInstruction { delay_ms: 3_000 }),
    ];
    let mut scenario = world()
        .eventai_template(GUARDIAN_ENTRY)
        .rolls([1])
        .eventai_native_definition(definition(
            EventAiSubject::Entry(ENTRY),
            1,
            InstructionSelection::All,
            instructions,
        ));
    scenario.auras.borrow_mut().insert((CREATURE, 8_909));

    dispatch(&mut scenario, CREATURE);

    assert_eq!(scenario.eventai_emotes(), vec![(CREATURE, 0, 18, 0)]);
    assert!(!scenario.auras.borrow().contains(&(CREATURE, 8_909)));
    assert_eq!(
        scenario.eventai_ai_events.borrow().as_slice(),
        [(CREATURE, CREATURE, AiEventKind::CustomA, 50)]
    );
    assert_eq!(
        scenario.eventai_stand_states.borrow().get(&CREATURE),
        Some(&7)
    );
    assert_eq!(
        scenario.eventai_react_states.borrow().get(&CREATURE),
        Some(&CreatureReactState::Defensive)
    );
    assert_eq!(
        scenario.eventai_forced_despawns.borrow().as_slice(),
        [(CREATURE, 3_000)]
    );
    assert!(scenario.creatures.borrow().contains_key(&CREATURE));

    let summoned = scenario.eventai_summoned_guids();
    let [summon] = summoned.as_slice() else {
        panic!("expected one owned spawn");
    };
    assert_eq!(scenario.creatures.borrow()[summon].entry, GUARDIAN_ENTRY);
    assert_eq!(scenario.creatures.borrow()[summon].at, point(0.0));
    assert!(scenario
        .fights
        .borrow()
        .iter()
        .any(|fight| fight.attacker == *summon && fight.victim == TARGET));
}

#[test]
fn guardian_removal_reaps_a_spell_pet_and_leaves_an_eventai_summon() {
    const GUARDIAN_GUID: u64 = 8_404;
    let spawn = |entry| {
        CreatureInstruction::SpawnAtActor(SpawnAtActorInstruction {
            creature_entry: entry,
            target: InstructionTarget::SelfActor,
            lifetime_ms: 10_000,
        })
    };
    let mut scenario = world()
        .eventai_template(GUARDIAN_ENTRY)
        .eventai_template(OTHER_GUARDIAN_ENTRY)
        .pet(GUARDIAN_GUID, CREATURE, point(0.0))
        .entry(GUARDIAN_GUID, GUARDIAN_ENTRY)
        .eventai_native_definition(definition(
            EventAiSubject::Entry(ENTRY),
            2,
            InstructionSelection::All,
            vec![
                spawn(OTHER_GUARDIAN_ENTRY),
                CreatureInstruction::RemoveGuardians(RemoveGuardiansInstruction {
                    creature_entry: 0,
                }),
            ],
        ));

    dispatch(&mut scenario, CREATURE);

    let remaining = scenario.eventai_summoned_guids();
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        scenario.creatures.borrow()[&remaining[0]].entry,
        OTHER_GUARDIAN_ENTRY
    );
    assert!(!scenario.creatures.borrow().contains_key(&GUARDIAN_GUID));
}

#[test]
fn immediate_forced_despawn_leaves_no_rule_or_summon_lifetime_state() {
    let mut scenario = world().eventai_native_definition(definition(
        EventAiSubject::Entry(ENTRY),
        3,
        InstructionSelection::All,
        vec![CreatureInstruction::ForceDespawn(ForceDespawnInstruction {
            delay_ms: 0,
        })],
    ));

    dispatch(&mut scenario, CREATURE);

    assert!(!scenario.creatures.borrow().contains_key(&CREATURE));
    assert!(!scenario.eventai_rule_state.borrow().contains_key(&CREATURE));
    assert!(!scenario
        .eventai_summon_expiry
        .borrow()
        .contains_key(&CREATURE));
    assert_eq!(
        scenario.eventai_forced_despawns.borrow().as_slice(),
        [(CREATURE, 0)]
    );
}

#[test]
fn missing_text_no_effect_keeps_random_branch_probability_and_success() {
    let rule = |roll| {
        world()
            .eventai_broadcast(7, "selected", crate::chat::CHAT_SAY)
            .rolls([roll])
            .eventai_native_definition(definition(
                EventAiSubject::Entry(ENTRY),
                4,
                InstructionSelection::RandomOne,
                vec![
                    CreatureInstruction::MissingTextTemplateNoEffect(MissingTextTemplateNoEffect {
                        template_id: 88_888,
                    }),
                    CreatureInstruction::Speak(SpeakInstruction {
                        mode: SpeechMode::Say,
                        broadcast_ids: vec![7],
                        legacy_text: String::new(),
                        target: InstructionTarget::SelfActor,
                    }),
                ],
            ))
    };
    let mut empty = rule(0);
    dispatch(&mut empty, CREATURE);
    assert!(empty.eventai_speech().is_empty());
    assert!(empty
        .eventai_state(CREATURE, 4)
        .is_some_and(|state| state.consumed));

    let mut spoken = rule(1);
    dispatch(&mut spoken, CREATURE);
    assert_eq!(
        spoken.eventai_speech(),
        vec![(CREATURE, crate::chat::CHAT_SAY, "selected".to_string())]
    );
}

#[test]
fn broadcast_text_emote_keeps_its_own_chat_type() {
    let mut scenario = world()
        .eventai_broadcast(7_133, "gestures", crate::chat::CHAT_TEXT_EMOTE)
        .eventai_native_definition(definition(
            EventAiSubject::Entry(ENTRY),
            5,
            InstructionSelection::All,
            vec![CreatureInstruction::Speak(SpeakInstruction {
                mode: SpeechMode::Say,
                broadcast_ids: vec![7_133],
                legacy_text: String::new(),
                target: InstructionTarget::SelfActor,
            })],
        ));

    dispatch(&mut scenario, CREATURE);

    assert_eq!(
        scenario.eventai_speech(),
        vec![(
            CREATURE,
            crate::chat::CHAT_TEXT_EMOTE,
            "gestures".to_string()
        )]
    );
}

#[test]
fn defensive_creatures_assist_but_passive_creatures_do_not() {
    let build = |state| {
        let mut scenario = Scenario::new(1_000_000)
            .creature(CREATURE, point(0.0))
            .entry(CREATURE, ENTRY)
            .creature(ALLY, point(2.0))
            .entry(ALLY, ENTRY + 1)
            .player(TARGET, point(3.0))
            .awake([CREATURE, ALLY])
            .at_war(BEASTS, ALLIANCE)
            .eventai_native_definition(definition(
                EventAiSubject::Guid(ALLY),
                6,
                InstructionSelection::All,
                vec![CreatureInstruction::SetReactState(
                    SetReactStateInstruction { state },
                )],
            ));
        dispatch(&mut scenario, ALLY);
        scenario
    };

    let mut defensive = build(CreatureReactState::Defensive);
    let tick = defensive.tick(
        true,
        TickScope::CatchAll {
            dedicated: HashSet::new(),
        },
    );
    run_cycle(&mut defensive, tick);
    assert!(defensive.pulls().contains(&(ALLY, TARGET, Pull::Assisted)));

    let mut passive = build(CreatureReactState::Passive);
    let tick = passive.tick(
        true,
        TickScope::CatchAll {
            dedicated: HashSet::new(),
        },
    );
    run_cycle(&mut passive, tick);
    assert!(!passive.pulls().iter().any(|(guid, _, _)| *guid == ALLY));
}

#[test]
fn random_home_and_random_current_keep_distinct_anchors() {
    let mut scenario = world().home(CREATURE, point(-8.0), false);
    assert!(EventAiWorld::apply_eventai_movement(
        &mut scenario,
        CREATURE,
        MovementOperation::ReplaceIdle(IdleMovementIntent::RandomAroundHomePosition(
            RandomMovementIntent { radius_yd: 20 },
        )),
    ));
    assert_eq!(
        IdleSink::home_of(&scenario, CREATURE),
        Some(Home {
            at: point(-8.0),
            wanders: true,
        })
    );
    assert_eq!(IdleSink::wander_radius(&scenario, CREATURE), 20.0);

    assert!(EventAiWorld::apply_eventai_movement(
        &mut scenario,
        CREATURE,
        MovementOperation::ReplaceIdle(IdleMovementIntent::RandomAroundCurrentPosition(
            RandomMovementIntent { radius_yd: 7 },
        )),
    ));
    assert_eq!(
        IdleSink::home_of(&scenario, CREATURE),
        Some(Home {
            at: point(0.0),
            wanders: true,
        })
    );
    assert_eq!(IdleSink::wander_radius(&scenario, CREATURE), 7.0);
}

#[test]
fn an_empty_text_action_does_not_cancel_sibling_actions() {
    let mut scenario = world().eventai_native_definition(definition(
        EventAiSubject::Entry(ENTRY),
        700,
        InstructionSelection::All,
        vec![
            CreatureInstruction::EmptyTextNoEffect,
            CreatureInstruction::SetPhase(SetPhaseInstruction { phase: 3 }),
        ],
    ));
    dispatch(&mut scenario, CREATURE);
    assert_eq!(scenario.eventai_creature_state(CREATURE).phase, 3);
}

#[test]
fn archer_actions_disable_melee_and_draw_the_ranged_weapon() {
    let mut scenario = world().eventai_native_definition(definition(
        EventAiSubject::Entry(ENTRY),
        701,
        InstructionSelection::All,
        vec![
            CreatureInstruction::SetAutoAttack(false),
            CreatureInstruction::SetSheath(2),
        ],
    ));
    dispatch(&mut scenario, CREATURE);
    assert_eq!(
        scenario.eventai_auto_attacks.borrow().get(&CREATURE),
        Some(&false)
    );
    assert_eq!(scenario.eventai_sheaths.borrow().get(&CREATURE), Some(&2));
}

#[test]
fn a_zone_combat_pulse_keeps_distant_instance_members_in_combat() {
    let far = TARGET + 1;
    let other_instance = TARGET + 2;
    let mut scenario = world()
        .tweak(CREATURE, |unit| {
            unit.map_id = 36;
            unit.instance_id = 91;
        })
        .tweak_player(TARGET, |unit| {
            unit.map_id = 36;
            unit.instance_id = 91;
        })
        .player(far, point(2_000.0))
        .tweak_player(far, |unit| {
            unit.map_id = 36;
            unit.instance_id = 91;
        })
        .player(other_instance, point(1.0))
        .tweak_player(other_instance, |unit| {
            unit.map_id = 36;
            unit.instance_id = 92;
        })
        .eventai_native_definition(definition(
            EventAiSubject::Entry(ENTRY),
            702,
            InstructionSelection::All,
            vec![CreatureInstruction::ZoneCombat],
        ));
    EngageSink::engage(&mut scenario, CREATURE, TARGET, Pull::Noticed);
    assert_eq!(scenario.threat.borrow().get(&(CREATURE, far)), Some(&0));
    assert!(!scenario
        .threat
        .borrow()
        .contains_key(&(CREATURE, other_instance)));
    scenario.now_micros.set(20_000_000);
    let scope = TickScope::CatchAll {
        dedicated: HashSet::new(),
    };
    super::super::combat_entry(&mut scenario, &scope);
    assert_eq!(scenario.combat_flags.borrow().get(&far), Some(&26_000));
}

#[test]
fn creature_whispers_require_and_keep_a_character_recipient() {
    let mode = lyracore_shared::chat::broadcast_chat::CREATURE_WHISPER;
    for (target, delivered) in [
        (InstructionTarget::CurrentOpponent, true),
        (InstructionTarget::SelfActor, false),
    ] {
        let mut scenario = world()
            .eventai_broadcast(11929, "Use the console", mode)
            .eventai_native_definition(definition(
                EventAiSubject::Entry(ENTRY),
                703,
                InstructionSelection::All,
                vec![CreatureInstruction::Speak(SpeakInstruction {
                    mode: SpeechMode::Whisper,
                    broadcast_ids: vec![11929],
                    legacy_text: String::new(),
                    target,
                })],
            ));
        dispatch(&mut scenario, CREATURE);
        assert_eq!(!scenario.eventai_speech().is_empty(), delivered);
        if delivered {
            assert_eq!(scenario.eventai_speech_targets(), vec![(CREATURE, TARGET)]);
        }
    }
}

fn command_list_definition(random: bool, abort_on_failure: Vec<u32>) -> EventAiDefinition {
    let mut result = definition(
        EventAiSubject::Entry(ENTRY),
        90,
        InstructionSelection::CommandLists(CommandLists {
            ends: vec![2, 4],
            abort_on_failure,
            random,
        }),
        vec![
            CreatureInstruction::Emote(EmoteInstruction {
                emote_id: 1,
                target: InstructionTarget::SelfActor,
            }),
            CreatureInstruction::SetPhase(SetPhaseInstruction { phase: 1 }),
            CreatureInstruction::Emote(EmoteInstruction {
                emote_id: 2,
                target: InstructionTarget::SelfActor,
            }),
            CreatureInstruction::SetPhase(SetPhaseInstruction { phase: 2 }),
        ],
    );
    result.rules[0].execution = ExecutionPolicy::ScriptEvent(ScriptEventPolicy {
        not_casting: false,
        check_result: false,
        predicate: EventPredicate::Always,
    });
    result
}

#[test]
fn random_command_list_runs_every_command_in_one_group() {
    let mut scenario = world()
        .rolls([1])
        .eventai_native_definition(command_list_definition(true, vec![]));
    dispatch(&mut scenario, CREATURE);
    assert_eq!(scenario.eventai_emotes(), vec![(CREATURE, 0, 2, CREATURE)]);
    assert_eq!(scenario.eventai_creature_state(CREATURE).phase, 2);
}

#[test]
fn failed_command_aborts_its_list_and_retry_leaves_the_other_list_effects() {
    let mut definition = command_list_definition(false, vec![0]);
    definition.rules[0].instructions[0] =
        CreatureInstruction::AttackStart(InstructionTarget::Spawner);
    definition.rules[0].execution = ExecutionPolicy::ScriptEvent(ScriptEventPolicy {
        not_casting: false,
        check_result: true,
        predicate: EventPredicate::Always,
    });
    let mut scenario = world().eventai_native_definition(definition);
    dispatch(&mut scenario, CREATURE);
    assert_eq!(scenario.eventai_creature_state(CREATURE).phase, 2);
    assert_eq!(scenario.eventai_emotes().len(), 1);
    let state = scenario.eventai_rule_state(CREATURE, 90).unwrap();
    assert!(!state.consumed);
    assert!(!state.executing);
    assert!(!state.invocation_started);
    dispatch(&mut scenario, CREATURE);
    assert_eq!(scenario.eventai_emotes().len(), 2);
}

#[test]
fn command_failure_without_abort_continues_and_spends_the_opportunity() {
    let mut definition = command_list_definition(false, vec![]);
    definition.rules[0].instructions[0] =
        CreatureInstruction::AttackStart(InstructionTarget::Spawner);
    let mut scenario = world().eventai_native_definition(definition);
    dispatch(&mut scenario, CREATURE);
    assert_eq!(scenario.eventai_creature_state(CREATURE).phase, 2);
    assert!(scenario.eventai_rule_state(CREATURE, 90).unwrap().consumed);
}

#[test]
fn script_event_waits_for_cast_to_finish_before_spending_the_opportunity() {
    let mut definition = command_list_definition(false, vec![]);
    definition.rules[0].execution = ExecutionPolicy::ScriptEvent(ScriptEventPolicy {
        not_casting: true,
        check_result: false,
        predicate: EventPredicate::Always,
    });
    let mut scenario = world().eventai_native_definition(definition);
    scenario.casting.borrow_mut().insert(CREATURE);
    dispatch(&mut scenario, CREATURE);
    assert!(scenario.eventai_emotes().is_empty());
    scenario.casting.borrow_mut().remove(&CREATURE);
    dispatch(&mut scenario, CREATURE);
    assert_eq!(scenario.eventai_emotes().len(), 2);
}

#[test]
fn provided_target_prefers_event_invoker_to_current_opponent() {
    let mut definition = command_list_definition(false, vec![]);
    definition.rules[0].instructions[0] = CreatureInstruction::Emote(EmoteInstruction {
        emote_id: 5,
        target: InstructionTarget::ProvidedTarget,
    });
    let mut scenario = world()
        .player(ALLY, point(3.0))
        .eventai_native_definition(definition);
    evaluate(
        &mut scenario,
        EventAiRequest::Edge(EventContext {
            current_target_guid: Some(TARGET),
            invoker_guid: Some(ALLY),
            engaged: true,
            ..EventContext::empty(EventKind::OnAggro, CREATURE, 1_000)
        }),
    );
    assert_eq!(scenario.eventai_emotes()[0], (CREATURE, 0, 5, ALLY));
}

#[test]
fn provided_speech_without_a_target_still_reaches_nearby_players() {
    let mut definition = command_list_definition(false, vec![]);
    definition.rules[0].instructions[0] = CreatureInstruction::Speak(SpeakInstruction {
        mode: SpeechMode::Say,
        broadcast_ids: vec![8],
        legacy_text: String::new(),
        target: InstructionTarget::ProvidedTarget,
    });
    let mut scenario = world()
        .eventai_broadcast(8, "Hello", crate::chat::CHAT_SAY)
        .eventai_native_definition(definition);
    evaluate(
        &mut scenario,
        EventAiRequest::Edge(EventContext::empty(EventKind::OnAggro, CREATURE, 1_000)),
    );
    assert_eq!(
        scenario.eventai_speech(),
        vec![(CREATURE, crate::chat::CHAT_SAY, "Hello".into())]
    );
}

#[test]
fn a_fleeing_creature_delivers_its_emote_to_the_player() {
    let mut scenario = world()
        .attacking(CREATURE, TARGET)
        .eventai_broadcast(1150, "%s attempts to run away in fear!", 2)
        .eventai_native_definition(definition(
            EventAiSubject::Entry(ENTRY),
            1,
            InstructionSelection::All,
            vec![
                CreatureInstruction::Speak(SpeakInstruction {
                    mode: SpeechMode::Emote,
                    broadcast_ids: vec![1150],
                    legacy_text: String::new(),
                    target: InstructionTarget::CurrentOpponent,
                }),
                CreatureInstruction::FleeForAssist,
            ],
        ));
    dispatch(&mut scenario, CREATURE);
    assert_eq!(scenario.eventai_speech().len(), 1);
    assert_eq!(scenario.eventai_speech_targets(), [(CREATURE, TARGET)]);
    assert!(rout_window_open(1_000, scenario.rout_ends_ms(CREATURE)));
}

#[test]
fn every_random_emote_choice_reaches_the_player() {
    for choices in [vec![18, 20], vec![4, 21, 22, 11]] {
        for (roll, expected) in choices.iter().enumerate() {
            let mut scenario = world()
                .rolls([roll as u32])
                .eventai_native_definition(definition(
                    EventAiSubject::Entry(ENTRY),
                    1,
                    InstructionSelection::All,
                    vec![CreatureInstruction::RandomEmote(RandomEmoteInstruction {
                        emote_ids: choices.clone(),
                    })],
                ));
            dispatch(&mut scenario, CREATURE);
            assert_eq!(
                scenario.eventai_emotes(),
                [(CREATURE, 0, *expected as u32, 0)]
            );
        }
    }
}

#[test]
fn a_summon_without_a_current_opponent_still_appears_for_the_player() {
    let mut scenario = world()
        .eventai_template(GUARDIAN_ENTRY)
        .eventai_native_definition(definition(
            EventAiSubject::Entry(ENTRY),
            1,
            InstructionSelection::All,
            vec![CreatureInstruction::SpawnAtActor(SpawnAtActorInstruction {
                creature_entry: GUARDIAN_ENTRY,
                target: InstructionTarget::CurrentOpponent,
                lifetime_ms: 10_000,
            })],
        ));
    evaluate(
        &mut scenario,
        EventAiRequest::Edge(EventContext::empty(EventKind::OnAggro, CREATURE, 1_000)),
    );
    let summons = scenario.eventai_summoned_guids();
    assert_eq!(summons.len(), 1);
    assert_eq!(
        scenario.creatures.borrow()[&summons[0]].entry,
        GUARDIAN_ENTRY
    );
    assert!(scenario.fights.borrow().is_empty());
}
