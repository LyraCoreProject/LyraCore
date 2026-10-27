use crate::terrain::game_terrain_chunk;
use lyracore_shared::terrain::cell_key;
use lyracore_shared::vmap::{decode, RayFlavor, VmapTri};
use spacetimedb::{reducer, table, ReducerContext, Table};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

const STAGING: u8 = 0;
const VERIFIED: u8 = 1;
const ACTIVE: u8 = 2;
const DISCARDED: u8 = 3;
const MANIFEST_DOMAIN: &[u8] = b"lyracore-vmap-manifest-v1";
const COVERAGE_MANIFEST_DOMAIN: &[u8] = b"lyracore-vmap-nav-coverage-manifest-v1";

const QUERY_CACHE_BYTES: usize = 4 * 1024 * 1024;
const QUERY_CACHE_CELLS: usize = 128;

#[derive(Default)]
struct QueryCache {
    cells: BTreeMap<(u64, u64), Option<Rc<[VmapTri]>>>,
    bytes: usize,
}

impl QueryCache {
    fn cell(
        &mut self,
        generation_id: u64,
        key: u64,
        load: impl FnOnce() -> Option<Vec<VmapTri>>,
    ) -> Option<CellTriangles> {
        if let Some(cached) = self.cells.get(&(generation_id, key)) {
            return cached.clone().map(CellTriangles::Shared);
        }
        let triangles = load();
        let bytes = triangles
            .as_ref()
            .map_or(0, |rows| std::mem::size_of_val(rows.as_slice()));
        if bytes > QUERY_CACHE_BYTES {
            return triangles.map(CellTriangles::Owned);
        }
        if self.cells.len() == QUERY_CACHE_CELLS || self.bytes + bytes > QUERY_CACHE_BYTES {
            self.cells.clear();
            self.bytes = 0;
        }
        let triangles = triangles.map(Rc::<[VmapTri]>::from);
        self.bytes += bytes;
        self.cells.insert((generation_id, key), triangles.clone());
        triangles.map(CellTriangles::Shared)
    }
}

thread_local! {
    static QUERY_CACHE: RefCell<Option<QueryCache>> = const { RefCell::new(None) };
}

// Static cells are immutable after generation verification; dynamic colliders bypass this cache.
pub(crate) fn with_query_cache<T>(query: impl FnOnce() -> T) -> T {
    struct RestoreCache(Option<QueryCache>);
    impl Drop for RestoreCache {
        fn drop(&mut self) {
            QUERY_CACHE.with(|cache| *cache.borrow_mut() = self.0.take());
        }
    }
    let _restore =
        RestoreCache(QUERY_CACHE.with(|cache| cache.replace(Some(QueryCache::default()))));
    query()
}

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

// Digest framing and order must match the importer's canonical spool stream.
fn manifest_digest<'a>(chunks: impl IntoIterator<Item = (u64, u32, &'a [u8])>) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(MANIFEST_DOMAIN);
    for (key, shard_ordinal, blob) in chunks {
        hasher.update(&key.to_le_bytes());
        hasher.update(&shard_ordinal.to_le_bytes());
        hasher.update(&(blob.len() as u32).to_le_bytes());
        hasher.update(blob);
    }
    *hasher.finalize().as_bytes()
}

fn require_manifest<'a>(
    expected_bytes: u64,
    expected_digest: &[u8],
    chunks: impl IntoIterator<Item = (u64, u32, &'a [u8])>,
) -> Result<(), String> {
    let chunks: Vec<_> = chunks.into_iter().collect();
    let actual_bytes = chunks.iter().try_fold(0u64, |total, (_, _, blob)| {
        total
            .checked_add(blob.len() as u64)
            .ok_or_else(|| "vmap generation byte count overflow".to_string())
    })?;
    if actual_bytes != expected_bytes {
        return Err(format!(
            "vmap generation has {actual_bytes}/{expected_bytes} manifest bytes"
        ));
    }
    if manifest_digest(chunks) != expected_digest {
        return Err("vmap generation manifest digest does not match staged chunks".to_string());
    }
    Ok(())
}

