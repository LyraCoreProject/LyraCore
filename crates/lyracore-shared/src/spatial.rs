//! Spatial helpers for visibility / interest management.
//!
//! The slice queries visibility with a bounding box over `x`/`y`, but the entity table also
//! carries pre-baked `grid_x`/`grid_y` cell columns so production can switch to grid-cell
//! interest management without a schema migration. These helpers compute those cells and the
//! box predicate consistently in both tiers.

/// Vanilla map coordinate span constant: cells are measured from +COORD_MAX downwards, the
/// same convention the client/grids use, so cell math matches existing tooling.
pub const MAP_COORD_MAX: f32 = 17066.666;

/// Side length of an AOI interest cell in yards. The gateway subscribes a `(2·BOX_HALF_SPAN+1)`-square
/// `GridBox`; the guaranteed-visible radius (player → nearest box edge, worst case at a cell boundary)
/// is `BOX_HALF_SPAN × GRID_CELL_SIZE`. Tuned for VANILLA feel: 50yd cells × span 2 (5×5) → min-visible
/// **100yd** (≈ the ~90-100yd vanilla unit view, so no pop-in), loading to ~150yd on an axis / ~212yd at
/// a corner. Finer cells + a wider span give a tighter box for the same min-visible than a coarse 3×3
/// (less cell-anchor slop). A box is a square approximation of vanilla's circle (the subscription SQL
/// can't do the `(x-px)²+(y-py)²` arithmetic), so a √2 corner over-reach is inherent; a true radius
/// would need an un-indexed `Box2` x/y filter. Both tiers bake `grid_x`/`grid_y` from this, so a change
/// needs a re-grid (`debug_regrid`).
pub const GRID_CELL_SIZE: f32 = 50.0;

/// AOI box half-span in cells: the box is the anchor cell ± this on each axis, i.e.
/// `(2·BOX_HALF_SPAN+1)²` cells. 2 → a 5×5 box. The gateway subscription AND the module's
/// movement-recipient selection both derive their range from this (via `GridBox::bounds`), so they
/// stay ALIGNED automatically (a peer is animated exactly when it's spawned).
pub const BOX_HALF_SPAN: i32 = 2;

/// Default visibility radius in yards for the slice's box predicate. Roughly the vanilla
/// active-object range; tune during bring-up.
pub const DEFAULT_VISIBILITY_RADIUS: f32 = 100.0;

/// Compute the grid cell (x, y) for a world position, matching the vanilla convention.
pub fn grid_cell(x: f32, y: f32) -> (i32, i32) {
    let gx = ((MAP_COORD_MAX - x) / GRID_CELL_SIZE).floor() as i32;
    let gy = ((MAP_COORD_MAX - y) / GRID_CELL_SIZE).floor() as i32;
    (gx, gy)
}

/// The inclusive grid-cell box `(gx0, gx1, gy0, gy1)` covering every point within `radius` yards
/// of `(x, y)` — the query shape for a `by_grid`-indexed neighborhood read.
/// `grid_cell` inverts the axis (`(MAP_COORD_MAX - v) / CELL`), so `+radius` yields the LOW cell
/// index and `-radius` the high one; monotonicity of `grid_cell` makes the two endpoints bracket
/// every intermediate cell. A non-positive (or NaN-tainted) radius degrades to the single anchor
/// cell rather than an inverted, silently-empty range.
pub fn covering_cell_box(x: f32, y: f32, radius: f32) -> (i32, i32, i32, i32) {
    let r = if radius > 0.0 { radius } else { 0.0 };
    let (gx0, gy0) = grid_cell(x + r, y + r);
    let (gx1, gy1) = grid_cell(x - r, y - r);
    (gx0, gx1, gy0, gy1)
}

/// Axis-aligned bounding box used by the slice's nearby-entity subscription predicate:
/// `map_id == m AND x BETWEEN x0..x1 AND y BETWEEN y0..y1`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Box2 {
    pub map_id: u32,
    pub x0: f32,
    pub x1: f32,
    pub y0: f32,
    pub y1: f32,
}

impl Box2 {
    /// The visibility box centered on `(x, y)` with the given radius on `map_id`.
    pub fn around(map_id: u32, x: f32, y: f32, radius: f32) -> Self {
        Self {
            map_id,
            x0: x - radius,
            x1: x + radius,
            y0: y - radius,
            y1: y + radius,
        }
    }

