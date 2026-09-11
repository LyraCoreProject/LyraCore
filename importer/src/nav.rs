//! `--nav <client Data/ dir>` — work-item 241: rasterize ADT terrain + WMO/M2 collision
//! geometry into per-cell nav blobs → `game_nav_chunk` (decision #8: grid tables, no
//! vmap/Detour runtime). Same licensing firewall as `--terrain`. Honors `--map` + `--box`.
//!
//! Pipeline: parse every ADT tile in the box (heights via terrain.rs's `collect_cells` — the
//! SAME code the heightmap import runs), collect WMO/M2 placements deduped by `unique_id`
//! (placements repeat on adjacent tiles), load each referenced model's collision triangles
//! once, transform to world space, bin by terrain cell, then rasterize each cell's 64×64
//! walkability bitmask + 32×32 obstruction-height grid through `lyracore_shared::nav` (the same
//! codec the module reads with). The geometry rasterization policy itself lives in
//! `nav::derive_cell`; this file only adds the terrain-only blockers (slope, MCNK holes) and
//! merges the two through `nav::merge_cells`.
//!
//! The placement ROTATION convention is folkloric (like the ADT filename axis order), so it is
//! CALIBRATED, not trusted: MODF carries each WMO placement's world AABB; we try the candidate
//! (axis-shuffle × yaw) transforms and keep the one that reproduces those AABBs, bailing loudly
//! if none does. MDDF (M2) reuses the WMO-calibrated convention — it has no bounds to check.
// Deliberate simplification: yaw-only rotation (placements with real pitch/roll rasterize
// slightly wrong — rare fence-on-slope doodads); tri footprint = tri AABB (overmarks diagonal
// walls by <1 cell, conservative direction). Move to exact tri-box + full Euler when a live case
// demands it.

use anyhow::{bail, Context, Result};
use lyracore_shared::nav::{
    derive_cell, merge_cells, sub_index, walk_set, NavCellData, OBS_BYTES, OBS_NONE, WALK_BYTES,
    WALK_DIM, WALK_MARGIN,
};
use lyracore_shared::terrain::{cell_index, cell_key};
use lyracore_shared::vmap::{TriClass, VmapTri};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Cursor;
use std::path::Path;
use wow_mpq::PatchChain;

/// 32 tiles × 533.33 yd — the ADT placement-space origin offset.
const PLACE_K: f32 = 32.0 * 533.333_3;
/// Slope gate: a 4.17 yd sub-quad rising more than tan(50°)×4.17 ≈ 4.97 yd is unwalkable.
const MAX_QUAD_RISE: f32 = 4.97;
/// Reducer batch ceiling (bytes of packed payload) — same ballpark the other importers respect.
const BATCH_BYTES: usize = 28_000;

pub(crate) type Tri = [[f32; 3]; 3];

// ---------------------------------------------------------------------------------------------
// Geometry extraction
// ---------------------------------------------------------------------------------------------

/// One WMO collision triangle plus the group it came from — `vmap.rs` (#520) packs this
/// per-triangle metadata (source class + group id + MOGP flags); `nav.rs`'s rasterizer only
/// needs `.tri`.
pub(crate) struct WmoTri {
    pub(crate) tri: Tri,
    pub(crate) group_id: u32,
    pub(crate) mogp_flags: u32,
}

/// One placement's loaded collision mesh — WMO carries per-triangle group metadata, M2 doesn't
/// (a doodad has no groups/MOGP flags).
pub(crate) enum Mesh {
    Wmo(Vec<WmoTri>),
    M2(Vec<Tri>),
}

impl Mesh {
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Wmo(tris) => tris.len(),
            Self::M2(tris) => tris.len(),
        }
    }
}

/// Collidable triangles of one WMO (all groups), model-local coords, tagged with group id + MOGP
/// flags. MOPY rule (wowdev): collidable = F_COLLISION (0x08) set, OR neither F_DETAIL (0x04) nor
/// F_NOCAMCOLLIDE (0x02).
fn wmo_tris(chain: &mut PatchChain, name: &str) -> Result<Vec<WmoTri>> {
    let root_bytes = chain
        .read_file(name)
        .with_context(|| format!("reading WMO root {name}"))?;
    let wow_wmo::ParsedWmo::Root(root) = wow_wmo::parse_wmo(&mut Cursor::new(&root_bytes))
        .with_context(|| format!("parsing WMO root {name}"))?
    else {
        bail!("{name} parsed as a group file, expected root");
    };
    let stem = name.strip_suffix(".wmo").unwrap_or(name);
    let mut tris = Vec::new();
    for g in 0..root.n_groups {
        let gname = format!("{stem}_{g:03}.wmo");
        let bytes = chain
            .read_file(&gname)
            .with_context(|| format!("reading {gname}"))?;
        let wow_wmo::ParsedWmo::Group(group) = wow_wmo::parse_wmo(&mut Cursor::new(&bytes))
            .with_context(|| format!("parsing {gname}"))?
        else {
            bail!("{gname} parsed as a root file, expected group");
        };
        let v = &group.vertex_positions;
        for (t, idx) in group.vertex_indices.chunks_exact(3).enumerate() {
            let collidable = group
                .material_info
                .get(t)
                .map(|m| m.flags & 0x08 != 0 || (m.flags & 0x04 == 0 && m.flags & 0x02 == 0))
                .unwrap_or(true);
            if !collidable {
                continue;
            }
            let (a, b, c) = (idx[0] as usize, idx[1] as usize, idx[2] as usize);
            if c >= v.len() || b >= v.len() || a >= v.len() {
                continue;
            }
            tris.push(WmoTri {
                tri: [
                    [v[a].x, v[a].y, v[a].z],
                    [v[b].x, v[b].y, v[b].z],
                    [v[c].x, v[c].y, v[c].z],
                ],
                group_id: group.group_index,
                mogp_flags: group.flags,
            });
        }
    }
    Ok(tris)
}

