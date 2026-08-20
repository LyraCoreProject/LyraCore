//! Engaged EventAI conditions, target selection and combat actions. The decisions here are pure
//! logic over the [`EventAiWorld`] Seam, shared by the durable world and the test Fake; only
//! `rows_for` and `authored_combat` read durable state directly.

use std::cmp::Ordering;

use spacetimedb::ReducerContext;

use super::engine::EventAiWorld;
use super::{
    ActionKind, ActionResult, EventAiUnit, EventContext, EventKind, Rule, RuleAction, TargetPolicy,
    ACTION_CAST, ACTION_FLEE_FOR_ASSIST, CAST_AURA_ABSENT, CAST_INTERRUPT_PREVIOUS,
    CAST_PLAYER_ONLY, CAST_TARGET_CASTING, CAST_TRIGGERED, EVENT_CREATURE_HP,
    EVENT_FRIENDLY_HP_DEFICIT, EVENT_TARGET_RANGE, EVENT_TIMED_IN_COMBAT,
};
use crate::creatures::ai::{rout_close_ms, rout_window_open, TickScope};
use crate::{game_creature_ai_event, game_world_entity};

pub(super) fn engaged_contexts<W: EventAiWorld>(world: &W, scope: &TickScope) -> Vec<EventContext> {
    let now_ms = world.eventai_now_ms();
    world
        .eventai_fights(scope)
        .into_iter()
        .flat_map(|fight| {
            [
                EventKind::TimedInCombat,
                EventKind::CreatureHp,
                EventKind::TargetRange,
                EventKind::FriendlyHpDeficit,
            ]
            .into_iter()
            .map(move |kind| EventContext {
                kind,
                creature_guid: fight.creature_guid,
                invoker_guid: Some(fight.victim_guid),
                event_target_guid: Some(fight.victim_guid),
                current_target_guid: Some(fight.victim_guid),
                assisted: false,
                now_ms,
            })
        })
        .collect()
}

/// Checks the rule's event condition and may name the actor selected by that condition.
pub(super) fn condition<W: EventAiWorld>(
    world: &W,
    context: &EventContext,
    rule: &Rule,
) -> Option<EventContext> {
    let creature = world.eventai_unit(context.creature_guid)?;
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
            let target = world.eventai_unit(context.current_target_guid?)?;
            let distance = distance_yd(&creature, &target);
            (distance >= rule.event_params[0] as f32 && distance <= rule.event_params[1] as f32)
                .then_some(*context)
        }
        EventKind::FriendlyHpDeficit => {
            wounded_friendly(world, &creature, rule.event_params[1], rule.event_params[0]).map(
                |guid| EventContext {
                    event_target_guid: Some(guid),
                    ..*context
                },
            )
        }
        EventKind::OnAggro | EventKind::OnDeath | EventKind::OnSpawn => Some(*context),
    }
}

pub(super) fn execute<W: EventAiWorld>(
    world: &mut W,
    context: &EventContext,
    action: &RuleAction,
) -> ActionResult {
    match action.kind {
        ActionKind::Emote => {
            let target_guid = target(world, context, action).unwrap_or(0);
            if world.eventai_deliver_emote(context.creature_guid, action.params[0], target_guid) {
                ActionResult::Applied
            } else {
                ActionResult::Refused
            }
        }
        ActionKind::FleeForAssist => {
            let Some(rout_ends_ms) = world.eventai_rout_ends_ms(context.creature_guid) else {
                return ActionResult::Refused;
            };
            // A spent window bars the FIXED low-health rout, which is once per engagement. An
            // authored flee is not: cmangos re-runs the action every time its rule fires, so the
            // window re-opens once the previous run has finished. Re-stamping an OPEN window would
            // instead extend one flee forever.
            if !rout_window_open(context.now_ms as u32, rout_ends_ms) {
                world.stamp_eventai_rout(
                    context.creature_guid,
                    rout_close_ms(context.now_ms as u32),
                );
            }
            ActionResult::Applied
        }
        ActionKind::CallForHelp => call_for_help(world, context, action.params[0]),
        _ => ActionResult::Unsupported,
    }
}

pub(super) fn target<W: EventAiWorld>(
    world: &W,
    context: &EventContext,
    action: &RuleAction,
) -> Option<u64> {
    let creature = world.eventai_unit(context.creature_guid)?;
    let target = match action.target {
        TargetPolicy::Current => context.current_target_guid,
        TargetPolicy::SelfActor => Some(context.creature_guid),
        TargetPolicy::Invoker => context.invoker_guid,
        TargetPolicy::EventTarget => context.event_target_guid,
        TargetPolicy::TopThreat | TargetPolicy::TopThreatPlayer => {
            ranked_threat(world, &creature, action).first().copied()
        }
        TargetPolicy::SecondThreat => ranked_threat(world, &creature, action).get(1).copied(),
        TargetPolicy::RandomThreat | TargetPolicy::RandomThreatPlayer => {
            pick(world, &ranked_threat(world, &creature, action))
        }
        TargetPolicy::NearestArea => nearest_area(world, &creature, action),
        TargetPolicy::FarthestHostile => farthest_hostile(world, &creature, action),
    }?;
    if action.cast_options.contains(CAST_PLAYER_ONLY) && !world.eventai_unit(target)?.is_player {
        return None;
    }
    Some(target)
}

