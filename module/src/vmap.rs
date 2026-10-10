//! Static collision generations and instance-aware geometry queries.
//! The global Gate controls gameplay rays. Static hits require an active generation;
//! closed DOOR/BUTTON hits use their own instance-scoped registration.

use crate::terrain::game_terrain_chunk;
use lyracore_shared::terrain::cell_key;
use lyracore_shared::vmap::{decode, RayFlavor, VmapTri};
use spacetimedb::{reducer, table, ReducerContext, Table};

const STAGING: u8 = 0;
const VERIFIED: u8 = 1;
const ACTIVE: u8 = 2;
const DISCARDED: u8 = 3;
const MANIFEST_DOMAIN: &[u8] = b"lyracore-vmap-manifest-v1";
const COVERAGE_MANIFEST_DOMAIN: &[u8] = b"lyracore-vmap-nav-coverage-manifest-v1";

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

/// Hash the canonical, transport-independent stream of staged shards. The importer makes this
/// same sequence while it drains its sorted disk spool: cell key, per-cell ordinal, blob length,
/// then the exact packed bytes. Keeping it here makes verification reject a complete-but-wrong
/// upload rather than trusting transport progress as data integrity.
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

/// Refuse a staged stream whose content is not exactly the immutable manifest accepted at stage
/// time. This is deliberately pure so the reducer contract has a small, headless test seam.
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

/// Map-scoped import lifecycle. The operator supplies `id`, making retries and importer resume
/// deterministic without needing a reducer return value to discover an auto-incremented key.
#[table(accessor = game_vmap_generation, public, index(accessor = by_map_state, btree(columns = [map_id, state])))]
pub struct VmapGeneration {
    #[primary_key]
    pub id: u64,
    pub map_id: u32,
    pub state: u8,
    pub expected_chunks: u32,
    pub accepted_chunks: u32,
    /// Immutable byte total of all packed shard blobs in this generation's manifest.
    pub expected_bytes: u64,
    /// Immutable 32-byte BLAKE3 digest of the canonical manifest stream.
    pub manifest_digest: Vec<u8>,

    pub source_identity: String,
    /// Immutable canonical identity of the selected map/cell coverage.
    pub selection_identity: String,
}

/// Public, blob-free receipt ledger for resumable imports. Operators and the importer can read
/// this table to skip an already accepted `(generation_id, key, shard_ordinal)` without
/// subscribing to or re-transferring collision payloads from the private chunk store.
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

/// Start a generation. Repeating the exact request resumes it; a conflicting request is refused.
#[reducer]
#[allow(clippy::too_many_arguments)] // A reducer's arguments are its call signature.
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

/// Validate the complete staged generation before it can affect gameplay.
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
    // The decode pass that validates every staged chunk also classifies it: a cell whose triangles
    // include an indoor WMO group gets a marker row below (`tri_is_indoor`). Free — the blobs are
    // decoded here exactly once either way, and nothing about the manifest or the digest changes.
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
    let receipts = ctx.db.game_vmap_generation_receipt();
    let receipt_ids: Vec<u64> = receipts
        .by_generation()
        .filter(&generation_id)
        .map(|receipt| receipt.id)
        .collect();
    for id in receipt_ids {
        receipts.id().delete(id);
    }
    // The derived indoor markers go with the geometry they were derived from.
    let indoor_cells = ctx.db.game_vmap_indoor_cell();
    let indoor_ids: Vec<u64> = indoor_cells
        .by_generation()
        .filter(&generation_id)
        .map(|cell| cell.id)
        .collect();
    for id in indoor_ids {
        indoor_cells.id().delete(id);
    }
    // Derived path-grid coverage is likewise scoped to the generation it came from.
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

// ===========================================================================================
//  Nav coverage derivation — per-cell walk/obstruction blobs derived module-side
//  from a generation's OWN staged `VmapGenerationChunk` rows, never a re-import and never
//  `game_nav_chunk` (that table has no generation column and is cleared wholesale by the `--nav`
//  import pipeline). Coverage is generation-scoped so it can never half-exist: it belongs to
//  exactly one generation, and the manifest below only ever gets written once, fully formed.
//  Same rasterizer as the importer's `--nav` mode (`lyracore_shared::nav::derive_cell`) — one
//  rasterization policy, never two.
// ===========================================================================================

