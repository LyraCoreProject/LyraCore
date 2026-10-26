use crate::spatial::MAP_COORD_MAX;
use crate::terrain::{interpolate, CELL_SIZE};
use crate::vmap::{TriClass, VmapTri};

// 64 walk cells per terrain-cell axis, about 0.5208 yards each.
pub const WALK_DIM: usize = 64;
// 32 obstruction cells per terrain-cell axis, about 1.0417 yards each.
pub const OBS_DIM: usize = 32;
pub const WALK_BYTES: usize = WALK_DIM * WALK_DIM / 8;
pub const OBS_BYTES: usize = OBS_DIM * OBS_DIM;
// Height quantum in yards, spanning 127 yards above base_z.
pub const OBS_STEP: f32 = 0.5;
pub const OBS_NONE: u8 = 255;
// Geometry below WALK_STEP_UP is a step; geometry above WALK_HEIGHT is headroom.
pub const WALK_STEP_UP: f32 = 0.9;
pub const WALK_HEIGHT: f32 = 2.2;

pub fn sub_index(coord: f32, cell: u16, dim: usize) -> Option<usize> {
    let local = (MAP_COORD_MAX - coord) - cell as f32 * CELL_SIZE;
    if !(0.0..CELL_SIZE).contains(&local) {
        return None;
    }
    Some(((local / CELL_SIZE) * dim as f32) as usize).map(|i| i.min(dim - 1))
}

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

pub fn obs_top(obs: &[u8], base_z: f32, ox: usize, oy: usize) -> Option<f32> {
    match obs.get(oy * OBS_DIM + ox) {
        None | Some(&OBS_NONE) => None,
        Some(&v) => Some(base_z + v as f32 * OBS_STEP),
    }
}

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
        obs_raise(&mut obs, base, 3, 3, 86.0);
        assert!((obs_top(&obs, base, 3, 3).unwrap() - 86.0).abs() < OBS_STEP + 1e-3);
        obs_raise(&mut obs, base, 3, 3, 82.0);
        assert!(obs_top(&obs, base, 3, 3).unwrap() >= 86.0 - OBS_STEP);
        obs_raise(&mut obs, base, 3, 3, base + 10_000.0);
        assert!(obs_top(&obs, base, 3, 3).is_some());
    }
}

#[derive(Clone)]
pub struct NavCellData {
    pub base_z: f32,
    pub walk: Vec<u8>,
    pub obs: Vec<u8>,
}

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
}

pub const LOS_EYE_HEIGHT: f32 = 2.0;
// Half the obstruction resolution in yards, so no column is skipped.
const LOS_STEP: f32 = 0.5;

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

// A committed step exempts its starting obstruction column so a mover can leave the margin.
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

pub fn has_los(
    fetch: &mut impl FnMut(u16, u16) -> Option<NavCellData>,
    a: (f32, f32, f32),
    b: (f32, f32, f32),
) -> bool {
    los_hit(fetch, a, b, false).is_none()
}

pub fn step_hit(
    fetch: &mut impl FnMut(u16, u16) -> Option<NavCellData>,
    a: (f32, f32, f32),
    b: (f32, f32, f32),
) -> Option<(f32, f32)> {
    los_hit(fetch, a, b, true)
}

pub const NAV_RES: f32 = CELL_SIZE / WALK_DIM as f32;