#[table(accessor = game_vmap_generation, public, index(accessor = by_map_state, btree(columns = [map_id, state])))]
pub struct VmapGeneration {
    #[primary_key]
    pub id: u64,
    pub map_id: u32,
    pub state: u8,
    pub expected_chunks: u32,
    pub accepted_chunks: u32,
    pub expected_bytes: u64,
    pub manifest_digest: Vec<u8>,

    pub source_identity: String,
    pub selection_identity: String,
}

#[table(
    accessor = game_vmap_generation_receipt,
    public,
    index(accessor = by_generation, btree(columns = [generation_id])),
    index(accessor = by_generation_cell, btree(columns = [generation_id, key]))
)]
pub struct VmapGenerationReceipt {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub generation_id: u64,
    pub shard_ordinal: u32,
    pub key: u64,
}

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

#[table(accessor = game_vmap_chunk, index(accessor = by_cell, btree(columns = [key])))]
pub struct VmapChunk {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub key: u64,
    pub map_id: u32,
    pub cell_x: u16,
    pub cell_y: u16,
    pub blob: Vec<u8>,
}

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

fn same_stage_request(
    row: &VmapGeneration,
    map_id: u32,
    expected_chunks: u32,
    expected_bytes: u64,
    manifest_digest: &[u8],
    source_identity: &str,
    selection_identity: &str,
) -> bool {
    row.map_id == map_id
        && row.expected_chunks == expected_chunks
        && row.expected_bytes == expected_bytes
        && row.manifest_digest == manifest_digest
        && row.source_identity == source_identity
        && row.selection_identity == selection_identity
        && row.state == STAGING
}

#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn stage_vmap_generation(
    ctx: &ReducerContext,
    generation_id: u64,
    map_id: u32,
    expected_chunks: u32,
    expected_bytes: u64,
    manifest_digest_hex: String,
    source_identity: String,
    selection_identity: String,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if expected_chunks == 0 {
        return Err("vmap generation needs at least one expected chunk".to_string());
    }
    if expected_bytes == 0 {
        return Err("vmap generation needs at least one expected byte".to_string());
    }
    if source_identity.is_empty() {
        return Err("vmap generation source identity must not be empty".to_string());
    }
    if selection_identity.is_empty() {
        return Err("vmap generation selection identity must not be empty".to_string());
    }
    let manifest_digest = hex_decode(&manifest_digest_hex)?;
    if manifest_digest.len() != 32 {
        return Err("vmap generation manifest digest must be 32 bytes of hex".to_string());
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
                expected_bytes,
                manifest_digest,
                source_identity,
                selection_identity,
            });
            Ok(())
        }
        Some(row)
            if same_stage_request(
                &row,
                map_id,
                expected_chunks,
                expected_bytes,
                &manifest_digest,
                &source_identity,
                &selection_identity,
            ) =>
        {
            Ok(())
        }
        Some(_) => Err(format!(
            "vmap generation {generation_id} already exists with different lifecycle data"
        )),
    }
}

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
    let receipts = ctx.db.game_vmap_generation_receipt();
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
                receipts.insert(VmapGenerationReceipt {
                    id: 0,
                    generation_id,
                    shard_ordinal,
                    key,
                });
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