    /// True if `(x, y)` on `map_id` falls inside this box.
    pub fn contains(&self, map_id: u32, x: f32, y: f32) -> bool {
        map_id == self.map_id && x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }
}

/// The Area-of-Interest footprint: a `(2·BOX_HALF_SPAN+1)`-square grid-cell box anchored on one cell. The
/// gateway subscribes to `game_world_entity` rows whose `(map_id, grid_x, grid_y)` fall in this box (an
/// index-friendly range on the `by_grid` btree), and RECENTERS it (unsubscribe+resubscribe) when the
/// player crosses out of the anchor cell. Pure so the cell/box math is unit-tested + shared between tiers.
/// At `GRID_CELL_SIZE` 50yd × `BOX_HALF_SPAN` 2 the 5×5 spans ~250yd with a **100yd** guaranteed-visible
/// radius (≈ the vanilla unit view → no pop-in), culling beyond ~150yd/axis (~212yd corner). Changing the
/// cell size needs a re-grid of stored `grid_x`/`grid_y` (the `debug_regrid` reducer re-stamps in place).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridBox {
    pub map_id: u32,
    /// The anchor cell (the box is `gx-1..=gx+1`, `gy-1..=gy+1`).
    pub gx: i32,
    pub gy: i32,
}

impl GridBox {
    /// The box (anchor cell ± [`BOX_HALF_SPAN`]) anchored on the cell containing `(x, y)` on `map_id`.
    pub fn around(map_id: u32, x: f32, y: f32) -> Self {
        let (gx, gy) = grid_cell(x, y);
        Self { map_id, gx, gy }
    }

    /// Inclusive grid-cell bounds of the box (anchor ± [`BOX_HALF_SPAN`]): `(gx_min, gx_max, gy_min,
    /// gy_max)` — the WHERE-clause range the AOI subscription uses (`grid_x BETWEEN gx_min AND gx_max AND
    /// grid_y BETWEEN ...`). The SAME range drives the module's movement-recipient selection, so the
    /// gateway's spawn set and the peer-movement relay stay aligned.
    pub fn bounds(&self) -> (i32, i32, i32, i32) {
        (
            self.gx - BOX_HALF_SPAN,
            self.gx + BOX_HALF_SPAN,
            self.gy - BOX_HALF_SPAN,
            self.gy + BOX_HALF_SPAN,
        )
    }

    /// Every grid cell in the box as `(gx, gy)` — the ENUMERATION form of [`GridBox::bounds`], for a
    /// caller that probes a `(map_id, instance_id, grid_x, grid_y)` btree cell by cell instead of
    /// filtering a full scan with the four `bounds()` inequalities (the module's movement-recipient
    /// selection does exactly that). Kept here, next to `bounds()`, because the two MUST agree: the
    /// unit test asserts the emitted set is exactly the cells `bounds()` accepts, with no duplicate —
    /// a range typo would otherwise silently shrink or double a peer's movement-recipient set, which
    /// is invisible in code review and shows up live as a frozen or double-stepping peer.
    pub fn cells(self) -> impl Iterator<Item = (i32, i32)> {
        let (gx_min, gx_max, gy_min, gy_max) = self.bounds();
        (gx_min..=gx_max).flat_map(move |gx| (gy_min..=gy_max).map(move |gy| (gx, gy)))
    }

    /// Should the box RECENTER on `(map_id, x, y)`? True when the position is on a different map, or in a
    /// different cell than the anchor (the player crossed a cell boundary). The cell-change trigger keeps
    /// the player roughly centered; at 50yd a walking player crosses a boundary every ~7s, so the resubscribe
    /// rate stays low. (A ½-cell hysteresis to damp exact-boundary thrash is a refinement.)
    pub fn needs_recenter(&self, map_id: u32, x: f32, y: f32) -> bool {
        if map_id != self.map_id {
            return true;
        }
        let (gx, gy) = grid_cell(x, y);
        gx != self.gx || gy != self.gy
    }
}

