//! Per-cell collision-triangle blob codec — the exact-model half of decision #10 (`docs/decisions.md`
//! §10, epic #169). One blob per terrain cell (same 33.3 yd `terrain::cell_key` the nav grid bins
//! by): every world-space collision triangle whose AABB touches that cell, carrying enough source
//! metadata for the two vanilla ray flavors the next slice adds (LoS = WMO only, collision = WMO +
//! M2 doodads) — see decision #10 for why LoS excludes doodads.
//!
//! Unlike `nav`'s fixed-size blobs this format is variable-length (a cell's triangle count isn't
//! bounded), so it carries an explicit VERSION byte: a schema change bumps `VMAP_VERSION` and
//! `decode` rejects anything else outright rather than silently misreading bytes.
//!
//! #520 extracted, binned, packed and reported. #521 (this file's `cast_ray` half) adds the query
//! surface: exact segment–triangle tests, walking only the terrain cells the ray's XY projection
//! crosses (a 2D DDA, Amanatides–Woo) rather than sampling every few tenths of a yard like
//! `nav::has_los`'s parametric scan — an exact per-triangle store can afford (and needs) an exact
//! walk. Two vanilla ray flavors: LoS (`RayFlavor::Los`, WMO-class triangles only — doodads/forests
//! never block sight) and collision (`RayFlavor::Collision`, WMO + M2 doodads, for movement/reach
//! clamps). The module-side table + import reducers + `vmap_enabled` gate live in
//! `module/src/vmap.rs`.

/// Blob format version. Bump on any layout change; `decode` rejects a mismatch rather than guess.
pub const VMAP_VERSION: u8 = 1;

/// One packed triangle's source: which vanilla ray flavors it participates in. WMO carries its
/// group id + raw MOGP flags (indoor/outdoor bits live in there — decoded by the query surface,
/// not this codec); M2 (doodad) triangles have neither and never gate the WMO-only LoS ray.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TriClass {
    Wmo { group_id: u32, mogp_flags: u32 },
    M2,
}

/// One world-space collision triangle plus its source class.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VmapTri {
    pub verts: [[f32; 3]; 3],
    pub class: TriClass,
}

/// Bytes per packed triangle: 1 (class tag) + 4 (group_id) + 4 (mogp_flags) + 36 (9 × f32 verts).
/// `pub` so the importer can size a per-blob shard cap (`#521`: a dense cell's triangle count
/// isn't bounded, but a single `spacetime call` argument is — see `importer/src/vmap.rs`).
pub const TRI_BYTES: usize = 1 + 4 + 4 + 36;
/// Header: 1 version byte + 4-byte LE triangle count. `pub` for the same reason as `TRI_BYTES`.
pub const HEADER_BYTES: usize = 5;

const CLASS_WMO: u8 = 0;
const CLASS_M2: u8 = 1;

#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// Blob shorter than the fixed header, or shorter than its declared triangle count implies.
    Truncated,
    /// First byte isn't `VMAP_VERSION` — a schema mismatch, not a corrupt read.
    VersionMismatch(u8),
    /// A triangle's class tag byte is neither `CLASS_WMO` nor `CLASS_M2`.
    BadClassTag(u8),
}

/// Encode a batch of triangles (any order, any cell membership — the caller bins by cell before
/// calling this per-cell) into one versioned blob.
pub fn encode(tris: &[VmapTri]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(HEADER_BYTES + tris.len() * TRI_BYTES);
    buf.push(VMAP_VERSION);
    buf.extend_from_slice(&(tris.len() as u32).to_le_bytes());
    for t in tris {
        let (tag, group_id, mogp_flags) = match t.class {
            TriClass::Wmo {
                group_id,
                mogp_flags,
            } => (CLASS_WMO, group_id, mogp_flags),
            TriClass::M2 => (CLASS_M2, 0, 0),
        };
        buf.push(tag);
        buf.extend_from_slice(&group_id.to_le_bytes());
        buf.extend_from_slice(&mogp_flags.to_le_bytes());
        for v in &t.verts {
            for c in v {
                buf.extend_from_slice(&c.to_le_bytes());
            }
        }
    }
    buf
}

