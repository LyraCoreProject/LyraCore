//! Operator-only loading of normalized EventAI definitions.

use std::collections::HashSet;

use spacetimedb::{reducer, ReducerContext, Table};

use super::{
    CallForHelpInstruction, CastInstruction, CreatureAiDefinition, CreatureHealthCondition,
    CreatureInstruction, EmoteInstruction, EventAiRule, EventCondition, ExecutionPolicy,
    FriendlyHealthDeficitCondition, InstructionSelection, InstructionTarget, PhaseSet,
    RangedPostureInstruction, RecurrencePolicy, SetPhaseInstruction, SpeakInstruction, SpeechMode,
    SummonInstruction, TargetRangeCondition, TimeWindow,
};
use crate::game_creature_ai_definition;
#[cfg(feature = "debug_reducers")]
use crate::{
    game_creature_ai_rule_state, game_creature_ai_state, game_melee_attack, game_world_entity,
};

/// Replace all normalized definitions and load the first batch.
#[reducer]
pub fn import_creature_ai_definitions(ctx: &ReducerContext, packed: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    load_definition_batch(ctx, &packed, true)
}

/// Append a validated batch after [`import_creature_ai_definitions`].
#[reducer]
pub fn import_creature_ai_definitions_append(
    ctx: &ReducerContext,
    packed: String,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    load_definition_batch(ctx, &packed, false)
}

fn load_definition_batch(ctx: &ReducerContext, packed: &str, replace: bool) -> Result<(), String> {
    let definitions = parse_definition_batch(packed)?;
    let table = ctx.db.game_creature_ai_definition();
    if !replace {
        for definition in &definitions {
            let duplicate = if definition.creature_entry != 0 {
                table
                    .by_entry()
                    .filter(&definition.creature_entry)
                    .next()
                    .is_some()
            } else {
                table
                    .by_guid()
                    .filter(&definition.creature_guid)
                    .next()
                    .is_some()
            };
            if duplicate {
                return Err(format!(
                    "definition subject already loaded: entry={} guid={}",
                    definition.creature_entry, definition.creature_guid
                ));
            }
        }
    }
    if replace {
        for id in table.iter().map(|row| row.id).collect::<Vec<_>>() {
            table.id().delete(id);
        }
    }
    for definition in definitions {
        table.insert(definition);
    }
    Ok(())
}

fn parse_definition_batch(packed: &str) -> Result<Vec<CreatureAiDefinition>, String> {
    let mut subjects = HashSet::new();
    packed
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let definition = parse_definition(line)?;
            let subject = (definition.creature_entry, definition.creature_guid);
            if !subjects.insert(subject) {
                return Err(format!(
                    "definition subject appears twice: entry={} guid={}",
                    subject.0, subject.1
                ));
            }
            Ok(definition)
        })
        .collect()
}

fn parse_definition(line: &str) -> Result<CreatureAiDefinition, String> {
    let mut fields = line.splitn(3, '@');
    let subject = fields.next().ok_or("definition subject is missing")?;
    let revision = parse_u64(fields.next().ok_or("definition revision is missing")?)?;
    let encoded_rules = fields.next().ok_or("definition rules are missing")?;
    if encoded_rules.is_empty() {
        return Err("definition has no rules".to_string());
    }
    let expected_revision = definition_revision(&format!("{subject}@{encoded_rules}"));
    if revision != expected_revision {
        return Err(format!(
            "definition revision mismatch: supplied={revision} expected={expected_revision}"
        ));
    }

    let (creature_entry, creature_guid) = if let Some(value) = subject.strip_prefix("entry:") {
        let entry = parse_u32(value)?;
        if entry == 0 {
            return Err("entry subject must be nonzero".to_string());
        }
        (entry, 0)
    } else if let Some(value) = subject.strip_prefix("guid:") {
        let guid = parse_u64(value)?;
        if guid == 0 {
            return Err("guid subject must be nonzero".to_string());
        }
        (0, guid)
    } else {
        return Err(format!("unknown definition subject: {subject}"));
    };

    let rules: Vec<EventAiRule> = encoded_rules
        .split('~')
        .map(parse_rule)
        .collect::<Result<_, _>>()?;
    if rules
        .windows(2)
        .any(|pair| pair[0].source_rule_id >= pair[1].source_rule_id)
    {
        return Err("definition rules must have unique ascending source ids".to_string());
    }
    Ok(CreatureAiDefinition {
        id: 0,
        creature_entry,
        creature_guid,
        definition_revision: revision,
        rules,
    })
}

