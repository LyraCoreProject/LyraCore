use std::collections::{BTreeMap, HashSet};

use spacetimedb::{ReducerContext, Table};

use super::{
    ActionResult, BroadcastLine, CreatureAiRuleState, CreatureAiState, CreatureInstruction,
    CreatureState, DefinitionRevision, EngagedFight, EventAiDefinition, EventAiRequest,
    EventAiRule, EventAiUnit, EventCondition, EventContext, ExecutionPolicy, InstructionSelection,
    InstructionTarget, RecurrencePolicy, RuleState, SpeakInstruction, SpeechMode, SummonLocation,
};
use crate::chat::{is_supported_chat_type, CHAT_SAY, CHAT_YELL};
use crate::creatures::ai::TickScope;
use crate::{
    game_creature_ai_broadcast_text, game_creature_ai_rule_state, game_creature_ai_state,
    game_creature_ai_summon, game_creature_template, game_faction_template, game_melee_attack,
    game_pending_cast, game_threat, game_world_entity, WorldEntity,
};

/// How long a rule waits after a Refusal from its opening cast; `hold_opportunity_open` explains
/// the policy.
const CAST_RETRY_MS: u64 = 1_500;

/// The Seam between the EventAI engine and a world: facts read world state, effects change it.
/// Conditions, target selection and action logic live ABOVE this Seam, in `engine`, `combat` and
/// `mobility`, so `DatabaseWorld` and the test Fake run the same decisions.
pub(crate) trait EventAiWorld {
    // Facts.
    fn eventai_now_ms(&self) -> u64;
    /// The live melee fights whose attacker runs its entry's EventAI, within `scope`.
    fn eventai_fights(&self, scope: &TickScope) -> Vec<EngagedFight>;
    fn eventai_definition(&self, creature_guid: u64) -> EventAiDefinition;
    fn eventai_creature_state(&self, creature_guid: u64) -> CreatureState;
    fn eventai_rule_state(&self, creature_guid: u64, rule_id: u64) -> Option<RuleState>;
    fn eventai_unit(&self, guid: u64) -> Option<EventAiUnit>;
    /// Candidates around `center` in its partition, coarsely: the shared logic re-checks the
    /// exact distance.
    fn eventai_units_near(&self, center: &EventAiUnit, radius_yd: f32) -> Vec<EventAiUnit>;
    /// The raw threat rows one creature holds, as `(source guid, threat)`, unordered.
    fn eventai_threat(&self, creature_guid: u64) -> Vec<(u64, i64)>;
    fn eventai_has_aura(&self, guid: u64, spell_id: u32) -> bool;
    fn eventai_is_casting(&self, guid: u64) -> bool;
    fn eventai_factions_friendly(&self, first: u32, second: u32) -> bool;
    fn eventai_is_engaged(&self, guid: u64) -> bool;
    /// The rout clock on this creature's own melee row; `None` without a fight to break off from.
    fn eventai_rout_ends_ms(&self, creature_guid: u64) -> Option<u32>;
    fn eventai_broadcast(&self, id: u32) -> Option<BroadcastLine>;
    fn eventai_summon_location(&self, id: u32) -> Option<SummonLocation>;
    fn eventai_summon_template_exists(&self, entry: u32) -> bool;
    fn eventai_roll(&self) -> u32;

