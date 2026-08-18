//! Nav-grid cell math + blob codec shared by the module's runtime queries (242) and the
//! importer's rasterizer (241) — same one-copy rule as `terrain.rs` (PR-9 review).
//!
//! One nav chunk per terrain MCNK cell (33.33 yd): a 64×64 walkability bitmask (0.52 yd nav
//! cells) and a 32×32 obstruction-height byte grid (1.04 yd) for line-of-sight. Cells with no
//! obstruction/slope/hole anywhere emit NO row — a missing chunk means "unobstructed here (or
//! un-imported)", and both readers fall back to today's straight-line behavior.
//!
//! Blob formats (fixed, versionless — schema changes reload the table):
//! - walk: 512 bytes; bit `ny*64+nx`, 1 = standable. Sub-indices count DOWN from the cell's
//!   high-coordinate corner, the same direction as `terrain::cell_index`.
//! - obs:  1024 bytes; `oy*32+ox`; 255 = no obstruction, else top-of-obstruction =
//!   `base_z + value * OBS_STEP` (`base_z` rides on the row). Height-above-BASE, not
//!   above-terrain, so a LoS ray tests against it with zero interpolation.
// Deliberate simplification: one obstruction column per 1 yd cell — can't express "clear under the
// bridge / two floors". Move to per-cell height LAYERS (or Recast polys, decision #8's fallback)
// when dungeons or bridges matter; Elwynn/Westfall exteriors don't.

use crate::spatial::MAP_COORD_MAX;
use crate::terrain::{interpolate, CELL_SIZE};
use crate::vmap::{TriClass, VmapTri};

/// Walkability sub-cells per terrain-cell axis (64 → 0.5208 yd).
pub const WALK_DIM: usize = 64;
/// Obstruction sub-cells per terrain-cell axis (32 → 1.0417 yd).
pub const OBS_DIM: usize = 32;
pub const WALK_BYTES: usize = WALK_DIM * WALK_DIM / 8; // 512
pub const OBS_BYTES: usize = OBS_DIM * OBS_DIM; // 1024
/// Obstruction height quantum in yards (u8 → 127 yd span above `base_z`, saturating).
pub const OBS_STEP: f32 = 0.5;
/// Obs byte meaning "no obstruction in this column".
pub const OBS_NONE: u8 = 255;
/// Walkable clearance band above ground a blocking triangle must intersect (yd): geometry
/// below `WALK_STEP_UP` is a step/floor you walk onto; above `WALK_HEIGHT` is a ceiling you
/// walk under.
pub const WALK_STEP_UP: f32 = 0.9;
pub const WALK_HEIGHT: f32 = 2.2;

/// World coord → sub-index 0..dim inside terrain cell `cell` on one axis, or None when the
/// coord isn't in that cell. Same DOWNWARD-counting convention as `terrain::cell_index`.
pub fn sub_index(coord: f32, cell: u16, dim: usize) -> Option<usize> {
    let local = (MAP_COORD_MAX - coord) - cell as f32 * CELL_SIZE;
    if !(0.0..CELL_SIZE).contains(&local) {
        return None;
    }
    Some(((local / CELL_SIZE) * dim as f32) as usize).map(|i| i.min(dim - 1))
}

/// Center world coord of sub-cell `i` (inverse of `sub_index`, for rasterizer sampling).
pub fn sub_center(cell: u16, i: usize, dim: usize) -> f32 {
    MAP_COORD_MAX - (cell as f32 * CELL_SIZE + (i as f32 + 0.5) * CELL_SIZE / dim as f32)
}

pub fn walk_get(walk: &[u8], nx: usize, ny: usize) -> bool {
    let bit = ny * WALK_DIM + nx;
    walk.get(bit / 8).is_some_and(|b| b & (1 << (bit % 8)) != 0)
}

pub fn walk_set(walk: &mut [u8], nx: usize, ny: usize, walkable: bool) {
    let bit = ny * WALK_DIM + nx;
    if walkable {
        walk[bit / 8] |= 1 << (bit % 8);
    } else {
        walk[bit / 8] &= !(1 << (bit % 8));
    }
}

/// Obstruction top-Z for a column, or None when clear.
pub fn obs_top(obs: &[u8], base_z: f32, ox: usize, oy: usize) -> Option<f32> {
    match obs.get(oy * OBS_DIM + ox) {
        None | Some(&OBS_NONE) => None,
        Some(&v) => Some(base_z + v as f32 * OBS_STEP),
    }
}

/// Raise a column's obstruction top to at least `top_z` (saturating at the u8 span).
pub fn obs_raise(obs: &mut [u8], base_z: f32, ox: usize, oy: usize, top_z: f32) {
    let v = (((top_z - base_z) / OBS_STEP).ceil().max(0.0) as u32).min(OBS_NONE as u32 - 1) as u8;
    let slot = &mut obs[oy * OBS_DIM + ox];
    if *slot == OBS_NONE || *slot < v {
        *slot = v;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::cell_index;

    #[test]
    fn sub_index_roundtrips_through_sub_center() {
        let cell = cell_index(-8913.0).unwrap();
        for dim in [WALK_DIM, OBS_DIM] {
            for i in [0usize, 1, dim / 2, dim - 1] {
                let c = sub_center(cell, i, dim);
                assert_eq!(sub_index(c, cell, dim), Some(i), "dim={dim} i={i}");
            }
        }
        // A coord in the NEXT cell over is rejected, not aliased into this one.
        assert_eq!(sub_index(-8913.0 - CELL_SIZE, cell, WALK_DIM), None);
    }

    #[test]
    fn walk_bits_set_and_clear_independently() {
        let mut walk = vec![0xFFu8; WALK_BYTES];
        walk_set(&mut walk, 5, 9, false);
        assert!(!walk_get(&walk, 5, 9));
        assert!(walk_get(&walk, 4, 9) && walk_get(&walk, 5, 8) && walk_get(&walk, 6, 10));
        walk_set(&mut walk, 5, 9, true);
        assert!(walk_get(&walk, 5, 9));
    }

    #[test]
    fn obs_raise_only_raises_and_saturates() {
        let mut obs = vec![OBS_NONE; OBS_BYTES];
        let base = 80.0;
        assert_eq!(obs_top(&obs, base, 3, 3), None);
        obs_raise(&mut obs, base, 3, 3, 86.0); // 6 yd wall
        assert!((obs_top(&obs, base, 3, 3).unwrap() - 86.0).abs() < OBS_STEP + 1e-3);
        obs_raise(&mut obs, base, 3, 3, 82.0); // lower geometry must not shrink the column
        assert!(obs_top(&obs, base, 3, 3).unwrap() >= 86.0 - OBS_STEP);
        obs_raise(&mut obs, base, 3, 3, base + 10_000.0); // saturates, no panic
        assert!(obs_top(&obs, base, 3, 3).is_some());
    }
}

// =============================================================================================
//  Runtime queries (work-item 242) — pure algorithms over fetched chunks. The chunk source is
//  a closure (module: one PK find per cell; tests: synthetic chunks) so ALL pathing/LoS logic
//  unit-tests here without a database. Missing chunk = "no obstacles known" (the 241 contract:
//  fully-clear cells emit no row), so every query degrades to today's straight-line behavior
//  off-slice.
// =============================================================================================

/// One fetched nav cell (the module clones the row's blobs; a few KB per crossed cell).
#[derive(Clone)]
pub struct NavCellData {
    pub base_z: f32,
    pub walk: Vec<u8>,
    pub obs: Vec<u8>,
}

/// Internal per-query memo so a leg/ray touching the same cell many times fetches it once.
struct Cache<'a, F: FnMut(u16, u16) -> Option<NavCellData>> {
    fetch: &'a mut F,
    seen: std::collections::HashMap<(u16, u16), Option<NavCellData>>,
}