#[reducer]
pub fn verify_vmap_generation(ctx: &ReducerContext, generation_id: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let mut generation = generation(ctx, generation_id)?;
    require_staging(generation.state)?;
    require_complete(generation.expected_chunks, generation.accepted_chunks)?;
    let mut staged: Vec<VmapGenerationChunk> = ctx
        .db
        .game_vmap_generation_chunk()
        .by_generation()
        .filter(&generation_id)
        .collect();
    staged.sort_unstable_by_key(|chunk| (chunk.key, chunk.shard_ordinal));
    let mut indoor_cells = std::collections::BTreeSet::new();
    for chunk in &staged {
        let tris =
            decode(&chunk.blob).map_err(|err| format!("invalid staged vmap chunk: {err:?}"))?;
        if tris.iter().any(tri_is_indoor) {
            indoor_cells.insert(chunk.key);
        }
    }
    require_manifest(
        generation.expected_bytes,
        &generation.manifest_digest,
        staged
            .iter()
            .map(|chunk| (chunk.key, chunk.shard_ordinal, chunk.blob.as_slice())),
    )?;
    record_indoor_cells(ctx, generation_id, indoor_cells);
    generation.state = VERIFIED;
    ctx.db.game_vmap_generation().id().update(generation);
    Ok(())
}

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
    let receipts = ctx.db.game_vmap_generation_receipt();
    let receipt_ids: Vec<u64> = receipts
        .by_generation()
        .filter(&generation_id)
        .map(|receipt| receipt.id)
        .collect();
    for id in receipt_ids {
        receipts.id().delete(id);
    }
    let indoor_cells = ctx.db.game_vmap_indoor_cell();
    let indoor_ids: Vec<u64> = indoor_cells
        .by_generation()
        .filter(&generation_id)
        .map(|cell| cell.id)
        .collect();
    for id in indoor_ids {
        indoor_cells.id().delete(id);
    }
    let coverage = ctx.db.game_vmap_nav_coverage();
    let coverage_ids: Vec<u64> = coverage
        .by_generation()
        .filter(&generation_id)
        .map(|cell| cell.id)
        .collect();
    for id in coverage_ids {
        coverage.id().delete(id);
    }
    let manifests = ctx.db.game_vmap_nav_coverage_manifest();
    if manifests.generation_id().find(generation_id).is_some() {
        manifests.generation_id().delete(generation_id);
    }
    generation.state = DISCARDED;
    ctx.db.game_vmap_generation().id().update(generation);
    Ok(())
}

fn require_coverage_preparable(state: u8) -> Result<(), String> {
    matches!(state, VERIFIED | ACTIVE)
        .then_some(())
        .ok_or_else(|| "vmap nav coverage needs a verified or active generation".to_string())
}

fn coverage_digest<'a>(
    cells: impl IntoIterator<Item = (u64, f32, &'a [u8], &'a [u8])>,
) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(COVERAGE_MANIFEST_DOMAIN);
    for (key, base_z, walk, obs) in cells {
        hasher.update(&key.to_le_bytes());
        hasher.update(&base_z.to_le_bytes());
        hasher.update(&(walk.len() as u32).to_le_bytes());
        hasher.update(walk);
        hasher.update(&(obs.len() as u32).to_le_bytes());
        hasher.update(obs);
    }
    *hasher.finalize().as_bytes()
}

#[table(
    accessor = game_vmap_nav_coverage,
    index(accessor = by_generation, btree(columns = [generation_id])),
    index(accessor = by_generation_cell, btree(columns = [generation_id, cell_key]))
)]
pub struct VmapNavCoverage {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub generation_id: u64,
    pub cell_key: u64,
    pub map_id: u32,
    pub cell_x: u16,
    pub cell_y: u16,
    pub base_z: f32,
    pub walk: Vec<u8>,
    pub obs: Vec<u8>,
}

#[table(accessor = game_vmap_nav_coverage_manifest, public)]
pub struct VmapNavCoverageManifest {
    #[primary_key]
    pub generation_id: u64,
    pub map_id: u32,
    pub cell_count: u32,
    pub digest: Vec<u8>,
    pub complete: bool,
}

#[derive(spacetimedb::SpacetimeType)]
pub struct VmapNavCoverageCell {
    pub cell_x: u16,
    pub cell_y: u16,
}

