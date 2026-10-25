#![cfg_attr(not(has_packages), allow(dead_code, unused_imports))]

use spacetimedb::{ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::creatures::{game_creature_move_schedule, GLOBAL_TICK_INSTANCE};
use crate::import_meta::game_import_meta;

pub(crate) use crate::threat::top_threat_target;

pub(crate) use crate::account_ownership::SessionActor;
pub(crate) use crate::combat::{
    apply_hit, enter_combat, final_damage, fold_incoming_damage, kill_creature, Hit, HitSource,
};
pub(crate) use crate::creatures::{
    build_creature_entity, despawn_creature_entity, insert_creature_entity, timer_never,
};
pub(crate) use crate::group::sync_group_mirror;
pub(crate) use crate::items::{grant_item, remove_items};
pub(crate) use crate::loot::tag::clear as clear_loot_tag;
pub(crate) use crate::nav::record_change as record_navigation_change;
pub(crate) use crate::spell::{
    cast_triggered, learn_spell, start_creature_spell, CreatureSpellCasterAdmission,
    CreatureSpellStart, CreatureSpellStartMode, CreatureSpellTarget,
};
pub(crate) use crate::xp::grant_xp;

pub(crate) fn do_cancel_aura(
    ctx: &ReducerContext,
    player_guid: u64,
    spell_id: u32,
) -> Result<(), String> {
    crate::spell::do_cancel_aura(ctx, player_guid, spell_id).map_err(String::from)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_move_spline(
    ctx: &ReducerContext,
    guid: u64,
    start: (f32, f32, f32),
    destination: (f32, f32, f32),
    duration_ms: u32,
    run: bool,
    spline_id: u32,
    map_id: u32,
    instance_id: u64,
    grid: (i32, i32),
) {
    crate::creatures::tick::emit_move_spline(
        ctx,
        guid,
        start,
        destination,
        duration_ms,
        run,
        spline_id,
        map_id,
        instance_id,
        grid,
        None,
    );
}

pub fn apply_damage(
    ctx: &ReducerContext,
    target_guid: u64,
    amount: u32,
    attacker_guid: u64,
) -> Result<(), String> {
    crate::debug::debug_apply_damage(ctx, target_guid, amount, attacker_guid)
}

pub fn remove_live_character(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    let entity = crate::helpers::live_entity(ctx, character_guid)?;
    if !entity.is_player() {
        return Err(format!("live entity {character_guid} is not a Character"));
    }
    crate::world::remove_live_character(ctx, entity);
    Ok(())
}

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

pub fn client_cast(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    target_guid: u64,
) -> Result<(), String> {
    crate::gw::gw_cast_at(
        ctx,
        SessionActor {
            guid: caster_guid,
            ownership: None,
        },
        spell_id,
        target_guid,
    )
}

pub fn admit_to_instance(
    ctx: &ReducerContext,
    character_guid: u64,
    map_id: u32,
    instance_id: u64,
    party_id: u64,
    request_actor: SessionActor,
) -> Result<(), String> {
    crate::instance::ensure_instance(ctx, instance_id, map_id, party_id, request_actor)?;
    let admitted = crate::instance::resolve_or_create_instance(ctx, character_guid, map_id)?;
    if admitted != instance_id {
        return Err(format!(
            "fixture entry resolved instance {admitted}, not {instance_id}"
        ));
    }
    Ok(())
}

pub fn record_completed_transfer(
    ctx: &ReducerContext,
    character_guid: u64,
    map_id: u32,
    instance_id: u64,
) {
    crate::realm_core::record_shard(ctx, character_guid, map_id, instance_id);
}

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
