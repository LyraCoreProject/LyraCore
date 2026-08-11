//! Exact per-cell collision-triangle store + ray queries (issue #521, part of the #169 full-vmap
//! epic; design record `docs/decisions.md` §10). Builds on #520's codec/binning
//! (`lyracore_shared::vmap`): this slice adds the module-PRIVATE table the packed per-cell blobs
//! land in, the import reducers that fill it (mirroring `nav::import_nav_chunks`/`_append`), and
//! the module-side wrappers over `lyracore_shared::vmap::cast_ray` — LoS (WMO-class triangles
//! only) and collision (WMO + M2 doodads).
//!
//! Gated on `game_config.vmap_enabled` (default OFF, mirroring `nav_enabled`'s pre-244 posture):
//! off, or a world with no vmap data imported, both ray queries return `None` (clear) — the same
//! missing-chunk-means-unobstructed contract `nav` uses, so nothing regresses on an unimported map.

use lyracore_shared::terrain::cell_key;
use lyracore_shared::vmap::{decode, RayFlavor, VmapTri};
use spacetimedb::{reducer, table, ReducerContext, Table};

/// One SHARD of a terrain cell's packed collision triangles (`lyracore_shared::vmap` codec).
/// PRIVATE — no gateway binding needed (the headless reducer-add pattern, `docs/danger-zones.md`
/// §1.2: "a table binding is only needed if the gateway subscribes to or reads that table", and
/// nothing outside this module does; an operator inspects it with `spacetime sql`).
///
/// MULTIPLE rows can share the same `key` (unlike `game_nav_chunk`'s one-row-per-cell, this table
/// is NOT unique on `key`): a dense cell's triangle count is unbounded, but a single
/// `spacetime call` CLI argument is not (Linux caps one argv string around 128 KB), so the
/// importer shards an oversized cell into several independently-decodable blobs
/// (`importer/src/vmap.rs`'s `MAX_ROW_TRI_BYTES` split). `fetcher` below gathers every row for a
/// cell and concatenates their decoded triangles.
#[table(accessor = game_vmap_chunk, index(accessor = by_cell, btree(columns = [key])))]
pub struct VmapChunk {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    /// Same `cell_key` as `game_nav_chunk`/`game_terrain_chunk`: (map << 32) | (cell_x << 16) | cell_y.
    pub key: u64,
    pub map_id: u32,
    pub cell_x: u16,
    pub cell_y: u16,
    /// Already `lyracore_shared::vmap::encode`d by the importer — this side stores + decodes,
    /// never re-packs. One shard's worth of triangles, not necessarily the whole cell.
    pub blob: Vec<u8>,
}

// ===========================================================================================
//  Import reducers — same packed-string convention as `nav::import_nav_chunks`: rows `;`,
//  fields `,`, the blob hex. Unlike nav's fixed-size fields, the blob is the LAST field and
//  `splitn` keeps it intact even though hex never itself contains a `,`.
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

fn load_vmap_batch(ctx: &ReducerContext, packed: &str) -> Result<u32, String> {
    let chunks = ctx.db.game_vmap_chunk();
    let mut loaded = 0u32;
    for row in packed.split(';').filter(|r| !r.is_empty()) {
        let f: Vec<&str> = row.splitn(4, ',').collect();
        if f.len() != 4 {
            return Err(format!("vmap row needs 4 fields, got {}", f.len()));
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
        let blob = hex_decode(f[3])?;
        chunks.insert(VmapChunk {
            id: 0, // auto_inc
            key: cell_key(map_id, cell_x, cell_y),
            map_id,
            cell_x,
            cell_y,
            blob,
        });
        loaded += 1;
    }
    Ok(loaded)
}

/// Clear + load the first vmap batch (operator-only), mirroring `import_nav_chunks`.
#[reducer]
pub fn import_vmap_chunks(ctx: &ReducerContext, packed: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let chunks = ctx.db.game_vmap_chunk();
    let ids: Vec<u64> = chunks.iter().map(|c| c.id).collect();
    for id in ids {
        chunks.id().delete(id);
    }
    if load_vmap_batch(ctx, &packed)? == 0 {
        return Err("vmap import payload was empty".to_string());
    }
    Ok(())
}

/// Append a vmap batch WITHOUT the reset — a zone's cells span many `spacetime call` args.
#[reducer]
pub fn import_vmap_chunks_append(ctx: &ReducerContext, packed: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    load_vmap_batch(ctx, &packed)?;
    Ok(())
}

// ===========================================================================================
//  Runtime consumption (#521) — module-side wrappers over `lyracore_shared::vmap::cast_ray`,
//  gated on `game_config.vmap_enabled`.
// ===========================================================================================

use crate::game_config;

/// The 521 consumption gate. Missing config row = false (fresh DB stays baseline, mirrors
/// `nav::nav_enabled`).
pub fn vmap_enabled(ctx: &ReducerContext) -> bool {
    ctx.db
        .game_config()
        .id()
        .find(0)
        .map(|c| c.vmap_enabled)
        .unwrap_or(false)
}

/// Chunk fetch closure for `cast_ray`: one indexed scan + decode per crossed cell, gathering
/// every shard row for that cell key (a dense cell may be split across several — see
/// `VmapChunk`'s doc comment) and concatenating their decoded triangles. A shard decode failure
/// (shouldn't happen — only this module writes the table) is skipped rather than panicking a ray
/// query. `None` only when NO row at all exists for the cell (the missing-chunk contract).
fn fetcher(ctx: &ReducerContext, map_id: u32) -> impl FnMut(u16, u16) -> Option<Vec<VmapTri>> + '_ {
    move |cx, cy| {
        let key = cell_key(map_id, cx, cy);
        let mut tris = Vec::new();
        let mut any = false;
        for row in ctx.db.game_vmap_chunk().by_cell().filter(&key) {
            any = true;
            if let Ok(mut t) = decode(&row.blob) {
                tris.append(&mut t);
            }
        }
        any.then_some(tris)
    }
}