    // Effects.
    fn set_eventai_phase(&mut self, creature_guid: u64, phase: u8);
    /// Adopt one normalized definition and clear only reversible state owned by the old revision.
    fn adopt_eventai_revision(
        &mut self,
        creature_guid: u64,
        revision: DefinitionRevision,
    ) -> CreatureState;
    fn put_eventai_rule_state(&mut self, creature_guid: u64, rule_id: u64, state: RuleState);
    fn delete_eventai_rule_state(&mut self, creature_guid: u64, rule_id: u64);
    /// Remove state for missing rules on this evaluated creature. Lifecycle edges clean state for
    /// creatures that no longer produce evaluation contexts.
    fn reap_eventai_rule_state(&mut self, creature_guid: u64, valid_rule_ids: &HashSet<u64>);
    /// Deliver one line through the say/yell chokepoint; `true` when it was spoken.
    fn eventai_deliver_line(
        &mut self,
        speaker_guid: u64,
        chat_type: u8,
        language: u8,
        message: String,
    ) -> bool;
    fn eventai_deliver_emote(&mut self, source_guid: u64, emote_id: u32, target_guid: u64) -> bool;
    /// Start the cast; `false` is the spell tier's Refusal (cooldown, cost, range).
    fn eventai_begin_cast(&mut self, caster: &EventAiUnit, spell_id: u32, target_guid: u64)
        -> bool;
    fn eventai_interrupt_cast(&mut self, caster_guid: u64);
    fn stamp_eventai_rout(&mut self, creature_guid: u64, ends_ms: u32);
    fn set_eventai_ranged_posture(&mut self, creature_guid: u64, distance_yd: f32, angle_rad: f32);
    /// The idle friend joins the fight against `victim_guid` as an assist.
    fn eventai_engage_assist(&mut self, creature_guid: u64, victim_guid: u64);
    /// Reserve the next summon sequence number and its lifetime bookkeeping.
    fn eventai_claim_summon_sequence(&mut self, lifetime_ms: u32) -> u64;
    /// Give back a claimed sequence whose summon was refused.
    fn eventai_release_summon_sequence(&mut self, sequence: u64);
    fn eventai_place_summon(
        &mut self,
        sequence: u64,
        guid: u64,
        entry: u32,
        location: &SummonLocation,
        summoner: &EventAiUnit,
    );
    /// The fresh summon joins the fight against `target_guid`.
    fn eventai_engage_summon(&mut self, summon_guid: u64, target_guid: u64);
}

pub(crate) fn evaluate<W: EventAiWorld>(world: &mut W, request: EventAiRequest<'_>) -> u64 {
    let contexts = match &request {
        EventAiRequest::Edge(context) => vec![*context],
        EventAiRequest::Engaged(scope) => super::combat::engaged_contexts(world, scope),
    };
    let mut visited = 0;
    let mut contexts_by_creature: BTreeMap<u64, Vec<EventContext>> = BTreeMap::new();
    for context in contexts {
        contexts_by_creature
            .entry(context.creature_guid)
            .or_default()
            .push(context);
    }
    for (creature_guid, contexts) in contexts_by_creature {
        let definition = world.eventai_definition(creature_guid);
        visited += definition
            .rules
            .iter()
            .map(|rule| rule.instructions.len() as u64)
            .sum::<u64>();
        let creature_state = world.eventai_creature_state(creature_guid);
        let legacy_state_without_definition = definition.rules.is_empty()
            && definition.revision == DefinitionRevision::default()
            && (creature_state.phase != 0
                || creature_state.ranged_posture_active
                || creature_state.ranged_distance != 0.0
                || creature_state.ranged_angle != 0.0);
        if creature_state.definition_revision != definition.revision
            || legacy_state_without_definition
        {
            world.adopt_eventai_revision(creature_guid, definition.revision);
        }
        let valid_rule_ids = definition
            .rules
            .iter()
            .map(|rule| rule.source_rule_id)
            .collect();
        world.reap_eventai_rule_state(creature_guid, &valid_rule_ids);
        for rule in &definition.rules {
            let Some(context) = contexts
                .iter()
                .find(|context| context.kind == rule.event.kind())
            else {
                continue;
            };
            let creature_state = world.eventai_creature_state(creature_guid);
            evaluate_rule(world, context, rule, creature_state);
        }
    }
    visited
}

