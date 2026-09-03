//! EventAI edge dispatch and lifecycle resets.

use std::collections::HashSet;

use spacetimedb::ReducerContext;
use spacetimedb::{table, Table};

use super::mobility::game_creature_ai_summon_origin;
use super::{EventAiRequest, EventCondition, EventContext, EventKind};
use crate::{
    game_creature_ai_rule_state, game_creature_ai_state, game_melee_attack, game_world_entity,
    WorldEntity,
};

/// Who runs its entry's EventAI at all. A Character carries no rules, and a pet answers its
/// owner's commands rather than the wild entry it was built from, so a tamed beast must neither
/// evaluate its entry's rules nor keep rule state of its own.
pub(crate) fn runs_eventai(creature: &WorldEntity) -> bool {
    !creature.is_player() && creature.owner_guid == 0
}

/// One unit whose engagement reset waits until the current creature-death hooks finish.
#[table(accessor = game_creature_ai_reset_deferral)]
pub struct CreatureAiResetDeferral {
    #[primary_key]
    #[unique]
    pub creature_guid: u64,
}

/// A creature whose evade return has not reached its spawn post.
#[table(accessor = game_creature_ai_returning_home)]
pub struct CreatureAiReturningHome {
    #[primary_key]
    #[unique]
    pub creature_guid: u64,
}

pub(crate) fn begin_death_dispatch(
    ctx: &ReducerContext,
    victim_guid: u64,
    killer_guid: Option<u64>,
) {
    let deferrals = ctx.db.game_creature_ai_reset_deferral();
    for creature_guid in Some(victim_guid).into_iter().chain(killer_guid) {
        if deferrals.creature_guid().find(creature_guid).is_none() {
            deferrals.insert(CreatureAiResetDeferral { creature_guid });
        }
    }
}

pub(crate) fn finish_death_dispatch(
    ctx: &ReducerContext,
    victim_guid: u64,
    killer_guid: Option<u64>,
) {
    let deferrals = ctx.db.game_creature_ai_reset_deferral();
    deferrals.creature_guid().delete(victim_guid);
    if let Some(killer_guid) = killer_guid {
        deferrals.creature_guid().delete(killer_guid);
        if !crate::combat::is_engaged(ctx, killer_guid) {
            reset_engagement(ctx, killer_guid);
        }
    }
}

crate::game_hook!(on_aggro, fn creature_ai_on_aggro(ctx, payload) {
    evaluate_edge(
        ctx,
        EventKind::OnAggro,
        payload.creature_guid,
        Some(payload.target_guid),
        None,
        Some(payload.target_guid),
        payload.assist,
    );
});

crate::game_hook!(on_creature_spawn, fn creature_ai_on_creature_spawn(ctx, payload) {
    let Some(creature) = ctx.db.game_world_entity().guid().find(payload.guid) else {
        return;
    };
    if creature.dead || !runs_eventai(&creature) {
        return;
    }
    reset_creature_lifecycle(ctx, payload.guid);
    evaluate_edge(
        ctx,
        EventKind::OnSpawn,
        payload.guid,
        None,
        None,
        None,
        false,
    );
});

crate::game_hook!(on_creature_death, fn creature_ai_on_creature_death(ctx, payload) {
    evaluate_edge(
        ctx,
        EventKind::OnDeath,
        payload.creature_guid,
        (payload.killer_guid != 0).then_some(payload.killer_guid),
        None,
        (payload.current_target_guid != 0).then_some(payload.current_target_guid),
        false,
    );
    super::cancel_relay_runs_for_source(ctx, payload.creature_guid);
    reset_creature_lifecycle(ctx, payload.creature_guid);
});

