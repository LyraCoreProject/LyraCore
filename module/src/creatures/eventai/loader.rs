//! Operator-only loading of normalized EventAI definitions.

use std::collections::HashSet;

use spacetimedb::{reducer, ReducerContext, Table};

use super::{
    AiEventKind, AuraStackCondition, CallForHelpInstruction, CastInstruction, CreatureAiDefinition,
    CreatureEntryCondition, CreatureHealthCondition, CreatureInstruction,
    CreaturePresentationInstruction, CreaturePresentationMount, DeathCondition, EmoteInstruction,
    EvadeInstruction, EventAiRule, EventCondition, EventPredicate, ExecutionPolicy,
    FacingCondition, FacingInstruction, FriendlyAuraSelection, FriendlyCrowdControlCondition,
    FriendlyHealthDeficitCondition, FriendlyMissingAuraCondition, IdleMovementIntent,
    ImmobilizationInstruction, IncrementPhaseInstruction, InstructionSelection, InstructionTarget,
    KillCondition, MovementOperation, MovementSwitch, NotifyEncounterInstruction,
    OutOfCombatSightCondition, PatrolIntent, PatrolPause, PercentageCondition, PhaseSet,
    PostureAdmission, QuestTakenPredicate, RandomMovementIntent, RandomPhaseInstruction,
    RandomPhaseRangeInstruction, RangedMode, RangedModeInstruction, RangedPostureInstruction,
    ReceiveAiEventCondition, ReceiveEmoteCondition, RecurrencePolicy, ScaleAllThreatInstruction,
    ScaleSelectedThreatInstruction, SetLethalDamageFloorInstruction, SetPhaseInstruction,
    SpawnCondition, SpawnMapCondition, SpawnZoneOrAreaCondition, SpeakInstruction, SpeechMode,
    SpellCasterRole, SpellEventCondition, SpellStartMode, SpellTargetRole, StartRelayInstruction,
    SummonInstruction, TargetRangeCondition, TimeWindow, WalkingMode,
};
use crate::creatures::presentation::NpcFlagsProjection;
use crate::game_creature_ai_definition;
use crate::quest::{KillCredit, QuestCreditRecipientPolicy, QuestEvent};
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