fn evaluate_rule<W: EventAiWorld>(
    world: &mut W,
    context: &EventContext,
    rule: &EventAiRule,
    creature_state: CreatureState,
) {
    if rule.event.kind() != context.kind {
        return;
    }
    if creature_state.phase >= 32 || rule.allowed_phases.bits & (1u32 << creature_state.phase) == 0
    {
        return;
    }
    let state = world
        .eventai_rule_state(context.creature_guid, rule.source_rule_id)
        .filter(|state| {
            state.lifecycle_id == creature_state.lifecycle_id
                && state.engagement_id == creature_state.engagement_id
        });
    if state.is_none() {
        world.delete_eventai_rule_state(context.creature_guid, rule.source_rule_id);
    }
    if state.is_some_and(|state| state.consumed) {
        return;
    }

    if state.is_some_and(|state| context.now_ms < state.next_eligible_ms) {
        return;
    }

    if let (EventCondition::TimedInCombat(window), None) = (rule.event, state) {
        let delay = roll_window(world, window.min_ms, window.max_ms);
        world.put_eventai_rule_state(
            context.creature_guid,
            rule.source_rule_id,
            RuleState {
                next_eligible_ms: context.now_ms.saturating_add(delay),
                consumed: false,
                lifecycle_id: creature_state.lifecycle_id,
                engagement_id: creature_state.engagement_id,
            },
        );
        return;
    }

    let Some(context) = super::combat::condition(world, context, rule) else {
        return;
    };

    // A missed chance roll costs the opportunity, never the rule. The source contract re-arms a recurring
    // event's repeat window before it rolls and returns without disabling the event, so a repeat
    // rule waits one window and a once-only rule stays armed to roll again next opportunity.
    if rule.chance_pct < 100 && world.eventai_roll() % 100 >= rule.chance_pct as u32 {
        if let Some(state) = repeat_state(world, &context, rule, creature_state) {
            world.put_eventai_rule_state(context.creature_guid, rule.source_rule_id, state);
        }
        return;
    }

    let instructions: Vec<&CreatureInstruction> = match (rule.selection, rule.instructions.len()) {
        (InstructionSelection::RandomOne, 0) => Vec::new(),
        (InstructionSelection::RandomOne, len) => rule
            .instructions
            .get(world.eventai_roll() as usize % len)
            .into_iter()
            .collect(),
        (InstructionSelection::All, _) => rule.instructions.iter().collect(),
    };
    for (index, instruction) in instructions.into_iter().enumerate() {
        if context.assisted && matches!(instruction, CreatureInstruction::Speak(_)) {
            continue;
        }
        let result = execute_instruction(world, &context, instruction);
        match result {
            ActionResult::Applied => {}
            // A Refusal from a LATER action does not rewind the actions already applied, so it
            // spends the opportunity like any other outcome.
            ActionResult::Refused
                if index == 0 && matches!(instruction, CreatureInstruction::Cast(_)) =>
            {
                hold_opportunity_open(world, &context, rule, creature_state);
                return;
            }
            ActionResult::Refused => {}
            ActionResult::Unsupported => return,
        }
    }
    finish_opportunity(world, &context, rule, creature_state);
}

fn execute_instruction<W: EventAiWorld>(
    world: &mut W,
    context: &EventContext,
    instruction: &CreatureInstruction,
) -> ActionResult {
    match instruction {
        CreatureInstruction::Speak(speech) => {
            speak(world, context, speech);
            ActionResult::Applied
        }
        CreatureInstruction::Cast(cast) => {
            let Some(target) = basic_target(world, context, cast.target, Some(cast)) else {
                return ActionResult::Refused;
            };
            if cast.spell_id == 0 {
                ActionResult::Refused
            } else {
                super::combat::cast(world, context, cast, target)
            }
        }
        CreatureInstruction::SetPhase(set_phase) => {
            if set_phase.phase >= 32 {
                return ActionResult::Refused;
            }
            world.set_eventai_phase(context.creature_guid, set_phase.phase);
            ActionResult::Applied
        }
        CreatureInstruction::Emote(_)
        | CreatureInstruction::FleeForAssist
        | CreatureInstruction::CallForHelp(_) => {
            super::combat::execute(world, context, instruction)
        }
        CreatureInstruction::Summon(_) | CreatureInstruction::SetRangedPosture(_) => {
            super::mobility::execute(world, context, instruction)
        }
    }
}

