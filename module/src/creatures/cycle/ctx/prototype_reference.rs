use super::*;
use crate::creatures::cycle::native_prototype::{
    self as prototype, Action, CREATURE_GUID, TARGET_GUID, TRACE_PREFIX,
};
use crate::creatures::eventai::movement::game_creature_ai_movement_intent;
use crate::creatures::eventai::{DefinitionRevision, ImmobilizationInstruction, MovementOperation};

#[spacetimedb::reducer]
pub fn debug_native_simulation_reference(
    ctx: &ReducerContext,
    scenario_name: String,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let scenario = prototype::scenario(&scenario_name)?;
    for guid in [CREATURE_GUID, TARGET_GUID] {
        if ctx.db.game_world_entity().guid().find(guid).is_some()
            || ctx
                .db
                .game_melee_attack()
                .attacker_guid()
                .find(guid)
                .is_some()
            || ctx
                .db
                .game_creature_ai_movement_intent()
                .creature_guid()
                .find(guid)
                .is_some()
        {
            return Err("prototype fixture identifiers are occupied".into());
        }
    }
    if ctx.db.game_creature_spline().iter().next().is_some() {
        return Err("the reference requires a disposable Shard without active splines".into());
    }
    for row in &scenario.entities {
        ctx.db.game_world_entity().insert(row.clone());
    }
    for row in &scenario.splines {
        ctx.db.game_creature_spline().insert(row.clone());
    }
    for row in &scenario.attacks {
        ctx.db.game_melee_attack().insert(row.clone());
    }
    emit_frame(ctx, scenario.name, 0, scenario.start_micros);
    for request in &scenario.requests {
        // Only this fixture uses a synthetic clock; table access still belongs to this transaction.
        let mut clock = ReducerContext::__dummy();
        clock.timestamp =
            spacetimedb::Timestamp::from_micros_since_unix_epoch(request.at_micros as i64);
        match request.action {
            Action::Tick => {
                advance(&clock, request.at_micros);
            }
            Action::MoveTarget {
                x,
                y,
                z,
                orientation,
                movement_flags,
            } => {
                let mut target = clock
                    .db
                    .game_world_entity()
                    .guid()
                    .find(TARGET_GUID)
                    .unwrap();
                target.x = x;
                target.y = y;
                target.z = z;
                target.orientation = orientation;
                target.movement_flags = movement_flags;
                target.last_move_ms = (request.at_micros / 1_000) as u32;
                (target.grid_x, target.grid_y) = spatial::grid_cell(x, y);
                target.cell = spatial::grid_cell_id(target.grid_x, target.grid_y);
                clock.db.game_world_entity().guid().update(target);
            }
            Action::Immobilize { enabled } => {
                eventai::movement::apply(
                    &clock,
                    CREATURE_GUID,
                    DefinitionRevision { value: 1 },
                    MovementOperation::SetImmobilized(ImmobilizationInstruction {
                        enabled,
                        combat_only: false,
                    }),
                );
            }
            Action::RemoveCreature => {
                clock.db.game_world_entity().guid().delete(CREATURE_GUID);
                clock
                    .db
                    .game_creature_ai_movement_intent()
                    .creature_guid()
                    .delete(CREATURE_GUID);
            }
        }
        emit_frame(&clock, scenario.name, request.sequence, request.at_micros);
    }
    for guid in [CREATURE_GUID, TARGET_GUID] {
        ctx.db.game_creature_spline().guid().delete(guid);
        ctx.db.game_melee_attack().attacker_guid().delete(guid);
        ctx.db
            .game_creature_ai_movement_intent()
            .creature_guid()
            .delete(guid);
        ctx.db.game_world_entity().guid().delete(guid);
    }
    Ok(())
}

fn advance(ctx: &ReducerContext, at_micros: u64) {
    let mut world = CtxWorld {
        ctx,
        regen_window: crate::combat::RegenWindow::new(at_micros / 1_000, 0),
        advances: Vec::new(),
    };
    let tick = prototype::tick_context(at_micros);
    super::super::advance_legs(&mut world, &tick);
    super::super::chase(&mut world, &tick);
    world.settle_advances();
}