crate::game_hook!(on_death, fn creature_ai_on_unit_death(ctx, payload) {
    if payload.killer_guid != 0 {
        if let Some(killer) = ctx.db.game_world_entity().guid().find(payload.killer_guid) {
            if runs_eventai(&killer) {
                let mut context = edge_context(
                    ctx,
                    EventKind::OnKill,
                    killer.guid,
                    Some(payload.victim_guid),
                    None,
                    Some(payload.victim_guid),
                    false,
                );
                context.invoker_is_player = Some(payload.victim_is_player);
                context.engaged = true;
                super::evaluate_context(ctx, EventAiRequest::Edge(context));
            }
        }
    }

    let Some(origin) = ctx
        .db
        .game_creature_ai_summon_origin()
        .creature_guid()
        .find(payload.victim_guid)
    else {
        return;
    };
    let entry = ctx
        .db
        .game_world_entity()
        .guid()
        .find(payload.victim_guid)
        .map(|unit| unit.entry);
    let mut context = edge_context(
        ctx,
        EventKind::OnSummonedDeath,
        origin.summoner_guid,
        Some(payload.victim_guid),
        None,
        None,
        false,
    );
    context.creature_entry = entry;
    super::evaluate_context(ctx, EventAiRequest::Edge(context));
});

pub(crate) fn eventai_on_spell_hit(
    ctx: &ReducerContext,
    caster_guid: u64,
    target_guid: u64,
    spell_id: u32,
    school_mask: u32,
) {
    if let Some(target) = ctx.db.game_world_entity().guid().find(target_guid) {
        if runs_eventai(&target) {
            let mut context = edge_context(
                ctx,
                EventKind::OnSpellHit,
                target.guid,
                Some(caster_guid),
                None,
                (target.target_guid != 0).then_some(target.target_guid),
                false,
            );
            context.spell_id = Some(spell_id);
            context.spell_school_mask = school_mask;
            super::evaluate_context(ctx, EventAiRequest::Edge(context));
        }
    }
    if let Some(caster) = ctx.db.game_world_entity().guid().find(caster_guid) {
        if runs_eventai(&caster) {
            let mut context = edge_context(
                ctx,
                EventKind::OnSpellHitTarget,
                caster.guid,
                (target_guid != 0).then_some(target_guid),
                None,
                (caster.target_guid != 0).then_some(caster.target_guid),
                false,
            );
            context.spell_id = Some(spell_id);
            context.spell_school_mask = school_mask;
            super::evaluate_context(ctx, EventAiRequest::Edge(context));
        }
    }
}

pub(crate) fn eventai_on_receive_emote(
    ctx: &ReducerContext,
    creature_guid: u64,
    invoker_guid: u64,
    emote_id: u32,
) {
    let Some(creature) = ctx.db.game_world_entity().guid().find(creature_guid) else {
        return;
    };
    if creature.dead || !runs_eventai(&creature) {
        return;
    }
    let mut context = edge_context(
        ctx,
        EventKind::OnReceiveEmote,
        creature_guid,
        Some(invoker_guid),
        None,
        (creature.target_guid != 0).then_some(creature.target_guid),
        false,
    );
    context.emote_id = Some(emote_id);
    super::evaluate_context(ctx, EventAiRequest::Edge(context));
}

pub(crate) fn eventai_on_evade(ctx: &ReducerContext, creature_guid: u64) {
    let Some(creature) = ctx.db.game_world_entity().guid().find(creature_guid) else {
        return;
    };
    if creature.dead || !runs_eventai(&creature) {
        return;
    }
    evaluate_edge(
        ctx,
        EventKind::OnEvade,
        creature_guid,
        None,
        None,
        (creature.target_guid != 0).then_some(creature.target_guid),
        false,
    );
    let returning = ctx.db.game_creature_ai_returning_home();
    if returning.creature_guid().find(creature_guid).is_none() {
        returning.insert(CreatureAiReturningHome { creature_guid });
    }
}

pub(crate) fn eventai_on_reached_home(ctx: &ReducerContext, creature_guid: u64) {
    if !ctx
        .db
        .game_creature_ai_returning_home()
        .creature_guid()
        .delete(creature_guid)
    {
        return;
    }
    evaluate_edge(
        ctx,
        EventKind::OnReachedHome,
        creature_guid,
        None,
        None,
        None,
        false,
    );
}

pub(crate) fn eventai_on_receive_ai_event(
    ctx: &ReducerContext,
    creature_guid: u64,
    invoker_guid: Option<u64>,
    sender_guid: u64,
    kind: super::AiEventKind,
) {
    let current_target_guid = ctx
        .db
        .game_melee_attack()
        .attacker_guid()
        .find(creature_guid)
        .map(|fight| fight.target_guid);
    let mut context = edge_context(
        ctx,
        EventKind::OnReceiveAiEvent,
        creature_guid,
        invoker_guid,
        None,
        current_target_guid,
        false,
    );
    context.ai_sender_guid = Some(sender_guid);
    context.ai_event = Some(kind);
    super::evaluate_context(ctx, EventAiRequest::Edge(context));
}