// Derive from this generation's staged neighbourhood, never the currently active geometry.
fn coverage_neighborhood_tris(
    ctx: &ReducerContext,
    generation_id: u64,
    map_id: u32,
    cell_x: u16,
    cell_y: u16,
) -> Vec<VmapTri> {
    let chunks = ctx.db.game_vmap_generation_chunk();
    let mut tris = Vec::new();
    for dx in [-1i16, 0, 1] {
        let Some(nx) = cell_x.checked_add_signed(dx).filter(|&v| (v as u32) < 1024) else {
            continue;
        };
        for dy in [-1i16, 0, 1] {
            let Some(ny) = cell_y.checked_add_signed(dy).filter(|&v| (v as u32) < 1024) else {
                continue;
            };
            let key = cell_key(map_id, nx, ny);
            for row in chunks.by_generation_cell().filter((generation_id, key)) {
                if let Ok(mut t) = decode(&row.blob) {
                    tris.append(&mut t);
                }
            }
        }
    }
    tris
}

#[reducer]
pub fn prepare_vmap_nav_coverage(
    ctx: &ReducerContext,
    generation_id: u64,
    cells: Vec<VmapNavCoverageCell>,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let generation = generation(ctx, generation_id)?;
    require_coverage_preparable(generation.state)?;
    let coverage = ctx.db.game_vmap_nav_coverage();
    let mut inserted = false;
    for cell in cells {
        let key = cell_key(generation.map_id, cell.cell_x, cell.cell_y);
        if coverage
            .by_generation_cell()
            .filter((generation_id, key))
            .next()
            .is_some()
        {
            continue;
        }
        let tris = coverage_neighborhood_tris(
            ctx,
            generation_id,
            generation.map_id,
            cell.cell_x,
            cell.cell_y,
        );
        if tris.is_empty() {
            continue;
        }
        let heights = ctx
            .db
            .game_terrain_chunk()
            .key()
            .find(key)
            .map(|chunk| chunk.heights);
        let Some(derived) =
            lyracore_shared::nav::derive_cell(cell.cell_x, cell.cell_y, heights.as_deref(), &tris)
        else {
            continue;
        };
        coverage.insert(VmapNavCoverage {
            id: 0,
            generation_id,
            cell_key: key,
            map_id: generation.map_id,
            cell_x: cell.cell_x,
            cell_y: cell.cell_y,
            base_z: derived.base_z,
            walk: derived.walk,
            obs: derived.obs,
        });
        inserted = true;
    }
    if inserted && crate::nav::coverage_generation(ctx, generation.map_id) == Some(generation_id) {
        crate::nav::record_change(ctx)?;
    }
    Ok(())
}

#[reducer]
pub fn finalize_vmap_nav_coverage(ctx: &ReducerContext, generation_id: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let generation = generation(ctx, generation_id)?;
    require_coverage_preparable(generation.state)?;
    let mut cells: Vec<VmapNavCoverage> = ctx
        .db
        .game_vmap_nav_coverage()
        .by_generation()
        .filter(&generation_id)
        .collect();
    cells.sort_unstable_by_key(|cell| cell.cell_key);
    let digest = coverage_digest(cells.iter().map(|cell| {
        (
            cell.cell_key,
            cell.base_z,
            cell.walk.as_slice(),
            cell.obs.as_slice(),
        )
    }))
    .to_vec();
    let cell_count = cells.len() as u32;
    let manifests = ctx.db.game_vmap_nav_coverage_manifest();
    match manifests.generation_id().find(generation_id) {
        Some(mut row) => {
            row.cell_count = cell_count;
            row.digest = digest;
            row.complete = true;
            manifests.generation_id().update(row);
        }
        None => {
            manifests.insert(VmapNavCoverageManifest {
                generation_id,
                map_id: generation.map_id,
                cell_count,
                digest,
                complete: true,
            });
        }
    }
    Ok(())
}