#[cfg(feature = "debug_reducers")]
pub(crate) fn replace_definition_for_debug(
    ctx: &ReducerContext,
    packed: &str,
) -> Result<(), String> {
    load_definition_batch(ctx, packed, true)
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
        .map(|encoded| parse_rule_for_subject(encoded, creature_guid))
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

#[cfg(test)]
fn parse_rule(encoded: &str) -> Result<EventAiRule, String> {
    parse_rule_for_subject(encoded, 0)
}

fn parse_rule_for_subject(encoded: &str, creature_guid: u64) -> Result<EventAiRule, String> {
    let fields: Vec<&str> = encoded.splitn(9, ',').collect();
    if !(8..=9).contains(&fields.len()) {
        return Err(format!(
            "definition rule needs 8 or 9 fields, got {}: {encoded}",
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
    let (posture, encoded_instructions) = if fields.len() == 9 {
        (
            match fields[7] {
                "any-posture" => PostureAdmission::Any,
                "ranged-only" => PostureAdmission::RangedOnly,
                "melee-only" => PostureAdmission::MeleeOnly,
                value => return Err(format!("unknown posture admission: {value}")),
            },
            fields[8],
        )
    } else {
        (PostureAdmission::Any, fields[7])
    };
    let instructions = encoded_instructions
        .split('+')
        .map(|instruction| {
            parse_import_verified_instruction(instruction, source_rule_id, creature_guid)
        })
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
        posture,
        instructions,
    })
}

fn parse_import_verified_instruction(
    encoded: &str,
    source_rule_id: u64,
    creature_guid: u64,
) -> Result<CreatureInstruction, String> {
    const RAJAXX_RULE_ID: u64 = 1_534_108;
    const RAJAXX_CREATURE_GUID: u64 = 17_379_391_219_402_170_660;

    if encoded != "unit-flags:set:rajaxx-spawn-protection" {
        return parse_instruction(encoded);
    }
    if source_rule_id != RAJAXX_RULE_ID || creature_guid != RAJAXX_CREATURE_GUID {
        return Err("Rajaxx client projection is reserved for its pinned source rule".to_string());
    }
    Ok(CreatureInstruction::Presentation(
        super::import_verified_rajaxx_spawn_protection(),
    ))
}

fn parse_event(encoded: &str) -> Result<EventCondition, String> {
    let fields: Vec<&str> = encoded.split(':').collect();
    match fields.as_slice() {
        ["aggro"] => Ok(EventCondition::OnAggro),
        ["timer", min, max] | ["timer-combat", min, max] => {
            Ok(EventCondition::TimedInCombat(window(min, max)?))
        }
        ["timer-ooc", min, max] => Ok(EventCondition::TimedOutOfCombat(window(min, max)?)),
        ["health", min, max] => {
            let min_pct = parse_u8(min)?;
            let max_pct = parse_u8(max)?;
            if min_pct > max_pct || max_pct > 100 {
                return Err(format!("invalid health percentage window: {encoded}"));
            }
            Ok(EventCondition::CreatureHealth(CreatureHealthCondition {
                min_pct,
                max_pct,
                allow_out_of_combat: false,
            }))
        }
        ["health", min, max, allow_ooc] => {
            let min_pct = parse_u8(min)?;
            let max_pct = parse_u8(max)?;
            if min_pct > max_pct || max_pct > 100 {
                return Err(format!("invalid health percentage window: {encoded}"));
            }
            Ok(EventCondition::CreatureHealth(CreatureHealthCondition {
                min_pct,
                max_pct,
                allow_out_of_combat: parse_bool(allow_ooc)?,
            }))
        }
        ["power", min, max] => Ok(EventCondition::CreaturePower(percent(min, max)?)),
        ["kill", character_only] => Ok(EventCondition::OnKill(KillCondition {
            character_only: parse_bool(character_only)?,
        })),
        ["death", predicate] => Ok(EventCondition::OnDeath(DeathCondition {
            predicate: parse_event_predicate(predicate)?,
        })),
        ["evade"] => Ok(EventCondition::OnEvade),
        ["spell-hit", spell, school] => Ok(EventCondition::OnSpellHit(SpellEventCondition {
            spell_id: parse_u32(spell)?,
            school_mask: parse_u32(school)?,
        })),
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
        ["ooc-los", non_hostile, range, character, condition] => Ok(
            EventCondition::OutOfCombatSight(OutOfCombatSightCondition {
                require_non_hostile: parse_bool(non_hostile)?,
                max_range_yd: parse_u32(range)?,
                character_only: parse_bool(character)?,
                predicate: parse_event_predicate(condition)?,
            }),
        ),
        ["spawn"] | ["spawn", "always"] => Ok(EventCondition::OnSpawn(SpawnCondition::Always)),
        ["spawn", "map", value] => Ok(EventCondition::OnSpawn(SpawnCondition::Map(
            SpawnMapCondition {
                map_id: parse_u32(value)?,
            },
        ))),
        ["spawn", "zone-or-area", value] => Ok(EventCondition::OnSpawn(
            SpawnCondition::ZoneOrArea(SpawnZoneOrAreaCondition {
                zone_or_area_id: parse_u32(value)?,
            }),
        )),
        ["target-health", min, max] => Ok(EventCondition::TargetHealth(percent(min, max)?)),
        ["target-casting"] => Ok(EventCondition::TargetCasting),
        ["friendly-health", missing, radius] => Ok(EventCondition::FriendlyHealthDeficit(
            FriendlyHealthDeficitCondition {
                missing_health: parse_u32(missing)?,
                radius_yd: parse_u32(radius)?,
                percent: false,
            },
        )),
        ["friendly-health", missing, radius, percent] => Ok(EventCondition::FriendlyHealthDeficit(
            FriendlyHealthDeficitCondition {
                missing_health: parse_u32(missing)?,
                radius_yd: parse_u32(radius)?,
                percent: parse_bool(percent)?,
            },
        )),
        ["friendly-cc", radius] => Ok(EventCondition::FriendlyCrowdControlled(
            FriendlyCrowdControlCondition {
                radius_yd: parse_u32(radius)?,
            },
        )),
        ["friendly-missing-aura", spell, radius, selection] => Ok(
            EventCondition::FriendlyMissingAura(FriendlyMissingAuraCondition {
                spell_id: parse_u32(spell)?,
                radius_yd: parse_u32(radius)?,
                selection: match *selection {
                    "nearby-engaged" => FriendlyAuraSelection::NearbyWhileEngaged,
                    "match-actor-combat" => FriendlyAuraSelection::MatchActorCombatState,
                    "any-while-disengaged" => FriendlyAuraSelection::AnyWhileDisengaged,
                    value => return Err(format!("unknown friendly aura selection: {value}")),
                },
            }),
        ),
        ["summoned", entry] => Ok(EventCondition::OnSummoned(CreatureEntryCondition {
            creature_entry: parse_u32(entry)?,
        })),
        ["target-power", min, max] => Ok(EventCondition::TargetPower(percent(min, max)?)),
        ["home"] => Ok(EventCondition::OnReachedHome),
        ["receive-emote", emote, condition] => {
            Ok(EventCondition::OnReceiveEmote(ReceiveEmoteCondition {
                emote_id: parse_u32(emote)?,
                predicate: parse_event_predicate(condition)?,
            }))
        }
        ["aura", spell, stacks] => Ok(EventCondition::CreatureAura(AuraStackCondition {
            spell_id: parse_u32(spell)?,
            stacks: parse_u32(stacks)?,
        })),
        ["target-aura", spell, stacks] => Ok(EventCondition::TargetAura(AuraStackCondition {
            spell_id: parse_u32(spell)?,
            stacks: parse_u32(stacks)?,
        })),
        ["summoned-death", entry] => Ok(EventCondition::OnSummonedDeath(CreatureEntryCondition {
            creature_entry: parse_u32(entry)?,
        })),
        ["missing-aura", spell, stacks] => {
            Ok(EventCondition::CreatureMissingAura(AuraStackCondition {
                spell_id: parse_u32(spell)?,
                stacks: parse_u32(stacks)?,
            }))
        }
        ["target-missing-aura", spell, stacks] => {
            Ok(EventCondition::TargetMissingAura(AuraStackCondition {
                spell_id: parse_u32(spell)?,
                stacks: parse_u32(stacks)?,
            }))
        }
        ["timer-generic", min, max] => Ok(EventCondition::TimedGeneric(window(min, max)?)),
        ["ai-event", event_name, sender] => {
            Ok(EventCondition::OnReceiveAiEvent(ReceiveAiEventCondition {
                kind: parse_ai_event(event_name)?,
                sender_entry: parse_u32(sender)?,
            }))
        }
        ["select-attacking", min, max] => Ok(EventCondition::SelectAttackingTarget(
            TargetRangeCondition {
                min_yd: parse_u32(min)?,
                max_yd: parse_u32(max)?,
            },
        )),
        ["facing", behind] => Ok(EventCondition::FacingTarget(FacingCondition {
            behind: parse_bool(behind)?,
        })),
        ["spell-hit-target", spell, school] => {
            Ok(EventCondition::OnSpellHitTarget(SpellEventCondition {
                spell_id: parse_u32(spell)?,
                school_mask: parse_u32(school)?,
            }))
        }
        ["target-not-reachable"] => Ok(EventCondition::TargetNotReachable),
        _ => Err(format!("unknown event condition: {encoded}")),
    }
}

fn parse_event_predicate(encoded: &str) -> Result<EventPredicate, String> {
    match encoded {
        "always" => Ok(EventPredicate::Always),
        "alliance" => Ok(EventPredicate::Alliance),
        "horde" => Ok(EventPredicate::Horde),
        value => value
            .strip_prefix("quest-taken.")
            .map(parse_u32)
            .transpose()?
            .map(|quest_entry| EventPredicate::QuestTaken(QuestTakenPredicate { quest_entry }))
            .ok_or_else(|| format!("unknown EventAI predicate: {encoded}")),
    }
}

fn parse_recurrence(encoded: &str) -> Result<RecurrencePolicy, String> {
    let fields: Vec<&str> = encoded.split(':').collect();
    match fields.as_slice() {
        ["once"] => Ok(RecurrencePolicy::Once),
        ["repeat", min, max] => Ok(RecurrencePolicy::Repeat(window(min, max)?)),
        ["repeat-event"] => Ok(RecurrencePolicy::RepeatOnEvent),
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
                start_mode: if parse_bool(triggered)? {
                    SpellStartMode::Triggered
                } else {
                    SpellStartMode::Direct
                },
                caster_role: SpellCasterRole::Actor,
                target_role: SpellTargetRole::Selected,
                aura_absent: parse_bool(aura_absent)?,
                character_only: parse_bool(character)?,
                target_must_be_casting: parse_bool(casting)?,
                main_spell: false,
                distance_after_start: false,
            }))
        }
        ["cast", spell, target, interrupt, start, caster, target_role, aura_absent, character, casting, main, distance] =>
        {
            let spell_id = parse_u32(spell)?;
            if spell_id == 0 {
                return Err("cast spell must be nonzero".to_string());
            }
            Ok(CreatureInstruction::Cast(CastInstruction {
                spell_id,
                target: parse_target(target)?,
                interrupt_previous: parse_bool(interrupt)?,
                start_mode: match *start {
                    "direct" => SpellStartMode::Direct,
                    "triggered" => SpellStartMode::Triggered,
                    value => return Err(format!("unknown spell start mode: {value}")),
                },
                caster_role: match *caster {
                    "actor" => SpellCasterRole::Actor,
                    "selected" => SpellCasterRole::Selected,
                    value => return Err(format!("unknown spell caster role: {value}")),
                },
                target_role: match *target_role {
                    "selected" => SpellTargetRole::Selected,
                    "actor" => SpellTargetRole::Actor,
                    "caster" => SpellTargetRole::Caster,
                    "none" => SpellTargetRole::None,
                    "caster-area" => SpellTargetRole::CasterArea,
                    value => return Err(format!("unknown spell target role: {value}")),
                },
                aura_absent: parse_bool(aura_absent)?,
                character_only: parse_bool(character)?,
                target_must_be_casting: parse_bool(casting)?,
                main_spell: parse_bool(main)?,
                distance_after_start: parse_bool(distance)?,
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
        ["phase-inc", amount] => Ok(CreatureInstruction::IncrementPhase(
            IncrementPhaseInstruction {
                amount: parse_i32(amount)?,
            },
        )),
        ["phase-random", phases] => {
            let phases = phases
                .split('.')
                .map(parse_u8)
                .collect::<Result<Vec<_>, _>>()?;
            if phases.is_empty() || phases.iter().any(|phase| *phase >= 32) {
                return Err("random phases must be nonempty and below 32".to_string());
            }
            Ok(CreatureInstruction::RandomPhase(RandomPhaseInstruction {
                phases,
            }))
        }
        ["phase-range", min, max] => {
            let min_phase = parse_u8(min)?;
            let max_phase = parse_u8(max)?;
            if min_phase >= max_phase || max_phase >= 32 {
                return Err(format!(
                    "invalid random phase range: {min_phase}..={max_phase}"
                ));
            }
            Ok(CreatureInstruction::RandomPhaseRange(
                RandomPhaseRangeInstruction {
                    min_phase,
                    max_phase,
                },
            ))
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
        ["lethal-floor", enabled] => Ok(CreatureInstruction::SetLethalDamageFloor(
            SetLethalDamageFloorInstruction {
                enabled: match *enabled {
                    "on" => true,
                    "off" => false,
                    value => return Err(format!("unknown lethal damage floor state: {value}")),
                },
            },
        )),
        ["force-death"] => Ok(CreatureInstruction::ForceDeath),
        ["threat-selected", percent, target] => Ok(CreatureInstruction::ScaleSelectedThreat(
            ScaleSelectedThreatInstruction {
                percent: parse_threat_percent(percent)?,
                target: parse_target(target)?,
            },
        )),
        ["threat-all", percent] => Ok(CreatureInstruction::ScaleAllThreat(
            ScaleAllThreatInstruction {
                percent: parse_threat_percent(percent)?,
            },
        )),
        ["faction", faction_template] => {
            let faction_template = parse_u32(faction_template)?;
            if faction_template == 0 {
                return Err("faction template must be nonzero".to_string());
            }
            Ok(CreatureInstruction::Presentation(
                CreaturePresentationInstruction::SetFaction { faction_template },
            ))
        }
        ["display-template", template_entry] => {
            let template_entry = parse_u32(template_entry)?;
            if template_entry == 0 {
                return Err("display template entry must be nonzero".to_string());
            }
            Ok(CreatureInstruction::Presentation(
                CreaturePresentationInstruction::ShowTemplateDisplay { template_entry },
            ))
        }
        ["creature-mount", mount] => Ok(CreatureInstruction::Presentation(
            CreaturePresentationInstruction::SetCreatureMount {
                mount: match *mount {
                    "clear" => CreaturePresentationMount::Clear,
                    "raider" => CreaturePresentationMount::Raider,
                    "kerr" => CreaturePresentationMount::Kerr,
                    "huntress" => CreaturePresentationMount::Huntress,
                    "twilight-marauder" => CreaturePresentationMount::TwilightMarauder,
                    value => return Err(format!("unknown creature mount: {value}")),
                },
            },
        )),
        ["npc-flags", flags] => Ok(CreatureInstruction::Presentation(
            CreaturePresentationInstruction::SetNpcFlags {
                flags: match *flags {
                    "clear" => NpcFlagsProjection::Clear,
                    "gossip-and-quest" => NpcFlagsProjection::GossipAndQuest,
                    value => return Err(format!("unknown NPC flag projection: {value}")),
                },
            },
        )),
        ["mana", "empty"] => Ok(CreatureInstruction::Presentation(
            CreaturePresentationInstruction::EmptyMana,
        )),
        ["virtual-main-hand", "clear"] => Ok(CreatureInstruction::Presentation(
            CreaturePresentationInstruction::ClearVirtualMainHand,
        )),
        ["unit-flags", "set", flag] => Ok(CreatureInstruction::Presentation(match *flag {
            "not-attackable" => CreaturePresentationInstruction::SetNotAttackable,
            "immune-to-players" => CreaturePresentationInstruction::SetImmuneToPlayers,
            "immune-to-creatures" => CreaturePresentationInstruction::SetImmuneToCreatures,
            "immune-to-players-and-creatures" => {
                CreaturePresentationInstruction::SetImmuneToPlayersAndCreatures
            }
            "not-selectable" => CreaturePresentationInstruction::SetNotSelectable,
            value => return Err(format!("unknown set unit flag: {value}")),
        })),
        ["unit-flags", "clear", flag] => Ok(CreatureInstruction::Presentation(match *flag {
            "not-attackable" => CreaturePresentationInstruction::ClearNotAttackable,
            "immune-to-players" => CreaturePresentationInstruction::ClearImmuneToPlayers,
            "immune-to-creatures" => CreaturePresentationInstruction::ClearImmuneToCreatures,
            "immune-to-players-and-creatures" => {
                CreaturePresentationInstruction::ClearImmuneToPlayersAndCreatures
            }
            value => return Err(format!("unknown clear unit flag: {value}")),
        })),
        ["quest-event", quest, recipient] => Ok(CreatureInstruction::QuestCredit(
            crate::quest::EventAiQuestCredit::QuestEvent(QuestEvent {
                quest_entry: nonzero_u32(quest, "quest event quest")?,
                recipient_policy: parse_quest_credit_recipient(recipient)?,
            }),
        )),
        ["kill-credit", creature, recipient] => Ok(CreatureInstruction::QuestCredit(
            crate::quest::EventAiQuestCredit::KillCredit(KillCredit {
                creature_entry: nonzero_u32(creature, "kill credit creature")?,
                recipient_policy: parse_quest_credit_recipient(recipient)?,
            }),
        )),
        ["idle", "stationary"] => Ok(CreatureInstruction::Movement(
            MovementOperation::ReplaceIdle(IdleMovementIntent::Stationary),
        )),
        ["idle", "random-current", radius] => Ok(CreatureInstruction::Movement(
            MovementOperation::ReplaceIdle(IdleMovementIntent::RandomAroundCurrentPosition(
                RandomMovementIntent {
                    radius_yd: parse_u32(radius)?,
                },
            )),
        )),
        ["idle", "patrol", path] => Ok(CreatureInstruction::Movement(
            MovementOperation::ReplaceIdle(IdleMovementIntent::Patrol(PatrolIntent {
                path_id: parse_u32(path)?,
            })),
        )),
        ["patrol-paused", paused] => Ok(CreatureInstruction::Movement(
            MovementOperation::SetPatrolPaused(PatrolPause {
                paused: parse_bool(paused)?,
            }),
        )),
        ["combat-movement", enabled] => Ok(CreatureInstruction::Movement(
            MovementOperation::SetCombatMovement(MovementSwitch {
                enabled: parse_bool(enabled)?,
            }),
        )),
        ["ranged-mode", mode, distance] => Ok(CreatureInstruction::Movement(
            MovementOperation::SetRangedMode(RangedModeInstruction {
                mode: match *mode {
                    "none" => RangedMode::None,
                    "full-caster" => RangedMode::FullCaster,
                    "proximity" => RangedMode::Proximity,
                    "no-melee" => RangedMode::NoMelee,
                    "distancer" => RangedMode::Distancer,
                    value => return Err(format!("unknown ranged mode: {value}")),
                },
                distance_yd: parse_u32(distance)?,
            }),
        )),
        ["facing", target, reset] => Ok(CreatureInstruction::SetFacing(FacingInstruction {
            target: parse_target(target)?,
            reset: parse_bool(reset)?,
        })),
        ["walking", mode] => Ok(CreatureInstruction::Movement(
            MovementOperation::SetWalking(match *mode {
                "run-default" => WalkingMode::RunByDefault,
                "walk-default" => WalkingMode::WalkByDefault,
                "run-chase" => WalkingMode::RunWhileChasing,
                "walk-chase" => WalkingMode::WalkWhileChasing,
                value => return Err(format!("unknown walking mode: {value}")),
            }),
        )),
        ["immobilized", enabled, combat_only] => Ok(CreatureInstruction::Movement(
            MovementOperation::SetImmobilized(ImmobilizationInstruction {
                enabled: parse_bool(enabled)?,
                combat_only: parse_bool(combat_only)?,
            }),
        )),
        ["follow-movement", enabled] => Ok(CreatureInstruction::Movement(
            MovementOperation::SetFollowMovement(MovementSwitch {
                enabled: parse_bool(enabled)?,
            }),
        )),
        ["evade", combat_only] => Ok(CreatureInstruction::Movement(MovementOperation::Evade(
            EvadeInstruction {
                combat_only: parse_bool(combat_only)?,
            },
        ))),
        ["notify-encounter", binding, signal] => Ok(CreatureInstruction::NotifyEncounter(
            NotifyEncounterInstruction {
                binding: parse_encounter_binding(binding)?,
                signal: parse_encounter_signal(signal)?,
            },
        )),
        ["start-relay", relays, target] => {
            let relay_ids = relays
                .split('.')
                .map(parse_u32)
                .collect::<Result<Vec<_>, _>>()?;
            if relay_ids.is_empty() || relay_ids.contains(&0) {
                return Err("start-relay needs nonzero definition ids".to_string());
            }
            Ok(CreatureInstruction::StartRelay(StartRelayInstruction {
                relay_ids,
                target: parse_target(target)?,
            }))
        }
        _ => Err(format!("unknown creature instruction: {encoded}")),
    }
}

fn parse_quest_credit_recipient(value: &str) -> Result<QuestCreditRecipientPolicy, String> {
    match value {
        "selected-character" => Ok(QuestCreditRecipientPolicy::SelectedCharacter),
        "invoker-beneficiary" => Ok(QuestCreditRecipientPolicy::InvokerBeneficiary),
        "tap-group" => Ok(QuestCreditRecipientPolicy::TapGroup),
        "eligible-group" => Ok(QuestCreditRecipientPolicy::EligibleGroup),
        _ => Err(format!("unknown quest credit recipient: {value}")),
    }
}

fn nonzero_u32(value: &str, label: &str) -> Result<u32, String> {
    let value = parse_u32(value)?;
    if value == 0 {
        return Err(format!("{label} must be nonzero"));
    }
    Ok(value)
}

fn parse_encounter_binding(value: &str) -> Result<crate::encounter::EncounterBinding, String> {
    use crate::encounter::EncounterBinding::*;
    match value {
        "blackfathom-deeps-kelris" => Ok(BlackfathomDeepsKelris),
        "blackrock-depths-tomb-of-seven" => Ok(BlackrockDepthsTombOfSeven),
        "dire-maul-alzzin" => Ok(DireMaulAlzzin),
        "razorfen-kraul-ward-keepers" => Ok(RazorfenKraulWardKeepers),
        "shadowfang-keep-rethilgore" => Ok(ShadowfangKeepRethilgore),
        "shadowfang-keep-fenrus" => Ok(ShadowfangKeepFenrus),
        "shadowfang-keep-nandos" => Ok(ShadowfangKeepNandos),
        "sunken-temple-avatar" => Ok(SunkenTempleAvatar),
        "wailing-caverns-anacondra" => Ok(WailingCavernsAnacondra),
        "wailing-caverns-cobrahn" => Ok(WailingCavernsCobrahn),
        "wailing-caverns-pythas" => Ok(WailingCavernsPythas),
        "wailing-caverns-serpentis" => Ok(WailingCavernsSerpentis),
        "wailing-caverns-mutanus" => Ok(WailingCavernsMutanus),
        "zul-gurub-ohgan" => Ok(ZulGurubOhgan),
        _ => Err(format!("unknown encounter binding: {value}")),
    }
}

fn parse_encounter_signal(value: &str) -> Result<crate::encounter::EncounterSignal, String> {
    use crate::encounter::EncounterSignal::*;
    match value {
        "begin" => Ok(Begin),
        "fail" => Ok(Fail),
        "complete" => Ok(Complete),
        "break-alzzin-crumble-wall" => Ok(BreakAlzzinCrumbleWall),
        "interrupt-avatar-suppression" => Ok(InterruptAvatarSuppression),
        "send-mandokir-downstairs" => Ok(SendMandokirDownstairs),
        _ => Err(format!("unknown encounter signal: {value}")),
    }
}

fn parse_target(value: &str) -> Result<InstructionTarget, String> {
    match value {
        "opponent" => Ok(InstructionTarget::CurrentOpponent),
        "self" => Ok(InstructionTarget::SelfActor),
        "highest-threat" => Ok(InstructionTarget::HighestThreat),
        "second-threat" => Ok(InstructionTarget::SecondThreat),
        "random-threat" => Ok(InstructionTarget::RandomThreat),
        "random-threat-except-highest" => Ok(InstructionTarget::RandomThreatExceptHighest),
        "invoker" => Ok(InstructionTarget::Invoker),
        "beneficiary" => Ok(InstructionTarget::Beneficiary),
        "ai-sender" => Ok(InstructionTarget::AiSender),
        "spawner" => Ok(InstructionTarget::Spawner),
        "event-subject" => Ok(InstructionTarget::EventSubject),
        "highest-threat-character" => Ok(InstructionTarget::HighestThreatCharacter),
        "random-threat-character" => Ok(InstructionTarget::RandomThreatCharacter),
        "random-threat-character-except-highest" => {
            Ok(InstructionTarget::RandomThreatCharacterExceptHighest)
        }
        "no-explicit-spell-target" => Ok(InstructionTarget::NoExplicitSpellTarget),
        "random-hostile-mana-user" => Ok(InstructionTarget::RandomHostileManaUser),
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

fn percent(min: &str, max: &str) -> Result<PercentageCondition, String> {
    let min_pct = parse_u8(min)?;
    let max_pct = parse_u8(max)?;
    if min_pct > max_pct || max_pct > 100 {
        return Err(format!("invalid percentage window: {min_pct}..={max_pct}"));
    }
    Ok(PercentageCondition { min_pct, max_pct })
}

fn parse_ai_event(value: &str) -> Result<AiEventKind, String> {
    match value {
        "just-died" => Ok(AiEventKind::JustDied),
        "critical-health" => Ok(AiEventKind::CriticalHealth),
        "lost-health" => Ok(AiEventKind::LostHealth),
        "lost-some-health" => Ok(AiEventKind::LostSomeHealth),
        "got-full-health" => Ok(AiEventKind::GotFullHealth),
        "custom-a" => Ok(AiEventKind::CustomA),
        "custom-b" => Ok(AiEventKind::CustomB),
        "crowd-controlled" => Ok(AiEventKind::CrowdControlled),
        "custom-c" => Ok(AiEventKind::CustomC),
        "custom-d" => Ok(AiEventKind::CustomD),
        "custom-e" => Ok(AiEventKind::CustomE),
        "custom-f" => Ok(AiEventKind::CustomF),
        _ => Err(format!("unknown AI event: {value}")),
    }
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

fn parse_threat_percent(value: &str) -> Result<i32, String> {
    let percent = parse_i32(value)?;
    if !(-100..=100).contains(&percent) {
        return Err(format!("threat percent must be -100..=100: {percent}"));
    }
    Ok(percent)
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
    fn native_definition_decodes_named_death_requests() {
        let rules = "900002,aggro,100,4294967295,once,all,ordinary,any-posture,lethal-floor:on+lethal-floor:off+force-death";
        let material = format!("entry:6@{rules}");
        let packed = format!("entry:6@{}@{rules}", definition_revision(&material));
        let definition = parse_definition(&packed).unwrap();

        assert_eq!(
            definition.rules[0].instructions,
            vec![
                CreatureInstruction::SetLethalDamageFloor(SetLethalDamageFloorInstruction {
                    enabled: true
                }),
                CreatureInstruction::SetLethalDamageFloor(SetLethalDamageFloorInstruction {
                    enabled: false
                }),
                CreatureInstruction::ForceDeath,
            ]
        );
    }

    #[test]
    fn revision_covers_the_normalized_definition() {
        let first = "entry:6@17,aggro,100,1,once,all,ordinary,flee";
        let second = "entry:6@17,aggro,100,1,once,all,ordinary,help:8";
        assert_ne!(definition_revision(first), definition_revision(second));
    }

    #[test]
    fn importer_posture_vocabulary_decodes_at_the_module_boundary() {
        for (encoded, expected) in [
            ("any-posture", PostureAdmission::Any),
            ("ranged-only", PostureAdmission::RangedOnly),
            ("melee-only", PostureAdmission::MeleeOnly),
        ] {
            let rule =
                parse_rule(&format!("17,aggro,100,1,once,all,ordinary,{encoded},flee")).unwrap();
            assert_eq!(rule.posture, expected);
        }
    }

    #[test]
    fn percent_threat_vocabulary_decodes_with_signed_bounds() {
        let selected = parse_instruction("threat-selected:-50:opponent").unwrap();
        assert!(matches!(
            selected,
            CreatureInstruction::ScaleSelectedThreat(ScaleSelectedThreatInstruction {
                percent: -50,
                target: InstructionTarget::CurrentOpponent,
            })
        ));

        let all = parse_instruction("threat-all:-100").unwrap();
        assert!(matches!(
            all,
            CreatureInstruction::ScaleAllThreat(ScaleAllThreatInstruction { percent: -100 })
        ));
        assert!(parse_instruction("threat-all:-101").is_err());
    }

    #[test]
    fn presentation_vocabulary_has_no_generic_unit_field_or_flag_form() {
        let faction = parse_instruction("faction:777").unwrap();
        assert!(matches!(
            faction,
            CreatureInstruction::Presentation(CreaturePresentationInstruction::SetFaction {
                faction_template: 777
            })
        ));
        let mount = parse_instruction("creature-mount:twilight-marauder").unwrap();
        assert!(matches!(
            mount,
            CreatureInstruction::Presentation(CreaturePresentationInstruction::SetCreatureMount {
                mount: CreaturePresentationMount::TwilightMarauder
            })
        ));
        assert!(parse_instruction("unit-flags:set:rajaxx-spawn-protection").is_err());
        let quarantine_rule =
            "1534108,aggro,100,1,once,all,ordinary,unit-flags:set:rajaxx-spawn-protection";
        let quarantine_subject = "guid:17379391219402170660";
        let material = format!("{quarantine_subject}@{quarantine_rule}");
        let quarantine = parse_definition(&format!(
            "{quarantine_subject}@{}@{quarantine_rule}",
            definition_revision(&material)
        ))
        .unwrap();
        assert!(matches!(
            quarantine.rules[0].instructions[0],
            CreatureInstruction::Presentation(
                CreaturePresentationInstruction::SetRajaxxSpawnProtection(_)
            )
        ));
        let invalid_rule = quarantine_rule.replacen("1534108", "1534109", 1);
        let material = format!("{quarantine_subject}@{invalid_rule}");
        assert!(parse_definition(&format!(
            "{quarantine_subject}@{}@{invalid_rule}",
            definition_revision(&material)
        ))
        .is_err());
        assert!(parse_instruction("unit-field:147:3").is_err());
        assert!(parse_instruction("unit-flags:set:832").is_err());
    }

    #[test]
    fn pinned_quest_credit_instructions_decode_to_typed_requests() {
        let cases = [
            (
                "quest-event:8353:selected-character",
                CreatureInstruction::QuestCredit(crate::quest::EventAiQuestCredit::QuestEvent(
                    QuestEvent {
                        quest_entry: 8_353,
                        recipient_policy: QuestCreditRecipientPolicy::SelectedCharacter,
                    },
                )),
            ),
            (
                "kill-credit:12299:tap-group",
                CreatureInstruction::QuestCredit(crate::quest::EventAiQuestCredit::KillCredit(
                    KillCredit {
                        creature_entry: 12_299,
                        recipient_policy: QuestCreditRecipientPolicy::TapGroup,
                    },
                )),
            ),
            (
                "quest-event:8354:eligible-group",
                CreatureInstruction::QuestCredit(crate::quest::EventAiQuestCredit::QuestEvent(
                    QuestEvent {
                        quest_entry: 8_354,
                        recipient_policy: QuestCreditRecipientPolicy::EligibleGroup,
                    },
                )),
            ),
        ];

        for (encoded, expected) in cases {
            assert_eq!(parse_instruction(encoded).unwrap(), expected);
        }
        assert!(parse_instruction("quest-event:0:selected-character").is_err());
        assert!(parse_instruction("quest-event:8353:nearest-character").is_err());
        assert!(parse_instruction("cast-credit:12297:456:invoker-beneficiary").is_err());
        assert!(parse_instruction("quest-event:8353:threat-list-characters").is_err());
    }
}