pub(crate) fn send_relay_ai_event(
    ctx: &ReducerContext,
    source_guid: u64,
    invoker_guid: u64,
    kind: super::AiEventKind,
    radius_yd: u32,
) -> Result<(), String> {
    let source = ctx
        .db
        .game_world_entity()
        .guid()
        .find(source_guid)
        .filter(runs_eventai)
        .ok_or_else(|| format!("relay AI-event source {source_guid} is unavailable"))?;
    if ctx
        .db
        .game_world_entity()
        .guid()
        .find(invoker_guid)
        .is_none()
    {
        return Err(format!(
            "relay AI-event invoker {invoker_guid} is unavailable"
        ));
    }
    let recipients = if radius_yd == 0 {
        vec![source_guid]
    } else {
        let radius = radius_yd as f32;
        let radius_sq = radius * radius;
        let mut guids = crate::helpers::entities_near(
            ctx,
            source.map_id,
            source.instance_id,
            source.x,
            source.y,
            radius,
        )
        .into_iter()
        .filter(|candidate| {
            runs_eventai(candidate)
                && !candidate.dead
                && (candidate.x - source.x).powi(2)
                    + (candidate.y - source.y).powi(2)
                    + (candidate.z - source.z).powi(2)
                    <= radius_sq
        })
        .map(|candidate| candidate.guid)
        .collect::<Vec<_>>();
        guids.sort_unstable();
        guids.dedup();
        guids
    };
    for recipient in recipients {
        eventai_on_receive_ai_event(ctx, recipient, Some(invoker_guid), source_guid, kind);
    }
    Ok(())
}

pub(crate) fn eventai_on_target_not_reachable(ctx: &ReducerContext, creature_guid: u64) {
    let current_target_guid = ctx
        .db
        .game_melee_attack()
        .attacker_guid()
        .find(creature_guid)
        .map(|fight| fight.target_guid);
    evaluate_edge(
        ctx,
        EventKind::TargetNotReachable,
        creature_guid,
        None,
        None,
        current_target_guid,
        false,
    );
}

pub(crate) fn eventai_on_summoned(
    ctx: &ReducerContext,
    summoner_guid: u64,
    summon_guid: u64,
    summon_entry: u32,
) {
    let mut context = edge_context(
        ctx,
        EventKind::OnSummoned,
        summoner_guid,
        Some(summon_guid),
        None,
        None,
        false,
    );
    context.creature_entry = Some(summon_entry);
    super::evaluate_context(ctx, EventAiRequest::Edge(context));
}

/// Start the creature's next engagement: bump `engagement_id`, drop the phase and ranged posture,
/// and clear the rule state every engagement rule earned in the fight that just ended. Once-only
/// aggro rules re-arm and timed rules wait their initial window again on the next pull. Spawn and
/// death rules keep their state, moved onto the new engagement, because they belong to the
/// lifecycle instead. Called however a fight ends, so no path can leave a creature half-armed.
pub(crate) fn reset_engagement(ctx: &ReducerContext, creature_guid: u64) {
    if ctx
        .db
        .game_creature_ai_reset_deferral()
        .creature_guid()
        .find(creature_guid)
        .is_some()
    {
        return;
    }
    let Some(creature) = ctx.db.game_world_entity().guid().find(creature_guid) else {
        return;
    };
    crate::creatures::presentation::clear_relay_temporary_faction(ctx, creature_guid);
    if !runs_eventai(&creature) {
        return;
    }
    super::movement::reset_engagement(ctx, creature_guid);

    let definition = super::combat::definition_for(ctx, creature_guid);
    let known_rule_ids: HashSet<u64> = definition
        .rules
        .iter()
        .map(|rule| rule.source_rule_id)
        .collect();
    let engagement_rule_ids: HashSet<u64> = definition
        .rules
        .iter()
        .filter(|rule| {
            !matches!(
                rule.event,
                EventCondition::OnDeath(_)
                    | EventCondition::OnSpawn(_)
                    | EventCondition::TimedGeneric(_)
            )
        })
        .map(|rule| rule.source_rule_id)
        .collect();
    let states = ctx.db.game_creature_ai_state();
    let engagement_id = states.creature_guid().find(creature_guid).map(|mut state| {
        state.engagement_id = state.engagement_id.saturating_add(1);
        state.phase = 0;
        state.ranged_distance = 0.0;
        state.ranged_angle = 0.0;
        state.ranged_posture_active = false;
        let state = states.creature_guid().update(state);
        state.engagement_id
    });
    let rule_state = ctx.db.game_creature_ai_rule_state();
    for mut state in rule_state
        .by_creature()
        .filter(&creature_guid)
        .collect::<Vec<_>>()
    {
        if !known_rule_ids.contains(&state.source_rule_id)
            || engagement_rule_ids.contains(&state.source_rule_id)
        {
            rule_state.id().delete(state.id);
        } else if let Some(engagement_id) = engagement_id {
            state.engagement_id = engagement_id;
            rule_state.id().update(state);
        }
    }
}

