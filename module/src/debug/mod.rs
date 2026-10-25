mod audit;
mod auth;
mod encounter;
mod eventai;
mod fingerprint;
mod instance;
mod mail;
mod meeting_stone;
mod package_config;
mod readout;
mod runtime_script;

pub use fingerprint::{game_catalogue_fingerprint, CatalogueFingerprint};

pub use instance::debug_set_vmap_enabled;

pub use readout::{game_debug_readout, DebugReadout};

use lyracore_package_delta::{packed_creature_spawn_guid, packed_gameobject_spawn_guid};
use lyracore_shared::{constants, spatial};
use spacetimedb::{log, reducer, ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::{
    game_character, game_config, game_corpse, game_creature_move_schedule, game_creature_spawn,
    game_creature_spline, game_creature_template, game_dynamic_object, game_entity_motion,
    game_gameobject, game_gameobject_pool, game_gameobject_pool_member, game_gameobject_template,
    game_gameobject_unlocked, game_ground_area, game_item_template, game_melee_attack,
    game_quest_template, game_spell, game_spell_effect, game_world_entity, CreatureMoveSchedule,
    CreatureSpawn, GroundArea, ItemInstance, ServerConfig,
};

#[reducer]
pub fn debug_teleport(
    ctx: &ReducerContext,
    character_guid: u64,
    map_id: u32,
    x: f32,
    y: f32,
    z: f32,
    o: f32,
) -> Result<(), String> {
    if ctx
        .db
        .game_world_entity()
        .guid()
        .find(character_guid)
        .is_none()
    {
        return Err(format!("no live entity for guid {character_guid}"));
    }
    crate::world::teleport_player(ctx, character_guid, map_id, 0, x, y, z, o);
    Ok(())
}

#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn debug_bot_transfer(
    ctx: &ReducerContext,
    character_guid: u64,
    map_id: u32,
    instance_id: u64,
    x: f32,
    y: f32,
    z: f32,
    o: f32,
    reason: String,
) -> Result<(), String> {
    if ctx
        .db
        .game_world_entity()
        .guid()
        .find(character_guid)
        .is_none()
    {
        return Err(format!("no live entity for guid {character_guid}"));
    }
    crate::transfer::emit_bot_transfer_intent(
        ctx,
        character_guid,
        crate::transfer::Destination {
            map_id,
            instance_id,
            x,
            y,
            z,
            o,
        },
        &reason,
        0,
    )
    .map_err(|refusal| format!("{:?}: {}", refusal.kind, refusal))?;
    log::info!(
        "debug_bot_transfer: character {character_guid} -> map {map_id} instance {instance_id} \
         ({reason})"
    );
    Ok(())
}

#[reducer]
pub fn debug_apply_damage(
    ctx: &ReducerContext,
    target_guid: u64,
    amount: u32,
    attacker_guid: u64,
) -> Result<(), String> {
    let e = ctx
        .db
        .game_world_entity()
        .guid()
        .find(target_guid)
        .ok_or_else(|| format!("no live entity for guid {target_guid}"))?;
    if e.dead {
        return Err("target is dead".to_string());
    }
    let capped = amount.min(e.health.saturating_sub(1));
    let (dmg, _absorbed) =
        crate::combat::fold_incoming_damage(ctx, attacker_guid, target_guid, capped);
    let damage = crate::combat::final_damage(ctx, target_guid, dmg);
    crate::combat::apply_hit(
        ctx,
        attacker_guid,
        target_guid,
        damage,
        crate::combat::Hit::weapon(crate::combat::HitSource::MainHand, false),
    );
    Ok(())
}

#[reducer]
pub fn debug_set_health(ctx: &ReducerContext, guid: u64, health: u32) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let mut e = entities
        .guid()
        .find(guid)
        .ok_or_else(|| format!("no live entity for guid {guid}"))?;

    let clamped = health.min(e.max_health);
    if clamped > 0 {
        e.health = clamped;
        let was_dead = e.dead;
        e.update(ctx);
        if was_dead {
            crate::ghost::finish_resurrection(ctx, guid);
        }
        return Ok(());
    }

    if e.is_player() {
        crate::combat::kill_player(ctx, guid, 0);
        log::info!("debug_set_health: player {guid} killed (dead, awaiting release)");
        return Ok(());
    }

    crate::combat::kill_creature(ctx, guid, None);
    log::info!("debug_set_health: creature {guid} killed (corpse via kill_creature)");
    Ok(())
}

#[reducer]
pub fn debug_age_corpse(
    ctx: &ReducerContext,
    corpse_guid: u64,
    age_seconds: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let mut corpse = ctx
        .db
        .game_corpse()
        .guid()
        .find(corpse_guid)
        .ok_or("no such Corpse")?;
    corpse.created_at =
        ctx.timestamp - TimeDuration::from_micros(i64::from(age_seconds) * 1_000_000);
    corpse.update(ctx);
    crate::corpse::sweep_corpse_decay(ctx);
    Ok(())
}

#[reducer]
pub fn debug_repop(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    Ok(crate::actor::repop(ctx, character_guid)?)
}

#[reducer]
pub fn debug_spirit_healer_res(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    Ok(crate::actor::spirit_res(ctx, character_guid)?)
}

#[reducer]
pub fn debug_self_resurrect(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    Ok(crate::actor::self_resurrect(ctx, character_guid)?)
}

#[reducer]
pub fn debug_resurrect_response(
    ctx: &ReducerContext,
    character_guid: u64,
    accept: bool,
) -> Result<(), String> {
    Ok(crate::spell::do_resurrect_response(
        ctx,
        character_guid,
        accept,
    )?)
}