/// M2 collision mesh in model-local coordinates. Missing or malformed collision is an import
/// failure, since publishing an empty mesh would mark missing obstacles as clear ground.
pub(crate) fn m2_tris(chain: &mut PatchChain, name: &str) -> Result<Vec<Tri>> {
    let m2_name = name
        .rsplit_once('.')
        .map(|(stem, _)| format!("{stem}.m2"))
        .unwrap_or_else(|| name.to_string());
    let bytes = chain
        .read_file(&m2_name)
        .with_context(|| format!("reading M2 {m2_name}"))?;
    crate::m2_collision::triangles(&bytes)
        .with_context(|| format!("reading collision from M2 {m2_name}"))
}

// ---------------------------------------------------------------------------------------------
// Placement transform (calibrated, not trusted)
// ---------------------------------------------------------------------------------------------

/// Placement-space position → world: (K − p.z, K − p.x, p.y). Verified empirically in the 240
/// spike (nsabbey placement (17245, 80, 25964) → world (−8897, −178, 80), matching the live
/// abbey coords).
pub(crate) fn place_pos(p: [f32; 3]) -> [f32; 3] {
    [PLACE_K - p[2], PLACE_K - p[0], p[1]]
}

/// One candidate local→world convention: optional axis shuffle then yaw about world +Z.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Convention {
    /// false: local verts are already world-axis Z-up (v as-is).
    /// true: local verts are placement-axis Y-up (shuffle like `place_pos`: (−z, −x, y)).
    pub(crate) shuffle: bool,
    /// world yaw = `sign` × rot[1] + `offset_deg`, in degrees.
    pub(crate) sign: f32,
    pub(crate) offset_deg: f32,
}

fn apply(
    conv: Convention,
    rot_deg: [f32; 3],
    scale: f32,
    pos_w: [f32; 3],
    v: [f32; 3],
) -> [f32; 3] {
    let l = if conv.shuffle {
        [-v[2], -v[0], v[1]]
    } else {
        v
    };
    let l = [l[0] * scale, l[1] * scale, l[2] * scale];
    let theta = (conv.sign * rot_deg[1] + conv.offset_deg).to_radians();
    let (s, c) = theta.sin_cos();
    [
        pos_w[0] + c * l[0] - s * l[1],
        pos_w[1] + s * l[0] + c * l[1],
        pos_w[2] + l[2],
    ]
}

pub(crate) fn aabb(points: impl Iterator<Item = [f32; 3]>) -> ([f32; 3], [f32; 3]) {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}

fn mohd_calibration_points(bounds_min: [f32; 3], bounds_max: [f32; 3]) -> Result<Vec<[f32; 3]>> {
    if !bounds_min.into_iter().chain(bounds_max).all(f32::is_finite) {
        bail!("WMO MOHD bounds are not finite");
    }
    if (0..3).any(|axis| bounds_min[axis] > bounds_max[axis]) {
        bail!("WMO MOHD bounds are not ordered");
    }

    let mut points = Vec::with_capacity(8);
    for x in [bounds_min[0], bounds_max[0]] {
        for y in [bounds_min[1], bounds_max[1]] {
            for z in [bounds_min[2], bounds_max[2]] {
                points.push([x, y, z]);
            }
        }
    }
    Ok(points)
}

/// MOHD and MODF are the authored local and placed bounds of the same WMO. Comparing their
/// corners checks the placement transform without depending on how tightly MOVT vertices fill
/// the authored box.
fn wmo_calibration_points(chain: &mut PatchChain, name: &str) -> Result<Vec<[f32; 3]>> {
    let root_bytes = chain
        .read_file(name)
        .with_context(|| format!("reading WMO root {name}"))?;
    let wow_wmo::ParsedWmo::Root(root) = wow_wmo::parse_wmo(&mut Cursor::new(&root_bytes))
        .with_context(|| format!("parsing WMO root {name}"))?
    else {
        bail!("{name} parsed as a group file, expected root");
    };
    mohd_calibration_points(root.bounding_box_min, root.bounding_box_max)
        .with_context(|| format!("validating WMO root bounds {name}"))
}

/// Pick the convention that reproduces the MODF world AABBs across the calibration placements
/// (mean per-axis corner error, capped sample). Hard-fails above 1.5 yd — a wrong convention
/// must never silently rasterize rotated buildings.
pub(crate) struct Calibration {
    pub(crate) convention: Convention,
    pub(crate) q25_error_yards: f32,
}