/// Decode a blob produced by `encode`. Rejects a version mismatch or a short/truncated buffer
/// outright — never returns a partial result.
pub fn decode(blob: &[u8]) -> Result<Vec<VmapTri>, DecodeError> {
    if blob.len() < HEADER_BYTES {
        return Err(DecodeError::Truncated);
    }
    let version = blob[0];
    if version != VMAP_VERSION {
        return Err(DecodeError::VersionMismatch(version));
    }
    let count = u32::from_le_bytes([blob[1], blob[2], blob[3], blob[4]]) as usize;
    let need = HEADER_BYTES + count * TRI_BYTES;
    if blob.len() < need {
        return Err(DecodeError::Truncated);
    }
    let mut out = Vec::with_capacity(count);
    let mut off = HEADER_BYTES;
    for _ in 0..count {
        let tag = blob[off];
        let group_id = u32::from_le_bytes(blob[off + 1..off + 5].try_into().unwrap());
        let mogp_flags = u32::from_le_bytes(blob[off + 5..off + 9].try_into().unwrap());
        let class = match tag {
            CLASS_WMO => TriClass::Wmo {
                group_id,
                mogp_flags,
            },
            CLASS_M2 => TriClass::M2,
            other => return Err(DecodeError::BadClassTag(other)),
        };
        let mut verts = [[0.0f32; 3]; 3];
        let mut vo = off + 9;
        for v in &mut verts {
            for c in v.iter_mut() {
                *c = f32::from_le_bytes(blob[vo..vo + 4].try_into().unwrap());
                vo += 4;
            }
        }
        out.push(VmapTri { verts, class });
        off += TRI_BYTES;
    }
    Ok(out)
}

// ===========================================================================================
//  Ray queries (#521) — exact segment–triangle tests over a 2D DDA cell walk.
// ===========================================================================================

/// Which vanilla ray flavor to cast: LoS ignores doodads (M2 never blocks sight); collision
/// blocks on both WMO and M2 triangles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RayFlavor {
    Los,
    Collision,
}

/// Cast an exact segment `a -> b` against the stored triangles, visiting only the terrain cells
/// the segment's XY projection crosses. `fetch` decodes and returns one cell's triangles, or
/// `None` for an unimported/empty cell — treated as "no obstruction known here", the same
/// missing-chunk contract `nav` uses. Returns the *nearest* hit world point, or `None` for a
/// clear segment.
///
/// Cells are still walked in DDA order, but a cell can't stop the search early: the importer
/// (`importer::bin_by_cell`) duplicates a triangle into every cell its AABB touches, so a large
/// triangle can be found in an earlier cell than the true nearest obstruction that lives (only)
/// in a later one. Correctness requires scanning every crossed cell and taking the global min
/// `t`, not the first cell with any hit — the intersection point of any triangle that truly hits
/// the segment lies within that triangle's AABB, hence within a cell the same DDA walk visits, so
/// the global minimum found this way is exact.
pub fn cast_ray(
    fetch: &mut impl FnMut(u16, u16) -> Option<Vec<VmapTri>>,
    a: [f32; 3],
    b: [f32; 3],
    flavor: RayFlavor,
) -> Option<[f32; 3]> {
    let want =
        |class: &TriClass| flavor == RayFlavor::Collision || matches!(class, TriClass::Wmo { .. });
    nearest_hit(fetch, a, b, want).map(|(t, _)| point_at(a, b, t))
}

