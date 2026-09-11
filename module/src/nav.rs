//! Nav grid: walkability + line-of-sight obstruction data (work-item 241, decision #8).
//!
//! One row per terrain MCNK cell that has ANY obstruction, steep slope, or hole — fully-clear
//! cells emit no row, so a missing chunk means "no obstacles known here" and every reader keeps
//! today's straight-line behavior (the same off-slice fallback contract as `game_terrain_chunk`).
//! Imported from the operator's own client geometry by `importer --nav` (licensing firewall).
//! Blob layout + cell math live in `lyracore_shared::nav` (one copy, unit-tested there; the importer
//! rasterizes through the SAME functions).

use lyracore_shared::nav::{self, OBS_BYTES, OBS_DIM, WALK_BYTES, WALK_DIM};
use lyracore_shared::terrain::{cell_index, cell_key};
use spacetimedb::{reducer, table, ReducerContext, Table};

/// One terrain cell's nav data. `walk` is the 64×64 walkability bitmask; `obs` is the 32×32
/// obstruction-height grid (may be EMPTY when the cell only has slope/hole unwalkability and
/// nothing tall enough to block sight). [static]
#[table(accessor = game_nav_chunk, public)]
pub struct NavChunk {
    /// (map << 32) | (cell_x << 16) | cell_y — same `cell_key` as `game_terrain_chunk`.
    #[primary_key]
    pub key: u64,
    pub map_id: u32,
    pub cell_x: u16,
    pub cell_y: u16,
    /// Obs heights quantize against this (the cell's min terrain corner height).
    pub base_z: f32,
    /// 512-byte walkability bitmask (`lyracore_shared::nav::walk_get`).
    pub walk: Vec<u8>,
    /// 1024-byte obstruction grid (`lyracore_shared::nav::obs_top`), or empty = all-clear.
    pub obs: Vec<u8>,
}

/// Terrain and navigation imports, plus effective coverage changes, advance this singleton in
/// their transaction. An absent row means the installed inputs predate revision tracking.
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

