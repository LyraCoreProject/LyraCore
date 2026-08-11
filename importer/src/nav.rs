//! `--nav <client Data/ dir>` — work-item 241: rasterize ADT terrain + WMO/M2 collision
//! geometry into per-cell nav blobs → `game_nav_chunk` (decision #8: grid tables, no
//! vmap/Detour runtime). Same licensing firewall as `--terrain`. Honors `--map` + `--box`.
//!
//! Pipeline: parse every ADT tile in the box (heights via terrain.rs's `collect_cells` — the
//! SAME code the heightmap import runs), collect WMO/M2 placements deduped by `unique_id`
//! (placements repeat on adjacent tiles), load each referenced model's collision triangles
//! once, transform to world space, bin by terrain cell, then rasterize each cell's 64×64
//! walkability bitmask + 32×32 obstruction-height grid through `lyracore_shared::nav` (the same
//! codec the module reads with).
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
    obs_raise, sub_center, sub_index, walk_set, OBS_BYTES, OBS_DIM, OBS_NONE, WALK_BYTES, WALK_DIM,
    WALK_HEIGHT, WALK_STEP_UP,
};
use lyracore_shared::terrain::{cell_index, cell_key, interpolate, CELL_SIZE};
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
            Mesh::Wmo(v) => v.len(),
            Mesh::M2(v) => v.len(),
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

