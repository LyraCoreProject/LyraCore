//! Nav grid: walkability + line-of-sight obstruction data (work-item 241, decision #8).
//!
//! One row per terrain MCNK cell that has ANY obstruction, steep slope, or hole — fully-clear
//! cells emit no row, so a missing chunk means "no obstacles known here" and every reader keeps
//! today's straight-line behavior (the same off-slice fallback contract as `game_terrain_chunk`).
//! Imported from the operator's own client geometry by `importer --nav` (licensing firewall).
//! Blob layout + cell math live in `lyracore_shared::nav` (one copy, unit-tested there; the importer
//! rasterizes through the SAME functions).

use lyracore_shared::nav::{self, OBS_BYTES, OBS_DIM, WALK_BYTES, WALK_DIM};
use lyracore_shared::terrain::{cell_index, cell_key};
use spacetimedb::{reducer, table, ReducerContext, Table};

/// One terrain cell's nav data. `walk` is the 64×64 walkability bitmask; `obs` is the 32×32
/// obstruction-height grid (may be EMPTY when the cell only has slope/hole unwalkability and
/// nothing tall enough to block sight). [static]
#[table(accessor = game_nav_chunk, public)]
pub struct NavChunk {
    /// (map << 32) | (cell_x << 16) | cell_y — same `cell_key` as `game_terrain_chunk`.
    #[primary_key]
    pub key: u64,
    pub map_id: u32,
    pub cell_x: u16,
    pub cell_y: u16,
    /// Obs heights quantize against this (the cell's min terrain corner height).
    pub base_z: f32,
    /// 512-byte walkability bitmask (`lyracore_shared::nav::walk_get`).
    pub walk: Vec<u8>,
    /// 1024-byte obstruction grid (`lyracore_shared::nav::obs_top`), or empty = all-clear.
    pub obs: Vec<u8>,
}

/// Is (x, y) standable? `None` = no nav chunk here (un-imported or fully clear) — callers keep
/// their current behavior, exactly like `terrain::ground_z`'s off-slice contract. One PK find.
pub fn walkable(ctx: &ReducerContext, map_id: u32, x: f32, y: f32) -> Option<bool> {
    let (cx, cy) = (cell_index(x)?, cell_index(y)?);
    let chunk = ctx
        .db
        .game_nav_chunk()
        .key()
        .find(cell_key(map_id, cx, cy))?;
    let (nx, ny) = (
        nav::sub_index(x, cx, WALK_DIM)?,
        nav::sub_index(y, cy, WALK_DIM)?,
    );
    Some(nav::walk_get(&chunk.walk, nx, ny))
}

/// Obstruction top-Z over (x, y), or `None` when the column (or the whole cell) is clear.
pub fn obstruction_top(ctx: &ReducerContext, map_id: u32, x: f32, y: f32) -> Option<f32> {
    let (cx, cy) = (cell_index(x)?, cell_index(y)?);
    let chunk = ctx
        .db
        .game_nav_chunk()
        .key()
        .find(cell_key(map_id, cx, cy))?;
    let (ox, oy) = (
        nav::sub_index(x, cx, OBS_DIM)?,
        nav::sub_index(y, cy, OBS_DIM)?,
    );
    nav::obs_top(&chunk.obs, chunk.base_z, ox, oy)
}

// ===========================================================================================
//  Import reducers — blobs ride the packed-string path like terrain (`Vec<u8>` has no
//  spacetime-SQL literal): rows `;`, fields `,`, blobs hex. An EMPTY walk field means
//  "all 4096 bits set" (the common some-obs-but-fully-walkable cell, e.g. a low fence).
// ===========================================================================================

fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("odd-length hex blob".to_string());
    }
    (0..s.len() / 2)
        .map(|i| {
            u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|_| format!("bad hex at {i}"))
        })
        .collect()
}

