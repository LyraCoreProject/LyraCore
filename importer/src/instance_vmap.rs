//! Bounded global-WMO extraction for the Map 36 entry and exit route.
//!
//! Vanilla WMO-only maps describe one map-wide placement in the map's WDT rather than in ADT
//! tiles. This module parses that placement and checks whether the selected WMO groups reference
//! active doodads. The existing vmap packer owns triangle transforms and output rows.

use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;
use std::path::Path;
use wow_mpq::PatchChain;

use crate::nav::{Convention, Placement};
use crate::world_import_scope::InstanceVmapSlice;

const WDT_VERSION: u32 = 18;
const MAIN_BYTES: usize = 64 * 64 * 8;
const MODF_BYTES: usize = 64;

pub(crate) struct GlobalWmoPlacement {
    pub(crate) map_id: u32,
    pub(crate) map_name: String,
    pub(crate) wdt: ArchiveEntry,
    pub(crate) placement: Placement,
    pub(crate) unique_id: u32,
    pub(crate) placement_flags: u16,
    pub(crate) doodad_set: u16,
    pub(crate) name_set: u16,
}

pub(crate) struct ArchiveEntry {
    pub(crate) path: String,
    pub(crate) bytes: usize,
    pub(crate) blake3: String,
}

pub(crate) struct WmoGroupEntry {
    pub(crate) source: ArchiveEntry,
    pub(crate) touches_selection: bool,
    pub(crate) active_doodad_refs: Vec<u32>,
}

pub(crate) struct DoodadInspection {
    pub(crate) root: ArchiveEntry,
    pub(crate) selected_start: u32,
    pub(crate) selected_end: u32,
    pub(crate) groups: Vec<WmoGroupEntry>,
    pub(crate) relevant_refs: Vec<u32>,
}

pub(crate) fn read_global_wmo(
    data_dir: &Path,
    chain: &mut PatchChain,
    slice: &InstanceVmapSlice,
) -> Result<GlobalWmoPlacement> {
    let map_name = map_internal_name(data_dir, slice.map_id)?;
    let wdt_name = format!("World\\Maps\\{map_name}\\{map_name}.wdt");
    let bytes = chain
        .read_file(&wdt_name)
        .with_context(|| format!("reading instance WDT {wdt_name}"))?;
    let mut global =
        parse_global_wmo(&bytes).with_context(|| format!("parsing instance WDT {wdt_name}"))?;
    global.map_name = map_name;
    global.map_id = slice.map_id;
    global.wdt = archive_entry(wdt_name, &bytes);
    Ok(global)
}

fn archive_entry(path: String, bytes: &[u8]) -> ArchiveEntry {
    ArchiveEntry {
        path,
        bytes: bytes.len(),
        blake3: blake3::hash(bytes).to_hex().to_string(),
    }
}

fn map_internal_name(data_dir: &Path, map_id: u32) -> Result<String> {
    let mut chain = crate::dbc::open_chain(data_dir)?;
    let maps: wow_dbc::vanilla_tables::map::Map = crate::dbc::read_table(&mut chain)?;
    let mut rows = maps.rows.iter().filter(|row| row.id.id == map_id);
    let row = rows
        .next()
        .with_context(|| format!("Map.dbc has no map {map_id}"))?;
    if rows.next().is_some() {
        bail!("Map.dbc has more than one map {map_id}");
    }
    if row.internal_name.is_empty()
        || !row
            .internal_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        bail!(
            "Map.dbc map {map_id} has unsafe internal name {:?}",
            row.internal_name
        );
    }
    Ok(row.internal_name.clone())
}

