use lyracore_package_delta::packed_creature_spawn_guid;
use spacetimedb::{log, reducer, ReducerContext, Table};

use crate::creatures::build_creature_entity;
use crate::nav::game_nav_chunk;
use crate::vmap::game_vmap_chunk;
use crate::{
    game_config, game_creature_spawn, game_creature_template, game_gameobject, game_instance,
    game_world_entity, CreatureSpawn, ServerConfig,
};

#[reducer]
pub fn debug_create_fixture_instance(
    ctx: &ReducerContext,
    character_guid: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let player = crate::helpers::live_entity(ctx, character_guid)?;
    let map_id = player.map_id;
    let instance_id = crate::instance::resolve_or_create_instance(ctx, character_guid, map_id)?;
    crate::world::teleport_player(
        ctx,
        character_guid,
        map_id,
        instance_id,
        player.x,
        player.y,
        player.z,
        player.orientation,
    );
    log::info!("debug_create_fixture_instance: character {character_guid} → instance {instance_id} on map {map_id}");
    Ok(())
}

#[reducer]
pub fn debug_enter_instance(
    ctx: &ReducerContext,
    character_guid: u64,
    instance_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let player = crate::helpers::live_entity(ctx, character_guid)?;
    if instance_id != 0 {
        let inst = ctx
            .db
            .game_instance()
            .instance_id()
            .find(instance_id)
            .ok_or_else(|| format!("no such instance {instance_id}"))?;
        if inst.map_id != player.map_id {
            return Err(format!(
                "instance {instance_id} is on map {} but the character is on map {} (same-map only)",
                inst.map_id, player.map_id
            ));
        }
    }
    crate::world::teleport_player(
        ctx,
        character_guid,
        player.map_id,
        instance_id,
        player.x,
        player.y,
        player.z,
        player.orientation,
    );
    Ok(())
}

#[reducer]
pub fn debug_reap_instance(ctx: &ReducerContext, instance_id: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if instance_id == 0 {
        return Err("instance 0 is the open world".to_string());
    }
    if ctx
        .db
        .game_instance()
        .instance_id()
        .find(instance_id)
        .is_none()
    {
        return Err(format!("no such instance {instance_id}"));
    }
    if crate::instance::occupied_instances(ctx).contains(&instance_id) {
        return Err(format!(
            "instance {instance_id} is occupied or claimed — reap refused"
        ));
    }
    crate::instance::teardown_instance(ctx, instance_id);
    Ok(())
}

#[reducer]
pub fn debug_reset_instance(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::instance::apply_reset_instances(ctx, character_guid).map(|n| {
        log::info!("debug_reset_instance: {n} instance(s) flagged for reset by {character_guid}");
    })
}

#[reducer]
pub fn debug_nav_probe(ctx: &ReducerContext, map: u32, x: f32, y: f32) {
    let walk = crate::nav::walkable(ctx, map, x, y);
    let obs = crate::nav::obstruction_top(ctx, map, x, y);
    let ground = crate::terrain::ground_z(ctx, map, x, y);
    log::info!(
        "nav probe ({x:.1},{y:.1}) map {map}: walkable={walk:?} obstruction_top={obs:?} ground_z={ground:?}"
    );
}