fn parse_rule(encoded: &str) -> Result<EventAiRule, String> {
    let fields: Vec<&str> = encoded.splitn(8, ',').collect();
    if fields.len() != 8 {
        return Err(format!(
            "definition rule needs 8 fields, got {}: {encoded}",
            fields.len()
        ));
    }
    let source_rule_id = parse_u64(fields[0])?;
    if source_rule_id == 0 {
        return Err("source rule id must be nonzero".to_string());
    }
    let chance_pct = parse_u8(fields[2])?;
    if !(1..=100).contains(&chance_pct) {
        return Err(format!("chance must be 1..=100: {chance_pct}"));
    }
    let allowed_phases = PhaseSet {
        bits: parse_u32(fields[3])?,
    };
    if allowed_phases.bits == 0 {
        return Err("allowed phase set must be nonempty".to_string());
    }
    let instructions = fields[7]
        .split('+')
        .map(parse_instruction)
        .collect::<Result<Vec<_>, _>>()?;
    if instructions.is_empty() {
        return Err("rule has no instructions".to_string());
    }
    Ok(EventAiRule {
        source_rule_id,
        event: parse_event(fields[1])?,
        chance_pct,
        allowed_phases,
        recurrence: parse_recurrence(fields[4])?,
        selection: match fields[5] {
            "all" => InstructionSelection::All,
            "random" => InstructionSelection::RandomOne,
            value => return Err(format!("unknown instruction selection: {value}")),
        },
        execution: match fields[6] {
            "ordinary" => ExecutionPolicy::Ordinary,
            "combat" => ExecutionPolicy::CombatAction,
            value => return Err(format!("unknown execution policy: {value}")),
        },
        instructions,
    })
}

fn parse_event(encoded: &str) -> Result<EventCondition, String> {
    let fields: Vec<&str> = encoded.split(':').collect();
    match fields.as_slice() {
        ["aggro"] => Ok(EventCondition::OnAggro),
        ["timer", min, max] => Ok(EventCondition::TimedInCombat(window(min, max)?)),
        ["health", min, max] => {
            let min_pct = parse_u8(min)?;
            let max_pct = parse_u8(max)?;
            if min_pct > max_pct || max_pct > 100 {
                return Err(format!("invalid health percentage window: {encoded}"));
            }
            Ok(EventCondition::CreatureHealth(CreatureHealthCondition {
                min_pct,
                max_pct,
            }))
        }
        ["death"] => Ok(EventCondition::OnDeath),
        ["range", min, max] => {
            let min_yd = parse_u32(min)?;
            let max_yd = parse_u32(max)?;
            if min_yd > max_yd {
                return Err(format!("invalid range window: {encoded}"));
            }
            Ok(EventCondition::TargetRange(TargetRangeCondition {
                min_yd,
                max_yd,
            }))
        }
        ["spawn"] => Ok(EventCondition::OnSpawn),
        ["friendly-health", missing, radius] => Ok(EventCondition::FriendlyHealthDeficit(
            FriendlyHealthDeficitCondition {
                missing_health: parse_u32(missing)?,
                radius_yd: parse_u32(radius)?,
            },
        )),
        _ => Err(format!("unknown event condition: {encoded}")),
    }
}

fn parse_recurrence(encoded: &str) -> Result<RecurrencePolicy, String> {
    let fields: Vec<&str> = encoded.split(':').collect();
    match fields.as_slice() {
        ["once"] => Ok(RecurrencePolicy::Once),
        ["repeat", min, max] => Ok(RecurrencePolicy::Repeat(window(min, max)?)),
        _ => Err(format!("unknown recurrence policy: {encoded}")),
    }
}

