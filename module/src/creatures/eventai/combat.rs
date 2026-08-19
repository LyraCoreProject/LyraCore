//! Engaged EventAI conditions and combat actions.

use std::cmp::Ordering;
use std::collections::HashSet;

use spacetimedb::{ReducerContext, Table};

use super::{
    effective_rule_id, ActionKind, ActionResult, CreatureState, EventContext, EventKind, Rule,
    RuleAction, TargetPolicy, CAST_AURA_ABSENT, CAST_INTERRUPT_PREVIOUS, CAST_PLAYER_ONLY,
    CAST_TARGET_CASTING, CAST_TRIGGERED,
};
use crate::creatures::ai::{rout_close_ms, TickScope};
use crate::{
    game_creature_ai_event, game_creature_ai_rule_state, game_creature_ai_state,
    game_faction_template, game_melee_attack, game_pending_cast, game_threat, game_world_entity,
};

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
            (super::runs_eventai(&creature) && !creature.dead && scope.covers(creature.instance_id))
                .then_some((fight, creature.guid))
        })
        .flat_map(|(fight, creature_guid)| {
            [
                EventKind::TimedInCombat,
                EventKind::CreatureHp,
                EventKind::TargetRange,
                EventKind::FriendlyHpDeficit,
            ]
            .into_iter()
            .map(move |kind| EventContext {
                kind,
                creature_guid,
                invoker_guid: Some(fight.target_guid),
                event_target_guid: Some(fight.target_guid),
                current_target_guid: Some(fight.target_guid),
                assisted: false,
                now_ms,
            })
        })
        .collect()
}

pub(super) fn condition(
    ctx: &ReducerContext,
    context: &EventContext,
    rule: &Rule,
) -> Option<EventContext> {
    let creature = ctx
        .db
        .game_world_entity()
        .guid()
        .find(context.creature_guid)?;
    match context.kind {
        EventKind::TimedInCombat => Some(*context),
        EventKind::CreatureHp => in_inclusive_pct(
            creature.health,
            creature.max_health,
            rule.event_params[0],
            rule.event_params[1],
        )
        .then_some(*context),
        EventKind::TargetRange => {
            let target = ctx
                .db
                .game_world_entity()
                .guid()
                .find(context.current_target_guid?)?;
            let distance = distance_yd(&creature, &target);
            (distance >= rule.event_params[0] as f32 && distance <= rule.event_params[1] as f32)
                .then_some(*context)
        }
        EventKind::FriendlyHpDeficit => {
            wounded_friendly(ctx, &creature, rule.event_params[1], rule.event_params[0]).map(
                |guid| EventContext {
                    event_target_guid: Some(guid),
                    ..*context
                },
            )
        }
        EventKind::OnAggro | EventKind::OnDeath | EventKind::OnSpawn => Some(*context),
    }
}

pub(super) fn execute(
    ctx: &ReducerContext,
    context: &EventContext,
    action: &RuleAction,
) -> ActionResult {
    match action.kind {
        ActionKind::Emote => {
            let Some(creature) = ctx
                .db
                .game_world_entity()
                .guid()
                .find(context.creature_guid)
            else {
                return ActionResult::Refused;
            };
            let target = target(ctx, context, action).unwrap_or(0);
            crate::chat::apply_send_emote(ctx, creature, 0, action.params[0], target)
                .map_or(ActionResult::Refused, |_| ActionResult::Applied)
        }
        ActionKind::FleeForAssist => {
            let melee = ctx.db.game_melee_attack();
            let Some(mut fight) = melee.attacker_guid().find(context.creature_guid) else {
                return ActionResult::Refused;
            };
            if fight.rout_ends_ms == 0 {
                fight.rout_ends_ms = rout_close_ms(context.now_ms as u32);
                melee.attacker_guid().update(fight);
            }
            ActionResult::Applied
        }
        ActionKind::CallForHelp => call_for_help(ctx, context, action.params[0]),
        _ => ActionResult::Unsupported,
    }
}