fn parse_global_wmo(bytes: &[u8]) -> Result<GlobalWmoPlacement> {
    let chunks = wdt_chunks(bytes)?;
    let version = required_chunk(&chunks, b"MVER")?;
    if version.len() != 4 || u32::from_le_bytes(version.try_into().unwrap()) != WDT_VERSION {
        bail!("WDT MVER must contain vanilla version {WDT_VERSION}");
    }
    let header = required_chunk(&chunks, b"MPHD")?;
    if header.len() != 32 {
        bail!("WDT MPHD must contain 32 bytes, got {}", header.len());
    }
    let flags = u32::from_le_bytes(header[..4].try_into().unwrap());
    if flags & 1 == 0 {
        bail!("WDT MPHD does not select a global WMO");
    }
    let main = required_chunk(&chunks, b"MAIN")?;
    if main.len() != MAIN_BYTES {
        bail!(
            "WDT MAIN must contain {MAIN_BYTES} bytes, got {}",
            main.len()
        );
    }
    if main
        .chunks_exact(8)
        .any(|tile| u32::from_le_bytes(tile[..4].try_into().unwrap()) & 1 != 0)
    {
        bail!("global-WMO extraction does not accept WDT terrain tiles");
    }

    let names = required_chunk(&chunks, b"MWMO")?;
    if names.last() != Some(&0) {
        bail!("WDT MWMO name is not nul-terminated");
    }
    let names = names[..names.len() - 1]
        .split(|byte| *byte == 0)
        .collect::<Vec<_>>();
    if names.len() != 1 || names[0].is_empty() {
        bail!("global-WMO WDT must name exactly one WMO");
    }
    let name = std::str::from_utf8(names[0])
        .context("WDT MWMO name is not UTF-8")?
        .to_owned();
    if !name.to_ascii_lowercase().ends_with(".wmo") {
        bail!("WDT MWMO name is not a WMO path: {name}");
    }

    let placement = required_chunk(&chunks, b"MODF")?;
    if placement.len() != MODF_BYTES {
        bail!(
            "global-WMO WDT must contain one {MODF_BYTES}-byte MODF placement, got {} bytes",
            placement.len()
        );
    }
    if read_u32(placement, 0) != 0 {
        bail!("global-WMO MODF name id must select the sole MWMO name");
    }
    let position = read_vec3(placement, 8)?;
    let rotation = read_vec3(placement, 20)?;
    let bounds_min = read_vec3(placement, 32)?;
    let bounds_max = read_vec3(placement, 44)?;
    if bounds_min
        .iter()
        .zip(bounds_max)
        .any(|(minimum, maximum)| *minimum > maximum)
    {
        bail!("WDT MODF placement bounds are reversed");
    }
    let placement_flags = read_u16(placement, 56);
    if placement_flags & 1 != 0 {
        bail!("global WMO is destroyable and cannot be staged as static geometry");
    }
    Ok(GlobalWmoPlacement {
        map_id: 0,
        map_name: String::new(),
        wdt: archive_entry(String::new(), bytes),
        placement: Placement {
            name,
            is_wmo: true,
            position,
            rotation,
            scale: 1.0,
            bounds_min: Some(bounds_min),
            bounds_max: Some(bounds_max),
        },
        unique_id: read_u32(placement, 4),
        placement_flags,
        doodad_set: read_u16(placement, 58),
        name_set: read_u16(placement, 60),
    })
}

fn wdt_chunks(bytes: &[u8]) -> Result<BTreeMap<[u8; 4], &[u8]>> {
    let mut chunks = BTreeMap::new();
    let mut at = 0usize;
    while at < bytes.len() {
        if bytes.len() - at < 8 {
            bail!("WDT has {} trailing header byte(s)", bytes.len() - at);
        }
        let raw_id: [u8; 4] = bytes[at..at + 4].try_into().unwrap();
        let id = [raw_id[3], raw_id[2], raw_id[1], raw_id[0]];
        let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let start = at + 8;
        let end = start
            .checked_add(len)
            .filter(|end| *end <= bytes.len())
            .with_context(|| {
                format!(
                    "WDT chunk {} extends past input",
                    String::from_utf8_lossy(&id)
                )
            })?;
        if chunks.insert(id, &bytes[start..end]).is_some() {
            bail!("WDT repeats chunk {}", String::from_utf8_lossy(&id));
        }
        at = end;
    }
    Ok(chunks)
}

fn required_chunk<'a>(chunks: &'a BTreeMap<[u8; 4], &'a [u8]>, id: &[u8; 4]) -> Result<&'a [u8]> {
    chunks
        .get(id)
        .copied()
        .with_context(|| format!("WDT is missing {}", String::from_utf8_lossy(id)))
}

