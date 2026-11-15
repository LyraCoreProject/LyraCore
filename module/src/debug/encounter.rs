use spacetimedb::{log, reducer, ReducerContext};

#[reducer]
pub(crate) fn debug_encounter_watch_hp(
    ctx: &ReducerContext,
    entry: u32,
    pct: u8,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::encounter::watch_hp_threshold(ctx, entry, pct)
}

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

#[allow(clippy::too_many_arguments)]
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

#[reducer]
pub fn debug_encounter_equip(
    ctx: &ReducerContext,
    creature_guid: u64,
    main_hand: u32,
    off_hand: u32,
    ranged: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::creatures::presentation::equip_swap(ctx, creature_guid, main_hand, off_hand, ranged)
}

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
    crate::creatures::leg::move_to_point(ctx, creature_guid, x, y, z, run)
}

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

#[reducer]
pub fn debug_sweep_encounter_state(ctx: &ReducerContext, instance_id: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::encounter::sweep_encounter_state(ctx, instance_id);
    Ok(())
}