fn require_coverage_preparable(state: u8) -> Result<(), String> {
    matches!(state, VERIFIED | ACTIVE)
        .then_some(())
        .ok_or_else(|| "vmap nav coverage needs a verified or active generation".to_string())
}

/// Hash the canonical, cell-key-sorted stream of a generation's coverage rows: key, `base_z` (the
/// standing-ground floor folds into the digest per the README rule), then the walk/obs blobs.
/// Same shape as `manifest_digest` above, one level down.
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

/// One derived coverage cell. PRIVATE like `VmapGenerationChunk` — the nav read seam
/// (`nav.rs::fetcher`) reads it module-side; nothing outside this module needs a gateway binding.
/// A missing row means no coverage for that cell, the same "no obstacles known" contract as
/// `game_nav_chunk`: `derive_cell` returns `None` for a fully-clear cell and no row is written.
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
    /// Same `cell_key` as `game_nav_chunk`/`game_terrain_chunk`/`game_vmap_generation_chunk`.
    pub cell_key: u64,
    pub map_id: u32,
    pub cell_x: u16,
    pub cell_y: u16,
    pub base_z: f32,
    pub walk: Vec<u8>,
    pub obs: Vec<u8>,
}

/// Public, blob-free per-generation summary. `complete` is set exactly once, inside
/// `finalize_vmap_nav_coverage`'s single insert-or-update — there is no persisted half-finished
/// manifest, so its mere existence already means "finalized."
#[table(accessor = game_vmap_nav_coverage_manifest, public)]
pub struct VmapNavCoverageManifest {
    #[primary_key]
    pub generation_id: u64,
    pub map_id: u32,
    pub cell_count: u32,
    pub digest: Vec<u8>,
    pub complete: bool,
}

/// One cell to prepare in [`prepare_vmap_nav_coverage`]'s batch. Cell coordinates only — the
/// storage key is always rebuilt server-side from the generation's own `map_id`, the same trust
/// boundary `append_vmap_generation_chunks` already applies to its cell key.
#[derive(spacetimedb::SpacetimeType)]
pub struct VmapNavCoverageCell {
    pub cell_x: u16,
    pub cell_y: u16,
}

/// Every triangle from `(cell_x, cell_y)` plus its 8 neighbours, gathered from this generation's
/// OWN staged chunks (never the live `game_vmap_chunk` table). Nav derivation needs geometry
/// within `nav::WALK_MARGIN` of the cell — wider than the vmap importer's exact-AABB-touch
/// binning — so the neighbours cover that gap; `derive_cell` clamps everything back to the
/// requested cell, so the extra triangles are free and correct, never a border seam.
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

/// Derive and store coverage for a batch of cells, decoding one cell's neighbourhood at a time so
/// memory stays bounded by the batch, not the map. Idempotent per cell: a key that already has a
/// row — from an earlier call, including a retried one — is skipped outright, so re-submitting
/// the same batch (or an overlapping one) never duplicates a row or perturbs the eventual digest.
/// Allowed for a verified or active generation (the active map can be retrofitted); refused for
/// staging or discarded data.
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

/// Lock in whatever coverage is currently prepared as this generation's manifest: cell count +
/// digest over every stored row, sorted by cell key for a canonical stream. Safe to call again
/// after preparing more cells, or with nothing new — the digest just reproduces. Same
/// verified-or-active gate as preparation.
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

/// This map's active generation's coverage manifest, but only once it is finalized (`complete`).
/// Coverage is optional: a verified-but-not-yet-finalized, or deliberately uncovered, active
/// generation selects none, and `nav.rs::fetcher` degrades to terrain-only exactly as if coverage
/// had never been prepared. Re-evaluated on every call rather than snapshotted at activation time,
/// because preparation can retrofit an already active generation.
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
//  Runtime consumption — module-side wrappers over `lyracore_shared::vmap::cast_ray`,
//  gated on `game_config.vmap_enabled`.
// ===========================================================================================

