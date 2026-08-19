//! `--vmap <client Data/ dir>` — work-item #520 (part of the #169 full-vmap epic; design record:
//! `docs/decisions.md` §10). Exact per-cell collision triangles, replacing the nav rasterizer's
//! obstruction-height half.
//!
//! Reuses `nav.rs`'s tile scan (heights + deduped WMO/M2 placements), mesh loading, and MODF-bounds
//! rotation calibration verbatim — only the consumption differs: instead of rasterizing into a
//! walkability/obstruction grid, every collision triangle is transformed to world space with FULL
//! rotation (yaw + pitch + roll — the rasterizer's yaw-only limitation doesn't apply to an exact
//! per-triangle store), binned by the SAME 33.3 yd terrain cell key the nav grid uses, and packed
//! into a per-cell blob via `lyracore_shared::vmap` (source class + WMO group id + MOGP flags).
//!
//! #520 stopped at extract + pack + report. #521 adds `--apply`: batches each cell's packed blob
//! into `import_vmap_chunks`/`import_vmap_chunks_append` calls (same convention as `--nav`), and
//! the module-side table + LoS/collision ray queries those reducers feed live in
//! `module/src/vmap.rs` + `lyracore_shared::vmap::cast_ray`. Same licensing firewall as
//! `--nav`/`--terrain`: in-memory only, nothing written to disk.
//!
//! `--vmap-prepare-coverage <generation_id>` drives path-grid coverage derivation for an
//! already-staged generation (`run_coverage` below): no client Data/ dir, since the geometry is
//! already staged and the module derives coverage from it directly. Enumerates the generation's
//! own cells, batches `prepare_vmap_nav_coverage` calls, and finalizes — resumable by skipping
//! cells a prior run already covered.

use anyhow::{bail, Context, Result};
use lyracore_shared::terrain::cell_key;
use lyracore_shared::vmap::{encode, TriClass, VmapTri, HEADER_BYTES, TRI_BYTES};
#[cfg(test)]
use std::collections::HashMap;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use wow_mpq::PatchChain;

use crate::nav::{Mesh, Placement, Tri};

/// Full local→world rotation, unlike `nav::apply`'s yaw-only shortcut (the rasterizer's documented
/// simplification — thin walls with real pitch/roll rasterize slightly wrong but the walkability
/// grid tolerates it; an exact triangle store must not).
///
/// Axis order confirmed against the reference VMAP extractor convention (TrinityCore
/// `model.cpp::ExtractSet`): `Matrix3::fromEulerAnglesZYX(Rotation.y, Rotation.x, Rotation.z)`,
/// i.e. world rotation = Rz(yaw) · Ry(pitch) · Rx(roll) applied to the local vertex, where
/// yaw = `rotation[1]`, pitch = `rotation[0]`, roll = `rotation[2]` — matching `nav.rs`'s existing
/// yaw-only convention (which already uses `rotation[1]` as the calibrated Z-axis term). Only the
/// yaw term is empirically calibrated against MODF bounds (`nav::calibrate`); pitch/roll compose in
/// the same local frame before that calibrated yaw+translate. Reasonable synthesis of a documented
/// convention, not verified against a live tilted placement — flag for a measured follow-up if one
/// turns up wrong (same "deliberate simplification, revisit when a live case demands it" posture
/// `nav.rs` uses elsewhere).
fn apply_full(
    conv: crate::nav::Convention,
    rotation: [f32; 3],
    scale: f32,
    pos_w: [f32; 3],
    v: [f32; 3],
) -> [f32; 3] {
    let l = if conv.shuffle {
        [-v[2], -v[0], v[1]]
    } else {
        v
    };
    let mut p = [l[0] * scale, l[1] * scale, l[2] * scale];

    // Roll about local X (rotation[2]).
    let (rs, rc) = rotation[2].to_radians().sin_cos();
    p = [p[0], rc * p[1] - rs * p[2], rs * p[1] + rc * p[2]];
    // Pitch about local Y (rotation[0]).
    let (ps, pc) = rotation[0].to_radians().sin_cos();
    p = [pc * p[0] + ps * p[2], p[1], -ps * p[0] + pc * p[2]];
    // Calibrated yaw about world Z (rotation[1], same sign/offset nav.rs's rasterizer uses).
    let theta = (conv.sign * rotation[1] + conv.offset_deg).to_radians();
    let (ys, yc) = theta.sin_cos();
    [
        pos_w[0] + yc * p[0] - ys * p[1],
        pos_w[1] + ys * p[0] + yc * p[1],
        pos_w[2] + p[2],
    ]
}

#[cfg(test)]
#[allow(dead_code)]
fn placement_tris(p: &Placement, mesh: &Mesh, conv: crate::nav::Convention) -> Vec<VmapTri> {
    let pos_w = crate::nav::place_pos(p.position);
    let transform = |t: &Tri| -> [[f32; 3]; 3] {
        [
            apply_full(conv, p.rotation, p.scale, pos_w, t[0]),
            apply_full(conv, p.rotation, p.scale, pos_w, t[1]),
            apply_full(conv, p.rotation, p.scale, pos_w, t[2]),
        ]
    };
    match mesh {
        Mesh::Wmo(v) => v
            .iter()
            .map(|w| VmapTri {
                verts: transform(&w.tri),
                class: TriClass::Wmo {
                    group_id: w.group_id,
                    mogp_flags: w.mogp_flags,
                },
            })
            .collect(),
        Mesh::M2(v) => v
            .iter()
            .map(|t| VmapTri {
                verts: transform(t),
                class: TriClass::M2,
            })
            .collect(),
    }
}

