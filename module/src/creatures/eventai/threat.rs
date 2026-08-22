//! EventAI percent-threat instructions.

use super::engine::EventAiWorld;
use super::{
    ActionResult, CreatureInstruction, EventContext, ScaleAllThreatInstruction,
    ScaleSelectedThreatInstruction,
};

pub(super) fn execute<W: EventAiWorld>(
    world: &mut W,
    context: &EventContext,
    instruction: &CreatureInstruction,
    choice: u64,
) -> ActionResult {
    match instruction {
        CreatureInstruction::ScaleSelectedThreat(ScaleSelectedThreatInstruction {
            percent,
            target,
        }) => {
            let Some(source_guid) =
                super::combat::unit_target(world, context, *target, None, choice)
            else {
                return ActionResult::Refused;
            };
            world.eventai_scale_selected_threat(crate::threat::ScaleSelectedThreat {
                creature_guid: context.creature_guid,
                source_guid,
                percent: *percent,
            });
            ActionResult::Applied
        }
        CreatureInstruction::ScaleAllThreat(ScaleAllThreatInstruction { percent }) => {
            world.eventai_scale_all_threat(crate::threat::ScaleAllThreat {
                creature_guid: context.creature_guid,
                percent: *percent,
            });
            ActionResult::Applied
        }
        _ => ActionResult::Unsupported,
    }
}