impl<'a, F: FnMut(u16, u16) -> Option<NavCellData>> Cache<'a, F> {
    fn new(fetch: &'a mut F) -> Self {
        Self {
            fetch,
            seen: std::collections::HashMap::new(),
        }
    }
    fn get(&mut self, cx: u16, cy: u16) -> Option<&NavCellData> {
        let f = &mut self.fetch;
        self.seen
            .entry((cx, cy))
            .or_insert_with(|| f(cx, cy))
            .as_ref()
    }
    /// Walkable at world (x, y)? Missing chunk = walkable (no obstacles known).
    fn walkable(&mut self, x: f32, y: f32) -> bool {
        let (Some(cx), Some(cy)) = (crate::terrain::cell_index(x), crate::terrain::cell_index(y))
        else {
            return false; // off the map square entirely
        };
        match self.get(cx, cy) {
            None => true,
            Some(c) => {
                let (Some(nx), Some(ny)) = (sub_index(x, cx, WALK_DIM), sub_index(y, cy, WALK_DIM))
                else {
                    return true;
                };
                walk_get(&c.walk, nx, ny)
            }
        }
    }
}

/// Eye height added to both endpoints of a sight line (vanilla uses ~2 yd collision height).
pub const LOS_EYE_HEIGHT: f32 = 2.0;
/// Sight-line sample spacing in yards (half the obs-grid resolution — can't skip a column).
const LOS_STEP: f32 = 0.5;

/// The obs-grid column containing world (x, y), or None off-grid.
fn obs_col(x: f32, y: f32) -> Option<(u16, u16, usize, usize)> {
    let (cx, cy) = (
        crate::terrain::cell_index(x)?,
        crate::terrain::cell_index(y)?,
    );
    Some((
        cx,
        cy,
        sub_index(x, cx, OBS_DIM)?,
        sub_index(y, cy, OBS_DIM)?,
    ))
}

/// The raymarch behind `has_los` and `step_hit`: the first sample along a→b whose obstruction
/// column tops the ray, or None when clear. `exempt_start` skips samples in a's OWN obs column
/// — the committed-step gate needs it (a mover hugging a wall stands inside the blob's
/// conservative margin, and counting its own column as blocked would hold it in place forever —
/// the same live find that exempts the start cell in `line_walkable`), while sight checks keep
/// the strict test.
// Deliberate simplification: parametric sampling, not an exact DDA — at 0.5 yd steps over a
// 1.04 yd obs grid a column cannot be stepped over; move to exact cell walking when someone
// measures the difference.
fn los_hit(
    fetch: &mut impl FnMut(u16, u16) -> Option<NavCellData>,
    a: (f32, f32, f32),
    b: (f32, f32, f32),
    exempt_start: bool,
) -> Option<(f32, f32)> {
    let mut cache = Cache::new(fetch);
    let start_col = if exempt_start {
        obs_col(a.0, a.1)
    } else {
        None
    };
    let (dx, dy, dz) = (b.0 - a.0, b.1 - a.1, b.2 - a.2);
    let len = (dx * dx + dy * dy).sqrt();
    let steps = (len / LOS_STEP).ceil().max(1.0) as u32;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let (x, y) = (a.0 + dx * t, a.1 + dy * t);
        let ray_z = a.2 + LOS_EYE_HEIGHT + (dz * t);
        let Some((cx, cy, ox, oy)) = obs_col(x, y) else {
            continue;
        };
        if start_col == Some((cx, cy, ox, oy)) {
            continue;
        }
        let Some(cell) = cache.get(cx, cy) else {
            continue;
        };
        if let Some(top) = obs_top(&cell.obs, cell.base_z, ox, oy) {
            if ray_z < top {
                return Some((x, y));
            }
        }
    }
    None
}

/// Line of sight from a to b: sample the segment every `LOS_STEP` and compare the ray's
/// interpolated Z (+eye height at both ends) against the obstruction column tops.
pub fn has_los(
    fetch: &mut impl FnMut(u16, u16) -> Option<NavCellData>,
    a: (f32, f32, f32),
    b: (f32, f32, f32),
) -> bool {
    los_hit(fetch, a, b, false).is_none()
}

/// Returns the first blocked sample so a committed step can stop short of it.
/// The mover's own obstruction column is exempt; `None` means the step is clear.
pub fn step_hit(
    fetch: &mut impl FnMut(u16, u16) -> Option<NavCellData>,
    a: (f32, f32, f32),
    b: (f32, f32, f32),
) -> Option<(f32, f32)> {
    los_hit(fetch, a, b, true)
}

/// Nav-grid resolution in yards (one walk sub-cell).
pub const NAV_RES: f32 = CELL_SIZE / WALK_DIM as f32;

/// Global walk-grid coordinate: `cell_index * 64 + sub_index` (counts DOWN from
/// +MAP_COORD_MAX like everything else). u32 range 0..65536.
fn grid_coord(coord: f32) -> Option<u32> {
    let c = (MAP_COORD_MAX - coord) / NAV_RES;
    if c < 0.0 || c >= (1024 * WALK_DIM) as f32 {
        return None;
    }
    Some(c as u32)
}

