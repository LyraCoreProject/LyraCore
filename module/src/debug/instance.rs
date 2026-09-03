//! Dungeon-instancing fixture + operator levers (work-item 190 slices 2+3), plus the nav-probe and
//! one-time-backfill levers that accreted next to them (no section of their own).

use spacetimedb::{log, reducer, ReducerContext, Table};

// trait imports — VmapChunk/NavChunk are module-private, no crate-root glob re-export
use crate::nav::game_nav_chunk;
use crate::vmap::game_vmap_chunk;
use crate::{
    build_creature_entity, game_config, game_creature_spawn, game_creature_template,
    game_gameobject, game_instance, game_world_entity, CreatureSpawn, ServerConfig,
};

/// THE FIXTURE DUNGEON (190's done-when, headless, zero imports): resolve-or-create an instance of
/// the character's CURRENT map through the REAL production path (`resolve_or_create_instance`:
/// own binding → party's live instance → create, 5-cap enforced) and same-map-teleport them into
/// it in place. On the dev map this instances the SEEDED world — the chicken/wolf/trainer roster
/// copies in as the per-instance population, so two ungrouped characters get DISJOINT populations
/// (distinct instance ids, distinct creature guids) and two GROUPED characters land in the SAME
/// one. Call it twice for one character → the second call RESOLVES the binding (same id back).
/// Verify: `spacetime sql "SELECT * FROM game_instance"` / `"... FROM game_instance_binding"`,
/// and the log line below carries the id.
///
/// CLIENT-VIEW CAVEAT (dev-only, inherent to the same-map hop): a same-map instance switch is a
/// MSG_MOVE_TELEPORT_ACK teleport — the gateway's per-viewer `created` dedup set only resets on a
/// CROSS-map transfer (the enter_world resubscribe), so a live 1.12 client keeps rendering the
/// instance-0 objects it already saw (frozen — their relays are now instance-gated) until relog.
/// SERVER-side isolation is full either way (module gates + relay gates); wire assertions about
/// "sees only its own population" should therefore be made on a FRESH login after the switch
/// (login rebuilds into `pending_instance_id` with a fresh created set + initial-apply sweep).
/// The real dungeon flow (areatrigger portal) is cross-map and has no such caveat.
///
/// COST NOTE: the population copy covers EVERY spawn template on the character's map — on the
/// bare seeded dev node that's a handful of creatures; on a node with the full Elwynn/Westfall
/// import it's the whole ~2k-spawn map-0 roster per instance. Fixture-scale testing belongs on
/// the seeded node (or accept the copy cost knowingly).
#[reducer]
pub fn debug_create_fixture_instance(
    ctx: &ReducerContext,
    character_guid: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let player = crate::helpers::live_entity(ctx, character_guid)?;
    let map_id = player.map_id;
    let instance_id = crate::instance::resolve_or_create_instance(ctx, character_guid, map_id)?;
    // Same-map teleport in place: the in-place branch stamps entity + Character with the instance.
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

/// Place a character into an EXISTING instance on their current map (bypasses binding/resolve —
/// the raw placement lever for cap/isolation experiments). Same-map only: a cross-map placement
/// would land them at their current coords on a foreign map (garbage) — use the areatrigger path
/// or `debug_teleport` + `debug_enter_instance` in two steps for cross-map trips.
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

/// Force-reap an instance NOW (the full slice-3 teardown: population → encounter sweep → tick row
/// → bindings → row) without waiting out the 30min empty window — refuses if players are inside
/// (the teardown's own belt re-checks). The headless reap-verification lever.
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
    if ctx
        .db
        .game_world_entity()
        .iter()
        .any(|e| e.instance_id == instance_id && e.is_player())
    {
        return Err(format!(
            "instance {instance_id} has live players — reap refused"
        ));
    }
    crate::instance::teardown_instance(ctx, instance_id);
    Ok(())
}

