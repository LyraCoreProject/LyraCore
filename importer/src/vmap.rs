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

use anyhow::{bail, Result};
use lyracore_shared::terrain::cell_key;
use lyracore_shared::vmap::{encode, TriClass, VmapTri, HEADER_BYTES, TRI_BYTES};
use std::collections::HashMap;
use std::path::Path;

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
pub(crate) fn bin_by_cell(map_id: u32, tris: &[VmapTri]) -> HashMap<u64, Vec<VmapTri>> {
    let mut by_cell: HashMap<u64, Vec<VmapTri>> = HashMap::new();
    for t in tris {
        let Some((cx0, cx1, cy0, cy1)) = tri_cell_range(t) else {
            continue; // off the map square (shouldn't happen inside the box)
        };
        for cx in cx0..=cx1 {
            for cy in cy0..=cy1 {
                by_cell.entry(cell_key(map_id, cx, cy)).or_default().push(*t);
            }
        }
    }
    by_cell
}

pub(crate) fn run(args: &crate::Args) -> Result<()> {
    let data_dir = Path::new(args.vmap.as_ref().expect("caller checked"));
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

    // Pass 1: parse tiles — heights (unused here) + deduped placements.
    let scan = crate::nav::scan_tiles(
        &mut chain,
        map_name,
        map_id,
        (cell_x_min, cell_x_max, cell_y_min, cell_y_max),
    )?;
    if scan.cells.is_empty() {
        bail!("no MCNK cells intersected the slice");
    }
    println!(
        "vmap: {} tile(s), {} unique placements ({} WMO)",
        scan.tiles_read,
        scan.placements.len(),
        scan.placements.iter().filter(|p| p.is_wmo).count()
    );

    // Pass 2: load each referenced model's collision mesh once.
    let meshes = crate::nav::load_meshes(&mut chain, &scan.placements)?;
    let mesh_tris: usize = meshes.values().map(Mesh::len).sum();
    println!(
        "vmap: {} unique models, {mesh_tris} local tris",
        meshes.len()
    );

    // Pass 3: calibrate the rotation convention against MODF bounds (WMOs only), capped sample —
    // same calibration the rasterizer uses; only the yaw term is empirically fit.
    let conv = crate::nav::calibrate_from_placements(&mut chain, &scan.placements)?;

    // Pass 4: transform every placement's mesh to world space (FULL rotation) + bin by cell.
    let mut world_tris: Vec<VmapTri> = Vec::new();
    for p in &scan.placements {
        world_tris.extend(placement_tris(p, &meshes[&p.name], conv));
    }
    let by_cell = bin_by_cell(map_id, &world_tris);

    // Pass 5: shard + pack each cell's bin once — feeds both the size report and (if --apply)
    // the reducer batches below. A dense cell (e.g. a WMO complex whose AABB touches it) can hold
    // far more triangles than fit in one `spacetime call` CLI argument (Linux caps a single argv
    // string around 128 KB), so a cell's triangle list is FIRST split into `MAX_ROW_TRI_BYTES`-
    // capped shards — each independently `lyracore_shared::vmap::decode`able — and only THEN
    // batched by total payload size like `nav.rs`. `game_vmap_chunk` is a multi-row-per-cell
    // table for exactly this reason (see its doc comment).
    let cell_blobs: Vec<(u64, Vec<u8>)> = by_cell
        .iter()
        .flat_map(|(&key, tris)| shard_cell(tris).into_iter().map(move |b| (key, b)))
        .collect();
    let total_bytes: usize = cell_blobs.iter().map(|(_, b)| b.len()).sum();
    let wmo_tris = world_tris
        .iter()
        .filter(|t| matches!(t.class, TriClass::Wmo { .. }))
        .count();
    let m2_tris = world_tris.len() - wmo_tris;
    println!(
        "vmap: map {map_id} — {} world tris ({wmo_tris} WMO, {m2_tris} M2) across {} cells, \
         {total_bytes} packed bytes ({:.1} KB, {} shard row(s))",
        world_tris.len(),
        by_cell.len(),
        total_bytes as f64 / 1024.0,
        cell_blobs.len()
    );

    // Pass 6 (#521): batch shard rows by byte budget + apply — same convention as `nav::run`
    // (rows `;` separated, first batch clears via `import_vmap_chunks`, the rest append). No row
    // can exceed `BATCH_BYTES` on its own now (shards are already capped well under it), so this
    // batching step only ever GROUPS rows, never has to special-case an oversized one.
    let mut batches: Vec<String> = Vec::new();
    let mut cur = String::new();
    for (key, blob) in &cell_blobs {
        let cell_x = ((key >> 16) & 0xFFFF) as u16;
        let cell_y = (key & 0xFFFF) as u16;
        let hex: String = blob.iter().map(|b| format!("{b:02x}")).collect();
        let row = format!("{map_id},{cell_x},{cell_y},{hex}");
        if !cur.is_empty() && cur.len() + row.len() + 1 > BATCH_BYTES {
            batches.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(';');
        }
        cur.push_str(&row);
    }
    if !cur.is_empty() {
        batches.push(cur);
    }
    println!("vmap: {} reducer batch(es)", batches.len());
    if !args.apply {
        println!(
            "-- DRY RUN: would call import_vmap_chunks (batch 0, clears) + {} × import_vmap_chunks_append",
            batches.len().saturating_sub(1)
        );
        return Ok(());
    }
    for (i, batch) in batches.iter().enumerate() {
        let reducer = if i == 0 {
            "import_vmap_chunks"
        } else {
            "import_vmap_chunks_append"
        };
        crate::call_reducer(args, reducer, batch)?;
    }
    println!("vmap: applied.");
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
fn shard_cell(tris: &[VmapTri]) -> Vec<Vec<u8>> {
    let per_shard = ((MAX_ROW_TRI_BYTES.saturating_sub(HEADER_BYTES)) / TRI_BYTES).max(1);
    if tris.is_empty() {
        return vec![encode(tris)]; // preserve the header-only "cell exists but empty" row
    }
    tris.chunks(per_shard).map(encode).collect()
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
        let (cx, cy) = (
            (x_hi + x_lo) / 2.0,
            (y_hi + y_lo) / 2.0,
        );
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
}
