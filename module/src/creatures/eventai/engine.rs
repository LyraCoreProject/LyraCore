use std::collections::{BTreeMap, HashSet};

use spacetimedb::{log, ReducerContext, Table};

use super::{
    effective_rule_id, ActionKind, ActionResult, CreatureAiEvent, CreatureAiRuleState,
    CreatureAiState, CreatureState, Diagnostic, DiagnosticKind, EventAiRequest, EventContext,
    EventKind, RepeatPolicy, Rule, RuleAction, RuleState, Subject, TargetPolicy,
    SOURCE_FLAG_COMBAT_ACTION,
};
use crate::chat::{ChatEvent, CHAT_SAY, CHAT_YELL};
use crate::{
    game_chat_event, game_creature_ai_broadcast_text, game_creature_ai_event,
    game_creature_ai_rule_state, game_creature_ai_state, game_world_entity,
};

pub(crate) trait EventAiWorld {
    fn eventai_contexts(&self, request: &EventAiRequest<'_>) -> Vec<EventContext>;
    fn eventai_rows(&self, creature_guid: u64) -> Vec<CreatureAiEvent>;
    fn eventai_creature_exists(&self, creature_guid: u64) -> bool;
    fn eventai_creature_state(&self, creature_guid: u64) -> CreatureState;
    fn set_eventai_phase(&mut self, creature_guid: u64, phase: u8);
    fn eventai_rule_state(&self, creature_guid: u64, rule_id: u64) -> Option<RuleState>;
    fn put_eventai_rule_state(&mut self, creature_guid: u64, rule_id: u64, state: RuleState);
    fn delete_eventai_rule_state(&mut self, creature_guid: u64, rule_id: u64);
    fn reap_eventai_rule_state(&mut self);
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
    fn eventai_target(&self, _context: &EventContext, _policy: TargetPolicy) -> Option<u64> {
        None
    }

    // Summon and ranged-posture operations extend here.
}

pub(crate) fn evaluate<W: EventAiWorld>(world: &mut W, request: EventAiRequest<'_>) -> u64 {
    if matches!(&request, EventAiRequest::Engaged(_)) {
        world.reap_eventai_rule_state();
    }
    let mut visited = 0;
    for context in world.eventai_contexts(&request) {
        let rows = world.eventai_rows(context.creature_guid);
        visited += rows.len() as u64;
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
            evaluate_rule(world, &context, &rule);
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

    if rule.event == EventKind::TimedInCombat {
        match state {
            None => {
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
            Some(current) if context.now_ms < current.next_eligible_ms => return,
            Some(_) => {}
        }
    }

    if rule.chance_pct < 100 && world.eventai_roll() % 100 >= rule.chance_pct as u32 {
        finish_opportunity(world, context, rule, creature_state);
        return;
    }

    for (index, action) in rule.actions.iter().enumerate() {
        if context.assisted && matches!(action.kind, ActionKind::Say | ActionKind::Yell) {
            continue;
        }
        let result = execute_action(world, context, action);
        match result {
            ActionResult::Applied => {}
            ActionResult::Refused
                if (index == 0
                    && action.kind == ActionKind::Cast
                    && rule.source_policy.contains(SOURCE_FLAG_COMBAT_ACTION))
                    || action.cast_options.contains(super::CAST_REQUIRED) =>
            {
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
    finish_opportunity(world, context, rule, creature_state);
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
            let Some(target) = basic_target(world, context, action.target) else {
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
    policy: TargetPolicy,
) -> Option<u64> {
    match policy {
        TargetPolicy::Current => context.current_target_guid,
        TargetPolicy::SelfActor => Some(context.creature_guid),
        TargetPolicy::Invoker => context.invoker_guid,
        TargetPolicy::EventTarget => context.event_target_guid,
        other => world.eventai_target(context, other),
    }
}

fn finish_opportunity<W: EventAiWorld>(
    world: &mut W,
    context: &EventContext,
    rule: &Rule,
    creature_state: CreatureState,
) {
    let state = match rule.repeat {
        RepeatPolicy::Once => RuleState {
            next_eligible_ms: 0,
            consumed: true,
            lifecycle_id: creature_state.lifecycle_id,
            engagement_id: creature_state.engagement_id,
        },
        RepeatPolicy::Repeat => RuleState {
            next_eligible_ms: context.now_ms.saturating_add(roll_window(
                world,
                rule.event_params[2],
                rule.event_params[3],
            )),
            consumed: false,
            lifecycle_id: creature_state.lifecycle_id,
            engagement_id: creature_state.engagement_id,
        },
    };
    world.put_eventai_rule_state(context.creature_guid, rule.id, state);
}

fn roll_window<W: EventAiWorld>(world: &W, min: u32, max: u32) -> u64 {
    if max == min {
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
        let rules = self.ctx.db.game_creature_ai_event();
        let Some(creature) = self.ctx.db.game_world_entity().guid().find(creature_guid) else {
            return Vec::new();
        };
        rules
            .by_entry()
            .filter(&creature.entry)
            .chain(rules.by_guid().filter(&creature_guid))
            .collect()
    }

    fn eventai_creature_exists(&self, creature_guid: u64) -> bool {
        self.ctx
            .db
            .game_world_entity()
            .guid()
            .find(creature_guid)
            .is_some()
    }

    fn eventai_creature_state(&self, creature_guid: u64) -> CreatureState {
        self.state_row(creature_guid)
            .map_or_else(CreatureState::default, |row| CreatureState {
                phase: row.phase,
                lifecycle_id: row.lifecycle_id,
                engagement_id: row.engagement_id,
                ranged_distance: row.ranged_distance,
                ranged_angle: row.ranged_angle,
            })
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

    fn reap_eventai_rule_state(&mut self) {
        let valid_rules: HashSet<u64> = self
            .ctx
            .db
            .game_creature_ai_event()
            .iter()
            .map(|row| effective_rule_id(&row))
            .collect();
        let state = self.ctx.db.game_creature_ai_rule_state();
        for row in state.iter().collect::<Vec<_>>() {
            if !self.eventai_creature_exists(row.creature_guid)
                || !valid_rules.contains(&row.source_rule_id)
            {
                state.id().delete(row.id);
            }
        }
    }

    fn eventai_roll(&self) -> u32 {
        self.ctx.random()
    }

    fn eventai_speak(&mut self, context: &EventContext, action: &RuleAction, chat_type: u8) {
        let broadcast = self
            .ctx
            .db
            .game_creature_ai_broadcast_text()
            .id()
            .find(action.params[0]);
        let (message, chat_type, language) = broadcast.map_or_else(
            || (action.legacy_text.clone(), chat_type, 0),
            |text| (text.male_text, text.chat_type, text.language_id),
        );
        if message.is_empty() {
            return;
        }
        self.ctx.db.game_chat_event().insert(ChatEvent {
            id: 0,
            sender_guid: context.creature_guid,
            chat_type,
            language,
            message,
            created_at: self.ctx.timestamp,
        });
    }

    fn eventai_cast(
        &mut self,
        context: &EventContext,
        action: &RuleAction,
        target_guid: u64,
    ) -> ActionResult {
        let implemented_options = super::CAST_TRIGGERED | super::CAST_REQUIRED;
        if action.cast_options.bits() & !implemented_options != 0 {
            return ActionResult::Unsupported;
        }
        let Some(creature) = self
            .ctx
            .db
            .game_world_entity()
            .guid()
            .find(context.creature_guid)
        else {
            return ActionResult::Refused;
        };
        if crate::spell::begin_cast(
            self.ctx,
            creature.guid,
            action.params[0],
            creature.level as u8,
            target_guid,
            action.cast_options.contains(super::CAST_TRIGGERED),
            None,
        )
        .is_ok()
        {
            ActionResult::Applied
        } else {
            ActionResult::Refused
        }
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

    fn eventai_target(&self, context: &EventContext, policy: TargetPolicy) -> Option<u64> {
        super::combat::target(self.ctx, context, policy)
    }

    fn eventai_diagnostic(&mut self, diagnostic: Diagnostic) {
        log::error!("EventAI rule refused: {diagnostic:?}");
    }
}
