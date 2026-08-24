//! Native creature EventAI rules and their durable state.

mod engine;
mod fixtures;
mod loader;
mod model;
mod presentation;
mod relay;
mod tables;

mod combat;
mod death;
mod edges;
mod mobility;
pub(crate) mod movement;
mod quest_credit;
mod threat;

pub(crate) use combat::{authored_combat, current_definition_revision};
#[cfg(test)]
pub(crate) use combat::{beneficiary_guid, condition, AuthoredCombat};
pub(crate) use edges::reset_creature_lifecycle;
pub use edges::CreatureAiResetDeferral;
pub use edges::CreatureAiReturningHome;
pub(crate) use edges::{
    begin_death_dispatch, creature_ai_on_aggro, creature_ai_on_creature_death,
    creature_ai_on_creature_spawn, creature_ai_on_unit_death, finish_death_dispatch,
    reset_engagement, runs_eventai,
};
#[allow(
    unused_imports,
    reason = "later EventAI actions call these typed edge producers"
)]
pub(crate) use edges::{
    eventai_on_evade, eventai_on_reached_home, eventai_on_receive_ai_event,
    eventai_on_receive_emote, eventai_on_spell_hit, eventai_on_target_not_reachable,
};
#[cfg(test)]
pub(crate) use engine::{evaluate, EventAiWorld};
pub(crate) use fixtures::seed_on_aggro_fixtures;
#[cfg(feature = "debug_reducers")]
pub(crate) use loader::replace_definition_for_debug;
#[cfg(test)]
pub(crate) use mobility::summon_lifetime_after;
pub(crate) use mobility::{active_object, drop_summon_expiry, ranged_posture, react_state};
pub use mobility::{
    expire_eventai_summon, fire_eventai_forced_despawn, CreatureAiForcedDespawn,
    CreatureAiSummonExpiry, CreatureAiSummonOrigin,
};
pub(crate) use model::*;
pub use movement::{CreatureAiMovementIntent, CreatureAiMovementPathWaypoint};
use presentation::import_verified_rajaxx_spawn_protection;
pub(crate) use presentation::{
    CreaturePresentationInstruction, CreaturePresentationMount, FlagOverride,
};
pub use relay::*;
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
            include_str!("threat.rs"),
            include_str!("quest_credit.rs"),
            include_str!("edges.rs"),
            include_str!("fixtures.rs"),
            include_str!("loader.rs"),
            include_str!("presentation.rs"),
            include_str!("relay.rs"),
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
            "game_creature_ai_spell_metadata",
            "game_creature_ai_summon",
            "game_creature_ai_summon_expiry",
            "game_creature_ai_returning_home",
            "game_creature_presentation",
            "game_creature_ai_movement_intent",
            "game_creature_ai_movement_path_waypoint",
            "game_creature_ai_relay_definition",
            "game_creature_ai_relay_run",
            "game_creature_ai_relay_continuation",
        ] {
            assert!(
                !subscriptions.contains(&format!("SELECT * FROM {table}")),
                "Gateway subscribes Module-only table `{table}`"
            );
        }
    }

    #[test]
    fn quest_credit_reaches_quest_authority_without_writing_quest_tables() {
        let capability = include_str!("quest_credit.rs");
        assert!(capability.contains("eventai_credit_quest"));
        for table_write in [
            "game_character_quest()",
            "game_character_quest_event_credit()",
            ".insert(",
            ".update(",
            ".delete(",
        ] {
            assert!(
                !capability.contains(table_write),
                "quest-credit capability directly writes `{table_write}`"
            );
        }
    }

    #[test]
    fn event_producers_stop_at_named_core_chokepoints() {
        let swing = include_str!("../../combat/swing.rs");
        assert!(swing.contains("eventai_on_evade(ctx, creature)"));
        assert!(swing.contains("eventai_on_spell_hit("));

        let cycle = include_str!("../cycle/mod.rs");
        let context = include_str!("../cycle/ctx.rs");
        assert!(cycle.contains("w.reached_home(c.guid)"));
        assert!(context.contains("eventai_on_reached_home(self.ctx, guid)"));

        let chat = include_str!("../../chat.rs");
        assert!(chat.contains("eventai_on_receive_emote("));

        let spell = include_str!("../../spell/cast/resolve.rs");
        assert!(spell.contains("eventai_on_spell_hit("));

        let edges = include_str!("edges.rs");
        for producer in [
            "fn eventai_on_receive_ai_event(",
            "fn eventai_on_target_not_reachable(",
        ] {
            assert!(edges.contains(producer));
        }
    }

    #[test]
    fn every_first_creature_engagement_uses_the_shared_aggro_edge() {
        for source in [
            include_str!("../../combat/swing.rs"),
            include_str!("../../spell/effects.rs"),
            include_str!("../cycle/ctx.rs"),
            include_str!("engine.rs"),
            include_str!("mobility.rs"),
        ] {
            assert!(source.contains("arm_creature_engagement("));
        }
    }

    #[test]
    fn threat_actions_call_the_authority_without_table_writes() {
        let capability = include_str!("threat.rs");
        for direct_table_operation in ["game_threat", ".insert(", ".update(", ".delete("] {
            assert!(
                !capability.contains(direct_table_operation),
                "EventAI threat capability writes through `{direct_table_operation}`"
            );
        }
    }
}