pub(super) fn target(
    ctx: &ReducerContext,
    context: &EventContext,
    action: &RuleAction,
) -> Option<u64> {
    let creature = ctx
        .db
        .game_world_entity()
        .guid()
        .find(context.creature_guid)?;
    let target = match action.target {
        TargetPolicy::Current => context.current_target_guid,
        TargetPolicy::SelfActor => Some(context.creature_guid),
        TargetPolicy::Invoker => context.invoker_guid,
        TargetPolicy::EventTarget => context.event_target_guid,
        TargetPolicy::TopThreat => ranked_threat(ctx, &creature, action).first().copied(),
        TargetPolicy::SecondThreat => ranked_threat(ctx, &creature, action).get(1).copied(),
        TargetPolicy::RandomThreat => pick(ctx, &ranked_threat(ctx, &creature, action)),
        TargetPolicy::TopThreatPlayer => ranked_threat(ctx, &creature, action).first().copied(),
        TargetPolicy::RandomThreatPlayer => pick(ctx, &ranked_threat(ctx, &creature, action)),
        TargetPolicy::NearestArea => nearest_area(ctx, &creature, action),
        TargetPolicy::FarthestHostile => farthest_hostile(ctx, &creature, action),
    }?;
    if action.cast_options.contains(CAST_PLAYER_ONLY)
        && !ctx.db.game_world_entity().guid().find(target)?.is_player()
    {
        return None;
    }
    Some(target)
}

pub(super) fn cast(
    ctx: &ReducerContext,
    context: &EventContext,
    action: &RuleAction,
    target_guid: u64,
) -> ActionResult {
    if action.cast_options.contains(CAST_TRIGGERED) {
        return ActionResult::Refused;
    }
    if action.cast_options.contains(CAST_PLAYER_ONLY)
        && !ctx
            .db
            .game_world_entity()
            .guid()
            .find(target_guid)
            .is_some_and(|target| target.is_player())
    {
        return ActionResult::Refused;
    }
    if action.cast_options.contains(CAST_AURA_ABSENT)
        && crate::spell::has_aura(ctx, target_guid, action.params[0])
    {
        return ActionResult::Refused;
    }
    if action.cast_options.contains(CAST_TARGET_CASTING)
        && ctx
            .db
            .game_pending_cast()
            .by_caster()
            .filter(&target_guid)
            .next()
            .is_none()
    {
        return ActionResult::Refused;
    }
    let Some(creature) = ctx
        .db
        .game_world_entity()
        .guid()
        .find(context.creature_guid)
    else {
        return ActionResult::Refused;
    };
    let pending = ctx
        .db
        .game_pending_cast()
        .by_caster()
        .filter(&creature.guid)
        .next()
        .is_some();
    if pending {
        if !action.cast_options.contains(CAST_INTERRUPT_PREVIOUS) {
            return ActionResult::Refused;
        }
        crate::spell::interrupt_cast(ctx, creature.guid);
    }
    crate::spell::begin_cast(
        ctx,
        creature.guid,
        action.params[0],
        creature.level as u8,
        target_guid,
        false,
        None,
    )
    .map_or(ActionResult::Refused, |_| ActionResult::Applied)
}

pub(crate) fn suppresses_flat_cast(ctx: &ReducerContext, creature_guid: u64) -> bool {
    applicable_engaged_rules(ctx, creature_guid)
        .into_iter()
        .flat_map(|rule| rule.actions)
        .any(|action| action.kind == ActionKind::Cast)
}

pub(crate) fn suppresses_fixed_rout(ctx: &ReducerContext, creature_guid: u64) -> bool {
    applicable_engaged_rules(ctx, creature_guid)
        .into_iter()
        .flat_map(|rule| rule.actions)
        .any(|action| action.kind == ActionKind::FleeForAssist)
}

