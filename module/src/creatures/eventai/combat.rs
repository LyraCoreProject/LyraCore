//! Engaged EventAI conditions and combat actions.

use spacetimedb::{ReducerContext, Table};

use super::{ActionResult, EventContext, RuleAction, TargetPolicy};
use crate::creatures::ai::TickScope;
use crate::{game_melee_attack, game_world_entity};

pub(super) fn engaged_contexts(
    ctx: &ReducerContext,
    scope: &TickScope,
    now_ms: u64,
) -> Vec<EventContext> {
    let entities = ctx.db.game_world_entity();
    ctx.db
        .game_melee_attack()
        .iter()
        .filter_map(|fight| {
            let creature = entities.guid().find(fight.attacker_guid)?;
            (!creature.is_player() && !creature.dead && scope.covers(creature.instance_id))
                .then_some(EventContext {
                    kind: super::EventKind::TimedInCombat,
                    creature_guid: creature.guid,
                    invoker_guid: Some(fight.target_guid),
                    event_target_guid: Some(fight.target_guid),
                    current_target_guid: Some(fight.target_guid),
                    assisted: false,
                    now_ms,
                })
        })
        .collect()
}

pub(super) fn execute(
    _ctx: &ReducerContext,
    _context: &EventContext,
    _action: &RuleAction,
) -> ActionResult {
    ActionResult::Unsupported
}

pub(super) fn target(
    _ctx: &ReducerContext,
    _context: &EventContext,
    _policy: TargetPolicy,
) -> Option<u64> {
    None
}
