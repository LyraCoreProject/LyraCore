use super::*;
use crate::creatures::eventai::{
    ImmobilizationInstruction, MovementSwitch, PatrolIntent, PatrolPause, RandomMovementIntent,
    RangedModeInstruction,
};

fn apply(world: &mut Scenario, guid: u64, operation: MovementOperation) -> bool {
    EventAiWorld::apply_eventai_movement(world, guid, operation)
}

#[test]
fn patrol_replacement_pause_and_resume_keep_the_route_cursor() {
    let mut world = Scenario::new(SETTLED)
        .creature(WOLF, p(2.0, 0.0, 10.0))
        .awake([WOLF])
        .home(WOLF, p(0.0, 0.0, 10.0), false)
        .route(WOLF, &[(1, p(0.0, 0.0, 10.0)), (2, p(10.0, 0.0, 10.0))]);
    assert!(apply(
        &mut world,
        WOLF,
        MovementOperation::ReplaceIdle(IdleMovementIntent::Patrol(PatrolIntent { path_id: 0 }))
    ));

    let tick = world.tick(false, catch_all());
    run_cycle(&mut world, tick);
    assert_eq!(world.at(WOLF).wp_target, 1);
    let first_effects = world.effects().len();

    assert!(apply(
        &mut world,
        WOLF,
        MovementOperation::SetPatrolPaused(PatrolPause { paused: true })
    ));
    world.advance_clock(2_000_000);
    let tick = world.tick(false, catch_all());
    run_cycle(&mut world, tick);
    assert_eq!(world.at(WOLF).wp_target, 1, "pause changed the cursor");
    assert_eq!(world.effects().len(), first_effects);

    assert!(apply(
        &mut world,
        WOLF,
        MovementOperation::SetPatrolPaused(PatrolPause { paused: false })
    ));
    world.advance_clock(1_000_000);
    let tick = world.tick(false, catch_all());
    run_cycle(&mut world, tick);
    assert_eq!(world.at(WOLF).wp_target, 2, "resume restarted the route");
    assert_eq!(world.effects().len(), first_effects + 1);
}

#[test]
fn explicit_paths_match_the_live_subject_and_path_zero_uses_the_spawn_route() {
    let mut world = Scenario::new(SETTLED)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .route(WOLF, &[(1, p(0.0, 0.0, 10.0)), (2, p(10.0, 0.0, 10.0))]);
    assert!(apply(
        &mut world,
        WOLF,
        MovementOperation::ReplaceIdle(IdleMovementIntent::Patrol(PatrolIntent { path_id: 0 }))
    ));
    assert!(!apply(
        &mut world,
        WOLF,
        MovementOperation::ReplaceIdle(IdleMovementIntent::Patrol(PatrolIntent { path_id: 7 }))
    ));

    world.eventai_paths.borrow_mut().insert(
        (WOLF as u32, MAP + 1, 7),
        vec![Waypoint {
            id: 1,
            at: p(40.0, 40.0, 10.0),
        }],
    );
    assert!(!apply(
        &mut world,
        WOLF,
        MovementOperation::ReplaceIdle(IdleMovementIntent::Patrol(PatrolIntent { path_id: 7 }))
    ));

    world.eventai_paths.borrow_mut().insert(
        (WOLF as u32, MAP, 7),
        vec![
            Waypoint {
                id: 1,
                at: p(0.0, 0.0, 10.0),
            },
            Waypoint {
                id: 2,
                at: p(0.0, 8.0, 10.0),
            },
        ],
    );
    assert!(apply(
        &mut world,
        WOLF,
        MovementOperation::ReplaceIdle(IdleMovementIntent::Patrol(PatrolIntent { path_id: 7 }))
    ));
    assert_eq!(IdleSink::route_of(&world, WOLF)[1].at.y, 8.0);

    world
        .creatures
        .borrow_mut()
        .get_mut(&WOLF)
        .unwrap()
        .instance_id = 42;
    assert_eq!(
        IdleSink::route_of(&world, WOLF)[1].at.y,
        0.0,
        "an intent captured in another instance remained active"
    );
    assert!(apply(
        &mut world,
        WOLF,
        MovementOperation::ReplaceIdle(IdleMovementIntent::Patrol(PatrolIntent { path_id: 7 }))
    ));
    assert_eq!(IdleSink::route_of(&world, WOLF)[1].at.y, 8.0);
}