/// Drive the player-facing `reset_instance` core by explicit guid (the CLI identity owns no
/// entity): flags every UNOCCUPIED instance the character may reset; the reaper (60s) tears each
/// down on its next firing. Returns Err when nothing was eligible, mirroring the player reducer.
#[reducer]
pub fn debug_reset_instance(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::instance::apply_reset_instances(ctx, character_guid).map(|n| {
        log::info!("debug_reset_instance: {n} instance(s) flagged for reset by {character_guid}");
    })
}

/// 241 spot-probe: log walkability + obstruction at (x, y) on `map`. The rasterizer's
/// server-side verification hook (nav has no client-visible readback until 243 wires it).
#[reducer]
pub fn debug_nav_probe(ctx: &ReducerContext, map: u32, x: f32, y: f32) {
    let walk = crate::nav::walkable(ctx, map, x, y);
    let obs = crate::nav::obstruction_top(ctx, map, x, y);
    let ground = crate::terrain::ground_z(ctx, map, x, y);
    log::info!(
        "nav probe ({x:.1},{y:.1}) map {map}: walkable={walk:?} obstruction_top={obs:?} ground_z={ground:?}"
    );
}

/// Toggle nav-grid consumption (`game_config.nav_enabled`, work-item 243) — upserts row 0 like
/// `debug_set_xp_rate`. OFF = pre-243 straight-line movement + wall-blind aggro/casts.
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

/// 243 spot-probe: log the nav-aware leg from (x0,y0) to (x1,y1) — the exact query
/// `nav_step` runs. Shows the string-pulled waypoints (detour) or the fast/fallback shape.
#[reducer]
pub fn debug_nav_leg(ctx: &ReducerContext, map: u32, x0: f32, y0: f32, x1: f32, y1: f32) {
    let enabled = crate::nav::nav_enabled(ctx);
    // LoS/collision endpoints stand on the ground like real units (falls back to z=0 off-slice).
    let z0 = crate::terrain::ground_z(ctx, map, x0, y0).unwrap_or(0.0);
    let z1 = crate::terrain::ground_z(ctx, map, x1, y1).unwrap_or(0.0);
    let step = crate::nav::nav_step(ctx, map, (x0, y0), (x1, y1), 4.0, 0.0, z0);
    let los = crate::nav::has_los(ctx, map, (x0, y0, z0), (x1, y1, z1));
    let raw = crate::nav::debug_find_leg(ctx, map, (x0, y0), (x1, y1));
    log::info!(
        "nav leg ({x0:.1},{y0:.1})->({x1:.1},{y1:.1}) map {map}: nav_enabled={enabled} first_step=({:.2},{:.2}) has_los={los} raw={raw}",
        step.0, step.1
    );
}

/// Toggle vmap ray consumption (`game_config.vmap_enabled`) — upserts row 0 like
/// `debug_set_nav_enabled`. OFF = every ray query returns clear (no vmap data consulted).
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

/// Toggle vmap-derived nav coverage in path planning (`game_config.nav_coverage_enabled`) —
/// upserts row 0 like `debug_set_vmap_enabled`. OFF = planning reads the imported nav grid alone.
/// This is the one-command rollback for a coverage rollout.
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

/// 521 spot-probe: cast an exact vmap ray from (x0,y0,z0) to (x1,y1,z1) on `map`, BOTH flavors —
/// LoS (WMO-only) and collision (WMO + M2 doodads). Analogue of `debug_nav_leg`; vmap has no
/// client-visible readback, so this is the server-side verification hook (decision). Logs
/// hit/miss + the first-hit point for each flavor so a doodad-vs-wall distinction is directly
/// observable: a doodad blocks `collision` but leaves `los` clear.
#[reducer]
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
    let enabled = crate::vmap::vmap_enabled(ctx, map);
    let a = [x0, y0, z0];
    let b = [x1, y1, z1];
    let (los, collision) = crate::vmap::probe_rays(ctx, map, a, b);
    log::info!(
        "vmap ray ({x0:.1},{y0:.1},{z0:.1})->({x1:.1},{y1:.1},{z1:.1}) map {map}: vmap_enabled={enabled} los={los:?} collision={collision:?}"
    );
}