fn read_u16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
}

fn read_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn read_vec3(bytes: &[u8], at: usize) -> Result<[f32; 3]> {
    let values = [
        f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()),
        f32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()),
        f32::from_le_bytes(bytes[at + 8..at + 12].try_into().unwrap()),
    ];
    if values.iter().all(|value| value.is_finite()) {
        Ok(values)
    } else {
        bail!("WDT MODF contains a non-finite placement value")
    }
}

pub(crate) fn inspect_relevant_doodads(
    chain: &mut PatchChain,
    map_id: u32,
    selected_cells: &BTreeSet<u64>,
    global: &GlobalWmoPlacement,
    convention: Convention,
) -> Result<DoodadInspection> {
    let name = &global.placement.name;
    let root_bytes = chain
        .read_file(name)
        .with_context(|| format!("reading WMO root {name}"))?;
    let wow_wmo::ParsedWmo::Root(root) = wow_wmo::parse_wmo(&mut Cursor::new(&root_bytes))
        .with_context(|| format!("parsing WMO root {name}"))?
    else {
        bail!("{name} parsed as a group file, expected root");
    };
    let root_source = archive_entry(name.clone(), &root_bytes);
    if root.n_doodad_sets as usize != root.doodad_sets.len()
        || root.n_doodad_defs as usize != root.doodad_defs.len()
    {
        bail!("WMO {name} doodad header counts do not match parsed chunks");
    }
    let (selected_start, active_end) = if root.doodad_sets.is_empty() {
        if !root.doodad_defs.is_empty() {
            bail!("WMO {name} defines doodads without a doodad set");
        }
        (0, 0)
    } else {
        let set = root
            .doodad_sets
            .get(global.doodad_set as usize)
            .with_context(|| {
                format!(
                    "WMO {name} has no selected doodad set {}",
                    global.doodad_set
                )
            })?;
        (
            set.start_index,
            set.start_index
                .checked_add(set.count)
                .context("WMO doodad set range overflows")?,
        )
    };
    if active_end as usize > root.doodad_defs.len() {
        bail!("WMO {name} selected doodad set extends past MODD definitions");
    }

    let stem = &name[..name.len() - 4];
    let mut relevant = BTreeSet::new();
    let mut groups = Vec::new();
    for ordinal in 0..root.n_groups {
        let group_name = format!("{stem}_{ordinal:03}.wmo");
        let bytes = chain
            .read_file(&group_name)
            .with_context(|| format!("reading WMO group {group_name}"))?;
        let wow_wmo::ParsedWmo::Group(group) = wow_wmo::parse_wmo(&mut Cursor::new(&bytes))
            .with_context(|| format!("parsing WMO group {group_name}"))?
        else {
            bail!("{group_name} parsed as a root file, expected group");
        };
        if group
            .doodad_refs
            .iter()
            .any(|index| usize::from(*index) >= root.doodad_defs.len())
        {
            bail!("WMO group {group_name} references a missing doodad definition");
        }
        let active = active_doodad_refs(&group.doodad_refs, selected_start, active_end);
        let touches_selection = group_touches_selection(
            map_id,
            selected_cells,
            &global.placement,
            convention,
            &group.bounding_box,
        )?;
        if touches_selection {
            relevant.extend(active.iter().copied());
        }
        groups.push(WmoGroupEntry {
            source: archive_entry(group_name, &bytes),
            touches_selection,
            active_doodad_refs: active.into_iter().collect(),
        });
    }
    Ok(DoodadInspection {
        root: root_source,
        selected_start,
        selected_end: active_end,
        groups,
        relevant_refs: relevant.into_iter().collect(),
    })
}

fn active_doodad_refs(refs: &[u16], start: u32, end: u32) -> BTreeSet<u32> {
    refs.iter()
        .copied()
        .map(u32::from)
        .filter(|index| *index >= start && *index < end)
        .collect()
}