fn load_nav_batch(ctx: &ReducerContext, packed: &str) -> Result<u32, String> {
    let chunks = ctx.db.game_nav_chunk();
    let mut loaded = 0u32;
    for row in packed.split(';').filter(|r| !r.is_empty()) {
        let f: Vec<&str> = row.split(',').collect();
        if f.len() != 6 {
            return Err(format!("nav row needs 6 fields, got {}", f.len()));
        }
        let map_id = f[0]
            .parse::<u32>()
            .map_err(|_| format!("bad u32: {}", f[0]))?;
        let cell_x = f[1]
            .parse::<u16>()
            .map_err(|_| format!("bad u16: {}", f[1]))?;
        let cell_y = f[2]
            .parse::<u16>()
            .map_err(|_| format!("bad u16: {}", f[2]))?;
        let base_z = f[3]
            .parse::<f32>()
            .map_err(|_| format!("bad f32: {}", f[3]))?;
        let walk = if f[4].is_empty() {
            vec![0xFF; WALK_BYTES]
        } else {
            hex_decode(f[4])?
        };
        let obs = hex_decode(f[5])?;
        if walk.len() != WALK_BYTES || !(obs.is_empty() || obs.len() == OBS_BYTES) {
            return Err(format!(
                "nav row blob sizes wrong: walk={} obs={}",
                walk.len(),
                obs.len()
            ));
        }
        chunks.insert(NavChunk {
            key: cell_key(map_id, cell_x, cell_y),
            map_id,
            cell_x,
            cell_y,
            base_z,
            walk,
            obs,
        });
        loaded += 1;
    }
    Ok(loaded)
}

/// Clear + load the first nav batch (operator-only), mirroring `import_terrain_chunks`.
#[reducer]
pub fn import_nav_chunks(ctx: &ReducerContext, packed: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let chunks = ctx.db.game_nav_chunk();
    let keys: Vec<u64> = chunks.iter().map(|c| c.key).collect();
    for k in keys {
        chunks.key().delete(k);
    }
    if load_nav_batch(ctx, &packed)? == 0 {
        return Err("nav import payload was empty".to_string());
    }
    Ok(())
}

/// Append a nav batch WITHOUT the reset — a zone spans many `spacetime call` args.
#[reducer]
pub fn import_nav_chunks_append(ctx: &ReducerContext, packed: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    load_nav_batch(ctx, &packed)?;
    Ok(())
}

// ===========================================================================================
//  Runtime consumption (work-item 243) — the module-side wrappers over `lyracore_shared::nav`'s
//  pure queries, gated on `game_config.nav_enabled` (default OFF; the 244 benchmark + live
//  verify flips it). Every wrapper degrades to the pre-243 straight-line behavior when the
//  flag is off, the chunk is missing, or no path exists.
// ===========================================================================================

use crate::game_config;
use lyracore_shared::nav::NavCellData;

/// The 243 consumption gate. Missing config row = false (fresh DB stays baseline).
pub fn nav_enabled(ctx: &ReducerContext) -> bool {
    ctx.db
        .game_config()
        .id()
        .find(0)
        .map(|c| c.nav_enabled)
        .unwrap_or(false)
}

/// Chunk fetch closure for the shared queries — one PK find per crossed cell, blobs cloned
/// (a few KB; the shared layer memoizes per query).
fn fetcher(ctx: &ReducerContext, map_id: u32) -> impl FnMut(u16, u16) -> Option<NavCellData> + '_ {
    move |cx, cy| {
        ctx.db
            .game_nav_chunk()
            .key()
            .find(cell_key(map_id, cx, cy))
            .map(|c| NavCellData {
                base_z: c.base_z,
                walk: c.walk,
                obs: c.obs,
            })
    }
}

