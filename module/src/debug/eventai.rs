//! EventAI production-boundary verifiers for standalone tests.

use spacetimedb::{reducer, ReducerContext, Table};

use crate::{
    game_creature_ai_relay_arrival, game_creature_spline, game_encounter_equip, game_world_entity,
};

const FIXTURE_OWNER_ENTRY: u32 = 51_000;
const FIXTURE_OWNER_GUID: u64 = (0xF130_u64 << 48) | ((FIXTURE_OWNER_ENTRY as u64) << 24) | 1;

/// Prove action 56 reaches spell-created guardians, which have an owner but no EventAI summon row.
#[reducer]
pub fn debug_verify_eventai_spell_guardian_cleanup(ctx: &ReducerContext) -> Result<(), String> {
    crate::creatures::replace_relay_catalogue_for_debug(ctx, "")?;
    let owner = ctx
        .db
        .game_world_entity()
        .guid()
        .find(FIXTURE_OWNER_GUID)
        .ok_or_else(|| "fixture EventAI owner is unavailable".to_string())?;

    let ordinary_summon = crate::encounter::spawn_wave(
        ctx,
        owner.instance_id,
        90_001,
        owner.map_id,
        &[FIXTURE_OWNER_ENTRY],
        owner.x + 2.0,
        owner.y,
        owner.z,
        owner.orientation,
    )
    .into_iter()
    .next()
    .ok_or_else(|| "ordinary EventAI summon fixture was not materialized".to_string())?;
    crate::creatures::mark_summon_origin_for_debug(ctx, ordinary_summon, FIXTURE_OWNER_GUID);

    crate::creatures::summon_pet(ctx, FIXTURE_OWNER_GUID, FIXTURE_OWNER_ENTRY);
    let guardian = crate::creatures::pet_of(ctx, FIXTURE_OWNER_GUID)
        .ok_or_else(|| "spell-created guardian was not materialized".to_string())?;
    crate::creatures::remove_guardians(ctx, FIXTURE_OWNER_GUID, 0)?;
    if crate::creatures::pet_of(ctx, FIXTURE_OWNER_GUID).is_some()
        || ctx
            .db
            .game_world_entity()
            .guid()
            .find(guardian.guid)
            .is_some()
    {
        return Err("action 56 left the spell-created guardian live".to_string());
    }
    if ctx
        .db
        .game_world_entity()
        .guid()
        .find(ordinary_summon)
        .is_none()
    {
        return Err("action 56 removed an ordinary EventAI summon".to_string());
    }

    let missing =
        crate::creatures::replace_single_relay_for_debug(ctx, 90_001, "set-equipment:0:999999:0:0")
            .expect_err("a missing relay equipment item must refuse the catalogue");
    if !missing.contains("item_template:999999 is missing") {
        return Err(format!("unexpected missing-item refusal: {missing}"));
    }

    let catalogue_version =
        crate::creatures::replace_single_relay_for_debug(ctx, 90_001, "set-equipment:0:50:0:0")?;
    crate::creatures::start_imported_relay(
        ctx,
        90_001,
        FIXTURE_OWNER_GUID,
        FIXTURE_OWNER_GUID,
        1,
        catalogue_version,
    )?;
    let equipment = ctx
        .db
        .game_encounter_equip()
        .creature_guid()
        .find(FIXTURE_OWNER_GUID)
        .ok_or_else(|| "item-backed relay did not project equipment".to_string())?;
    if (equipment.main_hand, equipment.off_hand, equipment.ranged) != (1_542, 0, 0) {
        return Err(format!(
            "item-backed relay projected unexpected displays: ({}, {}, {})",
            equipment.main_hand, equipment.off_hand, equipment.ranged
        ));
    }
    Ok(())
}

/// Create a replacement summon, deliver its prior life's callback, and leave a short-lived
/// disengaged summon for the real scheduler to expire.
#[reducer]
pub fn debug_verify_eventai_summon_expiry(ctx: &ReducerContext) -> Result<(), String> {
    let owner = ctx
        .db
        .game_world_entity()
        .guid()
        .find(FIXTURE_OWNER_GUID)
        .ok_or_else(|| "fixture EventAI owner is unavailable".to_string())?;
    crate::creatures::verify_summon_expiry_boundaries_for_debug(ctx, &owner, FIXTURE_OWNER_ENTRY)
}

/// Start a relay that runs `source_guid` to `selected_guid` and, once the leg lands, equips item 50
/// through an arrival relay. The test reads the equipment row to see whether the arrival fired.
/// It replaces the entire relay catalogue, so never call it on a live realm.
#[reducer]
pub fn debug_start_relay_move(
    ctx: &ReducerContext,
    source_guid: u64,
    selected_guid: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    start_relay_move(ctx, source_guid, selected_guid)
}

/// [`debug_start_relay_move`], then run its arrival at once. The arrival must put the mover on the
/// destination, reap the leg and run the arrival relay. It replaces the entire relay catalogue, so
/// never call it on a live realm.
#[reducer]
pub fn debug_verify_relay_arrival_placement(
    ctx: &ReducerContext,
    source_guid: u64,
    selected_guid: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    start_relay_move(ctx, source_guid, selected_guid)?;
    let arrival = ctx
        .db
        .game_creature_ai_relay_arrival()
        .iter()
        .find(|arrival| arrival.source_guid == source_guid)
        .ok_or_else(|| "the relay scheduled no arrival".to_string())?;
    ctx.db
        .game_creature_ai_relay_arrival()
        .scheduled_id()
        .delete(arrival.scheduled_id);
    let destination = (arrival.x, arrival.y, arrival.z);
    crate::creatures::run_relay_arrival(ctx, arrival);

    let mover = ctx
        .db
        .game_world_entity()
        .guid()
        .find(source_guid)
        .ok_or_else(|| "the mover is gone".to_string())?;
    if (mover.x, mover.y, mover.z) != destination {
        return Err(format!(
            "the arrival left the mover at ({}, {}, {}), not at {destination:?}",
            mover.x, mover.y, mover.z
        ));
    }
    if ctx
        .db
        .game_creature_spline()
        .guid()
        .find(source_guid)
        .is_some()
    {
        return Err("the arrival left the landed leg in place".to_string());
    }
    if ctx
        .db
        .game_encounter_equip()
        .creature_guid()
        .find(source_guid)
        .is_none()
    {
        return Err("the arrival relay did not run".to_string());
    }
    Ok(())
}

fn start_relay_move(
    ctx: &ReducerContext,
    source_guid: u64,
    selected_guid: u64,
) -> Result<(), String> {
    let catalogue_version = crate::creatures::replace_relays_for_debug(
        ctx,
        &[
            (90_002, "source>selected", "move-dynamic:0:0:0:run:90003"),
            (90_003, "source>source", "set-equipment:0:50:0:0"),
        ],
    )?;
    crate::creatures::start_imported_relay(
        ctx,
        90_002,
        source_guid,
        selected_guid,
        1,
        catalogue_version,
    )?;
    Ok(())
}
