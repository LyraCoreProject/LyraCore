use spacetimedb::{reducer, sys, table, ReducerContext, Table, Timestamp};

use super::prototype_reference::{apply_request, benchmark_frame};
use crate::creatures::cycle::native_prototype::benchmark::{digest, workload};
use crate::creatures::cycle::native_prototype::commit::{GAMEPLAY_TABLES, SNAPSHOT_PREFIX};
use crate::creatures::cycle::native_prototype::{self as prototype, Action, Request};
use crate::tables::*;

#[table(accessor = game_native_benchmark_state, public)]
pub struct NativeBenchmarkState {
    #[primary_key]
    pub id: u8,
    pub run: u64,
    pub scenario: String,
    pub creatures: u32,
    pub sequence: u64,
    pub at_micros: u64,
}

#[reducer]
pub fn debug_native_committed_setup(
    ctx: &ReducerContext,
    scenario: String,
    creatures: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let case = workload(&scenario, creatures)?;
    let previous = ctx.db.game_native_benchmark_state().id().find(0);
    let run = previous
        .as_ref()
        .map_or(Some(1), |state| state.run.checked_add(1))
        .ok_or("benchmark run counter exhausted")?;
    if let Some(previous) = &previous {
        for row in workload(&previous.scenario, previous.creatures)?.entities {
            ctx.db.game_world_entity().guid().delete(row.guid);
            ctx.db.game_creature_spline().guid().delete(row.guid);
            ctx.db.game_melee_attack().attacker_guid().delete(row.guid);
            ctx.db
                .game_creature_ai_movement_intent()
                .creature_guid()
                .delete(row.guid);
        }
    }
    for &name in GAMEPLAY_TABLES {
        let id = sys::table_id_from_name(name).map_err(|e| e.to_string())?;
        if sys::datastore_table_row_count(id).map_err(|e| e.to_string())? != 0 {
            return Err("committed benchmark requires an empty disposable Shard".into());
        }
    }
    for &name in SCHEDULE_TABLES {
        let id = sys::table_id_from_name(name).map_err(|e| format!("{name}: {e}"))?;
        sys::datastore_clear(id).map_err(|e| format!("{name}: {e}"))?;
    }
    for row in case.entities {
        ctx.db.game_world_entity().insert(row);
    }
    for row in case.splines {
        ctx.db.game_creature_spline().insert(row);
    }
    for row in case.attacks {
        ctx.db.game_melee_attack().insert(row);
    }
    let state = NativeBenchmarkState {
        id: 0,
        run,
        scenario,
        creatures,
        sequence: 0,
        at_micros: case.start_micros,
    };
    if previous.is_some() {
        ctx.db.game_native_benchmark_state().id().update(state);
    } else {
        ctx.db.game_native_benchmark_state().insert(state);
    }
    Ok(())
}

#[reducer]
pub fn debug_native_committed_step(
    ctx: &ReducerContext,
    run: u64,
    sequence: u64,
    at_micros: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let mut state = progress(ctx, run)?;
    check_next(&state, sequence, at_micros)?;
    let mut clock = ReducerContext::__dummy();
    clock.timestamp = Timestamp::from_micros_since_unix_epoch(at_micros as i64);
    apply_request(
        &clock,
        &Request {
            sequence,
            at_micros,
            action: Action::Tick,
        },
    );
    state.sequence = sequence;
    state.at_micros = at_micros;
    ctx.db.game_native_benchmark_state().id().update(state);
    Ok(())
}

#[reducer]
pub fn debug_native_database_fault(ctx: &ReducerContext, trap: bool) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let mut state = ctx
        .db
        .game_native_benchmark_state()
        .id()
        .find(0)
        .ok_or("benchmark is not initialized")?;
    state.sequence += 1;
    ctx.db.game_native_benchmark_state().id().update(state);
    if trap {
        panic!("native database rollback fixture");
    }
    Err("native database rollback fixture".into())
}

fn progress(ctx: &ReducerContext, run: u64) -> Result<NativeBenchmarkState, String> {
    let state = ctx
        .db
        .game_native_benchmark_state()
        .id()
        .find(0)
        .ok_or("committed benchmark is not initialized")?;
    if state.run != run {
        return Err("benchmark run is stale".into());
    }
    Ok(state)
}

fn check_next(state: &NativeBenchmarkState, sequence: u64, at_micros: u64) -> Result<(), String> {
    if state.sequence.checked_add(1) != Some(sequence) {
        return Err("benchmark commits must arrive in sequence".into());
    }
    let case = prototype::scenario(&state.scenario)?;
    let request = case
        .requests
        .iter()
        .find(|request| request.sequence == sequence)
        .ok_or("benchmark has no such tick")?;
    if request.at_micros != at_micros {
        return Err("benchmark tick time does not match its input".into());
    }
    Ok(())
}

#[reducer]
pub fn debug_native_committed_snapshot(
    ctx: &ReducerContext,
    trace: bool,
    nonce: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let state = ctx
        .db
        .game_native_benchmark_state()
        .id()
        .find(0)
        .ok_or("committed benchmark is not initialized")?;
    if trace && state.creatures > 4 {
        return Err("committed traces are limited to four creatures".into());
    }
    let frame = benchmark_frame(
        ctx,
        &Request {
            sequence: state.sequence,
            at_micros: state.at_micros,
            action: Action::Tick,
        },
    );
    spacetimedb::log::info!(
        "{SNAPSHOT_PREFIX}{}",
        serde_json::json!({
            "run": state.run, "sequence": state.sequence, "nonce": nonce, "digest": digest(&frame),
            "frame": trace.then_some(frame),
        })
    );
    Ok(())
}

const SCHEDULE_TABLES: &[&str] = &[
    "game_event_reaper_schedule",
    "game_motion_publish_schedule",
    "game_mail_timer",
    "game_transfer_reaper_schedule",
    "game_meeting_stone_reminder_schedule",
    "game_aura_schedule",
    "game_ground_area_schedule",
    "game_pending_cast",
    "game_pending_spell_impact",
    "game_breath_schedule",
    "game_session_reaper_schedule",
    "game_taxi_flight_schedule",
    "game_gateway_lease_reaper_schedule",
    "game_weather_schedule",
    "game_duel_schedule",
    "game_auction_expiry",
    "game_mail_escrow_reaper_schedule",
    "game_instance_reaper_schedule",
    "game_instance_removal",
    "game_melee_schedule",
    "game_ranged_impact_schedule",
    "game_pet_care_schedule",
    "game_creature_move_schedule",
    "game_creature_ai_relay_continuation",
    "game_creature_ai_relay_arrival",
    "game_creature_ai_summon_expiry",
    "game_creature_ai_forced_despawn",
];