/// Exact line-of-sight ray: WMO-class triangles only (doodads/forests never block sight). `None`
/// when vmap is off, unimported, or the segment is clear; `Some(point)` = first-hit world point.
pub fn los_ray(ctx: &ReducerContext, map_id: u32, a: [f32; 3], b: [f32; 3]) -> Option<[f32; 3]> {
    if !vmap_enabled(ctx) {
        return None;
    }
    lyracore_shared::vmap::cast_ray(&mut fetcher(ctx, map_id), a, b, RayFlavor::Los)
}

/// Exact collision ray (WMO + M2 doodads) — the first-hit point for movement/reach clamps.
pub fn collision_ray(
    ctx: &ReducerContext,
    map_id: u32,
    a: [f32; 3],
    b: [f32; 3],
) -> Option<[f32; 3]> {
    if !vmap_enabled(ctx) {
        return None;
    }
    lyracore_shared::vmap::cast_ray(&mut fetcher(ctx, map_id), a, b, RayFlavor::Collision)
}

// ===========================================================================================
//  Model floor heights (#526) — a down-ray probe over the same collision-class triangle store,
//  so creature Z-placement/movement can stand on model floors (bridges, WMO interiors like
//  Deadmines' decks) that terrain's heightmap knows nothing about (`terrain::ground_z` only
//  samples the ADT MCVT grid — the walkable surface UNDER a bridge, not the deck itself).
// ===========================================================================================

/// How far above `probe_z` the down-ray starts — covers a probe that's already sitting exactly
/// on (or a hair under, from float drift) the floor it should detect.
const FLOOR_PROBE_UP_YD: f32 = 2.0;
/// How far below `probe_z` the down-ray searches — generous enough to reach the ground floor of
/// a multi-deck WMO interior (Deadmines) from a probe standing on an upper deck.
const FLOOR_PROBE_DOWN_YD: f32 = 200.0;

/// Topmost model-floor (WMO + M2 collision-class) triangle at or below `probe_z`, at (x, y) on
/// `map_id`. A single downward `collision_ray` cast from `probe_z + FLOOR_PROBE_UP_YD` to
/// `probe_z - FLOOR_PROBE_DOWN_YD`: `cast_ray` returns the NEAREST hit along the segment, which
/// for a downward segment is exactly the highest surface at or below the start — the "topmost
/// floor" this function promises. `None` when vmap is off, unimported, or no floor triangle lies
/// in the search range (mirrors every other vmap query's missing-chunk-means-nothing-found
/// contract — callers keep their current Z).
pub fn floor_z(ctx: &ReducerContext, map_id: u32, x: f32, y: f32, probe_z: f32) -> Option<f32> {
    let top = [x, y, probe_z + FLOOR_PROBE_UP_YD];
    let bottom = [x, y, probe_z - FLOOR_PROBE_DOWN_YD];
    collision_ray(ctx, map_id, top, bottom).map(|hit| hit[2])
}

// ===========================================================================================
//  Indoor/outdoor area-info (#527) — mangos parity: `VMapManager2::getAreaInfo` / the indoor
//  half of `Player::CheckAreaExploreAndOutdoor`. Same down-ray probe shape as `floor_z`, but
//  WMO-only (a doodad standing in an open field doesn't make the point "indoor") and reporting
//  the containing group's id + MOGP indoor bit instead of just the hit height.
// ===========================================================================================

/// Which WMO group (if any) contains `(x, y, z)` on `map_id`, and whether it's indoor. `None`
/// when vmap is off, unimported, or no WMO group is found in the probe range (outdoors/open
/// world) — mirrors every other vmap query's missing-chunk-means-nothing-found contract.
pub fn area_info(
    ctx: &ReducerContext,
    map_id: u32,
    x: f32,
    y: f32,
    z: f32,
) -> Option<lyracore_shared::vmap::AreaInfo> {
    if !vmap_enabled(ctx) {
        return None;
    }
    let top = [x, y, z + FLOOR_PROBE_UP_YD];
    let bottom = [x, y, z - FLOOR_PROBE_DOWN_YD];
    lyracore_shared::vmap::cast_ray_area(&mut fetcher(ctx, map_id), top, bottom)
}
