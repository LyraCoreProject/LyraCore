//! Engaged EventAI conditions, target selection and combat actions. The decisions here are pure
//! logic over the [`EventAiWorld`] Seam, shared by the durable world and the test Scenario. Only
//! `definition_for` and `authored_combat` read durable definitions directly.

use std::cmp::Ordering;
use std::collections::HashSet;

use spacetimedb::ReducerContext;

use super::engine::EventAiWorld;
use super::{
    ActionResult, CastInstruction, CreatureInstruction, CycleActor, DefinitionRevision,
    EventAiDefinition, EventAiRule, EventAiSubject, EventAiUnit, EventCondition, EventContext,
    EventKind, FriendlyAuraSelection, InstructionTarget, PostureAdmission, SpawnCondition,
    SpellCastTarget, SpellCasterAdmission, SpellCasterRole, SpellStartMode, SpellTargetRole,
};
use crate::creatures::ai::{rout_close_ms, rout_window_open, TickScope};
use crate::{game_creature_ai_definition, game_world_entity};

pub(super) fn cycle_contexts<W: EventAiWorld>(
    world: &W,
    scope: &TickScope,
    active: &HashSet<u64>,
) -> Vec<EventContext> {
    contexts_for_actors(world, world.eventai_cycle_actors(scope, active))
}

fn contexts_for_actors<W: EventAiWorld>(world: &W, actors: Vec<CycleActor>) -> Vec<EventContext> {
    const KINDS: [EventKind; 19] = [
        EventKind::TimedInCombat,
        EventKind::TimedOutOfCombat,
        EventKind::CreatureHp,
        EventKind::CreaturePower,
        EventKind::TargetRange,
        EventKind::OutOfCombatSight,
        EventKind::TargetHp,
        EventKind::TargetCasting,
        EventKind::FriendlyHpDeficit,
        EventKind::FriendlyCrowdControlled,
        EventKind::FriendlyMissingAura,
        EventKind::TargetPower,
        EventKind::CreatureAura,
        EventKind::TargetAura,
        EventKind::CreatureMissingAura,
        EventKind::TargetMissingAura,
        EventKind::TimedGeneric,
        EventKind::SelectAttackingTarget,
        EventKind::FacingTarget,
    ];
    let now_ms = world.eventai_now_ms();
    actors
        .into_iter()
        .flat_map(|actor| {
            let spawner_guid = world.eventai_spawner_guid(actor.creature_guid);
            KINDS.into_iter().map(move |kind| EventContext {
                current_target_guid: actor.current_target_guid,
                spawner_guid,
                engaged: actor.engaged,
                ..EventContext::empty(kind, actor.creature_guid, now_ms)
            })
        })
        .collect()
}

