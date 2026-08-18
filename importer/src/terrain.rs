//! `--terrain <client Data/ dir>` — ADT heightmap slice → `game_terrain_chunk` (work-item 172).
//!
//! Reads the operator's OWN client terrain MPQs (same licensing firewall as `--dbc`: extracted
//! data never enters the repo), parses the ADT tiles covering the content slice with `wow-adt`,
//! and loads one row per MCNK cell through the `import_terrain_chunks` reducer(s) — `Vec<f32>`
//! has no spacetime-SQL literal, so terrain takes the packed-string reducer path like spawns.
//! Heights are rebased to ABSOLUTE world Z here (MCVT is relative to the chunk header Z), so
//! the module's `ground_z` is pure arithmetic. Dry-run prints the plan + a self-check; `--apply`
//! calls the reducers. Honors `--map/--center/--radius`, or `--box X0,X1,Y0,Y1` in place of
//! `--radius` for an exact rectangle (work-item 206 — a wide zone like Westfall+Elwynn together
//! needs a rectangle, not a center-radius circle's bounding square); `--center` still drives the
//! interpolate self-check below, so put it INSIDE the box.

use anyhow::{bail, Context, Result};
use lyracore_shared::terrain::{cell_index as shared_cell_index, cell_key, interpolate};
use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::Path;
use wow_mpq::PatchChain;

/// Vanilla map id → client map directory name. Both CONTINENTS are supported: map 0 is the Elwynn
/// corridor, and map 1 is the Phase B Kalimdor world shard (issue #24) — `--terrain`/`--nav --map 1`
/// read `World\Maps\Kalimdor\*.adt` through this same code path, self-checks included. Instance maps
/// are refused (see the map-36 arm).
// Deliberate simplification: two-arm match, not Map.dbc — add arms (or read the DBC) when a third
// CONTINENT matters.
pub(crate) fn map_dir(map_id: u32) -> Result<&'static str> {
    match map_id {
        0 => Ok("Azeroth"),
        1 => Ok("Kalimdor"),
        // Work-item 226 / design doc §4: instance maps (Deadmines 36) are WMO geometry — their
        // walkable floors are NOT in ADT MCNK heightmaps, and `game_terrain_chunk` can't represent
        // stacked floors anyway. The DECIDED design is NO terrain import for map 36: `ground_z`
        // returns `None` there and every runtime movement leg keeps its imported spawn/current Z
        // (the 173/174 baseline-safe fallback). This bail is the guard that keeps a stray
        // flat/wrong plane from ever replacing that correct `None`. Deliberate, not a TODO.
        36 => bail!(
            "map 36 (Deadmines) is a WMO instance map — ADT terrain import is DELIBERATELY \
             unsupported (ground_z None + verbatim spawn Zs is the decided design; \
             docs/design/instances-and-deadmines.md §4). Do not add an arm here."
        ),
        m => bail!("no map-name mapping for map {m} — extend terrain::map_dir (continents only; instance maps are WMO — see the map-36 arm)"),
    }
}

/// Open the terrain patch chain in client load order: `terrain.MPQ` < `patch.MPQ` < `patch-2.MPQ`
/// (ADTs live in terrain.MPQ in the 1.12 client; patches may override tiles).
fn open_terrain_chain(data_dir: &Path) -> Result<PatchChain> {
    let mut chain = PatchChain::new();
    let mut found_base = false;
    for (name, prio) in [
        ("terrain.MPQ", 0i32),
        ("patch.MPQ", 100),
        ("patch-2.MPQ", 200),
    ] {
        let path = data_dir.join(name);
        if path.exists() {
            chain
                .add_archive(&path, prio)
                .with_context(|| format!("opening {}", path.display()))?;
            found_base |= name == "terrain.MPQ";
        }
    }
    if !found_base {
        bail!(
            "no terrain.MPQ in {} (point --terrain at the client's Data/ dir)",
            data_dir.display()
        );
    }
    Ok(chain)
}