/// Shared nearest-hit walk behind `cast_ray` and `cast_ray_area`: same DDA cell walk + global-min
/// `t` correctness argument (see `cast_ray`'s doc comment), parameterized on which triangle
/// classes participate. Returns the winning hit parameter *and* that triangle's class, so a caller
/// that needs the source metadata (the WMO group/flags an area-info query reports) doesn't have to
/// re-walk the segment.
fn nearest_hit(
    fetch: &mut impl FnMut(u16, u16) -> Option<Vec<VmapTri>>,
    a: [f32; 3],
    b: [f32; 3],
    want: impl Fn(&TriClass) -> bool,
) -> Option<(f32, TriClass)> {
    let mut best: Option<(f32, TriClass)> = None;
    for (cx, cy) in crossed_cells(a, b) {
        let Some(tris) = fetch(cx, cy) else {
            continue;
        };
        for t in &tris {
            if !want(&t.class) {
                continue;
            }
            if let Some(hit_t) = segment_tri_hit(a, b, t.verts) {
                if best.as_ref().is_none_or(|(cur, _)| hit_t < *cur) {
                    best = Some((hit_t, t.class));
                }
            }
        }
    }
    best
}

fn point_at(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Bit in `TriClass::Wmo`'s `mogp_flags` marking a WMO group as an OUTDOOR space (vanilla WMO
/// group flags, MOGP chunk — mangos' `MOGP_FLAG_OUTDOOR`). Mangos does not test an "indoor" bit:
/// a group the probe actually found is interior UNLESS it carries this one.
pub const WMO_GROUP_OUTDOOR_FLAG: u32 = 0x8000;

/// Is a found WMO group an interior space? Mangos' rule, and the ONLY place the MOGP bit is
/// interpreted — the module's per-cell indoor-presence table classifies staged triangles through
/// this same function, so a query and its pre-reject can never disagree about a group.
pub fn group_is_indoor(mogp_flags: u32) -> bool {
    mogp_flags & WMO_GROUP_OUTDOOR_FLAG == 0
}

/// How far BELOW the queried point an AREA probe segment reaches (yd). An area query asks which
/// group a point is *in*, so its budget is the height of a room, not the ray/floor probes'
/// 200 yd reach — that one finds a WMO two storeys down and reports the point as inside it.
pub const AREA_PROBE_DOWN_YD: f32 = 10.0;

/// Point-in-WMO-group area-info: which WMO group (if any) contains `point`, and is it indoor —
/// mangos parity (`VMapManager2::getAreaInfo`). Implemented the same way mangos does: cast a
/// short vertical probe segment (`a` above `point`, `b` below it, no longer than
/// [`AREA_PROBE_DOWN_YD`]) and take the *nearest* WMO-class hit — the first model surface the
/// point sits under/on belongs to the containing group. `None` when no WMO triangle lies in the
/// probe range (outdoors, or vmap unimported for this cell).
pub fn cast_ray_area(
    fetch: &mut impl FnMut(u16, u16) -> Option<Vec<VmapTri>>,
    a: [f32; 3],
    b: [f32; 3],
) -> Option<AreaInfo> {
    let (t, class) = nearest_hit(fetch, a, b, |class| matches!(class, TriClass::Wmo { .. }))?;
    let TriClass::Wmo {
        group_id,
        mogp_flags,
    } = class
    else {
        unreachable!("nearest_hit filtered to TriClass::Wmo");
    };
    Some(AreaInfo {
        group_id,
        indoor: group_is_indoor(mogp_flags),
        point: point_at(a, b, t),
    })
}

/// Result of `cast_ray_area`: the containing WMO group and whether it's an indoor space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AreaInfo {
    pub group_id: u32,
    pub indoor: bool,
    pub point: [f32; 3],
}

