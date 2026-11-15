const VMAP_VERSION: u8 = 1;

// Maps with imported WMO instance slices; importer/manifests/world-profiles.json lists them.
pub const INSTANCE_SLICE_MAPS: &[u32] = &[36];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TriClass {
    Wmo { group_id: u32, mogp_flags: u32 },
    M2,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VmapTri {
    pub verts: [[f32; 3]; 3],
    pub class: TriClass,
}

// Triangle layout: class u8, group id u32, MOGP flags u32, nine f32 coordinates.
pub const TRI_BYTES: usize = 1 + 4 + 4 + 36;
// Header layout: version u8, triangle count u32 little-endian.
pub const HEADER_BYTES: usize = 5;

const CLASS_WMO: u8 = 0;
const CLASS_M2: u8 = 1;

#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    VersionMismatch(u8),
    BadClassTag(u8),
}

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

// M2 doodads block collision but never sight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RayFlavor {
    Los,
    Collision,
}

// Triangles span cells, so choose the globally nearest hit across every crossed cell.
pub fn cast_ray<T: AsRef<[VmapTri]>>(
    fetch: &mut impl FnMut(u16, u16) -> Option<T>,
    a: [f32; 3],
    b: [f32; 3],
    flavor: RayFlavor,
) -> Option<[f32; 3]> {
    let want =
        |tri: &VmapTri| flavor == RayFlavor::Collision || matches!(tri.class, TriClass::Wmo { .. });
    nearest_hit(fetch, a, b, want).map(|(t, _)| point_at(a, b, t))
}

pub fn cast_floor<T: AsRef<[VmapTri]>>(
    fetch: &mut impl FnMut(u16, u16) -> Option<T>,
    a: [f32; 3],
    b: [f32; 3],
) -> Option<[f32; 3]> {
    nearest_hit(fetch, a, b, |tri| {
        let normal = cross3(
            sub3(tri.verts[1], tri.verts[0]),
            sub3(tri.verts[2], tri.verts[0]),
        );
        supports_walking(normal, [0.0, 0.0, 1.0])
    })
    .map(|(t, _)| point_at(a, b, t))
}

pub fn supports_walking(normal: [f32; 3], up: [f32; 3]) -> bool {
    let lengths_squared = dot3(normal, normal) * dot3(up, up);
    let alignment = dot3(normal, up);
    lengths_squared > 0.0
        && lengths_squared.is_finite()
        && alignment * alignment >= 50.0f32.to_radians().cos().powi(2) * lengths_squared
}