/// Resolve one authored Say or Yell into a line and deliver it. A broadcast text carries its own
/// chat type; the authored action decides when that is not one this tier relays (a monster emote
/// line still reaches players as its say/yell). The broadcast text's emote belongs to the line,
/// so a Refusal at delivery silences both.
fn speak<W: EventAiWorld>(world: &mut W, context: &EventContext, speech: &SpeakInstruction) {
    let chat_type = match speech.mode {
        SpeechMode::Say => CHAT_SAY,
        SpeechMode::Yell => CHAT_YELL,
    };
    let ids = &speech.broadcast_ids;
    let picked = match ids.len() {
        0 => None,
        1 => Some(ids[0]),
        len => Some(ids[world.eventai_roll() as usize % len]),
    };
    let (message, chat_type, language, emote) = match picked {
        None => (speech.legacy_text.clone(), chat_type, 0, 0),
        Some(id) => {
            let Some(line) = world.eventai_broadcast(id) else {
                return;
            };
            (
                line.text,
                if is_supported_chat_type(line.chat_type) {
                    line.chat_type
                } else {
                    chat_type
                },
                line.language,
                line.emote,
            )
        }
    };
    let spoken = world.eventai_deliver_line(context.creature_guid, chat_type, language, message);
    if spoken && emote != 0 {
        world.eventai_deliver_emote(context.creature_guid, emote, 0);
    }
}

fn basic_target<W: EventAiWorld>(
    world: &W,
    context: &EventContext,
    target: InstructionTarget,
    cast: Option<&super::CastInstruction>,
) -> Option<u64> {
    match target {
        InstructionTarget::CurrentOpponent => context.current_target_guid,
        InstructionTarget::SelfActor => Some(context.creature_guid),
        InstructionTarget::Invoker => context.invoker_guid,
        InstructionTarget::EventSubject => context.event_target_guid,
        _ => super::combat::target(world, context, target, cast),
    }
}

fn finish_opportunity<W: EventAiWorld>(
    world: &mut W,
    context: &EventContext,
    rule: &EventAiRule,
    creature_state: CreatureState,
) {
    let state = repeat_state(world, context, rule, creature_state).unwrap_or(RuleState {
        next_eligible_ms: 0,
        consumed: true,
        lifecycle_id: creature_state.lifecycle_id,
        engagement_id: creature_state.engagement_id,
    });
    world.put_eventai_rule_state(context.creature_guid, rule.source_rule_id, state);
}

/// A Refusal from a rule's OPENING cast is transient (a cast already in flight, the target out of
/// range or gone), so the rule keeps its opportunity instead of spending a whole repeat window on
/// it. A rule the source marks as the creature's combat action comes back on the very next firing,
/// the way the source retries one; every other rule waits `CAST_RETRY_MS`, which is the retry the
/// timer evaluator this engine replaced used for the same Refusal. An edge rule keeps its arming
/// untouched either way: no window stamped on an edge would ever be reached again.
fn hold_opportunity_open<W: EventAiWorld>(
    world: &mut W,
    context: &EventContext,
    rule: &EventAiRule,
    creature_state: CreatureState,
) {
    if rule.execution == ExecutionPolicy::CombatAction || !rule.event.kind().recurs() {
        return;
    }
    world.put_eventai_rule_state(
        context.creature_guid,
        rule.source_rule_id,
        RuleState {
            next_eligible_ms: context.now_ms.saturating_add(CAST_RETRY_MS),
            consumed: false,
            lifecycle_id: creature_state.lifecycle_id,
            engagement_id: creature_state.engagement_id,
        },
    );
}

/// The rule state a repeat rule takes after an opportunity: its window rolled from now. `None`
/// when the rule has no next opportunity to wait for, which spends it instead.
fn repeat_state<W: EventAiWorld>(
    world: &W,
    context: &EventContext,
    rule: &EventAiRule,
    creature_state: CreatureState,
) -> Option<RuleState> {
    match rule.recurrence {
        RecurrencePolicy::Repeat(window) if rule.event.kind().recurs() => Some(RuleState {
            next_eligible_ms: context.now_ms.saturating_add(roll_window(
                world,
                window.min_ms,
                window.max_ms,
            )),
            consumed: false,
            lifecycle_id: creature_state.lifecycle_id,
            engagement_id: creature_state.engagement_id,
        }),
        RecurrencePolicy::Once | RecurrencePolicy::Repeat(_) => None,
    }
}

/// Random milliseconds in `[min, max]`. A fixed window costs no roll. The loader refuses an
/// inverted window, and this guard keeps an invalid test definition from wrapping the span.
fn roll_window<W: EventAiWorld>(world: &W, min: u32, max: u32) -> u64 {
    if max <= min {
        return min as u64;
    }
    min as u64 + (world.eventai_roll() as u64 % (u64::from(max) - u64::from(min) + 1))
}