pub(crate) fn active_vmap_nav_coverage_manifest(
    ctx: &ReducerContext,
    map_id: u32,
) -> Option<VmapNavCoverageManifest> {
    let generation_id = active_generation_id(ctx, map_id)?;
    ctx.db
        .game_vmap_nav_coverage_manifest()
        .generation_id()
        .find(generation_id)
        .filter(|manifest| manifest.complete)
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
            id: 0,
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

#[reducer]
pub fn import_vmap_chunks_append(ctx: &ReducerContext, packed: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    load_vmap_batch(ctx, &packed)?;
    Ok(())
}

use crate::game_config;

pub(crate) fn active_generation_id(ctx: &ReducerContext, map_id: u32) -> Option<u64> {
    ctx.db
        .game_vmap_generation()
        .by_map_state()
        .filter((map_id, ACTIVE))
        .next()
        .map(|generation| generation.id)
}

pub(crate) fn vmap_enabled(ctx: &ReducerContext, map_id: u32) -> bool {
    rays_enabled(ctx) && active_generation_id(ctx, map_id).is_some()
}

pub(crate) fn rays_enabled(ctx: &ReducerContext) -> bool {
    ctx.db
        .game_config()
        .id()
        .find(0)
        .is_some_and(|c| c.vmap_enabled)
}

enum CellTriangles {
    Owned(Vec<VmapTri>),
    Shared(Rc<[VmapTri]>),
}

impl AsRef<[VmapTri]> for CellTriangles {
    fn as_ref(&self) -> &[VmapTri] {
        match self {
            Self::Owned(triangles) => triangles,
            Self::Shared(triangles) => triangles,
        }
    }
}

fn fetcher(
    ctx: &ReducerContext,
    map_id: u32,
) -> impl FnMut(u16, u16) -> Option<CellTriangles> + '_ {
    let generation_id = active_generation_id(ctx, map_id);
    move |cx, cy| {
        let generation_id = generation_id?;
        let key = cell_key(map_id, cx, cy);
        let load = || {
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
        };
        QUERY_CACHE.with(|cache| match cache.borrow_mut().as_mut() {
            Some(cache) => cache.cell(generation_id, key, load),
            None => load().map(CellTriangles::Owned),
        })
    }
}

pub(crate) fn los_ray(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
    a: [f32; 3],
    b: [f32; 3],
) -> Option<[f32; 3]> {
    rays_enabled(ctx)
        .then(|| ray(ctx, map_id, instance_id, a, b, RayFlavor::Los))
        .flatten()
}

pub(crate) fn collision_ray(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
    a: [f32; 3],
    b: [f32; 3],
) -> Option<[f32; 3]> {
    rays_enabled(ctx)
        .then(|| ray(ctx, map_id, instance_id, a, b, RayFlavor::Collision))
        .flatten()
}

pub(crate) fn nearest_hit(
    a: [f32; 3],
    first: Option<[f32; 3]>,
    second: Option<[f32; 3]>,
) -> Option<[f32; 3]> {
    let distance = |p: [f32; 3]| p.iter().zip(a).map(|(x, y)| (x - y) * (x - y)).sum::<f32>();
    match (first, second) {
        (Some(x), Some(y)) => Some(if distance(x) <= distance(y) { x } else { y }),
        (x, None) | (None, x) => x,
    }
}

fn ray(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
    a: [f32; 3],
    b: [f32; 3],
    flavor: RayFlavor,
) -> Option<[f32; 3]> {
    let static_hit = lyracore_shared::vmap::cast_ray(&mut fetcher(ctx, map_id), a, b, flavor);
    nearest_hit(
        a,
        static_hit,
        crate::go_collider::ray(ctx, map_id, instance_id, a, b),
    )
}

