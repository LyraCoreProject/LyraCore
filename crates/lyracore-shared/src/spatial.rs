// Cell indices count down from +MAP_COORD_MAX.
pub const MAP_COORD_MAX: f32 = 17066.666;

// AOI cell side length in yards.
pub const GRID_CELL_SIZE: f32 = 50.0;

pub const BOX_HALF_SPAN: i32 = 2;

// An AOI viewer keeps its cell until it is this far past the edge, so border jitter keeps Grants.
pub const AOI_HYSTERESIS_YD: f32 = 10.0;

pub const DEFAULT_VISIBILITY_RADIUS: f32 = 100.0;

pub fn grid_cell(x: f32, y: f32) -> (i32, i32) {
    let gx = ((MAP_COORD_MAX - x) / GRID_CELL_SIZE).floor() as i32;
    let gy = ((MAP_COORD_MAX - y) / GRID_CELL_SIZE).floor() as i32;
    (gx, gy)
}

// SpacetimeDB 2.14.0 needs one equality query per packed cell for indexed AOI reads.
pub fn grid_cell_id(gx: i32, gy: i32) -> i64 {
    ((gx as i64) << 32) | (gy as u32 as i64)
}

pub fn grid_cell_of_id(id: i64) -> (i32, i32) {
    ((id >> 32) as i32, (id as u32) as i32)
}

/// Whether (x, y) lies inside `cell` widened by `margin` yards on every side.
pub fn near_cell(cell: i64, x: f32, y: f32, margin: f32) -> bool {
    let (gx, gy) = grid_cell_of_id(cell);
    let m = margin / GRID_CELL_SIZE;
    let inside = |coord: f32, g: i32| {
        let f = (MAP_COORD_MAX - coord) / GRID_CELL_SIZE;
        f >= g as f32 - m && f < (g + 1) as f32 + m
    };
    inside(x, gx) && inside(y, gy)
}

pub fn cell_id_at(x: f32, y: f32) -> i64 {
    let (gx, gy) = grid_cell(x, y);
    grid_cell_id(gx, gy)
}