pub(crate) struct DatabaseWorld<'a> {
    ctx: &'a ReducerContext,
}

impl<'a> DatabaseWorld<'a> {
    pub(crate) fn new(ctx: &'a ReducerContext) -> Self {
        Self { ctx }
    }

    fn now_ms(&self) -> u64 {
        (self.ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64
    }

    fn state_row(&self, creature_guid: u64) -> Option<CreatureAiState> {
        self.ctx
            .db
            .game_creature_ai_state()
            .creature_guid()
            .find(creature_guid)
    }
}

fn unit_of(entity: &WorldEntity) -> EventAiUnit {
    EventAiUnit {
        guid: entity.guid,
        x: entity.x,
        y: entity.y,
        z: entity.z,
        map_id: entity.map_id,
        instance_id: entity.instance_id,
        health: entity.health,
        max_health: entity.max_health,
        level: entity.level,
        faction_template: entity.faction_template,
        dead: entity.dead,
        is_player: entity.is_player(),
    }
}

impl EventAiWorld for DatabaseWorld<'_> {
    fn eventai_now_ms(&self) -> u64 {
        self.now_ms()
    }

    fn eventai_fights(&self, scope: &TickScope) -> Vec<EngagedFight> {
        let entities = self.ctx.db.game_world_entity();
        self.ctx
            .db
            .game_melee_attack()
            .iter()
            .filter_map(|fight| {
                let creature = entities.guid().find(fight.attacker_guid)?;
                (super::runs_eventai(&creature)
                    && !creature.dead
                    && scope.covers(creature.instance_id))
                .then_some(EngagedFight {
                    creature_guid: creature.guid,
                    victim_guid: fight.target_guid,
                })
            })
            .collect()
    }

    fn eventai_definition(&self, creature_guid: u64) -> EventAiDefinition {
        super::combat::definition_for(self.ctx, creature_guid)
    }

    fn eventai_creature_state(&self, creature_guid: u64) -> CreatureState {
        self.state_row(creature_guid)
            .map_or_else(CreatureState::default, CreatureState::from)
    }

    fn eventai_rule_state(&self, creature_guid: u64, rule_id: u64) -> Option<RuleState> {
        self.ctx
            .db
            .game_creature_ai_rule_state()
            .by_creature()
            .filter(&creature_guid)
            .find(|row| row.source_rule_id == rule_id)
            .map(|row| RuleState {
                next_eligible_ms: row.next_eligible_ms,
                consumed: row.consumed,
                lifecycle_id: row.lifecycle_id,
                engagement_id: row.engagement_id,
            })
    }

    fn eventai_unit(&self, guid: u64) -> Option<EventAiUnit> {
        self.ctx
            .db
            .game_world_entity()
            .guid()
            .find(guid)
            .map(|entity| unit_of(&entity))
    }

    fn eventai_units_near(&self, center: &EventAiUnit, radius_yd: f32) -> Vec<EventAiUnit> {
        crate::helpers::entities_near(
            self.ctx,
            center.map_id,
            center.instance_id,
            center.x,
            center.y,
            radius_yd,
        )
        .iter()
        .map(unit_of)
        .collect()
    }

    fn eventai_threat(&self, creature_guid: u64) -> Vec<(u64, i64)> {
        self.ctx
            .db
            .game_threat()
            .by_creature()
            .filter(&creature_guid)
            .map(|entry| (entry.source_guid, entry.threat))
            .collect()
    }

    fn eventai_has_aura(&self, guid: u64, spell_id: u32) -> bool {
        crate::spell::has_aura(self.ctx, guid, spell_id)
    }

    fn eventai_is_casting(&self, guid: u64) -> bool {
        self.ctx
            .db
            .game_pending_cast()
            .by_caster()
            .filter(&guid)
            .next()
            .is_some()
    }

    fn eventai_factions_friendly(&self, first: u32, second: u32) -> bool {
        crate::faction::is_friendly(self.ctx, first, second)
            || (self.ctx.db.game_faction_template().count() == 0 && first == second)
    }

    fn eventai_is_engaged(&self, guid: u64) -> bool {
        crate::combat::is_engaged(self.ctx, guid)
    }