#[cfg(feature = "debug_reducers")]
pub(crate) fn probe_rays(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
    a: [f32; 3],
    b: [f32; 3],
) -> (Option<[f32; 3]>, Option<[f32; 3]>) {
    (
        ray(ctx, map_id, instance_id, a, b, RayFlavor::Los),
        ray(ctx, map_id, instance_id, a, b, RayFlavor::Collision),
    )
}

const FLOOR_PROBE_UP_YD: f32 = 2.0;
const FLOOR_PROBE_DOWN_YD: f32 = 200.0;

pub(crate) fn floor_z(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
    x: f32,
    y: f32,
    probe_z: f32,
) -> Option<f32> {
    let top = [x, y, probe_z + FLOOR_PROBE_UP_YD];
    let bottom = [x, y, probe_z - FLOOR_PROBE_DOWN_YD];
    collision_ray(ctx, map_id, instance_id, top, bottom).map(|hit| hit[2])
}

pub(crate) fn walking_floor_z(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
    x: f32,
    y: f32,
    probe_z: f32,
) -> Option<f32> {
    if !rays_enabled(ctx) {
        return None;
    }
    let top = [x, y, probe_z + FLOOR_PROBE_UP_YD];
    let bottom = [x, y, probe_z - FLOOR_PROBE_DOWN_YD];
    nearest_hit(
        top,
        lyracore_shared::vmap::cast_floor(&mut fetcher(ctx, map_id), top, bottom),
        crate::go_collider::floor_ray(ctx, map_id, instance_id, top, bottom),
    )
    .map(|point| point[2])
}

#[cfg(feature = "debug_reducers")]
pub(crate) fn probe_floor_z(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
    x: f32,
    y: f32,
    probe_z: f32,
) -> Option<f32> {
    let top = [x, y, probe_z + FLOOR_PROBE_UP_YD];
    let bottom = [x, y, probe_z - FLOOR_PROBE_DOWN_YD];
    ray(ctx, map_id, instance_id, top, bottom, RayFlavor::Collision).map(|hit| hit[2])
}

pub(crate) fn area_info(
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
    let bottom = [x, y, z - lyracore_shared::vmap::AREA_PROBE_DOWN_YD];
    lyracore_shared::vmap::cast_ray_area(&mut fetcher(ctx, map_id), top, bottom)
}

#[table(
    accessor = game_vmap_indoor_cell,
    index(accessor = by_generation, btree(columns = [generation_id])),
    index(accessor = by_generation_cell, btree(columns = [generation_id, key]))
)]
pub struct VmapIndoorCell {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub generation_id: u64,
    pub key: u64,
}

fn tri_is_indoor(tri: &VmapTri) -> bool {
    match tri.class {
        lyracore_shared::vmap::TriClass::Wmo { mogp_flags, .. } => {
            lyracore_shared::vmap::group_is_indoor(mogp_flags)
        }
        lyracore_shared::vmap::TriClass::M2 => false,
    }
}

fn record_indoor_cells(
    ctx: &ReducerContext,
    generation_id: u64,
    keys: impl IntoIterator<Item = u64>,
) {
    let cells = ctx.db.game_vmap_indoor_cell();
    for key in keys {
        cells.insert(VmapIndoorCell {
            id: 0,
            generation_id,
            key,
        });
    }
}

fn cell_may_be_indoor(ctx: &ReducerContext, map_id: u32, x: f32, y: f32) -> bool {
    let Some(generation_id) = active_generation_id(ctx, map_id) else {
        return false;
    };
    let (Some(cell_x), Some(cell_y)) = (
        lyracore_shared::terrain::cell_index(x),
        lyracore_shared::terrain::cell_index(y),
    ) else {
        return false;
    };
    ctx.db
        .game_vmap_indoor_cell()
        .by_generation_cell()
        .filter((generation_id, cell_key(map_id, cell_x, cell_y)))
        .next()
        .is_some()
}