use crate::game_config;

/// The map's ACTIVE generation id, or `None` when it has none. The ONE `by_map_state` scan —
/// `vmap_enabled`, `fetcher` and the indoor-presence lookup all ask through here rather than
/// repeating the filter, so "which generation is live" has a single answer per map.
pub(crate) fn active_generation_id(ctx: &ReducerContext, map_id: u32) -> Option<u64> {
    ctx.db
        .game_vmap_generation()
        .by_map_state()
        .filter((map_id, ACTIVE))
        .next()
        .map(|generation| generation.id)
}

/// Static geometry consumes only with the global Gate and a complete active generation.
/// This decides whether navigation still needs its coarse grid fallback.
pub(crate) fn vmap_enabled(ctx: &ReducerContext, map_id: u32) -> bool {
    rays_enabled(ctx) && active_generation_id(ctx, map_id).is_some()
}

/// Global collision Gate. Dynamic doors do not require a static generation.
pub(crate) fn rays_enabled(ctx: &ReducerContext) -> bool {
    ctx.db
        .game_config()
        .id()
        .find(0)
        .is_some_and(|c| c.vmap_enabled)
}

/// Chunk fetch closure for `cast_ray`: one indexed scan + decode per crossed cell, gathering
/// every shard row for that cell key (a dense cell may be split across several — see
/// `VmapChunk`'s doc comment) and concatenating their decoded triangles. A shard decode failure
/// (shouldn't happen — only this module writes the table) is skipped rather than panicking a ray
/// query. `None` only when NO row at all exists for the cell (the missing-chunk contract).
fn fetcher(ctx: &ReducerContext, map_id: u32) -> impl FnMut(u16, u16) -> Option<Vec<VmapTri>> + '_ {
    let generation_id = active_generation_id(ctx, map_id);
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

/// Exact sight includes WMO geometry and closed doors in the requested instance.
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

/// Exact collision includes WMO, static doodads and closed doors.
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

/// Operator geometry probe, independent of the gameplay Gate.
#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
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
/// How far below `probe_z` the down-ray searches — generous enough to reach the ground floor of
/// a multi-deck WMO interior (Deadmines) from a probe standing on an upper deck.
const FLOOR_PROBE_DOWN_YD: f32 = 200.0;

/// Highest static or closed-door surface in the vertical probe segment in this partition.
/// The segment starts slightly above `probe_z` so a point already on the surface can find it.
/// Returns `None` when the gameplay Gate is off or no collision triangle lies in that segment.
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

/// Walkable model support for a movement waypoint. A steep face remains an obstacle even when
/// a downward ray intersects it near the preceding foot height.
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

/// Static and closed-door floor probe, independent of the gameplay Gate.
#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
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

// ===========================================================================================
//  Indoor/outdoor area-info — mangos parity: `VMapManager2::getAreaInfo` / the indoor
//  half of `Player::CheckAreaExploreAndOutdoor`. Same down-ray probe shape as `floor_z`, but
//  WMO-only (a doodad standing in an open field doesn't make the point "indoor") and reporting
//  the containing group's id + MOGP indoor bit instead of just the hit height.
// ===========================================================================================

/// Which WMO group (if any) contains `(x, y, z)` on `map_id`, and whether it's indoor. `None`
/// when vmap is off, unimported, or no WMO group is found in the probe range (outdoors/open
/// world) — mirrors every other vmap query's missing-chunk-means-nothing-found contract.
///
/// The downward budget is `AREA_PROBE_DOWN_YD`, NOT the 200 yd `FLOOR_PROBE_DOWN_YD`: a floor
/// probe wants the deepest deck it can reach, an area query wants only the surface this point is
/// standing on, and the long budget reports a point in the open as inside a WMO far below it.
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