fn grid_to_world(g: u32) -> f32 {
    MAP_COORD_MAX - (g as f32 + 0.5) * NAV_RES
}

fn grid_walkable(
    cache: &mut Cache<impl FnMut(u16, u16) -> Option<NavCellData>>,
    gx: u32,
    gy: u32,
) -> bool {
    cache.walkable(grid_to_world(gx), grid_to_world(gy))
}

/// Straight segment fully walkable? (the A* fast path + the string-pulling test.) Samples
/// inside the START's own nav cell are exempt: a chaser hugging an obstacle stands in the
/// blob's conservative margin (movement isn't walkability-gated), and counting its own cell
/// as blocked failed EVERY sightline — string-pulling collapsed to per-cell micro-steps and
/// the mob visibly stuttered each tick (live find, 2026-07-10).
fn line_walkable(
    cache: &mut Cache<impl FnMut(u16, u16) -> Option<NavCellData>>,
    from: (f32, f32),
    to: (f32, f32),
) -> bool {
    let start_cell = (grid_coord(from.0), grid_coord(from.1));
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let steps = ((dx * dx + dy * dy).sqrt() / (NAV_RES * 0.5))
        .ceil()
        .max(1.0) as u32;
    (0..=steps).all(|i| {
        let t = i as f32 / steps as f32;
        let (x, y) = (from.0 + dx * t, from.1 + dy * t);
        if (grid_coord(x), grid_coord(y)) == start_cell {
            return true;
        }
        cache.walkable(x, y)
    })
}

/// Short-leg grid A* with string-pulling. Returns world waypoints from AFTER `from` up to and
/// including `to`, or None when unreachable within `max_expansions`. The straight-line fast
/// path returns `[to]` with ZERO expansions — an open-field chase costs one line test.
/// Callers keep legs SHORT (the 500 ms tick's chase legs); long travel stays on the waypoint
/// graph (work-item 150).
pub fn find_leg(
    fetch: &mut impl FnMut(u16, u16) -> Option<NavCellData>,
    from: (f32, f32),
    to: (f32, f32),
    max_expansions: u32,
) -> Option<Vec<(f32, f32)>> {
    find_leg_ex(fetch, from, to, max_expansions).map(|(path, _, _)| path)
}

/// `find_leg` + the expansion count + completeness (exposed for tests and the 244 benchmark).
/// When the expansion budget exhausts before reaching the goal, returns the BEST-EFFORT path —
/// to the explored node nearest the goal (complete=false) — so a per-tick chase leg still makes
/// real progress around large obstacles and re-plans next tick, instead of falling back to a
/// straight wall-grind. Returns None only when the goal is unwalkable, off-grid, or no progress
/// at all is possible.
// The return tuple is the pathfinder's full result (leg, expansions used, whether it reached the goal).
#[allow(clippy::type_complexity)]
pub fn find_leg_ex(
    fetch: &mut impl FnMut(u16, u16) -> Option<NavCellData>,
    from: (f32, f32),
    to: (f32, f32),
    max_expansions: u32,
) -> Option<(Vec<(f32, f32)>, u32, bool)> {
    let mut cache = Cache::new(fetch);
    if line_walkable(&mut cache, from, to) {
        return Some((vec![to], 0, true));
    }
    let (sx, sy) = (grid_coord(from.0)?, grid_coord(from.1)?);
    let (tx, ty) = (grid_coord(to.0)?, grid_coord(to.1)?);
    if !grid_walkable(&mut cache, tx, ty) {
        return None; // goal itself is inside geometry — let the caller fall back
    }
    // A* with octile heuristic, integer costs (10 straight / 14 diagonal), corner-cut guard.
    use std::cmp::Reverse;
    use std::collections::{BinaryHeap, HashMap};
    let h = |x: u32, y: u32| {
        let (dx, dy) = (
            (x as i64 - tx as i64).unsigned_abs(),
            (y as i64 - ty as i64).unsigned_abs(),
        );
        10 * dx.max(dy) + 4 * dx.min(dy)
    };
    let mut open: BinaryHeap<Reverse<(u64, u32, u32)>> = BinaryHeap::new();
    let mut g_cost: HashMap<(u32, u32), u64> = HashMap::new();
    let mut came: HashMap<(u32, u32), (u32, u32)> = HashMap::new();
    g_cost.insert((sx, sy), 0);
    open.push(Reverse((h(sx, sy), sx, sy)));
    let mut expanded = 0u32;
    let mut found = false;
    let mut best = ((sx, sy), h(sx, sy)); // nearest-approach node for the partial fallback
    while let Some(Reverse((_, x, y))) = open.pop() {
        if (x, y) == (tx, ty) {
            found = true;
            break;
        }
        expanded += 1;
        if expanded > max_expansions {
            break; // budget exhausted — fall through to the best-effort partial path
        }
        let hxy = h(x, y);
        if hxy < best.1 {
            best = ((x, y), hxy);
        }
        let g0 = g_cost[&(x, y)];
        for (dx, dy) in [
            (1i64, 0i64),
            (-1, 0),
            (0, 1),
            (0, -1),
            (1, 1),
            (1, -1),
            (-1, 1),
            (-1, -1),
        ] {
            let (nx, ny) = (x as i64 + dx, y as i64 + dy);
            if nx < 0 || ny < 0 {
                continue;
            }
            let (nx, ny) = (nx as u32, ny as u32);
            if !grid_walkable(&mut cache, nx, ny) {
                continue;
            }
            // No cutting corners: a diagonal move needs both orthogonal neighbours open.
            if dx != 0
                && dy != 0
                && (!grid_walkable(&mut cache, nx, y) || !grid_walkable(&mut cache, x, ny))
            {
                continue;
            }
            let step = if dx != 0 && dy != 0 { 14 } else { 10 };
            let ng = g0 + step;
            if g_cost.get(&(nx, ny)).is_none_or(|&old| ng < old) {
                g_cost.insert((nx, ny), ng);
                came.insert((nx, ny), (x, y));
                open.push(Reverse((ng + h(nx, ny), nx, ny)));
            }
        }
    }
    let end = if found {
        (tx, ty)
    } else {
        if best.0 == (sx, sy) {
            return None; // zero progress possible (walled-in start) — caller falls back
        }
        best.0
    };
    // Reconstruct, then string-pull: keep a waypoint only where the direct segment breaks.
    let mut cells = vec![end];
    while let Some(&prev) = came.get(cells.last().unwrap()) {
        cells.push(prev);
        if prev == (sx, sy) {
            break;
        }
    }
    cells.reverse();
    let pts: Vec<(f32, f32)> = cells
        .iter()
        .map(|&(x, y)| (grid_to_world(x), grid_to_world(y)))
        .collect();
    let mut path: Vec<(f32, f32)> = Vec::new();
    let mut anchor = from;
    let mut i = 0;
    while i + 1 < pts.len() {
        // Furthest point still directly reachable from the anchor.
        let mut j = pts.len() - 1;
        while j > i + 1 && !line_walkable(&mut cache, anchor, pts[j]) {
            j -= 1;
        }
        path.push(pts[j]);
        anchor = pts[j];
        i = j;
    }
    // Land exactly on the requested destination (the goal cell center is ≤0.4 yd off) — only
    // for a COMPLETE path; a partial one ends at the nearest-approach node by design.
    if found && line_walkable(&mut cache, anchor, to) {
        path.pop();
        path.push(to);
    }
    Some((path, expanded, found))
}