#[reducer]
pub fn debug_set_nav_enabled(ctx: &ReducerContext, enabled: bool) -> Result<(), String> {
    let cfg = ctx.db.game_config();
    match cfg.id().find(0) {
        Some(mut c) => {
            c.nav_enabled = enabled;
            cfg.id().update(c);
        }
        None => {
            cfg.insert(ServerConfig {
                id: 0,
                xp_rate: 1.0,
                nav_enabled: enabled,
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
pub fn debug_nav_leg(ctx: &ReducerContext, map: u32, x0: f32, y0: f32, x1: f32, y1: f32) {
    let enabled = crate::nav::nav_enabled(ctx);
    let z0 = crate::terrain::ground_z(ctx, map, x0, y0).unwrap_or(0.0);
    let z1 = crate::terrain::ground_z(ctx, map, x1, y1).unwrap_or(0.0);
    let step = crate::nav::nav_step(ctx, map, 0, (x0, y0), (x1, y1), 4.0, 0.0, z0);
    let los = crate::nav::has_los(ctx, map, 0, (x0, y0, z0), (x1, y1, z1));
    let raw = crate::nav::debug_find_leg(ctx, map, (x0, y0), (x1, y1));
    log::info!(
        "nav leg ({x0:.1},{y0:.1})->({x1:.1},{y1:.1}) map {map}: nav_enabled={enabled} first_step=({:.2},{:.2}) has_los={los} raw={raw}",
        step.0, step.1
    );
}

#[reducer]
pub fn debug_set_vmap_enabled(ctx: &ReducerContext, enabled: bool) -> Result<(), String> {
    let cfg = ctx.db.game_config();
    match cfg.id().find(0) {
        Some(mut c) => {
            c.vmap_enabled = enabled;
            cfg.id().update(c);
        }
        None => {
            cfg.insert(ServerConfig {
                id: 0,
                xp_rate: 1.0,
                nav_enabled: false,
                hosts_instances: true,
                bots_idle: false,
                vmap_enabled: enabled,
                nav_coverage_enabled: false,
            });
        }
    }
    Ok(())
}

#[reducer]
pub fn debug_set_nav_coverage_enabled(ctx: &ReducerContext, enabled: bool) -> Result<(), String> {
    let cfg = ctx.db.game_config();
    match cfg.id().find(0) {
        Some(mut c) => {
            c.nav_coverage_enabled = enabled;
            cfg.id().update(c);
        }
        None => {
            cfg.insert(ServerConfig {
                id: 0,
                xp_rate: 1.0,
                nav_enabled: false,
                hosts_instances: true,
                bots_idle: false,
                vmap_enabled: false,
                nav_coverage_enabled: enabled,
            });
        }
    }
    Ok(())
}

#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn debug_vmap_ray(
    ctx: &ReducerContext,
    map: u32,
    x0: f32,
    y0: f32,
    z0: f32,
    x1: f32,
    y1: f32,
    z1: f32,
) {
    debug_vmap_ray_instance(ctx, map, x0, y0, z0, x1, y1, z1, 0);
}

#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn debug_vmap_ray_instance(
    ctx: &ReducerContext,
    map: u32,
    x0: f32,
    y0: f32,
    z0: f32,
    x1: f32,
    y1: f32,
    z1: f32,
    instance_id: u64,
) {
    let enabled = crate::vmap::rays_enabled(ctx);
    let a = [x0, y0, z0];
    let b = [x1, y1, z1];
    let (los, collision) = crate::vmap::probe_rays(ctx, map, instance_id, a, b);
    log::info!(
        "vmap ray ({x0:.1},{y0:.1},{z0:.1})->({x1:.1},{y1:.1},{z1:.1}) map {map} instance {instance_id}: vmap_enabled={enabled} los={los:?} collision={collision:?}"
    );
}

#[reducer]
pub fn debug_floor_probe(ctx: &ReducerContext, map: u32, x: f32, y: f32, probe_z: f32) {
    let enabled = crate::vmap::vmap_enabled(ctx, map);
    let ground = crate::terrain::ground_z(ctx, map, x, y);
    let floor = crate::vmap::probe_floor_z(ctx, map, 0, x, y, probe_z);
    let snapped = crate::terrain::snap_z(ctx, map, 0, x, y, probe_z);
    log::info!(
        "floor probe ({x:.1},{y:.1}) map {map} probe_z={probe_z:.2}: vmap_enabled={enabled} ground_z={ground:?} floor_z={floor:?} snap_z={snapped:.2}"
    );
}

#[reducer]
pub fn debug_vmap_area_info(ctx: &ReducerContext, map: u32, x: f32, y: f32, z: f32) {
    let enabled = crate::vmap::vmap_enabled(ctx, map);
    let info = crate::vmap::area_info(ctx, map, x, y, z);
    log::info!(
        "vmap area-info ({x:.1},{y:.1},{z:.1}) map {map}: vmap_enabled={enabled} group_id={:?} indoor={:?}",
        info.map(|i| i.group_id),
        info.map(|i| i.indoor)
    );
}

#[reducer]
pub fn debug_assert_floor_snap(
    ctx: &ReducerContext,
    map_id: u32,
    creature_entry: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;

    const ORIGIN: (f32, f32) = (2600.0, 2600.0);
    const FLOOR_Z: f32 = 20.0;
    const PROBE_UNDERSHOOT_YD: f32 = 0.5;
    const TERRAIN_FALLBACK: f32 = FLOOR_Z - PROBE_UNDERSHOOT_YD;

    let tris = [
        lyracore_shared::vmap::VmapTri {
            verts: [
                [ORIGIN.0 - 20.0, ORIGIN.1 - 20.0, FLOOR_Z],
                [ORIGIN.0 + 20.0, ORIGIN.1 - 20.0, FLOOR_Z],
                [ORIGIN.0 - 20.0, ORIGIN.1 + 20.0, FLOOR_Z],
            ],
            class: lyracore_shared::vmap::TriClass::Wmo {
                group_id: 0,
                mogp_flags: 0,
            },
        },
        lyracore_shared::vmap::VmapTri {
            verts: [
                [ORIGIN.0 + 20.0, ORIGIN.1 - 20.0, FLOOR_Z],
                [ORIGIN.0 + 20.0, ORIGIN.1 + 20.0, FLOOR_Z],
                [ORIGIN.0 - 20.0, ORIGIN.1 + 20.0, FLOOR_Z],
            ],
            class: lyracore_shared::vmap::TriClass::Wmo {
                group_id: 0,
                mogp_flags: 0,
            },
        },
    ];
    let blob = lyracore_shared::vmap::encode(&tris);

    let vmap_chunks = ctx.db.game_vmap_chunk();
    let mut written = std::collections::BTreeSet::new();
    let mut inserted_ids = Vec::new();
    for dx in [-15.0f32, 0.0, 15.0] {
        for dy in [-15.0f32, 0.0, 15.0] {
            let (Some(cx), Some(cy)) = (
                lyracore_shared::terrain::cell_index(ORIGIN.0 + dx),
                lyracore_shared::terrain::cell_index(ORIGIN.1 + dy),
            ) else {
                continue;
            };
            if !written.insert((cx, cy)) {
                continue;
            }
            let row = vmap_chunks.insert(crate::vmap::VmapChunk {
                id: 0,
                key: lyracore_shared::terrain::cell_key(map_id, cx, cy),
                map_id,
                cell_x: cx,
                cell_y: cy,
                blob: blob.clone(),
            });
            inserted_ids.push(row.id);
        }
    }

    let cfg = ctx.db.game_config();
    let prior_vmap = cfg.id().find(0).map(|c| c.vmap_enabled);
    match cfg.id().find(0) {
        Some(mut c) => {
            c.vmap_enabled = true;
            cfg.id().update(c);
        }
        None => {
            cfg.insert(ServerConfig {
                id: 0,
                xp_rate: 1.0,
                nav_enabled: false,
                hosts_instances: true,
                bots_idle: false,
                vmap_enabled: true,
                nav_coverage_enabled: false,
            });
        }
    }

    let floor = crate::vmap::floor_z(ctx, map_id, 0, ORIGIN.0, ORIGIN.1, FLOOR_Z + 5.0);
    let snapped = crate::terrain::snap_z(ctx, map_id, 0, ORIGIN.0, ORIGIN.1, TERRAIN_FALLBACK);
    let ground = crate::terrain::ground_z(ctx, map_id, ORIGIN.0, ORIGIN.1);

    let spawn_max = ctx
        .db
        .game_creature_spawn()
        .iter()
        .filter(|s| s.entry == creature_entry)
        .map(|s| s.guid & 0x00FF_FFFF)
        .max()
        .unwrap_or(0);
    let entity_max = ctx
        .db
        .game_world_entity()
        .iter()
        .filter(|e| e.entry == creature_entry && !e.is_player())
        .map(|e| e.guid & 0x00FF_FFFF)
        .max()
        .unwrap_or(0);
    let next_low = spawn_max.max(entity_max) + 1;
    let new_guid = packed_creature_spawn_guid(creature_entry, next_low as u32);
    let mut spawned_entity_guid = None;
    let spawn_result: Result<(), String> = (|| {
        let tmpl = ctx
            .db
            .game_creature_template()
            .entry()
            .find(creature_entry)
            .ok_or_else(|| format!("no creature template for entry {creature_entry}"))?;
        let spawn = CreatureSpawn {
            guid: new_guid,
            entry: creature_entry,
            map_id,
            x: ORIGIN.0,
            y: ORIGIN.1,
            z: TERRAIN_FALLBACK,
            orientation: 0.0,
            respawn_at: crate::creatures::timer_never(ctx),
            despawn_at: crate::creatures::timer_never(ctx),
            movement_type: crate::creatures::MOVEMENT_IDLE,
            respawn_secs: 0,
            life_seq: 0,
            source_guid: 0,
            instance_id: 0,
        };
        let entity = build_creature_entity(ctx, &spawn, &tmpl, 0, 0);
        ctx.db.game_creature_spawn().insert(spawn);
        crate::creatures::insert_creature_entity(ctx, entity);
        spawned_entity_guid = Some(new_guid);
        Ok(())
    })();

    let spawned_z = spawned_entity_guid
        .and_then(|g| ctx.db.game_world_entity().guid().find(g))
        .map(|e| e.z);

    for id in inserted_ids {
        ctx.db.game_vmap_chunk().id().delete(id);
    }
    if let Some(g) = spawned_entity_guid {
        ctx.db.game_creature_spawn().guid().delete(g);
        crate::WorldEntity::delete(ctx, g);
    }
    if let Some(prior) = prior_vmap {
        if let Some(mut c) = ctx.db.game_config().id().find(0) {
            c.vmap_enabled = prior;
            ctx.db.game_config().id().update(c);
        }
    }

    log::info!(
        "debug_assert_floor_snap: ground_z={ground:?} floor_z={floor:?} snap_z={snapped:.2} spawned_z={spawned_z:?} (floor at z={FLOOR_Z:.1})"
    );

    spawn_result?;
    let floor = floor.ok_or_else(|| {
        "vmap::floor_z found nothing — the down-ray probe missed the synthetic floor".to_string()
    })?;
    if (floor - FLOOR_Z).abs() > 0.1 {
        return Err(format!(
            "floor_z returned {floor:.2}, expected ~{FLOOR_Z:.1}"
        ));
    }
    if ground.is_some() {
        return Err("ground_z unexpectedly found terrain at the ORIGIN — pick an unimported patch so this assert actually distinguishes model-floor from terrain".to_string());
    }
    if (snapped - FLOOR_Z).abs() > 0.1 {
        return Err(format!(
            "snap_z returned {snapped:.2} (terrain fallback {TERRAIN_FALLBACK:.1}), expected the model floor ~{FLOOR_Z:.1}"
        ));
    }
    let spawned_z =
        spawned_z.ok_or_else(|| "spawned creature entity vanished mid-probe".to_string())?;
    if (spawned_z - FLOOR_Z).abs() > 0.1 {
        return Err(format!(
            "creature spawn landed at z={spawned_z:.2} (spawn.z was {TERRAIN_FALLBACK:.1}), expected it snapped up to the model floor ~{FLOOR_Z:.1}"
        ));
    }
    Ok(())
}

#[reducer]
pub fn debug_assert_blink_clamp(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let entities = ctx.db.game_world_entity();
    let start = entities
        .guid()
        .find(character_guid)
        .ok_or_else(|| format!("no live entity for guid {character_guid}"))?;
    let map_id = start.map_id;

    const ORIGIN: (f32, f32, f32) = (1200.0, 1200.0, 0.0);
    const WALL_X: f32 = ORIGIN.0 + 10.0;
    const BLINK_YD: f32 = 20.0;

    let tris = [
        lyracore_shared::vmap::VmapTri {
            verts: [
                [WALL_X, ORIGIN.1 - 20.0, -50.0],
                [WALL_X, ORIGIN.1 + 20.0, -50.0],
                [WALL_X, ORIGIN.1 - 20.0, 50.0],
            ],
            class: lyracore_shared::vmap::TriClass::Wmo {
                group_id: 0,
                mogp_flags: 0,
            },
        },
        lyracore_shared::vmap::VmapTri {
            verts: [
                [WALL_X, ORIGIN.1 + 20.0, -50.0],
                [WALL_X, ORIGIN.1 + 20.0, 50.0],
                [WALL_X, ORIGIN.1 - 20.0, 50.0],
            ],
            class: lyracore_shared::vmap::TriClass::Wmo {
                group_id: 0,
                mogp_flags: 0,
            },
        },
    ];
    let blob = lyracore_shared::vmap::encode(&tris);

    let vmap_chunks = ctx.db.game_vmap_chunk();
    let mut written = std::collections::BTreeSet::new();
    let mut inserted_ids = Vec::new();
    for dx in [-40.0f32, 0.0, 40.0] {
        for dy in [-40.0f32, 0.0, 40.0] {
            let (Some(cx), Some(cy)) = (
                lyracore_shared::terrain::cell_index(WALL_X + dx),
                lyracore_shared::terrain::cell_index(ORIGIN.1 + dy),
            ) else {
                continue;
            };
            if !written.insert((cx, cy)) {
                continue;
            }
            let row = vmap_chunks.insert(crate::vmap::VmapChunk {
                id: 0,
                key: lyracore_shared::terrain::cell_key(map_id, cx, cy),
                map_id,
                cell_x: cx,
                cell_y: cy,
                blob: blob.clone(),
            });
            inserted_ids.push(row.id);
        }
    }

    let cfg = ctx.db.game_config();
    let prior_vmap = cfg.id().find(0).map(|c| c.vmap_enabled);
    match cfg.id().find(0) {
        Some(mut c) => {
            c.vmap_enabled = true;
            cfg.id().update(c);
        }
        None => {
            cfg.insert(ServerConfig {
                id: 0,
                xp_rate: 1.0,
                nav_enabled: true,
                hosts_instances: true,
                bots_idle: false,
                vmap_enabled: true,
                nav_coverage_enabled: false,
            });
        }
    }

    crate::world::teleport_player(
        ctx,
        character_guid,
        map_id,
        0,
        ORIGIN.0,
        ORIGIN.1,
        ORIGIN.2,
        0.0,
    );
    crate::spell::blink_forward(ctx, character_guid, BLINK_YD);

    let landed = ctx.db.game_world_entity().guid().find(character_guid);

    for id in inserted_ids {
        ctx.db.game_vmap_chunk().id().delete(id);
    }
    if let Some(prior) = prior_vmap {
        if let Some(mut c) = ctx.db.game_config().id().find(0) {
            c.vmap_enabled = prior;
            ctx.db.game_config().id().update(c);
        }
    }

    let landed = landed.ok_or_else(|| "character vanished mid-probe".to_string())?;
    let advanced = landed.x - ORIGIN.0;
    let clearance = WALL_X - landed.x;
    log::info!(
        "debug_assert_blink_clamp: landed ({:.2},{:.2}) advanced={advanced:.2}yd clearance={clearance:.2}yd (wall at x={WALL_X:.1})",
        landed.x, landed.y
    );
    if advanced <= 1.0 {
        return Err(format!(
            "Blink barely moved ({advanced:.2}yd advanced) — the collision clamp looks like it gave up instead of clamping to the hit point"
        ));
    }
    if clearance <= 0.0 {
        return Err(format!(
            "Blink landed AT OR PAST the wall plane (x={:.2} >= wall x={WALL_X:.1}, clearance={clearance:.2}yd) — the collision ray isn't clamping the destination",
            landed.x
        ));
    }
    Ok(())
}

#[reducer]
pub fn debug_assert_chase_stops_at_column(
    ctx: &ReducerContext,
    character_guid: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let start = ctx
        .db
        .game_world_entity()
        .guid()
        .find(character_guid)
        .ok_or_else(|| format!("no live entity for guid {character_guid}"))?;
    let map_id = start.map_id;

    const ORIGIN: (f32, f32, f32) = (1400.0, 1400.0, 0.0);
    const WALL_X: f32 = ORIGIN.0 + 15.0;
    const DEST_X: f32 = ORIGIN.0 + 30.0;
    const STEP_YD: f32 = 100.0;

    let tris = [
        lyracore_shared::vmap::VmapTri {
            verts: [
                [WALL_X, ORIGIN.1 - 20.0, -50.0],
                [WALL_X, ORIGIN.1 + 20.0, -50.0],
                [WALL_X, ORIGIN.1 - 20.0, 50.0],
            ],
            class: lyracore_shared::vmap::TriClass::Wmo {
                group_id: 0,
                mogp_flags: 0,
            },
        },
        lyracore_shared::vmap::VmapTri {
            verts: [
                [WALL_X, ORIGIN.1 + 20.0, -50.0],
                [WALL_X, ORIGIN.1 + 20.0, 50.0],
                [WALL_X, ORIGIN.1 - 20.0, 50.0],
            ],
            class: lyracore_shared::vmap::TriClass::Wmo {
                group_id: 0,
                mogp_flags: 0,
            },
        },
    ];
    let blob = lyracore_shared::vmap::encode(&tris);

    let vmap_chunks = ctx.db.game_vmap_chunk();
    let mut written = std::collections::BTreeSet::new();
    let mut inserted_ids = Vec::new();
    for dx in [-40.0f32, 0.0, 40.0] {
        for dy in [-40.0f32, 0.0, 40.0] {
            let (Some(cx), Some(cy)) = (
                lyracore_shared::terrain::cell_index(WALL_X + dx),
                lyracore_shared::terrain::cell_index(ORIGIN.1 + dy),
            ) else {
                continue;
            };
            if !written.insert((cx, cy)) {
                continue;
            }
            let row = vmap_chunks.insert(crate::vmap::VmapChunk {
                id: 0,
                key: lyracore_shared::terrain::cell_key(map_id, cx, cy),
                map_id,
                cell_x: cx,
                cell_y: cy,
                blob: blob.clone(),
            });
            inserted_ids.push(row.id);
        }
    }

    let cfg = ctx.db.game_config();
    let prior_vmap = cfg.id().find(0).map(|c| c.vmap_enabled);
    match cfg.id().find(0) {
        Some(mut c) => {
            c.vmap_enabled = true;
            cfg.id().update(c);
        }
        None => {
            cfg.insert(ServerConfig {
                id: 0,
                xp_rate: 1.0,
                nav_enabled: false,
                hosts_instances: true,
                bots_idle: false,
                vmap_enabled: true,
                nav_coverage_enabled: false,
            });
        }
    }

    let stepped = crate::nav::nav_step(
        ctx,
        map_id,
        start.instance_id,
        (ORIGIN.0, ORIGIN.1),
        (DEST_X, ORIGIN.1),
        STEP_YD,
        0.0,
        ORIGIN.2,
    );

    for id in inserted_ids {
        ctx.db.game_vmap_chunk().id().delete(id);
    }
    if let Some(prior) = prior_vmap {
        if let Some(mut c) = ctx.db.game_config().id().find(0) {
            c.vmap_enabled = prior;
            ctx.db.game_config().id().update(c);
        }
    }

    let advanced = stepped.0 - ORIGIN.0;
    let clearance = WALL_X - stepped.0;
    log::info!(
        "debug_assert_chase_stops_at_column: stepped ({:.2},{:.2}) advanced={advanced:.2}yd clearance={clearance:.2}yd (column at x={WALL_X:.1})",
        stepped.0, stepped.1
    );
    if advanced <= 1.0 {
        return Err(format!(
            "nav_step barely moved ({advanced:.2}yd advanced) — the collision gate looks like it gave up instead of stopping at the hit point"
        ));
    }
    if clearance <= 0.0 {
        return Err(format!(
            "nav_step stepped AT OR PAST the column plane (x={:.2} >= column x={WALL_X:.1}, clearance={clearance:.2}yd) — the collision ray isn't gating the step",
            stepped.0
        ));
    }
    Ok(())
}

#[reducer]
pub fn debug_assert_unreachable_goal_stops_at_wall(
    ctx: &ReducerContext,
    character_guid: u64,
) -> Result<(), String> {
    use lyracore_shared::nav::{
        obs_raise, sub_center, walk_set, OBS_BYTES, OBS_DIM, OBS_NONE, WALK_BYTES, WALK_DIM,
    };
    use lyracore_shared::terrain::{cell_index, cell_key};

    crate::helpers::require_operator(ctx)?;
    let start = ctx
        .db
        .game_world_entity()
        .guid()
        .find(character_guid)
        .ok_or_else(|| format!("no live entity for guid {character_guid}"))?;
    let map_id = start.map_id;

    let (cx, cy) = (
        cell_index(1400.0).ok_or("probe corner off map")?,
        cell_index(1400.0).ok_or("probe corner off map")?,
    );
    let wall_x = sub_center(cx, 32, WALK_DIM);
    let cur = (sub_center(cx, 8, WALK_DIM), sub_center(cy, 32, WALK_DIM));
    let dest = (wall_x, cur.1);

    let mut walk = vec![0xFFu8; WALK_BYTES];
    let mut obs = vec![OBS_NONE; OBS_BYTES];
    for ny in 0..WALK_DIM {
        walk_set(&mut walk, 32, ny, false);
    }
    for oy in 0..OBS_DIM {
        obs_raise(&mut obs, 0.0, 16, oy, 2.5);
    }

    let key = cell_key(map_id, cx, cy);
    let nav_chunks = ctx.db.game_nav_chunk();
    let prior_row = nav_chunks
        .key()
        .find(key)
        .map(|r| (r.base_z, r.walk.clone(), r.obs.clone()));
    if prior_row.is_some() {
        nav_chunks.key().delete(key);
    }
    nav_chunks.insert(crate::nav::NavChunk {
        key,
        map_id,
        cell_x: cx,
        cell_y: cy,
        base_z: 0.0,
        walk,
        obs,
    });

    let cfg = ctx.db.game_config();
    let prior_flags = cfg.id().find(0).map(|c| (c.nav_enabled, c.vmap_enabled));
    match cfg.id().find(0) {
        Some(mut c) => {
            c.nav_enabled = true;
            c.vmap_enabled = false;
            cfg.id().update(c);
        }
        None => {
            cfg.insert(ServerConfig {
                id: 0,
                xp_rate: 1.0,
                nav_enabled: true,
                hosts_instances: true,
                bots_idle: false,
                vmap_enabled: false,
                nav_coverage_enabled: false,
            });
        }
    }

    let stepped = crate::nav::nav_step(ctx, map_id, start.instance_id, cur, dest, 100.0, 0.0, 0.0);

    nav_chunks.key().delete(key);
    if let Some((base_z, walk, obs)) = prior_row {
        nav_chunks.insert(crate::nav::NavChunk {
            key,
            map_id,
            cell_x: cx,
            cell_y: cy,
            base_z,
            walk,
            obs,
        });
    }
    if let Some((nav, vmap)) = prior_flags {
        if let Some(mut c) = ctx.db.game_config().id().find(0) {
            c.nav_enabled = nav;
            c.vmap_enabled = vmap;
            ctx.db.game_config().id().update(c);
        }
    }

    let advanced = cur.0 - stepped.0;
    let clearance = stepped.0 - wall_x;
    log::info!(
        "debug_assert_unreachable_goal_stops_at_wall: stepped ({:.2},{:.2}) advanced={advanced:.2}yd clearance={clearance:.2}yd (wall at x={wall_x:.1})",
        stepped.0, stepped.1
    );
    if advanced <= 1.0 {
        return Err(format!(
            "nav_step barely moved ({advanced:.2}yd advanced) — the grid step gate looks like it gave up instead of truncating at the obs column"
        ));
    }
    if clearance <= 0.0 {
        return Err(format!(
            "nav_step stepped AT OR PAST the wall plane (x={:.2} <= wall x={wall_x:.1}, clearance={clearance:.2}yd) — the no-path fallback isn't step-gated",
            stepped.0
        ));
    }
    Ok(())
}

#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn debug_bench_los(
    ctx: &ReducerContext,
    map: u32,
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
    directions: u32,
    radius: f32,
    exact: bool,
) {
    use lyracore_shared::constants;
    use std::f32::consts::TAU;

    let (cx, cy) = ((min_x + max_x) / 2.0, (min_y + max_y) / 2.0);
    let half_diag = (((max_x - min_x) / 2.0).powi(2) + ((max_y - min_y) / 2.0).powi(2)).sqrt();
    let creatures: Vec<(f32, f32, f32)> =
        crate::helpers::entities_near(ctx, map, 0, cx, cy, half_diag)
            .into_iter()
            .filter(|e| {
                e.type_mask == constants::type_mask::CREATURE
                    && e.x >= min_x
                    && e.x <= max_x
                    && e.y >= min_y
                    && e.y <= max_y
            })
            .map(|e| (e.x, e.y, e.z))
            .collect();

    let mut queries = 0u32;
    let mut hits = 0u32;
    for (cx, cy, cz) in &creatures {
        for d in 0..directions {
            let angle = TAU * (d as f32) / (directions.max(1) as f32);
            let (bx, by, bz) = (cx + radius * angle.cos(), cy + radius * angle.sin(), *cz);
            let blocked = if exact {
                crate::vmap::los_ray(ctx, map, 0, [*cx, *cy, *cz], [bx, by, bz]).is_some()
            } else {
                !crate::nav::has_los(ctx, map, 0, (*cx, *cy, *cz), (bx, by, bz))
            };
            queries += 1;
            if blocked {
                hits += 1;
            }
        }
    }
    log::info!(
        "debug_bench_los map={map} exact={exact} creatures={} directions={directions} radius={radius:.1} queries={queries} hits={hits}",
        creatures.len()
    );
}

#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn debug_bench_collision_gate(
    ctx: &ReducerContext,
    map: u32,
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
    directions: u32,
    radius: f32,
) {
    use lyracore_shared::constants;
    use std::f32::consts::TAU;

    let (cx, cy) = ((min_x + max_x) / 2.0, (min_y + max_y) / 2.0);
    let half_diag = (((max_x - min_x) / 2.0).powi(2) + ((max_y - min_y) / 2.0).powi(2)).sqrt();
    let creatures: Vec<(f32, f32, f32)> =
        crate::helpers::entities_near(ctx, map, 0, cx, cy, half_diag)
            .into_iter()
            .filter(|e| {
                e.type_mask == constants::type_mask::CREATURE
                    && e.x >= min_x
                    && e.x <= max_x
                    && e.y >= min_y
                    && e.y <= max_y
            })
            .map(|e| (e.x, e.y, e.z))
            .collect();

    let mut queries = 0u32;
    let mut gated = 0u32;
    for (cx, cy, cz) in &creatures {
        for d in 0..directions {
            let angle = TAU * (d as f32) / (directions.max(1) as f32);
            let dest = (cx + radius * angle.cos(), cy + radius * angle.sin());
            let stepped = crate::nav::nav_step(ctx, map, 0, (*cx, *cy), dest, radius, 0.0, *cz);
            queries += 1;
            if (stepped.0 - dest.0).abs() > 0.01 || (stepped.1 - dest.1).abs() > 0.01 {
                gated += 1;
            }
        }
    }
    log::info!(
        "debug_bench_collision_gate map={map} creatures={} directions={directions} radius={radius:.1} queries={queries} gated={gated}",
        creatures.len()
    );
}

#[reducer]
pub fn debug_grant_default_actions(
    ctx: &ReducerContext,
    character_name: String,
) -> Result<(), String> {
    let c = crate::helpers::character_by_name(ctx, &character_name)
        .ok_or_else(|| format!("no character named {character_name}"))?;
    crate::action_bar::grant_createinfo_actions(ctx, c.guid, c.owner_identity, c.race, c.class);
    log::info!("debug_grant_default_actions: {} (guid {})", c.name, c.guid);
    Ok(())
}

#[reducer]
pub fn debug_backfill_go_grid(ctx: &ReducerContext) {
    let gos = ctx.db.game_gameobject();
    let all: Vec<u64> = gos.iter().map(|g| g.guid).collect();
    let mut n = 0u32;
    for guid in all {
        if let Some(mut g) = gos.guid().find(guid) {
            let (gx, gy) = lyracore_shared::spatial::grid_cell(g.x, g.y);
            if g.grid_x != gx
                || g.grid_y != gy
                || g.cell != lyracore_shared::spatial::grid_cell_id(gx, gy)
            {
                g.grid_x = gx;
                g.grid_y = gy;
                g.cell = lyracore_shared::spatial::grid_cell_id(gx, gy);
                g.update(ctx);
                n += 1;
            }
        }
    }
    log::info!("debug_backfill_go_grid: {n} rows stamped");
}

const REMOVAL_FIXTURE_INSTANCE: u64 = 509_295_100;
const REMOVAL_FIXTURE_INSIDE: (u32, f32, f32, f32) = (36, -16.4, -383.07, 61.78);
const REMOVAL_FIXTURE_HOME: (u32, f32, f32, f32, u32) = (0, -8949.95, -132.493, 83.5312, 12);

const REMOVAL_FIXTURE_CHARACTERS: [(u64, &str, u8, bool, bool); 6] = [
    (509_295_001, "Removalalpha", 0, true, true),
    (509_295_002, "Removalbravo", 0, true, true),
    (509_295_003, "Removalcharlie", 0, true, true),
    (509_295_004, "Removaldelta", 1, true, true),
    (509_295_005, "Removalecho", 0, false, true),
    (509_295_006, "Removalfoxtrot", 0, true, false),
];

#[reducer]
pub fn debug_stage_instance_removal_fixture(
    ctx: &ReducerContext,
    party_id: u64,
) -> Result<(), String> {
    use crate::{game_account, game_character};
    crate::helpers::require_operator(ctx)?;
    let instances = ctx.db.game_instance();
    instances.instance_id().delete(REMOVAL_FIXTURE_INSTANCE);
    instances.insert(crate::GameInstance {
        instance_id: REMOVAL_FIXTURE_INSTANCE,
        map_id: REMOVAL_FIXTURE_INSIDE.0,
        party_id,
        created_at: ctx.timestamp,
        last_empty_at_micros: 0,
        reset_requested: false,
    });
    let (home_map, home_x, home_y, home_z, home_zone) = REMOVAL_FIXTURE_HOME;
    for (guid, name, gm_level, inside, online) in REMOVAL_FIXTURE_CHARACTERS {
        crate::InstanceRemoval::delete(ctx, guid);
        crate::WorldEntity::delete(ctx, guid);
        let mut character = crate::helpers::character_by_guid(ctx, 1)
            .ok_or("the seeded Tester is the fixture template")?;
        character.guid = guid;
        character.name = name.to_string();
        character.account_id = guid;
        crate::auth::ensure_shadow_account(ctx, guid);
        let accounts = ctx.db.game_account();
        let mut account = accounts
            .id()
            .find(guid)
            .ok_or("the fixture Account exists")?;
        account.gm_level = gm_level;
        accounts.id().update(account);
        character.online = online;
        character.first_login = false;
        (character.home_map, character.home_x, character.home_y) = (home_map, home_x, home_y);
        (character.home_z, character.home_zone) = (home_z, home_zone);
        if inside {
            let (map_id, x, y, z) = REMOVAL_FIXTURE_INSIDE;
            (character.map_id, character.x, character.y, character.z) = (map_id, x, y, z);
            character.pending_instance_id = REMOVAL_FIXTURE_INSTANCE;
        } else {
            (character.map_id, character.x, character.y) = (home_map, home_x, home_y);
            (character.z, character.zone_id) = (home_z, home_zone);
            character.pending_instance_id = 0;
        }
        if crate::helpers::character_by_guid(ctx, guid).is_some() {
            ctx.db.game_character().guid().update(character);
        } else {
            ctx.db.game_character().insert(character);
        }
        super::debug_spawn_player_entity(ctx, guid)?;
    }
    Ok(())
}

#[reducer]
pub fn debug_hold_instance_removals(ctx: &ReducerContext) -> Result<(), String> {
    use crate::game_instance_removal;
    crate::helpers::require_operator(ctx)?;
    const DAY_MICROS: i64 = 24 * 60 * 60 * 1_000_000;
    let due = spacetimedb::Timestamp::from_micros_since_unix_epoch(
        ctx.timestamp.to_micros_since_unix_epoch() + DAY_MICROS,
    );
    let removals = ctx.db.game_instance_removal();
    for mut removal in removals.iter().collect::<Vec<_>>() {
        crate::InstanceRemoval::delete(ctx, removal.character_guid);
        removal.scheduled_id = 0;
        removal.scheduled_at = spacetimedb::ScheduleAt::Time(due);
        removal.insert(ctx);
    }
    Ok(())
}

#[reducer]
pub fn debug_expire_instance_removal(
    ctx: &ReducerContext,
    character_guid: u64,
) -> Result<(), String> {
    use crate::game_instance_removal;
    crate::helpers::require_operator(ctx)?;
    let removal = ctx
        .db
        .game_instance_removal()
        .character_guid()
        .find(character_guid)
        .ok_or_else(|| format!("{character_guid} has no running Instance Removal"))?;
    crate::instance::fire_instance_removal(ctx, &removal);
    Ok(())
}

#[reducer]
pub fn debug_logout_character(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let entity = ctx
        .db
        .game_world_entity()
        .guid()
        .find(character_guid)
        .ok_or_else(|| format!("{character_guid} is not in the world"))?;
    crate::world::remove_live_character(ctx, entity);
    Ok(())
}