fn calibration(samples: &[(&Placement, Vec<[f32; 3]>)]) -> Result<Calibration> {
    let mut candidates = Vec::new();
    for shuffle in [false, true] {
        for sign in [1.0f32, -1.0] {
            for offset_deg in [0.0f32, 90.0, 180.0, 270.0] {
                candidates.push(Convention {
                    shuffle,
                    sign,
                    offset_deg,
                });
            }
        }
    }
    let mut best: Option<(f32, Convention)> = None;
    for conv in candidates {
        let mut errs: Vec<f32> = Vec::new();
        for (p, calibration_points) in samples {
            let (Some(bmin), Some(bmax)) = (p.bounds_min, p.bounds_max) else {
                continue;
            };
            // Bounds are placement-space: convert both corners, re-sort per axis.
            let (c1, c2) = (place_pos(bmin), place_pos(bmax));
            let want_lo = [c1[0].min(c2[0]), c1[1].min(c2[1]), c1[2].min(c2[2])];
            let want_hi = [c1[0].max(c2[0]), c1[1].max(c2[1]), c1[2].max(c2[2])];
            let pos_w = place_pos(p.position);
            let (got_lo, got_hi) = aabb(
                calibration_points
                    .iter()
                    .map(|v| apply(conv, p.rotation, p.scale, pos_w, *v)),
            );
            let mut e = 0.0f32;
            for k in 0..3 {
                e += (got_lo[k] - want_lo[k]).abs() + (got_hi[k] - want_hi[k]).abs();
            }
            errs.push(e / 6.0);
        }
        if errs.is_empty() {
            continue;
        }
        // A low quantile keeps an isolated archive outlier from overriding the transform shared
        // by the retained sample. The refusal below still rejects a sample with no fit.
        errs.sort_by(f32::total_cmp);
        let mean = errs[errs.len() / 4];
        if std::env::var("NAV_DEBUG").is_ok() {
            eprintln!("NAV_DEBUG {conv:?} mean={mean:.2}");
            for (p, calibration_points) in samples {
                let (Some(bmin), Some(bmax)) = (p.bounds_min, p.bounds_max) else {
                    continue;
                };
                let (c1, c2) = (place_pos(bmin), place_pos(bmax));
                let pos_w = place_pos(p.position);
                let (got_lo, got_hi) = aabb(
                    calibration_points
                        .iter()
                        .map(|v| apply(conv, p.rotation, p.scale, pos_w, *v)),
                );
                let mut e = 0.0f32;
                for k in 0..3 {
                    e +=
                        (got_lo[k] - c1[k].min(c2[k])).abs() + (got_hi[k] - c1[k].max(c2[k])).abs();
                }
                eprintln!(
                    "  err={:6.2} rot=[{:6.1},{:6.1},{:6.1}] {}",
                    e / 6.0,
                    p.rotation[0],
                    p.rotation[1],
                    p.rotation[2],
                    p.name.rsplit('\\').next().unwrap_or(&p.name)
                );
            }
        }
        if best.is_none_or(|(b, _)| mean < b) {
            best = Some((mean, conv));
        }
    }
    let Some((err, conv)) = best else {
        bail!("nav: no WMO placement had both bounds and geometry — cannot calibrate rotation");
    };
    println!("nav: rotation calibration — {conv:?} (q25 AABB corner error {err:.2} yd)");
    if err > 1.5 {
        bail!(
            "nav: best rotation convention still misses MODF bounds by q25 {err:.2} yd — transform \
             is wrong, refusing to rasterize garbage"
        );
    }
    Ok(Calibration {
        convention: conv,
        q25_error_yards: err,
    })
}

// ---------------------------------------------------------------------------------------------
// Tile collection
// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
pub(crate) struct Placement {
    pub(crate) name: String,
    pub(crate) is_wmo: bool,
    pub(crate) unique_id: u32,
    pub(crate) position: [f32; 3],
    pub(crate) rotation: [f32; 3],
    pub(crate) scale: f32,
    pub(crate) bounds_min: Option<[f32; 3]>,
    pub(crate) bounds_max: Option<[f32; 3]>,
    pub(crate) wmo: Option<WmoPlacement>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct WmoPlacement {
    pub(crate) flags: u16,
    pub(crate) doodad_set: u16,
    pub(crate) name_set: u16,
}

pub(crate) struct TileSource {
    pub(crate) path: String,
    pub(crate) bytes: usize,
    pub(crate) blake3: String,
}

fn offset_to_index(names: &[String]) -> BTreeMap<u32, usize> {
    let mut map = BTreeMap::new();
    let mut off = 0u32;
    for (i, n) in names.iter().enumerate() {
        map.insert(off, i);
        off += n.len() as u32 + 1;
    }
    map
}

fn resolve<'a>(names: &'a [String], indices: &[u32], name_id: u32) -> Result<&'a str> {
    let offsets = offset_to_index(names);
    let off = *indices
        .get(name_id as usize)
        .with_context(|| format!("name_id {name_id} out of range"))?;
    let idx = *offsets
        .get(&off)
        .with_context(|| format!("offset {off} doesn't start a name"))?;
    Ok(&names[idx])
}

// ---------------------------------------------------------------------------------------------
// Rasterizer
// ---------------------------------------------------------------------------------------------

struct NavRow {
    map_id: u32,
    cell_x: u16,
    cell_y: u16,
    base_z: f32,
    walk: Vec<u8>,
    obs: Vec<u8>,
}

impl NavRow {
    fn packed(&self) -> String {
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        let walk = if self.walk.iter().all(|&b| b == 0xFF) {
            String::new()
        } else {
            hex(&self.walk)
        };
        let obs = if self.obs.iter().all(|&b| b == OBS_NONE) {
            String::new()
        } else {
            hex(&self.obs)
        };
        format!(
            "{},{},{},{},{},{}",
            self.map_id, self.cell_x, self.cell_y, self.base_z, walk, obs
        )
    }
}

