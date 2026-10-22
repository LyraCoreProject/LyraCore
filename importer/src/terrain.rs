use anyhow::{bail, Context, Result};
use lyracore_shared::terrain::{cell_index as shared_cell_index, cell_key, interpolate};
use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::Path;
use wow_mpq::PatchChain;

pub(crate) fn map_dir(map_id: u32) -> Result<&'static str> {
    match map_id {
        0 => Ok("Azeroth"),
        1 => Ok("Kalimdor"),
        // A scalar heightmap cannot represent the stacked floors in Deadmines.
        36 => bail!(
            "map 36 (Deadmines) has stacked instance floors that game_terrain_chunk cannot \
             represent; terrain import remains unavailable"
        ),
        m => bail!("no terrain map-name mapping for map {m}"),
    }
}

const SELF_CHECK_TOLERANCE: f32 = 2.0;

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

pub(crate) fn map_tiles(chain: &mut PatchChain, map_name: &str) -> Result<Option<Vec<bool>>> {
    let name = format!("World\\Maps\\{map_name}\\{map_name}.wdt");
    let Ok(bytes) = chain.read_file(&name) else {
        return Ok(None);
    };
    parse_map_tiles(&bytes)
        .with_context(|| format!("parsing {name}"))
        .map(Some)
}

fn parse_map_tiles(mut bytes: &[u8]) -> Result<Vec<bool>> {
    while !bytes.is_empty() {
        if bytes.len() < 8 {
            bail!("truncated WDT chunk header");
        }
        let length = u32::from_le_bytes(bytes[4..8].try_into()?) as usize;
        let payload = bytes
            .get(8..)
            .and_then(|remaining| remaining.get(..length))
            .context("truncated WDT chunk payload")?;
        if &bytes[..4] == b"NIAM" {
            if payload.len() != 64 * 64 * 8 {
                bail!("WDT MAIN must contain 4096 tile entries");
            }
            return payload
                .chunks_exact(8)
                .map(|entry| Ok(u32::from_le_bytes(entry[..4].try_into()?) & 1 != 0))
                .collect();
        }
        bytes = &bytes[8 + length..];
    }
    bail!("WDT has no MAIN tile catalogue")
}

pub(crate) struct CellRow {
    pub(crate) map_id: u32,
    pub(crate) cell_x: u16,
    pub(crate) cell_y: u16,
    pub(crate) liquid_level: f32,
    pub(crate) has_liquid: bool,
    pub(crate) holes: u32,
    pub(crate) area_id: u32,
    pub(crate) heights: Vec<f32>,
    pub(crate) deep_water_tiles: u64,
}

impl CellRow {
    fn packed(&self) -> String {
        let hs: Vec<String> = self.heights.iter().map(|h| format!("{h}")).collect();
        format!(
            "{},{},{},{},{},{},{},{},{}",
            self.map_id,
            self.cell_x,
            self.cell_y,
            self.liquid_level,
            if self.has_liquid { 1 } else { 0 },
            self.holes,
            self.area_id,
            hs.join(":"),
            self.deep_water_tiles
        )
    }
}

fn cell_index(coord: f32) -> i32 {
    shared_cell_index(coord).map(|c| c as i32).unwrap_or(-1)
}

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
            "terrain: scope {} has no bounded map slices; Instance Vmap Slices do not import terrain",
            scope.name()
        );
        return Ok(());
    }
    let data_dir = Path::new(args.terrain.as_ref().expect("caller checked"));
    let mut chain = open_terrain_chain(data_dir)?;
    let (rows, tiles_read) = collect_scope(&mut chain, &scope)?;
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

