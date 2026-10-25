use anyhow::{bail, Context, Result};
use lyracore_shared::nav::{
    derive_cell, merge_cells, sub_index, walk_set, NavCellData, OBS_BYTES, OBS_NONE, WALK_BYTES,
    WALK_DIM, WALK_MARGIN,
};
use lyracore_shared::terrain::{cell_index, cell_key, CELL_SIZE};
use lyracore_shared::vmap::{TriClass, VmapTri};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Cursor;
use std::path::Path;
use wow_mpq::PatchChain;

// ADT placement origin offset in yards.
const PLACE_K: f32 = 32.0 * 533.333_3;
// A 50-degree slope rises about 4.97 yards over one 4.17-yard terrain quad.
const MAX_QUAD_RISE: f32 = 4.97;
const BATCH_BYTES: usize = 28_000;

pub(crate) type Tri = [[f32; 3]; 3];

pub(crate) struct WmoTri {
    pub(crate) tri: Tri,
    pub(crate) group_id: u32,
    pub(crate) mogp_flags: u32,
}

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

// MOPY collision: 0x08 set, or neither 0x04 nor 0x02 set.
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
        let group = crate::collision::parse_wmo_group(&bytes, &gname)
            .with_context(|| format!("parsing {gname}"))?;
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

// Placement coordinates map to world as K - z, K - x, y.
pub(crate) fn place_pos(p: [f32; 3]) -> [f32; 3] {
    [PLACE_K - p[2], PLACE_K - p[0], p[1]]
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Convention {
    // true converts Y-up local axes to Z-up world axes as -z, -x, y.
    pub(crate) shuffle: bool,
    // World yaw in degrees is sign * rotation[1] + offset_deg.
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

// MCNK hole rows follow world X, matching the first heightmap index.
fn has_hole(holes: u32, quad_x: usize, quad_y: usize) -> bool {
    holes & (1 << ((quad_x / 2) * 4 + quad_y / 2)) != 0
}

fn terrain_cell(cell: &crate::terrain::CellRow) -> Option<NavCellData> {
    let mut walk = vec![0xFFu8; WALK_BYTES];
    let mut dirty = false;

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
            if rise > MAX_QUAD_RISE || has_hole(cell.holes, qi, qj) {
                dirty = true;
                for ny in qj * 8..(qj + 1) * 8 {
                    for nx in qi * 8..(qi + 1) * 8 {
                        walk_set(&mut walk, nx, ny, false);
                    }
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
    let tiles = crate::terrain::map_tiles(chain, map_name)?;
    for tx in tx_min..=tx_max {
        for ty in ty_min..=ty_max {
            if let Some(tiles) = &tiles {
                let present = tiles
                    .get((tx * 64 + ty) as usize)
                    .context("terrain tile coordinates outside the WDT grid")?;
                if !present {
                    continue;
                }
            }
            let candidates = [
                format!("World\\Maps\\{map_name}\\{map_name}_{ty}_{tx}.adt"),
                format!("World\\Maps\\{map_name}\\{map_name}_{tx}_{ty}.adt"),
            ];
            let mut accepted = None;
            for name in &candidates {
                let Ok(bytes) = chain.read_file(name) else {
                    continue;
                };
                // Empty ADT patch entries remove terrain tiles.
                if bytes.is_empty() {
                    continue;
                }
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
            };
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

pub(crate) fn placement_overlaps_cells(placement: &Placement, range: (i32, i32, i32, i32)) -> bool {
    let (Some(min), Some(max)) = (placement.bounds_min, placement.bounds_max) else {
        return true;
    };
    if !(0..3).all(|axis| min[axis].is_finite() && max[axis].is_finite() && min[axis] <= max[axis])
    {
        return true;
    }
    let low = place_pos(max);
    let high = place_pos(min);
    let (x_min, x_max, y_min, y_max) = range;
    [(x_min, x_max), (y_min, y_max)]
        .into_iter()
        .enumerate()
        .all(|(axis, (first, last))| {
            let cell_low = PLACE_K - (last + 1) as f32 * CELL_SIZE;
            let cell_high = PLACE_K - first as f32 * CELL_SIZE;
            high[axis] + WALK_MARGIN >= cell_low && low[axis] - WALK_MARGIN <= cell_high
        })
}

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

pub(crate) fn load_mesh(chain: &mut PatchChain, placement: &Placement) -> Result<Mesh> {
    if placement.is_wmo {
        Ok(Mesh::Wmo(wmo_tris(chain, &placement.name)?))
    } else {
        Ok(Mesh::M2(m2_tris(chain, &placement.name)?))
    }
}

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
        args.shard.call_reducer(
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

    let scan = scan_tiles(
        chain,
        map_name,
        map_id,
        (cell_x_min, cell_x_max, cell_y_min, cell_y_max),
    )?;
    let (cells, mut placements) = (scan.cells, scan.placements);
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

    let conv = calibrate_from_placements(chain, &placements)?;
    placements.retain(|placement| {
        placement_overlaps_cells(placement, (cell_x_min, cell_x_max, cell_y_min, cell_y_max))
    });

    let meshes = load_meshes(chain, &placements)?;
    let mesh_tris: usize = meshes.values().map(Mesh::len).sum();
    println!(
        "nav: {} unique models, {mesh_tris} local tris",
        meshes.len()
    );

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
        // Include WALK_MARGIN so inset geometry reaches neighboring terrain cells.
        let (lo, hi) = aabb(t.verts.iter().copied());
        let (Some(cx0), Some(cx1), Some(cy0), Some(cy1)) = (
            cell_index(hi[0] + WALK_MARGIN),
            cell_index(lo[0] - WALK_MARGIN),
            cell_index(hi[1] + WALK_MARGIN),
            cell_index(lo[1] - WALK_MARGIN),
        ) else {
            continue;
        };
        for cx in cx0..=cx1 {
            for cy in cy0..=cy1 {
                by_cell.entry(cell_key(map_id, cx, cy)).or_default().push(i);
            }
        }
    }

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
                let mut x = x1;
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

    {
        let (ccx, ccy) = (slice.sample.0 as f32, slice.sample.1 as f32);
        let (cx, cy) = (
            cell_index(ccx).context("--center off map")?,
            cell_index(ccy).context("--center off map")?,
        );
        let walkable = match rows.iter().find(|r| r.cell_x == cx && r.cell_y == cy) {
            None => true,
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

    fn scan_test_adt(bytes: Vec<u8>) -> Result<TileScan> {
        let path = std::env::temp_dir().join(format!(
            "lyracore-nav-adt-{}-{}.mpq",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        wow_mpq::ArchiveBuilder::new()
            .add_file_data(bytes, "World\\Maps\\Azeroth\\Azeroth_32_32.adt")
            .build(&path)
            .unwrap();
        let mut chain = PatchChain::new();
        chain.add_archive(&path, 0).unwrap();
        let result = scan_tiles(&mut chain, "Azeroth", 0, (512, 513, 512, 513));
        drop(chain);
        std::fs::remove_file(path).unwrap();
        result
    }

    #[test]
    fn empty_adt_is_an_absent_navigation_tile() {
        let scan = scan_test_adt(vec![]).unwrap();
        assert_eq!(scan.tiles_read, 0);
        assert!(scan.cells.is_empty());
        assert!(scan.placements.is_empty());
        assert!(scan.sources.is_empty());
    }

    #[test]
    fn malformed_nonempty_adt_still_fails_navigation_import() {
        let error = scan_test_adt(vec![1, 2, 3]).err().unwrap();
        assert!(format!("{error:#}").contains("parsing World"), "{error:#}");
    }

    fn wmo_tri(v: Tri) -> VmapTri {
        VmapTri {
            verts: v,
            class: TriClass::Wmo {
                group_id: 0,
                mogp_flags: 0,
            },
        }
    }

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
            deep_water_tiles: 0,
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
    fn bounded_mesh_selection_excludes_neighboring_wmos_but_keeps_border_geometry() {
        let range = (450, 451, 460, 461);
        let mut placement = test_placement(true);
        let origin = PLACE_K - 462.0 * CELL_SIZE;
        placement.bounds_min = Some([PLACE_K - origin + 10.0, 0.0, 450.0 * CELL_SIZE]);
        placement.bounds_max = Some([PLACE_K - origin + 20.0, 10.0, 451.0 * CELL_SIZE]);
        assert!(!placement_overlaps_cells(&placement, range));

        placement.bounds_min.as_mut().unwrap()[0] = PLACE_K - origin + WALK_MARGIN / 2.0;
        assert!(placement_overlaps_cells(&placement, range));
        placement.bounds_min.as_mut().unwrap()[0] = 460.0 * CELL_SIZE;
        assert!(placement_overlaps_cells(&placement, range));
        assert!(placement_overlaps_cells(&test_placement(false), range));
    }

    #[test]
    fn calibration_uses_authored_bounds_when_movt_does_not_fill_their_corners() {
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
        let cell = flat_cell();
        let (cx, cy) = (cell.cell_x, cell.cell_y);
        let wall_x = sub_center(cx, 32, WALK_DIM);
        let (y0, y1) = (sub_center(cy, 60, WALK_DIM), sub_center(cy, 3, WALK_DIM));
        let wall = wmo_tri([[wall_x, y0, 79.0], [wall_x, y1, 79.0], [wall_x, y0, 92.0]]);
        let row = rasterize_cell(&cell, &[wall]).expect("a wall dirties the cell");
        assert!(!walk_get(&row.walk, 32, 32));
        assert!(!walk_get(&row.walk, 31, 32));
        assert!(!walk_get(&row.walk, 33, 32));
        assert!(walk_get(&row.walk, 30, 32));
        assert!(walk_get(&row.walk, 34, 32));
        assert!(obs_top(&row.obs, row.base_z, 16, 16).is_some());
        assert!(obs_top(&row.obs, row.base_z, 15, 16).is_none());
        assert!(obs_top(&row.obs, row.base_z, 17, 16).is_none());
    }

    #[test]
    fn an_asymmetric_terrain_hole_uses_world_coordinate_axes() {
        let mut cell = flat_cell();
        cell.holes = 0x0010;
        let row = rasterize_cell(&cell, &[]).unwrap();
        assert!(!walk_get(&row.walk, 24, 8));
        assert!(walk_get(&row.walk, 8, 24));
    }

    #[test]
    fn a_row_carries_both_the_terrain_blockers_and_the_geometry_ones() {
        let mut cell = flat_cell();
        cell.holes = 1;
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