/// The terrain half of a cell: slope and MCNK holes, the only blockers the height grid alone
/// knows. None when the terrain is clear everywhere in the cell.
fn terrain_cell(cell: &crate::terrain::CellRow) -> Option<NavCellData> {
    let mut walk = vec![0xFFu8; WALK_BYTES];
    let mut dirty = false;

    // Slope: per 4.17 yd sub-quad from the 9×9 outer corners (index i*17+j).
    for qi in 0..8 {
        for qj in 0..8 {
            let corner = |i: usize, j: usize| cell.heights[i * 17 + j];
            let (a, b, c, d) = (
                corner(qi, qj),
                corner(qi + 1, qj),
                corner(qi, qj + 1),
                corner(qi + 1, qj + 1),
            );
            let rise = a.max(b).max(c).max(d) - a.min(b).min(c).min(d);
            if rise > MAX_QUAD_RISE {
                dirty = true;
                for ny in qj * 8..(qj + 1) * 8 {
                    for nx in qi * 8..(qi + 1) * 8 {
                        walk_set(&mut walk, nx, ny, false);
                    }
                }
            }
        }
    }

    // MCNK holes: low 16 bits, 4×4 quadrants, bit k = quadrant (k%4 along x, k/4 along y).
    for k in 0..16u32 {
        if cell.holes & (1 << k) != 0 {
            dirty = true;
            let (hx, hy) = ((k % 4) as usize, (k / 4) as usize);
            for ny in hy * 16..(hy + 1) * 16 {
                for nx in hx * 16..(hx + 1) * 16 {
                    walk_set(&mut walk, nx, ny, false);
                }
            }
        }
    }

    dirty.then(|| NavCellData {
        base_z: cell.heights.iter().copied().fold(f32::MAX, f32::min),
        walk,
        obs: vec![OBS_NONE; OBS_BYTES],
    })
}

/// Rasterize one terrain cell against the triangles binned to it: the terrain blockers merged
/// with the shared collision-geometry derivation (`nav::derive_cell` — the module derives its
/// coverage through the same function, so the two can never drift). Returns None when the cell
/// is fully clear (all-walkable, no obstruction) — those emit NO row by design.
fn rasterize_cell(cell: &crate::terrain::CellRow, tris: &[VmapTri]) -> Option<NavRow> {
    let (cx, cy) = (cell.cell_x, cell.cell_y);
    let data = match (
        terrain_cell(cell),
        derive_cell(cx, cy, Some(&cell.heights), tris),
    ) {
        (Some(terrain), Some(coverage)) => merge_cells(&terrain, &coverage),
        (Some(terrain), None) => terrain,
        (None, Some(coverage)) => coverage,
        (None, None) => return None,
    };
    Some(NavRow {
        map_id: cell.map_id,
        cell_x: cx,
        cell_y: cy,
        base_z: data.base_z,
        walk: data.walk,
        obs: data.obs,
    })
}

// ---------------------------------------------------------------------------------------------
// Shared passes — reused verbatim by `vmap.rs` (#520): tile scan (heights + deduped placements),
// mesh loading, and rotation calibration are identical between the two importer modes; only the
// consumption (rasterize vs. transform-and-pack) differs.
// ---------------------------------------------------------------------------------------------

pub(crate) struct TileScan {
    pub(crate) cells: Vec<crate::terrain::CellRow>,
    pub(crate) placements: Vec<Placement>,
    pub(crate) sources: Vec<TileSource>,
    pub(crate) tiles_read: u32,
}

fn retain_placement(
    placements: &mut Vec<Placement>,
    seen: &mut HashMap<(bool, u32), usize>,
    candidate: Placement,
    source_path: &str,
) -> Result<()> {
    let key = (candidate.is_wmo, candidate.unique_id);
    if let Some(&index) = seen.get(&key) {
        if !placements_match(&placements[index], &candidate) {
            bail!(
                "{source_path} repeats placement {} with conflicting archive data",
                candidate.unique_id
            );
        }
        return Ok(());
    }
    seen.insert(key, placements.len());
    placements.push(candidate);
    Ok(())
}

fn placements_match(first: &Placement, second: &Placement) -> bool {
    first.name == second.name
        && first.is_wmo == second.is_wmo
        && first.unique_id == second.unique_id
        && first.position.map(f32::to_bits) == second.position.map(f32::to_bits)
        && first.rotation.map(f32::to_bits) == second.rotation.map(f32::to_bits)
        && first.scale.to_bits() == second.scale.to_bits()
        && first.bounds_min.map(|values| values.map(f32::to_bits))
            == second.bounds_min.map(|values| values.map(f32::to_bits))
        && first.bounds_max.map(|values| values.map(f32::to_bits))
            == second.bounds_max.map(|values| values.map(f32::to_bits))
        && first.wmo == second.wmo
}

