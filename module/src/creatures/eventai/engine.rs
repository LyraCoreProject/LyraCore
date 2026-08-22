use std::collections::{BTreeMap, HashSet};

use spacetimedb::{ReducerContext, Table};

use super::mobility::game_creature_ai_summon_origin;
use super::{
    ActionResult, BroadcastLine, CreatureAiRuleState, CreatureAiState, CreatureInstruction,
    CreatureState, CycleActor, DefinitionRevision, EngagedFight, EventAiDefinition, EventAiRequest,
    EventAiRule, EventAiUnit, EventCondition, EventContext, EventPredicate, ExecutionPolicy,
    InstructionSelection, RecurrencePolicy, RuleState, SpeakInstruction, SpeechMode,
    SummonLocation,
};
use crate::chat::{is_supported_chat_type, CHAT_SAY, CHAT_YELL};
use crate::creatures::ai::TickScope;
use crate::spell::{game_aura, game_spell};
use crate::{
    game_creature_ai_broadcast_text, game_creature_ai_rule_state, game_creature_ai_spell_metadata,
    game_creature_ai_state, game_creature_ai_summon, game_creature_template, game_faction_template,
    game_melee_attack, game_threat, game_world_entity, WorldEntity,
};

/// The Seam between the EventAI engine and a world: facts read world state, effects change it.
/// Conditions, target selection and action logic live ABOVE this Seam, in `engine`, `combat` and
/// `mobility`, so `DatabaseWorld` and the test Fake run the same decisions.
pub(crate) trait EventAiWorld {
    // Facts.
    fn eventai_now_ms(&self) -> u64;
    /// The live melee fights whose attacker runs its entry's EventAI, within `scope`.
    fn eventai_fights(&self, scope: &TickScope) -> Vec<EngagedFight>;
    fn eventai_cycle_actors(&self, scope: &TickScope, active: &HashSet<u64>) -> Vec<CycleActor>;
    fn eventai_definition(&self, creature_guid: u64) -> EventAiDefinition;
    fn eventai_creature_state(&self, creature_guid: u64) -> CreatureState;
    fn eventai_rule_state(&self, creature_guid: u64, rule_id: u64) -> Option<RuleState>;
    fn eventai_unit(&self, guid: u64) -> Option<EventAiUnit>;
    fn eventai_spawner_guid(&self, creature_guid: u64) -> Option<u64>;
    /// Candidates around `center` in its partition, coarsely: the shared logic re-checks the
    /// exact distance.
    fn eventai_units_near(&self, center: &EventAiUnit, radius_yd: f32) -> Vec<EventAiUnit>;
    /// The raw threat rows one creature holds, as `(source guid, threat)`, unordered.
    fn eventai_threat(&self, creature_guid: u64) -> Vec<(u64, i64)>;
    fn eventai_has_aura(&self, guid: u64, spell_id: u32) -> bool;
    fn eventai_aura_stacks(&self, guid: u64, spell_id: u32) -> u32;
    fn eventai_is_crowd_controlled(&self, guid: u64, dispel_type: u32) -> bool;
    fn eventai_is_casting(&self, guid: u64) -> bool;
    fn eventai_factions_friendly(&self, first: u32, second: u32) -> bool;
    fn eventai_factions_hostile(&self, first: u32, second: u32) -> bool;
    fn eventai_line_of_sight(&self, first: &EventAiUnit, second: &EventAiUnit) -> bool;
    fn eventai_spell_range(&self, spell_id: u32) -> Option<u32>;
    fn eventai_spell_excludes_caster(&self, spell_id: u32) -> bool;
    fn eventai_is_engaged(&self, guid: u64) -> bool;
    fn eventai_matches_predicate(&self, guid: u64, predicate: EventPredicate) -> bool;
    fn eventai_in_zone_or_area(&self, unit: &EventAiUnit, zone_or_area_id: u32) -> bool;
    fn eventai_combat_action_ready(&self, guid: u64) -> bool;
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
    /// Start the typed cast; `false` is the spell tier's Refusal.
    fn eventai_start_spell(
        &mut self,
        caster: &EventAiUnit,
        spell_id: u32,
        mode: super::SpellStartMode,
        target: super::SpellCastTarget,
        interrupt_previous: bool,
        admission: super::SpellCasterAdmission,
    ) -> bool;
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
        EventAiRequest::Cycle { scope, active } => {
            super::combat::cycle_contexts(world, scope, active)
        }
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
    let mut state = world
        .eventai_rule_state(context.creature_guid, rule.source_rule_id)
        .filter(|state| {
            state.lifecycle_id == creature_state.lifecycle_id
                && state.engagement_id == creature_state.engagement_id
        });
    if state.is_none() {
        world.delete_eventai_rule_state(context.creature_guid, rule.source_rule_id);
    }
    let mut initialized = false;
    if let (Some(window), None) = (initial_window(rule, context.engaged), state) {
        let delay = roll_window(world, window.min_ms, window.max_ms);
        let initial = RuleState {
            next_eligible_ms: context.now_ms.saturating_add(delay),
            consumed: false,
            lifecycle_id: creature_state.lifecycle_id,
            engagement_id: creature_state.engagement_id,
            invocation_seed: 0,
            invocation_started: false,
            executing: false,
            invocation_branch: 0,
            paused_at_ms: 0,
        };
        world.put_eventai_rule_state(context.creature_guid, rule.source_rule_id, initial);
        state = Some(initial);
        initialized = true;
    }
    let phase_allowed =
        creature_state.phase < 32 && rule.allowed_phases.bits & (1u32 << creature_state.phase) != 0;
    if !phase_allowed {
        if let Some(mut paused) = state.filter(|state| !state.consumed && state.paused_at_ms == 0) {
            paused.paused_at_ms = context.now_ms.saturating_add(1);
            world.put_eventai_rule_state(context.creature_guid, rule.source_rule_id, paused);
        }
        return;
    }
    if let Some(mut resumed) = state.filter(|state| state.paused_at_ms != 0) {
        let pause_started_ms = resumed.paused_at_ms - 1;
        resumed.next_eligible_ms = resumed
            .next_eligible_ms
            .saturating_add(context.now_ms.saturating_sub(pause_started_ms));
        resumed.paused_at_ms = 0;
        world.put_eventai_rule_state(context.creature_guid, rule.source_rule_id, resumed);
        state = Some(resumed);
    }
    if state.is_some_and(|state| state.consumed || state.executing) {
        return;
    }