/// Line of sight for gameplay checks (aggro sensing, hostile casts, engage, swing gate, caster
/// hold-range). Issue #523 (decision #10 §10, `#522`'s benchmark PASS): when exact vmap data is
/// consuming (`vmap::vmap_enabled`), this defers to the exact WMO-only ray
/// (`vmap::los_ray`) instead of the coarse grid raymarch — a hostile behind a real wall is
/// "unseen" at the true wall plane rather than the nearest whole nav cell. Falls back to the grid
/// (243's `nav_enabled` gate) when vmap is off, which itself falls back to `true` ("walls don't
/// exist") when nav is off too — the same three-tier degrade `blink_forward` uses. NOTE: this
/// degrade is per-FLAG, not per-cell — `vmap_enabled` on with zero `game_vmap_chunk` rows for a
/// map (the standard `import-world.sh` pipeline never populates it) reads as "every ray on that
/// map is clear", not "fall back to the grid". `vmap_enabled` therefore defaults OFF
/// (`config.rs`) and is a per-map operator opt-in once `importer --vmap` has actually populated
/// the map's chunks — never flip the default without a wired import step.
pub fn has_los(ctx: &ReducerContext, map_id: u32, a: (f32, f32, f32), b: (f32, f32, f32)) -> bool {
    if crate::vmap::vmap_enabled(ctx, map_id) {
        return crate::vmap::los_ray(ctx, map_id, [a.0, a.1, a.2], [b.0, b.1, b.2]).is_none();
    }
    if !nav_enabled(ctx) {
        return true;
    }
    lyracore_shared::nav::has_los(&mut fetcher(ctx, map_id), a, b)
}

/// Expansion cap for one chase leg's A*. An UNREACHABLE goal burns the whole budget before
/// falling back (242 note), so this stays small — a 500 ms leg only needs to round a corner,
/// not solve the zone. 244 owns the measured tuning.
const LEG_MAX_EXPANSIONS: u32 = 4096;

/// Nav-aware replacement for the `chase_step(cur → dest)` call sites: step toward the first
/// string-pulled waypoint of a walkable path instead of straight through geometry. Falls back
/// to the plain straight step when nav is off, the direct line is already clear (the fast path
/// returns `[dest]`), or no path exists (goal inside geometry / off-grid / walled-in) — but
/// EVERY result then passes through `step_gate`, the one commit-point gate (#525 + #102), so
/// the no-path fallback grinds to a stop at the first obstruction instead of beelining through
/// a wall (#102 (c)). `z` is the mover's current height (near-ground horizontal segment; walls
/// are vertical planes so the exact height barely matters).
///
/// PERF NOTE: the gate fires on every committed movement step for chase/return/wander/flee/
/// pet-follow — the ~500ms movement tick, 8x more often than the ~4s sense tick `#522`'s benchmark
/// (`debug_bench_los`) measured `vmap::los_ray` against. `debug_bench_collision_gate`
/// (`debug/instance.rs`) is the companion harness for THIS path — run it against the same box
/// before flipping `vmap_enabled` on a populated map, and compare its wall time to
/// `debug_bench_los(exact=true)`'s on that box.
pub fn nav_step(
    ctx: &ReducerContext,
    map_id: u32,
    cur: (f32, f32),
    dest: (f32, f32),
    max_step: f32,
    stop_dist: f32,
    z: f32,
) -> (f32, f32) {
    let stepped = if !nav_enabled(ctx) {
        crate::creatures::chase_step(cur.0, cur.1, dest.0, dest.1, max_step, stop_dist)
    } else {
        match lyracore_shared::nav::find_leg(&mut fetcher(ctx, map_id), cur, dest, LEG_MAX_EXPANSIONS)
        {
            // Real detour: head for the first corner, no stop-short (that's for the final approach).
            Some(path) if path.len() > 1 || path[0] != dest => {
                let wp = path[0];
                crate::creatures::chase_step(cur.0, cur.1, wp.0, wp.1, max_step, 0.0)
            }
            // Fast path ([dest]): the direct line is already verified walkable.
            Some(_) => {
                crate::creatures::chase_step(cur.0, cur.1, dest.0, dest.1, max_step, stop_dist)
            }
            // #102 (c): no path (goal inside geometry, off-grid, or walled-in start) — still AIM
            // the straight step a wall-less mob would take, because `step_gate` below truncates it
            // at the first obstruction: an unreachable goal stops at the wall instead of crossing
            // it. Holding outright here would freeze a chaser whenever its aim point (a lead past
            // the target, a wander roll) lands inside a wall's conservative margin.
            None => {
                crate::creatures::chase_step(cur.0, cur.1, dest.0, dest.1, max_step, stop_dist)
            }
        }
    };
    step_gate(ctx, map_id, cur, stepped, z)
}