#[reducer]
pub fn debug_spawn_at_feet(
    ctx: &ReducerContext,
    character_guid: u64,
    creature_entry: u32,
    offset: f32,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let player = entities
        .guid()
        .find(character_guid)
        .ok_or_else(|| format!("no live entity for guid {character_guid}"))?;
    let tmpl = ctx
        .db
        .game_creature_template()
        .entry()
        .find(creature_entry)
        .ok_or_else(|| format!("no creature template for entry {creature_entry}"))?;

    let spawn_max = ctx
        .db
        .game_creature_spawn()
        .iter()
        .filter(|s| s.entry == creature_entry)
        .map(|s| s.guid & 0x00FF_FFFF)
        .filter(|low| low & crate::instance::INSTANCE_POP_LOW_BAND == 0)
        .max()
        .unwrap_or(0);
    let entity_max = entities
        .iter()
        .filter(|e| e.entry == creature_entry && !e.is_player())
        .map(|e| e.guid & 0x00FF_FFFF)
        .filter(|low| low & crate::instance::INSTANCE_POP_LOW_BAND == 0)
        .max()
        .unwrap_or(0);
    let next_low = spawn_max.max(entity_max) + 1;
    let new_guid = packed_creature_spawn_guid(creature_entry, next_low as u32);

    let (sin_o, cos_o) = player.orientation.sin_cos();
    let spawn = CreatureSpawn {
        guid: new_guid,
        entry: creature_entry,
        map_id: player.map_id,
        x: player.x + offset * cos_o,
        y: player.y + offset * sin_o,
        z: player.z,
        orientation: player.orientation,
        respawn_at: crate::creatures::timer_never(ctx),
        despawn_at: crate::creatures::timer_never(ctx),
        movement_type: crate::creatures::MOVEMENT_IDLE,
        respawn_secs: 0,
        life_seq: 0,
        source_guid: 0,
        instance_id: player.instance_id,
    };
    let entity = build_creature_entity(&spawn, &tmpl, 0, player.instance_id);
    ctx.db.game_creature_spawn().insert(spawn);
    crate::creatures::insert_creature_entity(ctx, entity);
    log::info!(
        "debug_spawn_at_feet: spawned entry {creature_entry} as guid {new_guid} for player {character_guid}"
    );
    Ok(())
}

#[reducer]
pub fn debug_register_aoi_viewer(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::aoi::enter_viewer(ctx, character_guid)
}

#[reducer]
pub fn debug_spawn_player_entity(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    if entities.guid().find(character_guid).is_some() {
        log::info!("debug_spawn_player_entity: guid {character_guid} already live (no-op)");
        return Ok(());
    }

    let character = crate::helpers::require_character(ctx, character_guid)
        .map_err(|_| format!("no such character: {character_guid}"))?;

    let entity = crate::creatures::build_player_entity(ctx, &character, character.owner_identity);
    let (level, max_health) = (entity.level, entity.max_health);
    entity.insert(ctx);
    crate::mount::restore_mount_on_rebuild(ctx, character_guid);
    log::info!(
        "debug_spawn_player_entity: materialized player guid {character_guid} (lvl {level}, hp {max_health})"
    );
    Ok(())
}

#[reducer]
pub fn debug_force_cast(
    ctx: &ReducerContext,
    character_guid: u64,
    spell_id: u32,
) -> Result<(), String> {
    let e = crate::helpers::live_entity(ctx, character_guid)?;
    crate::spell::resolve_cast(ctx, character_guid, spell_id, e.level as u8)
}

fn resolve_debug_cast_at(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    target_guid: u64,
) -> Result<(), String> {
    let caster = crate::helpers::live_entity(ctx, caster_guid)?;
    crate::spell::resolve_cast_at(
        ctx,
        caster_guid,
        spell_id,
        caster.level as u8,
        target_guid,
        false,
        false,
        None,
    )
}

#[reducer]
pub fn debug_force_cast_at(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    target_guid: u64,
) -> Result<(), String> {
    resolve_debug_cast_at(ctx, caster_guid, spell_id, target_guid)
}

#[reducer]
pub fn debug_set_level(
    ctx: &ReducerContext,
    character_guid: u64,
    level: u32,
) -> Result<(), String> {
    crate::helpers::require_character(ctx, character_guid)?;
    crate::stats::set_character_level(ctx, character_guid, level)
}

#[reducer]
pub fn debug_clear_creatures(ctx: &ReducerContext, map_id: u32) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let guids: Vec<u64> = entities
        .iter()
        .filter(|e| e.map_id == map_id && !e.is_player())
        .map(|e| e.guid)
        .collect();
    let n = guids.len();
    for g in &guids {
        crate::combat::disengage(ctx, *g);
        crate::WorldEntity::delete(ctx, g);
    }
    log::info!("debug_clear_creatures: removed {n} creatures on map {map_id}");
    Ok(())
}

#[reducer]
pub fn debug_spawn_ground_area(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
) -> Result<(), String> {
    let caster =
        crate::helpers::live_entity(ctx, caster_guid).map_err(|_| "caster not in world")?;
    let hdr = ctx
        .db
        .game_spell()
        .spell_id()
        .find(spell_id)
        .ok_or("unknown spell")?;
    let eff = ctx
        .db
        .game_spell_effect()
        .iter()
        .find(|e| e.spell_id == spell_id && e.period_ms > 0 && e.radius_yd > 0.0)
        .ok_or("spell has no periodic-area effect (radius + period)")?;
    let now = ctx.timestamp;
    let expires_at = now
        .checked_add(TimeDuration::from_micros(
            (hdr.duration_ms.max(1) as i64) * 1000,
        ))
        .ok_or("duration overflow")?;
    crate::spell::arm_ground_area_tick(ctx);
    ctx.db.game_ground_area().insert(GroundArea {
        id: 0,
        spell_id,
        caster_guid,
        map_id: caster.map_id,
        instance_id: caster.instance_id,
        x: caster.x,
        y: caster.y,
        z: caster.z,
        radius_yd: eff.radius_yd,
        amount: eff.base_points,
        school_mask: hdr.school_mask,
        period_ms: eff.period_ms,
        next_tick_micros: now.to_micros_since_unix_epoch() + (eff.period_ms as i64) * 1000,
        expires_at,
    });
    log::info!(
        "debug_spawn_ground_area: {spell_id} at ({:.1},{:.1}) r={} amount={}/{}ms for {}ms",
        caster.x,
        caster.y,
        eff.radius_yd,
        eff.base_points,
        eff.period_ms,
        hdr.duration_ms
    );
    Ok(())
}

#[reducer]
pub fn debug_use_item(ctx: &ReducerContext, character_guid: u64, slot: u8) -> Result<(), String> {
    crate::actor::use_item(ctx, character_guid, slot).map_err(crate::items::refused)
}