#[cfg(test)]
mod runtime_tests {
    use super::*;
    use crate::terrain::cell_index;

    /// One synthetic chunk at the Northshire cell: a full-height wall along nx=32 (obs ox=16)
    /// with a 4-sub-cell doorway at ny 30..34 (obs oy 15..17), plus a sealed 4-wall pocket in
    /// the corner (nx 4..12, ny 4..12 ring). Every other cell in the world is "missing" (all
    /// clear) — exactly the 241 skip-all-clear contract.
    fn walled_cell() -> ((u16, u16), NavCellData) {
        let (cx, cy) = (cell_index(-8913.0).unwrap(), cell_index(-184.0).unwrap());
        let mut walk = vec![0xFFu8; WALK_BYTES];
        let mut obs = vec![OBS_NONE; OBS_BYTES];
        let base = 80.0;
        for ny in 0..WALK_DIM {
            if !(30..34).contains(&ny) {
                walk_set(&mut walk, 32, ny, false);
            }
        }
        for oy in 0..OBS_DIM {
            if !(15..17).contains(&oy) {
                obs_raise(&mut obs, base, 16, oy, base + 20.0);
            }
        }
        // Sealed pocket: ring walls, hollow inside.
        for i in 4..=12 {
            for (nx, ny) in [(i, 4), (i, 12), (4, i), (12, i)] {
                walk_set(&mut walk, nx, ny, false);
            }
        }
        (
            (cx, cy),
            NavCellData {
                base_z: base,
                walk,
                obs,
            },
        )
    }

    fn fetcher() -> impl FnMut(u16, u16) -> Option<NavCellData> {
        let ((cx, cy), data) = walled_cell();
        move |x, y| (x == cx && y == cy).then(|| data.clone())
    }

    /// World coords of a sub-cell in the walled cell.
    fn at(nx: usize, ny: usize) -> (f32, f32) {
        let ((cx, cy), _) = walled_cell();
        (sub_center(cx, nx, WALK_DIM), sub_center(cy, ny, WALK_DIM))
    }

    #[test]
    fn los_blocked_by_wall_clear_through_doorway() {
        let z = 80.0;
        // Across the wall at ny=50 (oy=25): blocked.
        let (ax, ay) = at(10, 50);
        let (bx, by) = at(54, 50);
        assert!(!has_los(&mut fetcher(), (ax, ay, z), (bx, by, z)));
        // Straight through the doorway line (ny=31, oy=15): clear.
        let (ax, ay) = at(10, 31);
        let (bx, by) = at(54, 31);
        assert!(has_los(&mut fetcher(), (ax, ay, z), (bx, by, z)));
        // Flying high over the wall (ray z above base+20): clear.
        let (ax, ay) = at(10, 50);
        let (bx, by) = at(54, 50);
        assert!(has_los(
            &mut fetcher(),
            (ax, ay, z + 30.0),
            (bx, by, z + 30.0)
        ));
    }

    #[test]
    fn find_leg_routes_through_the_doorway() {
        // Start and goal on opposite sides of the wall, far from the door.
        let from = at(10, 50);
        let to = at(54, 50);
        let (path, expanded, complete) =
            find_leg_ex(&mut fetcher(), from, to, 20_000).expect("reachable");
        assert!(complete);
        assert!(expanded > 0, "wall forces real A*, not the fast path");
        assert!(path
            .last()
            .is_some_and(|&p| (p.0 - to.0).abs() < 1e-3 && (p.1 - to.1).abs() < 1e-3));
        // Every string-pulled waypoint segment must itself be walkable — re-verify by sampling.
        let mut fetch = fetcher();
        let mut cache_check = |a: (f32, f32), b: (f32, f32)| {
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let steps = ((dx * dx + dy * dy).sqrt() / (NAV_RES * 0.5))
                .ceil()
                .max(1.0) as u32;
            (0..=steps).all(|i| {
                let t = i as f32 / steps as f32;
                let (x, y) = (a.0 + dx * t, a.1 + dy * t);
                let (cx, cy) = (cell_index(x).unwrap(), cell_index(y).unwrap());
                match fetch(cx, cy) {
                    None => true,
                    Some(c) => walk_get(
                        &c.walk,
                        sub_index(x, cx, WALK_DIM).unwrap(),
                        sub_index(y, cy, WALK_DIM).unwrap(),
                    ),
                }
            })
        };
        let mut prev = from;
        for &p in &path {
            assert!(
                cache_check(prev, p),
                "segment {prev:?} -> {p:?} crosses the wall"
            );
            prev = p;
        }
    }

    #[test]
    fn open_field_fast_path_takes_zero_expansions() {
        // Both points in the clear half of the cell — direct line, no A*.
        let from = at(40, 10);
        let to = at(60, 20);
        let (path, expanded, complete) =
            find_leg_ex(&mut fetcher(), from, to, 20_000).expect("open field");
        assert_eq!(expanded, 0);
        assert_eq!(path.len(), 1);
        assert!(complete);
    }

    #[test]
    fn start_inside_a_blocked_margin_cell_still_fast_paths() {
        // A chaser standing IN an unwalkable margin cell (movement isn't walkability-gated)
        // must still see the open field ahead — its own cell is exempt from the line test.
        let ((cx, cy), _) = walled_cell();
        let mut fetch = fetcher();
        // Stand exactly on the wall line (nx=32, ny=50 — unwalkable), aim into the open.
        let from = (sub_center(cx, 32, WALK_DIM), sub_center(cy, 50, WALK_DIM));
        let to = at(40, 50);
        let (path, expanded, complete) =
            find_leg_ex(&mut fetch, from, to, 20_000).expect("must escape the margin");
        assert_eq!(expanded, 0, "own-cell exemption keeps the fast path");
        assert_eq!(path.len(), 1);
        assert!(complete);
    }

