use crate::spatial::MAP_COORD_MAX;

// MCNK cell side length in yards.
pub const CELL_SIZE: f32 = 533.333_3 / 16.0;
// Nine outer corners per axis span eight terrain quads.
pub const QUAD: f32 = CELL_SIZE / 8.0;

// Comparison chains preserve legacy NaN acceptance; Range::contains would refuse it.
#[allow(clippy::manual_range_contains)]
pub fn cell_index(coord: f32) -> Option<u16> {
    let c = (MAP_COORD_MAX - coord) / CELL_SIZE;
    if c < 0.0 || c >= 1024.0 {
        return None;
    }
    Some(c as u16)
}

pub fn cell_key(map_id: u32, cell_x: u16, cell_y: u16) -> u64 {
    ((map_id as u64) << 32) | ((cell_x as u64) << 16) | cell_y as u64
}

// MCVT has 145 floats: nine outer and eight inner heights per row pair.
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

// MCLQ tile flag bit 0x80 marks deep water; a low nibble of 0x0F marks a tile without liquid.
pub fn deep_water_tiles(tile_flags: &[u8; 64]) -> u64 {
    tile_flags
        .iter()
        .enumerate()
        .filter(|(_, flags)| **flags & 0x0F != 0x0F && **flags & 0x80 != 0)
        .fold(0, |mask, (tile, _)| mask | 1 << tile)
}

// Tiles follow the MCVT row order: eight rows along x, eight tiles along y.
pub fn is_deep_water_at(deep_water_tiles: u64, cell_x: u16, cell_y: u16, x: f32, y: f32) -> bool {
    if cell_index(x) != Some(cell_x) || cell_index(y) != Some(cell_y) {
        return false;
    }
    let row = ((((MAP_COORD_MAX - x) - cell_x as f32 * CELL_SIZE) / QUAD) as u32).min(7);
    let column = ((((MAP_COORD_MAX - y) - cell_y as f32 * CELL_SIZE) / QUAD) as u32).min(7);
    deep_water_tiles & 1 << (row * 8 + column) != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_water_tiles_keep_flagged_liquid_tiles_only() {
        let mut flags = [0u8; 64];
        flags[0] = 0x80;
        flags[9] = 0x84;
        flags[10] = 0x8F;
        flags[63] = 0x04;
        assert_eq!(deep_water_tiles(&flags), 1 | 1 << 9);
    }

    #[test]
    fn deep_water_lookup_finds_the_tile_under_the_position() {
        let (x0, y0) = (-8949.95f32, -132.493f32);
        let (cx, cy) = (cell_index(x0).unwrap(), cell_index(y0).unwrap());
        let corner_x = MAP_COORD_MAX - cx as f32 * CELL_SIZE;
        let corner_y = MAP_COORD_MAX - cy as f32 * CELL_SIZE;
        let row_two_column_five = 1 << (2 * 8 + 5);
        let (x, y) = (corner_x - 2.5 * QUAD, corner_y - 5.5 * QUAD);
        assert!(is_deep_water_at(row_two_column_five, cx, cy, x, y));
        assert!(!is_deep_water_at(row_two_column_five, cx, cy, x - QUAD, y));
        assert!(!is_deep_water_at(row_two_column_five, cx, cy, x, y - QUAD));
        assert!(!is_deep_water_at(u64::MAX, cx + 1, cy, x, y));
    }

    #[test]
    fn bilinear_recovers_a_sloped_plane() {
        let mut h = vec![0.0f32; 145];
        for i in 0..9 {
            for j in 0..9 {
                h[i * 17 + j] = i as f32;
            }
        }
        let (x0, y0) = (-8949.95f32, -132.493f32);
        let (cx, cy) = (cell_index(x0).unwrap(), cell_index(y0).unwrap());
        let corner_x = MAP_COORD_MAX - cx as f32 * CELL_SIZE;
        let corner_y = MAP_COORD_MAX - cy as f32 * CELL_SIZE;
        let z = interpolate(
            &h,
            cx,
            cy,
            corner_x - CELL_SIZE / 2.0,
            corner_y - CELL_SIZE / 4.0,
        );
        assert!((z.unwrap() - 4.0).abs() < 1e-3);
    }

    #[test]
    fn flat_cell_is_exact() {
        let h = vec![83.5312f32; 145];
        let (x0, y0) = (-8949.95f32, -132.493f32);
        let (cx, cy) = (cell_index(x0).unwrap(), cell_index(y0).unwrap());
        let z = interpolate(&h, cx, cy, x0, y0).unwrap();
        assert!((z - 83.5312).abs() < 1e-4);
    }

    #[test]
    fn rejects_wrong_cell_and_bad_array() {
        let h = vec![10.0f32; 145];
        assert!(interpolate(&h, 0, 0, -8949.95, -132.493).is_none());
        assert!(interpolate(&vec![10.0f32; 144], 779, 516, -8905.0, -153.0).is_none());
        assert!(cell_index(20000.0).is_none());
    }
}