#[reducer]
pub fn debug_grant_item(
    ctx: &ReducerContext,
    character_guid: u64,
    entry: u32,
    count: u32,
) -> Result<(), String> {
    crate::items::grant_item(ctx, character_guid, entry, count)
}

#[reducer]
pub fn debug_bind_home(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    crate::world::set_home(ctx, character_guid);
    Ok(())
}

#[reducer]
pub fn debug_use_hearthstone(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    crate::world::recall_to_home(ctx, character_guid);
    Ok(())
}

#[reducer]
pub fn debug_move_item(
    ctx: &ReducerContext,
    character_guid: u64,
    from_slot: u8,
    to_slot: u8,
) -> Result<(), String> {
    crate::items::apply_item_move(ctx, character_guid, from_slot, to_slot)
        .map_err(crate::items::refused)
}

#[reducer]
pub fn debug_equip_item(
    ctx: &ReducerContext,
    character_guid: u64,
    from_slot: u8,
) -> Result<(), String> {
    crate::actor::equip_item(ctx, character_guid, from_slot).map_err(crate::items::refused)
}

#[reducer]
pub fn debug_unequip_item(
    ctx: &ReducerContext,
    character_guid: u64,
    from_slot: u8,
) -> Result<(), String> {
    crate::items::apply_unequip_item(ctx, character_guid, from_slot).map_err(crate::items::refused)
}

#[reducer]
pub fn debug_auto_bank_item(
    ctx: &ReducerContext,
    character_guid: u64,
    slot: u8,
) -> Result<(), String> {
    crate::items::apply_auto_bank_item(ctx, character_guid, slot)
}

#[reducer]
pub fn debug_repair_item(
    ctx: &ReducerContext,
    character_guid: u64,
    slot: u8,
) -> Result<(), String> {
    crate::items::apply_repair_item(ctx, character_guid, slot)
}

#[reducer]
pub fn debug_sell_item(
    ctx: &ReducerContext,
    character_guid: u64,
    vendor_guid: u64,
    slot: u8,
) -> Result<(), String> {
    crate::actor::sell_item(ctx, character_guid, vendor_guid, slot)
}

#[reducer]
pub fn debug_enter_areatrigger(
    ctx: &ReducerContext,
    character_guid: u64,
    trigger_id: u32,
) -> Result<(), String> {
    crate::quest::apply_enter_areatrigger(ctx, character_guid, trigger_id);
    Ok(())
}

#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn debug_replay_sessionless_areatrigger(
    ctx: &ReducerContext,
    character_guid: u64,
    trigger_id: u32,
    expected_map: u32,
    expected_instance: u64,
    controller_generation: u64,
    expected_relocation_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let relocation_id = crate::quest::enter_sessionless_areatrigger(
        ctx,
        character_guid,
        trigger_id,
        expected_map,
        expected_instance,
        controller_generation,
    )
    .map_err(|refusal| format!("{:?}: {}", refusal.kind, refusal))?;
    if expected_relocation_id != 0 && relocation_id != expected_relocation_id {
        return Err(format!(
            "session-less AreaTrigger replay returned relocation {relocation_id}, expected {expected_relocation_id}"
        ));
    }
    Ok(())
}

#[reducer]
pub fn debug_use_gameobject(
    ctx: &ReducerContext,
    character_guid: u64,
    go_guid: u64,
) -> Result<(), String> {
    crate::actor::use_gameobject(ctx, character_guid, go_guid)
}

#[reducer]
pub fn debug_use_gameobject_entry(
    ctx: &ReducerContext,
    character_guid: u64,
    go_entry: u32,
) -> Result<(), String> {
    let go = ctx
        .db
        .game_gameobject()
        .iter()
        .find(|g| g.template_entry == go_entry)
        .ok_or_else(|| format!("no spawned gameobject of entry {go_entry}"))?;
    crate::actor::use_gameobject(ctx, character_guid, go.guid)
}

#[allow(clippy::too_many_arguments)]
#[reducer]
pub fn debug_spawn_gameobject(
    ctx: &ReducerContext,
    template_entry: u32,
    type_id: u8,
    display_id: u32,
    data0: u32,
    map_id: u32,
    x: f32,
    y: f32,
    z: f32,
    data1: u32,
    gather_skill_line: u32,
    respawn_secs: u32,
    gather_gray: u32,
) -> Result<(), String> {
    if ctx
        .db
        .game_gameobject_template()
        .entry()
        .find(template_entry)
        .is_none()
    {
        ctx.db
            .game_gameobject_template()
            .insert(crate::gameobject::GameObjectTemplate {
                entry: template_entry,
                type_id,
                display_id,
                name: format!("Debug GO {template_entry}"),
                data0,
                data1,
                gather_skill_line,
                respawn_secs,
                gather_gray,
                lock_id: 0,
                size: 0.0,
            });
    }
    let guid = packed_gameobject_spawn_guid(template_entry);
    crate::GameObject::delete(ctx, guid);
    ctx.db.game_gameobject_unlocked().go_guid().delete(guid);
    let go = crate::gameobject::GameObject {
        guid,
        template_entry,
        map_id,
        x,
        y,
        z,
        orientation: 0.0,
        state: 0,
        created_at: ctx.timestamp,
        respawn_at_micros: 0,
        instance_id: 0,
        grid_x: lyracore_shared::spatial::grid_cell(x, y).0,
        grid_y: lyracore_shared::spatial::grid_cell(x, y).1,
        cell: lyracore_shared::spatial::cell_id_at(x, y),
        rotation_0: 0.0,
        rotation_1: 0.0,
        rotation_2: 0.0,
        rotation_3: 0.0,
    }
    .insert(ctx);
    crate::go_collider::register(ctx, &go);
    Ok(())
}

#[reducer]
pub fn debug_force_gameobject_respawn(ctx: &ReducerContext, go_entry: u32) -> Result<(), String> {
    let gos = ctx.db.game_gameobject();
    let mut go = gos
        .iter()
        .find(|g| g.template_entry == go_entry && g.state == 1)
        .ok_or_else(|| format!("no depleted gameobject of entry {go_entry}"))?;
    go.state = 0;
    go.respawn_at_micros = 0;
    go.update(ctx);
    Ok(())
}

