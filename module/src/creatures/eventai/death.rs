//! EventAI requests owned by combat death authority.

use super::engine::EventAiWorld;
use super::{ActionResult, CreatureInstruction, EventContext};

pub(super) fn execute<W: EventAiWorld>(
    world: &mut W,
    context: &EventContext,
    instruction: &CreatureInstruction,
) -> ActionResult {
    match instruction {
        CreatureInstruction::SetLethalDamageFloor(floor) => {
            let revision = world
                .eventai_creature_state(context.creature_guid)
                .definition_revision;
            if world.eventai_set_lethal_damage_floor(context.creature_guid, revision, floor.enabled)
            {
                ActionResult::Applied
            } else {
                ActionResult::Refused
            }
        }
        CreatureInstruction::ForceDeath => {
            if world.eventai_force_death(context.creature_guid) {
                ActionResult::Applied
            } else {
                ActionResult::Refused
            }
        }
        _ => ActionResult::Unsupported,
    }
}