fn for_each_placement_tri(
    p: &Placement,
    mesh: &Mesh,
    conv: crate::nav::Convention,
    mut emit: impl FnMut(VmapTri),
) {
    let pos_w = crate::nav::place_pos(p.position);
    let transform = |t: &Tri| {
        [
            apply_full(conv, p.rotation, p.scale, pos_w, t[0]),
            apply_full(conv, p.rotation, p.scale, pos_w, t[1]),
            apply_full(conv, p.rotation, p.scale, pos_w, t[2]),
        ]
    };
    match mesh {
        Mesh::Wmo(rows) => {
            for w in rows {
                emit(VmapTri {
                    verts: transform(&w.tri),
                    class: TriClass::Wmo {
                        group_id: w.group_id,
                        mogp_flags: w.mogp_flags,
                    },
                });
            }
        }
        Mesh::M2(rows) => {
            for tri in rows {
                emit(VmapTri {
                    verts: transform(tri),
                    class: TriClass::M2,
                });
            }
        }
    }
}

/// A world-space triangle's cell-index rectangle (high coord → LOW cell index, same convention as
/// `terrain::cell_index`/`nav.rs`'s binning). None when the triangle falls off the map square.
fn tri_cell_range(t: &VmapTri) -> Option<(u16, u16, u16, u16)> {
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for v in &t.verts {
        for k in 0..2 {
            lo[k] = lo[k].min(v[k]);
            hi[k] = hi[k].max(v[k]);
        }
    }
    let (cx0, cx1) = (
        lyracore_shared::terrain::cell_index(hi[0])?,
        lyracore_shared::terrain::cell_index(lo[0])?,
    );
    let (cy0, cy1) = (
        lyracore_shared::terrain::cell_index(hi[1])?,
        lyracore_shared::terrain::cell_index(lo[1])?,
    );
    Some((cx0, cx1, cy0, cy1))
}

/// Bin triangles by the terrain cell key; a triangle spanning a cell boundary lands in EVERY cell
/// its AABB touches (conservative — matches the nav rasterizer's binning direction).
#[cfg(test)]
pub(crate) fn bin_by_cell(map_id: u32, tris: &[VmapTri]) -> HashMap<u64, Vec<VmapTri>> {
    let mut by_cell: HashMap<u64, Vec<VmapTri>> = HashMap::new();
    for t in tris {
        let Some((cx0, cx1, cy0, cy1)) = tri_cell_range(t) else {
            continue; // off the map square (shouldn't happen inside the box)
        };
        for cx in cx0..=cx1 {
            for cy in cy0..=cy1 {
                by_cell
                    .entry(cell_key(map_id, cx, cy))
                    .or_default()
                    .push(*t);
            }
        }
    }
    by_cell
}

const SPOOL_RECORD_BYTES: usize = 45;

/// Disk-backed cell spool. Only one transformed triangle and one output shard need be resident;
/// filesystem ordering is never used as part of the manifest contract.
struct VmapSpool {
    dir: PathBuf,
}

impl VmapSpool {
    fn new(map_id: u32) -> Result<Self> {
        let dir =
            std::env::temp_dir().join(format!("lyracore-vmap-{map_id}-{}", std::process::id()));
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    fn path(&self, key: u64) -> PathBuf {
        self.dir.join(format!("{key:016x}.bin"))
    }

    fn append(&self, key: u64, tri: VmapTri) -> Result<()> {
        let mut out = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path(key))?;
        for vertex in tri.verts {
            for value in vertex {
                out.write_all(&value.to_le_bytes())?;
            }
        }
        match tri.class {
            TriClass::M2 => out.write_all(&[0])?,
            TriClass::Wmo {
                group_id,
                mogp_flags,
            } => {
                out.write_all(&[1])?;
                out.write_all(&group_id.to_le_bytes())?;
                out.write_all(&mogp_flags.to_le_bytes())?;
                return Ok(());
            }
        }
        out.write_all(&0u32.to_le_bytes())?;
        out.write_all(&0u32.to_le_bytes())?;
        Ok(())
    }

    fn keys(&self) -> Result<Vec<u64>> {
        let mut keys = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some(hex) = name.strip_suffix(".bin") else {
                continue;
            };
            keys.push(u64::from_str_radix(hex, 16).context("invalid vmap spool key")?);
        }
        keys.sort_unstable();
        Ok(keys)
    }

    fn for_each_shard(&self, key: u64, mut emit: impl FnMut(Vec<u8>) -> Result<()>) -> Result<()> {
        let mut file = File::open(self.path(key))?;
        let per_shard = ((MAX_ROW_TRI_BYTES - HEADER_BYTES) / TRI_BYTES).max(1);
        let mut tris = Vec::with_capacity(per_shard);
        loop {
            let mut bytes = [0u8; SPOOL_RECORD_BYTES];
            match file.read(&mut bytes[..1])? {
                0 => break,
                1 => file
                    .read_exact(&mut bytes[1..])
                    .context("truncated vmap spool record")?,
                _ => unreachable!(),
            }
            let mut at = 0;
            let mut verts = [[0.0; 3]; 3];
            for vertex in &mut verts {
                for value in vertex {
                    *value = f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
                    at += 4;
                }
            }
            let class = match bytes[at] {
                0 => TriClass::M2,
                1 => {
                    at += 1;
                    let group_id = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
                    at += 4;
                    let mogp_flags = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
                    TriClass::Wmo {
                        group_id,
                        mogp_flags,
                    }
                }
                tag => bail!("invalid vmap spool class {tag}"),
            };
            tris.push(VmapTri { verts, class });
            if tris.len() == per_shard {
                emit(encode(&tris))?;
                tris.clear();
            }
        }
        if !tris.is_empty() {
            emit(encode(&tris))?;
        }
        Ok(())
    }
}

impl Drop for VmapSpool {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn sql_has_map(sql_output: &str, map_id: u32) -> Result<bool> {
    let value: serde_json::Value =
        serde_json::from_str(sql_output).context("parse JSON SQL map-ownership response")?;
    Ok(sql_column_rows(&value, &["map_id"])
        .iter()
        .any(|row| row[0].as_u64() == Some(map_id as u64)))
}

/// SpacetimeDB JSON SQL returns a result set as `schema.elements` plus array rows, not JSON
/// objects. Match columns by schema name so a query cannot mistake row-count text for ownership.
fn sql_column_rows<'a>(
    value: &'a serde_json::Value,
    columns: &[&str],
) -> Vec<Vec<&'a serde_json::Value>> {
    let mut rows = Vec::new();
    let Some(results) = value.as_array() else {
        return rows;
    };
    for result in results {
        let Some(elements) = result
            .pointer("/schema/elements")
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        let indices: Option<Vec<usize>> = columns
            .iter()
            .map(|column| {
                elements.iter().position(|element| {
                    element
                        .pointer("/name/some")
                        .and_then(serde_json::Value::as_str)
                        == Some(*column)
                })
            })
            .collect();
        let Some(indices) = indices else {
            continue;
        };
        for row in result
            .get("rows")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(row) = row.as_array() else {
                continue;
            };
            if let Some(values) = indices
                .iter()
                .map(|&index| row.get(index))
                .collect::<Option<Vec<_>>>()
            {
                rows.push(values);
            }
        }
    }
    rows
}

