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

const STAGING: u8 = 0;
const VERIFIED: u8 = 1;
const ACTIVE: u8 = 2;
const DISCARDED: u8 = 3;

fn require_staging(state: u8) -> Result<(), String> {
    (state == STAGING)
        .then_some(())
        .ok_or_else(|| "vmap generation is not staging".to_string())
}

fn require_complete(expected_chunks: u32, accepted_chunks: u32) -> Result<(), String> {
    (accepted_chunks == expected_chunks)
        .then_some(())
        .ok_or_else(|| format!("vmap generation has {accepted_chunks}/{expected_chunks} chunks"))
}

fn require_verified(state: u8) -> Result<(), String> {
    (state == VERIFIED)
        .then_some(())
        .ok_or_else(|| "only a verified vmap generation can be activated".to_string())
}

/// Map-scoped import lifecycle. The operator supplies `id`, making retries and importer resume
/// deterministic without needing a reducer return value to discover an auto-incremented key.
#[table(accessor = game_vmap_generation, index(accessor = by_map_state, btree(columns = [map_id, state])))]
pub struct VmapGeneration {
    #[primary_key]
    pub id: u64,
    pub map_id: u32,
    pub state: u8,
    pub expected_chunks: u32,
    pub accepted_chunks: u32,
}

/// A staged chunk belongs to exactly one generation. `shard_ordinal` is stable within one cell,
/// so an ambiguous importer retry can be accepted idempotently instead of duplicating triangles.
#[table(
    accessor = game_vmap_generation_chunk,
    index(accessor = by_generation, btree(columns = [generation_id])),
    index(accessor = by_generation_cell, btree(columns = [generation_id, key]))
)]
pub struct VmapGenerationChunk {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub generation_id: u64,
    pub shard_ordinal: u32,
    pub key: u64,
    pub map_id: u32,
    pub cell_x: u16,
    pub cell_y: u16,
    pub blob: Vec<u8>,
}

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

fn generation(ctx: &ReducerContext, generation_id: u64) -> Result<VmapGeneration, String> {
    ctx.db
        .game_vmap_generation()
        .id()
        .find(generation_id)
        .ok_or_else(|| format!("unknown vmap generation {generation_id}"))
}

/// Start a generation. Repeating the exact request resumes it; a conflicting request is refused.
#[reducer]
pub fn stage_vmap_generation(
    ctx: &ReducerContext,
    generation_id: u64,
    map_id: u32,
    expected_chunks: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if expected_chunks == 0 {
        return Err("vmap generation needs at least one expected chunk".to_string());
    }
    let generations = ctx.db.game_vmap_generation();
    match generations.id().find(generation_id) {
        None => {
            generations.insert(VmapGeneration {
                id: generation_id,
                map_id,
                state: STAGING,
                expected_chunks,
                accepted_chunks: 0,
            });
            Ok(())
        }
        Some(row)
            if row.map_id == map_id
                && row.expected_chunks == expected_chunks
                && row.state == STAGING =>
        {
            Ok(())
        }
        Some(_) => Err(format!(
            "vmap generation {generation_id} already exists with different lifecycle data"
        )),
    }
}

/// Append one packed batch to a staging generation. Each row is
/// `shard_ordinal,map_id,cell_x,cell_y,blob_hex`.
#[reducer]
pub fn append_vmap_generation_chunks(
    ctx: &ReducerContext,
    generation_id: u64,
    packed: String,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let mut generation = generation(ctx, generation_id)?;
    require_staging(generation.state)?;
    let chunks = ctx.db.game_vmap_generation_chunk();
    for row in packed.split(';').filter(|row| !row.is_empty()) {
        let fields: Vec<&str> = row.splitn(5, ',').collect();
        if fields.len() != 5 {
            return Err(format!(
                "vmap generation row needs 5 fields, got {}",
                fields.len()
            ));
        }
        let shard_ordinal = fields[0]
            .parse::<u32>()
            .map_err(|_| format!("bad shard ordinal: {}", fields[0]))?;
        let map_id = fields[1]
            .parse::<u32>()
            .map_err(|_| format!("bad map id: {}", fields[1]))?;
        let cell_x = fields[2]
            .parse::<u16>()
            .map_err(|_| format!("bad cell x: {}", fields[2]))?;
        let cell_y = fields[3]
            .parse::<u16>()
            .map_err(|_| format!("bad cell y: {}", fields[3]))?;
        if map_id != generation.map_id {
            return Err(format!(
                "chunk map {map_id} does not match generation map {}",
                generation.map_id
            ));
        }
        let blob = hex_decode(fields[4])?;
        decode(&blob).map_err(|err| format!("invalid vmap chunk: {err:?}"))?;
        let key = cell_key(map_id, cell_x, cell_y);
        let prior = chunks
            .by_generation_cell()
            .filter((generation_id, key))
            .find(|chunk| chunk.shard_ordinal == shard_ordinal);
        match prior {
            Some(chunk) if chunk.blob == blob => {}
            Some(_) => return Err(format!("conflicting retry for generation {generation_id}, cell {key}, shard {shard_ordinal}")),
            None => {
                chunks.insert(VmapGenerationChunk { id: 0, generation_id, shard_ordinal, key, map_id, cell_x, cell_y, blob });
                generation.accepted_chunks += 1;
            }
        }
    }
    if generation.accepted_chunks > generation.expected_chunks {
        return Err("vmap generation received more chunks than expected".to_string());
    }
    ctx.db.game_vmap_generation().id().update(generation);
    Ok(())
}

