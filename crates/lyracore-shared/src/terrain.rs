//! Terrain cell math shared by the module's `ground_z` and the importer's self-check
//! (work-items 172/173; extracted per PR-9 review — one copy of the interpolation, unit-tested
//! here, with both consumers as thin callers).
//!
//! Geometry (mangos GridDefines parity): 64×64 tiles of 533.33333 yd per map, 16 MCNK cells per
//! tile → 1024×1024 cells of 33.33 yd. Cell indices count DOWN from +`spatial::MAP_COORD_MAX`,
//! the same coords-measured-downward convention as the AOI grid — one origin for both systems.

use crate::spatial::MAP_COORD_MAX;

/// Side of one MCNK terrain cell in yards (533.33333 yd tile / 16).
pub const CELL_SIZE: f32 = 533.333_3 / 16.0;
/// 9×9 corner-grid spacing inside a cell (8 sub-quads per axis).
pub const QUAD: f32 = CELL_SIZE / 8.0;

/// World coord → global terrain cell index on one axis, or None outside the map square.
///
/// NOT `!(0.0..1024.0).contains(&c)` (what `clippy::manual_range_contains` asks for): the two differ
/// on NaN. `contains` is false for NaN, so the negation REJECTS it; the comparison chain below is
/// false for NaN, so it ACCEPTS it and `c as u16` saturates to cell 0. Keeping the comparison form
/// preserves the shipped behavior byte-for-byte, and NaN cannot reach here anyway — the movement
/// reducer fences non-finite coordinates before any terrain read (`creatures::ai::finite_point`).
#[allow(clippy::manual_range_contains)]
pub fn cell_index(coord: f32) -> Option<u16> {
    let c = (MAP_COORD_MAX - coord) / CELL_SIZE;
    if c < 0.0 || c >= 1024.0 {
        return None;
    }
    Some(c as u16)
}

/// Pack a terrain cell address into a table PK: `(map << 32) | (cell_x << 16) | cell_y`.
pub fn cell_key(map_id: u32, cell_x: u16, cell_y: u16) -> u64 {
    ((map_id as u64) << 32) | ((cell_x as u64) << 16) | cell_y as u64
}

/// Bilinear ground height at world (x, y) from one cell's 145-float MCVT height array
/// (absolute heights, raw interleaved order: 9 outer + 8 inner per row pair; outer corner
/// (i, j) lives at `heights[i*17 + j]`). Returns None if (x, y) isn't inside `(cell_x, cell_y)`
/// or the array is malformed.
// Deliberate simplification: corner bilinear, not mangos' V8 triangle scheme — worst-case error
// on a 4.17 yd sub-quad is centimeters, invisible at wander scale; the 8x8 centers are stored for
// the upgrade.
pub fn interpolate(heights: &[f32], cell_x: u16, cell_y: u16, x: f32, y: f32) -> Option<f32> {
    if heights.len() != 145 {
        return None;
    }
    if cell_index(x) != Some(cell_x) || cell_index(y) != Some(cell_y) {
        return None;
    }
    let lx = ((MAP_COORD_MAX - x) - cell_x as f32 * CELL_SIZE) / QUAD;
    let ly = ((MAP_COORD_MAX - y) - cell_y as f32 * CELL_SIZE) / QUAD;
    let (ix, iy) = ((lx as usize).min(7), (ly as usize).min(7));
    let (fx, fy) = (lx - ix as f32, ly - iy as f32);
    let corner = |i: usize, j: usize| heights[i * 17 + j];
    let top = corner(ix, iy) * (1.0 - fy) + corner(ix, iy + 1) * fy;
    let bot = corner(ix + 1, iy) * (1.0 - fy) + corner(ix + 1, iy + 1) * fy;
    Some(top * (1.0 - fx) + bot * fx)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cell whose outer corners encode a plane z = row index recovers the plane everywhere,
    /// exercised through real world coordinates (not just local quad units).
    #[test]
    fn bilinear_recovers_a_sloped_plane() {
        let mut h = vec![0.0f32; 145];
        for i in 0..9 {
            for j in 0..9 {
                h[i * 17 + j] = i as f32;
            }
        }
        // Use the cell containing the Northshire start.
        let (x0, y0) = (-8949.95f32, -132.493f32);
        let (cx, cy) = (cell_index(x0).unwrap(), cell_index(y0).unwrap());
        // Cell-corner world coords (cells count DOWN from MAP_COORD_MAX).
        let corner_x = MAP_COORD_MAX - cx as f32 * CELL_SIZE;
        let corner_y = MAP_COORD_MAX - cy as f32 * CELL_SIZE;
        // Halfway down the cell in x → local row 4 → plane value 4.
        let z = interpolate(
            &h,
            cx,
            cy,
            corner_x - CELL_SIZE / 2.0,
            corner_y - CELL_SIZE / 4.0,
        );
        assert!((z.unwrap() - 4.0).abs() < 1e-3);
    }

    /// A flat cell at the known-good Northshire height reproduces exactly at any sample point.
    #[test]
    fn flat_cell_is_exact() {
        let h = vec![83.5312f32; 145];
        let (x0, y0) = (-8949.95f32, -132.493f32);
        let (cx, cy) = (cell_index(x0).unwrap(), cell_index(y0).unwrap());
        let z = interpolate(&h, cx, cy, x0, y0).unwrap();
        assert!((z - 83.5312).abs() < 1e-4);
    }

    /// Out-of-cell coords and malformed arrays return None instead of interpolating garbage.
    #[test]
    fn rejects_wrong_cell_and_bad_array() {
        let h = vec![10.0f32; 145];
        assert!(interpolate(&h, 0, 0, -8949.95, -132.493).is_none()); // wrong cell
        assert!(interpolate(&vec![10.0f32; 144], 779, 516, -8905.0, -153.0).is_none()); // short array
        assert!(cell_index(20000.0).is_none()); // off the map square
    }
}
