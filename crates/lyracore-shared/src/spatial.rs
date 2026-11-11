// Cell indices count down from +MAP_COORD_MAX.
pub const MAP_COORD_MAX: f32 = 32.0 * crate::terrain::ADT_SIZE;

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
