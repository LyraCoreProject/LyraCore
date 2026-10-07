#![cfg_attr(not(has_packages), allow(dead_code, unused_imports))]

//! Setup and observation steps for a Package's own debug fixtures. The root exists only with
//! `debug_reducers`, so a release build has none of it, and the Package API lint accepts it only in
//! a Package file gated on that feature.

use spacetimedb::{ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::creatures::{game_creature_move_schedule, GLOBAL_TICK_INSTANCE};
use crate::import_meta::game_import_meta;

/// The highest-threat living source on the creature's map and instance. Ties go to the lower guid.
pub use crate::threat::top_threat_target;

/// Damage a live, living target through the real Core damage pipeline as a main-hand hit from
/// `attacker_guid` (0 means anonymous). Damage is capped one below the target's health, so the
/// call never kills.
pub fn apply_damage(
    ctx: &ReducerContext,
    target_guid: u64,
    amount: u32,
    attacker_guid: u64,
) -> Result<(), String> {
    crate::debug::debug_apply_damage(ctx, target_guid, amount, attacker_guid)
}

/// Remove a live Character from the world as a logout does: `on_logout` fires, the Character keeps
/// its position and progress, and its combat, duel, trade and corpse end. The Character row stays.
pub fn remove_live_character(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    let entity = crate::helpers::live_entity(ctx, character_guid)?;
    if !entity.is_player() {
        return Err(format!("live entity {character_guid} is not a Character"));
    }
    crate::world::remove_live_character(ctx, entity);
    Ok(())
}

/// Refuse when the Shard holds imported content. The temporary weather seed a fresh Module stamps
/// is Core's own data, so it does not count.
pub fn require_no_imported_content(ctx: &ReducerContext) -> Result<(), String> {
    match ctx
        .db
        .game_import_meta()
        .iter()
        .find(|row| row.family != crate::weather::WEATHER_SEED_FAMILY)
    {
        Some(import) => Err(format!(
            "fixture refuses imported content ({})",
            import.family
        )),
        None => Ok(()),
    }
}

/// Cast through the Gates a client cast passes, as the Session Actor `caster_guid`.
pub fn client_cast(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    target_guid: u64,
) -> Result<(), String> {
    crate::gw::gw_cast_at(
        ctx,
        crate::SessionActor {
            guid: caster_guid,
            ownership: None,
        },
        spell_id,
        target_guid,
    )
}

/// Declare that the next creature movement tick fires once, `delay` from now. Refuses unless the
/// catch-all row is the only movement schedule, which holds on a fresh Shard. The row then fires
/// once, not on an interval.
pub fn declare_next_movement_tick(ctx: &ReducerContext, delay: TimeDuration) -> Result<(), String> {
    let schedules = ctx.db.game_creature_move_schedule();
    let mut rows = schedules.iter().take(2);
    let (Some(mut tick), None) = (rows.next(), rows.next()) else {
        return Err("fixture requires exactly one creature movement tick".to_string());
    };
    if tick.instance_id != GLOBAL_TICK_INSTANCE {
        return Err("fixture requires the catch-all creature movement tick".to_string());
    }
    let at = ctx
        .timestamp
        .checked_add(delay)
        .ok_or("fixture movement tick timestamp exhausted")?;
    tick.scheduled_at = ScheduleAt::Time(at);
    schedules.scheduled_id().update(tick);
    Ok(())
}
