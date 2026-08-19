//! EventAI summon actions and ranged posture.

use spacetimedb::ReducerContext;

use super::{ActionResult, EventContext, RuleAction};

pub(super) fn execute(
    _ctx: &ReducerContext,
    _context: &EventContext,
    _action: &RuleAction,
) -> ActionResult {
    ActionResult::Unsupported
}