// Classify before f32 rounding can move the coordinate across a cell edge.
fn grid_coord(coord: f32) -> Option<u32> {
    let c = (f64::from(MAP_COORD_MAX) - f64::from(coord)) / f64::from(NAV_RES);
    if !(0.0..(1024 * WALK_DIM) as f64).contains(&c) {
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
    let dimension = WALK_DIM as u32;
    if gx >= 1024 * dimension || gy >= 1024 * dimension {
        return false;
    }
    let cell = cache.get((gx / dimension) as u16, (gy / dimension) as u16);
    cell.is_none_or(|cell| {
        walk_get(
            &cell.walk,
            (gx % dimension) as usize,
            (gy % dimension) as usize,
        )
    })
}

// Exempt the start cell so a mover can leave a conservative obstruction margin.
fn line_walkable(
    cache: &mut Cache<impl FnMut(u16, u16) -> Option<NavCellData>>,
    from: (f32, f32),
    to: (f32, f32),
) -> bool {
    let (Some(mut x), Some(mut y), Some(tx), Some(ty)) = (
        grid_coord(from.0),
        grid_coord(from.1),
        grid_coord(to.0),
        grid_coord(to.1),
    ) else {
        return false;
    };
    let gx = (f64::from(MAP_COORD_MAX) - f64::from(from.0)) / f64::from(NAV_RES);
    let gy = (f64::from(MAP_COORD_MAX) - f64::from(from.1)) / f64::from(NAV_RES);
    let dx = (f64::from(from.0) - f64::from(to.0)) / f64::from(NAV_RES);
    let dy = (f64::from(from.1) - f64::from(to.1)) / f64::from(NAV_RES);
    let crossing = |cell: u32, target: u32, origin: f64, delta: f64| match cell.cmp(&target) {
        std::cmp::Ordering::Less => (cell + 1, (f64::from(cell + 1) - origin) / delta),
        std::cmp::Ordering::Greater => (cell - 1, (f64::from(cell) - origin) / delta),
        std::cmp::Ordering::Equal => (cell, f64::INFINITY),
    };
    while (x, y) != (tx, ty) {
        let (nx, cross_x) = crossing(x, tx, gx, dx);
        let (ny, cross_y) = crossing(y, ty, gy, dy);
        if cross_x == cross_y && (!grid_walkable(cache, nx, y) || !grid_walkable(cache, x, ny)) {
            return false;
        }
        if cross_x <= cross_y {
            x = nx;
        }
        if cross_y <= cross_x {
            y = ny;
        }
        if !grid_walkable(cache, x, y) {
            return false;
        }
    }
    true
}

pub fn walkable_prefix(
    fetch: &mut impl FnMut(u16, u16) -> Option<NavCellData>,
    from: (f32, f32),
    attempted: (f32, f32),
) -> (f32, f32) {
    if ![from.0, from.1, attempted.0, attempted.1]
        .iter()
        .all(|value| value.is_finite())
    {
        return from;
    }
    let mut cache = Cache::new(fetch);
    let mut candidate = attempted;
    for _ in 0..=f32::MANTISSA_DIGITS {
        if line_walkable(&mut cache, from, candidate) {
            return candidate;
        }
        let next = (
            from.0 + (candidate.0 - from.0) * 0.5,
            from.1 + (candidate.1 - from.1) * 0.5,
        );
        if next == from || next == candidate {
            return from;
        }
        candidate = next;
    }
    from
}

/// May return a partial leg; use `find_leg_ex` to distinguish complete and partial plans.
pub fn find_leg(
    fetch: &mut impl FnMut(u16, u16) -> Option<NavCellData>,
    from: (f32, f32),
    to: (f32, f32),
    max_expansions: u32,
) -> Option<Vec<(f32, f32)>> {
    find_leg_ex(fetch, from, to, max_expansions).map(|(path, _, _)| path)
}

/// Returns (waypoints, expansions, reached_goal); `None` means no usable leg.
#[allow(clippy::type_complexity)]
pub fn find_leg_ex(
    fetch: &mut impl FnMut(u16, u16) -> Option<NavCellData>,
    from: (f32, f32),
    to: (f32, f32),
    max_expansions: u32,
) -> Option<(Vec<(f32, f32)>, u32, bool)> {
    let search = search_leg(fetch, from, to, 0.0, max_expansions);
    match search.outcome {
        LegOutcome::Complete(path) => Some((path, search.expansions, true)),
        LegOutcome::Partial(path) => Some((path, search.expansions, false)),
        LegOutcome::Blocked => None,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum LegOutcome {
    Complete(Vec<(f32, f32)>),
    Partial(Vec<(f32, f32)>),
    Blocked,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LegSearch {
    pub outcome: LegOutcome,
    pub expansions: u32,
}

pub fn find_leg_in_range(
    fetch: &mut impl FnMut(u16, u16) -> Option<NavCellData>,
    from: (f32, f32),
    to: (f32, f32),
    stop_dist: f32,
    max_expansions: u32,
) -> Option<Vec<(f32, f32)>> {
    match find_leg_in_range_ex(fetch, from, to, stop_dist, max_expansions).outcome {
        LegOutcome::Complete(path) | LegOutcome::Partial(path) => Some(path),
        LegOutcome::Blocked => None,
    }
}

pub fn find_leg_in_range_ex(
    fetch: &mut impl FnMut(u16, u16) -> Option<NavCellData>,
    from: (f32, f32),
    to: (f32, f32),
    stop_dist: f32,
    max_expansions: u32,
) -> LegSearch {
    if !stop_dist.is_finite() {
        return LegSearch {
            outcome: LegOutcome::Blocked,
            expansions: 0,
        };
    }
    search_leg(fetch, from, to, stop_dist.max(0.0), max_expansions)
}

fn search_leg(
    fetch: &mut impl FnMut(u16, u16) -> Option<NavCellData>,
    from: (f32, f32),
    to: (f32, f32),
    stop_dist: f32,
    max_expansions: u32,
) -> LegSearch {
    let blocked = |expansions| LegSearch {
        outcome: LegOutcome::Blocked,
        expansions,
    };
    let (Some(sx), Some(sy), Some(tx), Some(ty)) = (
        grid_coord(from.0),
        grid_coord(from.1),
        grid_coord(to.0),
        grid_coord(to.1),
    ) else {
        return blocked(0);
    };
    if !stop_dist.is_finite() {
        return blocked(0);
    }
    let mut cache = Cache::new(fetch);
    let distance = (to.0 - from.0).hypot(to.1 - from.1);
    if distance <= stop_dist {
        return LegSearch {
            outcome: LegOutcome::Complete(vec![from]),
            expansions: 0,
        };
    }
    let direct = if stop_dist > 0.0 {
        let scale = (distance - stop_dist) / distance;
        (
            from.0 + (to.0 - from.0) * scale,
            from.1 + (to.1 - from.1) * scale,
        )
    } else {
        to
    };
    if line_walkable(&mut cache, from, direct) {
        return LegSearch {
            outcome: LegOutcome::Complete(vec![direct]),
            expansions: 0,
        };
    }
    let target_walkable = grid_walkable(&mut cache, tx, ty);
    if stop_dist == 0.0 && !target_walkable {
        return blocked(0);
    }
    // A 5/4 heuristic weight trades shortest paths for fewer expansions.
    use std::cmp::Reverse;
    use std::collections::{BinaryHeap, HashMap};
    let h = |x: u32, y: u32| {
        let (dx, dy) = (
            (x as i64 - tx as i64).unsigned_abs(),
            (y as i64 - ty as i64).unsigned_abs(),
        );
        10 * dx.max(dy) + 4 * dx.min(dy)
    };
    let remaining = |x, y| h(x, y).saturating_sub((stop_dist / NAV_RES * 14.0).ceil() as u64);
    let priority = |cost, x, y| cost * 4 + remaining(x, y) * 5;
    let mut open: BinaryHeap<Reverse<(u64, u64, u32, u32)>> = BinaryHeap::new();
    let mut g_cost: HashMap<(u32, u32), u64> = HashMap::new();
    let mut came: HashMap<(u32, u32), (u32, u32)> = HashMap::new();
    g_cost.insert((sx, sy), 0);
    open.push(Reverse((priority(0, sx, sy), h(sx, sy), sx, sy)));
    let mut expanded = 0u32;
    let mut found = false;
    let mut best = ((sx, sy), h(sx, sy));
    while let Some(Reverse((score, _, x, y))) = open.pop() {
        let g0 = g_cost[&(x, y)];
        if score != priority(g0, x, y) {
            continue;
        }
        if ((x, y) == (tx, ty) && target_walkable)
            || (stop_dist > 0.0
                && (grid_to_world(x) - to.0).hypot(grid_to_world(y) - to.1) <= stop_dist
                && grid_walkable(&mut cache, x, y))
        {
            found = true;
            best.0 = (x, y);
            break;
        }
        if expanded == max_expansions {
            break;
        }
        expanded += 1;
        let hxy = h(x, y);
        if hxy < best.1 {
            best = ((x, y), hxy);
        }
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
                open.push(Reverse((priority(ng, nx, ny), h(nx, ny), nx, ny)));
            }
        }
    }
    let end = if found {
        best.0
    } else {
        if best.0 == (sx, sy) {
            return blocked(expanded);
        }
        best.0
    };
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
        let mut j = pts.len() - 1;
        if !line_walkable(&mut cache, anchor, pts[j]) {
            let mut visible = i + 1;
            let mut stride = 1;
            while visible + stride < j {
                let probe = visible + stride;
                if !line_walkable(&mut cache, anchor, pts[probe]) {
                    j = probe;
                    break;
                }
                visible = probe;
                stride *= 2;
            }
            while j - visible > 1 {
                let probe = visible + (j - visible) / 2;
                if line_walkable(&mut cache, anchor, pts[probe]) {
                    visible = probe;
                } else {
                    j = probe;
                }
            }
            j = visible;
        }
        path.push(pts[j]);
        anchor = pts[j];
        i = j;
    }
    if path.is_empty() {
        anchor = (grid_to_world(end.0), grid_to_world(end.1));
        path.push(anchor);
    }
    if found
        && (stop_dist == 0.0 || (anchor.0 - to.0).hypot(anchor.1 - to.1) > stop_dist)
        && line_walkable(&mut cache, anchor, to)
    {
        let previous = path.iter().rev().nth(1).copied().unwrap_or(from);
        if line_walkable(&mut cache, previous, to) {
            path.pop();
        }
        path.push(to);
    }
    LegSearch {
        outcome: if found {
            LegOutcome::Complete(path)
        } else {
            LegOutcome::Partial(path)
        },
        expansions: expanded,
    }
}

#[cfg(test)]
mod runtime_tests {
    use super::*;
    use crate::terrain::cell_index;

    #[test]
    fn rounded_prefix_retains_a_walkable_movement_step() {
        let from = (-8_772.485, -86.496_69);
        let farther_waypoint = (-8_779.947, -78.384_766);
        let attempted = (-8_777.225, -81.344_84);
        let mut cell = NavCellData {
            base_z: 0.0,
            walk: vec![0xff; WALK_BYTES],
            obs: Vec::new(),
        };
        walk_set(&mut cell.walk, 14, 35, false);
        let mut fetch = |cx, cy| (cx == 775 && cy == 514).then(|| cell.clone());

        assert!(line_walkable(
            &mut Cache::new(&mut fetch),
            from,
            farther_waypoint
        ));
        assert!(!line_walkable(&mut Cache::new(&mut fetch), from, attempted));
        let endpoint = walkable_prefix(&mut fetch, from, attempted);
        assert_eq!(endpoint, (-8_773.67, -85.208_725));
        assert!(line_walkable(&mut Cache::new(&mut fetch), from, endpoint));
    }

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

    fn at(nx: usize, ny: usize) -> (f32, f32) {
        let ((cx, cy), _) = walled_cell();
        (sub_center(cx, nx, WALK_DIM), sub_center(cy, ny, WALK_DIM))
    }

    fn enters_rectangle(from: (f32, f32), to: (f32, f32), bounds: [(f64, f64); 2]) -> bool {
        let mut entry = 0.0_f64;
        let mut exit = 1.0_f64;
        for ((start, end), (low, high)) in [(from.0, to.0), (from.1, to.1)].into_iter().zip(bounds)
        {
            let start = f64::from(start);
            let delta = f64::from(end) - start;
            if delta == 0.0 {
                if start <= low || start >= high {
                    return false;
                }
            } else {
                let a = (low - start) / delta;
                let b = (high - start) / delta;
                entry = entry.max(a.min(b));
                exit = exit.min(a.max(b));
            }
        }
        entry < exit
    }

    #[test]
    fn los_blocked_by_wall_clear_through_doorway() {
        let z = 80.0;
        let (ax, ay) = at(10, 50);
        let (bx, by) = at(54, 50);
        assert!(!has_los(&mut fetcher(), (ax, ay, z), (bx, by, z)));
        let (ax, ay) = at(10, 31);
        let (bx, by) = at(54, 31);
        assert!(has_los(&mut fetcher(), (ax, ay, z), (bx, by, z)));
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
        let from = at(10, 50);
        let to = at(54, 50);
        let (path, expanded, complete) =
            find_leg_ex(&mut fetcher(), from, to, 20_000).expect("reachable");
        assert!(complete);
        assert!(expanded > 0, "wall forces real A*, not the fast path");
        assert!(path
            .last()
            .is_some_and(|&p| (p.0 - to.0).abs() < 1e-3 && (p.1 - to.1).abs() < 1e-3));
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
    fn find_leg_does_not_cut_a_blocked_cell_corner() {
        let mut cell = NavCellData {
            base_z: 50.0,
            walk: vec![0xff; WALK_BYTES],
            obs: vec![OBS_NONE; OBS_BYTES],
        };
        walk_set(&mut cell.walk, 0, 18, false);
        let mut fetch = |x, y| ((x, y) == (470, 476)).then(|| cell.clone());
        let endpoints = [(1398.908, 1190.3529), (1400.2607, 1189.8438)];
        let bounds = [
            (1_399.479_113_280_773_2, 1_399.999_946_594_238_3),
            (1_190.104_121_267_795_6, 1_190.624_954_581_260_7),
        ];
        for (from, to) in [(endpoints[0], endpoints[1]), (endpoints[1], endpoints[0])] {
            let (path, _, complete) = find_leg_ex(&mut fetch, from, to, 4096).unwrap();
            assert!(complete);
            assert_eq!(path.last(), Some(&to));
            let mut previous = from;
            for point in path {
                assert!(
                    !enters_rectangle(previous, point, bounds),
                    "blocked segment {previous:?} -> {point:?}"
                );
                previous = point;
            }
        }
    }

    #[test]
    fn reaching_the_exact_destination_preserves_a_needed_corner_waypoint() {
        let from = at(10, 0);
        let center = at(33, 32);
        let to = (center.0 - NAV_RES * 0.3, center.1 + NAV_RES * 0.25);
        let (path, _, complete) = find_leg_ex(&mut fetcher(), from, to, 4096).unwrap();
        assert!(complete);
        assert_eq!(path.last(), Some(&to));
        let bounds = [
            (-8_917.187_159_836_292, -8_916.666_326_522_827),
            (-182.291_659_712_791_44, -181.770_826_399_326_32),
        ];
        let mut previous = from;
        for point in path {
            assert!(
                !enters_rectangle(previous, point, bounds),
                "blocked final approach {previous:?} -> {point:?}"
            );
            previous = point;
        }
    }

    #[test]
    fn route_endpoints_on_either_side_of_a_cell_edge_keep_their_walkability() {
        let mut cell = NavCellData {
            base_z: 50.0,
            walk: vec![0xff; WALK_BYTES],
            obs: vec![OBS_NONE; OBS_BYTES],
        };
        walk_set(&mut cell.walk, 0, 18, false);
        let mut fetch = |x, y| ((x, y) == (470, 476)).then(|| cell.clone());
        for (start_x, end_x, reachable) in [
            (1401.0, 1400.0, true),
            (1401.0, 1400.0_f32.next_down(), false),
            (1398.5, 1399.4791, false),
            (1398.5, 1399.4791_f32.next_down(), true),
        ] {
            let route = find_leg(&mut fetch, (start_x, 1190.3), (end_x, 1190.3), 0);
            assert_eq!(route.is_some(), reachable, "endpoint x={end_x:?}");
        }
    }

    #[test]
    fn blocked_sub_cells_at_map_edges_refuse_entry_but_allow_escape() {
        for cell_index in [0, 1023] {
            for blocked in [0, WALK_DIM - 1] {
                let mut cell = NavCellData {
                    base_z: 50.0,
                    walk: vec![0xff; WALK_BYTES],
                    obs: Vec::new(),
                };
                walk_set(&mut cell.walk, blocked, blocked, false);
                let mut fetch = |x, y| ((x, y) == (cell_index, cell_index)).then(|| cell.clone());
                let inside = if blocked == 0 { 1 } else { WALK_DIM - 2 };
                let from = sub_center(cell_index, inside, WALK_DIM);
                let to = sub_center(cell_index, blocked, WALK_DIM);
                assert!(find_leg(&mut fetch, (from, from), (to, to), 0).is_none());
                assert_eq!(
                    find_leg(&mut fetch, (to, to), (from, from), 0),
                    Some(vec![(from, from)])
                );
            }
        }
    }

    #[test]
    fn a_direct_route_cannot_squeeze_between_blocked_corners() {
        let mut cell = NavCellData {
            base_z: 50.0,
            walk: vec![0xff; WALK_BYTES],
            obs: vec![OBS_NONE; OBS_BYTES],
        };
        walk_set(&mut cell.walk, 1, 2, false);
        let mut fetch = |x, y| ((x, y) == (470, 470)).then(|| cell.clone());
        let from = sub_center(470, 2, WALK_DIM);
        let to = sub_center(470, 0, WALK_DIM);
        assert!(find_leg(&mut fetch, (from, from), (to, to), 0).is_none());
    }

    #[test]
    fn open_field_fast_path_takes_zero_expansions() {
        let from = at(40, 10);
        let to = at(60, 20);
        let (path, expanded, complete) =
            find_leg_ex(&mut fetcher(), from, to, 20_000).expect("open field");
        assert_eq!(expanded, 0);
        assert_eq!(path.len(), 1);
        assert!(complete);
    }

    #[test]
    fn range_search_completes_before_a_blocked_target() {
        let from = at(10, 50);
        let to = at(32, 50);
        let search = find_leg_in_range_ex(&mut fetcher(), from, to, 4.0, 4096);
        let LegOutcome::Complete(path) = search.outcome else {
            panic!("the stopping distance ends before the wall");
        };
        assert_eq!(search.expansions, 0);
        let endpoint = *path.last().unwrap();
        assert!(((endpoint.0 - to.0).hypot(endpoint.1 - to.1) - 4.0).abs() < 0.01);
        assert_ne!(endpoint, from);
    }

    #[test]
    fn bounded_search_prioritizes_progress_around_a_wall() {
        let from = at(10, 50);
        let to = at(54, 50);
        let search = find_leg_in_range_ex(&mut fetcher(), from, to, 0.0, 512);
        let LegOutcome::Complete(path) = search.outcome else {
            panic!("a short detour must complete within 512 expansions");
        };
        assert_eq!(path.last(), Some(&to));
        let mut fetch = fetcher();
        let mut cache = Cache::new(&mut fetch);
        let mut previous = from;
        for point in path {
            assert!(line_walkable(&mut cache, previous, point));
            previous = point;
        }
    }

    #[test]
    fn range_search_retains_safe_partial_progress_at_the_expansion_cap() {
        let from = at(50, 8);
        let to = at(8, 8);
        let search = find_leg_in_range_ex(&mut fetcher(), from, to, 0.0, 32);
        let LegOutcome::Partial(path) = search.outcome else {
            panic!("the sealed pocket permits only partial progress");
        };
        assert_eq!(search.expansions, 32);
        assert_ne!(path.last().copied(), Some(from));
        let mut fetch = fetcher();
        let mut cache = Cache::new(&mut fetch);
        let mut previous = from;
        for point in path {
            assert!(line_walkable(&mut cache, previous, point));
            previous = point;
        }
    }

    #[test]
    fn bounded_search_completes_a_long_detour_without_cutting_corners() {
        const WIDTH: usize = 192;
        const HEIGHT: usize = 192;
        const WALL_X: usize = WIDTH / 2;
        const GAP: std::ops::Range<usize> = 8..12;
        const BASE_CELL: u16 = 400;
        const BUDGET: u32 = 16_384;

        let point = |x: usize, y: usize| {
            (
                sub_center(BASE_CELL + (x / WALK_DIM) as u16, x % WALK_DIM, WALK_DIM),
                sub_center(BASE_CELL + (y / WALK_DIM) as u16, y % WALK_DIM, WALK_DIM),
            )
        };
        let mut fetch = |cell_x: u16, cell_y: u16| {
            let mut cell = NavCellData {
                base_z: 0.0,
                walk: vec![0; WALK_BYTES],
                obs: vec![OBS_NONE; OBS_BYTES],
            };
            let offset_x = (i32::from(cell_x) - i32::from(BASE_CELL)) * WALK_DIM as i32;
            let offset_y = (i32::from(cell_y) - i32::from(BASE_CELL)) * WALK_DIM as i32;
            for local_y in 0..WALK_DIM {
                for local_x in 0..WALK_DIM {
                    let x = offset_x + local_x as i32;
                    let y = offset_y + local_y as i32;
                    let walkable = x > 0
                        && y > 0
                        && x < WIDTH as i32 - 1
                        && y < HEIGHT as i32 - 1
                        && (x != WALL_X as i32 || GAP.contains(&(y as usize)));
                    walk_set(&mut cell.walk, local_x, local_y, walkable);
                }
            }
            Some(cell)
        };
        let from = point(WIDTH / 4, HEIGHT / 2);
        let to = point(WIDTH * 3 / 4, HEIGHT / 2);

        let search = find_leg_in_range_ex(&mut fetch, from, to, 0.0, BUDGET);
        let LegOutcome::Complete(path) = search.outcome else {
            panic!("the reachable detour must fit the search budget");
        };
        assert!(search.expansions < BUDGET);
        assert_eq!(path.last(), Some(&to));
        let mut previous = from;
        let mut cache = Cache::new(&mut fetch);
        for point in path {
            assert!(line_walkable(&mut cache, previous, point));
            previous = point;
        }
    }

    #[test]
    fn blocked_search_retains_expansions_without_a_straight_fallback() {
        let ((cx, cy), mut cell) = walled_cell();
        cell.walk.fill(0);
        walk_set(&mut cell.walk, 10, 10, true);
        walk_set(&mut cell.walk, 50, 50, true);
        let mut fetch = |x, y| (x == cx && y == cy).then(|| cell.clone());
        let search = find_leg_in_range_ex(&mut fetch, at(10, 10), at(50, 50), 0.0, 4096);
        assert_eq!(search.outcome, LegOutcome::Blocked);
        assert_eq!(search.expansions, 1);
    }

    #[test]
    fn zero_search_budget_never_expands_a_node() {
        let search = find_leg_in_range_ex(&mut fetcher(), at(10, 50), at(54, 50), 0.0, 0);
        assert_eq!(search.outcome, LegOutcome::Blocked);
        assert_eq!(search.expansions, 0);
    }

    #[test]
    fn nonfinite_stopping_distance_does_not_produce_a_route() {
        for distance in [f32::NAN, f32::INFINITY, -f32::INFINITY] {
            let search =
                find_leg_in_range_ex(&mut fetcher(), at(10, 50), at(20, 50), distance, 4096);
            assert_eq!(search.outcome, LegOutcome::Blocked);
            assert_eq!(search.expansions, 0);
        }
    }

    #[test]
    fn start_inside_a_blocked_margin_cell_still_fast_paths() {
        let ((cx, cy), _) = walled_cell();
        let mut fetch = fetcher();
        let from = (sub_center(cx, 32, WALK_DIM), sub_center(cy, 50, WALK_DIM));
        let to = at(40, 50);
        let (path, expanded, complete) =
            find_leg_ex(&mut fetch, from, to, 20_000).expect("must escape the margin");
        assert_eq!(expanded, 0, "own-cell exemption keeps the fast path");
        assert_eq!(path.len(), 1);
        assert!(complete);
        assert_eq!(walkable_prefix(&mut fetch, from, to), to);
    }

    #[test]
    fn step_hit_stops_a_wall_crossing_step_and_clears_the_doorway() {
        let z = 80.0;
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
        let (ax, ay) = at(10, 31);
        let (bx, by) = at(54, 31);
        assert!(step_hit(&mut fetcher(), (ax, ay, z), (bx, by, z)).is_none());
    }

    #[test]
    fn step_hit_exempts_the_movers_own_obs_column() {
        let z = 80.0;
        let ((cx, cy), _) = walled_cell();
        let from = (sub_center(cx, 16, OBS_DIM), sub_center(cy, 25, OBS_DIM));
        let to = at(10, 50);
        assert!(step_hit(&mut fetcher(), (from.0, from.1, z), (to.0, to.1, z)).is_none());
        assert!(!has_los(
            &mut fetcher(),
            (from.0, from.1, z),
            (to.0, to.1, z)
        ));
    }

    #[test]
    fn no_path_fallback_straight_step_is_caught_by_the_gate() {
        let z = 80.0;
        let ((cx, _), _) = walled_cell();
        let wall_x = sub_center(cx, 16, OBS_DIM);
        let from = at(10, 50);
        let goal = at(32, 50);
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
        let from = at(50, 8);
        let to = at(8, 8);
        let (path, _, complete) =
            find_leg_ex(&mut fetcher(), from, to, 2_000).expect("partial progress");
        assert!(!complete);
        assert!(!path.is_empty());
        assert!(find_leg(&mut fetcher(), from, at(32, 50), 20_000).is_none());
    }
}

// Inflation in yards exceeds half a walk-cell diagonal to catch thin walls.
const RASTER_MARGIN: f32 = 0.35;

// Body radius in yards; the obstruction grid retains the true footprint.
const AGENT_RADIUS: f32 = 0.5;

// Triangle bins must include this margin to cover inset geometry across cell boundaries.
pub const WALK_MARGIN: f32 = RASTER_MARGIN + AGENT_RADIUS;

// Vertical interval gap tolerance in yards.
const OBS_GAP: f32 = 0.75;
// Transformed coplanar floors may differ by this many yards.
const FLOOR_EPSILON_YD: f32 = 0.01;

struct WorldTri {
    lo: [f32; 3],
    hi: [f32; 3],
    is_wmo: bool,
    v: [[f32; 3]; 3],
    // Plane equation n.p = d; near-vertical planes use the full z range.
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

    fn z_at(&self, x: f32, y: f32, margin: f32) -> Option<(f32, f32)> {
        if x < self.lo[0] - margin
            || x > self.hi[0] + margin
            || y < self.lo[1] - margin
            || y > self.hi[1] + margin
        {
            return None;
        }
        for i in 0..3 {
            let (a, b, c) = (self.v[i], self.v[(i + 1) % 3], self.v[(i + 2) % 3]);
            let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
            let len = (ex * ex + ey * ey).sqrt();
            if len < 1e-6 {
                continue;
            }
            let cross = |px: f32, py: f32| ex * (py - a[1]) - ey * (px - a[0]);
            let side_c = cross(c[0], c[1]);
            let side_p = cross(x, y);
            let dist = side_p * side_c.signum() / len;
            if dist < -margin {
                return None;
            }
            if margin == 0.0 && side_p == 0.0 {
                // Shared edges must keep treads separate and cover mesh diagonals.
                let dx = ex * side_c.signum();
                let dy = ey * side_c.signum();
                if !(dy > 0.0 || (dy == 0.0 && dx < 0.0)) {
                    return None;
                }
            }
        }
        // Steep planes have unstable projected heights; use their full vertical span.
        let n_len = (self.n[0] * self.n[0] + self.n[1] * self.n[1] + self.n[2] * self.n[2]).sqrt();
        if self.n[2].abs() < 0.5 * n_len {
            return Some((self.lo[2], self.hi[2]));
        }
        let z =
            ((self.d - self.n[0] * x - self.n[1] * y) / self.n[2]).clamp(self.lo[2], self.hi[2]);
        Some((z, z))
    }
}

// Clamp in local coordinates; rounded world bounds can fall outside their own cell.
fn clamp_axis(lo_c: f32, hi_c: f32, cell_i: u16, dim: usize) -> Option<(usize, usize)> {
    let local = |coord: f32| (MAP_COORD_MAX - coord) - cell_i as f32 * CELL_SIZE;
    let (lo_local, hi_local) = (local(hi_c), local(lo_c));
    if hi_local <= 0.0 || lo_local >= CELL_SIZE {
        return None;
    }
    let index =
        |l: f32| (((l.clamp(0.0, CELL_SIZE) / CELL_SIZE) * dim as f32) as usize).min(dim - 1);
    Some((index(lo_local), index(hi_local)))
}

fn standing_heights(
    cell_x: u16,
    cell_y: u16,
    dim: usize,
    terrain: &impl Fn(f32, f32) -> f32,
    triangles: &[WorldTri],
) -> Vec<f32> {
    let side = dim + 2;
    let resolution = CELL_SIZE / dim as f32;
    let center = |cell, i: usize| match i {
        0 => sub_center(cell, 0, dim) + resolution,
        i if i > dim => sub_center(cell, dim - 1, dim) - resolution,
        i => sub_center(cell, i - 1, dim),
    };
    // Continue terrain into the collar; binned geometry already includes the body radius.
    let mut ground: Vec<_> = (0..side)
        .flat_map(|ny| {
            (0..side).map(move |nx| {
                terrain(
                    center(cell_x, nx.clamp(1, dim)),
                    center(cell_y, ny.clamp(1, dim)),
                )
            })
        })
        .collect();
    let mut floor = vec![f32::INFINITY; side * side];
    for t in triangles {
        if !t.is_wmo || !crate::vmap::supports_walking(t.n, [0.0, 0.0, 1.0]) {
            continue;
        }
        for ny in 0..side {
            let y = center(cell_y, ny);
            if y < t.lo[1] || y > t.hi[1] {
                continue;
            }
            for nx in 0..side {
                let x = center(cell_x, nx);
                if x < t.lo[0] || x > t.hi[0] {
                    continue;
                }
                let index = ny * side + nx;
                let Some((z, _)) = t.z_at(x, y, 0.0) else {
                    continue;
                };
                if z >= ground[index] {
                    floor[index] = floor[index].min(z);
                }
            }
        }
    }
    // The lowest surface above terrain prevents a low ceiling from becoming the floor.
    let mut pending = Vec::new();
    for index in 0..ground.len() {
        if floor[index].is_finite() && floor[index] <= ground[index] + WALK_HEIGHT {
            ground[index] = floor[index];
            floor[index] = f32::INFINITY;
            pending.push(index);
        }
    }
    // Continue connected ramps beyond the terrain-relative selection band.
    while let Some(index) = pending.pop() {
        let x = index % side;
        let y = index / side;
        for (dx, dy) in [(1isize, 0isize), (-1, 0), (0, 1), (0, -1)] {
            let Some(nx) = x.checked_add_signed(dx).filter(|x| *x < side) else {
                continue;
            };
            let Some(ny) = y.checked_add_signed(dy).filter(|y| *y < side) else {
                continue;
            };
            let neighbor = ny * side + nx;
            if floor[neighbor].is_finite()
                && (floor[neighbor] - ground[index]).abs() <= WALK_STEP_UP
            {
                ground[neighbor] = floor[neighbor];
                floor[neighbor] = f32::INFINITY;
                pending.push(neighbor);
            }
        }
    }
    ground
}

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

    let triangles: Vec<_> = tris.iter().map(WorldTri::new).collect();
    let walk_ground = standing_heights(cell_x, cell_y, WALK_DIM, &ground, &triangles);
    let obs_ground = standing_heights(cell_x, cell_y, OBS_DIM, &ground, &triangles);

    let mut walk = vec![0xFFu8; WALK_BYTES];
    let mut obs = vec![OBS_NONE; OBS_BYTES];
    let mut dirty = false;
    let mut col_ivals: Vec<Vec<(f32, f32)>> = vec![Vec::new(); OBS_BYTES];

    for ny in 0..WALK_DIM {
        for nx in 0..WALK_DIM {
            let index = (ny + 1) * (WALK_DIM + 2) + nx + 1;
            let g = walk_ground[index];
            // Rounded world heights can put an exact step just above the limit.
            if [
                index - 1,
                index + 1,
                index - WALK_DIM - 2,
                index + WALK_DIM + 2,
            ]
            .iter()
            .any(|&neighbor| {
                walk_ground[neighbor] > g + WALK_STEP_UP || g > walk_ground[neighbor] + WALK_STEP_UP
            }) {
                walk_set(&mut walk, nx, ny, false);
                dirty = true;
            }
        }
    }

    for t in &triangles {
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
                    let g = walk_ground[(ny + 1) * (WALK_DIM + 2) + nx + 1];
                    // A separate overhead surface needs clearance even when below step height.
                    let overhead = t.is_wmo
                        && t.n[2].abs() > 1e-6
                        && t.z_at(x, y, 0.0)
                            .is_some_and(|(z, _)| z > g + FLOOR_EPSILON_YD && z < g + WALK_HEIGHT);
                    if overhead || (z_lo < g + WALK_HEIGHT && z_hi > g + WALK_STEP_UP) {
                        walk_set(&mut walk, nx, ny, false);
                        dirty = true;
                    }
                }
            }
        }
        // M2 doodads block movement but do not block vanilla sight lines.
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

    // Only merged obstruction intervals rooted in the head-height band block sight.
    for oy in 0..OBS_DIM {
        for ox in 0..OBS_DIM {
            let ivals = &mut col_ivals[oy * OBS_DIM + ox];
            if ivals.is_empty() {
                continue;
            }
            let g = obs_ground[(oy + 1) * (OBS_DIM + 2) + ox + 1];
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

    fn at(nx: usize, ny: usize) -> (f32, f32) {
        let (cx, cy) = test_cell();
        (sub_center(cx, nx, WALK_DIM), sub_center(cy, ny, WALK_DIM))
    }

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
    fn interaction_range_can_end_before_a_blocked_target() {
        let (cx, cy) = test_cell();
        let tris = wall(32, 0, 40, 12.0, wmo());
        let cell = derive_cell(cx, cy, Some(&flat_heights()), &tris);
        let (from, to) = (at(10, 20), at(32, 20));
        assert!(find_leg(&mut cell_fetcher(cell.clone()), from, to, 4096).is_none());
        let path = find_leg_in_range(&mut cell_fetcher(cell), from, to, 4.0, 4096)
            .expect("a reachable interaction point");
        let end = *path.last().unwrap();
        assert!((end.0 - to.0).hypot(end.1 - to.1) <= 4.01);
        assert!((end.0 - from.0).hypot(end.1 - from.1) > 1.0);
    }

    #[test]
    fn interaction_range_routes_around_an_intervening_wall() {
        let (cx, cy) = test_cell();
        let mut tris = wall(32, 0, 40, 12.0, wmo());
        tris.extend(wall(54, 0, 40, 12.0, wmo()));
        let cell = derive_cell(cx, cy, Some(&flat_heights()), &tris);
        let (from, to) = (at(10, 20), at(54, 20));
        let path = find_leg_in_range(&mut cell_fetcher(cell.clone()), from, to, 4.0, 4096)
            .expect("a route around the first wall");
        assert!(path.len() > 1);
        let end = *path.last().unwrap();
        assert!((end.0 - to.0).hypot(end.1 - to.1) <= 4.01);
        let mut fetch = cell_fetcher(cell);
        let mut cache = Cache::new(&mut fetch);
        let mut previous = from;
        for point in path {
            assert!(line_walkable(&mut cache, previous, point));
            previous = point;
        }
    }

    #[test]
    fn invalid_coordinates_do_not_produce_a_route() {
        for invalid in [f32::NAN, f32::INFINITY, -f32::INFINITY] {
            assert!(
                find_leg_in_range(&mut |_, _| None, (invalid, 0.0), (0.0, 0.0), 4.0, 4096)
                    .is_none()
            );
        }
    }

    #[test]
    fn a_floor_continues_across_a_chunk_boundary() {
        let (cx, cy) = test_cell();
        let tris = [slab(GROUND_Z + 1.6, wmo())];
        let here = derive_cell(cx, cy, Some(&flat_heights()), &tris).unwrap();
        let next = derive_cell(cx + 1, cy, Some(&flat_heights()), &tris).unwrap();
        assert!(walk_get(&here.walk, 63, 32));
        assert!(walk_get(&next.walk, 0, 32));
    }

    #[test]
    fn adjacent_stair_treads_share_a_walkable_edge() {
        let (cx, cy) = test_cell();
        let (seam, y) = at(32, 32);
        let tread = |x0, x1, z| {
            [
                VmapTri {
                    verts: [[x0, y - 50.0, z], [x1, y - 50.0, z], [x1, y + 50.0, z]],
                    class: wmo(),
                },
                VmapTri {
                    verts: [[x0, y - 50.0, z], [x1, y + 50.0, z], [x0, y + 50.0, z]],
                    class: wmo(),
                },
            ]
        };
        for (low, high) in [(0.5, 0.9), (0.0, WALK_STEP_UP)] {
            let mut tris = Vec::from(tread(seam, seam + 50.0, GROUND_Z + low));
            tris.extend(tread(seam - 50.0, seam, GROUND_Z + high));
            for _ in 0..2 {
                let cell = derive_cell(cx, cy, Some(&flat_heights()), &tris).unwrap();
                assert!(walk_get(&cell.walk, 32, 32), "shared tread edge");
                for (from, to) in [(at(16, 32), at(48, 32)), (at(48, 32), at(16, 32))] {
                    assert_eq!(
                        find_leg(&mut cell_fetcher(Some(cell.clone())), from, to, 4096),
                        Some(vec![to])
                    );
                }
                tris.reverse();
                for t in &mut tris {
                    t.verts.swap(0, 1);
                }
            }
        }
    }

    #[test]
    fn a_ceiling_mesh_diagonal_still_blocks_headroom() {
        let (cx, cy) = test_cell();
        let (x, y) = at(32, 32);
        let z = GROUND_Z + 1.5;
        let mut tris = vec![
            slab(GROUND_Z + 0.5, wmo()),
            VmapTri {
                verts: [
                    [x - 8.0, y - 8.0, z],
                    [x + 8.0, y - 8.0, z],
                    [x + 8.0, y + 8.0, z],
                ],
                class: wmo(),
            },
            VmapTri {
                verts: [
                    [x - 8.0, y - 8.0, z],
                    [x + 8.0, y + 8.0, z],
                    [x - 8.0, y + 8.0, z],
                ],
                class: wmo(),
            },
        ];
        for _ in 0..2 {
            let cell = derive_cell(cx, cy, Some(&flat_heights()), &tris).unwrap();
            assert!(!walk_get(&cell.walk, 32, 32), "ceiling mesh diagonal");
            assert!(
                find_leg(&mut cell_fetcher(Some(cell)), at(24, 32), at(32, 32), 4096).is_none()
            );
            tris.reverse();
            for t in &mut tris {
                t.verts.swap(0, 1);
            }
        }
    }

    #[test]
    fn a_low_ceiling_inside_the_floor_selection_band_blocks_routing() {
        let (cx, cy) = test_cell();
        for headroom in [0.25, 0.5, 1.0, 1.5] {
            let tris = [
                slab(GROUND_Z + 0.5, wmo()),
                slab(GROUND_Z + 0.5 + headroom, wmo()),
            ];
            let cell = derive_cell(cx, cy, Some(&flat_heights()), &tris).unwrap();
            assert!(!walk_get(&cell.walk, 32, 20), "headroom={headroom}");
            assert!(
                find_leg(&mut cell_fetcher(Some(cell)), at(10, 20), at(54, 20), 4096).is_none()
            );
        }
    }

    #[test]
    fn overlapping_floor_triangles_tolerate_transform_rounding() {
        let (cx, cy) = test_cell();
        let tris = [slab(GROUND_Z + 1.6, wmo()), slab(GROUND_Z + 1.601, wmo())];
        let cell = derive_cell(cx, cy, Some(&flat_heights()), &tris);
        let (path, _, complete) =
            find_leg_ex(&mut cell_fetcher(cell), at(10, 20), at(54, 20), 4096)
                .expect("coplanar floors have room to stand");
        assert!(complete);
        assert_eq!(path, vec![at(54, 20)]);
    }

    #[test]
    fn a_raised_floor_keeps_low_ceilings_blocked() {
        let (cx, cy) = test_cell();
        let tris = [slab(GROUND_Z + 1.6, wmo()), slab(GROUND_Z + 3.0, wmo())];
        let cell = derive_cell(cx, cy, Some(&flat_heights()), &tris).unwrap();
        assert!(!walk_get(&cell.walk, 32, 32));
    }

    #[test]
    fn a_raised_floor_cannot_be_entered_across_a_high_edge() {
        let (cx, cy) = test_cell();
        let (x0, y0) = at(20, 20);
        let (x1, y1) = at(50, 50);
        let z = GROUND_Z + 1.6;
        let tris = [
            VmapTri {
                verts: [[x0, y0, z], [x1, y0, z], [x1, y1, z]],
                class: wmo(),
            },
            VmapTri {
                verts: [[x0, y0, z], [x1, y1, z], [x0, y1, z]],
                class: wmo(),
            },
        ];
        let cell = derive_cell(cx, cy, Some(&flat_heights()), &tris).unwrap();
        assert!(walk_get(&cell.walk, 35, 35), "the floor itself is walkable");
        let (_, _, complete) =
            find_leg_ex(&mut cell_fetcher(Some(cell)), at(10, 35), at(35, 35), 4096)
                .expect("partial progress toward the edge");
        assert!(!complete, "a floor needs a step or ramp to reach it");
    }

    #[test]
    fn a_raised_wmo_floor_supports_a_route_around_walls() {
        let (cx, cy) = test_cell();
        let mut tris = wall(32, 0, 40, 12.0, wmo());
        tris.push(slab(GROUND_Z + 1.6, wmo()));
        let cell = derive_cell(cx, cy, Some(&flat_heights()), &tris);
        let (path, _, complete) =
            find_leg_ex(&mut cell_fetcher(cell), at(10, 20), at(54, 20), 4096)
                .expect("the floor supports the route");
        assert!(complete);
        assert!(path.len() > 1, "the wall still requires a detour");
    }

    fn raised_ramp(offset: f32) -> [VmapTri; 2] {
        let (x0, y0) = at(10, 10);
        let (x1, y1) = at(55, 55);
        let low = GROUND_Z + 0.5 + offset;
        let high = GROUND_Z + 4.5 + offset;
        [
            VmapTri {
                verts: [[x0, y0, low], [x1, y0, high], [x1, y1, high]],
                class: wmo(),
            },
            VmapTri {
                verts: [[x0, y0, low], [x1, y1, high], [x0, y1, low]],
                class: wmo(),
            },
        ]
    }

    #[test]
    fn a_connected_ramp_continues_above_the_terrain_selection_band() {
        let (cx, cy) = test_cell();
        let mut tris = raised_ramp(0.0);
        for _ in 0..2 {
            let cell = derive_cell(cx, cy, Some(&flat_heights()), &tris);
            assert_eq!(
                find_leg(&mut cell_fetcher(cell), at(14, 32), at(51, 32), 4096),
                Some(vec![at(51, 32)])
            );
            tris.reverse();
            for triangle in &mut tris {
                triangle.verts.swap(0, 1);
            }
        }
    }

    #[test]
    fn a_connected_ramp_keeps_its_low_ceiling_blocked_above_terrain() {
        let (cx, cy) = test_cell();
        let mut tris = raised_ramp(0.0).to_vec();
        tris.extend(raised_ramp(1.5));
        let cell = derive_cell(cx, cy, Some(&flat_heights()), &tris).unwrap();
        assert!(!walk_get(&cell.walk, 51, 32));
        assert!(find_leg(&mut cell_fetcher(Some(cell)), at(14, 32), at(51, 32), 4096).is_none());
    }

    #[test]
    fn a_wmo_wall_detours_find_leg_only_once_its_coverage_is_merged_in() {
        let tris = wall(32, 0, 40, 12.0, wmo());
        let (cx, cy) = test_cell();
        let coverage =
            derive_cell(cx, cy, Some(&flat_heights()), &tris).expect("the wall blocks the cell");
        let merged = merge_cells(&open_terrain(), &coverage);

        let (from, to) = (at(10, 20), at(54, 20));
        let (detour, expanded, complete) =
            find_leg_ex(&mut cell_fetcher(Some(merged)), from, to, 20_000).expect("reachable");
        assert!(complete && expanded > 0, "the wall must force real A*");
        assert!(detour.len() > 1, "a detour needs a corner waypoint");

        let (straight, expanded, _) =
            find_leg_ex(&mut cell_fetcher(Some(open_terrain())), from, to, 20_000)
                .expect("open field");
        assert_eq!((straight.len(), expanded), (1, 0));

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
        for nx in 31..=33 {
            assert!(!walk_get(&cell.walk, nx, 32), "nx={nx} must be blocked");
        }
        assert!(walk_get(&cell.walk, 30, 32) && walk_get(&cell.walk, 34, 32));
        assert!(obs_top(&cell.obs, cell.base_z, 16, 16).is_some());
        assert!(obs_top(&cell.obs, cell.base_z, 15, 16).is_none());
        assert!(obs_top(&cell.obs, cell.base_z, 17, 16).is_none());
    }

    #[test]
    fn a_thin_wall_tip_does_not_extend_past_its_clearance() {
        let (cx, cy) = test_cell();
        let (x, y) = at(32, 32);
        let wall = VmapTri {
            verts: [
                [x, y + WALK_MARGIN + 0.05, GROUND_Z],
                [x - 0.05, y + WALK_MARGIN + 0.35, GROUND_Z + 5.0],
                [x + 0.05, y + WALK_MARGIN + 0.35, GROUND_Z + 5.0],
            ],
            class: wmo(),
        };
        let cell = derive_cell(cx, cy, Some(&flat_heights()), &[wall]).unwrap();
        assert!(
            walk_get(&cell.walk, 32, 32),
            "beyond the inflated footprint"
        );
        assert!(!walk_get(&cell.walk, 32, 31), "inside the wall clearance");
    }

    #[test]
    fn a_triangle_spanning_a_cell_boundary_blocks_in_both_cells_clamped_to_each() {
        let (cx, cy) = test_cell();
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
        assert!(!walk_get(&here.walk, 63, 32) && !walk_get(&over.walk, 0, 32));
        assert!(!walk_get(&here.walk, 40, 32) && !walk_get(&over.walk, 10, 32));
        assert!(walk_get(&here.walk, 36, 32) && walk_get(&over.walk, 14, 32));
    }

    #[test]
    fn geometry_outside_the_standing_band_is_not_a_blocker() {
        let (cx, cy) = test_cell();
        let h = flat_heights();
        let step = slab(GROUND_Z + WALK_STEP_UP - 0.5, wmo());
        assert!(derive_cell(cx, cy, Some(&h), &[step]).is_none());
        let ceiling = slab(GROUND_Z + 5.0, wmo());
        assert!(derive_cell(cx, cy, Some(&h), &[ceiling]).is_none());
    }

    #[test]
    fn the_standing_band_under_doodads_follows_terrain_ground() {
        let (cx, cy) = test_cell();
        let mut heights = vec![0.0f32; 145];
        for i in 0..9 {
            for j in 0..9 {
                heights[i * 17 + j] = GROUND_Z + i as f32;
            }
        }
        let cell = derive_cell(
            cx,
            cy,
            Some(&heights),
            &[slab(GROUND_Z + 5.0, TriClass::M2)],
        )
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
        walk_set(&mut terrain.walk, 5, 5, false);
        obs_raise(&mut terrain.obs, terrain.base_z, 2, 2, GROUND_Z + 3.0);
        let mut coverage = NavCellData {
            base_z: GROUND_Z - 4.0,
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
        let top = obs_top(&merged.obs, merged.base_z, 2, 2).unwrap();
        assert!((top - (GROUND_Z + 9.0)).abs() <= OBS_STEP);
        let top = obs_top(&merged.obs, merged.base_z, 4, 4).unwrap();
        assert!((top - (GROUND_Z + 1.0)).abs() <= OBS_STEP);
        assert!(obs_top(&merged.obs, merged.base_z, 1, 1).is_none());
    }
}