/// Validate the complete staged generation before it can affect gameplay.
#[reducer]
pub fn verify_vmap_generation(ctx: &ReducerContext, generation_id: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let mut generation = generation(ctx, generation_id)?;
    require_staging(generation.state)?;
    require_complete(generation.expected_chunks, generation.accepted_chunks)?;
    for chunk in ctx
        .db
        .game_vmap_generation_chunk()
        .by_generation()
        .filter(&generation_id)
    {
        decode(&chunk.blob).map_err(|err| format!("invalid staged vmap chunk: {err:?}"))?;
    }
    generation.state = VERIFIED;
    ctx.db.game_vmap_generation().id().update(generation);
    Ok(())
}

/// Atomically replace the map's active generation after successful verification.
#[reducer]
pub fn activate_vmap_generation(ctx: &ReducerContext, generation_id: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let mut generation = generation(ctx, generation_id)?;
    require_verified(generation.state)?;
    let generations = ctx.db.game_vmap_generation();
    for mut active in generations
        .by_map_state()
        .filter((generation.map_id, ACTIVE))
    {
        active.state = DISCARDED;
        generations.id().update(active);
    }
    generation.state = ACTIVE;
    generations.id().update(generation);
    Ok(())
}

/// Discard incomplete or failed staging data without touching a prior active generation.
#[reducer]
pub fn discard_vmap_generation(ctx: &ReducerContext, generation_id: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let mut generation = generation(ctx, generation_id)?;
    if generation.state == ACTIVE {
        return Err("cannot discard the active vmap generation".to_string());
    }
    if generation.state == DISCARDED {
        return Ok(());
    }
    let chunks = ctx.db.game_vmap_generation_chunk();
    let ids: Vec<u64> = chunks
        .by_generation()
        .filter(&generation_id)
        .map(|chunk| chunk.id)
        .collect();
    for id in ids {
        chunks.id().delete(id);
    }
    generation.state = DISCARDED;
    ctx.db.game_vmap_generation().id().update(generation);
    Ok(())
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

/// Exact geometry can consume only when the global kill-switch is on AND this map has a complete
/// active generation. Staging or legacy chunks never make an uncovered map read as clear.
pub fn vmap_enabled(ctx: &ReducerContext, map_id: u32) -> bool {
    let configured = ctx
        .db
        .game_config()
        .id()
        .find(0)
        .map(|c| c.vmap_enabled)
        .unwrap_or(false);
    configured
        && ctx
            .db
            .game_vmap_generation()
            .by_map_state()
            .filter((map_id, ACTIVE))
            .next()
            .is_some()
}

/// Chunk fetch closure for `cast_ray`: one indexed scan + decode per crossed cell, gathering
/// every shard row for that cell key (a dense cell may be split across several — see
/// `VmapChunk`'s doc comment) and concatenating their decoded triangles. A shard decode failure
/// (shouldn't happen — only this module writes the table) is skipped rather than panicking a ray
/// query. `None` only when NO row at all exists for the cell (the missing-chunk contract).
fn fetcher(ctx: &ReducerContext, map_id: u32) -> impl FnMut(u16, u16) -> Option<Vec<VmapTri>> + '_ {
    let generation_id = ctx
        .db
        .game_vmap_generation()
        .by_map_state()
        .filter((map_id, ACTIVE))
        .next()
        .map(|generation| generation.id);
    move |cx, cy| {
        let generation_id = generation_id?;
        let key = cell_key(map_id, cx, cy);
        let mut tris = Vec::new();
        let mut any = false;
        for row in ctx
            .db
            .game_vmap_generation_chunk()
            .by_generation_cell()
            .filter((generation_id, key))
        {
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
    if !vmap_enabled(ctx, map_id) {
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
    if !vmap_enabled(ctx, map_id) {
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
    if !vmap_enabled(ctx, map_id) {
        return None;
    }
    let top = [x, y, z + FLOOR_PROBE_UP_YD];
    let bottom = [x, y, z - FLOOR_PROBE_DOWN_YD];
    lyracore_shared::vmap::cast_ray_area(&mut fetcher(ctx, map_id), top, bottom)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct LifecycleHarness {
        generations: BTreeMap<u64, (u32, u8, u32, u32)>,
        chunks: BTreeMap<(u64, u64, u32), Vec<u8>>,
    }

    impl LifecycleHarness {
        fn stage(&mut self, id: u64, map: u32, expected: u32) {
            self.generations.insert(id, (map, STAGING, expected, 0));
        }

        fn append(&mut self, id: u64, key: u64, shard: u32, blob: Vec<u8>) -> Result<(), String> {
            let generation = self.generations.get_mut(&id).ok_or("missing generation")?;
            require_staging(generation.1)?;
            match self.chunks.get(&(id, key, shard)) {
                Some(prior) if prior == &blob => Ok(()),
                Some(_) => Err("conflicting retry".to_string()),
                None => {
                    self.chunks.insert((id, key, shard), blob);
                    generation.3 += 1;
                    Ok(())
                }
            }
        }

        fn verify(&mut self, id: u64) -> Result<(), String> {
            let generation = self.generations.get_mut(&id).ok_or("missing generation")?;
            require_staging(generation.1)?;
            require_complete(generation.2, generation.3)?;
            generation.1 = VERIFIED;
            Ok(())
        }

        fn activate(&mut self, id: u64) -> Result<(), String> {
            let map = self.generations.get(&id).ok_or("missing generation")?.0;
            require_verified(self.generations[&id].1)?;
            for generation in self.generations.values_mut() {
                if generation.0 == map && generation.1 == ACTIVE {
                    generation.1 = DISCARDED;
                }
            }
            self.generations.get_mut(&id).unwrap().1 = ACTIVE;
            Ok(())
        }

        fn discard(&mut self, id: u64) -> Result<(), String> {
            let generation = self.generations.get_mut(&id).ok_or("missing generation")?;
            if generation.1 == ACTIVE {
                return Err("cannot discard active".to_string());
            }
            generation.1 = DISCARDED;
            self.chunks
                .retain(|(generation_id, _, _), _| *generation_id != id);
            Ok(())
        }

        fn active(&self, map: u32) -> Option<u64> {
            self.generations.iter().find_map(|(&id, generation)| {
                (generation.0 == map && generation.1 == ACTIVE).then_some(id)
            })
        }
    }

    #[test]
    fn only_a_complete_staging_generation_can_be_verified() {
        assert!(require_staging(STAGING).is_ok());
        assert!(require_complete(2, 2).is_ok());
        assert!(require_complete(2, 1).is_err());
        assert!(require_staging(VERIFIED).is_err());
    }

    #[test]
    fn activation_requires_verification() {
        assert!(require_verified(VERIFIED).is_ok());
        assert!(require_verified(STAGING).is_err());
        assert!(require_verified(ACTIVE).is_err());
        assert!(require_verified(DISCARDED).is_err());
    }

    #[test]
    fn lifecycle_states_are_distinct() {
        assert_ne!(STAGING, VERIFIED);
        assert_ne!(VERIFIED, ACTIVE);
        assert_ne!(ACTIVE, DISCARDED);
    }

    #[test]
    fn lifecycle_harness_retries_activation_and_map_selection() {
        let mut h = LifecycleHarness::default();
        h.stage(1, 0, 1);
        h.append(1, 7, 0, vec![1]).unwrap();
        h.append(1, 7, 0, vec![1]).unwrap();
        assert_eq!(h.generations[&1].3, 1, "retry must not duplicate a chunk");
        h.verify(1).unwrap();
        h.activate(1).unwrap();
        assert_eq!(h.active(0), Some(1));
        assert_eq!(h.active(1), None, "another map remains baseline");

        h.stage(2, 0, 1);
        assert!(
            h.verify(2).is_err(),
            "incomplete replacement cannot activate"
        );
        assert_eq!(
            h.active(0),
            Some(1),
            "prior active generation survives failure"
        );
        h.append(2, 8, 0, vec![2]).unwrap();
        h.verify(2).unwrap();
        h.activate(2).unwrap();
        assert_eq!(h.active(0), Some(2));
        assert_eq!(h.generations[&1].1, DISCARDED);
    }

    #[test]
    fn lifecycle_harness_discards_staging_without_touching_active() {
        let mut h = LifecycleHarness::default();
        h.stage(1, 0, 1);
        h.append(1, 7, 0, vec![1]).unwrap();
        h.verify(1).unwrap();
        h.activate(1).unwrap();
        h.stage(2, 0, 1);
        h.append(2, 8, 0, vec![2]).unwrap();
        h.discard(2).unwrap();
        assert_eq!(h.active(0), Some(1));
        assert!(!h.chunks.keys().any(|(generation, _, _)| *generation == 2));
        assert!(h.discard(1).is_err(), "active data cannot be discarded");
    }
}
