use std::collections::{BTreeMap, HashSet};

use spacetimedb::{log, ReducerContext, Table};

use super::{
    effective_rule_id, ActionKind, ActionResult, CreatureAiEvent, CreatureAiRuleState,
    CreatureAiState, CreatureState, Diagnostic, DiagnosticKind, EventAiRequest, EventContext,
    EventKind, RepeatPolicy, Rule, RuleAction, RuleState, Subject, TargetPolicy,
    SOURCE_FLAG_COMBAT_ACTION,
};
use crate::chat::{is_supported_chat_type, CHAT_SAY, CHAT_YELL};
use crate::{
    game_creature_ai_broadcast_text, game_creature_ai_rule_state, game_creature_ai_state,
    game_world_entity,
};

/// How long a rule waits after a Refusal from its opening cast; `hold_opportunity_open` explains
/// the policy.
const CAST_RETRY_MS: u64 = 1_500;

pub(crate) trait EventAiWorld {
    fn eventai_contexts(&self, request: &EventAiRequest<'_>) -> Vec<EventContext>;
    /// Checks the rule's event condition and may name the actor selected by that condition.
    fn eventai_condition(&self, context: &EventContext, rule: &Rule) -> Option<EventContext>;
    fn eventai_rows(&self, creature_guid: u64) -> Vec<CreatureAiEvent>;
    fn eventai_creature_state(&self, creature_guid: u64) -> CreatureState;
    fn set_eventai_phase(&mut self, creature_guid: u64, phase: u8);
    fn eventai_rule_state(&self, creature_guid: u64, rule_id: u64) -> Option<RuleState>;
    fn put_eventai_rule_state(&mut self, creature_guid: u64, rule_id: u64, state: RuleState);
    fn delete_eventai_rule_state(&mut self, creature_guid: u64, rule_id: u64);
    /// Remove state for missing rules on this evaluated creature. Lifecycle edges clean state for
    /// creatures that no longer produce evaluation contexts.
    fn reap_eventai_rule_state(&mut self, creature_guid: u64, valid_rule_ids: &HashSet<u64>);
    fn eventai_roll(&self) -> u32;
    fn eventai_speak(&mut self, context: &EventContext, action: &RuleAction, chat_type: u8);
    fn eventai_cast(
        &mut self,
        context: &EventContext,
        action: &RuleAction,
        target_guid: u64,
    ) -> ActionResult;
    fn eventai_combat_action(
        &mut self,
        _context: &EventContext,
        _action: &RuleAction,
    ) -> ActionResult {
        ActionResult::Unsupported
    }
    fn eventai_mobility_action(
        &mut self,
        _context: &EventContext,
        _action: &RuleAction,
    ) -> ActionResult {
        ActionResult::Unsupported
    }
    fn eventai_diagnostic(&mut self, diagnostic: Diagnostic);

    // Combat actions and target selection extend here. Until then, these policies fail closed.
    fn eventai_target(&self, _context: &EventContext, _action: &RuleAction) -> Option<u64> {
        None
    }

    // Summon and ranged-posture operations extend here.
}

pub(crate) fn evaluate<W: EventAiWorld>(world: &mut W, request: EventAiRequest<'_>) -> u64 {
    let mut visited = 0;
    let mut contexts_by_creature: BTreeMap<u64, Vec<EventContext>> = BTreeMap::new();
    for context in world.eventai_contexts(&request) {
        contexts_by_creature
            .entry(context.creature_guid)
            .or_default()
            .push(context);
    }
    for (creature_guid, contexts) in contexts_by_creature {
        let rows = world.eventai_rows(creature_guid);
        visited += rows.len() as u64;
        let valid_rule_ids = rows.iter().map(effective_rule_id).collect();
        world.reap_eventai_rule_state(creature_guid, &valid_rule_ids);
        let mut groups: BTreeMap<u64, Vec<CreatureAiEvent>> = BTreeMap::new();
        for row in rows {
            groups.entry(effective_rule_id(&row)).or_default().push(row);
        }
        for rows in groups.into_values() {
            let rule = match Rule::decode(rows) {
                Ok(rule) => rule,
                Err(diagnostic) => {
                    world.eventai_diagnostic(diagnostic);
                    continue;
                }
            };
            let Some(context) = contexts.iter().find(|context| context.kind == rule.event) else {
                continue;
            };
            evaluate_rule(world, context, &rule);
        }
    }
    visited
}

fn evaluate_rule<W: EventAiWorld>(world: &mut W, context: &EventContext, rule: &Rule) {
    if rule.event != context.kind || !subject_matches(rule.subject, context.creature_guid) {
        return;
    }
    let creature_state = world.eventai_creature_state(context.creature_guid);
    if creature_state.phase >= 32 || rule.allowed_phase_mask & (1u32 << creature_state.phase) == 0 {
        return;
    }
    let state = world
        .eventai_rule_state(context.creature_guid, rule.id)
        .filter(|state| {
            state.lifecycle_id == creature_state.lifecycle_id
                && state.engagement_id == creature_state.engagement_id
        });
    if state.is_none() {
        world.delete_eventai_rule_state(context.creature_guid, rule.id);
    }
    if state.is_some_and(|state| state.consumed) {
        return;
    }

    if state.is_some_and(|state| context.now_ms < state.next_eligible_ms) {
        return;
    }

    if rule.event == EventKind::TimedInCombat && state.is_none() {
        let delay = roll_window(world, rule.event_params[0], rule.event_params[1]);
        world.put_eventai_rule_state(
            context.creature_guid,
            rule.id,
            RuleState {
                next_eligible_ms: context.now_ms.saturating_add(delay),
                consumed: false,
                lifecycle_id: creature_state.lifecycle_id,
                engagement_id: creature_state.engagement_id,
            },
        );
        return;
    }

    let Some(context) = world.eventai_condition(context, rule) else {
        return;
    };

    // A missed chance roll costs the opportunity, never the rule. CMaNGOS re-arms a recurring
    // event's repeat window before it rolls and returns without disabling the event, so a repeat
    // rule waits one window and a once-only rule stays armed to roll again next opportunity.
    if rule.chance_pct < 100 && world.eventai_roll() % 100 >= rule.chance_pct as u32 {
        if let Some(state) = repeat_state(world, &context, rule, creature_state) {
            world.put_eventai_rule_state(context.creature_guid, rule.id, state);
        }
        return;
    }

    let actions: Vec<&RuleAction> = if rule
        .source_policy
        .contains(super::SOURCE_FLAG_RANDOM_ACTION)
    {
        rule.actions
            .get(world.eventai_roll() as usize % rule.actions.len())
            .into_iter()
            .collect()
    } else {
        rule.actions.iter().collect()
    };
    for (index, action) in actions.into_iter().enumerate() {
        if context.assisted && matches!(action.kind, ActionKind::Say | ActionKind::Yell) {
            continue;
        }
        let result = execute_action(world, &context, action);
        match result {
            ActionResult::Applied => {}
            // A Refusal from a LATER action does not rewind the actions already applied, so it
            // spends the opportunity like any other outcome.
            ActionResult::Refused if index == 0 && action.kind == ActionKind::Cast => {
                hold_opportunity_open(world, &context, rule, creature_state);
                return;
            }
            ActionResult::Refused => {}
            ActionResult::Unsupported => {
                world.eventai_diagnostic(Diagnostic {
                    rule_id: rule.id,
                    row_id: action.row_id,
                    kind: DiagnosticKind::UnsupportedAction,
                    value: action.kind as u64,
                });
                return;
            }
        }
    }
    finish_opportunity(world, &context, rule, creature_state);
}

fn execute_action<W: EventAiWorld>(
    world: &mut W,
    context: &EventContext,
    action: &RuleAction,
) -> ActionResult {
    match action.kind {
        ActionKind::Say => {
            world.eventai_speak(context, action, CHAT_SAY);
            ActionResult::Applied
        }
        ActionKind::Yell => {
            world.eventai_speak(context, action, CHAT_YELL);
            ActionResult::Applied
        }
        ActionKind::Cast => {
            let Some(target) = basic_target(world, context, action) else {
                return ActionResult::Refused;
            };
            if action.params[0] == 0 {
                ActionResult::Refused
            } else {
                world.eventai_cast(context, action, target)
            }
        }
        ActionKind::SetPhase => {
            let Ok(phase) = u8::try_from(action.params[0]) else {
                return ActionResult::Refused;
            };
            if phase >= 32 {
                return ActionResult::Refused;
            }
            world.set_eventai_phase(context.creature_guid, phase);
            ActionResult::Applied
        }
        ActionKind::Emote | ActionKind::FleeForAssist | ActionKind::CallForHelp => {
            world.eventai_combat_action(context, action)
        }
        ActionKind::Summon | ActionKind::SetRangedPosture => {
            world.eventai_mobility_action(context, action)
        }
    }
}

fn basic_target<W: EventAiWorld>(
    world: &W,
    context: &EventContext,
    action: &RuleAction,
) -> Option<u64> {
    match action.target {
        TargetPolicy::Current => context.current_target_guid,
        TargetPolicy::SelfActor => Some(context.creature_guid),
        TargetPolicy::Invoker => context.invoker_guid,
        TargetPolicy::EventTarget => context.event_target_guid,
        _ => world.eventai_target(context, action),
    }
}

fn finish_opportunity<W: EventAiWorld>(
    world: &mut W,
    context: &EventContext,
    rule: &Rule,
    creature_state: CreatureState,
) {
    let state = repeat_state(world, context, rule, creature_state).unwrap_or(RuleState {
        next_eligible_ms: 0,
        consumed: true,
        lifecycle_id: creature_state.lifecycle_id,
        engagement_id: creature_state.engagement_id,
    });
    world.put_eventai_rule_state(context.creature_guid, rule.id, state);
}

/// A Refusal from a rule's OPENING cast is transient (a cast already in flight, the target out of
/// range or gone), so the rule keeps its opportunity instead of spending a whole repeat window on
/// it. A rule the source marks as the creature's combat action comes back on the very next firing,
/// the way CMaNGOS retries one; every other rule waits `CAST_RETRY_MS`, which is the retry the
/// timer evaluator this engine replaced used for the same Refusal. An edge rule keeps its arming
/// untouched either way: no window stamped on an edge would ever be reached again.
fn hold_opportunity_open<W: EventAiWorld>(
    world: &mut W,
    context: &EventContext,
    rule: &Rule,
    creature_state: CreatureState,
) {
    if rule.source_policy.contains(SOURCE_FLAG_COMBAT_ACTION) || !rule.event.recurs() {
        return;
    }
    world.put_eventai_rule_state(
        context.creature_guid,
        rule.id,
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
    rule: &Rule,
    creature_state: CreatureState,
) -> Option<RuleState> {
    (rule.repeat == RepeatPolicy::Repeat && rule.event.recurs()).then(|| RuleState {
        next_eligible_ms: context.now_ms.saturating_add(roll_window(
            world,
            rule.event_params[2],
            rule.event_params[3],
        )),
        consumed: false,
        lifecycle_id: creature_state.lifecycle_id,
        engagement_id: creature_state.engagement_id,
    })
}

/// Random ms in `[min, max]`. An empty or inverted window is a fixed cadence of `min` and costs no
/// roll: `Rule::decode` refuses an inverted one, and this is what keeps a row that slipped past it
/// from wrapping the span to weeks or dividing by zero.
fn roll_window<W: EventAiWorld>(world: &W, min: u32, max: u32) -> u64 {
    if max <= min {
        return min as u64;
    }
    min as u64 + (world.eventai_roll() as u64 % (u64::from(max) - u64::from(min) + 1))
}

fn subject_matches(subject: Subject, creature_guid: u64) -> bool {
    match subject {
        Subject::Entry(_) => true,
        Subject::Guid(guid) => guid == creature_guid,
    }
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

impl EventAiWorld for DatabaseWorld<'_> {
    fn eventai_contexts(&self, request: &EventAiRequest<'_>) -> Vec<EventContext> {
        match request {
            EventAiRequest::Edge(context) => vec![*context],
            EventAiRequest::Engaged(scope) => {
                super::combat::engaged_contexts(self.ctx, scope, self.now_ms())
            }
        }
    }

    fn eventai_rows(&self, creature_guid: u64) -> Vec<CreatureAiEvent> {
        super::combat::rows_for(self.ctx, creature_guid)
    }

    fn eventai_condition(&self, context: &EventContext, rule: &Rule) -> Option<EventContext> {
        super::combat::condition(self.ctx, context, rule)
    }

    fn eventai_creature_state(&self, creature_guid: u64) -> CreatureState {
        self.state_row(creature_guid)
            .map_or_else(CreatureState::default, CreatureState::from)
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
                });
            }
        }
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

    fn eventai_roll(&self) -> u32 {
        self.ctx.random()
    }

    fn eventai_speak(&mut self, context: &EventContext, action: &RuleAction, chat_type: u8) {
        let ids: Vec<u32> = action.params.into_iter().filter(|id| *id != 0).collect();
        let broadcast = super::combat::pick(self.ctx, &ids)
            .and_then(|id| self.ctx.db.game_creature_ai_broadcast_text().id().find(id));
        let (message, chat_type, language, emote) = match (ids.is_empty(), broadcast) {
            (true, _) => (action.legacy_text.clone(), chat_type, 0, 0),
            // A broadcast text carries its own chat type; the authored action decides when that is
            // not one this tier relays (a monster emote line still reaches players as its say/yell).
            (false, Some(text)) => (
                text.male_text,
                if is_supported_chat_type(text.chat_type) {
                    text.chat_type
                } else {
                    chat_type
                },
                text.language_id,
                text.emote_id_1,
            ),
            (false, None) => return,
        };
        let Some(creature) = self
            .ctx
            .db
            .game_world_entity()
            .guid()
            .find(context.creature_guid)
        else {
            return;
        };
        // The say/yell chokepoint owns the dead-speaker Gate and the length cap; a creature line
        // goes through it like a player's rather than writing the event row itself. The broadcast
        // text's emote belongs to the line, so a Refusal there silences both.
        let spoken =
            crate::chat::apply_send_chat(self.ctx, creature, chat_type, language, message).is_ok();
        if !spoken || emote == 0 {
            return;
        }
        if let Some(speaker) = self
            .ctx
            .db
            .game_world_entity()
            .guid()
            .find(context.creature_guid)
        {
            let _ = crate::chat::apply_send_emote(self.ctx, speaker, 0, emote, 0);
        }
    }

    fn eventai_cast(
        &mut self,
        context: &EventContext,
        action: &RuleAction,
        target_guid: u64,
    ) -> ActionResult {
        super::combat::cast(self.ctx, context, action, target_guid)
    }

    fn eventai_combat_action(
        &mut self,
        context: &EventContext,
        action: &RuleAction,
    ) -> ActionResult {
        super::combat::execute(self.ctx, context, action)
    }

    fn eventai_mobility_action(
        &mut self,
        context: &EventContext,
        action: &RuleAction,
    ) -> ActionResult {
        super::mobility::execute(self.ctx, context, action)
    }

    fn eventai_target(&self, context: &EventContext, action: &RuleAction) -> Option<u64> {
        super::combat::target(self.ctx, context, action)
    }

    fn eventai_diagnostic(&mut self, diagnostic: Diagnostic) {
        log::error!("EventAI rule refused: {diagnostic:?}");
    }
}