pub(super) fn cast<W: EventAiWorld>(
    world: &mut W,
    context: &EventContext,
    action: &RuleAction,
    target_guid: u64,
) -> ActionResult {
    if action.cast_options.contains(CAST_TRIGGERED) {
        return ActionResult::Refused;
    }
    if action.cast_options.contains(CAST_PLAYER_ONLY)
        && !world
            .eventai_unit(target_guid)
            .is_some_and(|target| target.is_player)
    {
        return ActionResult::Refused;
    }
    if action.cast_options.contains(CAST_AURA_ABSENT)
        && world.eventai_has_aura(target_guid, action.params[0])
    {
        return ActionResult::Refused;
    }
    if action.cast_options.contains(CAST_TARGET_CASTING) && !world.eventai_is_casting(target_guid) {
        return ActionResult::Refused;
    }
    let Some(creature) = world.eventai_unit(context.creature_guid) else {
        return ActionResult::Refused;
    };
    if world.eventai_is_casting(creature.guid) {
        if !action.cast_options.contains(CAST_INTERRUPT_PREVIOUS) {
            return ActionResult::Refused;
        }
        world.eventai_interrupt_cast(creature.guid);
    }
    if world.eventai_begin_cast(&creature, action.params[0], target_guid) {
        ActionResult::Applied
    } else {
        ActionResult::Refused
    }
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

fn wounded_friendly<W: EventAiWorld>(
    world: &W,
    creature: &EventAiUnit,
    radius: u32,
    deficit: u32,
) -> Option<u64> {
    world
        .eventai_units_near(creature, radius as f32)
        .into_iter()
        .filter(|other| {
            !other.dead
                && (other.guid == creature.guid
                    || world.eventai_factions_friendly(
                        creature.faction_template,
                        other.faction_template,
                    ))
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

/// The creature's threat list, hostiles first by threat then by guid, with every target the
/// action could never take already gone.
fn ranked_threat<W: EventAiWorld>(
    world: &W,
    creature: &EventAiUnit,
    action: &RuleAction,
) -> Vec<u64> {
    let mut ranked: Vec<(u64, i64)> = world
        .eventai_threat(creature.guid)
        .into_iter()
        .filter_map(|(guid, threat)| {
            let source = world.eventai_unit(guid)?;
            (!source.dead
                && source.map_id == creature.map_id
                && source.instance_id == creature.instance_id
                && (!matches!(
                    action.target,
                    TargetPolicy::TopThreatPlayer | TargetPolicy::RandomThreatPlayer
                ) || source.is_player)
                && (!action.cast_options.contains(CAST_PLAYER_ONLY) || source.is_player)
                && (!action.cast_options.contains(CAST_AURA_ABSENT)
                    || !world.eventai_has_aura(guid, action.params[0]))
                && (!action.cast_options.contains(CAST_TARGET_CASTING)
                    || world.eventai_is_casting(guid)))
            .then_some((guid, threat))
        })
        .collect();
    ranked.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    ranked.into_iter().map(|(guid, _)| guid).collect()
}

fn nearest_area<W: EventAiWorld>(
    world: &W,
    creature: &EventAiUnit,
    action: &RuleAction,
) -> Option<u64> {
    ranked_threat(world, creature, action)
        .into_iter()
        .filter_map(|guid| {
            let target = world.eventai_unit(guid)?;
            Some((guid, distance_yd(creature, &target)))
        })
        .min_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap_or(Ordering::Equal)
                .then(a.0.cmp(&b.0))
        })
        .map(|(guid, _)| guid)
}

fn farthest_hostile<W: EventAiWorld>(
    world: &W,
    creature: &EventAiUnit,
    action: &RuleAction,
) -> Option<u64> {
    ranked_threat(world, creature, action)
        .into_iter()
        .filter_map(|guid| {
            let target = world.eventai_unit(guid)?;
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

fn call_for_help<W: EventAiWorld>(
    world: &mut W,
    context: &EventContext,
    radius: u32,
) -> ActionResult {
    let Some(caller) = world.eventai_unit(context.creature_guid) else {
        return ActionResult::Refused;
    };
    let Some(victim) = world.eventai_unit(context.current_target_guid.unwrap_or(0)) else {
        return ActionResult::Refused;
    };
    for helper in world.eventai_units_near(&caller, radius as f32) {
        if helper.guid == caller.guid
            || helper.is_player
            || helper.dead
            || !world.eventai_factions_friendly(caller.faction_template, helper.faction_template)
            || distance_yd(&caller, &helper) > radius as f32
            || world.eventai_is_engaged(helper.guid)
        {
            continue;
        }
        world.eventai_engage_assist(helper.guid, victim.guid);
    }
    ActionResult::Applied
}

/// One candidate at random, or `None` when there are none to choose between.
fn pick<W: EventAiWorld, T: Copy>(world: &W, candidates: &[T]) -> Option<T> {
    if candidates.is_empty() {
        return None;
    }
    candidates
        .get(world.eventai_roll() as usize % candidates.len())
        .copied()
}

fn distance_yd(first: &EventAiUnit, second: &EventAiUnit) -> f32 {
    let (dx, dy, dz) = (second.x - first.x, second.y - first.y, second.z - first.z);
    (dx * dx + dy * dy + dz * dz).sqrt()
}