/// Pass 1: parse every ADT tile in the cell-index box — heights (via the shared `collect_cells`)
/// plus WMO/M2 placements deduped by `unique_id` (placements repeat on adjacent tiles).
pub(crate) fn scan_tiles(
    chain: &mut PatchChain,
    map_name: &str,
    map_id: u32,
    cell_range: (i32, i32, i32, i32),
) -> Result<TileScan> {
    let (cell_x_min, cell_x_max, cell_y_min, cell_y_max) = cell_range;
    let (tx_min, tx_max) = (cell_x_min / 16, cell_x_max / 16);
    let (ty_min, ty_max) = (cell_y_min / 16, cell_y_max / 16);

    let mut cells: Vec<crate::terrain::CellRow> = Vec::new();
    let mut placements: Vec<Placement> = Vec::new();
    let mut sources = Vec::new();
    let mut seen_ids = HashMap::new();
    let mut tiles_read = 0u32;
    for tx in tx_min..=tx_max {
        for ty in ty_min..=ty_max {
            let candidates = [
                format!("World\\Maps\\{map_name}\\{map_name}_{ty}_{tx}.adt"),
                format!("World\\Maps\\{map_name}\\{map_name}_{tx}_{ty}.adt"),
            ];
            let mut accepted = None;
            for name in &candidates {
                let Ok(bytes) = chain.read_file(name) else {
                    continue;
                };
                let wow_adt::ParsedAdt::Root(root) = wow_adt::parse_adt(&mut Cursor::new(&bytes))
                    .with_context(|| format!("parsing {name}"))?
                else {
                    bail!("{name} is not a root terrain ADT");
                };
                if crate::terrain::tile_matches(&root.mcnk_chunks, tx, ty) {
                    accepted = Some((name.clone(), bytes, root));
                    break;
                }
            }
            let Some((source_path, source_bytes, root)) = accepted else {
                continue;
            }; // ocean/empty tiles simply don't exist
            tiles_read += 1;
            sources.push(TileSource {
                path: source_path.clone(),
                bytes: source_bytes.len(),
                blake3: blake3::hash(&source_bytes).to_hex().to_string(),
            });
            crate::terrain::collect_cells(
                &root.mcnk_chunks,
                map_id,
                (cell_x_min, cell_x_max, cell_y_min, cell_y_max),
                &mut cells,
            )?;
            for p in &root.wmo_placements {
                retain_placement(
                    &mut placements,
                    &mut seen_ids,
                    Placement {
                        name: resolve(&root.wmos, &root.wmo_indices, p.name_id)?.to_string(),
                        is_wmo: true,
                        unique_id: p.unique_id,
                        position: p.position,
                        rotation: p.rotation,
                        scale: 1.0,
                        bounds_min: Some(p.extents_min),
                        bounds_max: Some(p.extents_max),
                        wmo: Some(WmoPlacement {
                            flags: p.flags,
                            doodad_set: p.doodad_set,
                            name_set: p.name_set,
                        }),
                    },
                    &source_path,
                )?;
            }
            for p in &root.doodad_placements {
                retain_placement(
                    &mut placements,
                    &mut seen_ids,
                    Placement {
                        name: resolve(&root.models, &root.model_indices, p.name_id)?.to_string(),
                        is_wmo: false,
                        unique_id: p.unique_id,
                        position: p.position,
                        rotation: p.rotation,
                        scale: p.scale as f32 / 1024.0,
                        bounds_min: None,
                        bounds_max: None,
                        wmo: None,
                    },
                    &source_path,
                )?;
            }
        }
    }
    Ok(TileScan {
        cells,
        placements,
        sources,
        tiles_read,
    })
}

/// Pass 2: load each referenced model's collision mesh once (WMO keeps per-triangle group
/// metadata; M2 doesn't have groups).
pub(crate) fn load_meshes(
    chain: &mut PatchChain,
    placements: &[Placement],
) -> Result<HashMap<String, Mesh>> {
    let mut meshes: HashMap<String, Mesh> = HashMap::new();
    for p in placements {
        if !meshes.contains_key(&p.name) {
            let mesh = if p.is_wmo {
                Mesh::Wmo(wmo_tris(chain, &p.name)?)
            } else {
                Mesh::M2(m2_tris(chain, &p.name)?)
            };
            meshes.insert(p.name.clone(), mesh);
        }
    }
    Ok(meshes)
}

/// Load one collision mesh for the streaming vmap importer. Keeping this small interface avoids
/// retaining every unique model of a production-sized selection at once.
pub(crate) fn load_mesh(chain: &mut PatchChain, placement: &Placement) -> Result<Mesh> {
    if placement.is_wmo {
        Ok(Mesh::Wmo(wmo_tris(chain, &placement.name)?))
    } else {
        Ok(Mesh::M2(m2_tris(chain, &placement.name)?))
    }
}

/// Pass 3: calibrate the rotation convention against MODF bounds (WMOs only), capped sample.
pub(crate) fn calibrate_from_placements(
    chain: &mut PatchChain,
    placements: &[Placement],
) -> Result<Convention> {
    Ok(calibrate_from_placements_with_evidence(chain, placements)?.convention)
}

pub(crate) fn calibrate_from_placements_with_evidence(
    chain: &mut PatchChain,
    placements: &[Placement],
) -> Result<Calibration> {
    let mut samples: Vec<(&Placement, Vec<[f32; 3]>)> = Vec::new();
    let mut sampled_names: HashSet<&str> = HashSet::new();
    for p in placements
        .iter()
        .filter(|p| p.is_wmo)
        .filter(|p| sampled_names.insert(p.name.as_str()))
        .take(32)
    {
        samples.push((p, wmo_calibration_points(chain, &p.name)?));
    }
    calibration(&samples)
}

// ---------------------------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------------------------

pub(crate) fn run(args: &crate::Args) -> Result<()> {
    let scope = args.world_import_scope()?;
    if scope.bounded_slices.is_empty() {
        println!(
            "nav: scope {} has no bounded map slices; no navigation chunks to import",
            scope.name()
        );
        return Ok(());
    }
    let data_dir = Path::new(args.nav.as_ref().expect("caller checked"));
    let mut chain = crate::collision::open_geometry_chain(data_dir)?;
    let mut slice_rows = Vec::with_capacity(scope.bounded_slices.len());
    for slice in &scope.bounded_slices {
        slice_rows.push(rasterize_slice(&mut chain, slice).with_context(|| {
            format!(
                "nav scope {} slice {} (map {})",
                scope.name(),
                slice.name,
                slice.map_id
            )
        })?);
    }
    let rows = dedup_rows(slice_rows);
    let batches = batches(&rows);
    println!(
        "nav: {} unique rows in scope {} → {} reducer batch(es)",
        rows.len(),
        scope.name(),
        batches.len()
    );
    if !args.apply {
        println!(
            "-- DRY RUN: would call import_nav_chunks (batch 0, clears) + {} × import_nav_chunks_append",
            batches.len().saturating_sub(1)
        );
        return Ok(());
    }
    for (i, batch) in batches.iter().enumerate() {
        crate::call_reducer(
            args,
            if i == 0 {
                "import_nav_chunks"
            } else {
                "import_nav_chunks_append"
            },
            batch,
        )?;
    }
    println!("nav: applied.");
    Ok(())
}