/// One output row, already in the reducer wire format's field order.
pub(crate) struct CellRow {
    pub(crate) map_id: u32,
    pub(crate) cell_x: u16,
    pub(crate) cell_y: u16,
    pub(crate) liquid_level: f32,
    pub(crate) has_liquid: bool,
    pub(crate) holes: u32,
    pub(crate) area_id: u32,
    pub(crate) heights: Vec<f32>, // 145, absolute
}

impl CellRow {
    /// `map,cx,cy,liquid,has_liquid,holes,area,h0:h1:...:h144`
    fn packed(&self) -> String {
        let hs: Vec<String> = self.heights.iter().map(|h| format!("{h}")).collect();
        format!(
            "{},{},{},{},{},{},{},{}",
            self.map_id,
            self.cell_x,
            self.cell_y,
            self.liquid_level,
            if self.has_liquid { 1 } else { 0 },
            self.holes,
            self.area_id,
            hs.join(":")
        )
    }
}

/// World coord → global cell index (0..1023) as i32 for range math. Wraps the shared
/// `lyracore_shared::terrain::cell_index`; out-of-square is -1 (the caller range-checks hard).
fn cell_index(coord: f32) -> i32 {
    shared_cell_index(coord).map(|c| c as i32).unwrap_or(-1)
}

/// Cell-index range `(x_min, x_max, y_min, y_max)` covering the content slice: a `--box`
/// rectangle when given (work-item 206 — the Westfall widening needed an exact rectangle, not a
/// center±radius bounding square that either misses corners or overshoots into neighbours), else
/// the original center±radius square. Cell indices count DOWN as world coordinates grow (see
/// `cell_index`), so a box's HIGH world coordinate (x1/y1) maps to the LOW cell index and vice
/// versa. Pure so the corner math is unit-tested without a client Data/ dir.
pub(crate) fn slice_cell_range(
    bbox: Option<(f64, f64, f64, f64)>,
    center: (f64, f64, f64),
    radius: f64,
) -> (i32, i32, i32, i32) {
    match bbox {
        Some((x0, x1, y0, y1)) => (
            cell_index(x1 as f32),
            cell_index(x0 as f32),
            cell_index(y1 as f32),
            cell_index(y0 as f32),
        ),
        None => {
            let (cx0, cy0) = (center.0 as f32, center.1 as f32);
            let r = radius as f32;
            (
                cell_index(cx0 + r),
                cell_index(cx0 - r),
                cell_index(cy0 + r),
                cell_index(cy0 - r),
            )
        }
    }
}

pub(crate) fn run(args: &crate::Args) -> Result<()> {
    let scope = args.world_import_scope()?;
    if scope.bounded_slices.is_empty() {
        println!(
            "terrain: scope {} has no bounded map slices; WMO instance maps do not have terrain",
            scope.name()
        );
        return Ok(());
    }
    let data_dir = Path::new(args.terrain.as_ref().expect("caller checked"));
    let mut chain = open_terrain_chain(data_dir)?;
    let mut slice_rows = Vec::with_capacity(scope.bounded_slices.len());
    let mut tiles_read = 0u32;
    for slice in &scope.bounded_slices {
        let (rows, slice_tiles) = collect_slice(&mut chain, slice).with_context(|| {
            format!(
                "terrain scope {} slice {} (map {})",
                scope.name(),
                slice.name,
                slice.map_id
            )
        })?;
        tiles_read += slice_tiles;
        slice_rows.push(rows);
    }
    let rows = dedup_rows(slice_rows);
    let batches: Vec<String> = rows
        .chunks(24)
        .map(|c| c.iter().map(CellRow::packed).collect::<Vec<_>>().join(";"))
        .collect();
    println!(
        "terrain: {} cells from {tiles_read} tile(s) in scope {} → {} reducer batch(es)",
        rows.len(),
        scope.name(),
        batches.len()
    );
    if !args.apply {
        println!(
            "-- DRY RUN: would call import_terrain_chunks (batch 0, clears) + {} × import_terrain_chunks_append",
            batches.len() - 1
        );
        return Ok(());
    }
    for (i, batch) in batches.iter().enumerate() {
        let reducer = if i == 0 {
            "import_terrain_chunks"
        } else {
            "import_terrain_chunks_append"
        };
        crate::call_reducer(args, reducer, batch)?;
    }
    println!("terrain: applied.");
    Ok(())
}

