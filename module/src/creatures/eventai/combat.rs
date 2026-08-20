//! Engaged EventAI conditions and combat actions.

use std::cmp::Ordering;

use spacetimedb::{ReducerContext, Table};

use super::{
    ActionKind, ActionResult, EventContext, EventKind, Rule, RuleAction, TargetPolicy, ACTION_CAST,
    ACTION_FLEE_FOR_ASSIST, CAST_AURA_ABSENT, CAST_INTERRUPT_PREVIOUS, CAST_PLAYER_ONLY,
    CAST_TARGET_CASTING, CAST_TRIGGERED, EVENT_CREATURE_HP, EVENT_FRIENDLY_HP_DEFICIT,
    EVENT_TARGET_RANGE, EVENT_TIMED_IN_COMBAT,
};
use crate::creatures::ai::{rout_close_ms, rout_window_open, TickScope};
use crate::{
    game_creature_ai_event, game_faction_template, game_melee_attack, game_pending_cast,
    game_threat, game_world_entity,
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
            // A spent window bars the FIXED low-health rout, which is once per engagement. An
            // authored flee is not: cmangos re-runs the action every time its rule fires, so the
            // window re-opens once the previous run has finished. Re-stamping an OPEN window would
            // instead extend one flee forever.
            if !rout_window_open(context.now_ms as u32, fight.rout_ends_ms) {
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

/// The event kinds the engaged pass can ever fire, as the raw column values `authored_combat` reads.
/// An on-aggro, on-spawn or on-death row is an EDGE: it says nothing about how the creature fights
/// between those moments, so it takes nothing over.
const ENGAGED_EVENT_TYPES: [u8; 4] = [
    EVENT_TIMED_IN_COMBAT,
    EVENT_CREATURE_HP,
    EVENT_TARGET_RANGE,
    EVENT_FRIENDLY_HP_DEFICIT,
];

/// Which halves of a creature's fight an imported script has taken over. Both are properties of the
/// SCRIPT, not of this instant: eligibility, phase and the rule's own condition are all deliberately
/// ignored, because a health-gated cast rule has to silence the flat rotation from the first firing
/// of the fight rather than at the moment its band opens. The creature would otherwise hold at a
/// range it casts nothing from.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct AuthoredCombat {
    /// The script owns offensive casting: the flat rotation, the lone spell and the caster hold
    /// range are all off, so the creature closes to melee between authored casts unless an authored
    /// ranged posture holds it back.
    pub casting: bool,
    /// The script owns breaking off: the fixed low-health rout is off, and the authored window runs
    /// the creature whatever its health and whatever its creature type.
    pub flee: bool,
}

/// Read `AuthoredCombat` for one creature. Straight off the rows, with no decode, no rule state and
/// no grouping, because the cast phase, the chase and the rout each ask per engaged creature per
/// firing, and most creatures own no rows at all. `rows_for` answers the pet Gate, so an owned
/// creature reads as unscripted.
pub(crate) fn authored_combat(ctx: &ReducerContext, creature_guid: u64) -> AuthoredCombat {
    let mut authored = AuthoredCombat::default();
    for row in rows_for(ctx, creature_guid) {
        if !ENGAGED_EVENT_TYPES.contains(&row.event_type) {
            continue;
        }
        authored.casting |= row.action_type == ACTION_CAST;
        authored.flee |= row.action_type == ACTION_FLEE_FOR_ASSIST;
    }
    authored
}

/// Every EventAI row that governs one creature: its entry's rules plus any pinned to its own guid.
/// The single row fetch behind the engine, the lifecycle edges and the cycle's authored-combat
/// reads, so the "who runs EventAI at all" Gate is answered once, here.
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