    #[test]
    fn step_hit_stops_a_wall_crossing_step_and_clears_the_doorway() {
        let z = 80.0;
        // Step across the wall at ny=50 (oy=25): the gate ray hits, and the hit sample sits ON
        // the wall's obs column (ox=16) — the gated mover truncates short instead of crossing.
        let ((cx, _), _) = walled_cell();
        let wall_x = sub_center(cx, 16, OBS_DIM);
        let (ax, ay) = at(10, 50);
        let (bx, by) = at(54, 50);
        let (hx, _) =
            step_hit(&mut fetcher(), (ax, ay, z), (bx, by, z)).expect("wall blocks the step");
        assert!(
            (hx - wall_x).abs() <= 0.8,
            "hit sample {hx:.2} not at the wall column {wall_x:.2}"
        );
        // Straight through the doorway line (ny=31, oy=15): clear, same as `has_los`.
        let (ax, ay) = at(10, 31);
        let (bx, by) = at(54, 31);
        assert!(step_hit(&mut fetcher(), (ax, ay, z), (bx, by, z)).is_none());
    }

    #[test]
    fn step_hit_exempts_the_movers_own_obs_column() {
        // A mover standing INSIDE the wall's obs column (the blob's conservative margin — movement
        // isn't walkability-gated) must be able to step back into the open: its own column never
        // holds it (the same live find as `line_walkable`'s start-cell exemption)…
        let z = 80.0;
        let ((cx, cy), _) = walled_cell();
        let from = (sub_center(cx, 16, OBS_DIM), sub_center(cy, 25, OBS_DIM));
        let to = at(10, 50);
        assert!(step_hit(&mut fetcher(), (from.0, from.1, z), (to.0, to.1, z)).is_none());
        // `has_los` keeps the strict test so sight semantics remain unchanged.
        assert!(!has_los(
            &mut fetcher(),
            (from.0, from.1, z),
            (to.0, to.1, z)
        ));
    }

    #[test]
    fn no_path_fallback_straight_step_is_caught_by_the_gate() {
        // A goal inside the wall makes `find_leg` return `None`.
        // to a straight step AT the goal — the step-gate ray must hit at the wall column so the
        // committed move truncates short of it instead of walking into geometry.
        let z = 80.0;
        let ((cx, _), _) = walled_cell();
        let wall_x = sub_center(cx, 16, OBS_DIM);
        let from = at(10, 50);
        let goal = at(32, 50); // ON the wall line — unwalkable by construction
        assert!(find_leg(&mut fetcher(), from, goal, 20_000).is_none());
        let (hx, _) = step_hit(&mut fetcher(), (from.0, from.1, z), (goal.0, goal.1, z))
            .expect("the straight fallback crosses the wall column");
        assert!(
            (hx - wall_x).abs() <= 0.8 && hx < from.0,
            "hit sample {hx:.2} not at the wall column {wall_x:.2}"
        );
    }

    #[test]
    fn sealed_pocket_yields_partial_progress_and_walls_yield_none() {
        let from = at(50, 8); // outside the pocket
        let to = at(8, 8); // inside the sealed ring
                           // Unreachable but walkable goal: budget exhausts → best-effort PARTIAL path toward the
                           // pocket (the creature bumps the wall and re-plans, like a straight-line mob would).
        let (path, _, complete) =
            find_leg_ex(&mut fetcher(), from, to, 2_000).expect("partial progress");
        assert!(!complete);
        assert!(!path.is_empty());
        // A wall-interior goal returns None immediately (goal cell unwalkable).
        assert!(find_leg(&mut fetcher(), from, at(32, 50), 20_000).is_none());
    }
}

// =============================================================================================
//  Derivation: collision triangles to one cell's blobs. This is the only rasterization policy.
//  The importer's `--nav` mode and the module's coverage derivation both call `derive_cell`, so
//  a margin or band change cannot drift between them. Terrain-only inputs (slope, holes) stay
//  with their owner and fold in through `merge_cells`.
// =============================================================================================

/// Inflation margin for the point-in-triangle test (yd). Slightly over half a walk-cell diagonal,
/// so a thin vertical wall whose 2D projection is a sliver still hits the centers of the cells its
/// line crosses. Errs toward over-blocking by less than one cell.
const RASTER_MARGIN: f32 = 0.35;

/// Agent-body inset for walk cells. The obstruction and line-of-sight grid keeps the true
/// footprint.
const AGENT_RADIUS: f32 = 0.5;

/// The walk-grid rasterization margin: sliver-catching plus the body inset. Public because a
/// caller binning triangles into cells must inflate its cell range by the same amount, or a
/// triangle hugging a border never reaches the neighbour cell its inset footprint spills into.
pub const WALK_MARGIN: f32 = RASTER_MARGIN + AGENT_RADIUS;

/// Gap tolerance when fusing a column's z-intervals (yd): stacked wall bands abut without
/// overlapping exactly.
const OBS_GAP: f32 = 0.75;

/// One world-space collision triangle with its AABB and plane. `z_at` is the exact-rasterization
/// core: the triangle's z-interval over a 2D point, or None when the point falls outside the
/// inflated footprint. A coarse M2 bounding triangle spanning trunk to canopy therefore blocks
/// only near the trunk, where the plane-z is low, and leaves the space under the canopy walkable.
/// Stamping the whole AABB instead blocks the full canopy width.
struct WorldTri {
    lo: [f32; 3],
    hi: [f32; 3],
    /// WMO geometry participates in the obstruction grid. M2 doodads never do.
    is_wmo: bool,
    v: [[f32; 3]; 3],
    /// Plane normal and d (n·p = d). Near-vertical planes fall back to the full z-range.
    n: [f32; 3],
    d: f32,
}

impl WorldTri {
    fn new(t: &VmapTri) -> Self {
        let v = t.verts;
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in &v {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
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
            is_wmo: matches!(t.class, TriClass::Wmo { .. }),
            v,
            n,
            d,
        }
    }

