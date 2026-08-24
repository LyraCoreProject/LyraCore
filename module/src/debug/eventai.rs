//! EventAI production-boundary verifiers for standalone tests.

use spacetimedb::{reducer, ReducerContext};

use crate::{game_encounter_equip, game_world_entity};

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
