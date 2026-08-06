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
use crate::terrain::CELL_SIZE;

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

/// Line of sight from a to b: sample the segment every `LOS_STEP` and compare the ray's
/// interpolated Z (+eye height at both ends) against the obstruction column tops.
// Deliberate simplification: parametric sampling, not an exact DDA — at 0.5 yd steps over a
// 1.04 yd obs grid a column cannot be stepped over; move to exact cell walking when someone
// measures the difference.
pub fn has_los(
    fetch: &mut impl FnMut(u16, u16) -> Option<NavCellData>,
    a: (f32, f32, f32),
    b: (f32, f32, f32),
) -> bool {
    let mut cache = Cache::new(fetch);
    let (dx, dy, dz) = (b.0 - a.0, b.1 - a.1, b.2 - a.2);
    let len = (dx * dx + dy * dy).sqrt();
    let steps = (len / LOS_STEP).ceil().max(1.0) as u32;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let (x, y) = (a.0 + dx * t, a.1 + dy * t);
        let ray_z = a.2 + LOS_EYE_HEIGHT + (dz * t);
        let (Some(cx), Some(cy)) = (crate::terrain::cell_index(x), crate::terrain::cell_index(y))
        else {
            continue;
        };
        let Some(cell) = cache.get(cx, cy) else {
            continue;
        };
        let (Some(ox), Some(oy)) = (sub_index(x, cx, OBS_DIM), sub_index(y, cy, OBS_DIM)) else {
            continue;
        };
        if let Some(top) = obs_top(&cell.obs, cell.base_z, ox, oy) {
            if ray_z < top {
                return false;
            }
        }
    }
    true
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