fn rasterize_slice(
    chain: &mut PatchChain,
    slice: &crate::world_import_scope::BoundedMapSlice,
) -> Result<Vec<NavRow>> {
    let map_id = u32::try_from(slice.map_id).context("bounded slice map id is outside u32")?;
    let map_name = crate::terrain::map_dir(map_id)?;
    let (cell_x_min, cell_x_max, cell_y_min, cell_y_max) =
        crate::terrain::slice_cell_range(Some(slice.bounds), slice.sample, 0.0);
    for c in [cell_x_min, cell_x_max, cell_y_min, cell_y_max] {
        if !(0..1024).contains(&c) {
            bail!("slice cell index {c} outside the map square");
        }
    }

    // Pass 1: parse tiles — heights + deduped placements.
    let scan = scan_tiles(
        chain,
        map_name,
        map_id,
        (cell_x_min, cell_x_max, cell_y_min, cell_y_max),
    )?;
    let (cells, placements) = (scan.cells, scan.placements);
    if cells.is_empty() {
        bail!("no MCNK cells intersected the slice");
    }
    println!(
        "nav: {} tile(s), {} cells, {} unique placements ({} WMO)",
        scan.tiles_read,
        cells.len(),
        placements.len(),
        placements.iter().filter(|p| p.is_wmo).count()
    );

    // Pass 2: load each referenced model's collision mesh once.
    let meshes = load_meshes(chain, &placements)?;
    let mesh_tris: usize = meshes.values().map(Mesh::len).sum();
    println!(
        "nav: {} unique models, {mesh_tris} local tris",
        meshes.len()
    );

    // Pass 3: calibrate the rotation convention against MODF bounds (WMOs only), capped sample.
    let conv = calibrate_from_placements(chain, &placements)?;

    // Pass 4: transform to world space + bin by terrain cell.
    let mut world_tris: Vec<VmapTri> = Vec::new();
    for p in &placements {
        let pos_w = place_pos(p.position);
        let transform = |t: &Tri| -> Tri {
            [
                apply(conv, p.rotation, p.scale, pos_w, t[0]),
                apply(conv, p.rotation, p.scale, pos_w, t[1]),
                apply(conv, p.rotation, p.scale, pos_w, t[2]),
            ]
        };
        match &meshes[&p.name] {
            Mesh::Wmo(rows) => world_tris.extend(rows.iter().map(|w| VmapTri {
                verts: transform(&w.tri),
                class: TriClass::Wmo {
                    group_id: w.group_id,
                    mogp_flags: w.mogp_flags,
                },
            })),
            Mesh::M2(rows) => world_tris.extend(rows.iter().map(|t| VmapTri {
                verts: transform(t),
                class: TriClass::M2,
            })),
        }
    }
    let mut by_cell: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, t) in world_tris.iter().enumerate() {
        // High world coord → LOW cell index; iterate the covered index rectangle. Inflate by
        // `WALK_MARGIN` so a triangle hugging a cell border reaches the neighbor cell its inset
        // walk footprint spills into.
        let (lo, hi) = aabb(t.verts.iter().copied());
        let (Some(cx0), Some(cx1), Some(cy0), Some(cy1)) = (
            cell_index(hi[0] + WALK_MARGIN),
            cell_index(lo[0] - WALK_MARGIN),
            cell_index(hi[1] + WALK_MARGIN),
            cell_index(lo[1] - WALK_MARGIN),
        ) else {
            continue; // off the map square (shouldn't happen inside the box)
        };
        for cx in cx0..=cx1 {
            for cy in cy0..=cy1 {
                by_cell.entry(cell_key(map_id, cx, cy)).or_default().push(i);
            }
        }
    }

    // Pass 5: rasterize.
    let mut rows: Vec<NavRow> = Vec::new();
    for cell in &cells {
        let empty = Vec::new();
        let tri_idx = by_cell
            .get(&cell_key(map_id, cell.cell_x, cell.cell_y))
            .unwrap_or(&empty);
        let tris: Vec<VmapTri> = tri_idx.iter().map(|&i| world_tris[i]).collect();
        if let Some(row) = rasterize_cell(cell, &tris) {
            rows.push(row);
        }
    }
    println!(
        "nav: {} cells rasterized → {} rows with content ({} skipped all-clear)",
        cells.len(),
        rows.len(),
        cells.len() - rows.len()
    );

    // NAV_ASCII="x0,x1,y0,y1": print an ASCII walkability map of that world rectangle from the
    // in-memory rows (dry-run diagnostic; '#' blocked, '.' walkable, ' ' no row/all-clear).
    if let Ok(spec) = std::env::var("NAV_ASCII") {
        let v: Vec<f32> = spec
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        if v.len() == 4 {
            let (x0, x1, y0, y1) = (
                v[0].min(v[1]),
                v[0].max(v[1]),
                v[2].min(v[3]),
                v[2].max(v[3]),
            );
            let step = 1.0f32;
            let mut y = y1;
            while y >= y0 {
                let mut line = String::new();
                let mut x = x1; // world x decreases left→right? print x1 (high) at LEFT for map-like reading
                while x >= x0 {
                    let ch = match (cell_index(x), cell_index(y)) {
                        (Some(cx), Some(cy)) => {
                            match rows.iter().find(|r| r.cell_x == cx && r.cell_y == cy) {
                                None => ' ',
                                Some(r) => {
                                    match (sub_index(x, cx, WALK_DIM), sub_index(y, cy, WALK_DIM)) {
                                        (Some(nx), Some(ny)) => {
                                            if lyracore_shared::nav::walk_get(&r.walk, nx, ny) {
                                                '.'
                                            } else {
                                                '#'
                                            }
                                        }
                                        _ => '?',
                                    }
                                }
                            }
                        }
                        _ => '?',
                    };
                    line.push(ch);
                    x -= step;
                }
                println!("{y:7.1} {line}");
                y -= step;
            }
        }
    }

    // Self-check: --center must be walkable (the operator points it at a road/spawn, same
    // contract as the terrain interpolate check). Fails loudly on axis/transform breakage.
    {
        let (ccx, ccy) = (slice.sample.0 as f32, slice.sample.1 as f32);
        let (cx, cy) = (
            cell_index(ccx).context("--center off map")?,
            cell_index(ccy).context("--center off map")?,
        );
        let walkable = match rows.iter().find(|r| r.cell_x == cx && r.cell_y == cy) {
            None => true, // no row = all clear
            Some(r) => {
                let (nx, ny) = (
                    sub_index(ccx, cx, WALK_DIM).context("--center sub-index")?,
                    sub_index(ccy, cy, WALK_DIM).context("--center sub-index")?,
                );
                lyracore_shared::nav::walk_get(&r.walk, nx, ny)
            }
        };
        println!(
            "nav: slice {} map {map_id} self-check — sample walkable = {walkable}",
            slice.name
        );
        if !walkable {
            bail!("nav self-check failed: sample rasterized UNwalkable");
        }
    }

    Ok(rows)
}