fn collect_slice(
    chain: &mut PatchChain,
    slice: &crate::world_import_scope::BoundedMapSlice,
) -> Result<(Vec<CellRow>, u32)> {
    let map_id = u32::try_from(slice.map_id).context("bounded slice map id is outside u32")?;
    let map_name = map_dir(map_id)?;
    let (cx0, cy0, cz_expect) = (
        slice.sample.0 as f32,
        slice.sample.1 as f32,
        slice.sample.2 as f32,
    );
    let (cell_x_min, cell_x_max, cell_y_min, cell_y_max) =
        slice_cell_range(Some(slice.bounds), slice.sample, 0.0);
    for c in [cell_x_min, cell_x_max, cell_y_min, cell_y_max] {
        if !(0..1024).contains(&c) {
            bail!("slice cell index {c} outside the map square");
        }
    }
    let (tx_min, tx_max) = (cell_x_min / 16, cell_x_max / 16);
    let (ty_min, ty_max) = (cell_y_min / 16, cell_y_max / 16);
    let mut rows: Vec<CellRow> = Vec::new();
    let mut tiles_read = 0u32;
    for tx in tx_min..=tx_max {
        for ty in ty_min..=ty_max {
            // ADT filename order is folkloric ({name}_{a}_{b}.adt — which index is which axis?),
            // and for interior tiles BOTH candidate names exist (they're just different map
            // locations), so file existence proves nothing (PR-9 review). Arbitrate by CONTENT:
            // parse each candidate and accept the one whose own MCNK header positions land in
            // this (tx, ty) tile's cell range. Wrong-order picks are a hard error, not a
            // silently-holed slice.
            let candidates = [
                format!("World\\Maps\\{map_name}\\{map_name}_{ty}_{tx}.adt"),
                format!("World\\Maps\\{map_name}\\{map_name}_{tx}_{ty}.adt"),
            ];
            let mut accepted = None;
            for name in &candidates {
                let Ok(bytes) = chain.read_file(name) else {
                    continue;
                };
                let wow_adt::ParsedAdt::Root(root) = wow_adt::parse_adt(&mut Cursor::new(&bytes))
                    .with_context(|| format!("parsing {name}"))?
                else {
                    bail!("{name} is not a root terrain ADT (split-file format is post-vanilla)");
                };
                if tile_matches(&root.mcnk_chunks, tx, ty) {
                    accepted = Some((root, name.clone()));
                    break;
                }
            }
            let Some((root, which)) = accepted else {
                bail!(
                    "tile ({tx},{ty}): no candidate ADT ({} / {}) has MCNK positions inside this \
                     tile's cell range — filename-axis mapping broke, refusing to silently hole the slice",
                    candidates[0], candidates[1]
                );
            };
            tiles_read += 1;
            collect_cells(
                &root.mcnk_chunks,
                map_id,
                (cell_x_min, cell_x_max, cell_y_min, cell_y_max),
                &mut rows,
            )?;
            println!("terrain: {which} → {} cells so far", rows.len());
        }
    }
    if rows.is_empty() {
        bail!("no MCNK cells intersected the slice — bad tile mapping or empty ADTs");
    }
    let checked = interp_check(&rows, cx0, cy0)?;
    println!(
        "terrain: slice {} map {map_id} self-check — interpolated z at sample ({cx0:.1},{cy0:.1}) = {checked:.3} (expected ≈ {cz_expect:.3})",
        slice.name
    );
    if (checked - cz_expect).abs() > 2.0 {
        bail!("terrain self-check failed: interpolated {checked:.2} vs expected {cz_expect:.2}");
    }
    Ok((rows, tiles_read))
}

