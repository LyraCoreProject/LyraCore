use crate::game_area;
use lyracore_shared::terrain::{cell_index, cell_key};
use spacetimedb::{reducer, table, ReducerContext, Table};

// MCVT stores 145 absolute heights, interleaving nine outer and eight inner points per row pair.
#[table(accessor = game_terrain_chunk, public)]
pub struct TerrainChunk {
    #[primary_key]
    pub key: u64,
    pub map_id: u32,
    pub cell_x: u16,
    pub cell_y: u16,
    pub heights: Vec<f32>,
    pub liquid_level: f32,
    pub has_liquid: bool,
    pub holes: u32,
    pub area_id: u32,
    #[default(0u64)]
    pub deep_water_tiles: u64,
}

pub(crate) fn ground_z(ctx: &ReducerContext, map_id: u32, x: f32, y: f32) -> Option<f32> {
    let (cx, cy) = (cell_index(x)?, cell_index(y)?);
    let chunk = ctx
        .db
        .game_terrain_chunk()
        .key()
        .find(cell_key(map_id, cx, cy))?;
    lyracore_shared::terrain::interpolate(&chunk.heights, cx, cy, x, y)
}

pub(crate) fn snap_z(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
    x: f32,
    y: f32,
    fallback: f32,
) -> f32 {
    let base = ground_z(ctx, map_id, x, y).unwrap_or(fallback);
    match crate::vmap::floor_z(ctx, map_id, instance_id, x, y, fallback) {
        Some(floor) => base.max(floor),
        None => base,
    }
}

pub(crate) fn walking_z(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
    x: f32,
    y: f32,
    previous_z: f32,
    fallback: f32,
) -> f32 {
    match (
        ground_z(ctx, map_id, x, y),
        crate::vmap::walking_floor_z(ctx, map_id, instance_id, x, y, previous_z),
    ) {
        (Some(ground), Some(floor)) => ground.max(floor),
        (Some(z), None) | (None, Some(z)) => z,
        (None, None) => fallback,
    }
}

pub(crate) fn area_id_at(ctx: &ReducerContext, map_id: u32, x: f32, y: f32) -> Option<u32> {
    let (cx, cy) = (cell_index(x)?, cell_index(y)?);
    let chunk = ctx
        .db
        .game_terrain_chunk()
        .key()
        .find(cell_key(map_id, cx, cy))?;
    (chunk.area_id != 0).then_some(chunk.area_id)
}

pub(crate) fn liquid_level_at(ctx: &ReducerContext, map_id: u32, x: f32, y: f32) -> Option<f32> {
    let (cx, cy) = (cell_index(x)?, cell_index(y)?);
    let chunk = ctx
        .db
        .game_terrain_chunk()
        .key()
        .find(cell_key(map_id, cx, cy))?;
    chunk.has_liquid.then_some(chunk.liquid_level)
}

pub(crate) fn deep_water_level_at(
    ctx: &ReducerContext,
    map_id: u32,
    x: f32,
    y: f32,
) -> Option<f32> {
    let (cx, cy) = (cell_index(x)?, cell_index(y)?);
    let chunk = ctx
        .db
        .game_terrain_chunk()
        .key()
        .find(cell_key(map_id, cx, cy))?;
    (chunk.has_liquid
        && lyracore_shared::terrain::is_deep_water_at(chunk.deep_water_tiles, cx, cy, x, y))
    .then_some(chunk.liquid_level)
}

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

pub(crate) fn zone_id_at(ctx: &ReducerContext, map_id: u32, x: f32, y: f32) -> Option<u32> {
    area_at(ctx, map_id, x, y).map(|area| zone_of(&area))
}

pub(crate) fn area_at(
    ctx: &ReducerContext,
    map_id: u32,
    x: f32,
    y: f32,
) -> Option<crate::GameArea> {
    if ctx.db.game_area().count() == 0 {
        return None;
    }
    let area_id = area_id_at(ctx, map_id, x, y)?;
    ctx.db.game_area().id().find(area_id)
}

pub(crate) fn zone_of(area: &crate::GameArea) -> u32 {
    if area.parent_area_id != 0 {
        area.parent_area_id
    } else {
        area.id
    }
}

fn load_terrain_batch(ctx: &ReducerContext, packed: &str) -> Result<u32, String> {
    let chunks = ctx.db.game_terrain_chunk();
    let mut loaded = 0u32;
    for row in packed.split(';').filter(|r| !r.is_empty()) {
        let f: Vec<&str> = row.split(',').collect();
        // The ninth field, the deep-water tile mask, is optional.
        if !(8..=9).contains(&f.len()) {
            return Err(format!(
                "terrain row needs 8 or 9 fields, got {}: {row}",
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
            deep_water_tiles: f
                .get(8)
                .map(|s| s.parse::<u64>().map_err(|_| format!("bad u64: {s}")))
                .transpose()?
                .unwrap_or(0),
        });
        loaded += 1;
    }
    Ok(loaded)
}

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
    crate::nav::record_change(ctx)
}

#[reducer]
pub fn import_terrain_chunks_append(ctx: &ReducerContext, packed: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if load_terrain_batch(ctx, &packed)? > 0 {
        crate::nav::record_change(ctx)?;
    }
    Ok(())
}
