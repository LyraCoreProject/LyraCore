use spacetimedb::{ReducerContext, Table};

use super::{
    CommandLists, CreatureInstruction, EventAiRule, EventCondition, ExecutionPolicy,
    InstructionSelection, PhaseSet, PostureAdmission, RecurrencePolicy, ScriptEventPolicy,
};

// Policy storage must preserve the fieldless layout; see docs/data-ingestion.md, Command lists.
#[derive(spacetimedb::SpacetimeType, Clone, Copy)]
pub enum StoredSelection {
    All,
    RandomOne,
    CommandLists,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy)]
pub enum StoredExecution {
    Ordinary,
    CombatAction,
    ScriptEvent,
}

#[derive(spacetimedb::SpacetimeType, Clone)]
pub struct StoredEventAiRule {
    pub source_rule_id: u64,
    pub event: EventCondition,
    pub chance_pct: u8,
    pub allowed_phases: PhaseSet,
    pub recurrence: RecurrencePolicy,
    pub selection: StoredSelection,
    pub execution: StoredExecution,
    pub posture: PostureAdmission,
    pub instructions: Vec<CreatureInstruction>,
}

#[spacetimedb::table(accessor = game_creature_ai_script_policy)]
pub struct CreatureAiScriptPolicy {
    #[primary_key]
    pub id: String,
    pub selection: CommandLists,
    pub execution: ScriptEventPolicy,
}

fn policy_id(revision: u64, rule_id: u64) -> String {
    format!("{revision}:{rule_id}")
}

pub(super) fn store(ctx: &ReducerContext, revision: u64, rule: EventAiRule) -> StoredEventAiRule {
    if let (
        InstructionSelection::CommandLists(selection),
        ExecutionPolicy::ScriptEvent(execution),
    ) = (&rule.selection, rule.execution)
    {
        ctx.db
            .game_creature_ai_script_policy()
            .insert(CreatureAiScriptPolicy {
                id: policy_id(revision, rule.source_rule_id),
                selection: selection.clone(),
                execution,
            });
    }
    StoredEventAiRule {
        source_rule_id: rule.source_rule_id,
        event: rule.event,
        chance_pct: rule.chance_pct,
        allowed_phases: rule.allowed_phases,
        recurrence: rule.recurrence,
        selection: match rule.selection {
            InstructionSelection::All => StoredSelection::All,
            InstructionSelection::RandomOne => StoredSelection::RandomOne,
            InstructionSelection::CommandLists(_) => StoredSelection::CommandLists,
        },
        execution: match rule.execution {
            ExecutionPolicy::Ordinary => StoredExecution::Ordinary,
            ExecutionPolicy::CombatAction => StoredExecution::CombatAction,
            ExecutionPolicy::ScriptEvent(_) => StoredExecution::ScriptEvent,
        },
        posture: rule.posture,
        instructions: rule.instructions,
    }
}

impl StoredEventAiRule {
    pub(super) fn restore(self, ctx: &ReducerContext, revision: u64) -> Option<EventAiRule> {
        let (selection, execution) = match (self.selection, self.execution) {
            (StoredSelection::CommandLists, StoredExecution::ScriptEvent) => {
                let policy = ctx
                    .db
                    .game_creature_ai_script_policy()
                    .id()
                    .find(policy_id(revision, self.source_rule_id))?;
                (
                    InstructionSelection::CommandLists(policy.selection),
                    ExecutionPolicy::ScriptEvent(policy.execution),
                )
            }
            (StoredSelection::CommandLists, _) | (_, StoredExecution::ScriptEvent) => return None,
            (selection, execution) => (
                match selection {
                    StoredSelection::All => InstructionSelection::All,
                    StoredSelection::RandomOne => InstructionSelection::RandomOne,
                    StoredSelection::CommandLists => unreachable!(),
                },
                match execution {
                    StoredExecution::Ordinary => ExecutionPolicy::Ordinary,
                    StoredExecution::CombatAction => ExecutionPolicy::CombatAction,
                    StoredExecution::ScriptEvent => unreachable!(),
                },
            ),
        };
        Some(EventAiRule {
            source_rule_id: self.source_rule_id,
            event: self.event,
            chance_pct: self.chance_pct,
            allowed_phases: self.allowed_phases,
            recurrence: self.recurrence,
            selection,
            execution,
            posture: self.posture,
            instructions: self.instructions,
        })
    }
}
