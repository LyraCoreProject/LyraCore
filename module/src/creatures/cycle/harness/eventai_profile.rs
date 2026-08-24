//! Canonical-profile EventAI capability scenarios.

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
fn guardian_entry_removal_reaps_one_match_and_zero_reaps_the_rest() {
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
        .eventai_native_definition(definition(
            EventAiSubject::Entry(ENTRY),
            2,
            InstructionSelection::All,
            vec![
                spawn(GUARDIAN_ENTRY),
                spawn(GUARDIAN_ENTRY),
                spawn(OTHER_GUARDIAN_ENTRY),
                CreatureInstruction::RemoveGuardians(RemoveGuardiansInstruction {
                    creature_entry: GUARDIAN_ENTRY,
                }),
            ],
        ));

    dispatch(&mut scenario, CREATURE);

    let remaining = scenario.eventai_summoned_guids();
    assert_eq!(remaining.len(), 2);
    assert_eq!(
        remaining
            .iter()
            .map(|guid| scenario.creatures.borrow()[guid].entry)
            .collect::<Vec<_>>(),
        vec![GUARDIAN_ENTRY, OTHER_GUARDIAN_ENTRY]
    );
    assert!(EventAiWorld::eventai_remove_guardians(
        &mut scenario,
        CREATURE,
        0
    ));
    assert!(scenario.eventai_summoned_guids().is_empty());
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
