//! Quest-credit EventAI scenarios.

use super::*;
use crate::creatures::eventai::*;
use crate::quest::{
    EventAiQuestCredit, QuestCreditOutcome, QuestCreditRecipientPolicy, QuestEvent,
};

const CREATURE: u64 = 8_301;
const TARGET: u64 = 8_302;
const ENTRY: u32 = 931;

fn point(x: f32) -> Point {
    Point { x, y: 0.0, z: 0.0 }
}

fn credit_definition(request: EventAiQuestCredit) -> EventAiDefinition {
    let rule = EventAiRule {
        source_rule_id: 1,
        event: EventCondition::OnAggro,
        chance_pct: 100,
        allowed_phases: PhaseSet { bits: u32::MAX },
        recurrence: RecurrencePolicy::Once,
        selection: InstructionSelection::All,
        execution: ExecutionPolicy::Ordinary,
        posture: PostureAdmission::Any,
        instructions: vec![CreatureInstruction::QuestCredit(request)],
    };
    EventAiDefinition {
        subject: EventAiSubject::Entry(ENTRY),
        revision: normalized_revision(EventAiSubject::Entry(ENTRY), std::slice::from_ref(&rule)),
        rules: vec![rule],
    }
}

fn fire(scenario: &mut Scenario) {
    evaluate(
        scenario,
        EventAiRequest::Edge(EventContext {
            invoker_guid: Some(TARGET),
            invoker_is_player: Some(true),
            beneficiary_guid: Some(TARGET),
            ..EventContext::empty(EventKind::OnAggro, CREATURE, 1_000)
        }),
    );
}

#[test]
fn quest_event_observes_an_applied_quest_outcome() {
    let request = EventAiQuestCredit::QuestEvent(QuestEvent {
        quest_entry: 8_353,
        recipient_policy: QuestCreditRecipientPolicy::SelectedCharacter,
    });
    let mut scenario = Scenario::new(1_000_000)
        .creature(CREATURE, point(0.0))
        .entry(CREATURE, ENTRY)
        .player(TARGET, point(2.0))
        .eventai_native_definition(credit_definition(request));

    fire(&mut scenario);

    assert_eq!(
        scenario.eventai_quest_credit_results(),
        vec![QuestCreditOutcome::Applied]
    );
}

#[test]
fn a_quest_authority_refusal_does_not_retarget_credit() {
    let request = EventAiQuestCredit::QuestEvent(QuestEvent {
        quest_entry: 8_353,
        recipient_policy: QuestCreditRecipientPolicy::SelectedCharacter,
    });
    let mut scenario = Scenario::new(1_000_000)
        .creature(CREATURE, point(0.0))
        .entry(CREATURE, ENTRY)
        .player(TARGET, point(2.0))
        .eventai_quest_credit_outcome(QuestCreditOutcome::Refused)
        .eventai_native_definition(credit_definition(request));

    fire(&mut scenario);

    assert_eq!(
        scenario.eventai_quest_credit_results(),
        vec![QuestCreditOutcome::Refused]
    );
}