#[test]
fn follow_facing_walking_and_immobilization_share_the_cycle_writer() {
    let mut world = Scenario::new(SETTLED)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .creature(HUNTER, p(12.0, 0.0, 10.0))
        .awake([WOLF])
        .home(WOLF, p(0.0, 0.0, 10.0), false);
    world.eventai_summon_expiry.borrow_mut().insert(
        WOLF,
        ScenarioSummonExpiry {
            lifetime_ms: 10_000,
            remaining_ms: 10_000,
            last_checked_ms: SETTLED_MS,
            summoner_guid: HUNTER,
        },
    );
    assert!(apply(
        &mut world,
        WOLF,
        MovementOperation::SetFollowMovement(MovementSwitch { enabled: true })
    ));
    assert!(apply(
        &mut world,
        WOLF,
        MovementOperation::SetWalking(WalkingMode::RunByDefault)
    ));
    let tick = world.tick(false, catch_all());
    run_cycle(&mut world, tick);
    assert_eq!(world.effects().len(), 1);
    assert!(
        world.effects()[0].run,
        "the authored default gait was ignored"
    );

    world.advance_clock(2_000_000);
    assert!(apply(
        &mut world,
        WOLF,
        MovementOperation::SetFollowMovement(MovementSwitch { enabled: false })
    ));
    assert!(apply(&mut world, WOLF, MovementOperation::Face(HUNTER)));
    let tick = world.tick(false, catch_all());
    run_cycle(&mut world, tick);
    assert_eq!(world.effects().len(), 2);
    assert!(world.effects()[1].facing);

    world.advance_clock(1_000_000);
    assert!(apply(
        &mut world,
        WOLF,
        MovementOperation::SetImmobilized(ImmobilizationInstruction {
            enabled: true,
            combat_only: false,
        })
    ));
    assert!(apply(
        &mut world,
        WOLF,
        MovementOperation::ReplaceIdle(IdleMovementIntent::RandomAroundCurrentPosition(
            RandomMovementIntent { radius_yd: 15 }
        ))
    ));
    world.rolls.borrow_mut().extend([0, 0, 0]);
    let tick = world.tick(true, catch_all());
    run_cycle(&mut world, tick);
    assert_eq!(world.effects().len(), 3);
    let stop = world.effects()[2];
    assert_eq!(stop.start, stop.dest, "immobilization emitted translation");
}

#[test]
fn combat_movement_suppresses_chase_without_deleting_the_idle_intent() {
    let mut world = Scenario::new(SETTLED)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .awake([WOLF])
        .home(WOLF, p(0.0, 0.0, 10.0), false)
        .player(HUNTER, p(15.0, 0.0, 10.0))
        .attacking(WOLF, HUNTER);
    assert!(apply(
        &mut world,
        WOLF,
        MovementOperation::ReplaceIdle(IdleMovementIntent::Stationary)
    ));
    assert!(apply(
        &mut world,
        WOLF,
        MovementOperation::SetCombatMovement(MovementSwitch { enabled: false })
    ));
    let tick = world.tick(false, catch_all());
    run_cycle(&mut world, tick);
    assert!(world.effects().is_empty());
    assert_eq!(
        world.eventai_movement.borrow()[&WOLF].idle,
        Some(IdleMovementIntent::Stationary)
    );

    let mut casting = Scenario::new(SETTLED)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .channeling(WOLF);
    assert!(!apply(
        &mut casting,
        WOLF,
        MovementOperation::SetCombatMovement(MovementSwitch { enabled: false })
    ));
}