/// Terrain cells the segment's XY projection crosses, in traversal order from `a` to `b` — a 2D
/// DDA (Amanatides–Woo) over the same DOWNWARD-counting "cell index = floor(u / CELL_SIZE)"
/// convention as `terrain::cell_index` (`u = MAP_COORD_MAX - coord`, so higher world coord = lower
/// cell index). Cells outside the 1024×1024 map square are dropped, not clamped — a segment that
/// runs off the map square simply stops contributing hits there.
fn crossed_cells(a: [f32; 3], b: [f32; 3]) -> Vec<(u16, u16)> {
    let ua = (
        crate::spatial::MAP_COORD_MAX - a[0],
        crate::spatial::MAP_COORD_MAX - a[1],
    );
    let ub = (
        crate::spatial::MAP_COORD_MAX - b[0],
        crate::spatial::MAP_COORD_MAX - b[1],
    );
    let (dx, dy) = (ub.0 - ua.0, ub.1 - ua.1);

    let mut cx = (ua.0 / crate::terrain::CELL_SIZE).floor() as i64;
    let mut cy = (ua.1 / crate::terrain::CELL_SIZE).floor() as i64;
    let end_cx = (ub.0 / crate::terrain::CELL_SIZE).floor() as i64;
    let end_cy = (ub.1 / crate::terrain::CELL_SIZE).floor() as i64;

    let step_x: i64 = if dx > 0.0 {
        1
    } else if dx < 0.0 {
        -1
    } else {
        0
    };
    let step_y: i64 = if dy > 0.0 {
        1
    } else if dy < 0.0 {
        -1
    } else {
        0
    };
    let t_delta_x = if dx != 0.0 {
        crate::terrain::CELL_SIZE / dx.abs()
    } else {
        f32::INFINITY
    };
    let t_delta_y = if dy != 0.0 {
        crate::terrain::CELL_SIZE / dy.abs()
    } else {
        f32::INFINITY
    };
    let next_x = if step_x > 0 {
        (cx + 1) as f32 * crate::terrain::CELL_SIZE
    } else {
        cx as f32 * crate::terrain::CELL_SIZE
    };
    let next_y = if step_y > 0 {
        (cy + 1) as f32 * crate::terrain::CELL_SIZE
    } else {
        cy as f32 * crate::terrain::CELL_SIZE
    };
    let mut t_max_x = if dx != 0.0 {
        (next_x - ua.0) / dx
    } else {
        f32::INFINITY
    };
    let mut t_max_y = if dy != 0.0 {
        (next_y - ua.1) / dy
    } else {
        f32::INFINITY
    };

    let push = |cx: i64, cy: i64, out: &mut Vec<(u16, u16)>| {
        if (0..1024).contains(&cx) && (0..1024).contains(&cy) {
            out.push((cx as u16, cy as u16));
        }
    };
    let mut out = Vec::new();
    push(cx, cy, &mut out);
    // Safety cap: a segment cannot legitimately cross more cells than the map's diagonal; guards
    // a degenerate (NaN/inf) input from looping instead of terminating on the `cx==end && cy==end`
    // condition below.
    for _ in 0..2048 {
        if cx == end_cx && cy == end_cy {
            break;
        }
        if t_max_x < t_max_y {
            cx += step_x;
            t_max_x += t_delta_x;
        } else {
            cy += step_y;
            t_max_y += t_delta_y;
        }
        push(cx, cy, &mut out);
    }
    out
}