    fn eventai_rout_ends_ms(&self, creature_guid: u64) -> Option<u32> {
        self.ctx
            .db
            .game_melee_attack()
            .attacker_guid()
            .find(creature_guid)
            .map(|fight| fight.rout_ends_ms)
    }

    fn eventai_broadcast(&self, id: u32) -> Option<BroadcastLine> {
        self.ctx
            .db
            .game_creature_ai_broadcast_text()
            .id()
            .find(id)
            .map(|text| BroadcastLine {
                text: text.male_text,
                chat_type: text.chat_type,
                language: text.language_id,
                emote: text.emote_id_1,
            })
    }

    fn eventai_summon_location(&self, id: u32) -> Option<SummonLocation> {
        self.ctx
            .db
            .game_creature_ai_summon()
            .id()
            .find(id)
            .map(|row| SummonLocation {
                x: row.x,
                y: row.y,
                z: row.z,
                orientation: row.orientation,
                lifetime_ms: row.lifetime_ms,
            })
    }

    fn eventai_summon_template_exists(&self, entry: u32) -> bool {
        self.ctx
            .db
            .game_creature_template()
            .entry()
            .find(entry)
            .is_some()
    }

    fn eventai_roll(&self) -> u32 {
        self.ctx.random()
    }

    fn set_eventai_phase(&mut self, creature_guid: u64, phase: u8) {
        let table = self.ctx.db.game_creature_ai_state();
        match self.state_row(creature_guid) {
            Some(mut row) => {
                row.phase = phase;
                table.creature_guid().update(row);
            }
            None => {
                table.insert(CreatureAiState {
                    creature_guid,
                    phase,
                    lifecycle_id: 1,
                    engagement_id: 1,
                    ranged_distance: 0.0,
                    ranged_angle: 0.0,
                    ranged_posture_active: false,
                    definition_revision: 0,
                });
            }
        }
    }

    fn adopt_eventai_revision(
        &mut self,
        creature_guid: u64,
        revision: DefinitionRevision,
    ) -> CreatureState {
        let rule_state = self.ctx.db.game_creature_ai_rule_state();
        for row in rule_state
            .by_creature()
            .filter(&creature_guid)
            .collect::<Vec<_>>()
        {
            rule_state.id().delete(row.id);
        }

        let table = self.ctx.db.game_creature_ai_state();
        let row = match self.state_row(creature_guid) {
            Some(mut row) => {
                row.phase = 0;
                row.ranged_distance = 0.0;
                row.ranged_angle = 0.0;
                row.ranged_posture_active = false;
                row.definition_revision = revision.value;
                table.creature_guid().update(row)
            }
            None => table.insert(CreatureAiState {
                creature_guid,
                phase: 0,
                lifecycle_id: 1,
                engagement_id: 1,
                ranged_distance: 0.0,
                ranged_angle: 0.0,
                ranged_posture_active: false,
                definition_revision: revision.value,
            }),
        };
        CreatureState::from(row)
    }

    fn put_eventai_rule_state(&mut self, creature_guid: u64, rule_id: u64, state: RuleState) {
        let table = self.ctx.db.game_creature_ai_rule_state();
        if let Some(mut row) = table
            .by_creature()
            .filter(&creature_guid)
            .find(|row| row.source_rule_id == rule_id)
        {
            row.next_eligible_ms = state.next_eligible_ms;
            row.consumed = state.consumed;
            row.lifecycle_id = state.lifecycle_id;
            row.engagement_id = state.engagement_id;
            table.id().update(row);
        } else {
            table.insert(CreatureAiRuleState {
                id: 0,
                creature_guid,
                source_rule_id: rule_id,
                next_eligible_ms: state.next_eligible_ms,
                consumed: state.consumed,
                lifecycle_id: state.lifecycle_id,
                engagement_id: state.engagement_id,
            });
        }
    }

    fn delete_eventai_rule_state(&mut self, creature_guid: u64, rule_id: u64) {
        let table = self.ctx.db.game_creature_ai_rule_state();
        for row in table
            .by_creature()
            .filter(&creature_guid)
            .filter(|row| row.source_rule_id == rule_id)
            .collect::<Vec<_>>()
        {
            table.id().delete(row.id);
        }
    }

