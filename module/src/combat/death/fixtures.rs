use super::*;
use crate::game_corpse_loot;
use spacetimedb::ScheduleAt;

#[spacetimedb::reducer]
pub fn debug_stage_lethal_damage_floor_fixture(
    ctx: &ReducerContext,
    creature_guid: u64,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let mut creature = entities
        .guid()
        .find(creature_guid)
        .ok_or_else(|| format!("fixture creature does not exist: {creature_guid}"))?;
    if creature.is_player() || creature.dead {
        return Err("fixture target must be a live creature".to_string());
    }
    creature.health = 10;
    creature.stance = 1;
    creature.update(ctx);
    let definition_revision = crate::creatures::current_definition_revision(ctx, creature_guid);
    if definition_revision.value == 0
        || !set_lethal_damage_floor(ctx, creature_guid, definition_revision.value, true)
    {
        return Err("could not enable the fixture lethal floor".to_string());
    }
    Ok(())
}

#[spacetimedb::reducer]
pub fn debug_set_lethal_damage_floor_fixture(
    ctx: &ReducerContext,
    creature_guid: u64,
    enabled: bool,
) -> Result<(), String> {
    let revision = crate::creatures::current_definition_revision(ctx, creature_guid);
    set_lethal_damage_floor(ctx, creature_guid, revision.value, enabled)
        .then_some(())
        .ok_or_else(|| "could not update the fixture lethal floor".to_string())
}

#[spacetimedb::reducer]
pub fn debug_replace_definition_then_apply_damage_floor_fixture(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    packed_definition: String,
    rolled: u32,
) -> Result<(), String> {
    crate::creatures::replace_definition_for_debug(ctx, &packed_definition)?;
    debug_apply_lethal_damage_floor_fixture(ctx, attacker_guid, target_guid, rolled)
}

#[spacetimedb::reducer]
pub fn debug_stage_ranged_lethal_damage_floor_fixture(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    damage: u32,
    delay_ms: u32,
) -> Result<(), String> {
    ctx.db
        .game_world_entity()
        .guid()
        .find(attacker_guid)
        .ok_or_else(|| "fixture ranged attacker does not exist".to_string())?;
    ctx.db
        .game_world_entity()
        .guid()
        .find(target_guid)
        .ok_or_else(|| "fixture ranged target does not exist".to_string())?;
    let land_at = crate::time::now(ctx)
        .checked_add(TimeDuration::from_micros(i64::from(delay_ms) * 1_000))
        .unwrap_or(crate::time::now(ctx));
    ctx.db
        .game_ranged_impact_schedule()
        .insert(crate::RangedImpactSchedule {
            scheduled_id: 0,
            scheduled_at: ScheduleAt::Time(land_at),
            attacker_guid,
            target_guid,
            damage,
            ranged_spell_id: 75,
            is_crit: false,
        });
    Ok(())
}

#[spacetimedb::reducer]
pub fn debug_set_lethal_damage_floor_health_fixture(
    ctx: &ReducerContext,
    creature_guid: u64,
    health: u32,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let mut creature = entities
        .guid()
        .find(creature_guid)
        .ok_or_else(|| "fixture creature does not exist".to_string())?;
    if creature.dead || health == 0 || health > creature.max_health {
        return Err("fixture health must keep the creature alive".to_string());
    }
    creature.health = health;
    creature.update(ctx);
    Ok(())
}

#[spacetimedb::reducer]
pub fn debug_apply_lethal_damage_floor_fixture(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    rolled: u32,
) -> Result<(), String> {
    let target = ctx
        .db
        .game_world_entity()
        .guid()
        .find(target_guid)
        .ok_or_else(|| format!("fixture target does not exist: {target_guid}"))?;
    if target.dead {
        return Err("fixture target is dead".to_string());
    }
    let (post_mitigation, _) = fold_incoming_damage(ctx, attacker_guid, target_guid, rolled);
    let damage = final_damage(ctx, target_guid, post_mitigation);
    apply_hit(
        ctx,
        attacker_guid,
        target_guid,
        damage,
        Hit::weapon(HitSource::MainHand, false),
    );
    Ok(())
}

#[spacetimedb::reducer]
pub fn debug_force_lethal_damage_floor_fixture(
    ctx: &ReducerContext,
    creature_guid: u64,
) -> Result<(), String> {
    force_creature_death(ctx, creature_guid)
        .then_some(())
        .ok_or_else(|| "fixture ForceDeath did not kill a live creature".to_string())
}

#[spacetimedb::reducer]
pub fn debug_respawn_lethal_damage_floor_fixture(
    ctx: &ReducerContext,
    creature_guid: u64,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    if !crate::WorldEntity::delete(ctx, creature_guid) {
        return Err("fixture creature does not exist".to_string());
    }
    let spawns = ctx.db.game_creature_spawn();
    let mut spawn = spawns
        .guid()
        .find(creature_guid)
        .ok_or_else(|| "fixture creature has no spawn".to_string())?;
    spawn.respawn_at = crate::time::now(ctx);
    spawns.guid().update(spawn);
    crate::creatures::tick::pass_respawn(ctx);
    entities
        .guid()
        .find(creature_guid)
        .filter(|creature| !creature.dead)
        .map(|_| ())
        .ok_or_else(|| "fixture creature did not respawn".to_string())
}

#[spacetimedb::reducer]
#[allow(clippy::too_many_arguments)]
pub fn debug_verify_lethal_damage_floor_fixture(
    ctx: &ReducerContext,
    creature_guid: u64,
    expected_health: u32,
    expected_dead: bool,
    expected_floor: bool,
    expected_notification: bool,
    expected_absorb: i32,
) -> Result<(), String> {
    let creature = ctx
        .db
        .game_world_entity()
        .guid()
        .find(creature_guid)
        .ok_or_else(|| "fixture creature is absent".to_string())?;
    if expected_health != u32::MAX && creature.health != expected_health {
        return Err(format!(
            "fixture health mismatch: actual={} expected={expected_health}",
            creature.health
        ));
    }
    if creature.dead != expected_dead {
        return Err(format!(
            "fixture death mismatch: actual={} expected={expected_dead}",
            creature.dead
        ));
    }
    if !expected_dead
        && ctx
            .db
            .game_corpse_loot()
            .by_corpse()
            .filter(&creature_guid)
            .next()
            .is_some()
    {
        return Err("a surviving fixture creature received corpse loot".to_string());
    }
    let floor = ctx
        .db
        .game_creature_lethal_damage_floor()
        .creature_guid()
        .find(creature_guid);
    if floor.is_some() != expected_floor
        || floor
            .as_ref()
            .is_some_and(|row| row.notification_sent != expected_notification)
    {
        return Err("fixture lethal-floor state mismatch".to_string());
    }
    let absorb = ctx
        .db
        .game_aura()
        .by_target()
        .filter(&creature_guid)
        .filter(|aura| aura.eff_kind == crate::spell::A_ABSORB)
        .map(|aura| aura.amount_remaining)
        .sum::<i32>();
    if absorb != expected_absorb {
        return Err(format!(
            "fixture absorb mismatch: actual={absorb} expected={expected_absorb}"
        ));
    }
    Ok(())
}