fn evaluate_edge(
    ctx: &ReducerContext,
    kind: EventKind,
    creature_guid: u64,
    invoker_guid: Option<u64>,
    event_target_guid: Option<u64>,
    current_target_guid: Option<u64>,
    assisted: bool,
) {
    super::evaluate_context(
        ctx,
        EventAiRequest::Edge(edge_context(
            ctx,
            kind,
            creature_guid,
            invoker_guid,
            event_target_guid,
            current_target_guid,
            assisted,
        )),
    );
}

fn edge_context(
    ctx: &ReducerContext,
    kind: EventKind,
    creature_guid: u64,
    invoker_guid: Option<u64>,
    event_target_guid: Option<u64>,
    current_target_guid: Option<u64>,
    assisted: bool,
) -> EventContext {
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64;
    EventContext {
        invoker_guid,
        invoker_is_player: invoker_guid.and_then(|guid| {
            ctx.db
                .game_world_entity()
                .guid()
                .find(guid)
                .map(|invoker| invoker.is_player())
        }),
        beneficiary_guid: invoker_guid.and_then(|guid| beneficiary_guid(ctx, guid)),
        spawner_guid: ctx
            .db
            .game_creature_ai_summon_origin()
            .creature_guid()
            .find(creature_guid)
            .map(|origin| origin.summoner_guid),
        event_target_guid,
        current_target_guid,
        assisted,
        ..EventContext::empty(kind, creature_guid, now_ms)
    }
}

fn beneficiary_guid(ctx: &ReducerContext, guid: u64) -> Option<u64> {
    let invoker = ctx.db.game_world_entity().guid().find(guid)?;
    (invoker.owner_guid != 0
        && ctx
            .db
            .game_world_entity()
            .guid()
            .find(invoker.owner_guid)
            .is_some())
    .then_some(invoker.owner_guid)
    .or(Some(invoker.guid))
}

pub(crate) fn reset_creature_lifecycle(ctx: &ReducerContext, creature_guid: u64) {
    crate::creatures::presentation::clear_eventai_presentation(ctx, creature_guid);
    super::movement::drop_lifecycle(ctx, creature_guid);
    super::mobility::drop_forced_despawn(ctx, creature_guid);
    ctx.db
        .game_creature_ai_returning_home()
        .creature_guid()
        .delete(creature_guid);
    let rule_state = ctx.db.game_creature_ai_rule_state();
    for state in rule_state
        .by_creature()
        .filter(&creature_guid)
        .collect::<Vec<_>>()
    {
        rule_state.id().delete(state.id);
    }

    ctx.db
        .game_creature_ai_state()
        .creature_guid()
        .delete(creature_guid);
}

#[cfg(test)]
mod eventai_gate_tripwire {
    use crate::test_scan::code_of;