#[spacetimedb::reducer]
pub fn debug_native_simulation_benchmark(
    ctx: &ReducerContext,
    scenario_name: String,
    creatures: u32,
    sample: u32,
    trace: bool,
) -> Result<(), String> {
    use prototype::benchmark::{digest, workload, RESULT_PREFIX, TIMER_PREFIX};

    crate::helpers::require_operator(ctx)?;
    let scenario = workload(&scenario_name, creatures)?;
    if trace && creatures > 4 {
        return Err("benchmark tracing is limited to four creatures".into());
    }
    if ctx.db.game_world_entity().iter().next().is_some()
        || ctx.db.game_creature_spline().iter().next().is_some()
        || ctx.db.game_melee_attack().iter().next().is_some()
        || ctx
            .db
            .game_creature_ai_movement_intent()
            .iter()
            .next()
            .is_some()
    {
        return Err("the benchmark requires an empty disposable Shard".into());
    }
    for row in &scenario.entities {
        ctx.db.game_world_entity().insert(row.clone());
    }
    for row in &scenario.splines {
        ctx.db.game_creature_spline().insert(row.clone());
    }
    for row in &scenario.attacks {
        ctx.db.game_melee_attack().insert(row.clone());
    }
    let timer_name = format!("{TIMER_PREFIX}/{scenario_name}/{creatures}/{sample}");
    let timer = (!trace).then(|| spacetimedb::log_stopwatch::LogStopwatch::new(&timer_name));
    for request in &scenario.requests {
        let mut clock = ReducerContext::__dummy();
        clock.timestamp =
            spacetimedb::Timestamp::from_micros_since_unix_epoch(request.at_micros as i64);
        advance(&clock, request.at_micros);
        if trace {
            spacetimedb::log::info!(
                "{RESULT_PREFIX}{}",
                serde_json::json!({"scenario": scenario_name, "creatures": creatures,
                    "frame": benchmark_frame(&clock, request)})
            );
        }
    }
    drop(timer);
    let last = scenario.requests.last().unwrap();
    let frame = benchmark_frame(ctx, last);
    spacetimedb::log::info!(
        "{RESULT_PREFIX}{}",
        serde_json::json!({"scenario": scenario_name, "creatures": creatures, "sample": sample,
            "digest": digest(&frame), "ticks": scenario.requests.len()})
    );
    for row in &scenario.entities {
        ctx.db.game_creature_spline().guid().delete(row.guid);
        ctx.db.game_melee_attack().attacker_guid().delete(row.guid);
        ctx.db.game_world_entity().guid().delete(row.guid);
    }
    Ok(())
}

fn benchmark_frame(ctx: &ReducerContext, request: &prototype::Request) -> serde_json::Value {
    prototype::snapshot(
        request.sequence,
        request.at_micros,
        ctx.db.game_world_entity().iter().collect(),
        ctx.db.game_creature_spline().iter().collect(),
        ctx.db.game_melee_attack().iter().collect(),
        Vec::new(),
    )
}

fn emit_frame(ctx: &ReducerContext, scenario: &str, sequence: u64, at_micros: u64) {
    let guids = [CREATURE_GUID, TARGET_GUID];
    let frame = prototype::snapshot(
        sequence,
        at_micros,
        guids
            .iter()
            .filter_map(|g| ctx.db.game_world_entity().guid().find(g))
            .collect(),
        guids
            .iter()
            .filter_map(|g| ctx.db.game_creature_spline().guid().find(g))
            .collect(),
        guids
            .iter()
            .filter_map(|g| ctx.db.game_melee_attack().attacker_guid().find(g))
            .collect(),
        guids
            .into_iter()
            .filter(|&g| eventai::movement::intent(ctx, g).is_some_and(|i| i.immobilized))
            .collect(),
    );
    spacetimedb::log::info!(
        "{TRACE_PREFIX}{}",
        serde_json::json!({"scenario": scenario, "frame": frame})
    );
}