#[reducer]
pub fn debug_setup_gather_pool(
    ctx: &ReducerContext,
    pool_id: u32,
    max_active: u32,
    in_place: bool,
    members: String,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let pool_tbl = ctx.db.game_gameobject_pool();
    let member_tbl = ctx.db.game_gameobject_pool_member();
    let tmpl_tbl = ctx.db.game_gameobject_template();

    let prior: Vec<crate::gameobject::GameObjectPoolMember> =
        member_tbl.iter().filter(|m| m.pool_id == pool_id).collect();
    for m in &prior {
        crate::GameObject::delete(ctx, crate::gameobject::pool_point_guid(m.point_id));
        member_tbl.point_id().delete(m.point_id);
    }

    pool_tbl.pool_id().delete(pool_id);
    pool_tbl.insert(crate::gameobject::GameObjectPool {
        pool_id,
        max_active,
        in_place,
    });

    let pu32 = |s: &str| s.trim().parse::<u32>().map_err(|_| format!("bad u32: {s}"));
    let pf32 = |s: &str| s.trim().parse::<f32>().map_err(|_| format!("bad f32: {s}"));
    let mut count = 0u32;
    for row in members.split(';').filter(|r| !r.trim().is_empty()) {
        let f: Vec<&str> = row.split(',').collect();
        if f.len() != 12 {
            return Err(format!(
                "pool member row needs 12 fields, got {}: {row}",
                f.len()
            ));
        }
        let template_entry = pu32(f[0])?;
        let display_id = pu32(f[1])?;
        let data0 = pu32(f[2])?;
        let data1 = pu32(f[3])?;
        let gather_skill_line = pu32(f[4])?;
        let respawn_secs = pu32(f[5])?;
        let map_id = pu32(f[6])?;
        let (x, y, z, o) = (pf32(f[7])?, pf32(f[8])?, pf32(f[9])?, pf32(f[10])?);
        let weight = pu32(f[11])?;
        if tmpl_tbl.entry().find(template_entry).is_none() {
            tmpl_tbl.insert(crate::gameobject::GameObjectTemplate {
                entry: template_entry,
                type_id: crate::gameobject::go_type::GATHER,
                display_id,
                name: format!("Pool GO {template_entry}"),
                data0,
                data1,
                gather_skill_line,
                respawn_secs,
                gather_gray: 0,
                lock_id: 0,
                size: 0.0,
            });
        }
        member_tbl.insert(crate::gameobject::GameObjectPoolMember {
            point_id: 0,
            pool_id,
            template_entry,
            map_id,
            x,
            y,
            z,
            orientation: o,
            weight,
        });
        count += 1;
    }
    if count == 0 {
        return Err("pool setup had no members".to_string());
    }
    crate::gameobject::arm_pool(ctx, pool_id);
    Ok(())
}

#[reducer]
pub fn debug_force_pool_reroll(
    ctx: &ReducerContext,
    pool_id: u32,
    gathered_guid: u64,
) -> Result<(), String> {
    if ctx
        .db
        .game_gameobject_pool()
        .pool_id()
        .find(pool_id)
        .is_none()
    {
        return Err(format!("no pool {pool_id}"));
    }
    crate::gameobject::reroll_pool(ctx, pool_id, gathered_guid);
    Ok(())
}

#[reducer]
pub fn debug_buy_item(
    ctx: &ReducerContext,
    character_guid: u64,
    vendor_guid: u64,
    item_entry: u32,
    count: u32,
) -> Result<(), String> {
    crate::actor::buy_item(ctx, character_guid, vendor_guid, item_entry, count)
}

#[reducer]
pub fn debug_buy_bank_slot(
    ctx: &ReducerContext,
    character_guid: u64,
    banker_guid: u64,
) -> Result<(), String> {
    crate::items::buy_bank_slot_result(crate::items::apply_buy_bank_slot(
        ctx,
        character_guid,
        banker_guid,
    ))
}

#[reducer]
pub fn debug_split_item(
    ctx: &ReducerContext,
    character_guid: u64,
    slot: u8,
    count: u32,
    to_slot: u8,
) -> Result<(), String> {
    crate::items::apply_item_split(ctx, character_guid, slot, count, to_slot)
        .map_err(crate::items::refused)
}

#[reducer]
pub fn debug_engage(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
) -> Result<(), String> {
    let melee = ctx.db.game_melee_attack();
    let row = crate::MeleeAttack {
        attacker_guid,
        target_guid,
        last_swing_ms: 0,
        ranged_spell_id: 0,
        last_offhand_swing_ms: 0,
        rout_ends_ms: 0,
        pursuit_ends_ms: 0,
        leash_x: 0.0,
        leash_y: 0.0,
        swing_error: 0,
    };
    if melee.attacker_guid().find(attacker_guid).is_some() {
        crate::combat::MeleeAttack::update(ctx, row);
    } else {
        crate::combat::MeleeAttack::insert(ctx, row);
    }
    let entities = ctx.db.game_world_entity();
    if let Some(mut e) = entities.guid().find(attacker_guid) {
        e.target_guid = target_guid;
        e.update(ctx);
    }
    Ok(())
}

#[reducer]
pub fn debug_cast_at(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    target_guid: u64,
) -> Result<(), String> {
    resolve_debug_cast_at(ctx, caster_guid, spell_id, target_guid)
}

#[reducer]
pub fn debug_begin_cast(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    target_guid: u64,
) -> Result<(), String> {
    let e = crate::helpers::live_entity(ctx, caster_guid)?;
    crate::spell::begin_cast(
        ctx,
        caster_guid,
        spell_id,
        e.level as u8,
        target_guid,
        false,
        None,
    )
}

#[reducer]
pub fn debug_cast_spell_at(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    target_guid: u64,
    x: f32,
    y: f32,
    z: f32,
) -> Result<(), String> {
    let e = crate::helpers::live_entity(ctx, caster_guid)?;
    let target = if target_guid == 0 {
        caster_guid
    } else {
        target_guid
    };
    crate::spell::begin_cast(
        ctx,
        caster_guid,
        spell_id,
        e.level as u8,
        target,
        false,
        Some((x, y, z)),
    )
}

