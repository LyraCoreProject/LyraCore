//! Engaged EventAI conditions, target selection and combat actions. The decisions here are pure
//! logic over the [`EventAiWorld`] Seam, shared by the durable world and the test Scenario. Only
//! `definition_for` and `authored_combat` read durable definitions directly.

use std::cmp::Ordering;

use spacetimedb::ReducerContext;

use super::engine::EventAiWorld;
use super::{
    ActionResult, CastInstruction, CreatureInstruction, DefinitionRevision, EventAiDefinition,
    EventAiRule, EventAiSubject, EventAiUnit, EventCondition, EventContext, EventKind,
    InstructionTarget,
};
use crate::creatures::ai::{rout_close_ms, rout_window_open, TickScope};
use crate::{game_creature_ai_definition, game_world_entity};

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
    rule: &EventAiRule,
) -> Option<EventContext> {
    let creature = world.eventai_unit(context.creature_guid)?;
    match rule.event {
        EventCondition::TimedInCombat(_) => Some(*context),
        EventCondition::CreatureHealth(health) => in_inclusive_pct(
            creature.health,
            creature.max_health,
            u32::from(health.min_pct),
            u32::from(health.max_pct),
        )
        .then_some(*context),
        EventCondition::TargetRange(range) => {
            let target = world.eventai_unit(context.current_target_guid?)?;
            let distance = distance_yd(&creature, &target);
            (distance >= range.min_yd as f32 && distance <= range.max_yd as f32).then_some(*context)
        }
        EventCondition::FriendlyHealthDeficit(deficit) => {
            wounded_friendly(world, &creature, deficit.radius_yd, deficit.missing_health).map(
                |guid| EventContext {
                    event_target_guid: Some(guid),
                    ..*context
                },
            )
        }
        EventCondition::OnAggro | EventCondition::OnDeath | EventCondition::OnSpawn => {
            Some(*context)
        }
    }
}