/// M2 bounding (collision) mesh, model-local coords. Vanilla names end `.mdx` → archive `.m2`.
/// Parse failures return an EMPTY mesh with a warning (240 finding: 6/75 Northshire M2s —
/// all decorative particle props — fail upstream parsing; skipping loses nothing collidable).
fn m2_tris(chain: &mut PatchChain, name: &str) -> Vec<Tri> {
    let m2_name = name
        .rsplit_once('.')
        .map(|(stem, _)| format!("{stem}.m2"))
        .unwrap_or_else(|| name.to_string());
    let Ok(bytes) = chain.read_file(&m2_name) else {
        eprintln!("nav: WARN M2 unreadable, skipping: {m2_name}");
        return Vec::new();
    };
    let model = match wow_m2::parse_m2(&mut Cursor::new(&bytes)) {
        Ok(wow_m2::M2Format::Legacy(m) | wow_m2::M2Format::Chunked(m)) => m,
        Err(e) => {
            eprintln!("nav: WARN M2 parse failed, skipping: {m2_name} ({e})");
            return Vec::new();
        }
    };
    let vb = &model.raw_data.bounding_vertices; // 12 bytes per vec3<f32>
    let ib = &model.raw_data.bounding_triangles; // 2 bytes per u16 index
    let verts: Vec<[f32; 3]> = vb
        .chunks_exact(12)
        .map(|c| {
            [
                f32::from_le_bytes([c[0], c[1], c[2], c[3]]),
                f32::from_le_bytes([c[4], c[5], c[6], c[7]]),
                f32::from_le_bytes([c[8], c[9], c[10], c[11]]),
            ]
        })
        .collect();
    let idx: Vec<u16> = ib
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    idx.chunks_exact(3)
        .filter_map(|t| {
            let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
            Some([*verts.get(a)?, *verts.get(b)?, *verts.get(c)?])
        })
        .collect()
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

/// Every MOVT vertex of a WMO (all groups, NO collidability filter) — calibration compares
/// against MODF bounds, which cover ALL geometry; the collidable subset can be much smaller
/// (Stormwind is 2/3 detail-flagged) and its AABB never matches.
pub(crate) fn wmo_all_verts(chain: &mut PatchChain, name: &str) -> Result<Vec<[f32; 3]>> {
    let root_bytes = chain
        .read_file(name)
        .with_context(|| format!("reading WMO root {name}"))?;
    let wow_wmo::ParsedWmo::Root(root) = wow_wmo::parse_wmo(&mut Cursor::new(&root_bytes))
        .with_context(|| format!("parsing WMO root {name}"))?
    else {
        bail!("{name} parsed as a group file, expected root");
    };
    let stem = name.strip_suffix(".wmo").unwrap_or(name);
    let mut verts = Vec::new();
    for g in 0..root.n_groups {
        let gname = format!("{stem}_{g:03}.wmo");
        let bytes = chain
            .read_file(&gname)
            .with_context(|| format!("reading {gname}"))?;
        if let wow_wmo::ParsedWmo::Group(group) = wow_wmo::parse_wmo(&mut Cursor::new(&bytes))
            .with_context(|| format!("parsing {gname}"))?
        {
            verts.extend(group.vertex_positions.iter().map(|v| [v.x, v.y, v.z]));
        }
    }
    Ok(verts)
}

/// Pick the convention that reproduces the MODF world AABBs across the calibration placements
/// (mean per-axis corner error, capped sample). Hard-fails above 3 yd — a wrong convention
/// must never silently rasterize rotated buildings.
pub(crate) fn calibrate(samples: &[(&Placement, Vec<[f32; 3]>)]) -> Result<Convention> {
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
        // Per-sample mean corner error; score by MEDIAN across samples — a handful of
        // placements ship PADDED authored bounds (stormwind.wmo's extents span 143 yd more
        // height than its geometry) and would poison a mean.
        let mut errs: Vec<f32> = Vec::new();
        for (p, verts) in samples {
            let (Some(bmin), Some(bmax)) = (p.bounds_min, p.bounds_max) else {
                continue;
            };
            // Bounds are placement-space: convert both corners, re-sort per axis.
            let (c1, c2) = (place_pos(bmin), place_pos(bmax));
            let want_lo = [c1[0].min(c2[0]), c1[1].min(c2[1]), c1[2].min(c2[2])];
            let want_hi = [c1[0].max(c2[0]), c1[1].max(c2[1]), c1[2].max(c2[2])];
            let pos_w = place_pos(p.position);
            let (got_lo, got_hi) = aabb(
                verts
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
        // Score by the 25th percentile: tightly-authored bounds fit the CORRECT convention
        // near-exactly (0.04-0.2 yd on wallpiece/widebridge/animalden), while padded bounds
        // (stormwind: +143 yd of authored height) are noise under EVERY convention. A wrong
        // convention has no near-zero fits at all, so the low quantile separates cleanly
        // where mean and even median drift with the padding mix of the sampled box.
        errs.sort_by(f32::total_cmp);
        let mean = errs[errs.len() / 4];
        if std::env::var("NAV_DEBUG").is_ok() {
            eprintln!("NAV_DEBUG {conv:?} mean={mean:.2}");
            for (p, verts) in samples {
                let (Some(bmin), Some(bmax)) = (p.bounds_min, p.bounds_max) else {
                    continue;
                };
                let (c1, c2) = (place_pos(bmin), place_pos(bmax));
                let pos_w = place_pos(p.position);
                let (got_lo, got_hi) = aabb(
                    verts
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
    Ok(conv)
}

// ---------------------------------------------------------------------------------------------
// Tile collection
// ---------------------------------------------------------------------------------------------

pub(crate) struct Placement {
    pub(crate) name: String,
    pub(crate) is_wmo: bool,
    pub(crate) position: [f32; 3],
    pub(crate) rotation: [f32; 3],
    pub(crate) scale: f32,
    pub(crate) bounds_min: Option<[f32; 3]>,
    pub(crate) bounds_max: Option<[f32; 3]>,
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

/// Inflation margin for the point-in-triangle test (yd) — slightly over half a walk-cell
/// diagonal so a thin vertical wall whose 2D projection is a sliver still hits the centers of
/// the cells its line crosses. Conservative direction: over-blocking by <1 cell.
const RASTER_MARGIN: f32 = 0.35;

/// #102 (a): agent body radius (yd) the WALK grid is inset by at build time. `find_leg` plans a
/// CENTER line and `nav_step` walks it, so a walkable cell must keep a body's half-width clear
/// of geometry or the body clips wall corners the center-line misses. ~One walk sub-cell
/// (0.52 yd) ≈ the vanilla humanoid bounding radius (~0.39 yd) rounded up to the grid quantum —
/// conservative direction, same as `RASTER_MARGIN`. Walk-grid ONLY: the obs/LoS grid stays at
/// the true footprint (sight is a line, not a body).
const AGENT_RADIUS: f32 = 0.5;

/// The walk-grid rasterization margin: sliver-catching plus the #102 body inset. The candidate
/// sub-cell window inflates by the same amount so every center inside the margin gets tested.
const WALK_MARGIN: f32 = RASTER_MARGIN + AGENT_RADIUS;

/// One world-space collision triangle with its AABB, plane, and origin kind. `z_at` is the
/// exact-rasterization core: the triangle's z-interval over a 2D point (None = point outside
/// the inflated footprint). A coarse M2 bounding triangle spanning trunk→canopy thus blocks
/// only near the trunk (low plane-z) and never under the canopy — the AABB stamping this
/// replaces marked the whole canopy width unwalkable and turned Elwynn's forests into a maze.
struct WorldTri {
    lo: [f32; 3],
    hi: [f32; 3],
    is_wmo: bool,
    /// Vertices (world).
    v: Tri,
    /// Plane normal + d (n·p = d); near-vertical planes fall back to the full z-range.
    n: [f32; 3],
    d: f32,
}

impl WorldTri {
    fn new(v: Tri, is_wmo: bool) -> Self {
        let (lo, hi) = aabb(v.iter().copied());
        let e1 = [v[1][0] - v[0][0], v[1][1] - v[0][1], v[1][2] - v[0][2]];
        let e2 = [v[2][0] - v[0][0], v[2][1] - v[0][1], v[2][2] - v[0][2]];
        let n = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let d = n[0] * v[0][0] + n[1] * v[0][1] + n[2] * v[0][2];
        Self {
            lo,
            hi,
            is_wmo,
            v,
            n,
            d,
        }
    }

    /// The triangle's z-interval over 2D point (x, y), testing inside-ness with an `margin`-yd
    /// outward inflation (signed edge distance). None = outside the inflated footprint.
    fn z_at(&self, x: f32, y: f32, margin: f32) -> Option<(f32, f32)> {
        // 2D signed edge distances (positive = same side as the third vertex).
        for i in 0..3 {
            let (a, b, c) = (self.v[i], self.v[(i + 1) % 3], self.v[(i + 2) % 3]);
            let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
            let len = (ex * ex + ey * ey).sqrt();
            if len < 1e-6 {
                continue; // degenerate edge in 2D (vertical tri seen edge-on) — skip this edge test
            }
            let cross = |px: f32, py: f32| ex * (py - a[1]) - ey * (px - a[0]);
            let side_c = cross(c[0], c[1]);
            let side_p = cross(x, y);
            // Normalize to a distance and require p within `margin` outside c's side.
            let dist = side_p * side_c.signum() / len;
            if dist < -margin {
                return None;
            }
        }
        // Steep plane (wall/fence side, >60° from horizontal): the plane-z is ill-conditioned
        // over the projected sliver (it clamps to an arbitrary end of the z-range — the live
        // "mobs walk through the graveyard fence" find: fence sides tilted ~1° clamped to the
        // BOTTOM, below the standing band). Use the full z-span instead; anything this steep
        // is unwalkable surface anyway (the terrain slope gate is 50°).
        let n_len = (self.n[0] * self.n[0] + self.n[1] * self.n[1] + self.n[2] * self.n[2]).sqrt();
        if self.n[2].abs() < 0.5 * n_len {
            return Some((self.lo[2], self.hi[2]));
        }
        let z =
            ((self.d - self.n[0] * x - self.n[1] * y) / self.n[2]).clamp(self.lo[2], self.hi[2]);
        Some((z, z))
    }
}

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

/// Rasterize one terrain cell against the triangles binned to it. Returns None when the cell
/// is fully clear (all-walkable, no obstruction) — those emit NO row by design.
fn rasterize_cell(cell: &crate::terrain::CellRow, tris: &[&WorldTri]) -> Option<NavRow> {
    let (cx, cy) = (cell.cell_x, cell.cell_y);
    let mut walk = vec![0xFFu8; WALK_BYTES];
    let mut obs = vec![OBS_NONE; OBS_BYTES];
    let mut dirty = false;

    // Ground height per walk sub-cell (bilinear through the SAME shared interpolate the
    // module's ground_z uses).
    let mut ground = vec![0.0f32; WALK_DIM * WALK_DIM];
    for ny in 0..WALK_DIM {
        for nx in 0..WALK_DIM {
            let (x, y) = (sub_center(cx, nx, WALK_DIM), sub_center(cy, ny, WALK_DIM));
            ground[ny * WALK_DIM + nx] =
                interpolate(&cell.heights, cx, cy, x, y).unwrap_or(cell.heights[0]);
        }
    }

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

    // Collision triangles (world-space, exact per-cell rasterization — see WorldTri::z_at).
    let base_z = cell.heights.iter().copied().fold(f32::MAX, f32::min);
    let mut col_ivals: Vec<Vec<(f32, f32)>> = vec![Vec::new(); OBS_BYTES];
    let half_res = |dim: usize| 0.5 * CELL_SIZE / dim as f32;
    // Sub-index range of [lo_c, hi_c] clamped to this cell (high coord → LOW index).
    let clamp_axis = |lo_c: f32, hi_c: f32, cell_i: u16, dim: usize| -> Option<(usize, usize)> {
        let cell_hi = sub_center(cell_i, 0, dim) + half_res(dim);
        let cell_lo = sub_center(cell_i, dim - 1, dim) - half_res(dim);
        let lo_c = lo_c.max(cell_lo + 1e-4);
        let hi_c = hi_c.min(cell_hi - 1e-4);
        if lo_c > hi_c {
            return None;
        }
        Some((sub_index(hi_c, cell_i, dim)?, sub_index(lo_c, cell_i, dim)?))
    };
    for t in tris.iter() {
        // Walkability (all geometry): block a nav cell only when the triangle actually passes
        // through the standing band above THAT cell's ground. Footprint + window inflated by
        // `WALK_MARGIN` (#102 (a)): cells within a body radius of geometry rasterize blocked, so
        // the center line `find_leg`/`nav_step` walk keeps the body clear of wall corners.
        if let (Some((nx0, nx1)), Some((ny0, ny1))) = (
            clamp_axis(t.lo[0] - WALK_MARGIN, t.hi[0] + WALK_MARGIN, cx, WALK_DIM),
            clamp_axis(t.lo[1] - WALK_MARGIN, t.hi[1] + WALK_MARGIN, cy, WALK_DIM),
        ) {
            for ny in ny0..=ny1 {
                for nx in nx0..=nx1 {
                    let (x, y) = (sub_center(cx, nx, WALK_DIM), sub_center(cy, ny, WALK_DIM));
                    let Some((z_lo, z_hi)) = t.z_at(x, y, WALK_MARGIN) else {
                        continue;
                    };
                    let g = ground[ny * WALK_DIM + nx];
                    if z_lo < g + WALK_HEIGHT && z_hi > g + WALK_STEP_UP {
                        walk_set(&mut walk, nx, ny, false);
                        dirty = true;
                    }
                }
            }
        }
        // Obstruction / line of sight: WMO geometry ONLY — vanilla doodads (trees!) block
        // movement but never sight lines (spells/aggro see through a forest). Accumulate the
        // z-INTERVALS per column here; the rooted decision happens per COLUMN after merging
        // (below) — deciding per TRI kept only a wall's bottom band (walls are stacked tri
        // bands) and collapsed obs tops to ~2 yd, which let eye-height rays graze over and
        // thugs aggro through the abbey wall (live regression, 2026-07-10).
        if !t.is_wmo {
            continue;
        }
        if let (Some((ox0, ox1)), Some((oy0, oy1))) = (
            clamp_axis(t.lo[0], t.hi[0], cx, OBS_DIM),
            clamp_axis(t.lo[1], t.hi[1], cy, OBS_DIM),
        ) {
            for oy in oy0..=oy1 {
                for ox in ox0..=ox1 {
                    let (x, y) = (sub_center(cx, ox, OBS_DIM), sub_center(cy, oy, OBS_DIM));
                    let Some((z_lo, z_hi)) = t.z_at(x, y, RASTER_MARGIN) else {
                        continue;
                    };
                    col_ivals[oy * OBS_DIM + ox].push((z_lo, z_hi));
                }
            }
        }
    }

    // Per-column obs: merge the accumulated z-intervals (0.75 yd gap tolerance — wall bands
    // abut but don't overlap exactly), then keep only the merged run that reaches down into
    // the head-height band above THIS column's ground. Stacked wall bands fuse ground→top
    // (full wall height blocks over-the-roof sight); a floating eave/roof run stays a separate
    // high interval and is dropped — so standing under an overhang doesn't blank LoS.
    // Deliberate simplification: single-top obs column, no (bottom, top) pair — archways/bridges
    // read as walls; store both bytes when a live case needs to see THROUGH an opening.
    for oy in 0..OBS_DIM {
        for ox in 0..OBS_DIM {
            let ivals = &mut col_ivals[oy * OBS_DIM + ox];
            if ivals.is_empty() {
                continue;
            }
            let (x, y) = (sub_center(cx, ox, OBS_DIM), sub_center(cy, oy, OBS_DIM));
            let g = interpolate(&cell.heights, cx, cy, x, y).unwrap_or(base_z);
            ivals.sort_by(|a, b| a.0.total_cmp(&b.0));
            let (mut run_lo, mut run_hi) = ivals[0];
            let mut rooted_top: Option<f32> = None;
            let consider = |lo: f32, hi: f32, rooted_top: &mut Option<f32>| {
                if lo <= g + WALK_HEIGHT + 1.0 && hi > g - 1.0 && rooted_top.is_none_or(|t| hi > t)
                {
                    *rooted_top = Some(hi);
                }
            };
            for &(lo, hi) in ivals.iter().skip(1) {
                if lo <= run_hi + 0.75 {
                    run_hi = run_hi.max(hi);
                } else {
                    consider(run_lo, run_hi, &mut rooted_top);
                    (run_lo, run_hi) = (lo, hi);
                }
            }
            consider(run_lo, run_hi, &mut rooted_top);
            if let Some(top) = rooted_top {
                if top > base_z + 0.5 {
                    obs_raise(&mut obs, base_z, ox, oy, top);
                    dirty = true;
                }
            }
        }
    }

    dirty.then_some(NavRow {
        map_id: cell.map_id,
        cell_x: cx,
        cell_y: cy,
        base_z,
        walk,
        obs,
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
    pub(crate) tiles_read: u32,
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
    let mut seen_ids: HashSet<(bool, u32)> = HashSet::new();
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
                    accepted = Some(root);
                    break;
                }
            }
            let Some(root) = accepted else { continue }; // ocean/empty tiles simply don't exist
            tiles_read += 1;
            crate::terrain::collect_cells(
                &root.mcnk_chunks,
                map_id,
                (cell_x_min, cell_x_max, cell_y_min, cell_y_max),
                &mut cells,
            )?;
            for p in &root.wmo_placements {
                if seen_ids.insert((true, p.unique_id)) {
                    placements.push(Placement {
                        name: resolve(&root.wmos, &root.wmo_indices, p.name_id)?.to_string(),
                        is_wmo: true,
                        position: p.position,
                        rotation: p.rotation,
                        scale: 1.0,
                        bounds_min: Some(p.extents_min),
                        bounds_max: Some(p.extents_max),
                    });
                }
            }
            for p in &root.doodad_placements {
                if seen_ids.insert((false, p.unique_id)) {
                    placements.push(Placement {
                        name: resolve(&root.models, &root.model_indices, p.name_id)?.to_string(),
                        is_wmo: false,
                        position: p.position,
                        rotation: p.rotation,
                        scale: p.scale as f32 / 1024.0,
                        bounds_min: None,
                        bounds_max: None,
                    });
                }
            }
        }
    }
    Ok(TileScan {
        cells,
        placements,
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
                Mesh::M2(m2_tris(chain, &p.name))
            };
            meshes.insert(p.name.clone(), mesh);
        }
    }
    Ok(meshes)
}

/// Pass 3: calibrate the rotation convention against MODF bounds (WMOs only), capped sample.
pub(crate) fn calibrate_from_placements(
    chain: &mut PatchChain,
    placements: &[Placement],
) -> Result<Convention> {
    let mut samples: Vec<(&Placement, Vec<[f32; 3]>)> = Vec::new();
    let mut sample_verts: HashMap<&str, Vec<[f32; 3]>> = HashMap::new();
    let mut sampled_names: HashSet<&str> = HashSet::new();
    for p in placements
        .iter()
        .filter(|p| p.is_wmo)
        .filter(|p| sampled_names.insert(p.name.as_str()))
        .take(32)
    {
        let verts = match sample_verts.get(p.name.as_str()) {
            Some(v) => v.clone(),
            None => {
                let v = wmo_all_verts(chain, &p.name)?;
                sample_verts.insert(&p.name, v.clone());
                v
            }
        };
        if !verts.is_empty() {
            samples.push((p, verts));
        }
    }
    calibrate(&samples)
}

// ---------------------------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------------------------

pub(crate) fn run(args: &crate::Args) -> Result<()> {
    let data_dir = Path::new(args.nav.as_ref().expect("caller checked"));
    let mut chain = crate::collision::open_geometry_chain(data_dir)?;
    let map_name = crate::terrain::map_dir(args.map as u32)?;
    let map_id = args.map as u32;

    let (cell_x_min, cell_x_max, cell_y_min, cell_y_max) =
        crate::terrain::slice_cell_range(args.bbox, args.center, args.radius);
    for c in [cell_x_min, cell_x_max, cell_y_min, cell_y_max] {
        if !(0..1024).contains(&c) {
            bail!("slice cell index {c} outside the map square — check --box/--center/--radius");
        }
    }

    // Pass 1: parse tiles — heights + deduped placements.
    let scan = scan_tiles(
        &mut chain,
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
    let meshes = load_meshes(&mut chain, &placements)?;
    let mesh_tris: usize = meshes.values().map(Mesh::len).sum();
    println!(
        "nav: {} unique models, {mesh_tris} local tris",
        meshes.len()
    );

    // Pass 3: calibrate the rotation convention against MODF bounds (WMOs only), capped sample.
    let conv = calibrate_from_placements(&mut chain, &placements)?;

    // Pass 4: transform to world space + bin by terrain cell.
    let mut world_tris: Vec<WorldTri> = Vec::new();
    for p in &placements {
        let pos_w = place_pos(p.position);
        let local_tris: Vec<Tri> = match &meshes[&p.name] {
            Mesh::Wmo(v) => v.iter().map(|w| w.tri).collect(),
            Mesh::M2(v) => v.clone(),
        };
        for t in &local_tris {
            let w: Tri = [
                apply(conv, p.rotation, p.scale, pos_w, t[0]),
                apply(conv, p.rotation, p.scale, pos_w, t[1]),
                apply(conv, p.rotation, p.scale, pos_w, t[2]),
            ];
            world_tris.push(WorldTri::new(w, p.is_wmo));
        }
    }
    let mut by_cell: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, t) in world_tris.iter().enumerate() {
        // High world coord → LOW cell index; iterate the covered index rectangle, inflated by
        // `WALK_MARGIN` (#102 (a)) so a tri hugging a cell border also reaches the neighbor
        // cell its inset walk footprint spills into.
        let (Some(cx0), Some(cx1), Some(cy0), Some(cy1)) = (
            cell_index(t.hi[0] + WALK_MARGIN),
            cell_index(t.lo[0] - WALK_MARGIN),
            cell_index(t.hi[1] + WALK_MARGIN),
            cell_index(t.lo[1] - WALK_MARGIN),
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
        let tris: Vec<&WorldTri> = tri_idx.iter().map(|&i| &world_tris[i]).collect();
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
        let (ccx, ccy) = (args.center.0 as f32, args.center.1 as f32);
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
        println!("nav: self-check — --center walkable = {walkable}");
        if !walkable {
            bail!("nav self-check failed: --center rasterized UNwalkable — transform or band bug, not loading");
        }
    }

    // Batch + apply (byte-budgeted: rows vary 60 B – 3 KB).
    let mut batches: Vec<String> = Vec::new();
    let mut cur = String::new();
    for r in &rows {
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
    println!("nav: {} reducer batch(es)", batches.len());
    if !args.apply {
        println!("-- DRY RUN: would call import_nav_chunks (batch 0, clears) + {} × import_nav_chunks_append", batches.len().saturating_sub(1));
        return Ok(());
    }
    for (i, batch) in batches.iter().enumerate() {
        let reducer = if i == 0 {
            "import_nav_chunks"
        } else {
            "import_nav_chunks_append"
        };
        crate::call_reducer(args, reducer, batch)?;
    }
    println!("nav: applied.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::nav::{obs_top, walk_get};
    use lyracore_shared::terrain::cell_index;

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

    #[test]
    fn walk_grid_insets_by_the_agent_radius_but_obs_does_not() {
        // #102 (a): a thin vertical WMO wall down the x = walk-nx-32 line. The walk grid must
        // block one extra sub-cell on EACH side (the body inset — the corner-clip fix), while
        // the obs/LoS grid keeps the true footprint (sight is a line, not a body).
        let cell = flat_cell();
        let (cx, cy) = (cell.cell_x, cell.cell_y);
        let wall_x = sub_center(cx, 32, WALK_DIM);
        let (y0, y1) = (sub_center(cy, 60, WALK_DIM), sub_center(cy, 3, WALK_DIM));
        let wall = WorldTri::new(
            [[wall_x, y0, 79.0], [wall_x, y1, 79.0], [wall_x, y0, 92.0]],
            true,
        );
        let row = rasterize_cell(&cell, &[&wall]).expect("a wall dirties the cell");
        // On the wall line: blocked (pre-#102 behavior).
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
}