#[test]
fn chase_walk_mode_uses_the_pursuit_seam() {
    let mut world = Scenario::new(SETTLED)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .awake([WOLF])
        .home(WOLF, p(0.0, 0.0, 10.0), false)
        .player(HUNTER, p(15.0, 0.0, 10.0))
        .attacking(WOLF, HUNTER);
    assert!(apply(
        &mut world,
        WOLF,
        MovementOperation::SetWalking(WalkingMode::WalkWhileChasing)
    ));

    let tick = world.tick(false, catch_all());
    run_cycle(&mut world, tick);

    assert_eq!(world.effects().len(), 1);
    assert!(!world.effects()[0].run);
}

#[test]
fn ranged_mode_requires_a_main_spell_before_it_changes_posture() {
    let operation = MovementOperation::SetRangedMode(RangedModeInstruction {
        mode: RangedMode::Proximity,
        distance_yd: 25,
    });
    let mut missing = Scenario::new(SETTLED).creature(WOLF, p(0.0, 0.0, 10.0));
    assert!(!apply(&mut missing, WOLF, operation));
    assert!(missing.authored_ranged_posture(WOLF).is_none());

    let cast = CreatureInstruction::Cast(CastInstruction {
        spell_id: 501,
        target: InstructionTarget::CurrentOpponent,
        interrupt_previous: false,
        start_mode: SpellStartMode::Direct,
        caster_role: SpellCasterRole::Actor,
        target_role: SpellTargetRole::Selected,
        aura_absent: false,
        character_only: false,
        target_must_be_casting: false,
        main_spell: true,
        distance_after_start: false,
    });
    let definition = EventAiDefinition {
        subject: EventAiSubject::Guid(WOLF),
        revision: DefinitionRevision { value: 1 },
        rules: vec![EventAiRule {
            source_rule_id: 1,
            event: EventCondition::TimedInCombat(TimeWindow {
                min_ms: 0,
                max_ms: 0,
            }),
            chance_pct: 100,
            allowed_phases: PhaseSet { bits: u32::MAX },
            recurrence: RecurrencePolicy::Once,
            selection: InstructionSelection::All,
            execution: ExecutionPolicy::Ordinary,
            posture: PostureAdmission::Any,
            instructions: vec![cast],
        }],
    };
    let mut ready = Scenario::new(SETTLED)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .eventai_native_definition(definition);
    assert!(apply(&mut ready, WOLF, operation));
    assert_eq!(ready.authored_ranged_posture(WOLF), Some((25.0, 0.0)));
}

#[test]
fn higher_priority_movers_suspend_and_preserve_random_idle() {
    let random = MovementOperation::ReplaceIdle(IdleMovementIntent::RandomAroundCurrentPosition(
        RandomMovementIntent { radius_yd: 15 },
    ));

    let mut returning = Scenario::new(SETTLED)
        .creature(WOLF, p(20.0, 0.0, 10.0))
        .awake([WOLF])
        .home(WOLF, p(0.0, 0.0, 10.0), true);
    assert!(apply(&mut returning, WOLF, random));
    returning.eventai_returning_home.borrow_mut().insert(WOLF);
    let tick = returning.tick(true, catch_all());
    run_cycle(&mut returning, tick);
    assert_eq!(returning.effects().len(), 1);
    assert!(returning.effects()[0].dest.x < 20.0);
    assert!(matches!(
        returning.eventai_movement.borrow()[&WOLF].idle,
        Some(IdleMovementIntent::RandomAroundCurrentPosition(_))
    ));

    let mut feared = Scenario::new(SETTLED)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .player(HUNTER, p(5.0, 0.0, 10.0))
        .awake([WOLF])
        .home(WOLF, p(0.0, 0.0, 10.0), true)
        .feared_by(WOLF, HUNTER);
    assert!(apply(&mut feared, WOLF, random));
    feared.rolls.borrow_mut().extend([0, 0, 0, 0]);
    let tick = feared.tick(true, catch_all());
    run_cycle(&mut feared, tick);
    assert_eq!(feared.effects().len(), 1);
    assert!(matches!(
        feared.eventai_movement.borrow()[&WOLF].idle,
        Some(IdleMovementIntent::RandomAroundCurrentPosition(_))
    ));

    let mut retreating = Scenario::new(SETTLED)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .player(HUNTER, p(5.0, 0.0, 10.0))
        .awake([WOLF])
        .home(WOLF, p(0.0, 0.0, 10.0), true)
        .attacking(WOLF, HUNTER)
        .wounded_runner(WOLF);
    assert!(apply(&mut retreating, WOLF, random));
    EventAiWorld::stamp_eventai_rout(&mut retreating, WOLF, (SETTLED_MS + 10_000) as u32);
    let tick = retreating.tick(false, catch_all());
    run_cycle(&mut retreating, tick);
    assert_eq!(retreating.effects().len(), 1);
    assert!(matches!(
        retreating.eventai_movement.borrow()[&WOLF].idle,
        Some(IdleMovementIntent::RandomAroundCurrentPosition(_))
    ));
}