pub(super) fn execute<W: EventAiWorld>(
    world: &mut W,
    context: &EventContext,
    instruction: &CreatureInstruction,
) -> ActionResult {
    match instruction {
        CreatureInstruction::Emote(emote) => {
            let target_guid = target(world, context, emote.target, None).unwrap_or(0);
            if world.eventai_deliver_emote(context.creature_guid, emote.emote_id, target_guid) {
                ActionResult::Applied
            } else {
                ActionResult::Refused
            }
        }
        CreatureInstruction::FleeForAssist => {
            let Some(rout_ends_ms) = world.eventai_rout_ends_ms(context.creature_guid) else {
                return ActionResult::Refused;
            };
            // A spent window bars the FIXED low-health rout, which is once per engagement. An
            // authored flee is not: the source re-runs the action every time its rule fires, so the
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
        CreatureInstruction::CallForHelp(help) => call_for_help(world, context, help.radius_yd),
        _ => ActionResult::Unsupported,
    }
}

pub(super) fn target<W: EventAiWorld>(
    world: &W,
    context: &EventContext,
    target_policy: InstructionTarget,
    cast: Option<&CastInstruction>,
) -> Option<u64> {
    let creature = world.eventai_unit(context.creature_guid)?;
    let target = match target_policy {
        InstructionTarget::CurrentOpponent => context.current_target_guid,
        InstructionTarget::SelfActor => Some(context.creature_guid),
        InstructionTarget::Invoker => context.invoker_guid,
        InstructionTarget::EventSubject => context.event_target_guid,
        InstructionTarget::HighestThreat | InstructionTarget::HighestThreatCharacter => {
            ranked_threat(world, &creature, target_policy, cast)
                .first()
                .copied()
        }
        InstructionTarget::SecondThreat => ranked_threat(world, &creature, target_policy, cast)
            .get(1)
            .copied(),
        InstructionTarget::RandomThreat | InstructionTarget::RandomThreatCharacter => {
            pick(world, &ranked_threat(world, &creature, target_policy, cast))
        }
        InstructionTarget::EligibleCasterArea => {
            nearest_area(world, &creature, target_policy, cast)
        }
        InstructionTarget::FarthestHostile => {
            farthest_hostile(world, &creature, target_policy, cast)
        }
    }?;
    if cast.is_some_and(|cast| cast.character_only) && !world.eventai_unit(target)?.is_player {
        return None;
    }
    Some(target)
}

pub(super) fn cast<W: EventAiWorld>(
    world: &mut W,
    context: &EventContext,
    cast: &CastInstruction,
    target_guid: u64,
) -> ActionResult {
    if cast.triggered {
        return ActionResult::Refused;
    }
    if cast.character_only
        && !world
            .eventai_unit(target_guid)
            .is_some_and(|target| target.is_player)
    {
        return ActionResult::Refused;
    }
    if cast.aura_absent && world.eventai_has_aura(target_guid, cast.spell_id) {
        return ActionResult::Refused;
    }
    if cast.target_must_be_casting && !world.eventai_is_casting(target_guid) {
        return ActionResult::Refused;
    }
    let Some(creature) = world.eventai_unit(context.creature_guid) else {
        return ActionResult::Refused;
    };
    if world.eventai_is_casting(creature.guid) {
        if !cast.interrupt_previous {
            return ActionResult::Refused;
        }
        world.eventai_interrupt_cast(creature.guid);
    }
    if world.eventai_begin_cast(&creature, cast.spell_id, target_guid) {
        ActionResult::Applied
    } else {
        ActionResult::Refused
    }
}

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

/// Read `AuthoredCombat` from one creature's composed definition. Most creatures have no definition
/// and keep the default combat behavior.
pub(crate) fn authored_combat(ctx: &ReducerContext, creature_guid: u64) -> AuthoredCombat {
    let mut authored = AuthoredCombat::default();
    for rule in definition_for(ctx, creature_guid).rules {
        if !rule.event.kind().recurs() {
            continue;
        }
        for instruction in rule.instructions {
            authored.casting |= matches!(instruction, CreatureInstruction::Cast(_));
            authored.flee |= matches!(instruction, CreatureInstruction::FleeForAssist);
        }
    }
    authored
}

pub(super) fn definition_for(ctx: &ReducerContext, creature_guid: u64) -> EventAiDefinition {
    let Some(creature) = ctx.db.game_world_entity().guid().find(creature_guid) else {
        return EventAiDefinition::empty(creature_guid);
    };
    if !super::runs_eventai(&creature) {
        return EventAiDefinition::empty(creature_guid);
    }
    let definitions = ctx.db.game_creature_ai_definition();
    let mut rows = Vec::with_capacity(2);
    if let Some(row) = definitions
        .by_entry()
        .filter(&creature.entry)
        .min_by_key(|row| row.id)
    {
        rows.push(EventAiDefinition {
            subject: EventAiSubject::Entry(row.creature_entry),
            revision: DefinitionRevision {
                value: row.definition_revision,
            },
            rules: row.rules,
        });
    }
    if let Some(row) = definitions
        .by_guid()
        .filter(&creature_guid)
        .min_by_key(|row| row.id)
    {
        rows.push(EventAiDefinition {
            subject: EventAiSubject::Guid(row.creature_guid),
            revision: DefinitionRevision {
                value: row.definition_revision,
            },
            rules: row.rules,
        });
    }
    EventAiDefinition::compose(creature_guid, rows)
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
    target: InstructionTarget,
    cast: Option<&CastInstruction>,
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
                    target,
                    InstructionTarget::HighestThreatCharacter
                        | InstructionTarget::RandomThreatCharacter
                ) || source.is_player)
                && (!cast.is_some_and(|cast| cast.character_only) || source.is_player)
                && (!cast.is_some_and(|cast| cast.aura_absent)
                    || !world.eventai_has_aura(guid, cast.map_or(0, |cast| cast.spell_id)))
                && (!cast.is_some_and(|cast| cast.target_must_be_casting)
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
    target: InstructionTarget,
    cast: Option<&CastInstruction>,
) -> Option<u64> {
    ranked_threat(world, creature, target, cast)
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
    target: InstructionTarget,
    cast: Option<&CastInstruction>,
) -> Option<u64> {
    ranked_threat(world, creature, target, cast)
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