pub(crate) fn applicable_rules(ctx: &ReducerContext, creature_guid: u64) -> Vec<Rule> {
    let state = ctx
        .db
        .game_creature_ai_state()
        .creature_guid()
        .find(creature_guid)
        .map_or_else(CreatureState::default, CreatureState::from);
    let mut grouped = std::collections::BTreeMap::new();
    for row in rows_for(ctx, creature_guid) {
        grouped
            .entry(effective_rule_id(&row))
            .or_insert_with(Vec::new)
            .push(row);
    }
    let consumed_rule_ids: HashSet<u64> = ctx
        .db
        .game_creature_ai_rule_state()
        .by_creature()
        .filter(&creature_guid)
        .filter(|rule_state| {
            rule_state.lifecycle_id == state.lifecycle_id
                && rule_state.engagement_id == state.engagement_id
                && rule_state.consumed
        })
        .map(|rule_state| rule_state.source_rule_id)
        .collect();
    grouped
        .into_values()
        .filter_map(|rows| Rule::decode(rows).ok())
        .filter(|rule| state.phase < 32 && rule.allowed_phase_mask & (1u32 << state.phase) != 0)
        .filter(|rule| !consumed_rule_ids.contains(&rule.id))
        .collect()
}

fn applicable_engaged_rules(ctx: &ReducerContext, creature_guid: u64) -> Vec<Rule> {
    applicable_rules(ctx, creature_guid)
        .into_iter()
        .filter(|rule| {
            matches!(
                rule.event,
                EventKind::TimedInCombat
                    | EventKind::CreatureHp
                    | EventKind::TargetRange
                    | EventKind::FriendlyHpDeficit
            )
        })
        .collect()
}

/// Every EventAI row that governs one creature: its entry's rules plus any pinned to its own guid.
/// The single row fetch behind the engine, the lifecycle edges and the cycle's suppression reads,
/// so the "who runs EventAI at all" Gate is answered once, here.
pub(super) fn rows_for(ctx: &ReducerContext, creature_guid: u64) -> Vec<super::CreatureAiEvent> {
    let Some(creature) = ctx.db.game_world_entity().guid().find(creature_guid) else {
        return Vec::new();
    };
    if !super::runs_eventai(&creature) {
        return Vec::new();
    }
    let rules = ctx.db.game_creature_ai_event();
    rules
        .by_entry()
        .filter(&creature.entry)
        .chain(rules.by_guid().filter(&creature_guid))
        .collect()
}

fn in_inclusive_pct(value: u32, max: u32, min_pct: u32, max_pct: u32) -> bool {
    max != 0
        && u64::from(value) * 100 >= u64::from(min_pct) * u64::from(max)
        && u64::from(value) * 100 <= u64::from(max_pct) * u64::from(max)
}

fn wounded_friendly(
    ctx: &ReducerContext,
    creature: &crate::WorldEntity,
    radius: u32,
    deficit: u32,
) -> Option<u64> {
    crate::helpers::entities_near(
        ctx,
        creature.map_id,
        creature.instance_id,
        creature.x,
        creature.y,
        radius as f32,
    )
    .into_iter()
    .filter(|other| {
        !other.dead
            && (other.guid == creature.guid || friendly(ctx, creature, other))
            && distance_yd(creature, other) <= radius as f32
            && other.max_health.saturating_sub(other.health) >= deficit
    })
    .min_by(|a, b| {
        let a_missing = a.max_health - a.health;
        let b_missing = b.max_health - b.health;
        b_missing.cmp(&a_missing).then(a.guid.cmp(&b.guid))
    })
    .map(|other| other.guid)
}

