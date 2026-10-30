#[cfg(feature = "debug_reducers")]
use lyracore_shared::nav::OBS_DIM;
use lyracore_shared::nav::{self, OBS_BYTES, WALK_BYTES, WALK_DIM};
use lyracore_shared::terrain::{cell_index, cell_key};
use spacetimedb::{reducer, table, ReducerContext, Table};
use std::cell::RefCell;
use std::collections::HashMap;

#[table(accessor = game_nav_chunk, public)]
pub struct NavChunk {
    #[primary_key]
    pub key: u64,
    pub map_id: u32,
    pub cell_x: u16,
    pub cell_y: u16,
    pub base_z: f32,
    pub walk: Vec<u8>,
    pub obs: Vec<u8>,
}

#[table(accessor = game_navigation_revision, public)]
pub struct NavigationRevision {
    #[primary_key]
    pub id: u8,
    pub revision: u64,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub struct NavigationInputs {
    pub imported_revision: Option<u64>,
    pub navigation_enabled: bool,
    pub collision_enabled: bool,
    pub coverage_enabled: bool,
    pub static_generation: Option<u64>,
    pub coverage_generation: Option<u64>,
}

pub(crate) fn inputs(ctx: &ReducerContext, map_id: u32) -> NavigationInputs {
    NavigationInputs {
        imported_revision: ctx
            .db
            .game_navigation_revision()
            .id()
            .find(0)
            .map(|r| r.revision),
        navigation_enabled: nav_enabled(ctx),
        collision_enabled: crate::vmap::rays_enabled(ctx),
        coverage_enabled: nav_coverage_enabled(ctx),
        static_generation: crate::vmap::active_generation_id(ctx, map_id),
        coverage_generation: coverage_generation(ctx, map_id),
    }
}

pub(crate) fn record_change(ctx: &ReducerContext) -> Result<(), String> {
    let rows = ctx.db.game_navigation_revision();
    if let Some(mut row) = rows.id().find(0) {
        row.revision = row
            .revision
            .checked_add(1)
            .ok_or("navigation revision exhausted")?;
        rows.id().update(row);
    } else {
        rows.insert(NavigationRevision { id: 0, revision: 1 });
    }
    Ok(())
}

// Missing chunks mean unimported or fully clear terrain, so they do not prove coverage.
pub(crate) fn walkable(ctx: &ReducerContext, map_id: u32, x: f32, y: f32) -> Option<bool> {
    let (cx, cy) = (cell_index(x)?, cell_index(y)?);
    let chunk = ctx
        .db
        .game_nav_chunk()
        .key()
        .find(cell_key(map_id, cx, cy))?;
    let (nx, ny) = (
        nav::sub_index(x, cx, WALK_DIM)?,
        nav::sub_index(y, cy, WALK_DIM)?,
    );
    Some(nav::walk_get(&chunk.walk, nx, ny))
}

#[cfg(feature = "debug_reducers")]
pub(crate) fn obstruction_top(ctx: &ReducerContext, map_id: u32, x: f32, y: f32) -> Option<f32> {
    let (cx, cy) = (cell_index(x)?, cell_index(y)?);
    let chunk = ctx
        .db
        .game_nav_chunk()
        .key()
        .find(cell_key(map_id, cx, cy))?;
    let (ox, oy) = (
        nav::sub_index(x, cx, OBS_DIM)?,
        nav::sub_index(y, cy, OBS_DIM)?,
    );
    nav::obs_top(&chunk.obs, chunk.base_z, ox, oy)
}

fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("odd-length hex blob".to_string());
    }
    (0..s.len() / 2)
        .map(|i| {
            u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|_| format!("bad hex at {i}"))
        })
        .collect()
}

fn load_nav_batch(ctx: &ReducerContext, packed: &str) -> Result<u32, String> {
    let chunks = ctx.db.game_nav_chunk();
    let mut loaded = 0u32;
    for row in packed.split(';').filter(|r| !r.is_empty()) {
        let f: Vec<&str> = row.split(',').collect();
        if f.len() != 6 {
            return Err(format!("nav row needs 6 fields, got {}", f.len()));
        }
        let map_id = f[0]
            .parse::<u32>()
            .map_err(|_| format!("bad u32: {}", f[0]))?;
        let cell_x = f[1]
            .parse::<u16>()
            .map_err(|_| format!("bad u16: {}", f[1]))?;
        let cell_y = f[2]
            .parse::<u16>()
            .map_err(|_| format!("bad u16: {}", f[2]))?;
        let base_z = f[3]
            .parse::<f32>()
            .map_err(|_| format!("bad f32: {}", f[3]))?;
        let walk = if f[4].is_empty() {
            vec![0xFF; WALK_BYTES]
        } else {
            hex_decode(f[4])?
        };
        let obs = hex_decode(f[5])?;
        if walk.len() != WALK_BYTES || !(obs.is_empty() || obs.len() == OBS_BYTES) {
            return Err(format!(
                "nav row blob sizes wrong: walk={} obs={}",
                walk.len(),
                obs.len()
            ));
        }
        chunks.insert(NavChunk {
            key: cell_key(map_id, cell_x, cell_y),
            map_id,
            cell_x,
            cell_y,
            base_z,
            walk,
            obs,
        });
        loaded += 1;
    }
    Ok(loaded)
}