#[reducer]
pub fn debug_fill_aura_slots(
    ctx: &ReducerContext,
    target_guid: u64,
    caster_guid: u64,
    debuff: bool,
    count: u8,
) -> Result<(), String> {
    let range_len = if debuff {
        constants::tracer_spell::AURA_SLOTS - constants::tracer_spell::BUFF_SLOT_COUNT
    } else {
        constants::tracer_spell::BUFF_SLOT_COUNT
    };
    if count > range_len {
        return Err(format!(
            "count {count} exceeds the real {range_len}-slot {} range",
            if debuff { "debuff" } else { "buff" }
        ));
    }
    let base_slot = if debuff {
        constants::tracer_spell::BUFF_SLOT_COUNT
    } else {
        0
    };
    let now = ctx.timestamp;
    let expires = now
        .checked_add(TimeDuration::from_micros(3_600_000_000))
        .unwrap_or(now);
    for i in 0..count {
        let slot = base_slot + i;
        crate::Aura {
            id: 0,
            target_guid,
            caster_guid,
            spell_id: 9_000_000 + slot as u32,
            slot,
            level: 1,
            flags: if debuff {
                0
            } else {
                constants::tracer_spell::AURA_FLAGS
            },
            applied_at: now,
            expires_at: expires,
            effect_id: 0,
            eff_kind: 0,
            amount: 0,
            eff_p0: 0,
            eff_p0_kind: 0,
            eff_p1: 0,
            period_ms: 0,
            amount_remaining: 0,
            stacks: 1,
            next_tick_micros: 0,
            channel_target: 0,
            enters_combat: false,
            proc_flags: 0,
            proc_chance: 0,
            proc_ppm: 0.0,
            proc_ex: 0,
            proc_school_mask: 0,
            proc_family_name: 0,
            proc_family_flags: 0,
            proc_charges: 0,
            proc_icd_ms: 0,
            proc_ready_micros: 0,
        }
        .insert(ctx);
    }
    Ok(())
}

#[reducer]
pub fn debug_log_vendor_discount(
    ctx: &ReducerContext,
    player_guid: u64,
    faction_template_id: u32,
) -> Result<(), String> {
    let pct = crate::reputation::vendor_discount_pct(ctx, player_guid, faction_template_id);
    log::info!("debug_log_vendor_discount: player {player_guid} faction_template {faction_template_id} → {pct}% off");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
#[reducer]
pub fn debug_score_movement(
    ctx: &ReducerContext,
    guid: u64,
    old_x: f32,
    old_y: f32,
    x: f32,
    y: f32,
    z: f32,
    old_move_ms: u32,
    move_time_ms: u32,
) -> Result<(), String> {
    crate::world::score_and_log_movement(
        ctx,
        guid,
        &crate::world::MovementDelta {
            old_x,
            old_y,
            old_z: 0.0,
            x,
            y,
            z,
            old_move_ms,
            move_time_ms,
        },
    );
    Ok(())
}

#[reducer]
pub fn debug_take_loot(
    ctx: &ReducerContext,
    character_guid: u64,
    corpse_guid: u64,
    loot_slot: u8,
) -> Result<(), String> {
    crate::actor::take_loot(ctx, character_guid, corpse_guid, loot_slot)
}

#[reducer]
pub fn debug_equip_weapon(
    ctx: &ReducerContext,
    character_guid: u64,
    item_entry: u32,
) -> Result<(), String> {
    equip_into(
        ctx,
        character_guid,
        item_entry,
        constants::starter_item::MAINHAND_SLOT,
    )
}

#[reducer]
pub fn debug_equip_offhand(
    ctx: &ReducerContext,
    character_guid: u64,
    item_entry: u32,
) -> Result<(), String> {
    equip_into(
        ctx,
        character_guid,
        item_entry,
        crate::items::equip_slot::OFFHAND,
    )
}

fn equip_into(
    ctx: &ReducerContext,
    character_guid: u64,
    item_entry: u32,
    slot: u8,
) -> Result<(), String> {
    let e = crate::helpers::live_entity(ctx, character_guid)?;
    let tmpl = ctx.db.game_item_template().entry().find(item_entry);
    let random_property_id = tmpl
        .as_ref()
        .map(|t| crate::items::select_property(ctx, t))
        .transpose()?
        .unwrap_or(0);
    let durability = tmpl.as_ref().map(|t| t.max_durability).unwrap_or(0);
    let soulbound = tmpl
        .as_ref()
        .map(|t| crate::items::binds_on_grant(t.bonding) || crate::items::binds_on_equip(t.bonding))
        .unwrap_or(false);
    if let Some(mut inst) = crate::items::item_in_slot(ctx, character_guid, slot) {
        if inst.entry != item_entry {
            inst.random_property_id = random_property_id;
        }
        inst.entry = item_entry;
        inst.durability = durability;
        inst.soulbound = inst.soulbound || soulbound;
        inst.update(ctx);
    } else {
        let guid = crate::items::next_item_guid(ctx)?;
        crate::ItemInstance::insert(
            ctx,
            ItemInstance {
                guid,
                entry: item_entry,
                owner_identity: e.owner_identity,
                owner_guid: character_guid,
                slot,
                stack_count: 1,
                durability,
                created_at: ctx.timestamp,
                enchant_id: 0,
                soulbound,
                random_property_id,
                item_text_id: 0,
            },
        );
    }
    Ok(())
}

#[reducer]
pub fn debug_accept_quest(
    ctx: &ReducerContext,
    character_guid: u64,
    giver_guid: u64,
    quest_entry: u32,
) -> Result<(), String> {
    crate::actor::accept_quest(ctx, character_guid, giver_guid, quest_entry)
}

#[reducer]
pub fn debug_stress_relay(
    ctx: &ReducerContext,
    character_guid: u64,
    victim_entry: u32,
    item_entry: u32,
    junk_rows: u32,
) -> Result<(), String> {
    if junk_rows > 0 {
        spacetimedb::log::warn!(
            "debug_stress_relay: ignoring junk_rows={junk_rows} — the game_creature_move_event \
             ballast is no longer supported; pass 0"
        );
    }
    crate::quest::on_creature_killed(ctx, character_guid, victim_entry);
    crate::items::grant_item(ctx, character_guid, item_entry, 1)?;
    Ok(())
}

#[reducer]
pub fn debug_grant_quest(
    ctx: &ReducerContext,
    character_guid: u64,
    quest_entry: u32,
) -> Result<(), String> {
    crate::actor::stage_quest(ctx, character_guid, quest_entry)
}

#[reducer]
pub fn debug_expire_quest(
    ctx: &ReducerContext,
    character_guid: u64,
    quest_entry: u32,
) -> Result<(), String> {
    crate::helpers::require_character(ctx, character_guid)?;
    crate::quest::debug_force_expire(ctx, character_guid, quest_entry)
}

#[reducer]
pub fn debug_push_quest(
    ctx: &ReducerContext,
    character_guid: u64,
    quest_entry: u32,
) -> Result<(), String> {
    crate::quest::apply_push_quest_to_party(ctx, character_guid, quest_entry)
}

#[reducer]
pub fn debug_gossip_select(
    ctx: &ReducerContext,
    character_guid: u64,
    npc_guid: u64,
    option_id: u32,
    option_row_id: u32,
    source_menu_id: u32,
    selected_action: u32,
) -> Result<(), String> {
    crate::world::apply_gossip_select(
        ctx,
        character_guid,
        npc_guid,
        option_id,
        option_row_id,
        source_menu_id,
        selected_action,
    )
}

#[reducer]
pub fn debug_turn_in_quest(
    ctx: &ReducerContext,
    character_guid: u64,
    giver_guid: u64,
    quest_entry: u32,
    reward_index: u32,
) -> Result<(), String> {
    crate::actor::turn_in_quest(ctx, character_guid, giver_guid, quest_entry, reward_index)
}

#[reducer]
pub fn debug_creature_path(
    ctx: &ReducerContext,
    guid: u64,
    x: f32,
    y: f32,
    z: f32,
    run: bool,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let mover = crate::helpers::live_entity(ctx, guid)?;
    crate::creatures::tick::emit_creature_path(ctx, mover, vec![(x, y, z)], run);
    Ok(())
}

#[reducer]
pub fn debug_creature_waypoints(
    ctx: &ReducerContext,
    guid: u64,
    points: Vec<crate::creatures::tick::CreaturePathPoint>,
    run: bool,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let mover = crate::helpers::live_entity(ctx, guid)?;
    let points: Vec<_> = points.into_iter().map(|p| (p.x, p.y, p.z)).collect();
    let points = lyracore_shared::movement_path::wire_points(&points)
        .ok_or("creature path cannot be encoded")?;
    crate::creatures::tick::emit_creature_path(ctx, mover, points, run);
    Ok(())
}

#[reducer]
pub fn debug_creature_face(ctx: &ReducerContext, guid: u64, x: f32, y: f32) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if !x.is_finite() || !y.is_finite() {
        return Err("facing point is not finite".into());
    }
    let mut mover = crate::helpers::live_entity(ctx, guid)?;
    crate::creatures::tick::stop_facing(ctx, &mut mover, (x, y));
    mover.update(ctx);
    Ok(())
}