/// Checks the rule's event condition and may name the actor selected by that condition.
#[allow(clippy::too_many_lines)] // One arm per EventAI event kind.
pub(crate) fn condition<W: EventAiWorld>(
    world: &W,
    context: &EventContext,
    rule: &EventAiRule,
    choice: u64,
) -> Option<EventContext> {
    let creature = world.eventai_unit(context.creature_guid)?;
    match rule.event {
        EventCondition::TimedInCombat(_) => context.engaged.then_some(*context),
        EventCondition::TimedOutOfCombat(_) => (!context.engaged).then_some(*context),
        EventCondition::TimedGeneric(_) => Some(*context),
        EventCondition::CreatureHealth(health) => ((context.engaged || health.allow_out_of_combat)
            && in_inclusive_pct(
                creature.health,
                creature.max_health,
                u32::from(health.min_pct),
                u32::from(health.max_pct),
            ))
        .then_some(*context),
        EventCondition::CreaturePower(power) => (context.engaged
            && creature.power_type == lyracore_shared::packing::power_type::MANA
            && in_inclusive_pct(
                creature.power,
                creature.max_power,
                u32::from(power.min_pct),
                u32::from(power.max_pct),
            ))
        .then_some(*context),
        EventCondition::TargetRange(range) => {
            if !context.engaged {
                return None;
            }
            let target = world.eventai_unit(context.current_target_guid?)?;
            let distance = distance_yd(&creature, &target);
            (distance >= range.min_yd as f32 && distance <= range.max_yd as f32).then_some(*context)
        }
        EventCondition::FriendlyHealthDeficit(deficit) if context.engaged => {
            let exclude_actor = rule.instructions.iter().any(|instruction| {
                matches!(
                    instruction,
                    CreatureInstruction::Cast(CastInstruction {
                        spell_id,
                        target: InstructionTarget::EventSubject,
                        ..
                    }) if world.eventai_spell_excludes_caster(*spell_id)
                )
            });
            wounded_friendly(world, &creature, deficit, exclude_actor)
                .map(|guid| event_target_context(context, guid))
        }
        EventCondition::FriendlyHealthDeficit(_) => None,
        EventCondition::TargetHealth(health) => {
            if !context.engaged {
                return None;
            }
            let target = world.eventai_unit(context.current_target_guid?)?;
            in_inclusive_pct(
                target.health,
                target.max_health,
                u32::from(health.min_pct),
                u32::from(health.max_pct),
            )
            .then_some(*context)
        }
        EventCondition::TargetPower(power) => {
            let target = world.eventai_unit(context.current_target_guid?)?;
            (context.engaged
                && target.power_type == lyracore_shared::packing::power_type::MANA
                && in_inclusive_pct(
                    target.power,
                    target.max_power,
                    u32::from(power.min_pct),
                    u32::from(power.max_pct),
                ))
            .then_some(*context)
        }
        EventCondition::TargetCasting if context.engaged => context
            .current_target_guid
            .filter(|guid| world.eventai_is_casting(*guid))
            .map(|_| *context),
        EventCondition::TargetCasting => None,
        EventCondition::OutOfCombatSight(sight) => {
            if context.engaged {
                return None;
            }
            let candidate = world
                .eventai_units_near(&creature, sight.max_range_yd as f32)
                .into_iter()
                .filter(|candidate| {
                    candidate.guid != creature.guid
                        && !candidate.dead
                        && candidate.map_id == creature.map_id
                        && candidate.instance_id == creature.instance_id
                        && (!sight.character_only || candidate.is_player)
                        && (sight.require_non_hostile
                            != world.eventai_factions_hostile(
                                creature.faction_template,
                                candidate.faction_template,
                            ))
                        && distance_yd(&creature, candidate) <= sight.max_range_yd as f32
                        && world.eventai_line_of_sight(&creature, candidate)
                        && beneficiary_guid(world, candidate.guid)
                            .and_then(|guid| world.eventai_unit(guid))
                            .filter(|beneficiary| beneficiary.is_player)
                            .is_none_or(|beneficiary| {
                                world.eventai_matches_predicate(beneficiary.guid, sight.predicate)
                            })
                })
                .min_by(|first, second| compare_distance(&creature, first, second))?;
            let beneficiary_guid = beneficiary_guid(world, candidate.guid);
            Some(EventContext {
                invoker_guid: Some(candidate.guid),
                beneficiary_guid,
                ..*context
            })
        }
        EventCondition::OnSpawn(spawn) => match spawn {
            SpawnCondition::Always => Some(*context),
            SpawnCondition::Map(map) => (creature.map_id == map.map_id).then_some(*context),
            SpawnCondition::ZoneOrArea(zone) => world
                .eventai_in_zone_or_area(&creature, zone.zone_or_area_id)
                .then_some(*context),
        },
        EventCondition::FriendlyCrowdControlled(condition) if context.engaged => {
            friendly_candidate(world, &creature, condition.radius_yd, |candidate| {
                world.eventai_is_engaged(candidate.guid)
                    && !candidate.is_player
                    && candidate.owner_guid == 0
                    && world.eventai_is_crowd_controlled(candidate.guid, 0)
            })
            .map(|guid| event_target_context(context, guid))
        }
        EventCondition::FriendlyCrowdControlled(_) => None,
        EventCondition::FriendlyMissingAura(condition) => {
            let actor_admitted = match condition.selection {
                FriendlyAuraSelection::NearbyWhileEngaged => context.engaged,
                FriendlyAuraSelection::MatchActorCombatState => true,
                FriendlyAuraSelection::AnyWhileDisengaged => !context.engaged,
            };
            if !actor_admitted {
                return None;
            }
            friendly_candidate(world, &creature, condition.radius_yd, |candidate| {
                let combat_state_matches = match condition.selection {
                    FriendlyAuraSelection::NearbyWhileEngaged => {
                        world.eventai_is_engaged(candidate.guid)
                    }
                    FriendlyAuraSelection::MatchActorCombatState => {
                        !context.engaged || world.eventai_is_engaged(candidate.guid)
                    }
                    FriendlyAuraSelection::AnyWhileDisengaged => true,
                };
                combat_state_matches
                    && !candidate.is_player
                    && candidate.owner_guid == 0
                    && !world.eventai_has_aura(candidate.guid, condition.spell_id)
            })
            .map(|guid| event_target_context(context, guid))
        }
        EventCondition::CreatureAura(aura) => {
            (world.eventai_aura_stacks(creature.guid, aura.spell_id) >= aura.stacks)
                .then_some(*context)
        }
        EventCondition::TargetAura(aura) => {
            if !context.engaged {
                return None;
            }
            let target = context.current_target_guid?;
            (world.eventai_aura_stacks(target, aura.spell_id) >= aura.stacks).then_some(*context)
        }
        EventCondition::CreatureMissingAura(aura) => {
            (world.eventai_aura_stacks(creature.guid, aura.spell_id) < aura.stacks)
                .then_some(*context)
        }
        EventCondition::TargetMissingAura(aura) => {
            if !context.engaged {
                return None;
            }
            let target = context.current_target_guid?;
            (world.eventai_aura_stacks(target, aura.spell_id) < aura.stacks).then_some(*context)
        }
        EventCondition::SelectAttackingTarget(range) => {
            let candidates: Vec<u64> = ranked_threat(
                world,
                &creature,
                InstructionTarget::RandomThreat,
                None,
                SpellCasterAdmission::Living,
            )
            .into_iter()
            .filter(|guid| {
                world.eventai_unit(*guid).is_some_and(|candidate| {
                    let distance = distance_yd(&creature, &candidate);
                    distance >= range.min_yd as f32 && distance <= range.max_yd as f32
                })
            })
            .collect();
            pick(choice, &candidates).map(|guid| event_target_context(context, guid))
        }
        EventCondition::FacingTarget(facing) => {
            let target = world.eventai_unit(context.current_target_guid?)?;
            (distance_sq(&creature, &target) <= crate::combat::MELEE_RANGE_SQ
                && actor_is_behind_target(&creature, &target) == facing.behind)
                .then_some(*context)
        }
        EventCondition::OnKill(kill) => context
            .invoker_is_player
            .or_else(|| {
                context
                    .invoker_guid
                    .and_then(|guid| world.eventai_unit(guid))
                    .map(|victim| victim.is_player)
            })
            .filter(|is_player| !kill.character_only || *is_player)
            .map(|_| *context),
        EventCondition::OnSpellHit(spell) | EventCondition::OnSpellHitTarget(spell) => {
            ((spell.spell_id == 0 || context.spell_id == Some(spell.spell_id))
                && (spell.school_mask == 0 || context.spell_school_mask & spell.school_mask != 0))
                .then_some(*context)
        }
        EventCondition::OnSummoned(entry) | EventCondition::OnSummonedDeath(entry) => {
            (entry.creature_entry == 0 || context.creature_entry == Some(entry.creature_entry))
                .then_some(*context)
        }
        EventCondition::OnReceiveEmote(receive) => (context.emote_id == Some(receive.emote_id)
            && context
                .beneficiary_guid
                .and_then(|guid| world.eventai_unit(guid))
                .filter(|beneficiary| beneficiary.is_player)
                .is_none_or(|beneficiary| {
                    world.eventai_matches_predicate(beneficiary.guid, receive.predicate)
                }))
        .then_some(*context),
        EventCondition::OnReceiveAiEvent(receive) => {
            let sender_matches = receive.sender_entry == 0
                || context
                    .ai_sender_guid
                    .and_then(|guid| world.eventai_unit(guid))
                    .is_some_and(|sender| sender.entry == receive.sender_entry);
            (context.ai_event == Some(receive.kind) && sender_matches).then_some(*context)
        }
        EventCondition::OnDeath(death) => matches!(death.predicate, super::EventPredicate::Always)
            .then_some(())
            .or_else(|| {
                let beneficiary = context
                    .beneficiary_guid
                    .and_then(|guid| world.eventai_unit(guid))
                    .filter(|beneficiary| beneficiary.is_player)?;
                world
                    .eventai_matches_predicate(beneficiary.guid, death.predicate)
                    .then_some(())
            })
            .map(|_| *context),
        EventCondition::OnAggro
        | EventCondition::OnEvade
        | EventCondition::OnReachedHome
        | EventCondition::TargetNotReachable => Some(*context),
    }
}