    fn reap_eventai_rule_state(&mut self, creature_guid: u64, valid_rule_ids: &HashSet<u64>) {
        let state = self.ctx.db.game_creature_ai_rule_state();
        for row in state
            .by_creature()
            .filter(&creature_guid)
            .collect::<Vec<_>>()
        {
            if !valid_rule_ids.contains(&row.source_rule_id) {
                state.id().delete(row.id);
            }
        }
    }

    fn eventai_deliver_line(
        &mut self,
        speaker_guid: u64,
        chat_type: u8,
        language: u8,
        message: String,
    ) -> bool {
        let Some(creature) = self.ctx.db.game_world_entity().guid().find(speaker_guid) else {
            return false;
        };
        // The say/yell chokepoint owns the dead-speaker Gate and the length cap; a creature line
        // goes through it like a player's rather than writing the event row itself.
        crate::chat::apply_send_chat(self.ctx, creature, chat_type, language, message).is_ok()
    }

    fn eventai_deliver_emote(&mut self, source_guid: u64, emote_id: u32, target_guid: u64) -> bool {
        let Some(source) = self.ctx.db.game_world_entity().guid().find(source_guid) else {
            return false;
        };
        crate::chat::apply_send_emote(self.ctx, source, 0, emote_id, target_guid).is_ok()
    }

    fn eventai_begin_cast(
        &mut self,
        caster: &EventAiUnit,
        spell_id: u32,
        target_guid: u64,
    ) -> bool {
        crate::spell::begin_cast(
            self.ctx,
            caster.guid,
            spell_id,
            caster.level as u8,
            target_guid,
            false,
            None,
        )
        .is_ok()
    }

    fn eventai_interrupt_cast(&mut self, caster_guid: u64) {
        crate::spell::interrupt_cast(self.ctx, caster_guid);
    }

    fn stamp_eventai_rout(&mut self, creature_guid: u64, ends_ms: u32) {
        let melee = self.ctx.db.game_melee_attack();
        if let Some(mut fight) = melee.attacker_guid().find(creature_guid) {
            fight.rout_ends_ms = ends_ms;
            melee.attacker_guid().update(fight);
        }
    }

    fn set_eventai_ranged_posture(&mut self, creature_guid: u64, distance_yd: f32, angle_rad: f32) {
        let table = self.ctx.db.game_creature_ai_state();
        match self.state_row(creature_guid) {
            Some(mut row) => {
                row.ranged_distance = distance_yd;
                row.ranged_angle = angle_rad;
                row.ranged_posture_active = true;
                table.creature_guid().update(row);
            }
            None => {
                table.insert(CreatureAiState {
                    creature_guid,
                    phase: 0,
                    lifecycle_id: 1,
                    engagement_id: 1,
                    ranged_distance: distance_yd,
                    ranged_angle: angle_rad,
                    ranged_posture_active: true,
                    definition_revision: 0,
                });
            }
        }
    }

    fn eventai_engage_assist(&mut self, creature_guid: u64, victim_guid: u64) {
        if crate::combat::apply_start_attack(self.ctx, creature_guid, victim_guid).is_ok() {
            crate::hooks::fire_on_aggro(
                self.ctx,
                &crate::hooks::AggroPayload {
                    creature_guid,
                    target_guid: victim_guid,
                    assist: true,
                },
            );
        }
    }

    fn eventai_claim_summon_sequence(&mut self, lifetime_ms: u32) -> u64 {
        super::mobility::claim_summon_sequence(self.ctx, lifetime_ms)
    }

    fn eventai_release_summon_sequence(&mut self, sequence: u64) {
        super::mobility::release_summon_sequence(self.ctx, sequence)
    }

    fn eventai_place_summon(
        &mut self,
        sequence: u64,
        guid: u64,
        entry: u32,
        location: &SummonLocation,
        summoner: &EventAiUnit,
    ) {
        super::mobility::place_summon(self.ctx, sequence, guid, entry, location, summoner)
    }

    fn eventai_engage_summon(&mut self, summon_guid: u64, target_guid: u64) {
        super::mobility::engage_summon(self.ctx, summon_guid, target_guid)
    }
}
