//! Dungeon-instancing fixture + operator levers (work-item 190 slices 2+3), plus the nav-probe and
//! one-time-backfill levers that accreted next to them (no section of their own).

use spacetimedb::{log, reducer, ReducerContext, Table};

use crate::{game_config, game_gameobject, game_instance, game_world_entity, ServerConfig};

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
    let step = crate::nav::nav_step(ctx, map, (x0, y0), (x1, y1), 4.0, 0.0);
    // LoS endpoints stand on the ground like real units (falls back to z=0 off-slice).
    let z0 = crate::terrain::ground_z(ctx, map, x0, y0).unwrap_or(0.0);
    let z1 = crate::terrain::ground_z(ctx, map, x1, y1).unwrap_or(0.0);
    let los = crate::nav::has_los(ctx, map, (x0, y0, z0), (x1, y1, z1));
    let raw = crate::nav::debug_find_leg(ctx, map, (x0, y0), (x1, y1));
    log::info!(
        "nav leg ({x0:.1},{y0:.1})->({x1:.1},{y1:.1}) map {map}: nav_enabled={enabled} first_step=({:.2},{:.2}) has_los={los} raw={raw}",
        step.0, step.1
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
    // REFUSE verdict (issue #30) — harness writers get the same fence as production ones.
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
            if g.grid_x != gx || g.grid_y != gy {
                g.grid_x = gx;
                g.grid_y = gy;
                gos.guid().update(g);
                n += 1;
            }
        }
    }
    log::info!("debug_backfill_go_grid: {n} rows stamped");
}