#[test]
fn lifecycle_cleanup_and_revision_adoption_drop_old_movement_intent() {
    let mut world = Scenario::new(SETTLED).creature(WOLF, p(0.0, 0.0, 10.0));
    assert!(apply(
        &mut world,
        WOLF,
        MovementOperation::ReplaceIdle(IdleMovementIntent::Stationary)
    ));
    EventAiWorld::adopt_eventai_revision(&mut world, WOLF, DefinitionRevision { value: 9 });
    assert!(!world.eventai_movement.borrow().contains_key(&WOLF));

    let mut despawned = Scenario::new(SETTLED).creature(WOLF, p(0.0, 0.0, 10.0));
    assert!(apply(
        &mut despawned,
        WOLF,
        MovementOperation::ReplaceIdle(IdleMovementIntent::Stationary)
    ));
    despawned.clear_eventai_summon(WOLF);
    assert!(!despawned.eventai_movement.borrow().contains_key(&WOLF));
}

#[test]
fn refused_chase_navigation_produces_target_not_reachable() {
    let definition = EventAiDefinition {
        subject: EventAiSubject::Guid(WOLF),
        revision: DefinitionRevision { value: 1 },
        rules: vec![EventAiRule {
            source_rule_id: 36,
            event: EventCondition::TargetNotReachable,
            chance_pct: 100,
            allowed_phases: PhaseSet { bits: u32::MAX },
            recurrence: RecurrencePolicy::RepeatOnEvent,
            selection: InstructionSelection::All,
            execution: ExecutionPolicy::Ordinary,
            posture: PostureAdmission::Any,
            instructions: vec![CreatureInstruction::Speak(SpeakInstruction {
                mode: SpeechMode::Say,
                broadcast_ids: vec![900],
                legacy_text: String::new(),
                target: InstructionTarget::SelfActor,
            })],
        }],
    };
    let mut world = Scenario::new(SETTLED)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .awake([WOLF])
        .home(WOLF, p(0.0, 0.0, 10.0), false)
        .player(HUNTER, p(15.0, 0.0, 10.0))
        .attacking(WOLF, HUNTER)
        .detour(WOLF, (0.0, 0.0))
        .eventai_native_definition(definition)
        .eventai_broadcast(900, "Blocked", crate::chat::CHAT_SAY);
    world.rolls.borrow_mut().extend([0, 0]);

    let tick = world.tick(false, catch_all());
    run_cycle(&mut world, tick);

    assert_eq!(
        world.eventai_speech(),
        vec![(WOLF, crate::chat::CHAT_SAY, "Blocked".into())]
    );
    assert!(world.effects().is_empty());
}
