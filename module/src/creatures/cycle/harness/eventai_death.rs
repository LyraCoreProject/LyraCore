//! EventAI death-authority scenarios.

use super::*;
use crate::creatures::eventai::*;

const CREATURE: u64 = 8_401;
const ENTRY: u32 = 941;

fn point(x: f32) -> Point {
    Point { x, y: 0.0, z: 0.0 }
}

fn rule(source_rule_id: u64, instructions: Vec<CreatureInstruction>) -> EventAiRule {
    EventAiRule {
        source_rule_id,
        event: EventCondition::OnAggro,
        chance_pct: 100,
        allowed_phases: PhaseSet { bits: u32::MAX },
        recurrence: RecurrencePolicy::Once,
        selection: InstructionSelection::All,
        execution: ExecutionPolicy::Ordinary,
        posture: PostureAdmission::Any,
        instructions,
    }
}

fn definition(instructions: Vec<CreatureInstruction>) -> EventAiDefinition {
    let subject = EventAiSubject::Entry(ENTRY);
    let rules = vec![rule(840_001, instructions)];
    EventAiDefinition {
        subject,
        revision: normalized_revision(subject, &rules),
        rules,
    }
}

fn scenario(definition: EventAiDefinition) -> Scenario {
    Scenario::new(1_000_000)
        .creature(CREATURE, point(0.0))
        .entry(CREATURE, ENTRY)
        .eventai_native_definition(definition)
}

fn aggro(scenario: &mut Scenario) {
    evaluate(
        scenario,
        EventAiRequest::Edge(EventContext {
            engaged: true,
            ..EventContext::empty(EventKind::OnAggro, CREATURE, 1_000)
        }),
    );
}

#[test]
fn lethal_floor_is_revision_owned_and_disable_does_not_kill() {
    let first = definition(vec![CreatureInstruction::SetLethalDamageFloor(
        SetLethalDamageFloorInstruction { enabled: true },
    )]);
    let mut world = scenario(first);

    aggro(&mut world);
    assert_eq!(
        world.eventai_lethal_floor(CREATURE),
        Some(
            world
                .eventai_creature_state
                .borrow()
                .get(&CREATURE)
                .unwrap()
                .definition_revision
        )
    );
    assert!(world.eventai_forced_deaths().is_empty());

    let replacement = definition(vec![CreatureInstruction::SetLethalDamageFloor(
        SetLethalDamageFloorInstruction { enabled: false },
    )]);
    world.replace_eventai_definition(replacement);
    aggro(&mut world);

    assert_eq!(world.eventai_lethal_floor(CREATURE), None);
    assert!(world.eventai_forced_deaths().is_empty());
}

#[test]
fn forced_death_bypasses_and_clears_the_floor() {
    let mut world = scenario(definition(vec![
        CreatureInstruction::SetLethalDamageFloor(SetLethalDamageFloorInstruction {
            enabled: true,
        }),
        CreatureInstruction::ForceDeath,
    ]));

    aggro(&mut world);

    assert_eq!(world.eventai_forced_deaths(), vec![CREATURE]);
    assert_eq!(world.eventai_lethal_floor(CREATURE), None);
    assert!(world.corpses.borrow().contains(&CREATURE));
}
