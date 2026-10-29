use spacetimedb::ReducerContext;

use crate::creatures::eventai::game_creature_ai_summon_expiry;
use crate::spell::{CastGateRefusal, Spell, SpellEffect, P_ENTRY, T_SELF};
use crate::{game_creature_template, game_world_entity, WorldEntity};

pub(crate) fn is_guardian(ctx: &ReducerContext, guid: u64) -> bool {
    faction(ctx, guid).is_some()
}

pub(crate) fn faction(ctx: &ReducerContext, guid: u64) -> Option<u32> {
    ctx.db
        .game_creature_ai_summon_expiry()
        .creature_guid()
        .find(guid)
        .and_then(|expiry| expiry.guardian_faction)
}

pub(crate) fn check(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    effect: &SpellEffect,
) -> Result<(), CastGateRefusal> {
    if caster.is_player()
        || effect.p0_kind != P_ENTRY
        || effect.p0 <= 0
        || effect.radius_yd != 0.0
        || effect.per_level != 0.0
        || !(0..=1).contains(&effect.die_sides)
        || effect.target != T_SELF
        || effect.p1 != 0
    {
        return Err("unsupported guardian summon definition".to_string().into());
    }
    if ctx
        .db
        .game_creature_template()
        .entry()
        .find(effect.p0 as u32)
        .is_none()
    {
        return Err(format!("guardian template {} is missing", effect.p0).into());
    }
    Ok(())
}

pub(crate) fn summon(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell: &Spell,
    effect: &SpellEffect,
    points: i32,
) -> Result<(), String> {
    let caster = crate::helpers::live_entity(ctx, caster_guid)?;
    check(ctx, &caster, effect)?;
    let count = ctx
        .db
        .game_world_entity()
        .by_owner_guid()
        .filter(caster_guid)
        .filter(|entity| entity.entry == effect.p0 as u32 && is_guardian(ctx, entity.guid))
        .count();
    if count > 15 {
        return Ok(());
    }
    let summons = points.max(1) as usize;
    let level_roll = ctx.random();
    for index in 0..summons {
        super::eventai::place_guardian(
            ctx,
            &caster,
            effect.p0 as u32,
            super::SummonLocation {
                x: caster.x,
                y: caster.y,
                z: caster.z,
                orientation: if index == 0 {
                    -caster.orientation
                } else {
                    caster.orientation
                },
                lifetime_ms: spell.duration_ms,
            },
            level_roll,
        )?;
    }
    Ok(())
}

pub(crate) fn die(ctx: &ReducerContext, guid: u64) -> bool {
    let Some(mut entity) = ctx.db.game_world_entity().guid().find(guid) else {
        return false;
    };
    let expiries = ctx.db.game_creature_ai_summon_expiry();
    let Some(mut expiry) = expiries
        .creature_guid()
        .find(entity.guid)
        .filter(|row| row.guardian_faction.is_some())
    else {
        return false;
    };
    super::tick::stop_where_rendered(ctx, &mut entity);
    entity.health = 0;
    entity.dead = true;
    entity.dynamic_flags = 0;
    entity = entity.update(ctx);
    expiry.remaining_ms = 15_000;
    expiry.last_checked_ms = (crate::time::now(ctx).to_micros_since_unix_epoch() / 1000) as u64;
    expiries.scheduled_id().update(expiry);
    crate::spell::break_channel(ctx, entity.guid);
    crate::spell::remove_auras_on_death(ctx, entity.guid);
    true
}