fn group_touches_selection(
    map_id: u32,
    selected_cells: &BTreeSet<u64>,
    placement: &Placement,
    convention: Convention,
    bounds: &[f32],
) -> Result<bool> {
    if bounds.len() != 6 || !bounds.iter().all(|value| value.is_finite()) {
        bail!("WMO group has malformed bounds");
    }
    let (lo, hi) = (&bounds[..3], &bounds[3..]);
    if lo
        .iter()
        .zip(hi)
        .any(|(minimum, maximum)| minimum > maximum)
    {
        bail!("WMO group has reversed bounds");
    }
    let mut world_lo = [f32::MAX; 2];
    let mut world_hi = [f32::MIN; 2];
    let position = crate::nav::place_pos(placement.position);
    for x in [lo[0], hi[0]] {
        for y in [lo[1], hi[1]] {
            for z in [lo[2], hi[2]] {
                let point = crate::vmap::apply_full(
                    convention,
                    placement.rotation,
                    placement.scale,
                    position,
                    [x, y, z],
                );
                for axis in 0..2 {
                    world_lo[axis] = world_lo[axis].min(point[axis]);
                    world_hi[axis] = world_hi[axis].max(point[axis]);
                }
            }
        }
    }
    let cx0 = lyracore_shared::terrain::cell_index(world_hi[0])
        .context("WMO group high x cannot be represented as a map cell")?;
    let cx1 = lyracore_shared::terrain::cell_index(world_lo[0])
        .context("WMO group low x cannot be represented as a map cell")?;
    let cy0 = lyracore_shared::terrain::cell_index(world_hi[1])
        .context("WMO group high y cannot be represented as a map cell")?;
    let cy1 = lyracore_shared::terrain::cell_index(world_lo[1])
        .context("WMO group low y cannot be represented as a map cell")?;
    Ok((cx0..=cx1).any(|cx| {
        (cy0..=cy1)
            .any(|cy| selected_cells.contains(&lyracore_shared::terrain::cell_key(map_id, cx, cy)))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend(id.iter().rev());
        bytes.extend((payload.len() as u32).to_le_bytes());
        bytes.extend(payload);
        bytes
    }

    fn global_wmo_wdt() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend(chunk(b"MVER", &WDT_VERSION.to_le_bytes()));
        let mut mphd = [0u8; 32];
        mphd[..4].copy_from_slice(&1u32.to_le_bytes());
        bytes.extend(chunk(b"MPHD", &mphd));
        bytes.extend(chunk(b"MAIN", &[0u8; MAIN_BYTES]));
        bytes.extend(chunk(b"MWMO", b"World\\Wmo\\Dungeon\\Test.wmo\0"));
        let mut modf = [0u8; MODF_BYTES];
        modf[4..8].copy_from_slice(&77u32.to_le_bytes());
        for (at, value) in [
            (8, 1.0f32),
            (12, 2.0),
            (16, 3.0),
            (20, 4.0),
            (24, 5.0),
            (28, 6.0),
            (32, 7.0),
            (36, 8.0),
            (40, 9.0),
            (44, 10.0),
            (48, 11.0),
            (52, 12.0),
        ] {
            modf[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        modf[58..60].copy_from_slice(&2u16.to_le_bytes());
        bytes.extend(chunk(b"MODF", &modf));
        bytes
    }

    #[test]
    fn vanilla_global_wmo_placement_is_preserved_exactly() {
        let global = parse_global_wmo(&global_wmo_wdt()).expect("valid global WMO WDT");
        assert_eq!(global.placement.name, "World\\Wmo\\Dungeon\\Test.wmo");
        assert_eq!(global.placement.position, [1.0, 2.0, 3.0]);
        assert_eq!(global.placement.rotation, [4.0, 5.0, 6.0]);
        assert_eq!(global.placement.bounds_min, Some([7.0, 8.0, 9.0]));
        assert_eq!(global.placement.bounds_max, Some([10.0, 11.0, 12.0]));
        assert_eq!(global.doodad_set, 2);
        assert_eq!(global.unique_id, 77);
        assert_eq!(global.placement_flags, 0);
        assert_eq!(global.name_set, 0);
        assert_eq!(global.map_id, 0);
        assert!(global.map_name.is_empty());
        assert_eq!(global.wdt.bytes, global_wmo_wdt().len());
    }

    #[test]
    fn malformed_or_terrain_backed_wdt_is_refused() {
        let mut truncated = global_wmo_wdt();
        truncated.pop();
        assert!(parse_global_wmo(&truncated).is_err());

        let mut active_tile = global_wmo_wdt();
        let main = active_tile
            .windows(4)
            .position(|bytes| bytes == b"NIAM")
            .expect("MAIN chunk");
        active_tile[main + 8..main + 12].copy_from_slice(&1u32.to_le_bytes());
        assert!(parse_global_wmo(&active_tile).is_err());

        let mut duplicate = global_wmo_wdt();
        duplicate.extend(chunk(b"MWMO", b"second.wmo\0"));
        assert!(parse_global_wmo(&duplicate).is_err());
    }

    #[test]
    fn non_finite_or_destroyable_placement_is_refused() {
        let mut non_finite = global_wmo_wdt();
        let modf = non_finite
            .windows(4)
            .position(|bytes| bytes == b"FDOM")
            .expect("MODF chunk");
        non_finite[modf + 16..modf + 20].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(parse_global_wmo(&non_finite).is_err());

        let mut destroyable = global_wmo_wdt();
        let modf = destroyable
            .windows(4)
            .position(|bytes| bytes == b"FDOM")
            .expect("MODF chunk");
        destroyable[modf + 64..modf + 66].copy_from_slice(&1u16.to_le_bytes());
        assert!(parse_global_wmo(&destroyable).is_err());

        let mut reversed_bounds = global_wmo_wdt();
        let modf = reversed_bounds
            .windows(4)
            .position(|bytes| bytes == b"FDOM")
            .expect("MODF chunk");
        reversed_bounds[modf + 40..modf + 44].copy_from_slice(&20.0f32.to_le_bytes());
        assert!(parse_global_wmo(&reversed_bounds).is_err());
    }

    #[test]
    fn transformed_group_bounds_select_only_route_cells() {
        let map_id = 36;
        let origin = 32.0 * 533.333_3;
        let placement = Placement {
            name: "World\\Wmo\\Dungeon\\Test.wmo".to_owned(),
            is_wmo: true,
            position: [origin, 0.0, origin],
            rotation: [0.0; 3],
            scale: 1.0,
            bounds_min: None,
            bounds_max: None,
        };
        let convention = Convention {
            shuffle: false,
            sign: 1.0,
            offset_deg: 0.0,
        };
        let origin_cell = lyracore_shared::terrain::cell_index(0.0).unwrap();
        let selected = BTreeSet::from([lyracore_shared::terrain::cell_key(
            map_id,
            origin_cell,
            origin_cell,
        )]);
        assert!(group_touches_selection(
            map_id,
            &selected,
            &placement,
            convention,
            &[-1.0, -1.0, -1.0, 1.0, 1.0, 1.0]
        )
        .unwrap());
        assert!(!group_touches_selection(
            map_id,
            &selected,
            &placement,
            convention,
            &[100.0, 100.0, -1.0, 101.0, 101.0, 1.0]
        )
        .unwrap());
        assert!(
            group_touches_selection(map_id, &selected, &placement, convention, &[f32::NAN; 6])
                .is_err()
        );
        assert!(group_touches_selection(
            map_id,
            &selected,
            &placement,
            convention,
            &[1.0, -1.0, -1.0, -1.0, 1.0, 1.0]
        )
        .is_err());

        let map_edge = Placement {
            position: [origin, 0.0, -1.0],
            ..placement
        };
        let edge_selected =
            BTreeSet::from([lyracore_shared::terrain::cell_key(map_id, 0, origin_cell)]);
        assert!(group_touches_selection(
            map_id,
            &edge_selected,
            &map_edge,
            convention,
            &[-2.0, -1.0, -1.0, 2.0, 1.0, 1.0]
        )
        .is_err());
    }

    #[test]
    fn only_the_selected_doodad_set_can_block_extraction() {
        assert_eq!(
            active_doodad_refs(&[1, 2, 3, 7, 8], 2, 8),
            BTreeSet::from([2, 3, 7])
        );
    }
}