fn collect_scope(
    chain: &mut PatchChain,
    scope: &crate::world_import_scope::WorldImportScope,
) -> Result<(Vec<CellRow>, u32)> {
    let mut slice_rows = Vec::with_capacity(scope.bounded_slices.len());
    let mut tiles_read = 0u32;
    for slice in &scope.bounded_slices {
        let (rows, slice_tiles) = collect_slice(chain, slice).with_context(|| {
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
    Ok((dedup_rows(slice_rows), tiles_read))
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
    let tiles = map_tiles(chain, map_name)?;
    for tx in tx_min..=tx_max {
        for ty in ty_min..=ty_max {
            // WDT filename axes reverse the world-coordinate cell axes.
            if let Some(tiles) = &tiles {
                let present = tiles
                    .get((tx * 64 + ty) as usize)
                    .context("terrain tile coordinates outside the WDT grid")?;
                if !present {
                    continue;
                }
            }
            let candidates = [
                format!("World\\Maps\\{map_name}\\{map_name}_{ty}_{tx}.adt"),
                format!("World\\Maps\\{map_name}\\{map_name}_{tx}_{ty}.adt"),
            ];
            let mut accepted = None;
            let mut any_file = false;
            for name in &candidates {
                let Ok(bytes) = chain.read_file(name) else {
                    continue;
                };
                // Empty ADT patch entries remove terrain tiles.
                if bytes.is_empty() {
                    continue;
                }
                any_file = true;
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
                if !any_file {
                    println!(
                        "terrain: WARN tile ({tx},{ty}) has no ADT in the client ({} / {}); \
                         open sea or unbuilt map edge, skipped",
                        candidates[0], candidates[1]
                    );
                    continue;
                }
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
    if (checked - cz_expect).abs() > SELF_CHECK_TOLERANCE {
        bail!(
            "terrain self-check failed: slice {} map {map_id} sample ({cx0:.2},{cy0:.2}) \
             interpolated {checked:.2} vs expected {cz_expect:.2} (limit {SELF_CHECK_TOLERANCE} yd; \
             a sample on a WMO floor such as a cave or porch reads off the heightmap — \
             pick a heightmap ground point)",
            slice.name
        );
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

pub(crate) fn collect_cells(
    mcnks: &[wow_adt::McnkChunk],
    map_id: u32,
    (x_min, x_max, y_min, y_max): (i32, i32, i32, i32),
    rows: &mut Vec<CellRow>,
) -> Result<()> {
    for mcnk in mcnks {
        let Some(mcvt) = &mcnk.heights else {
            continue;
        };
        if mcvt.heights.len() != 145 {
            continue;
        }
        let (px, py, pz) = (
            mcnk.header.position[0],
            mcnk.header.position[1],
            mcnk.header.position[2],
        );
        // Nudge boundary corners inside their cell.
        let (cx, cy) = (cell_index(px - 0.01), cell_index(py - 0.01));
        if cx < x_min || cx > x_max || cy < y_min || cy > y_max {
            continue;
        }
        let heights: Vec<f32> = mcvt.heights.iter().map(|h| h + pz).collect();
        // MCLQ max_height supplies the scalar liquid surface for near-flat classic liquids.
        let (has_liquid, liquid_level, deep_water_tiles) = match &mcnk.liquid {
            Some(l) => (
                true,
                l.max_height,
                lyracore_shared::terrain::deep_water_tiles(&l.tile_flags),
            ),
            None => (false, 0.0, 0),
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
            deep_water_tiles,
        });
    }
    Ok(())
}

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
    use crate::world_import_scope::{WorldImportProfile, WorldImportScope};

    #[test]
    fn map_tile_flags_retain_the_client_filename_axes() {
        let mut bytes = b"NIAM".to_vec();
        bytes.extend_from_slice(&(64u32 * 64 * 8).to_le_bytes());
        bytes.resize(8 + 64 * 64 * 8, 0);
        bytes[8 + (53 * 64 + 24) * 8] = 1;
        let tiles = parse_map_tiles(&bytes).unwrap();
        assert!(tiles[53 * 64 + 24]);
        assert!(!tiles[24 * 64 + 53]);
    }

    #[test]
    fn truncated_map_tile_catalogue_is_refused() {
        let mut bytes = b"NIAM".to_vec();
        bytes.extend_from_slice(&32768u32.to_le_bytes());
        assert!(parse_map_tiles(&bytes).is_err());
    }

    fn collect_test_adt(bytes: Vec<u8>) -> Result<(Vec<CellRow>, u32)> {
        let path = std::env::temp_dir().join(format!(
            "lyracore-terrain-adt-{}-{}.mpq",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        wow_mpq::ArchiveBuilder::new()
            .add_file_data(bytes, "World\\Maps\\Azeroth\\Azeroth_32_32.adt")
            .build(&path)
            .unwrap();
        let mut chain = PatchChain::new();
        chain.add_archive(&path, 0).unwrap();
        let scope = WorldImportScope::legacy(
            0,
            Some((-100.0, -50.0, -100.0, -50.0)),
            (-75.0, -75.0, 0.0),
            180.0,
            None,
            vec![],
        )
        .unwrap();
        let result = collect_scope(&mut chain, &scope);
        drop(chain);
        std::fs::remove_file(path).unwrap();
        result
    }

    #[test]
    fn empty_adt_is_an_absent_terrain_tile() {
        let error = collect_test_adt(vec![]).err().unwrap();
        assert!(
            format!("{error:#}").contains("no MCNK cells intersected"),
            "{error:#}"
        );
    }

    #[test]
    fn malformed_nonempty_adt_still_fails_terrain_import() {
        let error = collect_test_adt(vec![1, 2, 3]).err().unwrap();
        assert!(format!("{error:#}").contains("parsing World"), "{error:#}");
    }

    fn client_data_dir() -> Option<std::path::PathBuf> {
        let dir = std::env::var_os("LYRACORE_CLIENT_DATA")?;
        Some(std::path::PathBuf::from(dir))
    }

    #[test]
    #[ignore = "requires the Operator's LYRACORE_CLIENT_DATA"]
    fn alliance_eastern_terrain_dry_run_passes_every_slice_self_check() {
        let data_dir = client_data_dir()
            .expect("LYRACORE_CLIENT_DATA must name the owned client Data directory");
        let scope = WorldImportScope::canonical(WorldImportProfile::AllianceEastern)
            .expect("eastern profile");
        let mut chain = open_terrain_chain(&data_dir).expect("terrain patch chain");
        let (rows, tiles_read) =
            collect_scope(&mut chain, &scope).expect("every alliance-eastern slice self-check");
        assert!(tiles_read > 0);
        for slice in &scope.bounded_slices {
            let (x, y) = (slice.sample.0 as f32, slice.sample.1 as f32);
            let ground = interp_check(&rows, x, y).expect("sample cell imported");
            assert!(
                (ground - slice.sample.2 as f32).abs() <= SELF_CHECK_TOLERANCE,
                "slice {} sample z {} vs heightmap {ground}",
                slice.name,
                slice.sample.2
            );
        }
    }

    #[test]
    #[ignore = "requires the Operator's LYRACORE_CLIENT_DATA"]
    fn starting_profiles_terrain_dry_run_passes_every_slice_self_check() {
        let data_dir = client_data_dir()
            .expect("LYRACORE_CLIENT_DATA must name the owned client Data directory");
        let mut chain = open_terrain_chain(&data_dir).expect("terrain patch chain");
        for profile in [
            WorldImportProfile::StartingEastern,
            WorldImportProfile::StartingKalimdor,
        ] {
            let scope = WorldImportScope::canonical(profile).unwrap();
            let (rows, tiles_read) =
                collect_scope(&mut chain, &scope).expect("starting profile terrain self-checks");
            assert!(tiles_read > 0);
            for slice in &scope.bounded_slices {
                let ground = interp_check(&rows, slice.sample.0 as f32, slice.sample.1 as f32)
                    .expect("sample cell imported");
                assert!(
                    (ground - slice.sample.2 as f32).abs() <= SELF_CHECK_TOLERANCE,
                    "{} ground {ground} differs from {}",
                    slice.name,
                    slice.sample.2
                );
            }
        }
    }

    #[test]
    fn slice_cell_range_from_box_uses_corners_not_center_radius() {
        let (x0, x1, y0, y1) = (-11400.0f64, -8000.0, -1600.0, 2000.0);
        let (cx_min, cx_max, cy_min, cy_max) =
            slice_cell_range(Some((x0, x1, y0, y1)), (-8949.95, -132.493, 83.5312), 180.0);
        assert_eq!(cx_min, cell_index(x1 as f32));
        assert_eq!(cx_max, cell_index(x0 as f32));
        assert_eq!(cy_min, cell_index(y1 as f32));
        assert_eq!(cy_max, cell_index(y0 as f32));
        assert!(cx_min < cx_max);
        assert!(cy_min < cy_max);
    }

    #[test]
    fn map_dir_refuses_single_layer_terrain_for_map_36_and_maps_the_continents() {
        assert_eq!(super::map_dir(0).unwrap(), "Azeroth");
        assert_eq!(super::map_dir(1).unwrap(), "Kalimdor");
        let err36 = super::map_dir(36).unwrap_err().to_string();
        assert!(
            err36.contains("stacked instance floors"),
            "the map-36 refusal names the floor representation limit: {err36}"
        );
        assert!(super::map_dir(429).is_err());
    }

    #[test]
    fn slice_cell_range_without_box_falls_back_to_center_radius_square() {
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
            deep_water_tiles: 0,
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