fn parse_instruction(encoded: &str) -> Result<CreatureInstruction, String> {
    let fields: Vec<&str> = encoded.split(':').collect();
    match fields.as_slice() {
        ["speak", mode, target, ids] => {
            let broadcast_ids = ids
                .split('.')
                .map(parse_u32)
                .collect::<Result<Vec<_>, _>>()?;
            if broadcast_ids.is_empty() || broadcast_ids.contains(&0) {
                return Err("speak instruction needs nonzero broadcast ids".to_string());
            }
            Ok(CreatureInstruction::Speak(SpeakInstruction {
                mode: match *mode {
                    "say" => SpeechMode::Say,
                    "yell" => SpeechMode::Yell,
                    value => return Err(format!("unknown speech mode: {value}")),
                },
                broadcast_ids,
                legacy_text: String::new(),
                target: parse_target(target)?,
            }))
        }
        ["cast", spell, target, interrupt, triggered, aura_absent, character, casting] => {
            let spell_id = parse_u32(spell)?;
            if spell_id == 0 {
                return Err("cast spell must be nonzero".to_string());
            }
            Ok(CreatureInstruction::Cast(CastInstruction {
                spell_id,
                target: parse_target(target)?,
                interrupt_previous: parse_bool(interrupt)?,
                triggered: parse_bool(triggered)?,
                aura_absent: parse_bool(aura_absent)?,
                character_only: parse_bool(character)?,
                target_must_be_casting: parse_bool(casting)?,
            }))
        }
        ["emote", emote, target] => Ok(CreatureInstruction::Emote(EmoteInstruction {
            emote_id: parse_u32(emote)?,
            target: parse_target(target)?,
        })),
        ["flee"] => Ok(CreatureInstruction::FleeForAssist),
        ["help", radius] => Ok(CreatureInstruction::CallForHelp(CallForHelpInstruction {
            radius_yd: parse_u32(radius)?,
        })),
        ["phase", phase] => {
            let phase = parse_u8(phase)?;
            if phase >= 32 {
                return Err(format!("phase must be below 32: {phase}"));
            }
            Ok(CreatureInstruction::SetPhase(SetPhaseInstruction { phase }))
        }
        ["summon", entry, location, target] => {
            let creature_entry = parse_u32(entry)?;
            let summon_location_id = parse_u32(location)?;
            if creature_entry == 0 || summon_location_id == 0 {
                return Err("summon entry and location must be nonzero".to_string());
            }
            Ok(CreatureInstruction::Summon(SummonInstruction {
                creature_entry,
                summon_location_id,
                target: parse_target(target)?,
            }))
        }
        ["posture", distance, angle] => Ok(CreatureInstruction::SetRangedPosture(
            RangedPostureInstruction {
                distance_yd: parse_u32(distance)?,
                angle_degrees: parse_i32(angle)?,
            },
        )),
        _ => Err(format!("unknown creature instruction: {encoded}")),
    }
}

fn parse_target(value: &str) -> Result<InstructionTarget, String> {
    match value {
        "opponent" => Ok(InstructionTarget::CurrentOpponent),
        "self" => Ok(InstructionTarget::SelfActor),
        "highest-threat" => Ok(InstructionTarget::HighestThreat),
        "second-threat" => Ok(InstructionTarget::SecondThreat),
        "random-threat" => Ok(InstructionTarget::RandomThreat),
        "invoker" => Ok(InstructionTarget::Invoker),
        "event-subject" => Ok(InstructionTarget::EventSubject),
        "highest-threat-character" => Ok(InstructionTarget::HighestThreatCharacter),
        "random-threat-character" => Ok(InstructionTarget::RandomThreatCharacter),
        "eligible-caster-area" => Ok(InstructionTarget::EligibleCasterArea),
        "farthest-hostile" => Ok(InstructionTarget::FarthestHostile),
        _ => Err(format!("unknown instruction target: {value}")),
    }
}

fn window(min: &str, max: &str) -> Result<TimeWindow, String> {
    let min_ms = parse_u32(min)?;
    let max_ms = parse_u32(max)?;
    if min_ms > max_ms {
        return Err(format!("invalid time window: {min_ms}..{max_ms}"));
    }
    Ok(TimeWindow { min_ms, max_ms })
}

fn definition_revision(material: &str) -> u64 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"lyracore-eventai-definition-v1");
    hasher.update(material.as_bytes());
    u64::from_le_bytes(
        hasher.finalize().as_bytes()[..8]
            .try_into()
            .expect("a BLAKE3 digest has at least eight bytes"),
    )
}

fn parse_bool(value: &str) -> Result<bool, String> {
    match value {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(format!("boolean must be 0 or 1: {value}")),
    }
}

fn parse_u64(value: &str) -> Result<u64, String> {
    value.parse().map_err(|_| format!("invalid u64: {value}"))
}

fn parse_u32(value: &str) -> Result<u32, String> {
    value.parse().map_err(|_| format!("invalid u32: {value}"))
}

fn parse_u8(value: &str) -> Result<u8, String> {
    value.parse().map_err(|_| format!("invalid u8: {value}"))
}

fn parse_i32(value: &str) -> Result<i32, String> {
    value.parse().map_err(|_| format!("invalid i32: {value}"))
}