    /// The triangle's z-interval over 2D point (x, y), testing inside-ness with a `margin`-yd
    /// outward inflation by signed edge distance. None means outside the inflated footprint.
    fn z_at(&self, x: f32, y: f32, margin: f32) -> Option<(f32, f32)> {
        // 2D signed edge distances (positive = same side as the third vertex).
        for i in 0..3 {
            let (a, b, c) = (self.v[i], self.v[(i + 1) % 3], self.v[(i + 2) % 3]);
            let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
            let len = (ex * ex + ey * ey).sqrt();
            if len < 1e-6 {
                continue; // degenerate edge in 2D (vertical tri seen edge-on): skip this test
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
        // Steep plane (wall or fence side, more than 60° from horizontal): the plane-z is
        // ill-conditioned over the projected sliver and clamps to an arbitrary end of the
        // z-range. A fence side tilted about 1° clamps to the bottom, below the standing band,
        // which leaves the fence walkable. Use the full z-span instead. Anything this steep is
        // unwalkable surface anyway, since the terrain slope gate is 50°.
        let n_len = (self.n[0] * self.n[0] + self.n[1] * self.n[1] + self.n[2] * self.n[2]).sqrt();
        if self.n[2].abs() < 0.5 * n_len {
            return Some((self.lo[2], self.hi[2]));
        }
        let z =
            ((self.d - self.n[0] * x - self.n[1] * y) / self.n[2]).clamp(self.lo[2], self.hi[2]);
        Some((z, z))
    }
}

/// Sub-index range covering world span `[lo_c, hi_c]`, clamped to `cell_i`. A high coordinate
/// gives the low index. None when the span misses the cell entirely.
///
/// Clamps in the cell's local offset space instead of re-testing each end with `sub_index`. At
/// ±17k yd an f32 boundary coordinate rounds to either side of the cell edge, and a `None` there
/// drops the whole triangle from the cell, so a blocker reaching past a cell border goes
/// unrasterized at random.
fn clamp_axis(lo_c: f32, hi_c: f32, cell_i: u16, dim: usize) -> Option<(usize, usize)> {
    let local = |coord: f32| (MAP_COORD_MAX - coord) - cell_i as f32 * CELL_SIZE;
    // High coord → low local offset, so the span's ends swap.
    let (lo_local, hi_local) = (local(hi_c), local(lo_c));
    if hi_local <= 0.0 || lo_local >= CELL_SIZE {
        return None;
    }
    let index =
        |l: f32| (((l.clamp(0.0, CELL_SIZE) / CELL_SIZE) * dim as f32) as usize).min(dim - 1);
    Some((index(lo_local), index(hi_local)))
}

/// Derive one cell's blobs from the collision triangles binned to it: every triangle whose AABB
/// touches the cell, inflated by `WALK_MARGIN`. Anything outside the cell is clamped away here.
///
/// `heights` is the cell's 145-value MCNK height grid when a terrain chunk exists. The standing
/// band references terrain ground only, because a model deck is a blocker and never a floor to
/// stand on, so without heights the whole cell falls back to `base_z`, the lowest triangle vertex.
///
/// Returns None when nothing in the cell blocks. A fully-clear cell emits no row, and both readers
/// treat a missing cell as "no obstacles known".
pub fn derive_cell(
    cell_x: u16,
    cell_y: u16,
    heights: Option<&[f32]>,
    tris: &[VmapTri],
) -> Option<NavCellData> {
    if tris.is_empty() {
        return None;
    }
    let base_z = match heights {
        Some(h) if h.len() == 145 => h.iter().copied().fold(f32::MAX, f32::min),
        _ => tris
            .iter()
            .flat_map(|t| t.verts.iter())
            .map(|v| v[2])
            .fold(f32::MAX, f32::min),
    };
    let ground = |x: f32, y: f32| {
        heights
            .and_then(|h| interpolate(h, cell_x, cell_y, x, y))
            .unwrap_or(base_z)
    };

    let mut walk = vec![0xFFu8; WALK_BYTES];
    let mut obs = vec![OBS_NONE; OBS_BYTES];
    let mut dirty = false;
    let mut col_ivals: Vec<Vec<(f32, f32)>> = vec![Vec::new(); OBS_BYTES];

    for t in tris.iter().map(WorldTri::new) {
        // Walkability (all geometry): block a nav cell only when the triangle passes through the
        // standing band above that cell's ground. Footprint and window are inflated by
        // `WALK_MARGIN`, so cells within a body radius of geometry rasterize blocked and the
        // center line that `find_leg` and `nav_step` walk keeps the body clear of wall corners.
        if let (Some((nx0, nx1)), Some((ny0, ny1))) = (
            clamp_axis(
                t.lo[0] - WALK_MARGIN,
                t.hi[0] + WALK_MARGIN,
                cell_x,
                WALK_DIM,
            ),
            clamp_axis(
                t.lo[1] - WALK_MARGIN,
                t.hi[1] + WALK_MARGIN,
                cell_y,
                WALK_DIM,
            ),
        ) {
            for ny in ny0..=ny1 {
                for nx in nx0..=nx1 {
                    let (x, y) = (
                        sub_center(cell_x, nx, WALK_DIM),
                        sub_center(cell_y, ny, WALK_DIM),
                    );
                    let Some((z_lo, z_hi)) = t.z_at(x, y, WALK_MARGIN) else {
                        continue;
                    };
                    let g = ground(x, y);
                    if z_lo < g + WALK_HEIGHT && z_hi > g + WALK_STEP_UP {
                        walk_set(&mut walk, nx, ny, false);
                        dirty = true;
                    }
                }
            }
        }
        // Obstruction and line of sight: WMO geometry only. Vanilla doodads such as trees block
        // movement but never sight lines, so spells and aggro reach through a forest. Accumulate
        // the z-intervals per column here; the rooted decision happens per column after merging,
        // below. Deciding per triangle keeps only a wall's bottom band, because a wall is stacked
        // triangle bands, and collapses obs tops to about 2 yd, which lets eye-height rays graze
        // over the wall.
        if !t.is_wmo {
            continue;
        }
        if let (Some((ox0, ox1)), Some((oy0, oy1))) = (
            clamp_axis(t.lo[0], t.hi[0], cell_x, OBS_DIM),
            clamp_axis(t.lo[1], t.hi[1], cell_y, OBS_DIM),
        ) {
            for oy in oy0..=oy1 {
                for ox in ox0..=ox1 {
                    let (x, y) = (
                        sub_center(cell_x, ox, OBS_DIM),
                        sub_center(cell_y, oy, OBS_DIM),
                    );
                    let Some((z_lo, z_hi)) = t.z_at(x, y, RASTER_MARGIN) else {
                        continue;
                    };
                    col_ivals[oy * OBS_DIM + ox].push((z_lo, z_hi));
                }
            }
        }
    }

    // Per-column obs: merge the accumulated z-intervals with `OBS_GAP` tolerance, then keep only
    // the merged run that reaches down into the head-height band above this column's ground.
    // Stacked wall bands fuse ground to top, so a full wall height blocks over-the-roof sight. A
    // floating eave or roof run stays a separate high interval and is dropped, so standing under
    // an overhang does not blank line of sight.
    //
    // Simplification: one top per obs column, with no (bottom, top) pair, so archways and bridges
    // read as walls. Store both bytes when a real case needs to see through an opening.
    for oy in 0..OBS_DIM {
        for ox in 0..OBS_DIM {
            let ivals = &mut col_ivals[oy * OBS_DIM + ox];
            if ivals.is_empty() {
                continue;
            }
            let (x, y) = (
                sub_center(cell_x, ox, OBS_DIM),
                sub_center(cell_y, oy, OBS_DIM),
            );
            let g = ground(x, y);
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
                if lo <= run_hi + OBS_GAP {
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

    dirty.then_some(NavCellData { base_z, walk, obs })
}

/// Merge two cells covering the same terrain cell: `walk` is the bitwise AND, since either source
/// may block, and `obs` keeps the taller column. Both blobs rebase onto the lower `base_z`, so no
/// column underflows when the two disagree about the cell's floor.
pub fn merge_cells(a: &NavCellData, b: &NavCellData) -> NavCellData {
    let base_z = a.base_z.min(b.base_z);
    let walk: Vec<u8> = a.walk.iter().zip(&b.walk).map(|(x, y)| x & y).collect();
    let mut obs = vec![OBS_NONE; OBS_BYTES];
    for src in [a, b] {
        for oy in 0..OBS_DIM {
            for ox in 0..OBS_DIM {
                if let Some(top) = obs_top(&src.obs, src.base_z, ox, oy) {
                    obs_raise(&mut obs, base_z, ox, oy, top);
                }
            }
        }
    }
    NavCellData { base_z, walk, obs }
}

#[cfg(test)]
mod derive_tests {
    use super::*;
    use crate::terrain::cell_index;
    use crate::vmap::{cast_ray, RayFlavor};

    const GROUND_Z: f32 = 80.0;

    fn test_cell() -> (u16, u16) {
        (cell_index(-8913.0).unwrap(), cell_index(-184.0).unwrap())
    }

    fn flat_heights() -> Vec<f32> {
        vec![GROUND_Z; 145]
    }

    fn wmo() -> TriClass {
        TriClass::Wmo {
            group_id: 0,
            mogp_flags: 0,
        }
    }

    /// World center of a walk sub-cell in the test cell.
    fn at(nx: usize, ny: usize) -> (f32, f32) {
        let (cx, cy) = test_cell();
        (sub_center(cx, nx, WALK_DIM), sub_center(cy, ny, WALK_DIM))
    }

    /// Two triangles forming a vertical wall quad along the `nx` line, spanning walk rows
    /// `ny0..=ny1` and rising `height` yd off the ground.
    fn wall(nx: usize, ny0: usize, ny1: usize, height: f32, class: TriClass) -> Vec<VmapTri> {
        let (x, y0) = at(nx, ny0);
        let (_, y1) = at(nx, ny1);
        let (lo, hi) = (GROUND_Z - 1.0, GROUND_Z + height);
        vec![
            VmapTri {
                verts: [[x, y0, lo], [x, y1, lo], [x, y1, hi]],
                class,
            },
            VmapTri {
                verts: [[x, y0, lo], [x, y1, hi], [x, y0, hi]],
                class,
            },
        ]
    }

    /// One horizontal triangle at `z` that covers the whole test cell (vertices well outside it,
    /// so every sub-cell center falls inside the footprint).
    fn slab(z: f32, class: TriClass) -> VmapTri {
        let (cx, cy) = test_cell();
        let (hx, hy) = (
            sub_center(cx, 0, WALK_DIM) + 50.0,
            sub_center(cy, 0, WALK_DIM) + 50.0,
        );
        VmapTri {
            verts: [[hx, hy, z], [hx - 200.0, hy, z], [hx, hy - 200.0, z]],
            class,
        }
    }

    /// An all-clear terrain cell (what a slope/hole-free chunk contributes to a merge).
    fn open_terrain() -> NavCellData {
        NavCellData {
            base_z: GROUND_Z,
            walk: vec![0xFFu8; WALK_BYTES],
            obs: vec![OBS_NONE; OBS_BYTES],
        }
    }

    fn cell_fetcher(data: Option<NavCellData>) -> impl FnMut(u16, u16) -> Option<NavCellData> {
        let (cx, cy) = test_cell();
        move |x, y| (x == cx && y == cy).then(|| data.clone()).flatten()
    }

    #[test]
    fn a_wmo_wall_detours_find_leg_only_once_its_coverage_is_merged_in() {
        let tris = wall(32, 0, 40, 12.0, wmo());
        let (cx, cy) = test_cell();
        let coverage =
            derive_cell(cx, cy, Some(&flat_heights()), &tris).expect("the wall blocks the cell");
        let merged = merge_cells(&open_terrain(), &coverage);

        // Opposite sides of the wall, level with its far end so a detour exists.
        let (from, to) = (at(10, 20), at(54, 20));
        let (detour, expanded, complete) =
            find_leg_ex(&mut cell_fetcher(Some(merged)), from, to, 20_000).expect("reachable");
        assert!(complete && expanded > 0, "the wall must force real A*");
        assert!(detour.len() > 1, "a detour needs a corner waypoint");

        // Terrain alone knows nothing about the wall: the same leg is one straight segment.
        let (straight, expanded, _) =
            find_leg_ex(&mut cell_fetcher(Some(open_terrain())), from, to, 20_000)
                .expect("open field");
        assert_eq!((straight.len(), expanded), (1, 0));

        // The exact ray over the same triangles agrees the straight segment is blocked.
        let mut tri_fetch = |x: u16, y: u16| (x == cx && y == cy).then(|| tris.clone());
        assert!(cast_ray(
            &mut tri_fetch,
            [from.0, from.1, GROUND_Z + LOS_EYE_HEIGHT],
            [to.0, to.1, GROUND_Z + LOS_EYE_HEIGHT],
            RayFlavor::Los,
        )
        .is_some());
    }

    #[test]
    fn an_m2_doodad_blocks_walking_but_never_sight() {
        let (cx, cy) = test_cell();
        let tris = wall(32, 20, 30, 6.0, TriClass::M2);
        let cell = derive_cell(cx, cy, Some(&flat_heights()), &tris).expect("a doodad blocks");
        assert!(!walk_get(&cell.walk, 32, 25));
        assert!(
            cell.obs.iter().all(|&b| b == OBS_NONE),
            "doodads never touch the obstruction grid"
        );
    }

    #[test]
    fn a_thin_wall_blocks_a_body_radius_wide_and_no_further() {
        let (cx, cy) = test_cell();
        let tris = wall(32, 3, 60, 12.0, wmo());
        let cell = derive_cell(cx, cy, Some(&flat_heights()), &tris).expect("a wall blocks");
        // The wall line plus one sub-cell on each side (the agent-radius inset).
        for nx in 31..=33 {
            assert!(!walk_get(&cell.walk, nx, 32), "nx={nx} must be blocked");
        }
        // Two out (1.04 yd) is beyond the margin, so the inset is bounded.
        assert!(walk_get(&cell.walk, 30, 32) && walk_get(&cell.walk, 34, 32));
        // Sight keeps the true footprint, so only the wall's own obs column rises.
        assert!(obs_top(&cell.obs, cell.base_z, 16, 16).is_some());
        assert!(obs_top(&cell.obs, cell.base_z, 15, 16).is_none());
        assert!(obs_top(&cell.obs, cell.base_z, 17, 16).is_none());
    }

    #[test]
    fn a_triangle_spanning_a_cell_boundary_blocks_in_both_cells_clamped_to_each() {
        let (cx, cy) = test_cell();
        // One wall running along x from mid-cell into the next cell over (cx + 1 is lower x).
        let y = at(0, 32).1;
        let (x_a, x_b) = (
            sub_center(cx, 40, WALK_DIM),
            sub_center(cx + 1, 10, WALK_DIM),
        );
        let (lo, hi) = (GROUND_Z - 1.0, GROUND_Z + 10.0);
        let tris = vec![
            VmapTri {
                verts: [[x_a, y, lo], [x_b, y, lo], [x_b, y, hi]],
                class: wmo(),
            },
            VmapTri {
                verts: [[x_a, y, lo], [x_b, y, hi], [x_a, y, hi]],
                class: wmo(),
            },
        ];
        let here = derive_cell(cx, cy, Some(&flat_heights()), &tris).expect("blocks this cell");
        let over = derive_cell(cx + 1, cy, Some(&flat_heights()), &tris).expect("and the next one");
        // Neither derivation drops the triangle for reaching past its cell, and each writes only
        // its own sub-cells: the seam is blocked from both sides.
        assert!(!walk_get(&here.walk, 63, 32) && !walk_get(&over.walk, 0, 32));
        assert!(!walk_get(&here.walk, 40, 32) && !walk_get(&over.walk, 10, 32));
        // Past each end of the wall the cell stays open: the span is clamped, not stamped whole.
        assert!(walk_get(&here.walk, 36, 32) && walk_get(&over.walk, 14, 32));
    }

    #[test]
    fn geometry_outside_the_standing_band_is_not_a_blocker() {
        let (cx, cy) = test_cell();
        let h = flat_heights();
        // A doorstep below step-up height: walk onto it, no row at all.
        let step = slab(GROUND_Z + WALK_STEP_UP - 0.5, wmo());
        assert!(derive_cell(cx, cy, Some(&h), &[step]).is_none());
        // A ceiling well above head height: walk under it, no row at all.
        let ceiling = slab(GROUND_Z + 5.0, wmo());
        assert!(derive_cell(cx, cy, Some(&h), &[ceiling]).is_none());
    }

    #[test]
    fn the_standing_band_follows_terrain_ground_not_the_geometry() {
        let (cx, cy) = test_cell();
        // Ground rises 1 yd per outer-corner row along x (index i*17+j, i counts along x).
        let mut heights = vec![0.0f32; 145];
        for i in 0..9 {
            for j in 0..9 {
                heights[i * 17 + j] = GROUND_Z + i as f32;
            }
        }
        // One flat slab 5 yd above the cell's low end. Ground at walk column nx is
        // GROUND_Z + (nx + 0.5) / 8, so the slab sits inside the band only where the ground has
        // climbed to within [step-up, head height] of it.
        let cell = derive_cell(cx, cy, Some(&heights), &[slab(GROUND_Z + 5.0, wmo())])
            .expect("the slab blocks the band it crosses");
        let ground_at = |nx: usize| (nx as f32 + 0.5) / 8.0;
        for nx in 0..WALK_DIM {
            let clearance = 5.0 - ground_at(nx);
            let in_band = clearance < WALK_HEIGHT && clearance > WALK_STEP_UP;
            assert_eq!(
                !walk_get(&cell.walk, nx, 32),
                in_band,
                "nx={nx} clearance={clearance:.2}"
            );
        }
    }

    #[test]
    fn merging_ands_walkability_and_keeps_the_taller_obstruction() {
        let mut terrain = open_terrain();
        walk_set(&mut terrain.walk, 5, 5, false); // a hole or a steep quad
        obs_raise(&mut terrain.obs, terrain.base_z, 2, 2, GROUND_Z + 3.0);
        let mut coverage = NavCellData {
            base_z: GROUND_Z - 4.0, // coverage floors on its own lowest triangle
            walk: vec![0xFFu8; WALK_BYTES],
            obs: vec![OBS_NONE; OBS_BYTES],
        };
        walk_set(&mut coverage.walk, 9, 9, false);
        obs_raise(&mut coverage.obs, coverage.base_z, 2, 2, GROUND_Z + 9.0);
        obs_raise(&mut coverage.obs, coverage.base_z, 4, 4, GROUND_Z + 1.0);

        let merged = merge_cells(&terrain, &coverage);
        assert_eq!(
            merged.base_z,
            GROUND_Z - 4.0,
            "merge rebases onto the floor"
        );
        assert!(!walk_get(&merged.walk, 5, 5) && !walk_get(&merged.walk, 9, 9));
        assert!(walk_get(&merged.walk, 7, 7));
        // Contested column keeps the taller top; each source's own columns survive rebasing.
        let top = obs_top(&merged.obs, merged.base_z, 2, 2).unwrap();
        assert!((top - (GROUND_Z + 9.0)).abs() <= OBS_STEP);
        let top = obs_top(&merged.obs, merged.base_z, 4, 4).unwrap();
        assert!((top - (GROUND_Z + 1.0)).abs() <= OBS_STEP);
        assert!(obs_top(&merged.obs, merged.base_z, 1, 1).is_none());
    }
}
