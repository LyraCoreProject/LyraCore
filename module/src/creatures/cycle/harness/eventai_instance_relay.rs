use super::*;
use crate::creatures::eventai::*;
use crate::encounter::{EncounterBinding, EncounterSignal};

const CREATURE: u64 = 8_701;
const ENTRY: u32 = 4_832;

fn definition(instruction: CreatureInstruction) -> EventAiDefinition {
    let subject = EventAiSubject::Entry(ENTRY);
    let rules = vec![EventAiRule {
        source_rule_id: 870_001,
        event: EventCondition::OnDeath(DeathCondition {
            predicate: EventPredicate::Always,
        }),
        chance_pct: 100,
        allowed_phases: PhaseSet { bits: u32::MAX },
        recurrence: RecurrencePolicy::Once,
        selection: InstructionSelection::All,
        execution: ExecutionPolicy::Ordinary,
        posture: PostureAdmission::Any,
        instructions: vec![instruction],
    }];
    EventAiDefinition {
        subject,
        revision: normalized_revision(subject, &rules),
        rules,
    }
}

fn fire(world: &mut Scenario) {
    evaluate(
        world,
        EventAiRequest::Edge(EventContext::empty(EventKind::OnDeath, CREATURE, 1_000)),
    );
}

#[test]
fn named_encounter_notification_crosses_the_world_seam_only_on_its_bound_map() {
    let instruction = CreatureInstruction::NotifyEncounter(NotifyEncounterInstruction {
        binding: EncounterBinding::BlackfathomDeepsKelris,
        signal: EncounterSignal::Complete,
    });
    let mut world = Scenario::new(1_000_000)
        .creature(
            CREATURE,
            Point {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
        )
        .entry(CREATURE, ENTRY)
        .in_instance(CREATURE, 9)
        .tweak(CREATURE, |creature| creature.map_id = 48)
        .eventai_native_definition(definition(instruction.clone()));

    fire(&mut world);

    assert_eq!(
        world.eventai_encounter_notifications.borrow().as_slice(),
        &[(
            CREATURE,
            NotifyEncounterInstruction {
                binding: EncounterBinding::BlackfathomDeepsKelris,
                signal: EncounterSignal::Complete,
            },
        )]
    );

    let mut wrong_map = Scenario::new(1_000_000)
        .creature(
            CREATURE,
            Point {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
        )
        .entry(CREATURE, ENTRY)
        .in_instance(CREATURE, 9)
        .tweak(CREATURE, |creature| creature.map_id = 33)
        .eventai_native_definition(definition(instruction));
    fire(&mut wrong_map);
    assert!(wrong_map
        .eventai_encounter_notifications
        .borrow()
        .is_empty());
}

#[test]
fn random_relay_selection_keeps_the_invocations_linked_random_state() {
    let instruction = CreatureInstruction::StartRelay(StartRelayInstruction {
        relay_ids: vec![9_989, 9_990, 9_991],
        target: InstructionTarget::SelfActor,
        catalogue_version: 1,
    });
    let mut world = Scenario::new(1_000_000)
        .creature(
            CREATURE,
            Point {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
        )
        .entry(CREATURE, ENTRY)
        .eventai_native_definition(definition(instruction))
        .rolls([4]);

    fire(&mut world);

    let starts = world.eventai_relay_starts.borrow();
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0].0, 9_990);
    assert_eq!(starts[0].1, CREATURE);
    assert_eq!(starts[0].2, CREATURE);
    assert_eq!(starts[0].3 % 3, 1);
}