/// 526 spot-probe: log the model floor at `(x, y)` for `map`, both the raw `vmap::floor_z` and
/// `terrain::snap_z`'s combined `max(terrain, floor)` — the server-side verification hook for a
/// bridge/WMO-interior coordinate where the two disagree (done-when: returns the model floor, not
/// the terrain underneath it). Analogue of `debug_nav_probe`.
#[reducer]
pub fn debug_floor_probe(ctx: &ReducerContext, map: u32, x: f32, y: f32, probe_z: f32) {
    let enabled = crate::vmap::vmap_enabled(ctx, map);
    let ground = crate::terrain::ground_z(ctx, map, x, y);
    let floor = crate::vmap::probe_floor_z(ctx, map, x, y, probe_z);
    let snapped = crate::terrain::snap_z(ctx, map, x, y, probe_z);
    log::info!(
        "floor probe ({x:.1},{y:.1}) map {map} probe_z={probe_z:.2}: vmap_enabled={enabled} ground_z={ground:?} floor_z={floor:?} snap_z={snapped:.2}"
    );
}

/// 527 spot-probe: log the WMO area-info at `(x, y, z)` for `map` — which group (if any) contains
/// the point and whether it's indoor. Analogue of `debug_vmap_ray`/`debug_floor_probe`; vmap has
/// no client-visible readback, so this is the server-side verification hook (decision §query
/// surface). Done-when's live check: a point inside the Northshire abbey logs `indoor=true` with a
/// group id, a point outside logs `indoor=None`/no group.
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