fn nearest_hit<T: AsRef<[VmapTri]>>(
    fetch: &mut impl FnMut(u16, u16) -> Option<T>,
    a: [f32; 3],
    b: [f32; 3],
    want: impl Fn(&VmapTri) -> bool,
) -> Option<(f32, TriClass)> {
    let mut best: Option<(f32, TriClass)> = None;
    for (cx, cy) in crossed_cells(a, b) {
        let Some(tris) = fetch(cx, cy) else {
            continue;
        };
        for t in tris.as_ref() {
            if !want(t) {
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

// A found WMO group is indoors unless its MOGP flags include 0x8000.
pub const WMO_GROUP_OUTDOOR_FLAG: u32 = 0x8000;

pub fn group_is_indoor(mogp_flags: u32) -> bool {
    mogp_flags & WMO_GROUP_OUTDOOR_FLAG == 0
}

pub const AREA_PROBE_DOWN_YD: f32 = 10.0;

pub fn cast_ray_area<T: AsRef<[VmapTri]>>(
    fetch: &mut impl FnMut(u16, u16) -> Option<T>,
    a: [f32; 3],
    b: [f32; 3],
) -> Option<AreaInfo> {
    let (t, class) = nearest_hit(fetch, a, b, |tri| matches!(tri.class, TriClass::Wmo { .. }))?;
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AreaInfo {
    pub group_id: u32,
    pub indoor: bool,
    pub point: [f32; 3],
}

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
    // A segment crosses at most 2048 map cells; cap traversal for NaN or infinite inputs.
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

// Moller-Trumbore with both windings and segment-clamped t in 0..=1.
pub fn segment_tri_hit(a: [f32; 3], b: [f32; 3], tri: [[f32; 3]; 3]) -> Option<f32> {
    let dir = sub3(b, a);
    let e1 = sub3(tri[1], tri[0]);
    let e2 = sub3(tri[2], tri[0]);
    let pvec = cross3(dir, e2);
    let det = dot3(e1, pvec);
    if det.abs() < RAY_EPS {
        return None;
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
    fn walking_floor_keeps_ramps_but_rejects_steep_model_faces() {
        let class = TriClass::Wmo {
            group_id: 1,
            mogp_flags: 0,
        };
        for (surface_class, rise) in [
            (class, 0.0),
            (class, 1.0),
            (class, 3.0),
            (TriClass::M2, 0.0),
            (TriClass::M2, 1.0),
            (TriClass::M2, 3.0),
        ] {
            let surface = VmapTri {
                verts: [[0.0, 0.0, 10.0], [1.0, 0.0, 10.0 + rise], [0.0, 1.0, 10.0]],
                class: surface_class,
            };
            let mut fetch = |_, _| Some(vec![tri(5.0, class), surface]);
            let top = [0.25, 0.25, 12.0];
            let bottom = [0.25, 0.25, 0.0];
            let expected = if rise > 1.0 { 5.0 } else { 10.0 + rise * 0.25 };
            let floor = cast_floor(&mut fetch, top, bottom).unwrap();
            assert!(
                (floor[2] - expected).abs() < 0.001,
                "rise={rise}, floor={floor:?}"
            );
            let collision = cast_ray(&mut fetch, top, bottom, RayFlavor::Collision).unwrap();
            assert!((collision[2] - (10.0 + rise * 0.25)).abs() < 0.001);
        }
    }

    #[test]
    fn walking_floor_keeps_the_nearest_supporting_model_surface() {
        let class = TriClass::Wmo {
            group_id: 1,
            mogp_flags: 0,
        };
        let top = [0.25, 0.25, 12.0];
        let bottom = [0.25, 0.25, 0.0];
        let mut fetch = |_, _| Some(vec![tri(5.0, class), tri(10.0, TriClass::M2)]);
        assert!((cast_floor(&mut fetch, top, bottom).unwrap()[2] - 10.0).abs() < 0.001);
        assert!(cast_floor(&mut |_, _| None::<Vec<VmapTri>>, top, bottom).is_none());
    }

    #[test]
    fn walking_support_has_a_fifty_degree_limit_in_either_winding() {
        for (degrees, expected) in [(49.9_f32, true), (50.1, false)] {
            let angle = degrees.to_radians();
            for winding in [-1.0, 1.0] {
                let normal = [angle.sin() * winding, 0.0, angle.cos() * winding];
                assert_eq!(supports_walking(normal, [0.0, 0.0, 3.0]), expected);
            }
        }
        for invalid in [[0.0; 3], [f32::NAN; 3], [f32::INFINITY; 3]] {
            assert!(!supports_walking(invalid, [0.0, 0.0, 1.0]));
            assert!(!supports_walking([0.0, 0.0, 1.0], invalid));
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

    fn wall_cell(class: TriClass) -> (u16, u16, Vec<VmapTri>) {
        use crate::spatial::MAP_COORD_MAX;
        use crate::terrain::CELL_SIZE;
        let (cx, cy): (u16, u16) = (500, 500);
        let x_hi = MAP_COORD_MAX - cx as f32 * CELL_SIZE;
        let x_lo = x_hi - CELL_SIZE;
        let wall_y = MAP_COORD_MAX - (cy as f32 * CELL_SIZE + CELL_SIZE / 2.0);
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
        let start = [tris[0].verts[0][0], wall_y - 20.0, 5.0];
        let end = [tris[0].verts[0][0], wall_y - 10.0, 5.0];
        assert_eq!(cast_ray(&mut fetch, start, end, RayFlavor::Collision), None);
    }

    #[test]
    fn a_nearer_hit_in_a_later_cell_beats_a_farther_hit_duplicated_into_an_earlier_cell() {
        use crate::spatial::MAP_COORD_MAX;
        use crate::terrain::CELL_SIZE;
        let (cx_a, cy): (u16, u16) = (500, 500);
        let cx_b = cx_a + 1;
        let x_hi_a = MAP_COORD_MAX - cx_a as f32 * CELL_SIZE;
        let y_world = MAP_COORD_MAX - (cy as f32 * CELL_SIZE + CELL_SIZE / 2.0);

        fn wall_at(x: f32, y_world: f32, class: TriClass) -> VmapTri {
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
        let x_far = x_hi_a - CELL_SIZE - 20.0;
        let x_near = x_hi_a - CELL_SIZE - 5.0;
        let far_wall = wall_at(x_far, y_world, wmo(1));
        let near_wall = wall_at(x_near, y_world, wmo(2));

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

    #[test]
    fn a_probe_no_longer_than_the_area_budget_misses_a_distant_floor() {
        let (cx, cy, tris) = floor_cell(3, 0);
        let mut fetch = |fx: u16, fy: u16| (fx == cx && fy == cy).then(|| tris.clone());
        let x = tris[0].verts[0][0].midpoint(tris[0].verts[1][0]);
        let y = tris[0].verts[0][1].midpoint(tris[0].verts[2][1]);
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
        let mut fetch = |_fx: u16, _fy: u16| None::<Vec<VmapTri>>;
        assert_eq!(
            cast_ray_area(&mut fetch, [0.0, 0.0, 15.0], [0.0, 0.0, -5.0]),
            None
        );
    }

    #[test]
    fn a_doodad_alone_never_produces_area_info() {
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