#[reducer]
pub fn import_nav_chunks(ctx: &ReducerContext, packed: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let chunks = ctx.db.game_nav_chunk();
    let keys: Vec<u64> = chunks.iter().map(|c| c.key).collect();
    for k in keys {
        chunks.key().delete(k);
    }
    if load_nav_batch(ctx, &packed)? == 0 {
        return Err("nav import payload was empty".to_string());
    }
    record_change(ctx)
}

#[reducer]
pub fn import_nav_chunks_append(ctx: &ReducerContext, packed: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if load_nav_batch(ctx, &packed)? > 0 {
        record_change(ctx)?;
    }
    Ok(())
}

use crate::game_config;
use crate::vmap::game_vmap_nav_coverage;
use lyracore_shared::nav::NavCellData;

pub(crate) fn nav_enabled(ctx: &ReducerContext) -> bool {
    ctx.db
        .game_config()
        .id()
        .find(0)
        .map(|c| c.nav_enabled)
        .unwrap_or(false)
}

pub(crate) fn nav_coverage_enabled(ctx: &ReducerContext) -> bool {
    ctx.db
        .game_config()
        .id()
        .find(0)
        .map(|c| c.nav_coverage_enabled)
        .unwrap_or(false)
}

pub(crate) fn coverage_generation(ctx: &ReducerContext, map_id: u32) -> Option<u64> {
    if !nav_coverage_enabled(ctx) {
        return None;
    }
    crate::vmap::active_vmap_nav_coverage_manifest(ctx, map_id).map(|m| m.generation_id)
}

fn merged_cell(terrain: Option<NavCellData>, coverage: Option<NavCellData>) -> Option<NavCellData> {
    match (terrain, coverage) {
        (Some(terrain), Some(coverage)) => Some(nav::merge_cells(&terrain, &coverage)),
        (Some(cell), None) | (None, Some(cell)) => Some(cell),
        (None, None) => None,
    }
}

fn fetcher(ctx: &ReducerContext, map_id: u32) -> impl FnMut(u16, u16) -> Option<NavCellData> + '_ {
    let generation = coverage_generation(ctx, map_id);
    move |cx, cy| cached_cell(ctx, map_id, generation, cx, cy)
}

type CellKey = (u32, Option<u64>, u16, u16);

const CELL_CACHE_CELLS: usize = 4_096;

thread_local! {
    static CELL_CACHE: RefCell<Option<HashMap<CellKey, Option<NavCellData>>>> =
        const { RefCell::new(None) };
}

// Only the importer writes nav chunks and coverage, so a cell read once stays valid for the scope.
pub(crate) fn with_cell_cache<T>(query: impl FnOnce() -> T) -> T {
    struct RestoreCache(Option<HashMap<CellKey, Option<NavCellData>>>);
    impl Drop for RestoreCache {
        fn drop(&mut self) {
            CELL_CACHE.with(|cache| *cache.borrow_mut() = self.0.take());
        }
    }
    let _restore = RestoreCache(CELL_CACHE.with(|cache| cache.replace(Some(HashMap::new()))));
    query()
}

fn cached_cell(
    ctx: &ReducerContext,
    map_id: u32,
    generation: Option<u64>,
    cx: u16,
    cy: u16,
) -> Option<NavCellData> {
    let key = (map_id, generation, cx, cy);
    if let Some(hit) = CELL_CACHE.with(|cache| cache.borrow().as_ref()?.get(&key).cloned()) {
        return hit;
    }
    let cell = fetch_cell(ctx, map_id, generation, cx, cy).0;
    CELL_CACHE.with(|cache| {
        if let Some(cells) = cache.borrow_mut().as_mut() {
            if cells.len() == CELL_CACHE_CELLS {
                cells.clear();
            }
            cells.insert(key, cell.clone());
        }
    });
    cell
}

fn fetch_cell(
    ctx: &ReducerContext,
    map_id: u32,
    generation: Option<u64>,
    cx: u16,
    cy: u16,
) -> (Option<NavCellData>, bool) {
    let key = cell_key(map_id, cx, cy);
    let terrain = ctx
        .db
        .game_nav_chunk()
        .key()
        .find(key)
        .map(|c| NavCellData {
            base_z: c.base_z,
            walk: c.walk,
            obs: c.obs,
        });
    let coverage = generation.and_then(|generation_id| {
        ctx.db
            .game_vmap_nav_coverage()
            .by_generation_cell()
            .filter((generation_id, key))
            .next()
    });
    let matching = coverage.as_ref().is_some_and(|c| {
        c.map_id == map_id
            && c.cell_x == cx
            && c.cell_y == cy
            && c.walk.len() == WALK_BYTES
            && (c.obs.is_empty() || c.obs.len() == OBS_BYTES)
    });
    let coverage = coverage.map(|c| NavCellData {
        base_z: c.base_z,
        walk: c.walk,
        obs: c.obs,
    });
    (merged_cell(terrain, coverage), matching)
}

pub(crate) fn has_los(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
    a: (f32, f32, f32),
    b: (f32, f32, f32),
) -> bool {
    if crate::vmap::los_ray(ctx, map_id, instance_id, [a.0, a.1, a.2], [b.0, b.1, b.2]).is_some() {
        return false;
    }
    if crate::vmap::vmap_enabled(ctx, map_id) || !nav_enabled(ctx) {
        return true;
    }
    lyracore_shared::nav::has_los(&mut fetcher(ctx, map_id), a, b)
}