pub(super) fn posture_matches<W: EventAiWorld>(
    world: &W,
    context: &EventContext,
    posture: PostureAdmission,
) -> bool {
    let ranged = world
        .eventai_creature_state(context.creature_guid)
        .ranged_posture_active;
    match posture {
        PostureAdmission::Any => true,
        PostureAdmission::RangedOnly => ranged,
        PostureAdmission::MeleeOnly => !ranged,
    }
}

pub(super) fn execute<W: EventAiWorld>(
    world: &mut W,
    context: &EventContext,
    instruction: &CreatureInstruction,
    choice: u64,
) -> ActionResult {
    match instruction {
        CreatureInstruction::Emote(emote) => {
            let Some(target_guid) = unit_target(world, context, emote.target, None, choice) else {
                return ActionResult::Refused;
            };
            if world.eventai_deliver_emote(context.creature_guid, emote.emote_id, target_guid) {
                ActionResult::Applied
            } else {
                ActionResult::Refused
            }
        }
        CreatureInstruction::RandomEmote(emote) => {
            let Some(emote_id) = emote
                .emote_ids
                .get(choice as usize % emote.emote_ids.len())
                .copied()
            else {
                return ActionResult::Refused;
            };
            if emote_id < 0 {
                return ActionResult::Applied;
            }
            if world.eventai_deliver_emote(context.creature_guid, emote_id as u32, 0) {
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
    choice: u64,
) -> Option<SpellCastTarget> {
    let creature = world.eventai_unit(context.creature_guid)?;
    let admission = spell_caster_admission(context, &creature);
    if target_policy == InstructionTarget::NoExplicitSpellTarget {
        return cast
            .is_none_or(|cast| {
                spell_target_eligible(world, &creature, &creature, cast, admission, false)
            })
            .then_some(SpellCastTarget::None);
    }
    if target_policy == InstructionTarget::EligibleCasterArea {
        nearest_area(world, &creature, target_policy, cast, admission)?;
        return Some(SpellCastTarget::CasterArea);
    }
    let target_guid = match target_policy {
        InstructionTarget::CurrentOpponent => context.current_target_guid,
        InstructionTarget::SelfActor => Some(context.creature_guid),
        InstructionTarget::Invoker => context.invoker_guid,
        InstructionTarget::Beneficiary => context.beneficiary_guid,
        InstructionTarget::AiSender => context.ai_sender_guid,
        InstructionTarget::Spawner => context.spawner_guid,
        InstructionTarget::EventSubject => context.event_target_guid,
        InstructionTarget::HighestThreat | InstructionTarget::HighestThreatCharacter => {
            ranked_threat(world, &creature, target_policy, cast, admission)
                .first()
                .copied()
        }
        InstructionTarget::SecondThreat => {
            ranked_threat(world, &creature, target_policy, cast, admission)
                .get(1)
                .copied()
        }
        InstructionTarget::RandomThreat
        | InstructionTarget::RandomThreatCharacter
        | InstructionTarget::RandomHostileManaUser => pick(
            choice,
            &ranked_threat(world, &creature, target_policy, cast, admission),
        ),
        InstructionTarget::RandomThreatExceptHighest
        | InstructionTarget::RandomThreatCharacterExceptHighest => {
            let candidates = ranked_threat(world, &creature, target_policy, cast, admission);
            pick(choice, candidates.get(1..).unwrap_or_default())
        }
        InstructionTarget::EligibleCasterArea | InstructionTarget::NoExplicitSpellTarget => {
            unreachable!("target shape handled before unit selection")
        }
        InstructionTarget::FarthestHostile => {
            farthest_hostile(world, &creature, target_policy, cast, admission)
        }
    }?;
    let candidate = world.eventai_unit(target_guid)?;
    candidate_eligible(
        world,
        &creature,
        &candidate,
        target_policy,
        cast,
        admission,
        killed_invoker_target(context, target_policy, target_guid),
    )
    .then_some(SpellCastTarget::Unit(target_guid))
}

pub(super) fn unit_target<W: EventAiWorld>(
    world: &W,
    context: &EventContext,
    target_policy: InstructionTarget,
    cast: Option<&CastInstruction>,
    choice: u64,
) -> Option<u64> {
    match target(world, context, target_policy, cast, choice)? {
        SpellCastTarget::Unit(guid) => Some(guid),
        SpellCastTarget::None | SpellCastTarget::CasterArea => None,
    }
}

pub(super) fn cast<W: EventAiWorld>(
    world: &mut W,
    context: &EventContext,
    cast: &CastInstruction,
    selected: SpellCastTarget,
) -> ActionResult {
    let Some(actor) = world.eventai_unit(context.creature_guid) else {
        return ActionResult::Refused;
    };
    let admission = spell_caster_admission(context, &actor);
    let selected_guid = match selected {
        SpellCastTarget::Unit(guid) => Some(guid),
        SpellCastTarget::None | SpellCastTarget::CasterArea => None,
    };
    let caster = match cast.caster_role {
        SpellCasterRole::Actor => actor,
        SpellCasterRole::Selected => {
            let Some(caster) = selected_guid.and_then(|guid| world.eventai_unit(guid)) else {
                return ActionResult::Refused;
            };
            caster
        }
    };
    let spell_target = match cast.target_role {
        SpellTargetRole::Selected => selected,
        SpellTargetRole::Actor => SpellCastTarget::Unit(actor.guid),
        SpellTargetRole::Caster => SpellCastTarget::Unit(caster.guid),
        SpellTargetRole::None => SpellCastTarget::None,
        SpellTargetRole::CasterArea => SpellCastTarget::CasterArea,
    };
    if let SpellCastTarget::Unit(target_guid) = spell_target {
        let Some(target) = world.eventai_unit(target_guid) else {
            return ActionResult::Refused;
        };
        if !spell_target_eligible(
            world,
            &caster,
            &target,
            cast,
            admission,
            context.kind == EventKind::OnKill && context.invoker_guid == Some(target_guid),
        ) {
            return ActionResult::Refused;
        }
    }
    if world.eventai_is_casting(caster.guid)
        && !cast.interrupt_previous
        && cast.start_mode == SpellStartMode::Direct
    {
        return ActionResult::Refused;
    }
    if world.eventai_start_spell(
        &caster,
        cast.spell_id,
        cast.start_mode,
        spell_target,
        cast.interrupt_previous,
        admission,
    ) {
        if cast.distance_after_start {
            let distance = world.eventai_spell_range(cast.spell_id).unwrap_or(0) as f32;
            world.set_eventai_ranged_posture(actor.guid, distance, 0.0);
        }
        ActionResult::Applied
    } else {
        ActionResult::Refused
    }
}

/// Which compatibility behaviors EventAI owns for this creature.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct AuthoredCombat {
    /// An engaged cycle rule owns casting for this creature.
    pub casting: bool,
    /// The definition marks at least one spell for authored ranged-mode posture. T8 consumes this
    /// metadata with the Set Ranged Mode action.
    pub main_spell_posture: bool,
    /// The script owns breaking off: the fixed low-health rout is off, and the authored window runs
    /// the creature whatever its health and whatever its creature type.
    pub flee: bool,
}

/// Read compatibility ownership from the creature's composed definition.
pub(crate) fn authored_combat(ctx: &ReducerContext, creature_guid: u64) -> AuthoredCombat {
    let mut authored = AuthoredCombat::default();
    let definition = definition_for(ctx, creature_guid);
    for rule in definition.rules {
        if !rule.event.runs_while_engaged() {
            continue;
        }
        for instruction in rule.instructions {
            authored.casting |= matches!(instruction, CreatureInstruction::Cast(_));
            authored.main_spell_posture |= matches!(
                instruction,
                CreatureInstruction::Cast(CastInstruction {
                    main_spell: true,
                    ..
                })
            );
            authored.flee |= matches!(instruction, CreatureInstruction::FleeForAssist);
        }
    }
    authored
}

pub(crate) fn current_definition_revision(
    ctx: &ReducerContext,
    creature_guid: u64,
) -> DefinitionRevision {
    definition_for(ctx, creature_guid).revision
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
    if max == 0 {
        return false;
    }
    let percent = u64::from(value) * 100 / u64::from(max);
    percent >= u64::from(min_pct) && percent <= u64::from(max_pct)
}

fn wounded_friendly<W: EventAiWorld>(
    world: &W,
    creature: &EventAiUnit,
    condition: super::FriendlyHealthDeficitCondition,
    exclude_actor: bool,
) -> Option<u64> {
    let radius = condition.radius_yd;
    world
        .eventai_units_near(creature, radius as f32)
        .into_iter()
        .filter(|other| {
            !other.dead
                && !other.is_player
                && other.owner_guid == 0
                && (!exclude_actor || other.guid != creature.guid)
                && world.eventai_is_engaged(other.guid)
                && other.map_id == creature.map_id
                && other.instance_id == creature.instance_id
                && (other.guid == creature.guid
                    || world.eventai_factions_friendly(
                        creature.faction_template,
                        other.faction_template,
                    ))
                && distance_yd(creature, other) <= radius as f32
                && if condition.percent {
                    other.max_health != 0
                        && u64::from(other.max_health.saturating_sub(other.health)) * 100
                            > u64::from(condition.missing_health) * u64::from(other.max_health)
                } else {
                    other.max_health.saturating_sub(other.health) > condition.missing_health
                }
        })
        .min_by(|a, b| {
            let a_missing = a.max_health - a.health;
            let b_missing = b.max_health - b.health;
            if condition.percent {
                let a_scaled = u64::from(a_missing) * u64::from(b.max_health);
                let b_scaled = u64::from(b_missing) * u64::from(a.max_health);
                b_scaled.cmp(&a_scaled).then(a.guid.cmp(&b.guid))
            } else {
                b_missing.cmp(&a_missing).then(a.guid.cmp(&b.guid))
            }
        })
        .map(|other| other.guid)
}

fn friendly_candidate<W: EventAiWorld>(
    world: &W,
    creature: &EventAiUnit,
    radius: u32,
    predicate: impl Fn(&EventAiUnit) -> bool,
) -> Option<u64> {
    world
        .eventai_units_near(creature, radius as f32)
        .into_iter()
        .filter(|candidate| {
            !candidate.dead
                && candidate.map_id == creature.map_id
                && candidate.instance_id == creature.instance_id
                && (candidate.guid == creature.guid
                    || world.eventai_factions_friendly(
                        creature.faction_template,
                        candidate.faction_template,
                    ))
                && distance_yd(creature, candidate) <= radius as f32
                && predicate(candidate)
        })
        .min_by_key(|candidate| candidate.guid)
        .map(|candidate| candidate.guid)
}

fn event_target_context(context: &EventContext, guid: u64) -> EventContext {
    EventContext {
        event_target_guid: Some(guid),
        ..*context
    }
}

pub(crate) fn beneficiary_guid<W: EventAiWorld>(world: &W, guid: u64) -> Option<u64> {
    let invoker = world.eventai_unit(guid)?;
    (invoker.owner_guid != 0 && world.eventai_unit(invoker.owner_guid).is_some())
        .then_some(invoker.owner_guid)
        .or(Some(invoker.guid))
}

fn compare_distance(origin: &EventAiUnit, first: &EventAiUnit, second: &EventAiUnit) -> Ordering {
    distance_sq(origin, first)
        .partial_cmp(&distance_sq(origin, second))
        .unwrap_or(Ordering::Equal)
        .then(first.guid.cmp(&second.guid))
}

fn actor_is_behind_target(actor: &EventAiUnit, target: &EventAiUnit) -> bool {
    let direction = (actor.y - target.y).atan2(actor.x - target.x);
    (direction - target.orientation).cos() < 0.0
}

/// The creature's threat list, hostiles first by threat then by guid, with every target the
/// action could never take already gone.
fn ranked_threat<W: EventAiWorld>(
    world: &W,
    creature: &EventAiUnit,
    target: InstructionTarget,
    cast: Option<&CastInstruction>,
    admission: SpellCasterAdmission,
) -> Vec<u64> {
    let mut ranked: Vec<(u64, i64)> = world
        .eventai_threat(creature.guid)
        .into_iter()
        .filter_map(|(guid, threat)| {
            let source = world.eventai_unit(guid)?;
            candidate_eligible(world, creature, &source, target, cast, admission, false)
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
    admission: SpellCasterAdmission,
) -> Option<u64> {
    ranked_threat(world, creature, target, cast, admission)
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
    admission: SpellCasterAdmission,
) -> Option<u64> {
    ranked_threat(world, creature, target, cast, admission)
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

fn candidate_eligible<W: EventAiWorld>(
    world: &W,
    actor: &EventAiUnit,
    candidate: &EventAiUnit,
    target: InstructionTarget,
    cast: Option<&CastInstruction>,
    admission: SpellCasterAdmission,
    allow_dead_selected: bool,
) -> bool {
    if (candidate.dead
        && !allow_dead_selected
        && !(admission == SpellCasterAdmission::DeadCreatureCallback
            && candidate.guid == actor.guid
            && !candidate.is_player))
        || candidate.map_id != actor.map_id
        || candidate.instance_id != actor.instance_id
    {
        return false;
    }
    let hostile = matches!(
        target,
        InstructionTarget::CurrentOpponent
            | InstructionTarget::HighestThreat
            | InstructionTarget::SecondThreat
            | InstructionTarget::RandomThreat
            | InstructionTarget::RandomThreatExceptHighest
            | InstructionTarget::HighestThreatCharacter
            | InstructionTarget::RandomThreatCharacter
            | InstructionTarget::RandomThreatCharacterExceptHighest
            | InstructionTarget::RandomHostileManaUser
            | InstructionTarget::EligibleCasterArea
            | InstructionTarget::FarthestHostile
    );
    if hostile
        && !world.eventai_factions_hostile(actor.faction_template, candidate.faction_template)
    {
        return false;
    }
    let character = matches!(
        target,
        InstructionTarget::HighestThreatCharacter
            | InstructionTarget::RandomThreatCharacter
            | InstructionTarget::RandomThreatCharacterExceptHighest
    );
    if character && !candidate.is_player {
        return false;
    }
    if target == InstructionTarget::RandomHostileManaUser
        && candidate.power_type != lyracore_shared::packing::power_type::MANA
    {
        return false;
    }
    cast.is_none_or(|cast| {
        selection_spell_eligible(
            world,
            actor,
            candidate,
            cast,
            admission,
            allow_dead_selected,
        )
    })
}

fn selection_spell_eligible<W: EventAiWorld>(
    world: &W,
    actor: &EventAiUnit,
    selected: &EventAiUnit,
    cast: &CastInstruction,
    admission: SpellCasterAdmission,
    allow_dead_selected: bool,
) -> bool {
    if cast.start_mode == SpellStartMode::Triggered {
        return true;
    }
    if cast.caster_role == SpellCasterRole::Selected
        && cast.target_role == SpellTargetRole::Actor
        && !spell_target_eligible(world, actor, selected, cast, admission, allow_dead_selected)
    {
        return false;
    }
    let caster = match cast.caster_role {
        SpellCasterRole::Actor => actor,
        SpellCasterRole::Selected => selected,
    };
    let target = match cast.target_role {
        SpellTargetRole::Selected => selected,
        SpellTargetRole::Actor => actor,
        SpellTargetRole::Caster => caster,
        SpellTargetRole::CasterArea => selected,
        SpellTargetRole::None => caster,
    };
    spell_target_eligible(
        world,
        caster,
        target,
        cast,
        admission,
        allow_dead_selected && target.guid == selected.guid,
    )
}

fn spell_target_eligible<W: EventAiWorld>(
    world: &W,
    caster: &EventAiUnit,
    target: &EventAiUnit,
    cast: &CastInstruction,
    admission: SpellCasterAdmission,
    allow_dead_target: bool,
) -> bool {
    if (target.dead
        && !allow_dead_target
        && !(admission == SpellCasterAdmission::DeadCreatureCallback
            && target.guid == caster.guid
            && !target.is_player))
        || target.map_id != caster.map_id
        || target.instance_id != caster.instance_id
        || !world.eventai_line_of_sight(caster, target)
        || (cast.character_only && !target.is_player)
        || (cast.aura_absent && world.eventai_has_aura(target.guid, cast.spell_id))
        || (cast.target_must_be_casting && !world.eventai_is_casting(target.guid))
    {
        return false;
    }
    let Some(range) = world.eventai_spell_range(cast.spell_id) else {
        return false;
    };
    range == 0 || target.guid == caster.guid || distance_yd(caster, target) <= range as f32
}

fn killed_invoker_target(
    context: &EventContext,
    target_policy: InstructionTarget,
    selected_guid: u64,
) -> bool {
    context.kind == EventKind::OnKill
        && target_policy == InstructionTarget::Invoker
        && context.invoker_guid == Some(selected_guid)
}

fn spell_caster_admission(context: &EventContext, actor: &EventAiUnit) -> SpellCasterAdmission {
    if matches!(
        context.kind,
        EventKind::OnDeath | EventKind::OnSpellHit | EventKind::OnSpellHitTarget
    ) && !actor.is_player
    {
        SpellCasterAdmission::DeadCreatureCallback
    } else {
        SpellCasterAdmission::Living
    }
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
fn pick<T: Copy>(choice: u64, candidates: &[T]) -> Option<T> {
    if candidates.is_empty() {
        return None;
    }
    candidates.get(choice as usize % candidates.len()).copied()
}

fn distance_yd(first: &EventAiUnit, second: &EventAiUnit) -> f32 {
    distance_sq(first, second).sqrt()
}

fn distance_sq(first: &EventAiUnit, second: &EventAiUnit) -> f32 {
    let (dx, dy, dz) = (second.x - first.x, second.y - first.y, second.z - first.z);
    dx * dx + dy * dy + dz * dz
}