    if state.is_some_and(|state| context.now_ms < state.next_eligible_ms) {
        return;
    }

    if !super::combat::posture_matches(world, context, rule.posture) {
        return;
    }
    if rule.execution == ExecutionPolicy::CombatAction
        && !world.eventai_combat_action_ready(context.creature_guid)
    {
        return;
    }

    if initialized {
        return;
    }

    if super::combat::condition(world, context, rule, 0).is_none() {
        return;
    }
    let (seed, branch) = state.filter(|state| state.invocation_started).map_or_else(
        || fresh_invocation(world, context, rule),
        |state| (state.invocation_seed, state.invocation_branch),
    );
    let linked_choice = invocation_choice(seed, branch, 1);
    let context = super::combat::condition(world, context, rule, linked_choice)
        .expect("a linked choice cannot remove the last eligible condition candidate");

    let open_state = RuleState {
        next_eligible_ms: state.map_or(0, |state| state.next_eligible_ms),
        consumed: false,
        lifecycle_id: creature_state.lifecycle_id,
        engagement_id: creature_state.engagement_id,
        invocation_seed: seed,
        invocation_started: true,
        executing: true,
        invocation_branch: branch,
        paused_at_ms: 0,
    };
    world.put_eventai_rule_state(context.creature_guid, rule.source_rule_id, open_state);

    // Chance is one saved decision per authored opportunity. A miss spends both once-only and
    // repeating opportunities; a repeating rule then waits its next window.
    if rule.chance_pct < 100
        && invocation_choice(seed, branch, 0) % 100 >= u64::from(rule.chance_pct)
    {
        finish_opportunity(world, &context, rule, creature_state);
        return;
    }

