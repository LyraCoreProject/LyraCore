//! Instance-specific checks for WMO placements read from selected ADT tiles.
//!
//! The shared tile scanner and vmap packer own archive parsing, transforms, and collision output.
//! This module keeps the Instance Pool's stricter rule: a selected WMO group may not depend on an
//! active nested doodad until the importer can transform that doodad into the same generation.

use anyhow::{bail, Context, Result};
use std::collections::BTreeSet;
use std::io::Cursor;
use wow_mpq::PatchChain;

use crate::nav::{Convention, Placement};

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

pub(crate) struct PlacementInspection {
    pub(crate) unique_id: u32,
    pub(crate) placement_flags: u16,
    pub(crate) doodad_set: u16,
    pub(crate) name_set: u16,
    pub(crate) root: ArchiveEntry,
    pub(crate) selected_start: u32,
    pub(crate) selected_end: u32,
    pub(crate) groups: Vec<WmoGroupEntry>,
    pub(crate) touches_selection: bool,
    pub(crate) relevant_refs: Vec<u32>,
}

fn archive_entry(path: String, bytes: &[u8]) -> ArchiveEntry {
    ArchiveEntry {
        path,
        bytes: bytes.len(),
        blake3: blake3::hash(bytes).to_hex().to_string(),
    }
}

pub(crate) fn inspect_relevant_doodads(
    chain: &mut PatchChain,
    map_id: u32,
    selected_cells: &BTreeSet<u64>,
    placement: &Placement,
    convention: Convention,
) -> Result<PlacementInspection> {
    let wmo = placement
        .wmo
        .context("instance WMO placement has no MODF identity")?;
    let name = &placement.name;
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
            .get(wmo.doodad_set as usize)
            .with_context(|| format!("WMO {name} has no selected doodad set {}", wmo.doodad_set))?;
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

    let stem = name
        .strip_suffix(".wmo")
        .or_else(|| name.strip_suffix(".WMO"))
        .with_context(|| format!("WMO root path has no .wmo suffix: {name}"))?;
    let mut relevant = BTreeSet::new();
    let mut groups = Vec::new();
    let mut touches_selection = false;
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
        let group_selected = group_touches_selection(
            map_id,
            selected_cells,
            placement,
            convention,
            &group.bounding_box,
        )?;
        if group_selected {
            touches_selection = true;
            relevant.extend(active.iter().copied());
        }
        groups.push(WmoGroupEntry {
            source: archive_entry(group_name, &bytes),
            touches_selection: group_selected,
            active_doodad_refs: active.into_iter().collect(),
        });
    }
    Ok(PlacementInspection {
        unique_id: placement.unique_id,
        placement_flags: wmo.flags,
        doodad_set: wmo.doodad_set,
        name_set: wmo.name_set,
        root: root_source,
        selected_start,
        selected_end: active_end,
        groups,
        touches_selection,
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
    cells_touch_selection(map_id, selected_cells, world_lo, world_hi)
}

fn cells_touch_selection(
    map_id: u32,
    selected_cells: &BTreeSet<u64>,
    first: [f32; 2],
    second: [f32; 2],
) -> Result<bool> {
    let world_lo = [first[0].min(second[0]), first[1].min(second[1])];
    let world_hi = [first[0].max(second[0]), first[1].max(second[1])];
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
    use crate::nav::WmoPlacement;

    fn placement(position: [f32; 3]) -> Placement {
        Placement {
            name: "World\\Wmo\\Dungeon\\Test.wmo".to_owned(),
            is_wmo: true,
            unique_id: 77,
            position,
            rotation: [0.0; 3],
            scale: 1.0,
            bounds_min: None,
            bounds_max: None,
            wmo: Some(WmoPlacement {
                flags: 0,
                doodad_set: 2,
                name_set: 0,
            }),
        }
    }

    #[test]
    fn transformed_group_bounds_select_only_route_cells() {
        let map_id = 36;
        let origin = 32.0 * 533.333_3;
        let placement = placement([origin, 0.0, origin]);
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

        let map_edge = placement([origin, 0.0, -1.0]);
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

    #[test]
    fn malformed_group_bounds_are_refused() {
        let placement = placement([0.0; 3]);
        let convention = Convention {
            shuffle: false,
            sign: 1.0,
            offset_deg: 0.0,
        };
        assert!(group_touches_selection(
            36,
            &BTreeSet::new(),
            &placement,
            convention,
            &[1.0, -1.0, -1.0, -1.0, 1.0, 1.0]
        )
        .is_err());
    }
}