pub(crate) const LEG_MAX_EXPANSIONS: u32 = 16_384;

#[derive(spacetimedb::SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub enum RouteStatus {
    Complete,
    Partial,
    Blocked,
    Direct,
}

#[derive(spacetimedb::SpacetimeType, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(has_packages),
    allow(dead_code, reason = "retained Package API route value")
)]
pub enum CoverageEvidence {
    Unknown,
    VerifiedCells(VerifiedRouteCells),
}

#[derive(spacetimedb::SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct VerifiedRouteCells {
    pub generation_id: u64,
    pub checked_cells: u32,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RoutePoint {
    pub x: f32,
    pub y: f32,
}

impl From<(f32, f32)> for RoutePoint {
    fn from((x, y): (f32, f32)) -> Self {
        Self { x, y }
    }
}

#[derive(spacetimedb::SpacetimeType, Clone, Debug, PartialEq)]
#[cfg_attr(
    not(has_packages),
    allow(dead_code, reason = "retained Package API route value")
)]
pub struct RouteClip {
    pub attempted: RoutePoint,
    pub hit: RoutePoint,
}

#[derive(spacetimedb::SpacetimeType, Clone, Debug, PartialEq)]
#[cfg_attr(
    not(has_packages),
    allow(dead_code, reason = "retained Package API route value")
)]
pub struct RouteStep {
    pub from: RoutePoint,
    pub endpoint: RoutePoint,
    pub first_waypoint: Option<RoutePoint>,
    pub status: RouteStatus,
    pub expansions: u32,
    pub clipping: Option<RouteClip>,
    pub coverage: CoverageEvidence,
}

#[cfg_attr(
    not(has_packages),
    allow(dead_code, reason = "retained Package API route value")
)]
pub struct RoutePath {
    pub step: RouteStep,
    pub points: Vec<(f32, f32, f32)>,
}

#[cfg_attr(
    not(has_packages),
    allow(dead_code, reason = "retained Package API operation")
)]
pub(crate) fn route_path(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
    start: (f32, f32, f32),
    destination: (f32, f32, f32),
    stop_dist: f32,
) -> RoutePath {
    route_path_with_budget(
        ctx,
        map_id,
        instance_id,
        start,
        destination,
        stop_dist,
        LEG_MAX_EXPANSIONS,
    )
}

#[cfg_attr(
    not(has_packages),
    allow(dead_code, reason = "retained Package API operation")
)]
pub(crate) fn route_path_with_budget(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
    start: (f32, f32, f32),
    destination: (f32, f32, f32),
    stop_dist: f32,
    max_expansions: u32,
) -> RoutePath {
    use lyracore_shared::movement_path::{self, MAX_DISTANCE, MAX_POINTS};
    let cur = (start.0, start.1);
    let dest = (destination.0, destination.1);
    let mut result = RoutePath {
        step: RouteStep {
            from: cur.into(),
            endpoint: cur.into(),
            first_waypoint: None,
            status: RouteStatus::Blocked,
            expansions: 0,
            clipping: None,
            coverage: CoverageEvidence::Unknown,
        },
        points: Vec::new(),
    };
    if ![
        start.0,
        start.1,
        start.2,
        destination.0,
        destination.1,
        destination.2,
        stop_dist,
    ]
    .iter()
    .all(|v| v.is_finite())
        || stop_dist < 0.0
        || [cur.0, cur.1, dest.0, dest.1]
            .iter()
            .any(|&v| cell_index(v).is_none())
    {
        return result;
    }
    let enabled = nav_enabled(ctx);
    let generation = enabled.then(|| coverage_generation(ctx, map_id)).flatten();
    let mut checked = std::collections::BTreeSet::new();
    let mut all_covered = generation.is_some();
    let mut fetch = |cx, cy| {
        let (cell, covered) = fetch_cell(ctx, map_id, generation, cx, cy);
        checked.insert((cx, cy));
        all_covered &= covered;
        cell
    };
    let planned = if enabled {
        if let (Some(cx), Some(cy)) = (cell_index(cur.0), cell_index(cur.1)) {
            fetch(cx, cy);
        }
        let search = nav::find_leg_in_range_ex(
            &mut fetch,
            cur,
            dest,
            stop_dist,
            max_expansions.min(LEG_MAX_EXPANSIONS),
        );
        result.step.expansions = search.expansions;
        match search.outcome {
            nav::LegOutcome::Complete(points) => {
                result.step.status = RouteStatus::Complete;
                points
            }
            nav::LegOutcome::Partial(points) => {
                result.step.status = RouteStatus::Partial;
                points
            }
            nav::LegOutcome::Blocked => Vec::new(),
        }
    } else {
        result.step.status = RouteStatus::Direct;
        vec![crate::creatures::chase_step(
            cur.0,
            cur.1,
            dest.0,
            dest.1,
            MAX_DISTANCE,
            stop_dist,
        )]
    };
    result.step.first_waypoint = planned.first().copied().map(RoutePoint::from);
    let mut from = start;
    let mut remaining = MAX_DISTANCE;
    'route: for point in planned {
        while (point.0 - from.0).hypot(point.1 - from.1) >= 0.01 {
            if result.points.len() == MAX_POINTS || remaining < 0.01 {
                break 'route;
            }
            let sample = sample_walking_step(
                (from.0, from.1),
                point,
                remaining.min(lyracore_shared::terrain::QUAD),
                |stepped| {
                    let attempted_z = crate::terrain::walking_z(
                        ctx,
                        map_id,
                        instance_id,
                        stepped.0,
                        stepped.1,
                        from.2,
                        height_on_segment(start, destination, stepped),
                    );
                    let attempted = (stepped.0, stepped.1, attempted_z);
                    let (endpoint, clipping) =
                        step_gate_with_fetch(ctx, map_id, instance_id, from, attempted, &mut fetch);
                    WalkingStep {
                        attempted,
                        endpoint,
                        clipping,
                    }
                },
            );
            let stepped = (sample.attempted.0, sample.attempted.1);
            let attempted_z = sample.attempted.2;
            let endpoint = sample.endpoint;
            let clipping = sample.clipping;
            let endpoint = if enabled {
                nav::walkable_prefix(&mut fetch, (from.0, from.1), endpoint)
            } else {
                endpoint
            };
            result.step.clipping = clipping;
            if (endpoint.0 - from.0).hypot(endpoint.1 - from.1) < 0.01 {
                break 'route;
            }
            let z = height_on_segment(from, (stepped.0, stepped.1, attempted_z), endpoint);
            let next = (endpoint.0, endpoint.1, z);
            let travelled = movement_path::distance(from, next);
            if !travelled.is_finite() || travelled > remaining + 0.01 {
                break 'route;
            }
            remaining -= travelled;
            result.points.push(next);
            from = next;
            let shortened = endpoint != stepped || result.step.clipping.is_some();
            if shortened {
                break 'route;
            }
        }
    }
    if let Some(points) = fit_wire_path(start, &result.points, |from, to| {
        segment_clear_with_fetch(ctx, map_id, instance_id, from, to, enabled, &mut fetch)
    }) {
        result.points = points;
    } else {
        result.points.truncate(1);
    }
    if let Some(last) = result.points.last() {
        result.step.endpoint = (last.0, last.1).into();
    }
    if all_covered && !checked.is_empty() {
        if let Some(generation_id) = generation {
            result.step.coverage = CoverageEvidence::VerifiedCells(VerifiedRouteCells {
                generation_id,
                checked_cells: checked.len() as u32,
            });
        }
    }
    result
}

