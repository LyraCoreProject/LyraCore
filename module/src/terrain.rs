//! Terrain heightmap: ground-Z lookups for creature + bot movement (work-items 172/173).
//!
//! One row per ADT MCNK cell (33.33 yd square), imported from the operator's own client by
//! `importer --terrain` (same licensing firewall as `--dbc`: extracted data never enters the
//! repo). The module has no filesystem, so terrain lives here as an indexed table — a
//! "map/nav re-homeing" shape. Coverage follows
//! the content slice; `ground_z` returns `None` off-slice and every caller keeps its current
//! Z there, so unimported areas behave exactly as before this table existed.

use lyracore_shared::terrain::{cell_index, cell_key};
use spacetimedb::{reducer, table, ReducerContext, Table};

/// One MCNK cell's height data. `heights` is the raw 145-float MCVT layout (9 outer + 8 inner
/// per row pair, interleaved), already rebased to ABSOLUTE world Z by the importer (MCVT is
/// relative to the chunk's header Z). `ground_z` reads only the 9×9 outer corners; the 8×8
/// centers ride along for a later triangle-interpolation upgrade. [static]
#[table(accessor = game_terrain_chunk, public)]
pub struct TerrainChunk {
    /// (map << 32) | (cell_x << 16) | cell_y — see `cell_key`.
    #[primary_key]
    pub key: u64,
    pub map_id: u32,
    pub cell_x: u16,
    pub cell_y: u16,
    /// 145 absolute heights in MCVT interleaved order.
    pub heights: Vec<f32>,
    /// MCLQ water surface height; NaN-free: `has_liquid == false` ⇒ 0.0 and meaningless.
    pub liquid_level: f32,
    pub has_liquid: bool,
    /// MCNK hole bitmask (16 sub-quads) — carried for a later "don't wander into a hole" pass.
    pub holes: u32,
    /// AreaTable id for the cell (MCNK header) — future zone/area resolution.
    pub area_id: u32,
}

/// Ground height at (x, y), or `None` where no terrain is imported. ONE indexed PK find plus
/// arithmetic — this sits in the 0.5s movement tick, so it must never scan (work-item 173 /
/// de-risking spike #5). The cell math + bilinear interpolation live in
/// `lyracore_shared::terrain` (unit-tested there; the importer's self-check shares the SAME code,
/// so import-time verification covers this exact function).
pub fn ground_z(ctx: &ReducerContext, map_id: u32, x: f32, y: f32) -> Option<f32> {
    let (cx, cy) = (cell_index(x)?, cell_index(y)?);
    let chunk = ctx
        .db
        .game_terrain_chunk()
        .key()
        .find(cell_key(map_id, cx, cy))?;
    lyracore_shared::terrain::interpolate(&chunk.heights, cx, cy, x, y)
}

/// Snap a derived destination Z to terrain, keeping `fallback` off-slice. The one-liner every
/// wander/flee/fear/bot leg goes through (work-item 174).
pub fn snap_z(ctx: &ReducerContext, map_id: u32, x: f32, y: f32, fallback: f32) -> f32 {
    ground_z(ctx, map_id, x, y).unwrap_or(fallback)
}

/// The imported `AreaTable.dbc` area id (MCNK header field) for the cell at `(x, y)`, or `None` when
/// no terrain chunk is imported there OR the chunk's `area_id` is 0 (unset — some cells never got a
/// real client-side AreaTable assignment). Same single indexed lookup as `ground_z` — cheap enough
/// for the release-time graveyard-zone resolution it feeds (work-item 209:
/// `world::graveyard::resolve_zone_id` chases this up to the enclosing zone via
/// `game_area.parent_area_id`).
pub fn area_id_at(ctx: &ReducerContext, map_id: u32, x: f32, y: f32) -> Option<u32> {
    let (cx, cy) = (cell_index(x)?, cell_index(y)?);
    let chunk = ctx
        .db
        .game_terrain_chunk()
        .key()
        .find(cell_key(map_id, cx, cy))?;
    (chunk.area_id != 0).then_some(chunk.area_id)
}

// ===========================================================================================
//  Import reducers — `Vec<f32>` has no spacetime-SQL literal, so terrain loads through the
//  packed-string reducer path exactly like `import_creature_spawns` (rows `;`, fields `,`,
//  the 145 heights `:`-joined inside one field). First call clears + loads, `_append` adds.
// ===========================================================================================

fn load_terrain_batch(ctx: &ReducerContext, packed: &str) -> Result<u32, String> {
    let chunks = ctx.db.game_terrain_chunk();
    let mut loaded = 0u32;
    for row in packed.split(';').filter(|r| !r.is_empty()) {
        let f: Vec<&str> = row.split(',').collect();
        if f.len() != 8 {
            return Err(format!(
                "terrain row needs 8 fields, got {}: {row}",
                f.len()
            ));
        }
        let heights: Vec<f32> = f[7]
            .split(':')
            .map(|s| s.parse::<f32>().map_err(|_| format!("bad height f32: {s}")))
            .collect::<Result<_, _>>()?;
        if heights.len() != 145 {
            return Err(format!(
                "terrain row needs 145 heights, got {}",
                heights.len()
            ));
        }
        let pu32 = |s: &str| s.parse::<u32>().map_err(|_| format!("bad u32: {s}"));
        let pu16 = |s: &str| s.parse::<u16>().map_err(|_| format!("bad u16: {s}"));
        let (map_id, cell_x, cell_y) = (pu32(f[0])?, pu16(f[1])?, pu16(f[2])?);
        chunks.insert(TerrainChunk {
            key: cell_key(map_id, cell_x, cell_y),
            map_id,
            cell_x,
            cell_y,
            heights,
            liquid_level: f[3]
                .parse::<f32>()
                .map_err(|_| format!("bad f32: {}", f[3]))?,
            has_liquid: f[4] == "1",
            holes: pu32(f[5])?,
            area_id: pu32(f[6])?,
        });
        loaded += 1;
    }
    Ok(loaded)
}

/// Clear + load the first terrain batch (operator-only), mirroring `import_creature_spawns`.
#[reducer]
pub fn import_terrain_chunks(ctx: &ReducerContext, packed: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let chunks = ctx.db.game_terrain_chunk();
    let keys: Vec<u64> = chunks.iter().map(|c| c.key).collect();
    for k in keys {
        chunks.key().delete(k);
    }
    if load_terrain_batch(ctx, &packed)? == 0 {
        return Err("terrain import payload was empty".to_string());
    }
    Ok(())
}

/// Append a terrain batch WITHOUT the reset — a zone's cells span several `spacetime call` args.
#[reducer]
pub fn import_terrain_chunks_append(ctx: &ReducerContext, packed: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    load_terrain_batch(ctx, &packed)?;
    Ok(())
}

// Interpolation tests live with the shared math in `crates/lyracore-shared/src/terrain.rs`
// (this crate's native test target doesn't link — project bar is `cargo check`).