pub fn covering_cell_box(x: f32, y: f32, radius: f32) -> (i32, i32, i32, i32) {
    let r = if radius > 0.0 { radius } else { 0.0 };
    let (gx0, gy0) = grid_cell(x + r, y + r);
    let (gx1, gy1) = grid_cell(x - r, y - r);
    (gx0, gx1, gy0, gy1)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Box2 {
    pub map_id: u32,
    pub x0: f32,
    pub x1: f32,
    pub y0: f32,
    pub y1: f32,
}

impl Box2 {
    pub fn around(map_id: u32, x: f32, y: f32, radius: f32) -> Self {
        Self {
            map_id,
            x0: x - radius,
            x1: x + radius,
            y0: y - radius,
            y1: y + radius,
        }
    }

    pub fn contains(&self, map_id: u32, x: f32, y: f32) -> bool {
        map_id == self.map_id && x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridBox {
    pub map_id: u32,
    pub gx: i32,
    pub gy: i32,
}

impl GridBox {
    pub fn around(map_id: u32, x: f32, y: f32) -> Self {
        let (gx, gy) = grid_cell(x, y);
        Self { map_id, gx, gy }
    }

    pub fn bounds(&self) -> (i32, i32, i32, i32) {
        (
            self.gx - BOX_HALF_SPAN,
            self.gx + BOX_HALF_SPAN,
            self.gy - BOX_HALF_SPAN,
            self.gy + BOX_HALF_SPAN,
        )
    }

    pub fn cells(self) -> impl Iterator<Item = (i32, i32)> {
        let (gx_min, gx_max, gy_min, gy_max) = self.bounds();
        (gx_min..=gx_max).flat_map(move |gx| (gy_min..=gy_max).map(move |gy| (gx, gy)))
    }

    pub fn cell_ids(self) -> impl Iterator<Item = i64> {
        self.cells().map(|(gx, gy)| grid_cell_id(gx, gy))
    }

    pub fn needs_recenter(&self, map_id: u32, x: f32, y: f32) -> bool {
        if map_id != self.map_id {
            return true;
        }
        let (gx, gy) = grid_cell(x, y);
        gx != self.gx || gy != self.gy
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridRect {
    pub map_id: u32,
    pub gx_min: i32,
    pub gx_max: i32,
    pub gy_min: i32,
    pub gy_max: i32,
}

impl GridRect {
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

    pub fn bounds(&self) -> (i32, i32, i32, i32) {
        (self.gx_min, self.gx_max, self.gy_min, self.gy_max)
    }

    pub fn cell_ids(self) -> impl Iterator<Item = i64> {
        (self.gx_min..=self.gx_max)
            .flat_map(move |gx| (self.gy_min..=self.gy_max).map(move |gy| grid_cell_id(gx, gy)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_cell_widens_the_cell_by_the_margin() {
        let cell = cell_id_at(-8920.0, -180.0);
        let (gx, _) = grid_cell_of_id(cell);
        let edge = MAP_COORD_MAX - (gx + 1) as f32 * GRID_CELL_SIZE;
        assert!(near_cell(cell, -8920.0, -180.0, 0.0));
        assert!(!near_cell(cell, edge - 1.0, -180.0, 0.0));
        assert!(near_cell(cell, edge - 9.0, -180.0, AOI_HYSTERESIS_YD));
        assert!(!near_cell(cell, edge - 11.0, -180.0, AOI_HYSTERESIS_YD));
    }

    #[test]
    fn grid_box_bounds_and_recenter() {
        let b = GridBox::around(0, -8949.95, -132.493);
        let (xmin, xmax, ymin, ymax) = b.bounds();
        assert_eq!(xmax - xmin, 2 * BOX_HALF_SPAN);
        assert_eq!(ymax - ymin, 2 * BOX_HALF_SPAN);
        assert_eq!((xmin + BOX_HALF_SPAN, ymin + BOX_HALF_SPAN), (b.gx, b.gy));
        assert!(!b.needs_recenter(0, -8945.0, -130.0));
        assert!(b.needs_recenter(1, -8949.95, -132.493));
        assert!(b.needs_recenter(0, -8949.95 - 600.0, -132.493));
    }

    #[test]
    fn northshire_abbey_falls_in_grid_cell_520_343() {
        assert_eq!(grid_cell(-8949.95, -132.493), (520, 343));
        let b = GridBox::around(0, -8949.95, -132.493);
        assert_eq!(b.bounds(), (518, 522, 341, 345));
        assert_eq!(GridRect::from_box(&b).bounds(), (518, 522, 341, 345));
    }

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
        for gx in (x0 - 2)..=(x1 + 2) {
            for gy in (y0 - 2)..=(y1 + 2) {
                let inside = gx >= x0 && gx <= x1 && gy >= y0 && gy <= y1;
                assert_eq!(unique.contains(&(gx, gy)), inside, "cell ({gx},{gy})");
            }
        }
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
    fn grid_cell_id_round_trips_and_never_collides() {
        let interesting = [
            0i32,
            1,
            -1,
            2,
            -2,
            682,
            -682,
            0xFFFF,
            -0xFFFF,
            i32::MAX,
            i32::MIN,
            i32::MAX - 1,
            i32::MIN + 1,
        ];
        let mut seen = std::collections::HashMap::new();
        for gx in interesting {
            for gy in interesting {
                let id = grid_cell_id(gx, gy);
                assert_eq!(
                    grid_cell_of_id(id),
                    (gx, gy),
                    "({gx},{gy}) did not round-trip through {id}"
                );
                assert_eq!(
                    seen.insert(id, (gx, gy)),
                    None,
                    "id {id} collided: ({gx},{gy}) and {:?}",
                    seen.get(&id)
                );
            }
        }
        assert_eq!(grid_cell_id(0, -1), 0x0000_0000_FFFF_FFFFu64 as i64);
        assert_eq!(grid_cell_id(-1, 0), -1i64 << 32);
        assert_ne!(grid_cell_id(1, 0), grid_cell_id(0, 1));
        let mut all = std::collections::HashSet::new();
        for gx in -5..=690 {
            for gy in -5..=690 {
                assert!(all.insert(grid_cell_id(gx, gy)), "duplicate at ({gx},{gy})");
            }
        }
        assert_eq!(all.len(), 696 * 696);
    }

    #[test]
    fn cell_id_at_packs_the_cell_of_known_coordinates() {
        for ((x, y), (gx, gy)) in [
            ((-8949.95f32, -132.493f32), (520, 343)),
            ((0.0, 0.0), (341, 341)),
            ((MAP_COORD_MAX, MAP_COORD_MAX), (0, 0)),
            (
                (MAP_COORD_MAX + 1000.0, -MAP_COORD_MAX - 1000.0),
                (-20, 702),
            ),
            ((1600.0, -4400.0), (309, 429)),
        ] {
            assert_eq!(grid_cell(x, y), (gx, gy), "at ({x},{y})");
            assert_eq!(
                cell_id_at(x, y),
                (i64::from(gx) << 32) | i64::from(gy as u32)
            );
        }
    }

    #[test]
    fn box_and_rect_cell_ids_are_exactly_the_packed_cells() {
        let b = GridBox::around(0, -8949.95, -132.493);
        let got: Vec<i64> = b.cell_ids().collect();
        let side = (2 * BOX_HALF_SPAN + 1) as usize;
        assert_eq!(got.len(), side * side);
        assert_eq!(got[0], (518 << 32) | 341);
        assert_eq!(got[1], (518 << 32) | 342);
        assert_eq!(got[side], (519 << 32) | 341);
        assert_eq!(got[side * side - 1], (522 << 32) | 345);
        assert_eq!(
            got.iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            got.len(),
            "a duplicate id inside one box would double-deliver a cell"
        );
        assert_eq!(
            GridRect::from_box(&b).cell_ids().collect::<Vec<_>>(),
            got,
            "rect-of-a-whole-box must equal the box's own id sequence"
        );
        let neg = GridBox {
            map_id: 0,
            gx: -3,
            gy: -7,
        };
        assert_eq!(neg.cell_ids().count(), side * side);
        assert!(neg.cell_ids().any(|id| id == grid_cell_id(neg.gx, neg.gy)));
    }

    #[test]
    fn box_contains_center_and_edges() {
        let b = Box2::around(0, 100.0, 200.0, 10.0);
        assert!(b.contains(0, 100.0, 200.0));
        assert!(b.contains(0, 110.0, 210.0));
        assert!(!b.contains(0, 111.0, 200.0));
        assert!(!b.contains(1, 100.0, 200.0));
    }

    #[test]
    fn covering_cell_box_brackets_the_radius() {
        let (ax, ay) = grid_cell(0.0, 0.0);
        let (gx0, gx1, gy0, gy1) = covering_cell_box(0.0, 0.0, 150.0);
        assert_eq!((gx1 - gx0, gy1 - gy0), (6, 6));
        assert!(gx0 < ax && ax < gx1 && gy0 < ay && ay < gy1);
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
        assert_eq!(covering_cell_box(0.0, 0.0, 0.0), (ax, ax, ay, ay));
        assert_eq!(covering_cell_box(0.0, 0.0, -5.0), (ax, ax, ay, ay));
        assert_eq!(covering_cell_box(0.0, 0.0, f32::NAN), (ax, ax, ay, ay));
    }

    #[test]
    fn grid_cells_are_stable_for_nearby_points() {
        let a = grid_cell(-8949.95, -132.493);
        let b = grid_cell(-8945.0, -130.0);
        assert_eq!(a, b);
    }

    #[test]
    fn grid_cell_pins_exact_indices_at_cell_edges() {
        assert_eq!(grid_cell(MAP_COORD_MAX, MAP_COORD_MAX), (0, 0));
        assert_eq!(
            grid_cell(MAP_COORD_MAX - GRID_CELL_SIZE, MAP_COORD_MAX),
            (1, 0)
        );
        assert_eq!(
            grid_cell(MAP_COORD_MAX - GRID_CELL_SIZE + 0.002, MAP_COORD_MAX),
            (0, 0)
        );
        assert_eq!(
            grid_cell(MAP_COORD_MAX - GRID_CELL_SIZE - 0.002, MAP_COORD_MAX),
            (1, 0)
        );
        assert_eq!(
            grid_cell(MAP_COORD_MAX, MAP_COORD_MAX - 2.0 * GRID_CELL_SIZE),
            (0, 2)
        );
    }
}