const RAY_EPS: f32 = 1e-6;

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Möller–Trumbore, no backface culling (both winding orders block), segment-clamped (`t` in
/// `0..=1`, not an infinite ray). Returns the hit parameter along `a -> b`, or `None`.
fn segment_tri_hit(a: [f32; 3], b: [f32; 3], tri: [[f32; 3]; 3]) -> Option<f32> {
    let dir = sub3(b, a);
    let e1 = sub3(tri[1], tri[0]);
    let e2 = sub3(tri[2], tri[0]);
    let pvec = cross3(dir, e2);
    let det = dot3(e1, pvec);
    if det.abs() < RAY_EPS {
        return None; // parallel to the triangle's plane
    }
    let inv_det = 1.0 / det;
    let tvec = sub3(a, tri[0]);
    let u = dot3(tvec, pvec) * inv_det;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let qvec = cross3(tvec, e1);
    let v = dot3(dir, qvec) * inv_det;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = dot3(e2, qvec) * inv_det;
    if (0.0..=1.0).contains(&t) {
        Some(t)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri(z: f32, class: TriClass) -> VmapTri {
        VmapTri {
            verts: [[0.0, 0.0, z], [1.0, 0.0, z], [0.0, 1.0, z]],
            class,
        }
    }

    #[test]
    fn round_trips_a_mixed_batch_exactly() {
        let batch = vec![
            tri(
                10.0,
                TriClass::Wmo {
                    group_id: 3,
                    mogp_flags: WMO_GROUP_OUTDOOR_FLAG,
                },
            ),
            tri(20.0, TriClass::M2),
            tri(
                30.5,
                TriClass::Wmo {
                    group_id: 0,
                    mogp_flags: 0,
                },
            ),
        ];
        let blob = encode(&batch);
        let back = decode(&blob).expect("round trip");
        assert_eq!(back, batch);
    }

    #[test]
    fn m2_triangles_carry_no_group_semantics() {
        let batch = vec![tri(5.0, TriClass::M2)];
        let blob = encode(&batch);
        let back = decode(&blob).unwrap();
        assert_eq!(back[0].class, TriClass::M2);
    }

    #[test]
    fn empty_batch_round_trips_to_a_header_only_blob() {
        let blob = encode(&[]);
        assert_eq!(blob.len(), HEADER_BYTES);
        assert_eq!(decode(&blob).unwrap(), Vec::new());
    }

    #[test]
    fn rejects_a_version_mismatch() {
        let mut blob = encode(&[tri(1.0, TriClass::M2)]);
        blob[0] = VMAP_VERSION + 1;
        assert_eq!(
            decode(&blob),
            Err(DecodeError::VersionMismatch(VMAP_VERSION + 1))
        );
    }

    #[test]
    fn rejects_a_truncated_blob() {
        let blob = encode(&[tri(1.0, TriClass::M2), tri(2.0, TriClass::M2)]);
        assert_eq!(decode(&blob[..blob.len() - 3]), Err(DecodeError::Truncated));
        assert_eq!(decode(&blob[..2]), Err(DecodeError::Truncated));
    }

    // -------------------------------------------------------------------------------------
    //  Ray-query tests (#521) — a synthetic stand-in for the done-when's live scenarios: a
    //  vertical "column" wall (abbey pillar analogue), a clear segment, and a doodad that
    //  blocks the collision ray but not the LoS ray.
    // -------------------------------------------------------------------------------------

    /// A single-cell world: cell (500, 500), one big quad (two triangles) standing upright in
    /// the XZ plane at fixed y, spanning the full cell so any horizontal segment through it at
    /// z in [0, 10] must hit — a stand-in for a solid wall/column.
    fn wall_cell(class: TriClass) -> (u16, u16, Vec<VmapTri>) {
        use crate::spatial::MAP_COORD_MAX;
        use crate::terrain::CELL_SIZE;
        let (cx, cy): (u16, u16) = (500, 500);
        let x_hi = MAP_COORD_MAX - cx as f32 * CELL_SIZE;
        let x_lo = x_hi - CELL_SIZE;
        let wall_y = MAP_COORD_MAX - (cy as f32 * CELL_SIZE + CELL_SIZE / 2.0); // cell center
        let tris = vec![
            VmapTri {
                verts: [
                    [x_lo, wall_y, 0.0],
                    [x_hi, wall_y, 0.0],
                    [x_lo, wall_y, 10.0],
                ],
                class,
            },
            VmapTri {
                verts: [
                    [x_hi, wall_y, 0.0],
                    [x_hi, wall_y, 10.0],
                    [x_lo, wall_y, 10.0],
                ],
                class,
            },
        ];
        (cx, cy, tris)
    }

    #[test]
    fn a_ray_through_a_wall_hits_with_a_plausible_first_hit_point() {
        let (cx, cy, tris) = wall_cell(TriClass::Wmo {
            group_id: 1,
            mogp_flags: 0,
        });
        let mut fetch = |fx: u16, fy: u16| (fx == cx && fy == cy).then(|| tris.clone());
        // Straight segment crossing the wall's y at mid-height — must hit near the wall's y.
        let wall_y = tris[0].verts[0][1];
        let mid_x = tris[0].verts[0][0].midpoint(tris[0].verts[1][0]);
        let start = [mid_x, wall_y - 20.0, 5.0];
        let end = [mid_x, wall_y + 20.0, 5.0];
        let hit = cast_ray(&mut fetch, start, end, RayFlavor::Collision);
        let p = hit.expect("segment through the wall must hit");
        assert!(
            (p[1] - wall_y).abs() < 1e-2,
            "hit y should sit on the wall: {p:?}"
        );
        assert!(
            (p[2] - 5.0).abs() < 1e-2,
            "hit z should match the segment's height: {p:?}"
        );
    }

    #[test]
    fn a_clear_segment_misses() {
        let (cx, cy, tris) = wall_cell(TriClass::Wmo {
            group_id: 1,
            mogp_flags: 0,
        });
        let mut fetch = |fx: u16, fy: u16| (fx == cx && fy == cy).then(|| tris.clone());
        let wall_y = tris[0].verts[0][1];
        // Segment entirely on one side of the wall — never crosses its plane.
        let start = [tris[0].verts[0][0], wall_y - 20.0, 5.0];
        let end = [tris[0].verts[0][0], wall_y - 10.0, 5.0];
        assert_eq!(cast_ray(&mut fetch, start, end, RayFlavor::Collision), None);
    }

    #[test]
    fn a_nearer_hit_in_a_later_cell_beats_a_farther_hit_duplicated_into_an_earlier_cell() {
        // Regression for the "first-cell-with-any-hit" bug: the importer (`bin_by_cell`)
        // duplicates a triangle into every cell its AABB touches, so a large far wall can be
        // found in an earlier-visited cell while a small near obstruction lives only in a
        // later-visited cell. cast_ray must report the globally nearest hit, not the first cell's.
        use crate::spatial::MAP_COORD_MAX;
        use crate::terrain::CELL_SIZE;
        let (cx_a, cy): (u16, u16) = (500, 500);
        let cx_b = cx_a + 1; // adjacent cell, visited after A by the DDA (see `crossed_cells`)
        let x_hi_a = MAP_COORD_MAX - cx_a as f32 * CELL_SIZE;
        // Ray's fixed world y, at the center of cell row `cy` (so `crossed_cells` reports the
        // same `cy` for every visited cell — only `cx` advances).
        let y_world = MAP_COORD_MAX - (cy as f32 * CELL_SIZE + CELL_SIZE / 2.0);

        fn wall_at(x: f32, y_world: f32, class: TriClass) -> VmapTri {
            // A plane perpendicular to the ray's travel axis (x), spanning y and z around the
            // ray's fixed y, wide enough that the ray (at y=y_world, z=5) hits it.
            VmapTri {
                verts: [
                    [x, y_world - 50.0, 0.0],
                    [x, y_world + 50.0, 0.0],
                    [x, y_world - 50.0, 50.0],
                ],
                class,
            }
        }
        let wmo = |id| TriClass::Wmo {
            group_id: id,
            mogp_flags: 0,
        };
        let x_far = x_hi_a - CELL_SIZE - 20.0; // deep in cell B — the true farther obstruction
        let x_near = x_hi_a - CELL_SIZE - 5.0; // still inside cell B, nearer the A/B boundary
        let far_wall = wall_at(x_far, y_world, wmo(1));
        let near_wall = wall_at(x_near, y_world, wmo(2));

        // Cell A's fetch carries a duplicate of the far wall (as the importer would bin a
        // triangle whose AABB reaches back into a cell the ray crosses before the true hit).
        // Cell B carries both, matching where they truly sit.
        let cell_a_tris = vec![far_wall];
        let cell_b_tris = vec![far_wall, near_wall];
        let mut fetch = |fx: u16, fy: u16| {
            if fx == cx_a && fy == cy {
                Some(cell_a_tris.clone())
            } else if fx == cx_b && fy == cy {
                Some(cell_b_tris.clone())
            } else {
                None
            }
        };
        // Ray runs along -x, from inside cell A, through both walls, at fixed y/z.
        let start = [x_hi_a - 1.0, y_world, 5.0];
        let end = [x_far - 10.0, y_world, 5.0];
        let hit = cast_ray(&mut fetch, start, end, RayFlavor::Collision)
            .expect("segment must hit something");
        assert!(
            (hit[0] - x_near).abs() < 1e-2,
            "must report the nearer obstruction (x={x_near}), not the farther duplicate found \
             in the earlier cell: {hit:?}"
        );
    }

    #[test]
    fn a_doodad_blocks_the_collision_ray_but_not_the_los_ray() {
        let (cx, cy, tris) = wall_cell(TriClass::M2);
        let mut fetch = |fx: u16, fy: u16| (fx == cx && fy == cy).then(|| tris.clone());
        let wall_y = tris[0].verts[0][1];
        let mid_x = tris[0].verts[0][0].midpoint(tris[0].verts[1][0]);
        let start = [mid_x, wall_y - 20.0, 5.0];
        let end = [mid_x, wall_y + 20.0, 5.0];
        assert!(cast_ray(&mut fetch, start, end, RayFlavor::Collision).is_some());
        assert_eq!(cast_ray(&mut fetch, start, end, RayFlavor::Los), None);
    }

    // -------------------------------------------------------------------------------------
    //  Area-info tests (#527) — a synthetic stand-in for the done-when's live scenarios: a
    //  horizontal floor quad tagged indoor (abbey-interior analogue) under a probe reports the
    //  group + indoor=true; a probe with no WMO geometry in range reports "outdoors" (`None`).
    // -------------------------------------------------------------------------------------

    /// A single-cell world: cell (500, 500), one big quad (two triangles) lying flat at a fixed
    /// z, spanning the full cell — a stand-in for a WMO group's interior floor.
    fn floor_cell(group_id: u32, mogp_flags: u32) -> (u16, u16, Vec<VmapTri>) {
        use crate::spatial::MAP_COORD_MAX;
        use crate::terrain::CELL_SIZE;
        let (cx, cy): (u16, u16) = (500, 500);
        let x_hi = MAP_COORD_MAX - cx as f32 * CELL_SIZE;
        let x_lo = x_hi - CELL_SIZE;
        let y_hi = MAP_COORD_MAX - cy as f32 * CELL_SIZE;
        let y_lo = y_hi - CELL_SIZE;
        let class = TriClass::Wmo {
            group_id,
            mogp_flags,
        };
        let tris = vec![
            VmapTri {
                verts: [[x_lo, y_lo, 10.0], [x_hi, y_lo, 10.0], [x_lo, y_hi, 10.0]],
                class,
            },
            VmapTri {
                verts: [[x_hi, y_lo, 10.0], [x_hi, y_hi, 10.0], [x_lo, y_hi, 10.0]],
                class,
            },
        ];
        (cx, cy, tris)
    }

    /// The MOGP rule, at the bit level. Mangos tests ONE bit and tests it for OUTDOOR: a found
    /// group is interior unless `0x8000` is set. The pre-fix code read `0x2000` as "indoor",
    /// which was the wrong bit AND the wrong polarity — under it a plain interior group (flags 0,
    /// the common case) read as outdoors and a group carrying the outdoor bit read as indoors, so
    /// an indoor dismount built on it would have fired in the open world and nowhere else.
    #[test]
    fn only_the_outdoor_bit_decides_a_found_group() {
        assert!(
            group_is_indoor(0),
            "neither bit set — a found group is indoor"
        );
        assert!(
            group_is_indoor(0x2000),
            "0x2000 is not the outdoor bit and must not make a group outdoor"
        );
        assert!(!group_is_indoor(WMO_GROUP_OUTDOOR_FLAG));
        assert!(
            !group_is_indoor(WMO_GROUP_OUTDOOR_FLAG | 0x2000),
            "the outdoor bit wins whatever else is set"
        );
    }

    /// The area probe's budget is a room's height, not the 200 yd floor/ray budget: a probe that
    /// long finds a WMO two storeys under an open-world point and reports the point as inside it.
    /// Asserted against a floor deliberately placed BELOW the budget, so shortening the constant to
    /// nothing fails here too.
    #[test]
    fn a_probe_no_longer_than_the_area_budget_misses_a_distant_floor() {
        let (cx, cy, tris) = floor_cell(3, 0);
        let mut fetch = |fx: u16, fy: u16| (fx == cx && fy == cy).then(|| tris.clone());
        let x = tris[0].verts[0][0].midpoint(tris[0].verts[1][0]);
        let y = tris[0].verts[0][1].midpoint(tris[0].verts[2][1]);
        // The floor sits at z = 10; stand far enough above it that the SHORT budget cannot reach.
        let stand_z = 10.0 + AREA_PROBE_DOWN_YD * 3.0;
        assert_eq!(
            cast_ray_area(
                &mut fetch,
                [x, y, stand_z],
                [x, y, stand_z - AREA_PROBE_DOWN_YD]
            ),
            None,
            "a point well above a floor must not be reported as inside its group"
        );
        assert!(
            cast_ray_area(&mut fetch, [x, y, stand_z], [x, y, stand_z - 200.0]).is_some(),
            "the long ray/floor budget WOULD have found it — that is exactly the bug"
        );
    }

    #[test]
    fn a_probe_over_an_indoor_group_floor_reports_indoor_and_its_group() {
        let (cx, cy, tris) = floor_cell(7, 0);
        let mut fetch = |fx: u16, fy: u16| (fx == cx && fy == cy).then(|| tris.clone());
        let x = tris[0].verts[0][0].midpoint(tris[0].verts[1][0]);
        let y = tris[0].verts[0][1].midpoint(tris[0].verts[2][1]);
        let above = [x, y, 15.0];
        let below = [x, y, -5.0];
        let info = cast_ray_area(&mut fetch, above, below).expect("probe must hit the floor");
        assert_eq!(info.group_id, 7);
        assert!(info.indoor, "outdoor bit unset — must report indoor");
        assert!((info.point[2] - 10.0).abs() < 1e-2);
    }

    #[test]
    fn a_probe_over_an_outdoor_group_floor_reports_not_indoor() {
        let (cx, cy, tris) = floor_cell(9, WMO_GROUP_OUTDOOR_FLAG);
        let mut fetch = |fx: u16, fy: u16| (fx == cx && fy == cy).then(|| tris.clone());
        let x = tris[0].verts[0][0].midpoint(tris[0].verts[1][0]);
        let y = tris[0].verts[0][1].midpoint(tris[0].verts[2][1]);
        let info = cast_ray_area(&mut fetch, [x, y, 15.0], [x, y, -5.0])
            .expect("probe must hit the floor");
        assert_eq!(info.group_id, 9);
        assert!(!info.indoor, "outdoor bit set — must not report indoor");
    }

    #[test]
    fn a_probe_with_no_wmo_geometry_in_range_finds_no_area() {
        let mut fetch = |_fx: u16, _fy: u16| None;
        assert_eq!(
            cast_ray_area(&mut fetch, [0.0, 0.0, 15.0], [0.0, 0.0, -5.0]),
            None
        );
    }

    #[test]
    fn a_doodad_alone_never_produces_area_info() {
        // M2 (doodad) triangles carry no group semantics — a probe through one must still find
        // no WMO group, unlike `collision_ray` which would block on it.
        let (cx, cy, tris) = wall_cell(TriClass::M2);
        let mut fetch = |fx: u16, fy: u16| (fx == cx && fy == cy).then(|| tris.clone());
        let wall_y = tris[0].verts[0][1];
        let mid_x = tris[0].verts[0][0].midpoint(tris[0].verts[1][0]);
        assert_eq!(
            cast_ray_area(
                &mut fetch,
                [mid_x, wall_y - 20.0, 5.0],
                [mid_x, wall_y + 20.0, 5.0]
            ),
            None
        );
    }
}