#[cfg_attr(
    not(has_packages),
    allow(dead_code, reason = "retained Package API route value")
)]
struct WalkingStep {
    attempted: (f32, f32, f32),
    endpoint: (f32, f32),
    clipping: Option<RouteClip>,
}

// A shortened probe must clear the original collision point to avoid eroding walls.
#[cfg_attr(
    not(has_packages),
    allow(dead_code, reason = "retained Package API operation")
)]
fn sample_walking_step(
    from: (f32, f32),
    towards: (f32, f32),
    max_reach: f32,
    mut sample: impl FnMut((f32, f32)) -> WalkingStep,
) -> WalkingStep {
    let distance = (towards.0 - from.0).hypot(towards.1 - from.1);
    let mut reach = max_reach.min(distance);
    let point = |reach| {
        if reach >= distance {
            towards
        } else {
            crate::creatures::chase_step(from.0, from.1, towards.0, towards.1, reach, 0.0)
        }
    };
    let original = sample(point(reach));
    let Some(clip) = &original.clipping else {
        return original;
    };
    let hit_distance = (clip.hit.x - from.0).hypot(clip.hit.y - from.1);
    let minimum = lyracore_shared::nav::NAV_RES * 0.5;
    while reach > minimum {
        reach = (reach * 0.5).max(minimum);
        let stepped = point(reach);
        if (stepped.0 - from.0).hypot(stepped.1 - from.1) <= hit_distance + 0.01 {
            break;
        }
        let shorter = sample(stepped);
        if shorter.clipping.is_none() {
            return shorter;
        }
    }
    original
}

#[cfg_attr(
    not(has_packages),
    allow(dead_code, reason = "retained Package API operation")
)]
fn fit_wire_path(
    start: (f32, f32, f32),
    points: &[(f32, f32, f32)],
    mut clear: impl FnMut((f32, f32, f32), (f32, f32, f32)) -> bool,
) -> Option<Vec<(f32, f32, f32)>> {
    use lyracore_shared::movement_path;
    let points = movement_path::wire_points(points)?;
    let destination = *points.last()?;
    let mut checks = 0;
    let mut gate = |from, to| {
        checks += 1;
        checks <= 512 && clear(from, to)
    };
    let mut from = start;
    if points.iter().all(|&to| {
        let accepted = gate(from, to);
        from = to;
        accepted
    }) {
        return Some(points);
    }
    let offsets = [
        (0.0, 0.0),
        (0.25, 0.0),
        (-0.25, 0.0),
        (0.0, 0.25),
        (0.0, -0.25),
        (0.25, 0.25),
        (0.25, -0.25),
        (-0.25, 0.25),
        (-0.25, -0.25),
    ];
    let mut routes = vec![(vec![start], 0.0f32)];
    for point in &points[..points.len() - 1] {
        let mut next_routes = Vec::new();
        for (x, y) in offsets {
            let candidate = (point.0 + x, point.1 + y, point.2);
            if movement_path::distance(candidate, destination).powi(2) < 0.5
                || movement_path::packed_offset(destination, candidate).is_none()
            {
                continue;
            }
            if let Some((route, cost)) = routes.iter().find(|(route, _)| {
                gate(
                    *route.last().expect("a fitted route includes its start"),
                    candidate,
                )
            }) {
                let mut route = route.clone();
                route.push(candidate);
                next_routes.push((route, cost + x * x + y * y));
            }
        }
        if next_routes.is_empty() {
            return None;
        }
        next_routes.sort_by(|a, b| a.1.total_cmp(&b.1));
        routes = next_routes;
    }
    let (mut route, _) = routes.into_iter().find(|(route, _)| {
        gate(
            *route.last().expect("a fitted route includes its start"),
            destination,
        )
    })?;
    route.remove(0);
    route.push(destination);
    Some(route)
}

