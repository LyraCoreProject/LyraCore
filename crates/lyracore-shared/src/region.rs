//! Regions: the middle rung of the cell / region / shard hierarchy (issue #23, spec #12).
//!
//! Cells (50yd, [`crate::spatial`]) stay the fixed spatial primitive. A **region** is a contiguous
//! set of cells defined in DATA — never computed from gameplay state — and a **shard** owns one or
//! more regions through the epoch-versioned assignment table on realm-core. This module owns the
//! bottom half of that hierarchy: `region_of(map_id, cell)` as a BAKED LOOKUP, shared verbatim by
//! the module (which stores the definitions) and the gateway (which routes with them), so the two
//! tiers cannot disagree about which region a point is in.
//!
//! # A region is a cell RECTANGLE
//!
//! The spec's requirement is "contiguous cell sets, floor ~10×10 cells" — the floor comes from the
//! ~100yd interaction radius (`spatial::BOX_HALF_SPAN × GRID_CELL_SIZE`): a region smaller than the
//! interaction radius is *all seam*, so every interaction inside it would cross a shard boundary.
//!
//! # The format is content data
//!
//! See `docs/region-sharding.md` for the seam-menu format and a worked example; [`RegionMap::parse`]
//! is its only parser.

// Deliberate simplification: a region is an axis-aligned cell RECTANGLE, not an arbitrary
// contiguous cell set. A rectangle is trivially contiguous, `region_of` is four integer
// comparisons instead of a hash-set probe, and a zone's seam menu stays legible as five lines of
// text an operator can eyeball. Ceiling: you cannot draw an L-shaped region around a lake — you
// draw two rectangles and assign both to the same shard, which costs nothing because a seam
// between same-shard regions is free (that is the whole point of the design). Upgrade path if a
// zone ever genuinely needs one: keep this type as the storage row and add a `region_id`-grouped
// multi-rect form; `region_of`'s signature does not change.

use crate::spatial::{grid_cell, GRID_CELL_SIZE};

/// The region every cell belongs to until a seam menu says otherwise: "the rest of the map".
/// Region 0 is never assignable — a point in it routes through the ordinary `(map, instance)` shard
/// map, i.e. exactly as it did before regions existed. That is what makes an unimported map, or an
/// imported map with a hole in its menu, a strict no-op.
pub const DEFAULT_REGION: u32 = 0;

/// Minimum side of a region, in cells — the spec's ~10×10 floor. The ~100yd interaction radius is
/// `BOX_HALF_SPAN` (2) cells, so a 10-cell (500yd) side leaves a ~300yd seam-free interior after the
/// 2-cell interaction band on each edge; anything much smaller is *all seam*, and every interaction
/// inside it crosses a shard boundary. Enforced at DEFINITION time ([`RegionMap::build`]), which is
/// the only place it can be enforced cheaply.
pub const MIN_REGION_SIDE_CELLS: i32 = 10;

/// One region definition: `region_id` on `map_id` covers the inclusive cell rectangle
/// `[gx_min..=gx_max] × [gy_min..=gy_max]`.
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
    /// Does this region cover cell `(gx, gy)` on `map_id`?
    pub fn contains_cell(&self, map_id: u32, gx: i32, gy: i32) -> bool {
        map_id == self.map_id
            && gx >= self.gx_min
            && gx <= self.gx_max
            && gy >= self.gy_min
            && gy <= self.gy_max
    }

    /// Width × height in cells (inclusive bounds). Saturating, because the bounds come from OPERATOR
    /// TEXT: `0:1 = -2147483648..2147483647, 0..99` overflows a plain subtraction, which panics a
    /// debug build (the import reducer, and the gateway's rebuild of the menu on every world entry)
    /// instead of rejecting the row. A saturated side is still enormous, so it is accepted on its
    /// merits rather than by accident.
    pub fn size_cells(&self) -> (i32, i32) {
        let side = |lo: i32, hi: i32| hi.saturating_sub(lo).saturating_add(1);
        (
            side(self.gx_min, self.gx_max),
            side(self.gy_min, self.gy_max),
        )
    }

    /// Side length in yards, for the docs/log line that tells an operator what they just drew.
    pub fn size_yards(&self) -> (f32, f32) {
        let (w, h) = self.size_cells();
        (w as f32 * GRID_CELL_SIZE, h as f32 * GRID_CELL_SIZE)
    }

    /// Do two definitions claim the same cell? Overlap is rejected at build time: with overlap,
    /// `region_of` would depend on row order, and row order out of a database subscription is not
    /// something a routing decision may depend on.
    pub fn overlaps(&self, other: &Region) -> bool {
        self.map_id == other.map_id
            && self.gx_min <= other.gx_max
            && other.gx_min <= self.gx_max
            && self.gy_min <= other.gy_max
            && other.gy_min <= self.gy_max
    }
}