pub(crate) fn is_indoor(ctx: &ReducerContext, map_id: u32, x: f32, y: f32, z: f32) -> bool {
    cell_may_be_indoor(ctx, map_id, x, y)
        && area_info(ctx, map_id, x, y, z).is_some_and(|info| info.indoor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_rays_decode_each_static_cell_once_and_generation_changes_reload_it() {
        use lyracore_shared::vmap::{cast_ray, encode, TriClass};
        let wall = |x| {
            vec![VmapTri {
                verts: [[x, 1199.0, 49.0], [x, 1202.0, 49.0], [x, 1200.0, 52.0]],
                class: TriClass::Wmo {
                    group_id: 1,
                    mogp_flags: 0,
                },
            }]
        };
        let mut cache = QueryCache::default();
        let mut decodes = 0;
        let from = [1200.0, 1200.0, 50.0];
        let to = [1201.0, 1200.0, 50.0];
        let first_blob = encode(&wall(1200.25));
        let mut first_count = 0;
        for query in 0..32 {
            let hit = cast_ray(
                &mut |cx, cy| {
                    cache.cell(7, cell_key(0, cx, cy), || {
                        decodes += 1;
                        Some(decode(&first_blob).unwrap())
                    })
                },
                from,
                to,
                RayFlavor::Los,
            );
            assert_eq!(hit, Some([1200.25, 1200.0, 50.0]));
            if query == 0 {
                first_count = decodes;
            }
            assert_eq!(decodes, first_count);
        }
        let next_blob = encode(&wall(1200.75));
        let next_hit = cast_ray(
            &mut |cx, cy| {
                cache.cell(8, cell_key(0, cx, cy), || {
                    decodes += 1;
                    Some(decode(&next_blob).unwrap())
                })
            },
            from,
            to,
            RayFlavor::Collision,
        );
        assert_eq!(next_hit, Some([1200.75, 1200.0, 50.0]));
        assert_eq!(decodes, first_count * 2);
    }

    #[test]
    fn query_cache_does_not_carry_static_cells_into_the_next_firing() {
        with_query_cache(|| {
            QUERY_CACHE.with(|cache| {
                cache
                    .borrow_mut()
                    .as_mut()
                    .unwrap()
                    .cell(1, 2, || Some(Vec::new()));
            });
        });
        with_query_cache(|| {
            QUERY_CACHE.with(|cache| assert!(cache.borrow().as_ref().unwrap().cells.is_empty()));
        });
        QUERY_CACHE.with(|cache| assert!(cache.borrow().is_none()));
    }

    #[test]
    fn cached_missing_cells_have_a_fixed_memory_bound() {
        let mut cache = QueryCache::default();
        for key in 0..QUERY_CACHE_CELLS as u64 + 1 {
            assert!(cache.cell(7, key, || None).is_none());
            assert!(cache.cells.len() <= QUERY_CACHE_CELLS);
        }
        assert_eq!(cache.cells.len(), 1);
        assert_eq!(cache.bytes, 0);
    }

    #[test]
    fn oversized_cells_keep_the_decoded_allocation_and_leave_the_cache_unchanged() {
        let mut cache = QueryCache::default();
        let triangle = VmapTri {
            verts: [[0.0; 3]; 3],
            class: lyracore_shared::vmap::TriClass::M2,
        };
        let cached = cache.cell(7, 1, || Some(vec![triangle])).unwrap();
        let cached_pointer = cached.as_ref().as_ptr();
        let triangles = vec![triangle; QUERY_CACHE_BYTES / std::mem::size_of::<VmapTri>() + 1];
        let decoded_pointer = triangles.as_ptr();

        let oversized = cache.cell(7, 2, || Some(triangles)).unwrap();
        assert_eq!(oversized.as_ref().as_ptr(), decoded_pointer);
        assert_eq!(cache.cells.len(), 1);
        assert_eq!(cache.bytes, std::mem::size_of::<VmapTri>());
        let retained = cache
            .cell(7, 1, || panic!("the cached cell must not be loaded again"))
            .unwrap();
        assert_eq!(retained.as_ref().as_ptr(), cached_pointer);
    }

    #[test]
    fn only_a_complete_staging_generation_can_be_verified() {
        assert!(require_staging(STAGING).is_ok());
        assert!(require_complete(2, 2).is_ok());
        assert_eq!(
            require_complete(2, 1),
            Err("vmap generation has 1/2 chunks".to_string())
        );
        assert_eq!(
            require_staging(VERIFIED),
            Err("vmap generation is not staging".to_string())
        );
    }

    #[test]
    fn activation_requires_verification() {
        assert!(require_verified(VERIFIED).is_ok());
        for state in [STAGING, ACTIVE, DISCARDED] {
            assert_eq!(
                require_verified(state),
                Err("only a verified vmap generation can be activated".to_string())
            );
        }
    }

    #[test]
    fn lifecycle_states_are_distinct() {
        assert_ne!(STAGING, VERIFIED);
        assert_ne!(VERIFIED, ACTIVE);
        assert_ne!(ACTIVE, DISCARDED);
    }

    #[test]
    fn manifest_requires_exact_bytes_digest_and_canonical_order() {
        let first = [0x01, 0x02];
        let second = [0x03];
        let canonical = [(7, 0, first.as_slice()), (9, 0, second.as_slice())];
        let digest = manifest_digest(canonical);

        assert!(require_manifest(3, &digest, canonical).is_ok());
        assert_eq!(
            require_manifest(2, &digest, canonical),
            Err("vmap generation has 3/2 manifest bytes".to_string())
        );
        let mismatch =
            Err("vmap generation manifest digest does not match staged chunks".to_string());
        assert_eq!(
            require_manifest(
                3,
                &digest,
                [(9, 0, second.as_slice()), (7, 0, first.as_slice())]
            ),
            mismatch,
            "the same chunks in a different order have a different manifest"
        );
        assert_eq!(require_manifest(3, &[0; 32], canonical), mismatch);
    }

    #[test]
    fn only_indoor_wmo_triangles_mark_a_cell() {
        use lyracore_shared::vmap::{TriClass, VmapTri, WMO_GROUP_OUTDOOR_FLAG};
        let tri = |class| VmapTri {
            verts: [[0.0; 3]; 3],
            class,
        };
        assert!(tri_is_indoor(&tri(TriClass::Wmo {
            group_id: 1,
            mogp_flags: 0
        })));
        assert!(!tri_is_indoor(&tri(TriClass::Wmo {
            group_id: 1,
            mogp_flags: WMO_GROUP_OUTDOOR_FLAG
        })));
        assert!(!tri_is_indoor(&tri(TriClass::M2)));
    }

    #[test]
    fn restaging_requires_the_same_provenance_identities() {
        let row = VmapGeneration {
            id: 7,
            map_id: 0,
            state: STAGING,
            expected_chunks: 3,
            accepted_chunks: 0,
            expected_bytes: 12,
            manifest_digest: vec![0xAA; 32],
            source_identity: "client-1.12.1:abc".to_string(),
            selection_identity: "map=0;cells=0,0..1,1".to_string(),
        };
        assert!(same_stage_request(
            &row,
            0,
            3,
            12,
            &[0xAA; 32],
            "client-1.12.1:abc",
            "map=0;cells=0,0..1,1",
        ));
        assert!(
            !same_stage_request(
                &row,
                0,
                3,
                12,
                &[0xAA; 32],
                "client-1.12.1:def",
                "map=0;cells=0,0..1,1",
            ),
            "a matching payload from a different source cannot resume the generation"
        );
        assert!(
            !same_stage_request(
                &row,
                0,
                3,
                12,
                &[0xAA; 32],
                "client-1.12.1:abc",
                "map=0;cells=0,0..2,2",
            ),
            "a different coverage selection cannot resume the generation"
        );
    }
}