/// Done-when: server-side proof that a probe (and a creature spawn) stand on a model floor
/// instead of the terrain underneath it — the headless substitute for eyeballing a bridge/WMO
/// interior deck (e.g. Deadmines) in the live client. Same synthetic-rig shape as
/// `debug_assert_blink_clamp`/`debug_assert_chase_stops_at_column` (the exact
/// `lyracore_shared::vmap::encode` codec, a fixed ORIGIN in an unimported-terrain patch of the map
/// so `terrain::ground_z` is known-`None` there and can't accidentally agree with the synthetic
/// floor by coincidence), but the rig is a horizontal quad — a floor, not a vertical wall —
/// elevated `FLOOR_Z` above a PROBE that lands a hair (`PROBE_UNDERSHOOT_YD`, comfortably inside
/// `floor_z`'s upward search margin) below it, standing in for the ordinary case: imported spawn
/// data or a computed movement destination that's already close to the real surface but not
/// exact, because whatever produced it (client extraction, terrain interpolation) doesn't know
/// about the bridge deck. `floor_z`'s contract is "topmost floor AT OR BELOW the probe" — it is
/// not a levitation search, so this probe is deliberately realistic rather than far below the
/// deck. Asserts, in order: (1) `vmap::floor_z` finds it from a probe just under it, (2)
/// `terrain::snap_z` picks the floor over the (missing) terrain fallback — the `max(terrain,
/// floor)` integration the issue specifies, and (3) a creature spawned there through the REAL
/// `insert_creature_entity` chokepoint (spawn.z left at that same undershoot, exactly like an
/// imported spawn row with no floor awareness) lands on the floor, not at its spawn.z. Cleans up
/// the synthetic vmap rows, the spawn/entity rows and the `vmap_enabled` flag unconditionally
/// (cleanup runs before any assert can fail out), so it never leaves the database in a different
/// state than it found it.
#[reducer]
pub fn debug_assert_floor_snap(
    ctx: &ReducerContext,
    map_id: u32,
    creature_entry: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;

    const ORIGIN: (f32, f32) = (2600.0, 2600.0); // an unimported-terrain patch: ground_z is None
    const FLOOR_Z: f32 = 20.0; // the synthetic "bridge deck"
    const PROBE_UNDERSHOOT_YD: f32 = 0.5; // inside floor_z's FLOOR_PROBE_UP_YD margin
    const TERRAIN_FALLBACK: f32 = FLOOR_Z - PROBE_UNDERSHOOT_YD; // what a spawn/movement candidate near (but not exactly on) the deck carries

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
                id: 0, // auto_inc
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

    // (1) the raw floor probe, from above the deck looking down.
    let floor = crate::vmap::floor_z(ctx, map_id, ORIGIN.0, ORIGIN.1, FLOOR_Z + 5.0);
    // (2) the combined snap a movement/targeting call site actually uses.
    let snapped = crate::terrain::snap_z(ctx, map_id, ORIGIN.0, ORIGIN.1, TERRAIN_FALLBACK);
    let ground = crate::terrain::ground_z(ctx, map_id, ORIGIN.0, ORIGIN.1);

    // (3) spawn a creature AT the terrain-less fallback z (i.e. what an imported spawn row with
    // no floor awareness would carry) through the real insert chokepoint.
    const HIGHGUID_UNIT: u64 = 0xF130;
    let entry_hi = (creature_entry as u64) << 24;
    // Max over BOTH spawn rows AND live entities of this entry, one past it — never a fixed
    // low-24 suffix. A spawn-rows-only scan (or a fixed suffix) can re-collide with a lingering
    // entity from a prior crashed run and panic on insert (see debug/mod.rs's
    // debug_spawn_test_creature, live find 2026-07-10).
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
    let new_guid = (HIGHGUID_UNIT << 48) | entry_hi | next_low; // scratch guid, cleaned up below
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
        };
        let entity = build_creature_entity(&spawn, &tmpl, 0, 0);
        ctx.db.game_creature_spawn().insert(spawn);
        crate::creatures::insert_creature_entity(ctx, entity);
        spawned_entity_guid = Some(new_guid);
        Ok(())
    })();

    let spawned_z = spawned_entity_guid
        .and_then(|g| ctx.db.game_world_entity().guid().find(g))
        .map(|e| e.z);

    // Cleanup FIRST — never leave synthetic geometry, a spawn/entity row or a flipped flag behind,
    // even on assert failure.
    for id in inserted_ids {
        ctx.db.game_vmap_chunk().id().delete(id);
    }
    if let Some(g) = spawned_entity_guid {
        ctx.db.game_creature_spawn().guid().delete(g);
        ctx.db.game_world_entity().guid().delete(g);
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

/// Done-when: server-side proof that Blink's collision clamp lands SHORT of an obstacle
/// plane instead of teleporting through it — the headless substitute for a client screenshot at
/// the abbey columns. Wires a synthetic 2-triangle vertical wall straight out of
/// `lyracore_shared::vmap::encode` (the SAME codec `import_vmap_chunks` writes — not a shortcut
/// codec) directly into `game_vmap_chunk`, duplicated into every cell the probe's bounding box
/// could touch (mirrors the importer's AABB-duplication contract `vmap::cast_ray`'s doc comment
/// relies on, so the exact ~33.3yd cell boundary the fixed probe point happens to land on never
/// matters). Force-enables `vmap_enabled` for the probe and restores whatever it found after —
/// never leaves a live database in a different state than it found it. Teleports
/// `character_guid` to a fixed point 10yd short of the wall facing it (orientation 0, +x), then
/// calls `spell::blink_forward` — the EXACT function `E_BLINK`'s cast dispatch calls, not a
/// reimplementation of it — at Blink's real DBC radius (20yd, so an uncapped Blink would overshoot
/// the wall by 10yd if the clamp did nothing). Asserts the landed position moved meaningfully
/// forward (the clamp isn't just "gave up and stayed put") AND stopped short of the wall's x-plane
/// (the clamp isn't just "ignored the wall"). `Err` (never a panic) carries the actual coordinates
/// on failure — a debuggable assert, not a crash. Synthetic rows + the config flag are cleaned up
/// before returning either way.
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
    const WALL_X: f32 = ORIGIN.0 + 10.0; // 10yd ahead — inside Blink's 20yd reach
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
                id: 0, // auto_inc
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
        0.0, // orientation 0 = facing +x, straight at the wall
    );
    crate::spell::blink_forward(ctx, character_guid, BLINK_YD);

    let landed = ctx.db.game_world_entity().guid().find(character_guid);

    // Cleanup FIRST — never leave synthetic geometry or a flipped flag behind, even on assert failure.
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

