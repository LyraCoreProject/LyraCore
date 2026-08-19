//! EventAI edge dispatch and lifecycle resets.

use std::collections::HashSet;

use spacetimedb::ReducerContext;

use super::{
    effective_rule_id, EventAiRequest, EventContext, EventKind, EVENT_ON_DEATH, EVENT_ON_SPAWN,
};
use crate::{
    game_creature_ai_event, game_creature_ai_rule_state, game_creature_ai_state, game_world_entity,
};

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
    if creature.owner_guid != 0 {
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

pub(crate) fn reset_engagement(ctx: &ReducerContext, creature_guid: u64) {
    let Some(creature) = ctx.db.game_world_entity().guid().find(creature_guid) else {
        return;
    };
    if creature.is_player() || creature.owner_guid != 0 {
        return;
    }

    let rules = rules_for(ctx, creature_guid, creature.entry);
    let known_rule_ids: HashSet<u64> = rules.iter().map(effective_rule_id).collect();
    let engagement_rule_ids: HashSet<u64> = rules
        .iter()
        .filter(|rule| !matches!(rule.event_type, EVENT_ON_DEATH | EVENT_ON_SPAWN))
        .map(effective_rule_id)
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

fn rules_for(
    ctx: &ReducerContext,
    creature_guid: u64,
    creature_entry: u32,
) -> Vec<super::CreatureAiEvent> {
    let rules = ctx.db.game_creature_ai_event();
    rules
        .by_entry()
        .filter(&creature_entry)
        .chain(rules.by_guid().filter(&creature_guid))
        .collect()
}