/// Identifies the inputs to movement on one map. It does not certify coverage at a position;
/// the retained RouteStep carries that evidence. Dynamic obstacles still require bounded expiry.
pub fn inputs(ctx: &ReducerContext, map_id: u32) -> NavigationInputs {
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

/// Is (x, y) standable? `None` = no nav chunk here (un-imported or fully clear) — callers keep
/// their current behavior, exactly like `terrain::ground_z`'s off-slice contract. One PK find.
pub fn walkable(ctx: &ReducerContext, map_id: u32, x: f32, y: f32) -> Option<bool> {
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

/// Obstruction top-Z over (x, y), or `None` when the column (or the whole cell) is clear.
pub fn obstruction_top(ctx: &ReducerContext, map_id: u32, x: f32, y: f32) -> Option<f32> {
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

// ===========================================================================================
//  Import reducers — blobs ride the packed-string path like terrain (`Vec<u8>` has no
//  spacetime-SQL literal): rows `;`, fields `,`, blobs hex. An EMPTY walk field means
//  "all 4096 bits set" (the common some-obs-but-fully-walkable cell, e.g. a low fence).
// ===========================================================================================

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

/// Clear + load the first nav batch (operator-only), mirroring `import_terrain_chunks`.
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

/// Append a nav batch WITHOUT the reset — a zone spans many `spacetime call` args.
#[reducer]
pub fn import_nav_chunks_append(ctx: &ReducerContext, packed: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if load_nav_batch(ctx, &packed)? > 0 {
        record_change(ctx)?;
    }
    Ok(())
}

// ===========================================================================================
//  Runtime consumption (work-item 243) — the module-side wrappers over `lyracore_shared::nav`'s
//  pure queries, gated on `game_config.nav_enabled` (default OFF; the 244 benchmark + live
//  verify flips it). Every wrapper degrades to the pre-243 straight-line behavior when the
//  flag is off or a chunk is missing. `route_step` holds position when no path exists.
// ===========================================================================================

use crate::game_config;
use crate::vmap::game_vmap_nav_coverage;
use lyracore_shared::nav::NavCellData;

/// The 243 consumption gate. Missing config row = false (fresh DB stays baseline).
pub fn nav_enabled(ctx: &ReducerContext) -> bool {
    ctx.db
        .game_config()
        .id()
        .find(0)
        .map(|c| c.nav_enabled)
        .unwrap_or(false)
}

/// The vmap-derived coverage gate. Missing config row = false, like `nav_enabled`.
pub fn nav_coverage_enabled(ctx: &ReducerContext) -> bool {
    ctx.db
        .game_config()
        .id()
        .find(0)
        .map(|c| c.nav_coverage_enabled)
        .unwrap_or(false)
}

/// The generation whose derived coverage may join planning on this map, or `None` for the
/// terrain-only grid. Needs the gate on, an ACTIVE generation, and a COMPLETE manifest — partial
/// coverage never routes, and a non-active generation's rows are invisible here.
pub(crate) fn coverage_generation(ctx: &ReducerContext, map_id: u32) -> Option<u64> {
    if !nav_coverage_enabled(ctx) {
        return None;
    }
    crate::vmap::active_vmap_nav_coverage_manifest(ctx, map_id).map(|m| m.generation_id)
}

/// Fold coverage into the imported terrain cell. Either side may be absent: a cell only the vmap
/// knows about routes off coverage alone, and a cell only the importer knows about is untouched.
fn merged_cell(terrain: Option<NavCellData>, coverage: Option<NavCellData>) -> Option<NavCellData> {
    match (terrain, coverage) {
        (Some(terrain), Some(coverage)) => Some(nav::merge_cells(&terrain, &coverage)),
        (Some(cell), None) | (None, Some(cell)) => Some(cell),
        (None, None) => None,
    }
}

/// Chunk fetch closure for the shared queries — one PK find per crossed cell, blobs cloned
/// (a few KB; the shared layer memoizes per query). With coverage consuming, each cell also costs
/// one indexed coverage lookup and a blob merge; the generation itself resolves ONCE per query
/// (the same hoist `vmap::fetcher` does with its active-generation lookup), never per cell.
fn fetcher(ctx: &ReducerContext, map_id: u32) -> impl FnMut(u16, u16) -> Option<NavCellData> + '_ {
    let generation = coverage_generation(ctx, map_id);
    move |cx, cy| fetch_cell(ctx, map_id, generation, cx, cy).0
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

/// Closed doors apply even where static geometry falls back to the coarse navigation grid.
pub fn has_los(
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

/// Maximum expansions consumed by one route step. Packages reserve this budget before movement.
pub(crate) const LEG_MAX_EXPANSIONS: u32 = 16_384;

/// Complete and Partial describe the planned route, including when collision stops this step.
#[derive(spacetimedb::SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub enum RouteStatus {
    Complete,
    Partial,
    Blocked,
    /// Navigation is disabled. The collision Gate still applies.
    Direct,
}

#[derive(spacetimedb::SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub enum CoverageEvidence {
    Unknown,
    /// Every consulted navigation cell has a matching derived row from the active generation's
    /// finalized manifest. This does not certify terrain coverage or the world import scope.
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
pub struct RouteClip {
    pub attempted: RoutePoint,
    pub hit: RoutePoint,
}

/// `endpoint == from` means no movement. Complete means a route was planned, not arrival.
#[derive(spacetimedb::SpacetimeType, Clone, Debug, PartialEq)]
pub struct RouteStep {
    pub from: RoutePoint,
    pub endpoint: RoutePoint,
    pub first_waypoint: Option<RoutePoint>,
    pub status: RouteStatus,
    pub expansions: u32,
    pub clipping: Option<RouteClip>,
    pub coverage: CoverageEvidence,
}

/// Plan one bot movement step. A failed search holds position; sparse missing rows retain
/// unknown coverage. Collision can shorten complete, partial, and direct steps independently.
#[allow(clippy::too_many_arguments)] // A movement step carries its partition, endpoints and distances.
pub fn route_step(
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
        // The search exempts its starting sub-cell and may finish without fetching it.
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
    let (endpoint, clipping) =
        step_gate_with_fetch(ctx, map_id, instance_id, cur, attempted, z, &mut fetch);
    result.endpoint = endpoint.into();
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

/// Steps toward a walkable path's first waypoint, then gates every result against geometry.
/// A missing path may aim straight at the goal, but the commit gate stops it at obstructions.
///
/// PERF NOTE: the gate fires on every committed movement step for chase/return/wander/flee/
/// pet-follow — the ~500ms movement tick, 8x more often than the ~4s sense tick the benchmark
/// (`debug_bench_los`) measured `vmap::los_ray` against. `debug_bench_collision_gate`
/// (`debug/instance.rs`) is the companion harness for THIS path — run it against the same box
/// before flipping `vmap_enabled` on a populated map, and compare its wall time to
/// `debug_bench_los(exact=true)`'s on that box.
#[allow(clippy::too_many_arguments)] // A movement step carries its partition, endpoints and distances.
pub fn nav_step(
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
            // Keep aiming at an unreachable goal; the commit gate truncates the move at geometry.
            None => crate::creatures::chase_step(cur.0, cur.1, dest.0, dest.1, max_step, stop_dist),
        }
    };
    step_gate(ctx, map_id, instance_id, cur, stepped, z)
}

/// The margin `nav_step`'s step gate stops short of a ray hit by — same value
/// `blink_forward`'s clamp uses (`BLINK_CLEARANCE_YD`), so a walked stop and a Blink stop land at
/// the same distance from a wall.
const GATE_CLEARANCE_YD: f32 = 1.0;

/// The single commit-point gate for every `nav_step` result.
/// Exact vmap collision takes precedence; the obstruction grid is the fallback when nav is enabled.
fn step_gate(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
    cur: (f32, f32),
    stepped: (f32, f32),
    z: f32,
) -> (f32, f32) {
    let mut fetch = None;
    step_gate_with_fetch(ctx, map_id, instance_id, cur, stepped, z, &mut |cx, cy| {
        fetch.get_or_insert_with(|| fetcher(ctx, map_id))(cx, cy)
    })
    .0
}

fn step_gate_with_fetch(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
    cur: (f32, f32),
    stepped: (f32, f32),
    z: f32,
    fetch: &mut impl FnMut(u16, u16) -> Option<NavCellData>,
) -> ((f32, f32), Option<RouteClip>) {
    gate_step(cur, stepped, z, |from, to| {
        let exact = crate::vmap::collision_ray(ctx, map_id, instance_id, from, to);
        let grid = if !crate::vmap::vmap_enabled(ctx, map_id) && nav_enabled(ctx) {
            // The grid query adds eye height itself, so it starts at foot height.
            lyracore_shared::nav::step_hit(fetch, (from[0], from[1], z), (to[0], to[1], z))
                .map(|p| [p.0, p.1, from[2]])
        } else {
            None
        };
        crate::vmap::nearest_hit(from, exact, grid).map(|p| (p[0], p[1]))
    })
}

fn gate_step(
    cur: (f32, f32),
    stepped: (f32, f32),
    z: f32,
    mut collision: impl FnMut([f32; 3], [f32; 3]) -> Option<(f32, f32)>,
) -> ((f32, f32), Option<RouteClip>) {
    if stepped == cur {
        return (stepped, None);
    }
    // Probe above walkable steps. A ray at foot height stops on every stair riser.
    let probe_z = z + lyracore_shared::nav::WALK_STEP_UP;
    let hit = collision([cur.0, cur.1, probe_z], [stepped.0, stepped.1, probe_z]);
    match hit {
        Some((hx, hy)) => {
            let (dx, dy) = (hx - cur.0, hy - cur.1);
            let hit_dist = (dx * dx + dy * dy).sqrt();
            let land_dist = hit_dist - GATE_CLEARANCE_YD;
            let endpoint = if land_dist <= 0.0 {
                cur // hit inside the clearance margin — hold in place
            } else {
                (
                    cur.0 + dx / hit_dist * land_dist,
                    cur.1 + dy / hit_dist * land_dist,
                )
            };
            (
                endpoint,
                Some(RouteClip {
                    attempted: stepped.into(),
                    hit: (hx, hy).into(),
                }),
            )
        }
        None => (stepped, None),
    }
}

/// Probe-only: the raw `find_leg_ex` outcome as a printable summary (waypoints + expansions),
/// tagged with the coverage generation that planned it. That tag is what makes a before/after
/// pair readable: the same leg run with the gate off and on shows how many expansions routing
/// around real geometry actually costs against `LEG_MAX_EXPANSIONS`, and `complete=false` means
/// the budget ran out and the leg is best-effort.
pub fn debug_find_leg(
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
    fn collision_evidence_retains_the_attempt_when_the_step_cannot_advance() {
        let from = (0.0, 0.0);
        let attempted = (5.0, 0.0);
        let hit = (0.5, 0.0);
        let (endpoint, clipping) = gate_step(from, attempted, 80.0, |_, _| Some(hit));
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
        let (endpoint, clipping) = gate_step(from, attempted, 80.0, |_, _| Some(hit));
        assert_eq!(endpoint, (2.0, 0.0));
        assert_eq!(
            clipping,
            Some(RouteClip {
                attempted: attempted.into(),
                hit: hit.into()
            })
        );
    }

    #[test]
    fn an_unclipped_step_has_no_collision_evidence() {
        let attempted = (5.0, 0.0);
        let (endpoint, clipping) = gate_step((0.0, 0.0), attempted, 80.0, |_, _| None);
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
            let (stepped, _) = gate_step(cur, dest, 80.0, |from, to| {
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

    /// The whole point of the read seam: an imported cell that knows nothing about a wall picks up
    /// the coverage's blockers, and coverage alone still routes where no nav row was imported.
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

        // Neither source covering the cell keeps the missing-chunk contract: no obstacles known.
        assert!(merged_cell(None, None).is_none());
    }
}
