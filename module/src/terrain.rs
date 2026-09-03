//! Terrain heightmap: ground-Z lookups for creature + bot movement (work-items 172/173).
//!
//! One row per ADT MCNK cell (33.33 yd square), imported from the operator's own client by
//! `importer --terrain` (same licensing firewall as `--dbc`: extracted data never enters the
//! repo). The module has no filesystem, so terrain lives here as an indexed table — a
//! "map/nav re-homeing" shape. Coverage follows
//! the content slice; `ground_z` returns `None` off-slice and every caller keeps its current
//! Z there, so unimported areas behave exactly as before this table existed.

use crate::game_area; // zone_id_at's one-hop parent walk (accessor trait)
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
///
/// Also takes the `max` against the topmost imported vmap model floor (bridge, WMO
/// interior deck) at or below this same Z — `vmap::floor_z` returns `None` off vmap-slice/gate,
/// so an unimported map is byte-identical to before this line existed. The terrain heightmap and
/// the vmap floor never both apply to the same surface (a bridge deck isn't in the ADT MCVT
/// grid), so `max` picks whichever one actually has an answer here rather than averaging or
/// preferring one system outright.
pub fn snap_z(ctx: &ReducerContext, map_id: u32, x: f32, y: f32, fallback: f32) -> f32 {
    let base = ground_z(ctx, map_id, x, y).unwrap_or(fallback);
    match crate::vmap::floor_z(ctx, map_id, x, y, fallback) {
        Some(floor) => base.max(floor),
        None => base,
    }
}

/// The imported `AreaTable.dbc` area id (MCNK header field) for the cell at `(x, y)`, or `None` when
/// no terrain chunk is imported there OR the chunk's `area_id` is 0 (unset — some cells never got a
/// real client-side AreaTable assignment). Same single indexed lookup as `ground_z` — cheap enough
/// for the release-time graveyard-zone resolution it feeds (work-item 209, now `zone_id_at` below).
pub fn area_id_at(ctx: &ReducerContext, map_id: u32, x: f32, y: f32) -> Option<u32> {
    let (cx, cy) = (cell_index(x)?, cell_index(y)?);
    let chunk = ctx
        .db
        .game_terrain_chunk()
        .key()
        .find(cell_key(map_id, cx, cy))?;
    (chunk.area_id != 0).then_some(chunk.area_id)
}

/// The imported liquid surface at `(x, y)`, or `None` when terrain is absent or its cell records
/// no liquid. Like [`ground_z`] and [`area_id_at`], this is one indexed primary-key lookup with no
/// interpolation because imported liquid is flat for a cell.
pub fn liquid_level_at(ctx: &ReducerContext, map_id: u32, x: f32, y: f32) -> Option<f32> {
    let (cx, cy) = (cell_index(x)?, cell_index(y)?);
    let chunk = ctx
        .db
        .game_terrain_chunk()
        .key()
        .find(cell_key(map_id, cx, cy))?;
    chunk.has_liquid.then_some(chunk.liquid_level)
}

/// Logs whether the live player is submerged according to its current movement state and imported
/// terrain cell. This is a read-only operator probe; it does not change movement or environmental
/// damage behavior.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_check_submerged(ctx: &ReducerContext, guid: u64) -> Result<(), String> {
    let entity = crate::helpers::live_entity(ctx, guid)?;
    if !entity.is_player() {
        return Err(format!("guid {guid} is not a player"));
    }
    let liquid_level = liquid_level_at(ctx, entity.map_id, entity.x, entity.y);
    let submerged = liquid_level.is_some_and(|level| {
        lyracore_shared::env::is_submerged(entity.z, level, true, entity.movement_flags)
    });
    spacetimedb::log::info!(
        "submerged probe guid {guid} at map {} ({:.2},{:.2},{:.2}): liquid_level={liquid_level:?} movement_flags=0x{:08x} submerged={submerged}",
        entity.map_id,
        entity.x,
        entity.y,
        entity.z,
        entity.movement_flags,
    );
    Ok(())
}

/// Chase the position's MCNK `area_id` (`area_id_at` above) ONE hop up `game_area.parent_area_id` to
/// its enclosing zone — e.g. a Goldshire subzone area resolves to zone 12 (Elwynn). NOT a full
/// recursive area-hierarchy walk (a subzone-of-a-subzone would need more than one hop; deferred to
/// work-item 200, which needs full area resolution for exploration XP anyway). Returns `None` when
/// `game_area` is empty (unimported) or the position's terrain cell has no recorded/imported area, so
/// callers skip zone-scoping entirely rather than guessing wrong — a wrong guess would silently narrow
/// a candidate set (graveyards, fishing loot) to the WRONG zone.
///
/// The single canonical zone resolver (work-item 209 idiom): `world::graveyard`'s release pick
/// and `loot::apply_fish`'s catch roll both call this instead of each keeping its own one-hop walk.
pub fn zone_id_at(ctx: &ReducerContext, map_id: u32, x: f32, y: f32) -> Option<u32> {
    area_at(ctx, map_id, x, y).map(|area| zone_of(&area))
}

/// The imported `game_area` ROW at `(x, y)` — [`area_id_at`] plus its one indexed row read. Callers
/// that need both the area and its zone (the movement grid-crossing hook drives discovery XP and the
/// zone transition off the same crossing) resolve once here instead of looking the position up twice.
/// `None` under the same conditions as [`area_id_at`], plus an unimported `game_area`.
pub fn area_at(ctx: &ReducerContext, map_id: u32, x: f32, y: f32) -> Option<crate::GameArea> {
    if ctx.db.game_area().count() == 0 {
        return None;
    }
    let area_id = area_id_at(ctx, map_id, x, y)?;
    ctx.db.game_area().id().find(area_id)
}

/// The one-hop subzone→zone chase itself: an area with a `parent_area_id` IS a subzone, and its
/// parent is the zone; a top-level area is its own zone. Split out of [`zone_id_at`] so a caller
/// holding a resolved area does not repeat the rule.
pub fn zone_of(area: &crate::GameArea) -> u32 {
    if area.parent_area_id != 0 {
        area.parent_area_id
    } else {
        area.id
    }
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