#[reducer]
pub fn debug_kill_creature(
    ctx: &ReducerContext,
    killer_guid: u64,
    target_guid: u64,
) -> Result<(), String> {
    if crate::combat::kill_creature(ctx, target_guid, Some(killer_guid)) {
        Ok(())
    } else {
        Err(format!("nothing killed for target {target_guid}"))
    }
}

#[reducer]
pub fn debug_kill_nearest(
    ctx: &ReducerContext,
    killer_guid: u64,
    creature_entry: u32,
) -> Result<(), String> {
    let killer = crate::helpers::live_entity(ctx, killer_guid)
        .map_err(|_| format!("killer {killer_guid} not in world"))?;
    let target_guid = crate::helpers::nearest_entity(ctx, &killer, |e| {
        e.entry == creature_entry && e.guid != killer_guid && !e.dead
    })
    .ok_or_else(|| format!("no live creature entry {creature_entry} near killer"))?
    .guid;
    if crate::combat::kill_creature(ctx, target_guid, Some(killer_guid)) {
        Ok(())
    } else {
        Err(format!("nothing killed for entry {creature_entry}"))
    }
}

#[reducer]
pub fn debug_skin_nearest(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    let looter = crate::helpers::live_entity(ctx, character_guid)
        .map_err(|_| format!("skinner {character_guid} not in world"))?;
    let corpse_guid = crate::helpers::nearest_entity(ctx, &looter, |e| {
        e.dead && !e.is_player() && !e.skinned && crate::loot::entry_is_beast(ctx, e.entry)
    })
    .ok_or_else(|| "no skinnable beast corpse near".to_string())?
    .guid;
    Ok(crate::loot::skin_corpse(ctx, character_guid, corpse_guid)?)
}

#[reducer]
pub fn debug_fish(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    crate::loot::apply_fish(ctx, character_guid)
}

#[reducer]
pub fn debug_disenchant(ctx: &ReducerContext, character_guid: u64, slot: u8) -> Result<(), String> {
    crate::loot::apply_disenchant(ctx, character_guid, slot)
}

#[reducer]
pub fn debug_enchant_item(
    ctx: &ReducerContext,
    character_guid: u64,
    target_slot: u8,
    enchant_id: u32,
) -> Result<(), String> {
    crate::loot::apply_enchant_item(ctx, character_guid, target_slot, enchant_id)
}

#[reducer]
pub fn debug_ranged_attack_nearest(
    ctx: &ReducerContext,
    attacker_guid: u64,
    spell_id: u32,
) -> Result<(), String> {
    let attacker = crate::helpers::live_entity(ctx, attacker_guid)
        .map_err(|_| format!("attacker {attacker_guid} not in world"))?;
    let target_guid = crate::helpers::nearest_entity(ctx, &attacker, |e| {
        e.guid != attacker_guid && !e.dead && !e.is_player()
    })
    .ok_or_else(|| "no live creature near attacker".to_string())?
    .guid;
    crate::actor::ranged_attack(ctx, attacker_guid, target_guid, spell_id)
}

#[reducer]
pub fn debug_learn_spell(
    ctx: &ReducerContext,
    character_guid: u64,
    spell_id: u32,
) -> Result<(), String> {
    let e = crate::helpers::live_entity(ctx, character_guid)?;
    crate::spell::learn_spell(ctx, character_guid, e.owner_identity, spell_id);
    Ok(())
}

