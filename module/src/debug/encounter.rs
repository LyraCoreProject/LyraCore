//! Encounter kernel levers (work-item 228) — operator stand-ins until 227's Deadmines package
//! consumes the primitives for real. Each is a thin `?`-wrapper over the `crate::encounter` fn it
//! names, so the runbook can exercise every primitive on a live node without an encounter package.

use spacetimedb::{log, reducer, ReducerContext};

/// Register an HP-threshold watch (`encounter::watch_hp_threshold`). NOTE: a watch alone fires
/// nothing — `on_hp_threshold` handlers are compile-time registrations (`game_hook!` markers), so
/// without one anywhere in the build the kernel's damage probe stays on its zero-cost early-out.
#[reducer]
pub fn debug_encounter_watch_hp(ctx: &ReducerContext, entry: u32, pct: u8) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::encounter::watch_hp_threshold(ctx, entry, pct)
}

/// Set one encounter's state + payload (`encounter::set_encounter_state`/`set_encounter_data`) —
/// readable back via `spacetime sql "select * from game_encounter_state"`.
#[reducer]
pub fn debug_encounter_set_state(
    ctx: &ReducerContext,
    instance_id: u64,
    encounter_id: u32,
    state: u8,
    data: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::encounter::set_encounter_state(ctx, instance_id, encounter_id, state)?;
    crate::encounter::set_encounter_data(ctx, instance_id, encounter_id, data)?;
    Ok(())
}

/// Flip every DOOR/BUTTON of `go_entry` open (`encounter::open_door`); logs the flip count.
#[reducer]
pub fn debug_encounter_open_door(
    ctx: &ReducerContext,
    go_entry: u32,
    instance_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let opened = crate::encounter::open_door(ctx, go_entry, instance_id)?;
    log::info!("debug_encounter_open_door: opened {opened} gameobject(s) of entry {go_entry}");
    Ok(())
}

/// Spawn `count` adds of `entry` as one tracked wave (`encounter::spawn_wave`); logs the guids.
#[allow(clippy::too_many_arguments)] // a debug lever mirrors the primitive's full signature
#[reducer]
pub fn debug_encounter_spawn_wave(
    ctx: &ReducerContext,
    instance_id: u64,
    encounter_id: u32,
    map_id: u32,
    entry: u32,
    count: u32,
    x: f32,
    y: f32,
    z: f32,
    orientation: f32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let entries = vec![entry; count as usize];
    let guids = crate::encounter::spawn_wave(
        ctx,
        instance_id,
        encounter_id,
        map_id,
        &entries,
        x,
        y,
        z,
        orientation,
    );
    if guids.is_empty() {
        return Err(format!(
            "spawn_wave spawned nothing (no template for entry {entry}?)"
        ));
    }
    log::info!("debug_encounter_spawn_wave: spawned {guids:?} for encounter {encounter_id} in instance {instance_id}");
    Ok(())
}

/// Write a creature's virtual-item slots (`encounter::equip_swap`) — module-side row only; the
/// client-visible UNIT_VIRTUAL_ITEM_SLOT_DISPLAY relay is [V]-deferred (see game_encounter_equip).
#[reducer]
pub fn debug_encounter_equip(
    ctx: &ReducerContext,
    creature_guid: u64,
    main_hand: u32,
    off_hand: u32,
    ranged: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::encounter::equip_swap(ctx, creature_guid, main_hand, off_hand, ranged)
}

/// Send a creature on one move leg (`encounter::move_to_point`) — the Smite rack-run motion.
#[reducer]
pub fn debug_encounter_move(
    ctx: &ReducerContext,
    creature_guid: u64,
    x: f32,
    y: f32,
    z: f32,
    run: bool,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::encounter::move_to_point(ctx, creature_guid, x, y, z, run)
}

/// Reset one encounter (`encounter::encounter_reset`): state → NotStarted, tracked wave despawned.
/// HP fired-marks are entry-keyed, so clear them separately: `debug_encounter_reset_hp_fired`.
#[reducer]
pub fn debug_encounter_reset(
    ctx: &ReducerContext,
    instance_id: u64,
    encounter_id: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::encounter::encounter_reset(ctx, instance_id, encounter_id);
    Ok(())
}

/// Clear the per-instance HP fired-marks for `entry` (`encounter::reset_hp_fired`).
#[reducer]
pub fn debug_encounter_reset_hp_fired(
    ctx: &ReducerContext,
    instance_id: u64,
    entry: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::encounter::reset_hp_fired(ctx, instance_id, entry);
    Ok(())
}

/// Sweep EVERY kernel row for an instance (`encounter::sweep_encounter_state`) — kept as the
/// narrow kernel-only lever; the FULL instance reap (which calls this sweep as its 228 splice)
/// is `debug_reap_instance` (`debug/instance.rs`, 190 slice 3 landed).
#[reducer]
pub fn debug_sweep_encounter_state(ctx: &ReducerContext, instance_id: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::encounter::sweep_encounter_state(ctx, instance_id);
    Ok(())
}