/// The margin `nav_step`'s step gate stops short of a ray hit by — same value
/// `blink_forward`'s clamp uses (`BLINK_CLEARANCE_YD`), so a walked stop and a Blink stop land at
/// the same distance from a wall.
const GATE_CLEARANCE_YD: f32 = 1.0;

/// #525 + #102 (b): THE one commit-point gate — every `nav_step` result passes through here, so
/// a committed move never crosses geometry a data layer knows about. Tiered per-FLAG like
/// `has_los` (see that function's NOTE on the degrade contract):
/// - vmap consuming → the exact collision ray (`vmap::collision_ray`, WMO + M2 doodads), #525's
///   original gate;
/// - grid only (`nav_enabled`) → the obs-grid raymarch (`lyracore_shared::nav::step_hit`; the
///   mover's OWN obs column is exempt so a wall-hugger standing in the blob's conservative
///   margin isn't held in place);
/// - both off → no gate (the pre-243 straight-line world).
/// Either tier truncates the step to `GATE_CLEARANCE_YD` short of the first hit — mirroring
/// `blink_forward`'s clamp (`spell::cast::targeting`) — and holds the mover at `cur` when the
/// hit is inside the margin (a stuck-in-place creature, never a wall-clip). A missing chunk in
/// either layer reads as clear (the standard off-slice contract), so nothing changes on an
/// unimported map.
fn step_gate(
    ctx: &ReducerContext,
    map_id: u32,
    cur: (f32, f32),
    stepped: (f32, f32),
    z: f32,
) -> (f32, f32) {
    if stepped == cur {
        return stepped;
    }
    let hit = if crate::vmap::vmap_enabled(ctx, map_id) {
        crate::vmap::collision_ray(ctx, map_id, [cur.0, cur.1, z], [stepped.0, stepped.1, z])
            .map(|h| (h[0], h[1]))
    } else if nav_enabled(ctx) {
        lyracore_shared::nav::step_hit(
            &mut fetcher(ctx, map_id),
            (cur.0, cur.1, z),
            (stepped.0, stepped.1, z),
        )
    } else {
        None
    };
    match hit {
        Some((hx, hy)) => {
            let (dx, dy) = (hx - cur.0, hy - cur.1);
            let hit_dist = (dx * dx + dy * dy).sqrt();
            let land_dist = hit_dist - GATE_CLEARANCE_YD;
            if land_dist <= 0.0 {
                cur // hit inside the clearance margin — hold in place
            } else {
                (cur.0 + dx / hit_dist * land_dist, cur.1 + dy / hit_dist * land_dist)
            }
        }
        None => stepped, // clear (or no data under this segment) — the plain step stands
    }
}

/// Probe-only: the raw `find_leg_ex` outcome as a printable summary (waypoints + expansions).
pub fn debug_find_leg(
    ctx: &ReducerContext,
    map_id: u32,
    from: (f32, f32),
    to: (f32, f32),
) -> String {
    match lyracore_shared::nav::find_leg_ex(&mut fetcher(ctx, map_id), from, to, LEG_MAX_EXPANSIONS)
    {
        None => "None(fallback straight)".to_string(),
        Some((path, expanded, complete)) => {
            let pts: Vec<String> = path
                .iter()
                .map(|p| format!("({:.1},{:.1})", p.0, p.1))
                .collect();
            format!(
                "path[{}] expansions={} complete={} {}",
                path.len(),
                expanded,
                complete,
                pts.join(" ")
            )
        }
    }
}