/// Done-when: server-side proof that a COMMITTED `nav_step` move stops at a known column
/// instead of walking through it — the headless substitute for driving a live chase and eyeballing
/// the client. Same synthetic-wall rig as `debug_assert_blink_clamp` (the exact
/// `lyracore_shared::vmap::encode` codec, duplicated across every cell the wall's AABB could
/// touch), but probes `nav::nav_step` — the ONE choke point every movement pass (chase/return/
/// wander/flee/pet-follow) calls through — instead of `blink_forward`. Doesn't move
/// `character_guid`'s own entity: it reads that guid only to resolve a live map/instance to test
/// against, then calls `nav_step` directly as a pure query (`cur` fixed short of the wall, `dest`
/// far beyond it, `max_step` large enough to reach `dest` in one step if nothing blocked it).
/// Asserts the returned step advanced meaningfully forward (the gate isn't just "gave up") AND
/// stopped short of the wall's x-plane (the gate isn't just "ignored the wall"). Force-enables
/// `vmap_enabled` for the probe and restores whatever it found after — never leaves the database in
/// a different state than it found it.
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
    const WALL_X: f32 = ORIGIN.0 + 15.0; // the "column"
    const DEST_X: f32 = ORIGIN.0 + 30.0; // 30yd forward — well past the wall if the gate did nothing
    const STEP_YD: f32 = 100.0; // one committed step can cover the whole leg if nothing blocked it

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
                id: 0, // auto_inc
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
        (ORIGIN.0, ORIGIN.1),
        (DEST_X, ORIGIN.1),
        STEP_YD,
        0.0,
        ORIGIN.2,
    );

    // Cleanup FIRST — never leave synthetic geometry or a flipped flag behind, even on assert failure.
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

/// Proves that an unreachable grid-tier goal cannot make a unit cross a synthetic wall.
/// The probe restores the nav row and consumption flags it temporarily replaces.
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

    // Wall down the walk-nx=32 line (obs ox=16) of the cell at (1400, 1400) — same probe corner
    // All positions derive from the cell so nothing straddles a cell boundary.
    let (cx, cy) = (
        cell_index(1400.0).ok_or("probe corner off map")?,
        cell_index(1400.0).ok_or("probe corner off map")?,
    );
    let wall_x = sub_center(cx, 32, WALK_DIM);
    let cur = (sub_center(cx, 8, WALK_DIM), sub_center(cy, 32, WALK_DIM)); // ~12.5 yd before the wall
    let dest = (wall_x, cur.1); // ON the wall line — unwalkable goal, `find_leg` returns None

    let mut walk = vec![0xFFu8; WALK_BYTES];
    let mut obs = vec![OBS_NONE; OBS_BYTES];
    for ny in 0..WALK_DIM {
        walk_set(&mut walk, 32, ny, false);
    }
    for oy in 0..OBS_DIM {
        obs_raise(&mut obs, 0.0, 16, oy, 20.0);
    }

    // Stash-and-replace any REAL nav row at the probe cell (dev imports could cover it).
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

    let stepped = crate::nav::nav_step(ctx, map_id, cur, dest, 100.0, 0.0, 0.0);

    // Cleanup FIRST — never leave synthetic nav data or flipped flags behind, even on assert failure.
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

    let advanced = cur.0 - stepped.0; // the step walks toward -x (higher walk index = lower coord)
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