fn sql_query(args: &crate::Args, query: &str) -> Result<String> {
    let out = Command::new("spacetime")
        .args([
            "sql",
            "--format",
            "json",
            "-s",
            &args.server,
            &args.db,
            query,
        ])
        .output()
        .with_context(|| format!("run vmap SQL query: {query}"))?;
    if !out.status.success() {
        bail!(
            "vmap SQL query failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn accepted_chunk_ids(
    args: &crate::Args,
    generation_id: u64,
) -> Result<std::collections::HashSet<(u64, u32)>> {
    let output = sql_query(
        args,
        &format!(
            "SELECT key, shard_ordinal FROM game_vmap_generation_receipt WHERE generation_id = {generation_id}"
        ),
    )?;
    let value: serde_json::Value =
        serde_json::from_str(&output).context("parse JSON SQL vmap generation-chunk response")?;
    let mut accepted = std::collections::HashSet::new();
    for row in sql_column_rows(&value, &["key", "shard_ordinal"]) {
        if let (Some(key), Some(ordinal)) = (row[0].as_u64(), row[1].as_u64()) {
            if let Ok(ordinal) = u32::try_from(ordinal) {
                accepted.insert((key, ordinal));
            }
        }
    }
    Ok(accepted)
}

fn active_generation_status(args: &crate::Args, map_id: u32) -> Result<()> {
    let output = sql_query(
        args,
        &format!(
            "SELECT id, map_id, state, expected_chunks, accepted_chunks, expected_bytes, manifest_digest, source_identity, selection_identity FROM game_vmap_generation WHERE map_id = {map_id} AND state = 2"
        ),
    )?;
    println!(
        "vmap active-generation status for map {map_id}:\n{}",
        output.trim()
    );
    Ok(())
}

/// A vmap generation follows already-owned spatial data. Fail before opening client archives when
/// this database cannot authoritatively serve the requested map.
fn preflight_map_ownership(args: &crate::Args, map_id: u32) -> Result<()> {
    let query = |table: &str| -> Result<String> {
        sql_query(args, &format!("SELECT map_id FROM {table}"))
            .with_context(|| format!("query {table} map ownership"))
    };
    let terrain = query("game_terrain_chunk")?;
    let nav = query("game_nav_chunk")?;
    if sql_has_map(&terrain, map_id)? && sql_has_map(&nav, map_id)? {
        Ok(())
    } else {
        bail!("target database {} does not own complete spatial map {map_id} (terrain and nav ownership are both required)", args.db)
    }
}

pub(crate) fn run(args: &crate::Args) -> Result<()> {
    if args.vmap_status {
        if args.vmap.is_none() {
            return active_generation_status(args, args.map as u32);
        }
        bail!("--vmap-status is a standalone status command; omit --vmap");
    }
    let scope = args.world_import_scope()?;
    if scope.bounded_slices.is_empty() {
        println!(
            "vmap: scope {} has no bounded map slices; no vmap generation to import",
            scope.name()
        );
        return Ok(());
    }
    let mut slices_by_map = BTreeMap::new();
    for slice in &scope.bounded_slices {
        slices_by_map
            .entry(slice.map_id)
            .or_insert_with(Vec::new)
            .push(slice);
    }
    for &map_id in slices_by_map.keys() {
        let map_id = u32::try_from(map_id).context("bounded slice map id is outside u32")?;
        preflight_map_ownership(args, map_id)
            .with_context(|| format!("vmap scope {} map {map_id}", scope.name()))?;
    }
    let data_dir = Path::new(args.vmap.as_ref().expect("caller checked"));
    let mut chain = crate::collision::open_geometry_chain(data_dir)?;
    let source_identity = format!("client-data-path:{}", data_dir.display());
    let mut plans = Vec::with_capacity(slices_by_map.len());
    for (map_id, slices) in slices_by_map {
        plans.push(
            build_plan(
                &mut chain,
                map_id,
                &slices,
                &source_identity,
                scope.name() != "legacy",
            )
            .with_context(|| {
                let names = slices
                    .iter()
                    .map(|slice| slice.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("vmap scope {} map {map_id} slices {names}", scope.name())
            })?,
        );
    }
    for plan in &plans {
        println!(
            "vmap: map {} — {} world tris ({} WMO, {} M2) across {} cells, {} packed bytes ({:.1} KB, {} shard row(s))",
            plan.map_id,
            plan.world_tris,
            plan.wmo_tris,
            plan.world_tris - plan.wmo_tris,
            plan.keys.len(),
            plan.total_bytes,
            plan.total_bytes as f64 / 1024.0,
            plan.shard_rows
        );
        println!(
            "vmap: manifest {} generation {}",
            plan.manifest, plan.generation_id
        );
    }
    if !args.apply {
        println!("-- DRY RUN: would stage, append, verify, and activate each map generation");
        return Ok(());
    }
    for plan in &plans {
        apply_plan(args, plan)?;
    }
    println!("vmap: applied.");
    Ok(())
}

struct VmapPlan {
    map_id: u32,
    source_identity: String,
    selection_identity: String,
    spool: VmapSpool,
    keys: Vec<u64>,
    total_bytes: usize,
    shard_rows: usize,
    manifest: String,
    generation_id: u64,
    world_tris: usize,
    wmo_tris: usize,
}

fn build_plan(
    chain: &mut PatchChain,
    map_id: i64,
    slices: &[&crate::world_import_scope::BoundedMapSlice],
    source_identity: &str,
    restrict_to_scope: bool,
) -> Result<VmapPlan> {
    let map_id = u32::try_from(map_id).context("bounded slice map id is outside u32")?;
    let map_name = crate::terrain::map_dir(map_id)?;
    let mut selected_cells = BTreeSet::new();
    let mut placements = Vec::new();
    let mut placement_keys = HashSet::new();
    let mut tiles_read = 0u32;
    let mut selections = Vec::with_capacity(slices.len());
    let mut ranges = Vec::with_capacity(slices.len());
    for slice in slices {
        let range = crate::terrain::slice_cell_range(Some(slice.bounds), slice.sample, 0.0);
        for cell in [range.0, range.1, range.2, range.3] {
            if !(0..1024).contains(&cell) {
                bail!(
                    "slice {} has cell index {cell} outside the map square",
                    slice.name
                );
            }
        }
        let scan = crate::nav::scan_tiles(chain, map_name, map_id, range)?;
        if scan.cells.is_empty() {
            bail!("slice {} has no intersecting MCNK cells", slice.name);
        }
        let sample_cell = lyracore_shared::terrain::cell_index(slice.sample.0 as f32)
            .zip(lyracore_shared::terrain::cell_index(slice.sample.1 as f32));
        if !scan
            .cells
            .iter()
            .any(|row| Some((row.cell_x, row.cell_y)) == sample_cell)
        {
            bail!(
                "slice {} sample is not covered by its terrain cells",
                slice.name
            );
        }
        tiles_read += scan.tiles_read;
        selected_cells.extend(
            scan.cells
                .iter()
                .map(|row| cell_key(map_id, row.cell_x, row.cell_y)),
        );
        for placement in scan.placements {
            let key = placement_key(&placement);
            if placement_keys.insert(key) {
                placements.push(placement);
            }
        }
        selections.push(format!(
            "{}:{}..{}:{}..{}",
            slice.name, range.0, range.1, range.2, range.3
        ));
        ranges.push(range);
    }
    println!(
        "vmap: {tiles_read} tile(s), {} selected cells, {} unique placements ({} WMO)",
        selected_cells.len(),
        placements.len(),
        placements
            .iter()
            .filter(|placement| placement.is_wmo)
            .count()
    );
    let conv = crate::nav::calibrate_from_placements(chain, &placements)?;
    let spool = VmapSpool::new(map_id)?;
    placements.sort_by(|a, b| {
        a.name.cmp(&b.name).then_with(|| {
            a.position
                .iter()
                .map(|v| v.to_bits())
                .cmp(b.position.iter().map(|v| v.to_bits()))
        })
    });
    let mut world_tris = 0usize;
    let mut wmo_tris = 0usize;
    for (i, placement) in placements.iter().enumerate() {
        let mesh = crate::nav::load_mesh(chain, placement)?;
        let mut spool_error = None;
        for_each_placement_tri(placement, &mesh, conv, |tri| {
            if spool_error.is_some() {
                return;
            }
            world_tris += 1;
            wmo_tris += usize::from(matches!(tri.class, TriClass::Wmo { .. }));
            if let Some((cx0, cx1, cy0, cy1)) = tri_cell_range(&tri) {
                for cx in cx0..=cx1 {
                    for cy in cy0..=cy1 {
                        let key = cell_key(map_id, cx, cy);
                        if !restrict_to_scope || selected_cells.contains(&key) {
                            if let Err(err) = spool.append(key, tri) {
                                spool_error = Some(err);
                                return;
                            }
                        }
                    }
                }
            }
        });
        if let Some(err) = spool_error {
            return Err(err).with_context(|| format!("spooling placement {}", placement.name));
        }
        if (i + 1) % 100 == 0 {
            println!(
                "vmap: transformed {}/{} placements",
                i + 1,
                placements.len()
            );
        }
    }

    let keys = spool.keys()?;
    let mut total_bytes = 0usize;
    let mut shard_rows = 0usize;
    let mut digest = blake3::Hasher::new();
    digest.update(b"lyracore-vmap-manifest-v1");
    for &key in &keys {
        let mut ordinal = 0u32;
        spool.for_each_shard(key, |blob| {
            total_bytes += blob.len();
            shard_rows += 1;
            digest.update(&key.to_le_bytes());
            digest.update(&ordinal.to_le_bytes());
            digest.update(&(blob.len() as u32).to_le_bytes());
            digest.update(&blob);
            ordinal += 1;
            Ok(())
        })?;
    }
    let manifest = digest.finalize().to_hex().to_string();
    let generation_id = u64::from_le_bytes(
        blake3::hash(manifest.as_bytes()).as_bytes()[..8]
            .try_into()
            .unwrap(),
    );
    Ok(VmapPlan {
        map_id,
        source_identity: source_identity.to_owned(),
        selection_identity: if restrict_to_scope {
            format!("map={map_id};slices={}", selections.join(","))
        } else {
            let (x0, x1, y0, y1) = ranges[0];
            format!("map={map_id};cell_x={x0}..{x1};cell_y={y0}..{y1}")
        },
        spool,
        keys,
        total_bytes,
        shard_rows,
        manifest,
        generation_id,
        world_tris,
        wmo_tris,
    })
}

fn placement_key(placement: &Placement) -> String {
    format!(
        "{}:{}:{:?}:{:?}:{}",
        placement.is_wmo,
        placement.name,
        placement.position.map(f32::to_bits),
        placement.rotation.map(f32::to_bits),
        placement.scale.to_bits()
    )
}

fn apply_plan(args: &crate::Args, plan: &VmapPlan) -> Result<()> {
    let generation_id = plan.generation_id.to_string();
    let map_id = plan.map_id.to_string();
    let expected_chunks = plan.shard_rows.to_string();
    let expected_bytes = plan.total_bytes.to_string();
    crate::call_reducer_args(
        args,
        "stage_vmap_generation",
        &[
            &generation_id,
            &map_id,
            &expected_chunks,
            &expected_bytes,
            &plan.manifest,
            &plan.source_identity,
            &plan.selection_identity,
        ],
    )?;
    let accepted = accepted_chunk_ids(args, plan.generation_id)?;
    let mut batches = 0usize;
    let mut skipped = 0usize;
    let mut current = String::new();
    for &key in &plan.keys {
        let mut ordinal = 0u32;
        plan.spool.for_each_shard(key, |blob| {
            if accepted.contains(&(key, ordinal)) {
                skipped += 1;
                ordinal += 1;
                return Ok(());
            }
            let cell_x = ((key >> 16) & 0xFFFF) as u16;
            let cell_y = (key & 0xFFFF) as u16;
            let hex: String = blob.iter().map(|b| format!("{b:02x}")).collect();
            let row = format!("{ordinal},{map_id},{cell_x},{cell_y},{hex}");
            if !current.is_empty() && current.len() + row.len() + 1 > BATCH_BYTES {
                crate::call_reducer_args(
                    args,
                    "append_vmap_generation_chunks",
                    &[&generation_id, &current],
                )?;
                batches += 1;
                current.clear();
            }
            if !current.is_empty() {
                current.push(';');
            }
            current.push_str(&row);
            ordinal += 1;
            Ok(())
        })?;
    }
    if !current.is_empty() {
        crate::call_reducer_args(
            args,
            "append_vmap_generation_chunks",
            &[&generation_id, &current],
        )?;
        batches += 1;
    }
    crate::call_reducer_args(args, "verify_vmap_generation", &[&generation_id])?;
    crate::call_reducer_args(args, "activate_vmap_generation", &[&generation_id])?;
    println!("vmap: activated generation {generation_id} after {batches} batch(es), skipping {skipped} already-accepted shard row(s)");
    Ok(())
}

/// Byte budget per `spacetime call` payload, grouping several shard rows into one call (matches
/// `nav.rs`'s `BATCH_BYTES`).
const BATCH_BYTES: usize = 28_000;

/// Hard cap on ONE shard's packed byte size, well under the ~128 KB (`MAX_ARG_STRLEN`) a single
/// Linux argv string can hold even after this blob doubles in size as hex text — the failure this
/// guards was measured live (`Argument list too long`, os error 7) importing a dense WMO cell near
/// the Northshire abbey before this cap existed.
const MAX_ROW_TRI_BYTES: usize = 20_000;

/// Split one cell's triangle list into `lyracore_shared::vmap::encode`d shards, each at most
/// `MAX_ROW_TRI_BYTES`. Order within a cell doesn't matter (the module concatenates every shard's
/// decoded triangles back into one list — see `module/src/vmap.rs`'s `fetcher`), so this just
/// chunks by triangle count.
#[cfg(test)]
fn shard_cell(tris: &[VmapTri]) -> Vec<Vec<u8>> {
    let per_shard = ((MAX_ROW_TRI_BYTES.saturating_sub(HEADER_BYTES)) / TRI_BYTES).max(1);
    if tris.is_empty() {
        return vec![encode(tris)]; // preserve the header-only "cell exists but empty" row
    }
    tris.chunks(per_shard).map(encode).collect()
}

// =============================================================================================
//  `--vmap-prepare-coverage <generation_id>` — drives `prepare_vmap_nav_coverage` /
//  `finalize_vmap_nav_coverage` (module/src/vmap.rs) over a generation's OWN staged chunks. No
//  client Data/ dir: the derivation runs module-side from data already staged by an earlier
//  `--vmap --apply` run, so this workflow only enumerates cells and drives batches.
// =============================================================================================

/// The generation's map + lifecycle state, read before any coverage work so an ineligible target
/// fails closed the same way `preflight_map_ownership` does for `--vmap`.
struct GenerationInfo {
    map_id: u32,
    state: u64,
}

/// Lifecycle state codes mirrored from `module/src/vmap.rs` (no shared crate for a bare u8 enum).
const VERIFIED: u64 = 1;
const ACTIVE: u64 = 2;

fn generation_info(args: &crate::Args, generation_id: u64) -> Result<GenerationInfo> {
    let output = sql_query(
        args,
        &format!("SELECT map_id, state FROM game_vmap_generation WHERE id = {generation_id}"),
    )?;
    let value: serde_json::Value =
        serde_json::from_str(&output).context("parse JSON SQL vmap generation response")?;
    let row = sql_column_rows(&value, &["map_id", "state"])
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("unknown vmap generation {generation_id}"))?;
    let map_id = row[0]
        .as_u64()
        .context("vmap generation map_id")? as u32;
    let state = row[1].as_u64().context("vmap generation state")?;
    Ok(GenerationInfo { map_id, state })
}

/// Every `(cell_x, cell_y)` the generation's OWN staged chunks touch, deduped across shard rows —
/// the driving enumeration for coverage preparation (never `game_vmap_chunk`, the live table).
fn generation_cells(args: &crate::Args, generation_id: u64) -> Result<Vec<(u16, u16)>> {
    let output = sql_query(
        args,
        &format!(
            "SELECT cell_x, cell_y FROM game_vmap_generation_chunk WHERE generation_id = {generation_id}"
        ),
    )?;
    let value: serde_json::Value =
        serde_json::from_str(&output).context("parse JSON SQL vmap generation-chunk cells")?;
    let mut cells: std::collections::BTreeSet<(u16, u16)> = std::collections::BTreeSet::new();
    for row in sql_column_rows(&value, &["cell_x", "cell_y"]) {
        if let (Some(x), Some(y)) = (row[0].as_u64(), row[1].as_u64()) {
            cells.insert((x as u16, y as u16));
        }
    }
    Ok(cells.into_iter().collect())
}

/// Cells this generation already has a `VmapNavCoverage` row for — read directly (the operator
/// identity that runs this workflow owns the database, same as every other SQL query here; the
/// table's privacy only hides it from ordinary subscribing clients).
fn covered_cells(
    args: &crate::Args,
    generation_id: u64,
) -> Result<std::collections::HashSet<(u16, u16)>> {
    let output = sql_query(
        args,
        &format!(
            "SELECT cell_x, cell_y FROM game_vmap_nav_coverage WHERE generation_id = {generation_id}"
        ),
    )?;
    let value: serde_json::Value =
        serde_json::from_str(&output).context("parse JSON SQL vmap nav coverage cells")?;
    let mut cells = std::collections::HashSet::new();
    for row in sql_column_rows(&value, &["cell_x", "cell_y"]) {
        if let (Some(x), Some(y)) = (row[0].as_u64(), row[1].as_u64()) {
            cells.insert((x as u16, y as u16));
        }
    }
    Ok(cells)
}

/// Cells still needing preparation: every generation cell not already covered. Pure so the resume
/// skip is testable without a live database — mirrors `accepted_chunk_ids`'s already-accepted skip
/// for the base import above, one level up (whole cells instead of shard rows).
fn pending_cells(
    all: &[(u16, u16)],
    already_covered: &std::collections::HashSet<(u16, u16)>,
) -> Vec<(u16, u16)> {
    all.iter()
        .copied()
        .filter(|c| !already_covered.contains(c))
        .collect()
}

/// Split `cells` into JSON-array `prepare_vmap_nav_coverage` arguments, each bounded by
/// `BATCH_BYTES` like every other reducer batch in this file. Batch boundaries are an efficiency
/// detail only — the reducer is idempotent per cell, so splitting differently reproduces the same
/// final coverage set and manifest digest (deterministic coverage identity).
fn coverage_batches(cells: &[(u16, u16)]) -> Vec<String> {
    let mut batches = Vec::new();
    let mut cur = String::from("[");
    for &(x, y) in cells {
        let entry = format!("{{\"cell_x\":{x},\"cell_y\":{y}}}");
        if cur.len() > 1 && cur.len() + entry.len() + 1 > BATCH_BYTES {
            cur.push(']');
            batches.push(std::mem::replace(&mut cur, String::from("[")));
        }
        if cur.len() > 1 {
            cur.push(',');
        }
        cur.push_str(&entry);
    }
    if cur.len() > 1 {
        cur.push(']');
        batches.push(cur);
    }
    batches
}

/// `--vmap-prepare-coverage <generation_id>`: derive and store nav coverage for an already-staged
/// generation, resumable and manifest-reporting. Reuses `preflight_map_ownership` fail-closed —
/// coverage never prepares on a shard that does not own the map — and, like `--vmap`, only writes
/// with `--apply`; without it this reports the plan and touches nothing.
pub(crate) fn run_coverage(args: &crate::Args, generation_id: u64) -> Result<()> {
    let generation = generation_info(args, generation_id)?;
    preflight_map_ownership(args, generation.map_id)?;
    if !matches!(generation.state, VERIFIED | ACTIVE) {
        bail!("vmap generation {generation_id} is not verified or active (state {}) — coverage preparation refused", generation.state);
    }

    let all_cells = generation_cells(args, generation_id)?;
    if all_cells.is_empty() {
        bail!("vmap generation {generation_id} has no staged chunks to derive coverage from");
    }
    let already = covered_cells(args, generation_id)?;
    let pending = pending_cells(&all_cells, &already);
    let batches = coverage_batches(&pending);

    println!(
        "vmap-coverage: generation {generation_id} map {} — {} cell(s) total, {} already prepared, {} pending in {} batch(es)",
        generation.map_id,
        all_cells.len(),
        already.len(),
        pending.len(),
        batches.len()
    );
    if !args.apply {
        println!("-- DRY RUN: would call prepare_vmap_nav_coverage ({} batch(es)) + finalize_vmap_nav_coverage", batches.len());
        return Ok(());
    }

    for batch in &batches {
        crate::call_reducer_args(
            args,
            "prepare_vmap_nav_coverage",
            &[&generation_id.to_string(), batch],
        )?;
    }
    crate::call_reducer_args(
        args,
        "finalize_vmap_nav_coverage",
        &[&generation_id.to_string()],
    )?;

    let manifest = sql_query(
        args,
        &format!(
            "SELECT generation_id, map_id, cell_count, digest, complete FROM game_vmap_nav_coverage_manifest WHERE generation_id = {generation_id}"
        ),
    )?;
    println!("vmap-coverage: applied. manifest:\n{}", manifest.trim());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::spatial::MAP_COORD_MAX;
    use lyracore_shared::terrain::CELL_SIZE;

    const MAP: u32 = 0;

    /// World-coord bounds of terrain cell (cx, cy): (x_hi, x_lo, y_hi, y_lo) — cell indices count
    /// DOWN as world coords grow, same convention as `terrain::cell_index`.
    fn cell_bounds(cx: u16, cy: u16) -> (f32, f32, f32, f32) {
        let x_hi = MAP_COORD_MAX - cx as f32 * CELL_SIZE;
        let x_lo = x_hi - CELL_SIZE;
        let y_hi = MAP_COORD_MAX - cy as f32 * CELL_SIZE;
        let y_lo = y_hi - CELL_SIZE;
        (x_hi, x_lo, y_hi, y_lo)
    }

    fn tri_at(x: f32, y: f32, z: f32, half: f32, class: TriClass) -> VmapTri {
        VmapTri {
            verts: [[x - half, y, z], [x + half, y, z], [x, y + half, z]],
            class,
        }
    }

    /// Identity convention (no axis shuffle, yaw = rotation[1] verbatim) — isolates the
    /// pitch/roll math in `apply_full` from the calibrated-convention machinery so the assertions
    /// below are hand-computable straight from the doc comment's `Rz(yaw)*Ry(pitch)*Rx(roll)`
    /// formula.
    fn identity_convention() -> crate::nav::Convention {
        crate::nav::Convention {
            shuffle: false,
            sign: 1.0,
            offset_deg: 0.0,
        }
    }

    #[test]
    fn roll_of_90_degrees_rotates_local_y_onto_world_z() {
        // rotation = [pitch=0, yaw=0, roll=90]. Rx(90) sends the local Y axis to local/world Z
        // (roll is applied about local X first, before pitch/yaw, both of which are identity here).
        let out = apply_full(
            identity_convention(),
            [0.0, 0.0, 90.0],
            1.0,
            [0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
        );
        assert!((out[0] - 0.0).abs() < 1e-4, "x: {out:?}");
        assert!((out[1] - 0.0).abs() < 1e-4, "y: {out:?}");
        assert!((out[2] - 1.0).abs() < 1e-4, "z: {out:?}");
    }

    #[test]
    fn pitch_of_90_degrees_rotates_local_x_onto_negative_world_z() {
        // rotation = [pitch=90, yaw=0, roll=0]. Ry(90) sends the local X axis to −Z (roll and yaw
        // are identity here, so this isolates the pitch term).
        let out = apply_full(
            identity_convention(),
            [90.0, 0.0, 0.0],
            1.0,
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
        );
        assert!((out[0] - 0.0).abs() < 1e-4, "x: {out:?}");
        assert!((out[1] - 0.0).abs() < 1e-4, "y: {out:?}");
        assert!((out[2] - -1.0).abs() < 1e-4, "z: {out:?}");
    }

    #[test]
    fn roll_then_pitch_composition_order_is_roll_first() {
        // rotation = [pitch=90, yaw=0, roll=90], v = local Y. Isolated-axis tests above can't catch
        // an axis-order swap (both leave a lone X or lone Y vector on a single output axis either
        // way); this vector has zero X/Z components, so the two orders diverge. Roll-then-pitch
        // (the order `apply_full` applies): Rx(90) sends local Y (0,1,0) to local Z (0,0,1), then
        // Ry(90) sends local Z (0,0,1) to world X (1,0,0). Pitch-then-roll would instead leave the
        // vector at local Y all the way through pitch (Ry(90) doesn't touch a pure-Y vector), then
        // Rx(90) would send it to (0,0,1) — a different result, so this test pins the order.
        let out = apply_full(
            identity_convention(),
            [90.0, 0.0, 90.0],
            1.0,
            [0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
        );
        assert!((out[0] - 1.0).abs() < 1e-4, "x: {out:?}");
        assert!((out[1] - 0.0).abs() < 1e-4, "y: {out:?}");
        assert!((out[2] - 0.0).abs() < 1e-4, "z: {out:?}");
    }

    #[test]
    fn a_triangle_wholly_inside_one_cell_lands_in_exactly_one_cell() {
        let (x_hi, x_lo, y_hi, y_lo) = cell_bounds(500, 500);
        let (cx, cy) = ((x_hi + x_lo) / 2.0, (y_hi + y_lo) / 2.0);
        let t = tri_at(cx, cy, 100.0, 1.0, TriClass::M2);
        let by_cell = bin_by_cell(MAP, &[t]);
        assert_eq!(by_cell.len(), 1);
        let only = by_cell.values().next().unwrap();
        assert_eq!(only.len(), 1);
        assert_eq!(*only.iter().next().unwrap(), t);
    }

    #[test]
    fn a_triangle_spanning_a_cell_boundary_lands_in_every_cell_it_touches() {
        // Straddle the x boundary between cell 500 and 501 (a wide-enough triangle on the x axis,
        // centered exactly on the boundary), fully inside one cell on the y axis.
        let (x_hi_500, _x_lo_500, y_hi, y_lo) = cell_bounds(500, 500);
        let boundary_x = x_hi_500; // the coordinate separating cell 500 from cell 501
        let cy = (y_hi + y_lo) / 2.0;
        let t = tri_at(boundary_x, cy, 100.0, CELL_SIZE * 0.4, TriClass::M2);
        let by_cell = bin_by_cell(MAP, &[t]);
        assert_eq!(by_cell.len(), 2, "must land in both straddled cells");
        for tris in by_cell.values() {
            assert_eq!(tris.len(), 1);
            assert_eq!(tris[0], t);
        }
    }

    #[test]
    fn wmo_and_m2_classification_survives_binning_and_packing() {
        let (x_hi, x_lo, y_hi, y_lo) = cell_bounds(500, 500);
        let (cx, cy) = ((x_hi + x_lo) / 2.0, (y_hi + y_lo) / 2.0);
        let wmo = tri_at(
            cx,
            cy,
            100.0,
            1.0,
            TriClass::Wmo {
                group_id: 7,
                mogp_flags: 0x2000,
            },
        );
        let m2 = tri_at(cx, cy, 105.0, 1.0, TriClass::M2);
        let by_cell = bin_by_cell(MAP, &[wmo, m2]);
        assert_eq!(by_cell.len(), 1);
        let tris = by_cell.values().next().unwrap();
        assert_eq!(tris.len(), 2);
        let blob = encode(tris);
        let back = lyracore_shared::vmap::decode(&blob).expect("round trip");
        assert!(back.contains(&wmo));
        assert!(back.contains(&m2));
    }

    /// A cell whose triangle count fits comfortably under `MAX_ROW_TRI_BYTES` shards to exactly
    /// ONE blob — the common case shouldn't multiply row count for no reason.
    #[test]
    fn a_small_cell_shards_to_exactly_one_blob() {
        let tris: Vec<VmapTri> = (0..5)
            .map(|i| tri_at(0.0, 0.0, i as f32, 1.0, TriClass::M2))
            .collect();
        let shards = shard_cell(&tris);
        assert_eq!(shards.len(), 1);
        let back = lyracore_shared::vmap::decode(&shards[0]).expect("round trip");
        assert_eq!(back.len(), 5);
    }

    /// A dense cell (more triangles than one `MAX_ROW_TRI_BYTES`-capped blob can hold) splits into
    /// several shards, each individually decodable and each within the byte cap; the union of
    /// every shard's decoded triangles recovers the exact original set (unordered) — this is the
    /// live-measured fix for the `Argument list too long` failure importing a dense WMO cell near
    /// the Northshire abbey (a single oversized row exceeded Linux's ~128 KB argv-string limit
    /// before this cap existed).
    #[test]
    fn a_dense_cell_splits_into_byte_capped_shards_that_reassemble_exactly() {
        let per_shard = (MAX_ROW_TRI_BYTES - HEADER_BYTES) / TRI_BYTES;
        let n = per_shard * 3 + 7; // spans multiple shards, last one partial
        let tris: Vec<VmapTri> = (0..n)
            .map(|i| tri_at(0.0, 0.0, i as f32, 1.0, TriClass::M2))
            .collect();
        let shards = shard_cell(&tris);
        assert!(
            shards.len() >= 4,
            "expected at least 4 shards, got {}",
            shards.len()
        );
        for s in &shards {
            assert!(
                s.len() <= MAX_ROW_TRI_BYTES,
                "shard exceeds the byte cap: {} > {MAX_ROW_TRI_BYTES}",
                s.len()
            );
        }
        let mut reassembled: Vec<VmapTri> = shards
            .iter()
            .flat_map(|b| lyracore_shared::vmap::decode(b).expect("shard round trip"))
            .collect();
        let mut expected = tris.clone();
        // Order isn't part of the contract (the module concatenates shards in storage order, not
        // triangle-original order) — compare as multisets via a stable sort on the z coordinate,
        // which is unique per triangle in this fixture.
        reassembled.sort_by(|a, b| a.verts[0][2].partial_cmp(&b.verts[0][2]).unwrap());
        expected.sort_by(|a, b| a.verts[0][2].partial_cmp(&b.verts[0][2]).unwrap());
        assert_eq!(reassembled, expected);
    }

    #[test]
    fn disk_spool_orders_cells_and_bounds_live_shard_memory() {
        let spool = VmapSpool::new(77).unwrap();
        let per_shard = (MAX_ROW_TRI_BYTES - HEADER_BYTES) / TRI_BYTES;
        // Insert deliberately reverse key order; output must be keyed order, independent of
        // filesystem enumeration. Each cell is read one shard at a time, not as the full slice.
        for key in [9u64, 3] {
            for i in 0..(per_shard * 2 + 1) {
                spool
                    .append(key, tri_at(0.0, 0.0, i as f32, 1.0, TriClass::M2))
                    .unwrap();
            }
        }
        assert_eq!(spool.keys().unwrap(), vec![3, 9]);
        for key in spool.keys().unwrap() {
            let mut count = 0;
            spool
                .for_each_shard(key, |shard| {
                    assert!(shard.len() <= MAX_ROW_TRI_BYTES);
                    if count == 0 {
                        assert_eq!(
                            lyracore_shared::vmap::decode(&shard).unwrap().len(),
                            per_shard
                        );
                    }
                    count += 1;
                    Ok(())
                })
                .unwrap();
            assert_eq!(count, 3);
        }
    }

    #[test]
    fn placement_key_deduplicates_an_overlap_without_merging_distinct_placements() {
        let placement = Placement {
            name: "World\\wmo\\test.wmo".to_owned(),
            is_wmo: true,
            position: [1.0, 2.0, 3.0],
            rotation: [0.0, 90.0, 0.0],
            scale: 1.0,
            bounds_min: None,
            bounds_max: None,
        };
        let mut shifted = Placement {
            name: placement.name.clone(),
            is_wmo: placement.is_wmo,
            position: placement.position,
            rotation: placement.rotation,
            scale: placement.scale,
            bounds_min: None,
            bounds_max: None,
        };

        assert_eq!(placement_key(&placement), placement_key(&placement));
        shifted.position[0] += 1.0;
        assert_ne!(placement_key(&placement), placement_key(&shifted));
    }

    #[test]
    fn ownership_parser_accepts_the_requested_map_and_rejects_another() {
        let rows = r#"[{"schema":{"elements":[{"name":{"some":"map_id"}}]},"rows":[[0],[36]]}]"#;
        assert!(sql_has_map(rows, 0).unwrap());
        assert!(sql_has_map(rows, 36).unwrap());
        assert!(!sql_has_map(rows, 1).unwrap());
        assert!(!sql_has_map(
            r#"[{"schema":{"elements":[{"name":{"some":"map_id"}}]},"rows":[]}]"#,
            0
        )
        .unwrap());
    }

    #[test]
    fn pending_cells_skips_what_a_prior_run_already_covered() {
        let all = vec![(1, 1), (2, 2), (3, 3)];
        let mut covered = std::collections::HashSet::new();
        covered.insert((2, 2));
        assert_eq!(pending_cells(&all, &covered), vec![(1, 1), (3, 3)]);
        assert!(pending_cells(&all, &std::collections::HashSet::new()) == all);
        let all_covered: std::collections::HashSet<_> = all.iter().copied().collect();
        assert!(pending_cells(&all, &all_covered).is_empty(), "a fully resumed run submits nothing");
    }

    /// A fixture too large for one `BATCH_BYTES` call must split into several, and the split must
    /// be lossless and order-independent — batch boundaries are an efficiency detail, not part of
    /// coverage identity (the reducer keys each cell independently).
    #[test]
    fn coverage_batches_split_a_fixture_larger_than_one_batch_and_reassemble_exactly() {
        let cells: Vec<(u16, u16)> = (0..2000u32).map(|i| (i as u16, (i * 7 % 997) as u16)).collect();
        let batches = coverage_batches(&cells);
        assert!(batches.len() > 1, "fixture must not fit in one batch");
        for b in &batches {
            assert!(
                b.len() <= BATCH_BYTES,
                "batch exceeds the byte cap: {} > {BATCH_BYTES}",
                b.len()
            );
        }
        let mut reassembled: Vec<(u16, u16)> = batches
            .iter()
            .flat_map(|b| {
                let v: Vec<serde_json::Value> = serde_json::from_str(b).unwrap();
                v.into_iter().map(|c| {
                    (
                        c["cell_x"].as_u64().unwrap() as u16,
                        c["cell_y"].as_u64().unwrap() as u16,
                    )
                })
            })
            .collect();
        let mut expected = cells.clone();
        reassembled.sort_unstable();
        expected.sort_unstable();
        assert_eq!(
            reassembled, expected,
            "batch boundaries never drop or duplicate a cell"
        );
    }

    #[test]
    fn coverage_batches_of_an_empty_set_is_empty() {
        assert!(coverage_batches(&[]).is_empty());
    }
}