fn dedup_rows(slice_rows: Vec<Vec<CellRow>>) -> Vec<CellRow> {
    let mut rows = BTreeMap::new();
    for row in slice_rows.into_iter().flatten() {
        rows.entry(cell_key(row.map_id, row.cell_x, row.cell_y))
            .or_insert(row);
    }
    rows.into_values().collect()
}

/// Content check for the filename arbitration: does this parsed tile's terrain actually live in
/// grid tile (tx, ty)? True when every height-bearing MCNK's position-derived cell falls in the
/// tile's 16-cell range on both axes (sampling all chunks; a tile is 256 of them).
pub(crate) fn tile_matches(mcnks: &[wow_adt::McnkChunk], tx: i32, ty: i32) -> bool {
    let mut saw_any = false;
    for mcnk in mcnks {
        if mcnk.heights.is_none() {
            continue;
        }
        let (cx, cy) = (
            cell_index(mcnk.header.position[0] - 0.01),
            cell_index(mcnk.header.position[1] - 0.01),
        );
        if cx / 16 != tx || cy / 16 != ty {
            return false;
        }
        saw_any = true;
    }
    saw_any
}

/// Pull every MCNK inside the cell bbox out of one parsed ADT. Cell indices come from the
/// chunk's own header POSITION (world coords of its high-corner), not from tile/ix/iy
/// bookkeeping — immune to filename-order and row/column folklore.
pub(crate) fn collect_cells(
    mcnks: &[wow_adt::McnkChunk],
    map_id: u32,
    (x_min, x_max, y_min, y_max): (i32, i32, i32, i32),
    rows: &mut Vec<CellRow>,
) -> Result<()> {
    for mcnk in mcnks {
        let Some(mcvt) = &mcnk.heights else {
            continue; // no MCVT (WMO-only chunk) — nothing to stand on, skip
        };
        if mcvt.heights.len() != 145 {
            continue;
        }
        // MCNK header position: [0]/[1] are the world x/y of the chunk corner, [2] the base
        // height MCVT values are relative to. wow-adt 0.6.4's header.rs:238 doc claims the raw
        // field is [Z, X, Y] (and offers world_position() to reorder) — that doc is WRONG for
        // vanilla ADTs and contradicts line 106 of the same file. Arbitrated empirically at
        // commit 14e5702 (PR-9 review): dry-run self-checks pass reading [X, Y, Z] at both the
        // near-flat Northshire center (83.529 vs 83.5312) and a sloped point (80.400 exact);
        // were [0] really Z (~80), cell_index(80)=509 would fall outside the slice band
        // (774..784) and ZERO rows would collect. Worth reporting upstream.
        let (px, py, pz) = (
            mcnk.header.position[0],
            mcnk.header.position[1],
            mcnk.header.position[2],
        );
        let (cx, cy) = (cell_index(px - 0.01), cell_index(py - 0.01)); // corner sits ON the boundary; nudge inside
        if cx < x_min || cx > x_max || cy < y_min || cy > y_max {
            continue;
        }
        let heights: Vec<f32> = mcvt.heights.iter().map(|h| h + pz).collect();
        // MCLQ max_height is the vanilla water surface for the near-flat classic liquids.
        // Deliberate simplification: per-cell scalar, not the per-vertex surface — store the grid
        // when swimming physics needs per-vertex accuracy.
        let (has_liquid, liquid_level) = match &mcnk.liquid {
            Some(l) => (true, l.max_height),
            None => (false, 0.0),
        };
        rows.push(CellRow {
            map_id,
            cell_x: cx as u16,
            cell_y: cy as u16,
            liquid_level,
            has_liquid,
            holes: mcnk.header.holes_low_res as u32,
            area_id: mcnk.header.area_id,
            heights,
        });
    }
    Ok(())
}

