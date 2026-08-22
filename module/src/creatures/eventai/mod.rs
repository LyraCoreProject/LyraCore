//! Native creature EventAI rules and their durable state.

mod engine;
mod fixtures;
mod loader;
mod model;
mod tables;

mod combat;
mod edges;
mod mobility;

pub(crate) use combat::authored_combat;
#[cfg(test)]
pub(crate) use combat::AuthoredCombat;
pub(crate) use edges::reset_creature_lifecycle;
pub(crate) use edges::{
    creature_ai_on_aggro, creature_ai_on_creature_death, creature_ai_on_creature_spawn,
    reset_engagement, runs_eventai,
};
#[cfg(test)]
pub(crate) use engine::{evaluate, EventAiWorld};
pub(crate) use fixtures::seed_on_aggro_fixtures;
#[cfg(test)]
pub(crate) use mobility::summon_lifetime_after;
pub(crate) use mobility::{drop_summon_expiry, ranged_posture};
pub use mobility::{expire_eventai_summon, CreatureAiSummonExpiry};
pub(crate) use model::*;
pub use tables::*;

use spacetimedb::ReducerContext;

/// Run one explicit EventAI request against durable Module state.
pub(crate) fn evaluate_context(ctx: &ReducerContext, request: EventAiRequest) -> u64 {
    engine::evaluate(&mut engine::DatabaseWorld::new(ctx), request)
}

#[cfg(test)]
mod architecture_tests {
    #[test]
    fn source_discriminants_stop_at_the_importer_boundary() {
        let tables = include_str!("tables.rs");
        let legacy_start = tables
            .find("pub struct CreatureAiEvent {")
            .expect("the additive legacy migration table remains present");
        let native_start = tables
            .find("/// One normalized native EventAI definition")
            .expect("the native definition follows the legacy migration table");
        let legacy_table = &tables[legacy_start..native_start];
        let native_tables = format!("{}{}", &tables[..legacy_start], &tables[native_start..]);
        let production = [
            include_str!("model.rs"),
            include_str!("engine.rs"),
            include_str!("combat.rs"),
            include_str!("mobility.rs"),
            include_str!("edges.rs"),
            include_str!("fixtures.rs"),
            include_str!("loader.rs"),
            native_tables.as_str(),
        ];
        for forbidden in [
            "CMaNGOS",
            "cmangos",
            "inverse_phase_mask",
            "event_type",
            "action_type",
            "event_param_1",
            "action_param_1",
            "pub target_policy: u8",
            "pub cast_options: u32",
            "pub source_flags: u32",
            "NATIVE_EVENT_",
            "NATIVE_ACTION_",
            "NATIVE_TARGET_",
        ] {
            assert!(
                production.iter().all(|source| !source.contains(forbidden)),
                "Module production code names importer-only value `{forbidden}`"
            );
        }

        for retained_field in [
            "event_type",
            "action_type",
            "event_param_1",
            "action_param_1",
            "target_policy",
            "cast_options",
            "source_flags",
        ] {
            assert!(
                legacy_table.contains(retained_field),
                "the frozen legacy migration table lost `{retained_field}`"
            );
        }

        let importer = include_str!("../../../../importer/src/eventai.rs");
        for source_value in ["EVENT_TIMER_IN_COMBAT", "ACTION_CAST", "TARGET_HOSTILE"] {
            assert!(
                importer.contains(source_value),
                "the importer no longer owns `{source_value}`"
            );
        }
    }

    #[test]
    fn gateway_does_not_subscribe_module_only_eventai_tables() {
        let subscriptions = include_str!("../../../../gateway/src/stdb/connection.rs");
        for table in [
            "game_creature_ai_definition",
            "game_creature_ai_event",
            "game_creature_ai_state",
            "game_creature_ai_rule_state",
            "game_creature_ai_broadcast_text",
            "game_creature_ai_summon",
            "game_creature_ai_summon_expiry",
        ] {
            assert!(
                !subscriptions.contains(&format!("SELECT * FROM {table}")),
                "Gateway subscribes Module-only table `{table}`"
            );
        }
    }
}