// ===========================================================================================
//  Per-cell indoor presence — a derived, module-private pre-reject in front of
//  `area_info`, so a movement heartbeat can ask "am I indoors?" every 100 ms without paying a
//  ray cast anywhere the map has no interior geometry at all (which is nearly everywhere).
//
//  Kept in one region at the END of this file on purpose: the generation lifecycle above is
//  contested ground (the coverage derivation lands there too), and everything here is
//  additive — one table, one marker write inside `verify_vmap_generation`'s existing decode
//  pass, one indexed find on the read path.
// ===========================================================================================

/// A cell of one generation that contains at least one INDOOR WMO triangle. Marker-only: the row's
/// existence IS the fact, so there is no state to keep in sync. PRIVATE and derived — it carries
/// no information the staged chunks don't already, it is rebuilt from them by verification, and it
/// is deliberately outside the manifest/digest (hashing it would make an equivalent re-import
/// non-reproducible for no gain).
///
/// A MISSING row means outdoors. That is the fail-open contract the whole indoor feature rests
/// on: no generation, no verification, no row — no indoor dismount, ever.
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
    /// Same `cell_key` as `game_vmap_generation_chunk`: (map << 32) | (cell_x << 16) | cell_y.
    pub key: u64,
}

/// Does this triangle belong to an indoor WMO group? Doodads (M2) never do — a tent-shaped tree
/// standing in a field is not a room. Delegates the MOGP rule to `lyracore_shared::vmap` so the
/// pre-reject and the query it fronts can never disagree.
fn tri_is_indoor(tri: &VmapTri) -> bool {
    match tri.class {
        lyracore_shared::vmap::TriClass::Wmo { mogp_flags, .. } => {
            lyracore_shared::vmap::group_is_indoor(mogp_flags)
        }
        lyracore_shared::vmap::TriClass::M2 => false,
    }
}

/// Record the generation's indoor cells. Called once from `verify_vmap_generation` with the keys
/// its existing decode pass already classified — there is no second decode and no second read of
/// the staged chunks.
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

/// Is `(x, y)` in a cell that holds any indoor geometry at all? One indexed find against the
/// map's ACTIVE generation. `false` — no raycast, treat as outdoors — when vmap has no active
/// generation for the map, when the position is off the map square, or when the cell has no
/// marker row.
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

/// Is `(x, y, z)` inside a WMO interior? THE indoor question — the mount cast gate, the movement
/// heartbeat and the teleport path all ask it here rather than reading MOGP flags themselves.
///
/// Two stages, and both FAIL OPEN (answer `false`, "outdoors"): the per-cell marker find, which
/// skips the ray cast for a cell with no interior geometry, then `area_info`, which is already
/// `None` with vmap off, with no active generation, or with nothing under the probe. So an
/// operator running without vmap data never gets an indoor dismount, and never gets a false one.
pub(crate) fn is_indoor(ctx: &ReducerContext, map_id: u32, x: f32, y: f32, z: f32) -> bool {
    cell_may_be_indoor(ctx, map_id, x, y)
        && area_info(ctx, map_id, x, y, z).is_some_and(|info| info.indoor)
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn manifest_requires_exact_bytes_digest_and_canonical_order() {
        let first = [0x01, 0x02];
        let second = [0x03];
        let canonical = [(7, 0, first.as_slice()), (9, 0, second.as_slice())];
        let digest = manifest_digest(canonical);

        assert!(require_manifest(3, &digest, canonical).is_ok());
        assert!(
            require_manifest(2, &digest, canonical).is_err(),
            "byte truncation is refused"
        );
        assert!(
            require_manifest(
                3,
                &digest,
                [(9, 0, second.as_slice()), (7, 0, first.as_slice())]
            )
            .is_err(),
            "the same chunks in a different order have a different manifest"
        );
        assert!(
            require_manifest(3, &[0; 32], canonical).is_err(),
            "a different digest is refused"
        );
    }

    /// The presence table's classifier. A WMO group is indoor by the SHARED mangos rule (outdoor
    /// only when `0x8000` is set); a doodad never is, whatever its geometry looks like.
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