#[reducer]
pub fn debug_set_xp_rate(ctx: &ReducerContext, rate: f32) -> Result<(), String> {
    let rate = rate.max(0.0);
    let cfg = ctx.db.game_config();
    match cfg.id().find(0) {
        Some(mut c) => {
            c.xp_rate = rate;
            cfg.id().update(c);
        }
        None => {
            cfg.insert(ServerConfig {
                id: 0,
                xp_rate: rate,
                nav_enabled: false,
                hosts_instances: true,
                bots_idle: false,
                vmap_enabled: false,
                nav_coverage_enabled: false,
            });
        }
    }
    Ok(())
}

#[reducer]
pub fn debug_backfill_cell_ids(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let mut totals: Vec<(&str, usize)> = Vec::new();

    let entities = ctx.db.game_world_entity();
    let stale: Vec<u64> = entities
        .iter()
        .filter(|e| e.cell != spatial::grid_cell_id(e.grid_x, e.grid_y))
        .map(|e| e.guid)
        .collect();
    totals.push(("game_world_entity", stale.len()));
    for guid in stale {
        if let Some(mut e) = entities.guid().find(guid) {
            e.cell = spatial::grid_cell_id(e.grid_x, e.grid_y);
            e.update(ctx);
        }
    }

    let gos = ctx.db.game_gameobject();
    let stale: Vec<u64> = gos
        .iter()
        .filter(|g| g.cell != spatial::grid_cell_id(g.grid_x, g.grid_y))
        .map(|g| g.guid)
        .collect();
    totals.push(("game_gameobject", stale.len()));
    for guid in stale {
        if let Some(mut g) = gos.guid().find(guid) {
            g.cell = spatial::grid_cell_id(g.grid_x, g.grid_y);
            g.update(ctx);
        }
    }

    let motions = ctx.db.game_entity_motion();
    let stale: Vec<u64> = motions
        .iter()
        .filter(|m| m.cell != spatial::grid_cell_id(m.grid_x, m.grid_y))
        .map(|m| m.guid)
        .collect();
    totals.push(("game_entity_motion", stale.len()));
    for guid in stale {
        if let Some(mut m) = motions.guid().find(guid) {
            m.cell = spatial::grid_cell_id(m.grid_x, m.grid_y);
            motions.guid().update(m);
        }
    }

    let splines = ctx.db.game_creature_spline();
    let stale: Vec<u64> = splines
        .iter()
        .filter(|s| s.cell != spatial::grid_cell_id(s.grid_x, s.grid_y))
        .map(|s| s.guid)
        .collect();
    totals.push(("game_creature_spline", stale.len()));
    for guid in stale {
        if let Some(s) = splines.guid().find(guid) {
            s.repair_cell(ctx);
        }
    }

    let dynamic_objects = ctx.db.game_dynamic_object();
    let stale: Vec<_> = dynamic_objects
        .iter()
        .filter(|object| object.cell != object.position_cell())
        .collect();
    totals.push(("game_dynamic_object", stale.len()));
    for object in stale {
        object.repair_cell(ctx);
    }

    let corpses = ctx.db.game_corpse();
    let stale: Vec<_> = corpses
        .iter()
        .filter(|c| c.cell != c.position_cell())
        .collect();
    totals.push(("game_corpse", stale.len()));
    for corpse in stale {
        corpse.update(ctx);
    }

    for (table, n) in totals {
        log::info!("debug_backfill_cell_ids: {table} — {n} row(s) re-stamped");
    }
    Ok(())
}

#[reducer]
pub fn debug_regrid(ctx: &ReducerContext) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let updates: Vec<(u64, i32, i32)> = entities
        .iter()
        .filter_map(|e| {
            let (gx, gy) = spatial::grid_cell(e.x, e.y);
            (e.grid_x != gx || e.grid_y != gy || e.cell != spatial::grid_cell_id(gx, gy))
                .then_some((e.guid, gx, gy))
        })
        .collect();
    let n = updates.len();
    for (guid, gx, gy) in updates {
        if let Some(mut e) = entities.guid().find(guid) {
            e.grid_x = gx;
            e.grid_y = gy;
            e.cell = lyracore_shared::spatial::grid_cell_id(gx, gy);
            e.update(ctx);
        }
    }
    log::info!("debug_regrid: re-stamped {n} entities at the current GRID_CELL_SIZE");
    Ok(())
}

#[reducer]
pub fn debug_grant_reputation(
    ctx: &ReducerContext,
    character_guid: u64,
    faction_id: u32,
    amount: i32,
) -> Result<(), String> {
    crate::helpers::require_character(ctx, character_guid)?;
    crate::reputation::grant_reputation(ctx, character_guid, faction_id, amount);
    Ok(())
}

#[reducer]
pub fn debug_buy_trainer_spell(
    ctx: &ReducerContext,
    character_guid: u64,
    trainer_entry: u32,
    spell_id: u32,
) -> Result<(), String> {
    let trainer = ctx
        .db
        .game_world_entity()
        .iter()
        .find(|e| e.entry == trainer_entry && !e.is_player())
        .ok_or_else(|| format!("no spawned trainer with entry {trainer_entry}"))?;
    crate::actor::trainer_buy(ctx, character_guid, trainer.guid, spell_id)
        .map_err(|refusal| refusal.as_tag().into())
}

#[reducer]
pub fn debug_learn_talent(
    ctx: &ReducerContext,
    character_guid: u64,
    talent_id: u32,
) -> Result<(), String> {
    let e = crate::helpers::live_entity(ctx, character_guid)?;
    Ok(
        crate::talent::do_learn_talent(ctx, character_guid, e.owner_identity, talent_id)
            .map(|_| ())?,
    )
}

#[reducer]
pub fn debug_reset_talents(
    ctx: &ReducerContext,
    character_guid: u64,
    trainer_guid: u64,
) -> Result<(), String> {
    crate::talent::do_reset_talents(ctx, character_guid, trainer_guid).map(|_| ())
}

const INSTANCE_TICK_MS_FLOOR: u64 = 50;

const INSTANCE_TICK_MS_CEIL: u64 = 600_000;