fn ranked_threat(
    ctx: &ReducerContext,
    creature: &crate::WorldEntity,
    action: &RuleAction,
) -> Vec<u64> {
    let entities = ctx.db.game_world_entity();
    let mut ranked: Vec<(u64, i64)> = ctx
        .db
        .game_threat()
        .by_creature()
        .filter(&creature.guid)
        .filter_map(|entry| {
            let source = entities.guid().find(entry.source_guid)?;
            (!source.dead
                && crate::helpers::in_same_partition(
                    &source,
                    creature.map_id,
                    creature.instance_id,
                )
                && (!matches!(
                    action.target,
                    TargetPolicy::TopThreatPlayer | TargetPolicy::RandomThreatPlayer
                ) || source.is_player())
                && (!action.cast_options.contains(CAST_PLAYER_ONLY) || source.is_player())
                && (!action.cast_options.contains(CAST_AURA_ABSENT)
                    || !crate::spell::has_aura(ctx, source.guid, action.params[0]))
                && (!action.cast_options.contains(CAST_TARGET_CASTING)
                    || ctx
                        .db
                        .game_pending_cast()
                        .by_caster()
                        .filter(&source.guid)
                        .next()
                        .is_some()))
            .then_some((source.guid, entry.threat))
        })
        .collect();
    ranked.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    ranked.into_iter().map(|(guid, _)| guid).collect()
}

fn nearest_area(
    ctx: &ReducerContext,
    creature: &crate::WorldEntity,
    action: &RuleAction,
) -> Option<u64> {
    ranked_threat(ctx, creature, action)
        .into_iter()
        .filter_map(|guid| {
            let target = ctx.db.game_world_entity().guid().find(guid)?;
            Some((guid, distance_yd(creature, &target)))
        })
        .min_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap_or(Ordering::Equal)
                .then(a.0.cmp(&b.0))
        })
        .map(|(guid, _)| guid)
}

fn farthest_hostile(
    ctx: &ReducerContext,
    creature: &crate::WorldEntity,
    action: &RuleAction,
) -> Option<u64> {
    ranked_threat(ctx, creature, action)
        .into_iter()
        .filter_map(|guid| {
            let target = ctx.db.game_world_entity().guid().find(guid)?;
            let distance = distance_yd(creature, &target);
            (distance * distance > crate::combat::MELEE_RANGE_SQ).then_some((guid, distance))
        })
        .max_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap_or(Ordering::Equal)
                .then(b.0.cmp(&a.0))
        })
        .map(|(guid, _)| guid)
}

fn call_for_help(ctx: &ReducerContext, context: &EventContext, radius: u32) -> ActionResult {
    let entities = ctx.db.game_world_entity();
    let Some(caller) = entities.guid().find(context.creature_guid) else {
        return ActionResult::Refused;
    };
    let Some(victim) = entities
        .guid()
        .find(context.current_target_guid.unwrap_or(0))
    else {
        return ActionResult::Refused;
    };
    for helper in crate::helpers::entities_near(
        ctx,
        caller.map_id,
        caller.instance_id,
        caller.x,
        caller.y,
        radius as f32,
    ) {
        if helper.guid == caller.guid
            || helper.is_player()
            || helper.dead
            || !friendly(ctx, &caller, &helper)
            || distance_yd(&caller, &helper) > radius as f32
            || crate::combat::is_engaged(ctx, helper.guid)
        {
            continue;
        }
        if crate::combat::apply_start_attack(ctx, helper.guid, victim.guid).is_ok() {
            crate::hooks::fire_on_aggro(
                ctx,
                &crate::hooks::AggroPayload {
                    creature_guid: helper.guid,
                    target_guid: victim.guid,
                    assist: true,
                },
            );
        }
    }
    ActionResult::Applied
}

fn friendly(ctx: &ReducerContext, first: &crate::WorldEntity, second: &crate::WorldEntity) -> bool {
    crate::faction::is_friendly(ctx, first.faction_template, second.faction_template)
        || (ctx.db.game_faction_template().count() == 0
            && first.faction_template == second.faction_template)
}

/// One candidate at random, or `None` when there are none to choose between.
pub(super) fn pick<T: Copy>(ctx: &ReducerContext, candidates: &[T]) -> Option<T> {
    if candidates.is_empty() {
        return None;
    }
    candidates
        .get(ctx.random::<u32>() as usize % candidates.len())
        .copied()
}

fn distance_yd(first: &crate::WorldEntity, second: &crate::WorldEntity) -> f32 {
    crate::helpers::dist_sq(first, second).sqrt()
}
