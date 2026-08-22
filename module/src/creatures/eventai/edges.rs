//! EventAI edge dispatch and lifecycle resets.

use std::collections::HashSet;

use spacetimedb::ReducerContext;

use super::{EventAiRequest, EventCondition, EventContext, EventKind};
use crate::{game_creature_ai_rule_state, game_creature_ai_state, game_world_entity, WorldEntity};

/// Who runs its entry's EventAI at all. A Character carries no rules, and a pet answers its
/// owner's commands rather than the wild entry it was built from, so a tamed beast must neither
/// evaluate its entry's rules nor keep rule state of its own.
pub(crate) fn runs_eventai(creature: &WorldEntity) -> bool {
    !creature.is_player() && creature.owner_guid == 0
}

crate::game_hook!(on_aggro, fn creature_ai_on_aggro(ctx, payload) {
    evaluate_edge(
        ctx,
        EventKind::OnAggro,
        payload.creature_guid,
        Some(payload.target_guid),
        Some(payload.target_guid),
        Some(payload.target_guid),
        payload.assist,
    );
});

crate::game_hook!(on_creature_spawn, fn creature_ai_on_creature_spawn(ctx, payload) {
    let Some(creature) = ctx.db.game_world_entity().guid().find(payload.guid) else {
        return;
    };
    if !runs_eventai(&creature) {
        return;
    }
    reset_creature_lifecycle(ctx, payload.guid);
    evaluate_edge(
        ctx,
        EventKind::OnSpawn,
        payload.guid,
        None,
        None,
        None,
        false,
    );
});

crate::game_hook!(on_creature_death, fn creature_ai_on_creature_death(ctx, payload) {
    evaluate_edge(
        ctx,
        EventKind::OnDeath,
        payload.creature_guid,
        (payload.killer_guid != 0).then_some(payload.killer_guid),
        (payload.killer_guid != 0).then_some(payload.killer_guid),
        None,
        false,
    );
    reset_creature_lifecycle(ctx, payload.creature_guid);
});

/// Start the creature's next engagement: bump `engagement_id`, drop the phase and ranged posture,
/// and clear the rule state every engagement rule earned in the fight that just ended. Once-only
/// aggro rules re-arm and timed rules wait their initial window again on the next pull. Spawn and
/// death rules keep their state, moved onto the new engagement, because they belong to the
/// lifecycle instead. Called however a fight ends, so no path can leave a creature half-armed.
pub(crate) fn reset_engagement(ctx: &ReducerContext, creature_guid: u64) {
    let Some(creature) = ctx.db.game_world_entity().guid().find(creature_guid) else {
        return;
    };
    if !runs_eventai(&creature) {
        return;
    }

    let definition = super::combat::definition_for(ctx, creature_guid);
    let known_rule_ids: HashSet<u64> = definition
        .rules
        .iter()
        .map(|rule| rule.source_rule_id)
        .collect();
    let engagement_rule_ids: HashSet<u64> = definition
        .rules
        .iter()
        .filter(|rule| {
            !matches!(
                rule.event,
                EventCondition::OnDeath | EventCondition::OnSpawn
            )
        })
        .map(|rule| rule.source_rule_id)
        .collect();
    let states = ctx.db.game_creature_ai_state();
    let engagement_id = states.creature_guid().find(creature_guid).map(|mut state| {
        state.engagement_id = state.engagement_id.saturating_add(1);
        state.phase = 0;
        state.ranged_distance = 0.0;
        state.ranged_angle = 0.0;
        state.ranged_posture_active = false;
        let state = states.creature_guid().update(state);
        state.engagement_id
    });
    let rule_state = ctx.db.game_creature_ai_rule_state();
    for mut state in rule_state
        .by_creature()
        .filter(&creature_guid)
        .collect::<Vec<_>>()
    {
        if !known_rule_ids.contains(&state.source_rule_id)
            || engagement_rule_ids.contains(&state.source_rule_id)
        {
            rule_state.id().delete(state.id);
        } else if let Some(engagement_id) = engagement_id {
            state.engagement_id = engagement_id;
            rule_state.id().update(state);
        }
    }
}

fn evaluate_edge(
    ctx: &ReducerContext,
    kind: EventKind,
    creature_guid: u64,
    invoker_guid: Option<u64>,
    event_target_guid: Option<u64>,
    current_target_guid: Option<u64>,
    assisted: bool,
) {
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64;
    super::evaluate_context(
        ctx,
        EventAiRequest::Edge(EventContext {
            kind,
            creature_guid,
            invoker_guid,
            event_target_guid,
            current_target_guid,
            assisted,
            now_ms,
        }),
    );
}

pub(crate) fn reset_creature_lifecycle(ctx: &ReducerContext, creature_guid: u64) {
    let rule_state = ctx.db.game_creature_ai_rule_state();
    for state in rule_state
        .by_creature()
        .filter(&creature_guid)
        .collect::<Vec<_>>()
    {
        rule_state.id().delete(state.id);
    }

    ctx.db
        .game_creature_ai_state()
        .creature_guid()
        .delete(creature_guid);
}

#[cfg(test)]
mod eventai_gate_tripwire {
    use crate::test_scan::code_of;

    /// The three Gates below decide WHICH creature EventAI touches and WHERE its speech leaves the
    /// Module. Each reads durable state through a `ReducerContext`, which this crate has no test
    /// harness for, and each fails silently when it is dropped: a tamed beast quietly runs its wild
    /// entry's rules and never clears its rule state, and a creature line quietly skips the
    /// dead-speaker Gate and the length cap that every other spoken line goes through.
    #[test]
    fn every_eventai_entry_point_keeps_its_gate() {
        for (source, signature, needle, why) in [
            (
                include_str!("engine.rs"),
                "fn eventai_fights(&self, scope: &TickScope) -> Vec<EngagedFight> {",
                "super::runs_eventai(&creature)",
                "a pet must produce no evaluation context of its own",
            ),
            (
                include_str!("combat.rs"),
                "pub(super) fn definition_for(",
                "super::runs_eventai(&creature)",
                "the one definition fetch answers `who runs EventAI` for the engine, the edges and the \
                 cycle's authored-combat reads alike",
            ),
            (
                include_str!("edges.rs"),
                "pub(crate) fn reset_engagement(",
                "runs_eventai(&creature)",
                "a Character and a pet carry no engagement to reset",
            ),
            (
                include_str!("engine.rs"),
                "fn eventai_definition(&self, creature_guid: u64) -> EventAiDefinition {",
                "super::combat::definition_for(",
                "one entry-and-guid definition fetch, not a second copy that drifts from the gated one",
            ),
            (
                include_str!("engine.rs"),
                concat!(
                    "fn eventai_deliver_line(\n",
                    "        &mut self,\n",
                    "        speaker_guid: u64,\n",
                    "        chat_type: u8,\n",
                    "        language: u8,\n",
                    "        message: String,\n",
                    "    ) -> bool {",
                ),
                "crate::chat::apply_send_chat(",
                "the say/yell chokepoint owns the dead-speaker Gate and the length cap",
            ),
        ] {
            assert!(
                code_of(source, signature).contains(needle),
                "`{signature}` no longer contains `{needle}` — {why}"
            );
        }
    }
}
