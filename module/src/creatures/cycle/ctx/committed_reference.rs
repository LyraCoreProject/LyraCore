use std::collections::BTreeSet;

use spacetimedb::sats::bsatn;
use spacetimedb::table::IndexAlgo;
use spacetimedb::{reducer, sys, table, ReducerContext, Table, Timestamp};
use spacetimedb_primitives::{IndexId, TableId};

use super::prototype_reference::{apply_request, benchmark_frame};
use crate::creatures::cycle::native_prototype::benchmark::{digest, workload};
use crate::creatures::cycle::native_prototype::commit::{Batch, GAMEPLAY_TABLES, SNAPSHOT_PREFIX};
use crate::creatures::cycle::native_prototype::{self as prototype, Action, Request};
use crate::tables::*;

#[table(accessor = game_native_benchmark_state, public)]
pub struct NativeBenchmarkState {
    #[primary_key]
    pub id: u8,
    pub run: u64,
    pub scenario: String,
    pub creatures: u32,
    pub native: bool,
    pub sequence: u64,
    pub at_micros: u64,
    pub last_commit: Vec<u8>,
}

#[reducer]
pub fn debug_native_committed_setup(
    ctx: &ReducerContext,
    scenario: String,
    creatures: u32,
    native: bool,
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
        native,
        sequence: 0,
        at_micros: case.start_micros,
        last_commit: Vec::new(),
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
    let mut state = progress(ctx, run, false)?;
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

#[reducer]
pub fn debug_native_committed_apply(ctx: &ReducerContext, payload: Vec<u8>) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if payload.len() > 32 * 1024 * 1024 {
        return Err("benchmark commit exceeds 32 MiB".into());
    }
    let mut remaining = payload.as_slice();
    let batch: Batch = bsatn::from_reader(&mut remaining).map_err(|e| e.to_string())?;
    if !remaining.is_empty() {
        return Err("benchmark commit has trailing bytes".into());
    }
    let mut state = progress(ctx, batch.run, true)?;
    let fingerprint = blake3::hash(&payload).as_bytes().to_vec();
    if batch.sequence == state.sequence {
        return if state.last_commit == fingerprint {
            Ok(())
        } else {
            Err("benchmark commit sequence was reused with different input".into())
        };
    }
    check_next(&state, batch.sequence, batch.at_micros)?;
    apply_changes(ctx, &batch)?;
    state.sequence = batch.sequence;
    state.at_micros = batch.at_micros;
    state.last_commit = fingerprint;
    ctx.db.game_native_benchmark_state().id().update(state);
    Ok(())
}

fn progress(ctx: &ReducerContext, run: u64, native: bool) -> Result<NativeBenchmarkState, String> {
    let state = ctx
        .db
        .game_native_benchmark_state()
        .id()
        .find(0)
        .ok_or("committed benchmark is not initialized")?;
    if state.run != run || state.native != native {
        return Err("benchmark run or execution mode is stale".into());
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

struct CommitTable {
    name: &'static str,
    table: TableId,
    primary: IndexId,
}

fn commit_table<T: Table>(_: &T) -> Result<CommitTable, String> {
    if T::PRIMARY_KEY != Some(0) || T::UNIQUE_COLUMNS.iter().any(|&column| column != 0) {
        return Err(format!(
            "benchmark table has unsupported keys: {}",
            T::TABLE_NAME
        ));
    }
    let index = T::INDEXES
        .iter()
        .find(|index| match index.algo {
            IndexAlgo::BTree { columns } | IndexAlgo::Hash { columns } => columns == [0],
            IndexAlgo::Direct { column } => column == 0,
        })
        .ok_or("benchmark primary index is missing")?;
    Ok(CommitTable {
        name: T::TABLE_NAME,
        table: T::table_id(),
        primary: sys::index_id_from_name(index.source_name).map_err(|e| e.to_string())?,
    })
}

// Each movement fixture table starts with a u64 primary key in its BSATN row.
fn row_key(row: &[u8]) -> Result<u64, String> {
    Ok(u64::from_le_bytes(
        row.get(..8)
            .ok_or("benchmark row has no primary key")?
            .try_into()
            .unwrap(),
    ))
}

fn apply_changes(ctx: &ReducerContext, batch: &Batch) -> Result<(), String> {
    let tables = [
        commit_table(ctx.db.game_world_entity())?,
        commit_table(ctx.db.game_creature_spline())?,
        commit_table(ctx.db.game_melee_attack())?,
        commit_table(ctx.db.game_creature_ai_movement_intent())?,
    ];
    let mut old_keys = BTreeSet::new();
    let mut new_keys = BTreeSet::new();
    let operations = batch
        .changes
        .iter()
        .map(|change| {
            let table = tables
                .iter()
                .position(|table| table.name == change.table)
                .ok_or_else(|| {
                    format!("table is outside the movement benchmark: {}", change.table)
                })?;
            let before = change.before.as_deref().map(row_key).transpose()?;
            let after = change.after.as_deref().map(row_key).transpose()?;
            if before.is_some_and(|key| !old_keys.insert((table, key)))
                || after.is_some_and(|key| !new_keys.insert((table, key)))
            {
                return Err("benchmark batch repeats a row".into());
            }
            Ok((table, before.is_some() && before == after))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut current = Vec::new();
    for (change, &(table, update)) in batch.changes.iter().zip(&operations) {
        if let Some(before) = &change.before {
            let table = &tables[table];
            if update {
                current.clear();
                let mut rows = sys::datastore_index_scan_point_bsatn(table.primary, &before[..8])
                    .map_err(|e| e.to_string())?;
                while !rows.is_exhausted() {
                    rows.read(&mut current);
                }
                if &current != before {
                    return Err(format!("benchmark old row changed: {}", change.table));
                }
                continue;
            }
            // BSATN arrays begin with the element count as a little-endian u32.
            let mut relation = Vec::with_capacity(4 + before.len());
            relation.extend_from_slice(&1u32.to_le_bytes());
            relation.extend_from_slice(before);
            if sys::datastore_delete_all_by_eq_bsatn(table.table, &relation)
                .map_err(|e| e.to_string())?
                != 1
            {
                return Err(format!("benchmark old row changed: {}", change.table));
            }
        }
    }
    for (change, &(table, update)) in batch.changes.iter().zip(&operations) {
        if let Some(after) = &change.after {
            let mut row = after.clone();
            let table = &tables[table];
            let generated = if update {
                sys::datastore_update_bsatn(table.table, table.primary, &mut row)
            } else {
                sys::datastore_insert_bsatn(table.table, &mut row)
            }
            .map_err(|e| e.to_string())?;
            if !generated.is_empty() {
                return Err("movement benchmark cannot generate durable keys".into());
            }
        }
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