/// Load one definition and keep its creature in combat for the durable integration test.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_stage_eventai_revision_fixture(
    ctx: &ReducerContext,
    creature_guid: u64,
    target_guid: u64,
    packed: String,
) -> Result<(), String> {
    load_definition_batch(ctx, &packed, true)?;

    let entities = ctx.db.game_world_entity();
    entities
        .guid()
        .find(creature_guid)
        .ok_or_else(|| format!("fixture creature does not exist: {creature_guid}"))?;
    let mut target = entities
        .guid()
        .find(target_guid)
        .ok_or_else(|| format!("fixture target does not exist: {target_guid}"))?;
    target.health = 100_000;
    target.max_health = 100_000;
    entities.guid().update(target);

    let fights = ctx.db.game_melee_attack();
    if let Some(mut fight) = fights.attacker_guid().find(creature_guid) {
        fight.target_guid = target_guid;
        fight.last_swing_ms = 0;
        fight.last_offhand_swing_ms = 0;
        fight.rout_ends_ms = 0;
        fight.pursuit_ends_ms = 0;
        fights.attacker_guid().update(fight);
    } else {
        fights.insert(crate::MeleeAttack {
            attacker_guid: creature_guid,
            target_guid,
            last_swing_ms: 0,
            ranged_spell_id: 0,
            last_offhand_swing_ms: 0,
            rout_ends_ms: 0,
            pursuit_ends_ms: 0,
            leash_x: 0.0,
            leash_y: 0.0,
        });
    }
    Ok(())
}

/// Verify the durable fixture from a separate Module call without exposing private tables.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_verify_eventai_revision_fixture(
    ctx: &ReducerContext,
    creature_guid: u64,
    source_rule_id: u64,
    expect_rule_state: bool,
) -> Result<(), String> {
    let definition = super::combat::definition_for(ctx, creature_guid);
    let [rule] = definition.rules.as_slice() else {
        return Err(format!(
            "fixture needs one effective rule, got {}",
            definition.rules.len()
        ));
    };
    if rule.source_rule_id != source_rule_id {
        return Err(format!(
            "fixture source rule mismatch: actual={} expected={source_rule_id}",
            rule.source_rule_id
        ));
    }

    let state = ctx
        .db
        .game_creature_ai_state()
        .creature_guid()
        .find(creature_guid)
        .ok_or_else(|| "scheduled visit has not created creature state".to_string())?;
    if state.definition_revision != definition.revision.value {
        return Err(format!(
            "state revision has not adopted the definition: state={} definition={}",
            state.definition_revision, definition.revision.value
        ));
    }

    let rule_states = ctx
        .db
        .game_creature_ai_rule_state()
        .by_creature()
        .filter(&creature_guid)
        .collect::<Vec<_>>();
    if expect_rule_state {
        if rule.instructions.len() != 4
            || !matches!(rule.event, EventCondition::CreatureHealth(_))
            || state.phase != 3
            || !state.ranged_posture_active
        {
            return Err("the scheduled four-instruction rule has not completed".to_string());
        }
        let [rule_state] = rule_states.as_slice() else {
            return Err(format!(
                "fixture needs one durable Rule State, got {}",
                rule_states.len()
            ));
        };
        let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64;
        if rule_state.source_rule_id != source_rule_id
            || rule_state.consumed
            || rule_state.next_eligible_ms <= now_ms.saturating_add(50_000)
        {
            return Err("the recurring Rule State did not survive its scheduled calls".to_string());
        }
    } else if !matches!(rule.event, EventCondition::OnAggro)
        || rule.instructions.len() != 1
        || state.phase != 0
        || state.ranged_posture_active
        || state.ranged_distance != 0.0
        || state.ranged_angle != 0.0
        || !rule_states.is_empty()
    {
        return Err("revision cleanup is not complete or replayed the aggro edge".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_definition_decodes_more_than_three_ordered_instructions() {
        let rules = "17,aggro,100,4294967295,once,all,ordinary,emote:1:self+emote:2:self+emote:3:self+emote:4:self";
        let material = format!("entry:6@{rules}");
        let packed = format!("entry:6@{}@{rules}", definition_revision(&material));
        let definition = parse_definition(&packed).unwrap();
        assert_eq!(definition.rules[0].instructions.len(), 4);
        for (index, instruction) in definition.rules[0].instructions.iter().enumerate() {
            let CreatureInstruction::Emote(emote) = instruction else {
                panic!("instruction is not an emote");
            };
            assert_eq!(emote.emote_id, index as u32 + 1);
        }
    }

    #[test]
    fn revision_covers_the_normalized_definition() {
        let first = "entry:6@17,aggro,100,1,once,all,ordinary,flee";
        let second = "entry:6@17,aggro,100,1,once,all,ordinary,help:8";
        assert_ne!(definition_revision(first), definition_revision(second));
    }
}