/// Benchmark harness: run `directions` synthetic LoS queries around EVERY real creature
/// (`type_mask::CREATURE`) inside `[min_x,max_x]×[min_y,max_y]` on `map` — one probe segment per
/// creature per compass direction at `radius` yd, i.e. exactly the (creature × in-range-candidate)
/// shape `best_aggro_target` walks on a sense tick (`sense.rs`), scaled to a box of real,
/// live-imported creature positions instead of synthetic ones. `exact=false` times the grid
/// `nav::has_los` baseline (§8); `exact=true` times the new `vmap::los_ray` (§10). Both flavors
/// query the SAME segments so the comparison is apples-to-apples. Deliberately a single reducer
/// call per batch: total wall time for the whole batch is readable off the node's own
/// `spacetime_reducer_plus_query_duration_sec{reducer="debug_bench_los"}` metric (docs/testing
/// has no in-module clock — `std::time::Instant` does not exist on `wasm32-unknown-unknown`), so
/// this reducer does no timing itself — it only produces the query volume and logs a checksum
/// (`hits`) so two runs can be diffed for correctness, not just speed.
#[reducer]
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

    // Grid-indexed radius scan (open world, instance 0) covering the box, not a raw table scan —
    // `entities_near` is the shardable/scale-safe fetch (`tripwires::no_unwhitelisted_raw_spatial_scans`);
    // the exact box + creature-type filter below is a cheap in-memory pass over its (small) result.
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
                crate::vmap::los_ray(ctx, map, [*cx, *cy, *cz], [bx, by, bz]).is_some()
            } else {
                !crate::nav::has_los(ctx, map, (*cx, *cy, *cz), (bx, by, bz))
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

/// Perf-risk companion to `debug_bench_los`: `nav_step`'s collision gate (`vmap::collision_ray`)
/// runs on every COMMITTED movement step (chase/return/wander/flee/pet-follow — the 500ms tick),
/// not the 4s sense tick the LoS benchmark measured, so it pays the exact-ray cost 8x more often per mover. Same
/// shape as `debug_bench_los` — one probe per real creature per compass direction, `radius` yd out
/// — but calls `nav::nav_step` (the actual committed-step choke point) instead of a raw LoS ray, so
/// the query volume this produces is directly comparable against `debug_bench_los`'s numbers on the
/// SAME box: if this reducer's wall time isn't roughly `directions`× cheaper per creature than
/// `debug_bench_los(exact=true)`, the per-tick multiplication the review flagged is real and
/// needs the "cheap PK existence check" fallback the issue text proposed. `gated` counts how many
/// probes the ray actually truncated (checksum, not just speed, so two runs can be diffed for
/// correctness too).
#[reducer]
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
            let stepped = crate::nav::nav_step(ctx, map, (*cx, *cy), dest, radius, 0.0, *cz);
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

/// Backfill the imported default action bar (work-items 110/212) onto an EXISTING character —
/// chars created before the `game_createinfo_action` import have no `game_player_action` rows and
/// fall back to the gateway's known-spells synth (which slots passives and misses the stance
/// pages). Idempotent per button (the grant skips occupied ones). Takes effect next login.
#[reducer]
pub fn debug_grant_default_actions(
    ctx: &ReducerContext,
    character_name: String,
) -> Result<(), String> {
    // REFUSE verdict — harness writers get the same fence as production ones.
    let c = crate::helpers::character_by_name(ctx, &character_name)
        .ok_or_else(|| format!("no character named {character_name}"))?;
    crate::action_bar::grant_createinfo_actions(ctx, c.guid, c.owner_identity, c.race, c.class);
    log::info!("debug_grant_default_actions: {} (guid {})", c.name, c.guid);
    Ok(())
}

/// One-time backfill (246): stamp grid_x/grid_y on every existing gameobject row from its (x, y)
/// — imported/seeded rows predate the columns. Idempotent.
#[reducer]
pub fn debug_backfill_go_grid(ctx: &ReducerContext) {
    let gos = ctx.db.game_gameobject();
    let all: Vec<u64> = gos.iter().map(|g| g.guid).collect();
    let mut n = 0u32;
    for guid in all {
        if let Some(mut g) = gos.guid().find(guid) {
            let (gx, gy) = lyracore_shared::spatial::grid_cell(g.x, g.y);
            // `cell` too — see `debug_regrid`'s note; a migrated row's grid is already right.
            if g.grid_x != gx
                || g.grid_y != gy
                || g.cell != lyracore_shared::spatial::grid_cell_id(gx, gy)
            {
                g.grid_x = gx;
                g.grid_y = gy;
                g.cell = lyracore_shared::spatial::grid_cell_id(gx, gy);
                gos.guid().update(g);
                n += 1;
            }
        }
    }
    log::info!("debug_backfill_go_grid: {n} rows stamped");
}