    /// `CreatureAiState.lifecycle_id` is compared in `engine::rule_state_for` and in
    /// `presentation::apply_eventai_instruction` to decide whether stored state belongs to the
    /// creature standing here now. Both comparisons are vacuous unless the field is seeded from the
    /// spawn point's life counter: it once held a hardcoded 1 everywhere, so every comparison read
    /// `1 == 1` and passed. That failure is invisible — the code reads correctly and guards nothing.
    #[test]
    fn creature_state_seeds_its_lifecycle_from_the_spawn_points_life_counter() {
        let engine = include_str!("engine.rs");
        let mobility = include_str!("mobility.rs");
        for (source, signature) in [
            (
                engine,
                "fn set_eventai_phase(&mut self, creature_guid: u64, phase: u8) {",
            ),
            (
                engine,
                "fn adopt_eventai_revision(\n        &mut self,\n        creature_guid: u64,\n        revision: DefinitionRevision,\n    ) -> CreatureState {",
            ),
            (mobility, "fn default_state("),
        ] {
            let body = code_of(source, signature);
            assert!(
                body.contains("lifecycle_id: crate::creatures::current_life_seq("),
                "`{signature}` no longer seeds `lifecycle_id` from the spawn point's life counter,                  so every lifecycle comparison silently passes. Body was:\n{body}"
            );
        }
    }

    /// A summon guid repeats, because the summon sequence wraps. The summon's life number therefore
    /// has to survive every lifetime check: `expire_eventai_summon` re-inserts its row and `auto_inc`
    /// hands out a fresh `scheduled_id` each time, so the number must be CARRIED, never re-taken.
    /// Re-taking it would refuse a Relay Run on the next check of a perfectly healthy summon.
    #[test]
    fn a_summon_keeps_one_life_number_across_its_lifetime_checks() {
        let mobility = include_str!("mobility.rs");
        let place = code_of(mobility, "pub(super) fn place_summon(");
        assert!(
            place.contains("expiry.life_seq = sequence;"),
            "`place_summon` no longer pins the claim's own id as the summon's life number, so the \
             summon has no identity to Gate on. Body was:\n{place}"
        );
        let expire = code_of(mobility, "pub fn expire_eventai_summon(");
        assert!(
            expire.contains("life_seq: expiry.life_seq,"),
            "`expire_eventai_summon` no longer carries the summon's life number across its \
             re-insert, so a healthy summon changes identity on every lifetime check and its Relay \
             Runs are refused. Body was:\n{expire}"
        );
    }

    /// The three Gates below decide WHICH creature EventAI touches and WHERE its speech leaves the
    /// Module. Each reads durable state through a `ReducerContext`, which this crate has no test
    /// harness for, and each fails silently when it is dropped: a tamed beast quietly runs its wild
    /// entry's rules and never clears its rule state, and a creature line quietly skips the
    /// dead-speaker Gate and the length cap that every other spoken line goes through.
    #[test]
    fn every_eventai_entry_point_keeps_its_gate() {
        for (source, signature, needle, why) in [
            (
                include_str!("engine.rs"),
                "fn eventai_fights(&self, scope: &TickScope) -> Vec<EngagedFight> {",
                "super::runs_eventai(&creature)",
                "a pet must produce no evaluation context of its own",
            ),
            (
                include_str!("combat.rs"),
                "pub(super) fn definition_for(",
                "super::runs_eventai(&creature)",
                "the one definition fetch answers `who runs EventAI` for the engine, the edges and the \
                 cycle's authored-combat reads alike",
            ),
            (
                include_str!("edges.rs"),
                "pub(crate) fn reset_engagement(",
                "runs_eventai(&creature)",
                "a Character and a pet carry no engagement to reset",
            ),
            (
                include_str!("engine.rs"),
                "fn eventai_definition(&self, creature_guid: u64) -> EventAiDefinition {",
                "super::combat::definition_for(",
                "one entry-and-guid definition fetch, not a second copy that drifts from the gated one",
            ),
            (
                include_str!("engine.rs"),
                concat!(
                    "fn eventai_deliver_line(\n",
                    "        &mut self,\n",
                    "        speaker_guid: u64,\n",
                    "        target_guid: u64,\n",
                    "        chat_type: u8,\n",
                    "        language: u8,\n",
                    "        message: String,\n",
                    "    ) -> bool {",
                ),
                "crate::chat::apply_send_chat_to(",
                "the say/yell chokepoint owns the dead-speaker Gate and the length cap",
            ),
        ] {
            assert!(
                code_of(source, signature).contains(needle),
                "`{signature}` no longer contains `{needle}` — {why}"
            );
        }
    }
}
