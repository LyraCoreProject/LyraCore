//! EventAI's typed path into quest authority.

use super::engine::EventAiWorld;
use super::{ActionResult, CreatureInstruction, EventContext};
use crate::quest::{EventAiQuestCreditContext, QuestCreditOutcome};

pub(super) fn execute<W: EventAiWorld>(
    world: &mut W,
    context: &EventContext,
    instruction: &CreatureInstruction,
) -> ActionResult {
    let CreatureInstruction::QuestCredit(request) = instruction else {
        return ActionResult::Unsupported;
    };
    let Some(source) = world.eventai_unit(context.creature_guid) else {
        return ActionResult::Refused;
    };
    let threat_characters = world
        .eventai_threat(source.guid)
        .into_iter()
        .filter_map(|(guid, _)| world.eventai_unit(guid))
        .filter(|unit| unit.is_player)
        .map(|unit| unit.guid)
        .collect();
    let credit_context = EventAiQuestCreditContext {
        source_x: source.x,
        source_y: source.y,
        source_map_id: source.map_id,
        source_instance_id: source.instance_id,
        selected_character: context.invoker_guid,
        invoker_beneficiary: context.beneficiary_guid,
        threat_characters,
    };
    match world.eventai_credit_quest(*request, credit_context) {
        QuestCreditOutcome::Applied => ActionResult::Applied,
        QuestCreditOutcome::Refused => ActionResult::Refused,
    }
}
