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
    let credit_context = EventAiQuestCreditContext {
        source_creature_guid: source.guid,
        source_x: source.x,
        source_y: source.y,
        source_map_id: source.map_id,
        source_instance_id: source.instance_id,
        selected_character: context.invoker_guid,
        invoker_beneficiary: context.beneficiary_guid,
    };
    match world.eventai_credit_quest(*request, credit_context) {
        QuestCreditOutcome::Applied => ActionResult::Applied,
        QuestCreditOutcome::Refused => ActionResult::Refused,
    }
}