fn dedup_rows(slice_rows: Vec<Vec<NavRow>>) -> Vec<NavRow> {
    let mut rows = BTreeMap::new();
    for row in slice_rows.into_iter().flatten() {
        rows.entry(cell_key(row.map_id, row.cell_x, row.cell_y))
            .or_insert(row);
    }
    rows.into_values().collect()
}

fn batches(rows: &[NavRow]) -> Vec<String> {
    let mut batches: Vec<String> = Vec::new();
    let mut cur = String::new();
    for r in rows {
        let p = r.packed();
        if !cur.is_empty() && cur.len() + p.len() + 1 > BATCH_BYTES {
            batches.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(';');
        }
        cur.push_str(&p);
    }
    if !cur.is_empty() {
        batches.push(cur);
    }
    batches
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::nav::{obs_top, sub_center, walk_get};
    use lyracore_shared::terrain::cell_index;

    fn wmo_tri(v: Tri) -> VmapTri {
        VmapTri {
            verts: v,
            class: TriClass::Wmo {
                group_id: 0,
                mogp_flags: 0,
            },
        }
    }

    /// A flat 80.0-height cell at the same Northshire-ish coords the shared runtime tests use.
    fn flat_cell() -> crate::terrain::CellRow {
        crate::terrain::CellRow {
            map_id: 0,
            cell_x: cell_index(-8913.0).unwrap(),
            cell_y: cell_index(-184.0).unwrap(),
            liquid_level: 0.0,
            has_liquid: false,
            holes: 0,
            area_id: 0,
            heights: vec![80.0; 145],
        }
    }

    fn test_placement(is_wmo: bool) -> Placement {
        Placement {
            name: if is_wmo { "route.wmo" } else { "route.m2" }.to_owned(),
            is_wmo,
            unique_id: 77,
            position: [1.0, 2.0, 3.0],
            rotation: [4.0, 5.0, 6.0],
            scale: 1.0,
            bounds_min: is_wmo.then_some([0.0, 1.0, 2.0]),
            bounds_max: is_wmo.then_some([3.0, 4.0, 5.0]),
            wmo: is_wmo.then_some(WmoPlacement {
                flags: 0,
                doodad_set: 1,
                name_set: 2,
            }),
        }
    }

    #[test]
    fn calibration_uses_authored_bounds_when_movt_does_not_fill_their_corners() {
        // Map 36's retained Deadmines WMO placement, MOHD, and MODF values. MODF matches the
        // transformed authored box while the actual transformed MOVT cloud is 2.36 yd tighter.
        let expected = Convention {
            shuffle: false,
            sign: 1.0,
            offset_deg: 180.0,
        };
        let authored = mohd_calibration_points(
            [-242.05612, -172.29312, -52.13956],
            [311.44223, 162.8655, 57.554893],
        )
        .expect("retained MOHD bounds");
        let mut placement = test_placement(true);
        placement.position = [17718.262, 29.849575, 17223.072];
        placement.rotation = [0.0, -91.0, 0.0];
        placement.bounds_min = Some([17404.025, -22.289986, 17045.37]);
        placement.bounds_max = Some([17963.29, 87.404465, 17390.137]);

        let modf_world_lo: [f32; 3] = [-323.4707, -896.62305, -22.289986];
        let modf_world_hi: [f32; 3] = [21.296875, -337.35938, 87.404465];
        let movt_world_lo: [f32; 3] = [-317.0512, -895.1547, -22.262207];
        let movt_world_hi: [f32; 3] = [19.425507, -341.70523, 87.37669];
        let movt_error = (0..3)
            .map(|axis| {
                (movt_world_lo[axis] - modf_world_lo[axis]).abs()
                    + (movt_world_hi[axis] - modf_world_hi[axis]).abs()
            })
            .sum::<f32>()
            / 6.0;
        assert!((movt_error - 2.360_098_6).abs() < 0.000_1);

        let selected = calibration(&[(&placement, authored)]).expect("authored bounds fit");
        assert_eq!(selected.convention, expected);
        assert!(selected.q25_error_yards < 0.002);
    }

    #[test]
    fn calibration_refuses_malformed_mohd_bounds() {
        assert!(mohd_calibration_points([f32::NAN, 0.0, 0.0], [1.0; 3]).is_err());
        assert!(mohd_calibration_points([2.0, 0.0, 0.0], [1.0; 3]).is_err());
    }

    #[test]
    fn repeated_placements_must_match_before_deduplication() {
        let first = test_placement(true);
        let mut placements = Vec::new();
        let mut seen = HashMap::new();
        retain_placement(&mut placements, &mut seen, first.clone(), "first.adt").unwrap();
        retain_placement(&mut placements, &mut seen, first, "second.adt").unwrap();
        assert_eq!(
            placements.len(),
            1,
            "an identical repeat stays deduplicated"
        );

        let mut conflicting_wmo = test_placement(true);
        conflicting_wmo.wmo.as_mut().unwrap().doodad_set = 3;
        let error = retain_placement(&mut placements, &mut seen, conflicting_wmo, "second.adt")
            .unwrap_err();
        assert!(error.to_string().contains("conflicting archive data"));
        assert_eq!(placements[0].wmo.unwrap().doodad_set, 1);

        let first = test_placement(false);
        let mut placements = Vec::new();
        let mut seen = HashMap::new();
        retain_placement(&mut placements, &mut seen, first, "first.adt").unwrap();
        let mut conflicting_m2 = test_placement(false);
        conflicting_m2.scale = 2.0;
        assert!(
            retain_placement(&mut placements, &mut seen, conflicting_m2, "second.adt").is_err()
        );
    }

    #[test]
    fn dedup_rows_keeps_the_union_across_maps_and_overlaps() {
        let row = |map_id, cell_x, cell_y| NavRow {
            map_id,
            cell_x,
            cell_y,
            base_z: 0.0,
            walk: vec![0xFF; WALK_BYTES],
            obs: vec![OBS_NONE; OBS_BYTES],
        };

        let rows = dedup_rows(vec![
            vec![row(0, 10, 20), row(0, 11, 20)],
            vec![row(0, 11, 20), row(0, 12, 20)],
            vec![row(1, 10, 20)],
        ]);

        assert_eq!(rows.len(), 4);
        assert!(rows
            .iter()
            .any(|row| (row.map_id, row.cell_x, row.cell_y) == (0, 10, 20)));
        assert!(rows
            .iter()
            .any(|row| (row.map_id, row.cell_x, row.cell_y) == (0, 11, 20)));
        assert!(rows
            .iter()
            .any(|row| (row.map_id, row.cell_x, row.cell_y) == (0, 12, 20)));
        assert!(rows
            .iter()
            .any(|row| (row.map_id, row.cell_x, row.cell_y) == (1, 10, 20)));
    }

    #[test]
    fn walk_grid_insets_by_the_agent_radius_but_obs_does_not() {
        // A thin vertical WMO wall down the x = walk-nx-32 line must
        // block one extra sub-cell on EACH side (the body inset — the corner-clip fix), while
        // the obs/LoS grid keeps the true footprint (sight is a line, not a body).
        let cell = flat_cell();
        let (cx, cy) = (cell.cell_x, cell.cell_y);
        let wall_x = sub_center(cx, 32, WALK_DIM);
        let (y0, y1) = (sub_center(cy, 60, WALK_DIM), sub_center(cy, 3, WALK_DIM));
        let wall = wmo_tri([[wall_x, y0, 79.0], [wall_x, y1, 79.0], [wall_x, y0, 92.0]]);
        let row = rasterize_cell(&cell, &[wall]).expect("a wall dirties the cell");
        // On the wall line: blocked.
        assert!(!walk_get(&row.walk, 32, 32));
        // One sub-cell out (0.52 yd — inside `WALK_MARGIN`): blocked ONLY by the agent-radius
        // inset; the bare 0.35 yd `RASTER_MARGIN` left it walkable and the body clipped corners.
        assert!(!walk_get(&row.walk, 31, 32));
        assert!(!walk_get(&row.walk, 33, 32));
        // Two sub-cells out (1.04 yd — outside the margin): walkable, the inset is bounded.
        assert!(walk_get(&row.walk, 30, 32));
        assert!(walk_get(&row.walk, 34, 32));
        // Obs: only the wall's own column (ox=16) rises — no body inset on the sight grid.
        assert!(obs_top(&row.obs, row.base_z, 16, 16).is_some());
        assert!(obs_top(&row.obs, row.base_z, 15, 16).is_none());
        assert!(obs_top(&row.obs, row.base_z, 17, 16).is_none());
    }

    #[test]
    fn a_row_carries_both_the_terrain_blockers_and_the_geometry_ones() {
        // Holes are the importer's own blocker; the wall comes from the shared derivation. One
        // row must carry both, and a hole alone must still emit a row.
        let mut cell = flat_cell();
        cell.holes = 1; // quadrant (0, 0) → walk sub-cells 0..16 on both axes
        assert!(
            rasterize_cell(&cell, &[]).is_some(),
            "a hole dirties the cell"
        );

        let (cx, cy) = (cell.cell_x, cell.cell_y);
        let wall_x = sub_center(cx, 40, WALK_DIM);
        let (y0, y1) = (sub_center(cy, 60, WALK_DIM), sub_center(cy, 30, WALK_DIM));
        let wall = wmo_tri([[wall_x, y0, 79.0], [wall_x, y1, 79.0], [wall_x, y0, 92.0]]);
        let row = rasterize_cell(&cell, &[wall]).expect("hole + wall");
        assert!(!walk_get(&row.walk, 8, 8), "the hole survives the merge");
        assert!(!walk_get(&row.walk, 40, 40), "the wall survives the merge");
        assert!(walk_get(&row.walk, 20, 40), "clear ground stays walkable");
        assert!(obs_top(&row.obs, row.base_z, 20, 20).is_some());
    }
}