/// Interp at (x, y) from collected rows via the SAME `lyracore_shared::terrain::interpolate` the
/// module's ground_z calls — so the run()-time self-check verifies the import, not a second
/// copy of the math (PR-9 review).
fn interp_check(rows: &[CellRow], x: f32, y: f32) -> Result<f32> {
    let (cx, cy) = (cell_index(x) as u16, cell_index(y) as u16);
    let row = rows
        .iter()
        .find(|r| r.cell_x == cx && r.cell_y == cy)
        .context("slice center's cell not among collected rows")?;
    interpolate(&row.heights, cx, cy, x, y).context("interpolation rejected the center sample")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slice_cell_range_from_box_uses_corners_not_center_radius() {
        // work-item 206: the widened Westfall box. High world X/Y (x1/y1) map to the LOW cell index
        // (cell index counts DOWN as world coords grow) — the min/max pairing is corner-derived, not
        // center±radius, and radius is ignored entirely once a box is given.
        let (x0, x1, y0, y1) = (-11400.0f64, -8000.0, -1600.0, 2000.0);
        let (cx_min, cx_max, cy_min, cy_max) =
            slice_cell_range(Some((x0, x1, y0, y1)), (-8949.95, -132.493, 83.5312), 180.0);
        assert_eq!(cx_min, cell_index(x1 as f32)); // high world X → low cell index
        assert_eq!(cx_max, cell_index(x0 as f32)); // low world X → high cell index
        assert_eq!(cy_min, cell_index(y1 as f32));
        assert_eq!(cy_max, cell_index(y0 as f32));
        assert!(cx_min < cx_max);
        assert!(cy_min < cy_max);
    }

    #[test]
    fn map_dir_refuses_instance_map_36_by_design_and_maps_the_continents() {
        // Work-item 226 / design doc §4: `--terrain --map 36` must fail LOUD — a WMO instance map
        // has no ADT floors, and importing a stray flat plane would REPLACE the correct
        // ground_z-None fallback with an actively-wrong height that yanks creatures between floors.
        assert_eq!(super::map_dir(0).unwrap(), "Azeroth");
        assert_eq!(super::map_dir(1).unwrap(), "Kalimdor");
        let err36 = super::map_dir(36).unwrap_err().to_string();
        assert!(
            err36.contains("WMO"),
            "the map-36 refusal names the WMO rationale: {err36}"
        );
        assert!(
            err36.contains("DELIBERATELY"),
            "…and marks it as decided, not a gap: {err36}"
        );
        // Any other unknown map still fails loud (the pre-226 behavior, message widened).
        assert!(super::map_dir(429).is_err());
    }

    #[test]
    fn slice_cell_range_without_box_falls_back_to_center_radius_square() {
        // No --box: reproduces the original center±radius bounding square exactly.
        let center = (-8949.95f64, -132.493, 83.5312);
        let radius = 180.0f64;
        let got = slice_cell_range(None, center, radius);
        let want = (
            cell_index((center.0 + radius) as f32),
            cell_index((center.0 - radius) as f32),
            cell_index((center.1 + radius) as f32),
            cell_index((center.1 - radius) as f32),
        );
        assert_eq!(got, want);
    }

    #[test]
    fn dedup_rows_keeps_the_union_across_maps_and_overlaps() {
        let row = |map_id, cell_x, cell_y| CellRow {
            map_id,
            cell_x,
            cell_y,
            liquid_level: 0.0,
            has_liquid: false,
            holes: 0,
            area_id: 0,
            heights: vec![0.0; 145],
        };

        let rows = dedup_rows(vec![
            vec![row(0, 10, 20), row(0, 11, 20)],
            vec![row(0, 11, 20), row(0, 12, 20)],
            vec![row(1, 10, 20)],
        ]);

        assert_eq!(rows.len(), 4);
        assert!(rows
            .iter()
            .any(|row| (row.map_id, row.cell_x, row.cell_y) == (0, 10, 20)));
        assert!(rows
            .iter()
            .any(|row| (row.map_id, row.cell_x, row.cell_y) == (0, 11, 20)));
        assert!(rows
            .iter()
            .any(|row| (row.map_id, row.cell_x, row.cell_y) == (0, 12, 20)));
        assert!(rows
            .iter()
            .any(|row| (row.map_id, row.cell_x, row.cell_y) == (1, 10, 20)));
    }
}
