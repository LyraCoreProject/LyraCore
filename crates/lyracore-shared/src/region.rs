use crate::spatial::{grid_cell, GRID_CELL_SIZE};

// Region zero uses the ordinary map and instance Shard map.
pub const DEFAULT_REGION: u32 = 0;

pub const MIN_REGION_SIDE_CELLS: i32 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub map_id: u32,
    pub region_id: u32,
    pub gx_min: i32,
    pub gx_max: i32,
    pub gy_min: i32,
    pub gy_max: i32,
}

impl Region {
    pub fn contains_cell(&self, map_id: u32, gx: i32, gy: i32) -> bool {
        map_id == self.map_id
            && gx >= self.gx_min
            && gx <= self.gx_max
            && gy >= self.gy_min
            && gy <= self.gy_max
    }

    pub fn size_cells(&self) -> (i32, i32) {
        let side = |lo: i32, hi: i32| hi.saturating_sub(lo).saturating_add(1);
        (
            side(self.gx_min, self.gx_max),
            side(self.gy_min, self.gy_max),
        )
    }

    pub fn size_yards(&self) -> (f32, f32) {
        let (w, h) = self.size_cells();
        (w as f32 * GRID_CELL_SIZE, h as f32 * GRID_CELL_SIZE)
    }

    // Overlapping regions would make routing depend on subscription row order.
    pub fn overlaps(&self, other: &Region) -> bool {
        self.map_id == other.map_id
            && self.gx_min <= other.gx_max
            && other.gx_min <= self.gx_max
            && self.gy_min <= other.gy_max
            && other.gy_min <= self.gy_max
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RegionMap {
    regions: Vec<Region>,
}

impl RegionMap {
    pub fn build(rows: impl IntoIterator<Item = Region>) -> (Self, Vec<String>) {
        let mut regions: Vec<Region> = Vec::new();
        let mut rejected = Vec::new();
        for r in rows {
            let (w, h) = r.size_cells();
            let label = format!("map {} region {}", r.map_id, r.region_id);
            if r.region_id == DEFAULT_REGION {
                rejected.push(format!(
                    "{label}: region id {DEFAULT_REGION} is reserved for \"the rest of the map\""
                ));
            } else if r.gx_max < r.gx_min || r.gy_max < r.gy_min {
                rejected.push(format!("{label}: inverted bounds"));
            } else if w < MIN_REGION_SIDE_CELLS || h < MIN_REGION_SIDE_CELLS {
                rejected.push(format!(
                    "{label}: {w}×{h} cells is under the {MIN_REGION_SIDE_CELLS}-cell floor (a region \
                     smaller than the interaction radius is all seam)"
                ));
            } else if let Some(dup) = regions
                .iter()
                .find(|x| x.map_id == r.map_id && x.region_id == r.region_id)
            {
                rejected.push(format!("{label}: duplicate definition (already {dup:?})"));
            } else if let Some(hit) = regions.iter().find(|x| x.overlaps(&r)) {
                rejected.push(format!(
                    "{label}: overlaps region {} on the same map",
                    hit.region_id
                ));
            } else {
                regions.push(r);
            }
        }
        (Self { regions }, rejected)
    }

    // Format: map:region = gx_min..gx_max, gy_min..gy_max; # starts a line comment.
    pub fn parse(text: &str) -> (Self, Vec<String>) {
        let mut rows = Vec::new();
        let mut rejected = Vec::new();
        for raw in text
            .split('\n')
            .flat_map(|l| l.split('#').next().unwrap_or("").split(';'))
        {
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            match parse_region_line(line) {
                Some(r) => rows.push(r),
                None => rejected.push(format!(
                    "{line:?}: want `<map_id>:<region_id>=<gx_min>..<gx_max>,<gy_min>..<gy_max>`"
                )),
            }
        }
        let (map, mut build_rejects) = Self::build(rows);
        rejected.append(&mut build_rejects);
        (map, rejected)
    }

    pub fn region_of(&self, map_id: u32, gx: i32, gy: i32) -> u32 {
        self.regions
            .iter()
            .find(|r| r.contains_cell(map_id, gx, gy))
            .map_or(DEFAULT_REGION, |r| r.region_id)
    }

    pub fn region_at(&self, map_id: u32, x: f32, y: f32) -> u32 {
        let (gx, gy) = grid_cell(x, y);
        self.region_of(map_id, gx, gy)
    }

    pub fn regions(&self) -> &[Region] {
        &self.regions
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    pub fn count_by_region(
        &self,
        positions: impl IntoIterator<Item = (u32, i32, i32)>,
    ) -> Vec<(u32, u32, u32)> {
        let mut counts: std::collections::BTreeMap<(u32, u32), u32> =
            std::collections::BTreeMap::new();
        for (map_id, gx, gy) in positions {
            let region_id = self.region_of(map_id, gx, gy);
            if region_id == DEFAULT_REGION {
                continue;
            }
            *counts.entry((map_id, region_id)).or_insert(0) += 1;
        }
        counts
            .into_iter()
            .map(|((map_id, region_id), n)| (map_id, region_id, n))
            .collect()
    }
}

fn parse_region_line(line: &str) -> Option<Region> {
    let (id_part, rect) = line.split_once('=')?;
    let (map, region) = id_part.split_once(':')?;
    let (xs, ys) = rect.split_once(',')?;
    let (gx_min, gx_max) = parse_range(xs)?;
    let (gy_min, gy_max) = parse_range(ys)?;
    Some(Region {
        map_id: map.trim().parse().ok()?,
        region_id: region.trim().parse().ok()?,
        gx_min,
        gx_max,
        gy_min,
        gy_max,
    })
}

fn parse_range(s: &str) -> Option<(i32, i32)> {
    let (a, b) = s.trim().split_once("..")?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(map_id: u32, region_id: u32, gx: i32, gy: i32) -> Region {
        Region {
            map_id,
            region_id,
            gx_min: gx,
            gx_max: gx + 19,
            gy_min: gy,
            gy_max: gy + 19,
        }
    }

    #[test]
    fn an_empty_region_map_answers_the_default_region_everywhere() {
        let m = RegionMap::default();
        assert!(m.is_empty());
        assert_eq!(m.region_of(0, 0, 0), DEFAULT_REGION);
        assert_eq!(m.region_of(0, -9999, 12345), DEFAULT_REGION);
        assert_eq!(m.region_at(0, -8949.95, -132.493), DEFAULT_REGION);
    }

    #[test]
    fn region_of_is_a_cell_rectangle_lookup_with_inclusive_bounds() {
        let (m, rejected) = RegionMap::build([rect(0, 1, 100, 200)]);
        assert!(rejected.is_empty(), "{rejected:?}");
        assert_eq!(m.region_of(0, 100, 200), 1, "the low corner is INSIDE");
        assert_eq!(
            m.region_of(0, 119, 219),
            1,
            "the high corner is INSIDE (inclusive)"
        );
        assert_eq!(m.region_of(0, 110, 210), 1);
        assert_eq!(
            m.region_of(0, 99, 210),
            DEFAULT_REGION,
            "one cell west is outside"
        );
        assert_eq!(
            m.region_of(0, 120, 210),
            DEFAULT_REGION,
            "one cell east is outside"
        );
        assert_eq!(m.region_of(0, 110, 199), DEFAULT_REGION);
        assert_eq!(m.region_of(0, 110, 220), DEFAULT_REGION);
        assert_eq!(
            m.region_of(1, 110, 210),
            DEFAULT_REGION,
            "a different map is a different world"
        );
    }

    #[test]
    fn region_at_agrees_with_the_cell_math_both_tiers_already_bake() {
        let (m, _) = RegionMap::build([rect(0, 7, 515, 338)]);
        assert_eq!(m.region_at(0, -8949.95, -132.493), 7);
        assert_eq!(m.region_of(0, 520, 343), 7);
        assert_eq!(
            m.region_at(0, -8949.95 + 40.0 * GRID_CELL_SIZE, -132.493),
            DEFAULT_REGION
        );
    }

    #[test]
    fn a_region_under_the_interaction_radius_floor_is_rejected() {
        let small = Region {
            map_id: 0,
            region_id: 1,
            gx_min: 0,
            gx_max: 8,
            gy_min: 0,
            gy_max: 8,
        };
        let (m, rejected) = RegionMap::build([small]);
        assert_eq!(rejected.len(), 1, "{rejected:?}");
        assert!(rejected[0].contains("floor"), "{}", rejected[0]);
        assert_eq!(
            m.region_of(0, 4, 4),
            DEFAULT_REGION,
            "the rejected row routes nothing"
        );
        let at_floor = Region {
            map_id: 0,
            region_id: 1,
            gx_min: 0,
            gx_max: MIN_REGION_SIDE_CELLS - 1,
            gy_min: 0,
            gy_max: MIN_REGION_SIDE_CELLS - 1,
        };
        let (m, rejected) = RegionMap::build([at_floor]);
        assert!(rejected.is_empty(), "{rejected:?}");
        assert_eq!(m.region_of(0, 4, 4), 1);
        assert_eq!(
            at_floor.size_cells(),
            (MIN_REGION_SIDE_CELLS, MIN_REGION_SIDE_CELLS)
        );
        assert_eq!(MIN_REGION_SIDE_CELLS, 10, "the spec's ~10×10 floor");
    }

    #[test]
    fn overlapping_definitions_are_rejected_so_lookup_never_depends_on_row_order() {
        let (m, rejected) = RegionMap::build([rect(0, 1, 100, 200), rect(0, 2, 110, 210)]);
        assert_eq!(rejected.len(), 1, "{rejected:?}");
        assert!(rejected[0].contains("overlaps"), "{}", rejected[0]);
        assert_eq!(m.regions().len(), 1);
        assert_eq!(
            m.region_of(0, 115, 215),
            1,
            "the FIRST definition keeps the contested cells"
        );
        let (m, rejected) = RegionMap::build([rect(0, 1, 100, 200), rect(0, 2, 120, 200)]);
        assert!(rejected.is_empty(), "{rejected:?}");
        assert_eq!((m.region_of(0, 119, 205), m.region_of(0, 120, 205)), (1, 2));
        let (m, rejected) = RegionMap::build([rect(0, 1, 100, 200), rect(1, 1, 100, 200)]);
        assert!(rejected.is_empty(), "{rejected:?}");
        assert_eq!((m.region_of(0, 105, 205), m.region_of(1, 105, 205)), (1, 1));
    }

    #[test]
    fn region_zero_and_duplicate_ids_and_inverted_bounds_are_rejected() {
        let (m, rejected) = RegionMap::build([
            rect(0, DEFAULT_REGION, 100, 200),
            rect(0, 1, 400, 400),
            rect(0, 1, 500, 500),
            Region {
                map_id: 0,
                region_id: 9,
                gx_min: 50,
                gx_max: 10,
                gy_min: 0,
                gy_max: 99,
            },
        ]);
        assert_eq!(rejected.len(), 3, "{rejected:?}");
        assert!(rejected[0].contains("reserved"), "{}", rejected[0]);
        assert!(rejected[1].contains("duplicate"), "{}", rejected[1]);
        assert!(rejected[2].contains("inverted"), "{}", rejected[2]);
        assert_eq!(m.regions().len(), 1, "only the one good row survives");
        assert_eq!(m.region_of(0, 405, 405), 1);
        assert_eq!(
            m.region_of(0, 505, 505),
            DEFAULT_REGION,
            "the duplicate claimed nothing"
        );
    }

    #[test]
    fn the_seam_menu_format_parses_comments_blanks_and_whitespace() {
        let (m, rejected) = RegionMap::parse(
            "# Elwynn Forest seam menu\n\
             \n\
             0:1 = 460..479, 300..319   # Goldshire basin\n\
             0:2=480..499,300..319\n\
             ; 0:3 = 460..479, 320..339\n",
        );
        assert!(rejected.is_empty(), "{rejected:?}");
        assert_eq!(m.regions().len(), 3);
        assert_eq!(m.region_of(0, 465, 305), 1);
        assert_eq!(m.region_of(0, 485, 305), 2);
        assert_eq!(m.region_of(0, 465, 325), 3);
        assert_eq!(
            m.region_of(0, 485, 325),
            DEFAULT_REGION,
            "the menu has a hole; holes are legal"
        );
        assert_eq!(m.regions()[0].size_yards(), (1000.0, 1000.0));
    }

    #[test]
    fn an_absurdly_large_rectangle_is_answered_not_panicked_at() {
        let (m, rejected) = RegionMap::parse("0:1 = -2147483648..2147483647, 0..99\n");
        assert!(rejected.is_empty(), "{rejected:?}");
        assert_eq!(m.region_of(0, 0, 50), 1);
        assert_eq!(m.region_of(0, 0, 100), DEFAULT_REGION);
        let (m, rejected) = RegionMap::parse("0:1 = 2147483647..-2147483648, 0..99\n");
        assert_eq!(rejected.len(), 1, "{rejected:?}");
        assert!(m.is_empty());
    }

    #[test]
    fn count_by_region_buckets_positions_and_drops_the_default_region() {
        let (m, rejected) = RegionMap::build([rect(0, 1, 100, 200), rect(0, 2, 400, 400)]);
        assert!(rejected.is_empty(), "{rejected:?}");
        let counts = m.count_by_region([
            (0, 105, 205),
            (0, 106, 206),
            (0, 405, 405),
            (0, 0, 0),
            (1, 105, 205),
        ]);
        assert_eq!(
            counts,
            vec![(0, 1, 2), (0, 2, 1)],
            "sorted by (map_id, region_id)"
        );
    }

    #[test]
    fn count_by_region_is_empty_for_no_positions_or_an_unimported_map() {
        let empty_map = RegionMap::default();
        assert_eq!(empty_map.count_by_region([(0, 1, 2)]), vec![]);
        let (m, _) = RegionMap::build([rect(0, 1, 100, 200)]);
        assert_eq!(m.count_by_region(std::iter::empty()), vec![]);
    }

    #[test]
    fn a_semicolon_inside_a_comment_does_not_start_a_new_row() {
        let (m, rejected) = RegionMap::parse(
            "# region 1 is Elwynn; region 2 is the capital; both are inert until assigned\n\
             0:1 = 460..479, 300..319   # touching, not overlapping; that is a seam\n",
        );
        assert!(rejected.is_empty(), "{rejected:?}");
        assert_eq!(m.regions().len(), 1);
        assert_eq!(m.region_of(0, 465, 305), 1);
        let (m, rejected) = RegionMap::parse("0:1=460..479,300..319; 0:2=480..499,300..319 # both");
        assert!(rejected.is_empty(), "{rejected:?}");
        assert_eq!(m.regions().len(), 2);
    }

    #[test]
    fn a_malformed_seam_menu_line_is_dropped_not_guessed_at() {
        let (m, rejected) = RegionMap::parse(
            "nonsense\n0:=1..2,3..4\n0:1=1..2\n0:1=a..b,c..d\n0:1 = 460..479, 300..319\n",
        );
        assert_eq!(rejected.len(), 4, "{rejected:?}");
        assert_eq!(m.regions().len(), 1);
        assert_eq!(m.region_of(0, 465, 305), 1);
    }
}