fn segment_clear_with_fetch(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
    from: (f32, f32, f32),
    to: (f32, f32, f32),
    enabled: bool,
    fetch: &mut impl FnMut(u16, u16) -> Option<NavCellData>,
) -> bool {
    let end = (to.0, to.1);
    let start = (from.0, from.1);
    let (stepped, clipping) = step_gate_with_fetch(ctx, map_id, instance_id, from, to, fetch);
    clipping.is_none()
        && stepped == end
        && (!enabled || nav::walkable_prefix(fetch, start, end) == end)
}

pub(crate) fn route_segment_clear(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
    from: (f32, f32, f32),
    to: (f32, f32, f32),
) -> bool {
    segment_clear_with_fetch(
        ctx,
        map_id,
        instance_id,
        from,
        to,
        nav_enabled(ctx),
        &mut fetcher(ctx, map_id),
    )
}

#[allow(clippy::too_many_arguments)]
#[cfg_attr(
    not(has_packages),
    allow(dead_code, reason = "retained Package API operation")
)]
pub(crate) fn route_step(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
    cur: (f32, f32),
    dest: (f32, f32),
    max_step: f32,
    stop_dist: f32,
    z: f32,
) -> RouteStep {
    let mut result = RouteStep {
        from: cur.into(),
        endpoint: cur.into(),
        first_waypoint: None,
        status: RouteStatus::Blocked,
        expansions: 0,
        clipping: None,
        coverage: CoverageEvidence::Unknown,
    };
    if ![cur.0, cur.1, dest.0, dest.1, max_step, stop_dist, z]
        .iter()
        .all(|value| value.is_finite())
        || max_step < 0.0
        || stop_dist < 0.0
        || [cur.0, cur.1, dest.0, dest.1]
            .iter()
            .any(|&value| cell_index(value).is_none())
    {
        return result;
    }
    let enabled = nav_enabled(ctx);
    let generation = enabled.then(|| coverage_generation(ctx, map_id)).flatten();
    let mut checked = std::collections::BTreeSet::new();
    let mut all_covered = generation.is_some();
    let mut fetch = |cx, cy| {
        let (cell, covered) = fetch_cell(ctx, map_id, generation, cx, cy);
        checked.insert((cx, cy));
        all_covered &= covered;
        cell
    };
    let attempted = if enabled {
        if let (Some(cx), Some(cy)) = (cell_index(cur.0), cell_index(cur.1)) {
            fetch(cx, cy);
        }
        let search =
            nav::find_leg_in_range_ex(&mut fetch, cur, dest, stop_dist, LEG_MAX_EXPANSIONS);
        result.expansions = search.expansions;
        let path = match search.outcome {
            nav::LegOutcome::Complete(path) => {
                result.status = RouteStatus::Complete;
                path
            }
            nav::LegOutcome::Partial(path) => {
                result.status = RouteStatus::Partial;
                path
            }
            nav::LegOutcome::Blocked => Vec::new(),
        };
        result.first_waypoint = path.first().copied().map(RoutePoint::from);
        result.first_waypoint.map_or(cur, |wp| {
            crate::creatures::chase_step(cur.0, cur.1, wp.x, wp.y, max_step, 0.0)
        })
    } else {
        result.status = RouteStatus::Direct;
        crate::creatures::chase_step(cur.0, cur.1, dest.0, dest.1, max_step, stop_dist)
    };
    let (endpoint, clipping) = step_gate_with_fetch(
        ctx,
        map_id,
        instance_id,
        (cur.0, cur.1, z),
        (attempted.0, attempted.1, z),
        &mut fetch,
    );
    result.endpoint = if enabled {
        nav::walkable_prefix(&mut fetch, cur, endpoint)
    } else {
        endpoint
    }
    .into();
    result.clipping = clipping;
    if all_covered && !checked.is_empty() {
        if let Some(generation_id) = generation {
            result.coverage = CoverageEvidence::VerifiedCells(VerifiedRouteCells {
                generation_id,
                checked_cells: checked.len() as u32,
            });
        }
    }
    result
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn nav_step(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
    cur: (f32, f32),
    dest: (f32, f32),
    max_step: f32,
    stop_dist: f32,
    z: f32,
) -> (f32, f32) {
    let stepped = if !nav_enabled(ctx) {
        crate::creatures::chase_step(cur.0, cur.1, dest.0, dest.1, max_step, stop_dist)
    } else {
        match lyracore_shared::nav::find_leg_in_range(
            &mut fetcher(ctx, map_id),
            cur,
            dest,
            stop_dist,
            LEG_MAX_EXPANSIONS,
        ) {
            Some(path) => {
                let wp = path[0];
                crate::creatures::chase_step(cur.0, cur.1, wp.0, wp.1, max_step, 0.0)
            }
            None => crate::creatures::chase_step(cur.0, cur.1, dest.0, dest.1, max_step, stop_dist),
        }
    };
    step_gate(ctx, map_id, instance_id, cur, stepped, z)
}

const GATE_CLEARANCE_YD: f32 = 1.0;

fn step_gate(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
    cur: (f32, f32),
    stepped: (f32, f32),
    z: f32,
) -> (f32, f32) {
    let mut fetch = None;
    step_gate_with_fetch(
        ctx,
        map_id,
        instance_id,
        (cur.0, cur.1, z),
        (stepped.0, stepped.1, z),
        &mut |cx, cy| fetch.get_or_insert_with(|| fetcher(ctx, map_id))(cx, cy),
    )
    .0
}

fn step_gate_with_fetch(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
    cur: (f32, f32, f32),
    stepped: (f32, f32, f32),
    fetch: &mut impl FnMut(u16, u16) -> Option<NavCellData>,
) -> ((f32, f32), Option<RouteClip>) {
    gate_step(cur, stepped, |from, to| {
        let exact = crate::vmap::collision_ray(ctx, map_id, instance_id, from, to);
        let grid = if !crate::vmap::vmap_enabled(ctx, map_id) && nav_enabled(ctx) {
            // The grid query adds eye height itself; pass foot height here.
            lyracore_shared::nav::step_hit(fetch, cur, stepped)
                .map(|p| [p.0, p.1, height_on_segment(from.into(), to.into(), p)])
        } else {
            None
        };
        crate::vmap::nearest_hit(from, exact, grid).map(|p| (p[0], p[1]))
    })
}

fn height_on_segment(from: (f32, f32, f32), to: (f32, f32, f32), at: (f32, f32)) -> f32 {
    let length = (to.0 - from.0).hypot(to.1 - from.1);
    let fraction = ((at.0 - from.0).hypot(at.1 - from.1) / length.max(0.001)).min(1.0);
    from.2 + (to.2 - from.2) * fraction
}

fn gate_step(
    cur: (f32, f32, f32),
    stepped: (f32, f32, f32),
    mut collision: impl FnMut([f32; 3], [f32; 3]) -> Option<(f32, f32)>,
) -> ((f32, f32), Option<RouteClip>) {
    if stepped == cur {
        return ((stepped.0, stepped.1), None);
    }
    let probe_height = lyracore_shared::nav::WALK_STEP_UP;
    let hit = collision(
        [cur.0, cur.1, cur.2 + probe_height],
        [stepped.0, stepped.1, stepped.2 + probe_height],
    );
    match hit {
        Some((hx, hy)) => {
            let (dx, dy) = (hx - cur.0, hy - cur.1);
            let hit_dist = (dx * dx + dy * dy).sqrt();
            let land_dist = hit_dist - GATE_CLEARANCE_YD;
            let endpoint = if land_dist <= 0.0 {
                (cur.0, cur.1)
            } else {
                (
                    cur.0 + dx / hit_dist * land_dist,
                    cur.1 + dy / hit_dist * land_dist,
                )
            };
            (
                endpoint,
                Some(RouteClip {
                    attempted: (stepped.0, stepped.1).into(),
                    hit: (hx, hy).into(),
                }),
            )
        }
        None => ((stepped.0, stepped.1), None),
    }
}

#[cfg(feature = "debug_reducers")]
pub(crate) fn debug_find_leg(
    ctx: &ReducerContext,
    map_id: u32,
    from: (f32, f32),
    to: (f32, f32),
) -> String {
    let coverage = match coverage_generation(ctx, map_id) {
        Some(generation_id) => format!("coverage=generation:{generation_id}"),
        None => "coverage=off".to_string(),
    };
    match lyracore_shared::nav::find_leg_ex(&mut fetcher(ctx, map_id), from, to, LEG_MAX_EXPANSIONS)
    {
        None => format!("{coverage} budget={LEG_MAX_EXPANSIONS} None(fallback straight)"),
        Some((path, expanded, complete)) => {
            let pts: Vec<String> = path
                .iter()
                .map(|p| format!("({:.1},{:.1})", p.0, p.1))
                .collect();
            format!(
                "{coverage} budget={LEG_MAX_EXPANSIONS} path[{}] expansions={} complete={} {}",
                path.len(),
                expanded,
                complete,
                pts.join(" ")
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::nav::{walk_get, walk_set, OBS_BYTES, OBS_NONE, WALK_BYTES};

    #[test]
    fn walking_samples_climb_close_stairs_without_reducing_wall_clearance() {
        use lyracore_shared::vmap::{cast_ray, RayFlavor, TriClass, VmapTri};
        let from = (1200.0, 1200.0, 50.0);
        for wall in [false, true] {
            let mut triangles = Vec::new();
            for (x, height) in [(1200.125, 0.8), (1200.375, if wall { 4.0 } else { 1.6 })] {
                for verts in [
                    [
                        [x, 1199.0, 50.0],
                        [x, 1201.0, 50.0],
                        [x, 1201.0, 50.0 + height],
                    ],
                    [
                        [x, 1199.0, 50.0],
                        [x, 1201.0, 50.0 + height],
                        [x, 1199.0, 50.0 + height],
                    ],
                ] {
                    triangles.push(VmapTri {
                        verts,
                        class: TriClass::M2,
                    });
                }
            }
            let sample = |point: (f32, f32)| {
                let z = if !wall && point.0 >= 1200.375 {
                    51.6
                } else if point.0 >= 1200.125 && point.0 < 1200.375 {
                    50.8
                } else {
                    50.0
                };
                let attempted = (point.0, point.1, z);
                let (endpoint, clipping) = gate_step(from, attempted, |a, b| {
                    cast_ray(
                        &mut |_, _| Some(triangles.clone()),
                        a,
                        b,
                        RayFlavor::Collision,
                    )
                    .map(|hit| (hit[0], hit[1]))
                });
                WalkingStep {
                    attempted,
                    endpoint,
                    clipping,
                }
            };
            let step = sample_walking_step((from.0, from.1), (1204.0, 1200.0), 4.0, sample);
            if wall {
                assert_eq!(step.endpoint, (from.0, from.1));
                assert!(step.clipping.is_some());
            } else {
                assert!(step.endpoint.0 > 1200.375);
                assert_eq!(step.attempted.2, 51.6);
                assert!(step.clipping.is_none());
            }
        }
    }

    #[test]
    fn blocked_walking_samples_retain_the_original_collision_and_progress() {
        let step = sample_walking_step((1200.0, 1200.0), (1204.0, 1200.0), 4.0, |point| {
            let attempted = (point.0, point.1, 50.0);
            let (endpoint, clipping) = gate_step((1200.0, 1200.0, 50.0), attempted, |_, _| {
                Some((1203.0, 1200.0))
            });
            WalkingStep {
                attempted,
                endpoint,
                clipping,
            }
        });
        assert_eq!(step.endpoint, (1202.0, 1200.0));
        assert_eq!(step.clipping.unwrap().attempted, (1204.0, 1200.0).into());
    }

    #[test]
    fn collision_evidence_retains_the_attempt_when_the_step_cannot_advance() {
        let from = (0.0, 0.0);
        let attempted = (5.0, 0.0);
        let hit = (0.5, 0.0);
        let (endpoint, clipping) = gate_step(
            (from.0, from.1, 80.0),
            (attempted.0, attempted.1, 80.0),
            |_, _| Some(hit),
        );
        assert_eq!(endpoint, from);
        assert_eq!(
            clipping,
            Some(RouteClip {
                attempted: attempted.into(),
                hit: hit.into()
            })
        );
    }

    #[test]
    fn collision_evidence_retains_progress_before_the_hit() {
        let from = (0.0, 0.0);
        let attempted = (5.0, 0.0);
        let hit = (3.0, 0.0);
        let (endpoint, clipping) = gate_step(
            (from.0, from.1, 80.0),
            (attempted.0, attempted.1, 82.0),
            |_, _| Some(hit),
        );
        assert_eq!(endpoint, (2.0, 0.0));
        let height = height_on_segment((0.0, 0.0, 80.0), (5.0, 0.0, 82.0), endpoint);
        assert!((height - 80.8).abs() < 0.001);
        assert_eq!(
            clipping,
            Some(RouteClip {
                attempted: attempted.into(),
                hit: hit.into()
            })
        );
    }

    #[test]
    fn a_shortened_segment_keeps_height_within_its_endpoints() {
        assert_eq!(
            height_on_segment((0.0, 0.0, 80.0), (0.0, 0.0, 82.0), (0.0, 0.0)),
            80.0
        );
        assert_eq!(
            height_on_segment((0.0, 0.0, 80.0), (5.0, 0.0, 82.0), (6.0, 0.0)),
            82.0
        );
        assert_eq!(
            height_on_segment((0.0, 0.0, 82.0), (5.0, 0.0, 80.0), (2.5, 0.0)),
            81.0
        );
    }

    #[test]
    fn an_unclipped_step_has_no_collision_evidence() {
        let attempted = (5.0, 0.0);
        let (endpoint, clipping) = gate_step(
            (0.0, 0.0, 80.0),
            (attempted.0, attempted.1, 80.0),
            |_, _| None,
        );
        assert_eq!(endpoint, attempted);
        assert_eq!(clipping, None);
    }

    #[test]
    fn movement_steps_over_a_low_riser_but_stops_before_a_wall() {
        use lyracore_shared::vmap::{cast_ray, RayFlavor, TriClass, VmapTri};
        let cur = (-8910.0, -180.0);
        let dest = (-8915.0, -180.0);
        for (height, expected) in [(0.5, dest), (4.0, (-8911.5, -180.0))] {
            let x = -8912.5;
            let tris = vec![
                VmapTri {
                    verts: [
                        [x, -190.0, 80.0],
                        [x, -170.0, 80.0],
                        [x, -170.0, 80.0 + height],
                    ],
                    class: TriClass::Wmo {
                        group_id: 0,
                        mogp_flags: 0,
                    },
                },
                VmapTri {
                    verts: [
                        [x, -190.0, 80.0],
                        [x, -170.0, 80.0 + height],
                        [x, -190.0, 80.0 + height],
                    ],
                    class: TriClass::Wmo {
                        group_id: 0,
                        mogp_flags: 0,
                    },
                },
            ];
            let (stepped, _) =
                gate_step((cur.0, cur.1, 80.0), (dest.0, dest.1, 80.0), |from, to| {
                    cast_ray(
                        &mut |_, _| Some(tris.clone()),
                        from,
                        to,
                        RayFlavor::Collision,
                    )
                    .map(|h| (h[0], h[1]))
                });
            assert_eq!(stepped, expected, "height={height}");
        }
    }

    fn open(base_z: f32) -> NavCellData {
        NavCellData {
            base_z,
            walk: vec![0xFF; WALK_BYTES],
            obs: vec![OBS_NONE; OBS_BYTES],
        }
    }

    #[test]
    fn movement_follows_an_incline_without_hitting_the_floor() {
        use lyracore_shared::vmap::{cast_ray, RayFlavor, TriClass, VmapTri};
        let triangles = vec![
            VmapTri {
                verts: [
                    [1200.0, 1198.0, 50.0],
                    [1205.0, 1198.0, 52.0],
                    [1200.0, 1202.0, 50.0],
                ],
                class: TriClass::M2,
            },
            VmapTri {
                verts: [
                    [1205.0, 1198.0, 52.0],
                    [1205.0, 1202.0, 52.0],
                    [1200.0, 1202.0, 50.0],
                ],
                class: TriClass::M2,
            },
        ];
        let (endpoint, clipping) = gate_step(
            (1200.0, 1200.0, 50.0),
            (1205.0, 1200.0, 52.0),
            |from, to| {
                cast_ray(
                    &mut |_, _| Some(triangles.clone()),
                    from,
                    to,
                    RayFlavor::Collision,
                )
                .map(|hit| (hit[0], hit[1]))
            },
        );
        assert_eq!(endpoint, (1205.0, 1200.0));
        assert_eq!(clipping, None);
    }

    #[test]
    fn rising_movement_stops_before_crossing_an_overhead_surface() {
        use lyracore_shared::vmap::{cast_ray, RayFlavor, TriClass, VmapTri};
        let triangles = vec![
            VmapTri {
                verts: [
                    [1200.0, 1198.0, 51.5],
                    [1206.0, 1198.0, 51.5],
                    [1200.0, 1202.0, 51.5],
                ],
                class: TriClass::M2,
            },
            VmapTri {
                verts: [
                    [1206.0, 1198.0, 51.5],
                    [1206.0, 1202.0, 51.5],
                    [1200.0, 1202.0, 51.5],
                ],
                class: TriClass::M2,
            },
        ];
        for (from, to, expected_x) in [
            ((1200.0, 1200.0, 49.0), (1205.0, 1200.0, 51.0), 1203.0),
            ((1201.0, 1200.0, 49.0), (1201.0, 1200.0, 51.0), 1201.0),
        ] {
            let (endpoint, clipping) = gate_step(from, to, |from, to| {
                cast_ray(
                    &mut |_, _| Some(triangles.clone()),
                    from,
                    to,
                    RayFlavor::Collision,
                )
                .map(|hit| (hit[0], hit[1]))
            });
            assert!((endpoint.0 - expected_x).abs() < 0.01, "{endpoint:?}");
            assert_eq!(endpoint.1, 1200.0);
            assert!(clipping.is_some());
        }
    }

    #[test]
    fn either_side_can_block_and_a_lone_side_passes_through_unchanged() {
        let mut terrain = open(80.0);
        walk_set(&mut terrain.walk, 5, 5, false);
        let mut coverage = open(80.0);
        walk_set(&mut coverage.walk, 9, 9, false);

        let merged = merged_cell(Some(terrain.clone()), Some(coverage.clone())).unwrap();
        assert!(!walk_get(&merged.walk, 5, 5) && !walk_get(&merged.walk, 9, 9));

        let terrain_only = merged_cell(Some(terrain), None).unwrap();
        assert!(!walk_get(&terrain_only.walk, 5, 5) && walk_get(&terrain_only.walk, 9, 9));

        let coverage_only = merged_cell(None, Some(coverage)).unwrap();
        assert!(walk_get(&coverage_only.walk, 5, 5) && !walk_get(&coverage_only.walk, 9, 9));

        assert!(merged_cell(None, None).is_none());
    }
}

#[test]
fn wire_rounding_keeps_clearance_at_a_tight_corner() {
    let clear = |from: (f32, f32, f32), to: (f32, f32, f32)| {
        let t = (1.125 - from.0) / (to.0 - from.0);
        !(0.0..=1.0).contains(&t) || from.1 + (to.1 - from.1) * t > 1.05
    };
    let points =
        fit_wire_path((0.0, 0.0, 0.0), &[(1.1, 1.1, 0.0), (3.0, 0.0, 0.0)], clear).unwrap();
    assert_eq!(points, vec![(1.0, 1.25, 0.0), (3.0, 0.0, 0.0)]);
    assert_eq!(
        lyracore_shared::movement_path::wire_points(&points),
        Some(points)
    );
}

#[test]
fn wire_fitting_can_move_a_later_point_before_checking_its_predecessor() {
    let clear = |from: (f32, f32, f32), to: (f32, f32, f32)| {
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let t = (((2.0 - from.0) * dx - from.1 * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        (from.0 + dx * t - 2.0).hypot(from.1 + dy * t) > 0.03
    };
    let points = fit_wire_path(
        (0.0, 0.0, 0.0),
        &[(1.0, 0.0, 0.0), (2.1, 0.1, 0.0), (4.0, 0.0, 0.0)],
        clear,
    )
    .unwrap();
    assert_eq!(
        points,
        vec![(1.0, 0.0, 0.0), (2.0, 0.25, 0.0), (4.0, 0.0, 0.0)]
    );
}