#[reducer]
pub fn debug_arm_instance_tick(
    ctx: &ReducerContext,
    instance_id: u64,
    tick_ms: u64,
) -> Result<(), String> {
    if instance_id == crate::creatures::GLOBAL_TICK_INSTANCE {
        return Err(
            "instance_id u64::MAX is the catch-all sentinel — use debug_rearm_creature_tick".into(),
        );
    }
    if tick_ms < INSTANCE_TICK_MS_FLOOR {
        return Err(format!("tick_ms {tick_ms} below the {INSTANCE_TICK_MS_FLOOR}ms floor (serialized commit-stream tax)"));
    }
    if tick_ms > INSTANCE_TICK_MS_CEIL {
        return Err(format!("tick_ms {tick_ms} above the {INSTANCE_TICK_MS_CEIL}ms ceiling (overflow guard — see the const's doc)"));
    }
    let sched = ctx.db.game_creature_move_schedule();
    let stale: Vec<u64> = sched
        .iter()
        .filter(|r| r.instance_id == instance_id)
        .map(|r| r.scheduled_id)
        .collect();
    for id in stale {
        sched.scheduled_id().delete(id);
    }
    sched.insert(CreatureMoveSchedule {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(tick_ms as i64 * 1000)),
        instance_id,
    });
    Ok(())
}

#[reducer]
pub fn debug_disarm_instance_tick(ctx: &ReducerContext, instance_id: u64) -> Result<(), String> {
    if instance_id == crate::creatures::GLOBAL_TICK_INSTANCE {
        return Err("refusing to delete the load-bearing catch-all row".into());
    }
    let sched = ctx.db.game_creature_move_schedule();
    let ids: Vec<u64> = sched
        .iter()
        .filter(|r| r.instance_id == instance_id)
        .map(|r| r.scheduled_id)
        .collect();
    if ids.is_empty() {
        return Err(format!("no dedicated tick row for instance {instance_id}"));
    }
    for id in ids {
        sched.scheduled_id().delete(id);
    }
    Ok(())
}

#[reducer]
pub fn debug_verify_combat_regen(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    let e = crate::helpers::live_entity(ctx, character_guid)?;
    let pct = crate::spell::combat_health_regen_pct(ctx, character_guid);
    let pct_u32 = pct.max(0) as u32;
    let would_heal =
        crate::combat::regen_health_in_combat(e.health, e.max_health, e.spirit, e.level, pct_u32);
    let delta = would_heal.saturating_sub(e.health);
    log::info!(
        "debug_verify_combat_regen: guid={character_guid} pct={pct} health={} max={} spirit={} level={} → would_heal={would_heal} delta={delta}",
        e.health, e.max_health, e.spirit, e.level
    );
    if pct <= 0 {
        return Err(format!(
            "no A_COMBAT_HEALTH_REGEN_PCT aura on guid {character_guid} (pct={pct}); grant the passive first"
        ));
    }
    if delta == 0 {
        return Err(format!(
            "aura present (pct={pct}) but tick delta is 0 — entity spirit/level too low for integer math to produce ≥1 HP (spirit={} level={}); expected at higher level or spirit",
            e.spirit, e.level
        ));
    }
    Ok(())
}

#[reducer]
pub fn debug_cast_spell(
    ctx: &ReducerContext,
    character_guid: u64,
    spell_id: u32,
    target_guid: u64,
) -> Result<(), String> {
    let e = crate::helpers::live_entity(ctx, character_guid)?;
    if !crate::spell::knows_spell(ctx, character_guid, spell_id) {
        return Err(format!("spell {spell_id} is not in the caster's spellbook"));
    }
    let target = if target_guid == 0 {
        character_guid
    } else {
        target_guid
    };
    crate::spell::begin_cast(
        ctx,
        character_guid,
        spell_id,
        e.level as u8,
        target,
        false,
        None,
    )
}

#[reducer]
pub fn debug_sweep_slice_lens(_ctx: &ReducerContext) -> Result<(), String> {
    let delete_len = crate::CHARACTER_OWNED_DELETE_SWEEPS.len();
    let restamp_len = crate::CHARACTER_OWNED_RESTAMP_SWEEPS.len();
    log::info!(
        "debug_sweep_slice_lens: CHARACTER_OWNED_DELETE_SWEEPS={delete_len} CHARACTER_OWNED_RESTAMP_SWEEPS={restamp_len}"
    );
    if delete_len == 0 {
        return Err("CHARACTER_OWNED_DELETE_SWEEPS is empty — the build.rs marker scan found nothing (registry mechanism broken)".to_string());
    }
    if restamp_len == 0 {
        return Err("CHARACTER_OWNED_RESTAMP_SWEEPS is empty — the build.rs marker scan found nothing (registry mechanism broken)".to_string());
    }
    Ok(())
}

#[reducer]
pub fn debug_seed_scenario_fixtures(ctx: &ReducerContext) {
    crate::seed::seed_scenario_fixtures(ctx);
    crate::seed::seed_regen_fixture(ctx);
    crate::seed::seed_pw_shield_fixture(ctx);
    crate::seed::seed_stacking_probe_fixture(ctx);
    let quest = ctx
        .db
        .game_quest_template()
        .entry()
        .find(50900u32)
        .is_some() as u64;
    let npcs = [51003u32, 51004, 51005]
        .iter()
        .filter(|e| ctx.db.game_creature_template().entry().find(**e).is_some())
        .count() as u64;
    crate::import_meta::stamp(ctx, "debug_seed_scenario_fixtures", "", "", quest + npcs);
}

#[reducer]
pub fn debug_set_money(
    ctx: &ReducerContext,
    character_guid: u64,
    copper: u32,
) -> Result<(), String> {
    let chars = ctx.db.game_character();
    let mut c = crate::helpers::require_character(ctx, character_guid)?;
    c.money = copper;
    chars.guid().update(c);
    let entities = ctx.db.game_world_entity();
    if let Some(mut e) = entities.guid().find(character_guid) {
        e.money = copper;
        e.update(ctx);
    }
    Ok(())
}

#[reducer]
pub fn debug_set_power(ctx: &ReducerContext, guid: u64, power: u32) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let mut e = entities
        .guid()
        .find(guid)
        .ok_or_else(|| format!("no live entity for guid {guid}"))?;
    if e.max_power < power {
        e.max_power = power;
    }
    e.power = power;
    e.update(ctx);
    Ok(())
}

use crate::creatures::build_creature_entity;