    let instructions: Vec<&CreatureInstruction> = match (rule.selection, rule.instructions.len()) {
        (InstructionSelection::RandomOne, 0) => Vec::new(),
        (InstructionSelection::RandomOne, len) => rule
            .instructions
            .get(invocation_choice(seed, branch, 2) as usize % len)
            .into_iter()
            .collect(),
        (InstructionSelection::All, _) => rule.instructions.iter().collect(),
    };
    for (index, instruction) in instructions.into_iter().enumerate() {
        if context.assisted && matches!(instruction, CreatureInstruction::Speak(_)) {
            continue;
        }
        let result = execute_instruction(world, &context, instruction, linked_choice);
        match result {
            ActionResult::Applied => {}
            // A Refusal from a LATER action does not rewind the actions already applied, so it
            // spends the opportunity like any other outcome.
            ActionResult::Refused
                if index == 0 && rule.execution == ExecutionPolicy::CombatAction =>
            {
                hold_opportunity_open(world, &context, rule, open_state);
                return;
            }
            ActionResult::Refused => {}
            ActionResult::Unsupported => {
                finish_opportunity(world, &context, rule, creature_state);
                return;
            }
        }
    }
    finish_opportunity(world, &context, rule, creature_state);
}

fn execute_instruction<W: EventAiWorld>(
    world: &mut W,
    context: &EventContext,
    instruction: &CreatureInstruction,
    linked_choice: u64,
) -> ActionResult {
    match instruction {
        CreatureInstruction::Speak(speech) => {
            if super::combat::unit_target(world, context, speech.target, None, linked_choice)
                .is_none()
            {
                return ActionResult::Refused;
            }
            if speak(world, context, speech, linked_choice) {
                ActionResult::Applied
            } else {
                ActionResult::Refused
            }
        }
        CreatureInstruction::Cast(cast) => {
            let Some(target) =
                super::combat::target(world, context, cast.target, Some(cast), linked_choice)
            else {
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
        CreatureInstruction::IncrementPhase(increment) => {
            let phase = i32::from(world.eventai_creature_state(context.creature_guid).phase)
                .saturating_add(increment.amount)
                .clamp(0, 31) as u8;
            world.set_eventai_phase(context.creature_guid, phase);
            ActionResult::Applied
        }
        CreatureInstruction::RandomPhase(random) => {
            let Some(phase) = random
                .phases
                .get(linked_choice as usize % random.phases.len())
                .copied()
            else {
                return ActionResult::Refused;
            };
            world.set_eventai_phase(context.creature_guid, phase);
            ActionResult::Applied
        }
        CreatureInstruction::RandomPhaseRange(range) => {
            if range.min_phase >= range.max_phase || range.max_phase >= 32 {
                return ActionResult::Refused;
            }
            let span = u32::from(range.max_phase - range.min_phase) + 1;
            let phase = range.min_phase + (linked_choice % u64::from(span)) as u8;
            world.set_eventai_phase(context.creature_guid, phase);
            ActionResult::Applied
        }
        CreatureInstruction::Emote(_)
        | CreatureInstruction::FleeForAssist
        | CreatureInstruction::CallForHelp(_) => {
            super::combat::execute(world, context, instruction, linked_choice)
        }
        CreatureInstruction::Summon(_) | CreatureInstruction::SetRangedPosture(_) => {
            super::mobility::execute(world, context, instruction, linked_choice)
        }
    }
}

/// Resolve one authored Say or Yell into a line and deliver it. A broadcast text carries its own
/// chat type; the authored action decides when that is not one this tier relays (a monster emote
/// line still reaches players as its say/yell). The broadcast text's emote belongs to the line,
/// so a Refusal at delivery silences both.
fn speak<W: EventAiWorld>(
    world: &mut W,
    context: &EventContext,
    speech: &SpeakInstruction,
    linked_choice: u64,
) -> bool {
    let chat_type = match speech.mode {
        SpeechMode::Say => CHAT_SAY,
        SpeechMode::Yell => CHAT_YELL,
    };
    let ids = &speech.broadcast_ids;
    let picked = match ids.len() {
        0 => None,
        1 => Some(ids[0]),
        len => Some(ids[linked_choice as usize % len]),
    };
    let (message, chat_type, language, emote) = match picked {
        None => (speech.legacy_text.clone(), chat_type, 0, 0),
        Some(id) => {
            let Some(line) = world.eventai_broadcast(id) else {
                return false;
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
    spoken
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
        invocation_seed: 0,
        invocation_started: false,
        executing: false,
        invocation_branch: 0,
        paused_at_ms: 0,
    });
    world.put_eventai_rule_state(context.creature_guid, rule.source_rule_id, state);
}

/// A Refusal from a combat action's primary instruction keeps its saved opportunity open. The next
/// firing retries the same chance and linked choices. Ordinary rules spend a Refusal, and later
/// instructions never rewind effects already applied.
fn hold_opportunity_open<W: EventAiWorld>(
    world: &mut W,
    context: &EventContext,
    rule: &EventAiRule,
    open_state: RuleState,
) {
    if rule.execution != ExecutionPolicy::CombatAction {
        return;
    }
    let mut state = open_state;
    state.next_eligible_ms = context.now_ms;
    state.executing = false;
    world.put_eventai_rule_state(context.creature_guid, rule.source_rule_id, state);
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
        RecurrencePolicy::Repeat(window) if rule.event.kind().supports_repeat_cooldown() => {
            Some(RuleState {
                next_eligible_ms: context.now_ms.saturating_add(roll_window(
                    world,
                    window.min_ms,
                    window.max_ms,
                )),
                consumed: false,
                lifecycle_id: creature_state.lifecycle_id,
                engagement_id: creature_state.engagement_id,
                invocation_seed: 0,
                invocation_started: false,
                executing: false,
                invocation_branch: 0,
                paused_at_ms: 0,
            })
        }
        RecurrencePolicy::RepeatOnEvent => Some(RuleState {
            next_eligible_ms: context.now_ms,
            consumed: false,
            lifecycle_id: creature_state.lifecycle_id,
            engagement_id: creature_state.engagement_id,
            invocation_seed: 0,
            invocation_started: false,
            executing: false,
            invocation_branch: 0,
            paused_at_ms: 0,
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

fn initial_window(rule: &EventAiRule, engaged: bool) -> Option<super::TimeWindow> {
    match rule.event {
        EventCondition::TimedInCombat(window) if engaged => Some(window),
        EventCondition::TimedOutOfCombat(window) if !engaged => Some(window),
        EventCondition::TimedGeneric(window) => Some(window),
        EventCondition::FriendlyHealthDeficit(_)
        | EventCondition::FriendlyCrowdControlled(_)
        | EventCondition::FriendlyMissingAura(_)
        | EventCondition::SelectAttackingTarget(_)
            if engaged =>
        {
            match rule.recurrence {
                RecurrencePolicy::Repeat(window) => Some(window),
                RecurrencePolicy::Once | RecurrencePolicy::RepeatOnEvent => None,
            }
        }
        _ => None,
    }
}

fn fresh_invocation<W: EventAiWorld>(
    world: &W,
    context: &EventContext,
    rule: &EventAiRule,
) -> (u64, u32) {
    let deterministic = (context.creature_guid
        ^ rule.source_rule_id.rotate_left(23)
        ^ context.now_ms.rotate_left(41)) as u32;
    let chance = if rule.chance_pct < 100 {
        world.eventai_roll()
    } else {
        deterministic
    };
    let linked = if rule_uses_linked_random(rule) {
        world.eventai_roll()
    } else {
        deterministic.rotate_left(11)
    };
    let branch = if rule.selection == InstructionSelection::RandomOne {
        world.eventai_roll()
    } else {
        linked
    };
    (u64::from(chance) | (u64::from(linked) << 32), branch)
}

fn invocation_choice(seed: u64, branch: u32, lane: u64) -> u64 {
    match lane {
        0 => seed & u64::from(u32::MAX),
        1 => seed >> 32,
        2 => u64::from(branch),
        _ => unreachable!("invocation random has three lanes"),
    }
}

fn rule_uses_linked_random(rule: &EventAiRule) -> bool {
    matches!(rule.event, EventCondition::SelectAttackingTarget(_))
        || rule
            .instructions
            .iter()
            .any(|instruction| match instruction {
                CreatureInstruction::Speak(speech) => speech.broadcast_ids.len() > 1,
                CreatureInstruction::Cast(cast) => random_target(cast.target),
                CreatureInstruction::Emote(emote) => random_target(emote.target),
                CreatureInstruction::Summon(summon) => random_target(summon.target),
                CreatureInstruction::RandomPhase(_) | CreatureInstruction::RandomPhaseRange(_) => {
                    true
                }
                CreatureInstruction::FleeForAssist
                | CreatureInstruction::CallForHelp(_)
                | CreatureInstruction::SetPhase(_)
                | CreatureInstruction::IncrementPhase(_)
                | CreatureInstruction::SetRangedPosture(_) => false,
            })
}

fn random_target(target: super::InstructionTarget) -> bool {
    matches!(
        target,
        super::InstructionTarget::RandomThreat
            | super::InstructionTarget::RandomThreatExceptHighest
            | super::InstructionTarget::RandomThreatCharacter
            | super::InstructionTarget::RandomThreatCharacterExceptHighest
            | super::InstructionTarget::RandomHostileManaUser
    )
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
        entry: entity.entry,
        x: entity.x,
        y: entity.y,
        z: entity.z,
        map_id: entity.map_id,
        instance_id: entity.instance_id,
        zone_id: entity.zone_id,
        health: entity.health,
        max_health: entity.max_health,
        power: entity.power,
        max_power: entity.max_power,
        power_type: (entity.unit_bytes_0 >> 24) as u8,
        level: entity.level,
        faction_template: entity.faction_template,
        dead: entity.dead,
        is_player: entity.is_player(),
        orientation: entity.orientation,
        owner_guid: entity.owner_guid,
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

    fn eventai_cycle_actors(&self, scope: &TickScope, active: &HashSet<u64>) -> Vec<CycleActor> {
        let entities = self.ctx.db.game_world_entity();
        let fights = self.ctx.db.game_melee_attack();
        let mut actors = BTreeMap::new();
        for guid in active {
            let Some(creature) = entities.guid().find(guid) else {
                continue;
            };
            if creature.dead
                || !scope.covers(creature.instance_id)
                || !super::runs_eventai(&creature)
            {
                continue;
            }
            let target = fights
                .attacker_guid()
                .find(creature.guid)
                .map(|fight| fight.target_guid);
            actors.insert(
                creature.guid,
                CycleActor {
                    creature_guid: creature.guid,
                    current_target_guid: target,
                    engaged: target.is_some(),
                },
            );
        }
        for fight in self.eventai_fights(scope) {
            actors.entry(fight.creature_guid).or_insert(CycleActor {
                creature_guid: fight.creature_guid,
                current_target_guid: Some(fight.victim_guid),
                engaged: true,
            });
        }
        actors.into_values().collect()
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
                invocation_seed: row.invocation_seed,
                invocation_started: row.invocation_started,
                executing: row.executing,
                invocation_branch: row.invocation_branch,
                paused_at_ms: row.paused_at_ms,
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

    fn eventai_spawner_guid(&self, creature_guid: u64) -> Option<u64> {
        self.ctx
            .db
            .game_creature_ai_summon_origin()
            .creature_guid()
            .find(creature_guid)
            .map(|origin| origin.summoner_guid)
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

    fn eventai_aura_stacks(&self, guid: u64, spell_id: u32) -> u32 {
        self.ctx
            .db
            .game_aura()
            .by_target()
            .filter(&guid)
            .filter(|aura| aura.spell_id == spell_id)
            .map(|aura| u32::from(aura.stacks.max(1)))
            .max()
            .unwrap_or(0)
    }

    fn eventai_is_crowd_controlled(&self, guid: u64, dispel_type: u32) -> bool {
        let spells = self.ctx.db.game_spell();
        self.ctx
            .db
            .game_aura()
            .by_target()
            .filter(&guid)
            .any(|aura| {
                (aura.eff_kind == crate::spell::A_CONTROL
                    || (aura.eff_kind == crate::spell::A_MOD_SPEED
                        && aura.eff_p0 == i32::from(crate::spell::SPEED_MOVE)
                        && aura.amount < 0))
                    && (dispel_type == 0
                        || spells
                            .spell_id()
                            .find(aura.spell_id)
                            .is_some_and(|spell| u32::from(spell.dispel_type) == dispel_type))
            })
    }

    fn eventai_is_casting(&self, guid: u64) -> bool {
        crate::spell::is_non_melee_spell_casting(self.ctx, guid)
    }

    fn eventai_factions_friendly(&self, first: u32, second: u32) -> bool {
        crate::faction::is_friendly(self.ctx, first, second)
            || (self.ctx.db.game_faction_template().count() == 0 && first == second)
    }

    fn eventai_factions_hostile(&self, first: u32, second: u32) -> bool {
        crate::faction::is_hostile(self.ctx, first, second)
    }

    fn eventai_line_of_sight(&self, first: &EventAiUnit, second: &EventAiUnit) -> bool {
        first.map_id == second.map_id
            && first.instance_id == second.instance_id
            && crate::nav::has_los(
                self.ctx,
                first.map_id,
                (first.x, first.y, first.z),
                (second.x, second.y, second.z),
            )
    }

    fn eventai_spell_range(&self, spell_id: u32) -> Option<u32> {
        self.ctx
            .db
            .game_spell()
            .spell_id()
            .find(spell_id)
            .map(|spell| spell.range_yd)
    }

    fn eventai_spell_excludes_caster(&self, spell_id: u32) -> bool {
        self.ctx
            .db
            .game_creature_ai_spell_metadata()
            .spell_id()
            .find(spell_id)
            .is_some_and(|metadata| metadata.exclude_caster)
    }

    fn eventai_is_engaged(&self, guid: u64) -> bool {
        crate::combat::is_engaged(self.ctx, guid)
    }

    fn eventai_matches_predicate(&self, guid: u64, predicate: EventPredicate) -> bool {
        match predicate {
            EventPredicate::Always => true,
            EventPredicate::Alliance | EventPredicate::Horde => {
                let Some(character) = crate::helpers::character_by_guid(self.ctx, guid) else {
                    return false;
                };
                let team = lyracore_shared::faction::team_for_race(character.race);
                match predicate {
                    EventPredicate::Alliance => team == 469,
                    EventPredicate::Horde => team == 67,
                    _ => unreachable!("team predicates handled in this branch"),
                }
            }
            EventPredicate::QuestTaken(quest) => {
                crate::quest::quest_is_taken(self.ctx, guid, quest.quest_entry)
            }
        }
    }

    fn eventai_in_zone_or_area(&self, unit: &EventAiUnit, zone_or_area_id: u32) -> bool {
        unit.zone_id == zone_or_area_id
            || crate::terrain::area_id_at(self.ctx, unit.map_id, unit.x, unit.y)
                == Some(zone_or_area_id)
    }

    fn eventai_combat_action_ready(&self, guid: u64) -> bool {
        !crate::spell::is_action_blocked(self.ctx, guid)
            && !self.eventai_is_casting(guid)
            && !self.eventai_rout_ends_ms(guid).is_some_and(|ends| {
                crate::creatures::ai::rout_window_open(self.eventai_now_ms() as u32, ends)
            })
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
            row.invocation_seed = state.invocation_seed;
            row.invocation_started = state.invocation_started;
            row.executing = state.executing;
            row.invocation_branch = state.invocation_branch;
            row.paused_at_ms = state.paused_at_ms;
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
                invocation_seed: state.invocation_seed,
                invocation_started: state.invocation_started,
                executing: state.executing,
                invocation_branch: state.invocation_branch,
                paused_at_ms: state.paused_at_ms,
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

    fn eventai_start_spell(
        &mut self,
        caster: &EventAiUnit,
        spell_id: u32,
        mode: super::SpellStartMode,
        target: super::SpellCastTarget,
        interrupt_previous: bool,
        admission: super::SpellCasterAdmission,
    ) -> bool {
        let mode = match mode {
            super::SpellStartMode::Direct => crate::spell::CreatureSpellStartMode::Direct,
            super::SpellStartMode::Triggered => crate::spell::CreatureSpellStartMode::Triggered,
        };
        let target = match target {
            super::SpellCastTarget::Unit(guid) => crate::spell::CreatureSpellTarget::Unit(guid),
            super::SpellCastTarget::None => crate::spell::CreatureSpellTarget::None,
            super::SpellCastTarget::CasterArea => crate::spell::CreatureSpellTarget::CasterArea,
        };
        let admission = match admission {
            super::SpellCasterAdmission::Living => {
                crate::spell::CreatureSpellCasterAdmission::Living
            }
            super::SpellCasterAdmission::DeadCreatureCallback => {
                crate::spell::CreatureSpellCasterAdmission::DeadCreatureCallback
            }
        };
        crate::spell::start_creature_spell(
            self.ctx,
            crate::spell::CreatureSpellStart {
                caster_guid: caster.guid,
                caster_level: caster.level as u8,
                spell_id,
                mode,
                target,
                interrupt_previous,
                admission,
            },
        )
        .is_ok()
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
        crate::combat::arm_creature_engagement(self.ctx, creature_guid, victim_guid, true);
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
        super::mobility::place_summon(self.ctx, sequence, guid, entry, location, summoner);
        super::edges::eventai_on_summoned(self.ctx, summoner.guid, guid, entry);
    }

    fn eventai_engage_summon(&mut self, summon_guid: u64, target_guid: u64) {
        super::mobility::engage_summon(self.ctx, summon_guid, target_guid)
    }
}