/// The baked cell→region lookup for the whole realm: every map's accepted region definitions.
///
/// **Empty is the identity.** A `RegionMap` with no definitions answers [`DEFAULT_REGION`] for every
/// point, which routes through the ordinary shard map — so an un-imported deployment behaves exactly
/// as it did before this type existed. Every rejection path below degrades toward that identity.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RegionMap {
    regions: Vec<Region>,
}

impl RegionMap {
    /// Build from definition rows, dropping any that cannot be routed with and reporting why.
    ///
    /// Rejections (each one a `String` for the caller to log or fail on): a region id of
    /// [`DEFAULT_REGION`], inverted bounds, a side under [`MIN_REGION_SIDE_CELLS`], a duplicate
    /// `(map_id, region_id)`, or an overlap with an already-accepted region. **A rejected row is
    /// dropped, never partially applied** — the cells it claimed fall back to `DEFAULT_REGION` and
    /// therefore to the ordinary shard map. A malformed seam menu can only collapse toward today.
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

    /// Parse the seam-menu content format (`docs/region-sharding.md`). One region per line (or per
    /// `;`), `#` starts a comment:
    ///
    /// ```text
    /// # <map_id>:<region_id> = <gx_min>..<gx_max>, <gy_min>..<gy_max>
    /// 0:1 = 460..479, 300..319   # Elwynn — Goldshire basin
    /// ```
    ///
    /// Returns the map plus one rejection line per row that did not make it, from either the syntax
    /// or the [`RegionMap::build`] rules.
    /// **`#` is stripped BEFORE `;` is split on, and the order matters.** The other way round, a
    /// semicolon inside a comment ends the comment: `# a; b` splits into `# a` (a comment) and
    /// ` b` (which no longer starts with `#`, so it is a malformed ROW). Since
    /// `import_map_regions` refuses a menu with ANY rejection, one semicolon in an English prose
    /// comment would fail the whole import — which is exactly what the shipped fixture menu's
    /// header did before this was fixed (#327). A `;` in row position still separates rows, so
    /// nothing about the documented format changes.
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

    /// **The baked lookup.** Which region owns cell `(gx, gy)` on `map_id`? [`DEFAULT_REGION`] when
    /// no definition covers it. Overlap is impossible by construction, so the answer does not depend
    /// on definition order.
    pub fn region_of(&self, map_id: u32, gx: i32, gy: i32) -> u32 {
        self.regions
            .iter()
            .find(|r| r.contains_cell(map_id, gx, gy))
            .map_or(DEFAULT_REGION, |r| r.region_id)
    }

    /// [`RegionMap::region_of`] for a world position — the form routing actually calls, so the
    /// position→cell conversion happens in exactly one place for both tiers.
    pub fn region_at(&self, map_id: u32, x: f32, y: f32) -> u32 {
        let (gx, gy) = grid_cell(x, y);
        self.region_of(map_id, gx, gy)
    }

    /// The accepted definitions (diagnostics, and the docs generator).
    pub fn regions(&self) -> &[Region] {
        &self.regions
    }