/// An axis-aligned grid-cell RECTANGLE with no anchor — the per-shard split unit for cross-seam
/// view-merge (issue #73). A [`GridBox`] is always a fixed-size square around one anchor cell; a
/// `GridRect` is any inclusive `(gx_min..=gx_max) × (gy_min..=gy_max)` span, which is what a straddling
/// box's per-shard share looks like after [`crate::region`]'s rectangle boundaries cut it.
///
/// A non-straddling box's home share is exactly the whole box: [`GridRect::from_box`] and
/// [`GridBox::bounds`] emit the identical four integers, so a caller building the same 4 table
/// queries from either one produces byte-identical SQL — the property that makes "no seam in view"
/// cost nothing extra.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridRect {
    pub map_id: u32,
    pub gx_min: i32,
    pub gx_max: i32,
    pub gy_min: i32,
    pub gy_max: i32,
}

impl GridRect {
    /// The whole box, as a rect with no anchor — the identity conversion a non-straddling recenter
    /// takes.
    pub fn from_box(b: &GridBox) -> Self {
        let (gx_min, gx_max, gy_min, gy_max) = b.bounds();
        Self {
            map_id: b.map_id,
            gx_min,
            gx_max,
            gy_min,
            gy_max,
        }
    }

    /// Inclusive grid-cell bounds — the same 4-tuple shape [`GridBox::bounds`] returns, so both types
    /// can feed one query-builder.
    pub fn bounds(&self) -> (i32, i32, i32, i32) {
        (self.gx_min, self.gx_max, self.gy_min, self.gy_max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_box_bounds_and_recenter() {
        let b = GridBox::around(0, -8949.95, -132.493);
        let (xmin, xmax, ymin, ymax) = b.bounds();
        assert_eq!(xmax - xmin, 2 * BOX_HALF_SPAN); // 5x5 → anchor ±2
        assert_eq!(ymax - ymin, 2 * BOX_HALF_SPAN);
        assert_eq!((xmin + BOX_HALF_SPAN, ymin + BOX_HALF_SPAN), (b.gx, b.gy)); // anchor is the center
                                                                                // A few yards away → same cell → no recenter.
        assert!(!b.needs_recenter(0, -8945.0, -130.0));
        // A different map → recenter.
        assert!(b.needs_recenter(1, -8949.95, -132.493));
        // Well over a cell away (600yd ≫ the 50yd cell) → different cell → recenter.
        assert!(b.needs_recenter(0, -8949.95 - 600.0, -132.493));
    }

    /// #73: a non-straddling recenter's home share must be BYTE-IDENTICAL to today's whole-box
    /// query, and this is the property that makes that true — `GridRect::from_box` emits the exact
    /// same 4-tuple `GridBox::bounds` does, so a query builder driven by either type produces the
    /// same SQL string for the same box.
    #[test]
    fn grid_rect_from_box_matches_bounds_exactly() {
        let b = GridBox::around(0, -8949.95, -132.493);
        let r = GridRect::from_box(&b);
        assert_eq!((r.map_id, r.gx_min, r.gx_max, r.gy_min, r.gy_max), {
            let (x0, x1, y0, y1) = b.bounds();
            (b.map_id, x0, x1, y0, y1)
        });
        assert_eq!(r.bounds(), b.bounds());
    }

    /// `cells()` must emit EXACTLY the cell set `bounds()`'s four inequalities accept — the module's
    /// movement relay swapped a full entity scan filtered by those inequalities for one indexed probe
    /// per emitted cell, so the two forms are only interchangeable while this holds. A `..` for a
    /// `..=`, a swapped axis or a repeated cell all show up here (and nowhere else in the suite).
    #[test]
    fn grid_box_cells_are_exactly_the_cells_bounds_accepts() {
        let b = GridBox::around(0, -8949.95, -132.493);
        let (x0, x1, y0, y1) = b.bounds();
        let cells: Vec<(i32, i32)> = b.cells().collect();
        let side = (2 * BOX_HALF_SPAN + 1) as usize;
        assert_eq!(cells.len(), side * side, "the box is {side}×{side} cells");
        let unique: std::collections::HashSet<(i32, i32)> = cells.iter().copied().collect();
        assert_eq!(
            unique.len(),
            cells.len(),
            "a repeated cell would double a peer's movement rows"
        );
        // Sweep a margin around the box: membership must agree with the inequality predicate in BOTH
        // directions (a dropped edge cell = a peer that stops seeing the mover; an extra one = a
        // recipient outside the AOI box the gateway spawns from).
        for gx in (x0 - 2)..=(x1 + 2) {
            for gy in (y0 - 2)..=(y1 + 2) {
                let inside = gx >= x0 && gx <= x1 && gy >= y0 && gy <= y1;
                assert_eq!(unique.contains(&(gx, gy)), inside, "cell ({gx},{gy})");
            }
        }
        // Negative cell indices (the far quadrant of a map) enumerate identically — the anchor is
        // always emitted, and the count never changes with the sign of the anchor.
        let neg = GridBox {
            map_id: 0,
            gx: -3,
            gy: -7,
        };
        assert_eq!(neg.cells().count(), side * side);
        assert!(
            neg.cells().any(|c| c == (neg.gx, neg.gy)),
            "the anchor cell must be in its own box"
        );
    }

    #[test]
    fn box_contains_center_and_edges() {
        let b = Box2::around(0, 100.0, 200.0, 10.0);
        assert!(b.contains(0, 100.0, 200.0));
        assert!(b.contains(0, 110.0, 210.0));
        assert!(!b.contains(0, 111.0, 200.0));
        assert!(!b.contains(1, 100.0, 200.0)); // different map
    }

    #[test]
    fn covering_cell_box_brackets_the_radius() {
        // 150yd radius over 50yd cells → 7 cells per axis, anchor cell strictly inside.
        let (ax, ay) = grid_cell(0.0, 0.0);
        let (gx0, gx1, gy0, gy1) = covering_cell_box(0.0, 0.0, 150.0);
        assert_eq!((gx1 - gx0, gy1 - gy0), (6, 6));
        assert!(gx0 < ax && ax < gx1 && gy0 < ay && ay < gy1);
        // Every point inside the radius maps into the box (monotonicity, spot-checked at the rim).
        for (px, py) in [
            (150.0, 0.0),
            (-150.0, 0.0),
            (0.0, 150.0),
            (0.0, -150.0),
            (106.0, 106.0),
        ] {
            let (cx, cy) = grid_cell(px, py);
            assert!(
                gx0 <= cx && cx <= gx1 && gy0 <= cy && cy <= gy1,
                "({px},{py}) escaped the box"
            );
        }
        // Degenerate radii collapse to the single anchor cell — never an inverted/empty range.
        assert_eq!(covering_cell_box(0.0, 0.0, 0.0), (ax, ax, ay, ay));
        assert_eq!(covering_cell_box(0.0, 0.0, -5.0), (ax, ax, ay, ay));
        assert_eq!(covering_cell_box(0.0, 0.0, f32::NAN), (ax, ax, ay, ay));
    }

    #[test]
    fn grid_cells_are_stable_for_nearby_points() {
        let a = grid_cell(-8949.95, -132.493);
        let b = grid_cell(-8945.0, -130.0);
        assert_eq!(a, b); // points a few yards apart share a cell
    }

    #[test]
    fn grid_cell_pins_exact_indices_at_cell_edges() {
        // At the coordinate origin, both axes sit in cell 0.
        assert_eq!(grid_cell(MAP_COORD_MAX, MAP_COORD_MAX), (0, 0));
        // Exactly one cell size in: the boundary itself belongs to the NEXT cell (floor semantics).
        assert_eq!(
            grid_cell(MAP_COORD_MAX - GRID_CELL_SIZE, MAP_COORD_MAX),
            (1, 0)
        );
        // A hair before the boundary is still cell 0; a hair past it is already cell 1.
        assert_eq!(
            grid_cell(MAP_COORD_MAX - GRID_CELL_SIZE + 0.002, MAP_COORD_MAX),
            (0, 0)
        );
        assert_eq!(
            grid_cell(MAP_COORD_MAX - GRID_CELL_SIZE - 0.002, MAP_COORD_MAX),
            (1, 0)
        );
        // Two cells in on the y axis, independent of x.
        assert_eq!(
            grid_cell(MAP_COORD_MAX, MAP_COORD_MAX - 2.0 * GRID_CELL_SIZE),
            (0, 2)
        );
    }
}