    /// No definitions at all — the identity map, where every point is [`DEFAULT_REGION`].
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// Bucket a set of open-world positions (already resolved to cells) into per-region player
    /// counts — the pure half of issue #78's "which region is crowded" gauge, shared by the gateway
    /// (which owns the impure half: reading live positions off its subscription cache) and its
    /// tests.
    ///
    /// [`DEFAULT_REGION`] is never returned: it is "the rest of the map", not a candidate seam, so a
    /// count against it tells an operator nothing they can act on — the gateway's sample would just
    /// be spending a ring-buffer slot on a bucket nobody can activate.
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

/// `<map_id>:<region_id>=<gx_min>..<gx_max>,<gy_min>..<gy_max>` → a row, or `None` if it doesn't
/// parse. Bounds validation is [`RegionMap::build`]'s job; this is syntax only.
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

/// `a..b` → `(a, b)`.
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
        // THE no-op property: with no definitions imported, every cell on every map is
        // DEFAULT_REGION, which routes through the plain (map, instance) shard map. An
        // un-imported deployment is byte-identical to one with no region code at all.
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
        // `region_at` must be `region_of` composed with `spatial::grid_cell` and nothing else —
        // if it drifted, the module's baked grid_x/grid_y and the gateway's routing would disagree
        // about which region a player is standing in, which is the one thing this lookup exists to
        // prevent.
        let (gx, gy) = grid_cell(-8949.95, -132.493);
        let (m, _) = RegionMap::build([rect(0, 7, gx - 5, gy - 5)]);
        assert_eq!(m.region_at(0, -8949.95, -132.493), 7);
        assert_eq!(m.region_of(0, gx, gy), 7);
        // A point a whole region away is back on the default.
        assert_eq!(
            m.region_at(0, -8949.95 + 40.0 * GRID_CELL_SIZE, -132.493),
            DEFAULT_REGION
        );
    }

    #[test]
    fn a_region_under_the_interaction_radius_floor_is_rejected() {
        // The spec's geometry rule: floor ~10×10 cells, because a region smaller than the ~100yd
        // interaction radius is all seam. A rejected row is DROPPED, so its cells fall back to
        // DEFAULT_REGION rather than being routed by a definition nobody can play inside.
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
        // Exactly at the floor is ACCEPTED (the boundary belongs to the legal side).
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
        // Two regions claiming one cell would make `region_of` — and therefore the shard a player
        // lands on — depend on the order rows arrive out of a subscription. Reject the second.
        let (m, rejected) = RegionMap::build([rect(0, 1, 100, 200), rect(0, 2, 110, 210)]);
        assert_eq!(rejected.len(), 1, "{rejected:?}");
        assert!(rejected[0].contains("overlaps"), "{}", rejected[0]);
        assert_eq!(m.regions().len(), 1);
        assert_eq!(
            m.region_of(0, 115, 215),
            1,
            "the FIRST definition keeps the contested cells"
        );
        // Touching-but-not-overlapping is fine — that is a seam, and seams are the point.
        let (m, rejected) = RegionMap::build([rect(0, 1, 100, 200), rect(0, 2, 120, 200)]);
        assert!(rejected.is_empty(), "{rejected:?}");
        assert_eq!((m.region_of(0, 119, 205), m.region_of(0, 120, 205)), (1, 2));
        // Same rectangle on a DIFFERENT map does not overlap.
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
        // The bounds are OPERATOR TEXT, and `build` promises every bad row "degrades toward the
        // identity". A whole-i32 span overflowed the inclusive-size subtraction, which panics a
        // debug build — `import_map_regions` (a reducer) and `Coordinator::map_regions` (once per
        // world entry) both run this on data an operator typed, so it must answer instead.
        let (m, rejected) = RegionMap::parse("0:1 = -2147483648..2147483647, 0..99\n");
        assert!(rejected.is_empty(), "{rejected:?}"); // a legal, if absurd, rectangle
        assert_eq!(m.region_of(0, 0, 50), 1);
        assert_eq!(m.region_of(0, 0, 100), DEFAULT_REGION);
        // The other extreme: an inverted whole-i32 span is REJECTED, not panicked at.
        let (m, rejected) = RegionMap::parse("0:1 = 2147483647..-2147483648, 0..99\n");
        assert_eq!(rejected.len(), 1, "{rejected:?}");
        assert!(m.is_empty());
    }

    #[test]
    fn count_by_region_buckets_positions_and_drops_the_default_region() {
        let (m, rejected) = RegionMap::build([rect(0, 1, 100, 200), rect(0, 2, 400, 400)]);
        assert!(rejected.is_empty(), "{rejected:?}");
        let counts = m.count_by_region([
            (0, 105, 205), // region 1
            (0, 106, 206), // region 1
            (0, 405, 405), // region 2
            (0, 0, 0),     // DEFAULT_REGION — must be dropped, not counted as a region
            (1, 105, 205), // a different map has no definitions here — also DEFAULT_REGION
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
        // Found shipping #327's fixture menu: `split(['\n', ';'])` split BEFORE the `#` strip, so
        // the tail of any prose comment containing a semicolon became a malformed row — and
        // `import_map_regions` errors on ANY rejection, so a single semicolon in the header would
        // have failed the whole import. Comments run to end of LINE, full stop.
        let (m, rejected) = RegionMap::parse(
            "# region 1 is Elwynn; region 2 is the capital; both are inert until assigned\n\
             0:1 = 460..479, 300..319   # touching, not overlapping; that is a seam\n",
        );
        assert!(rejected.is_empty(), "{rejected:?}");
        assert_eq!(m.regions().len(), 1);
        assert_eq!(m.region_of(0, 465, 305), 1);
        // A `;` in ROW position still separates rows — the documented format is unchanged.
        let (m, rejected) = RegionMap::parse("0:1=460..479,300..319; 0:2=480..499,300..319 # both");
        assert!(rejected.is_empty(), "{rejected:?}");
        assert_eq!(m.regions().len(), 2);
    }

    #[test]
    fn the_shipped_fixture_seam_menu_parses_and_places_its_documented_landmarks() {
        // #327: `content/regions/fixture.regions` is the seam menu the out-of-the-box realm
        // imports, so it is CONTENT that ships — and a shipped menu whose rectangles have drifted
        // off the landmarks its own header cites would put the seam somewhere nobody drew it. This
        // reads the real file (not a copy) and re-derives every claim the header makes.
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../content/regions/fixture.regions"
        );
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("the shipped fixture seam menu must exist at {path}: {e}"));
        let (m, rejected) = RegionMap::parse(&text);
        // A successful parse IS the no-overlap assertion: `build` rejects any region overlapping an
        // already-accepted one (and duplicates, inverted bounds, region 0, and the 10×10 floor), so
        // an empty rejection list already proves regions 1 and 2 are disjoint. Re-checking overlap
        // here would only re-test `Region::overlaps`, which the tests above already cover.
        assert!(rejected.is_empty(), "{rejected:?}");
        assert_eq!(
            m.regions().len(),
            2,
            "Northshire Valley (1) and the rest of Elwynn (2)"
        );

        // Region 1 — Northshire Valley, where `init` seeds the fixture's whole spatial content and
        // where a new character spawns. Checked by the coordinate committed in this repo, through
        // the map's OWN lookup rather than by re-reading the rectangle's bounds.
        assert_eq!(
            m.region_at(0, -8949.95, -132.493),
            1,
            "Northshire start (constants.rs start_human_warrior) is in the valley"
        );
        assert_eq!(m.region_of(0, 520, 343), 1, "…and that is cell (520, 343)");
        assert_eq!(
            m.region_at(0, -8935.33, -188.646),
            1,
            "Northshire graveyard (world.rs world_safe_locs 105) is in the valley"
        );
        assert_eq!(m.region_of(0, 520, 345), 1, "…and that is cell (520, 345)");

        // Region 2 — the rest of Elwynn, which is the half that MOVES to a second database.
        assert_eq!(
            m.region_at(0, -9339.59, 171.73),
            2,
            "Goldshire graveyard (world.rs world_safe_locs 106) is over the seam"
        );
        assert_eq!(m.region_of(0, 528, 337), 2, "…and that is cell (528, 337)");
        assert_eq!(
            m.region_at(0, -9552.73, -1374.84),
            2,
            "Eastvale Logging Camp (world.rs world_safe_locs 854) is over the seam"
        );
        assert_eq!(m.region_of(0, 532, 368), 2, "…and that is cell (532, 368)");
        // The 6 Goldshire class trainers at x = -9461.85 (import-world.sh) are over it too —
        // gx 530, on any of Goldshire's own y values.
        assert_eq!(grid_cell(-9461.85, 171.73).0, 530);
        assert_eq!(m.region_of(0, 530, 337), 2);

        // The seam: one cell apart across the gx 524/525 line, on two different regions. This is
        // the crossing every new character makes walking the road out of the valley.
        assert_eq!(
            (m.region_of(0, 524, 343), m.region_of(0, 525, 343)),
            (1, 2),
            "the seam is the constant-x line between gx 524 and gx 525"
        );
    }

    #[test]
    fn a_malformed_seam_menu_line_is_dropped_not_guessed_at() {
        // Same rule the shard map uses: a typo can never silently reroute players. Every rejected
        // line's cells stay on DEFAULT_REGION, and the VALID lines around it still apply.
        let (m, rejected) = RegionMap::parse(
            "nonsense\n0:=1..2,3..4\n0:1=1..2\n0:1=a..b,c..d\n0:1 = 460..479, 300..319\n",
        );
        assert_eq!(rejected.len(), 4, "{rejected:?}");
        assert_eq!(m.regions().len(), 1);
        assert_eq!(m.region_of(0, 465, 305), 1);
    }
}
