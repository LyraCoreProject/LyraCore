//! `--dump-collision <client Data/ dir>` — work-item 240 SPIKE: prove we can read WMO/M2
//! collision geometry out of the operator's own client MPQs before building the nav rasterizer
//! (241). Same licensing firewall as `--dbc`/`--terrain`: in-memory read, nothing written.
//!
//! Reads the ONE root ADT tile containing `--center`, lists its WMO (MODF) and doodad (MDDF)
//! placements, then opens every referenced WMO root+group file and M2 model and reports
//! triangle counts. Dry-run ONLY — no tables, no schema, no `--apply`.

use anyhow::{bail, Context, Result};
use lyracore_shared::terrain::cell_index;
use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::Path;
use wow_mpq::PatchChain;

/// Model/WMO/terrain archives in client load order. Wider than the terrain chain: geometry
/// lives in model.MPQ (M2) and wmo.MPQ; base.MPQ catches strays; patches override everything.
pub(crate) fn open_geometry_chain(data_dir: &Path) -> Result<PatchChain> {
    let mut chain = PatchChain::new();
    let mut found = 0u32;
    for (name, prio) in [
        ("terrain.MPQ", 0i32),
        ("model.MPQ", 0),
        ("wmo.MPQ", 0),
        ("base.MPQ", 0),
        ("patch.MPQ", 100),
        ("patch-2.MPQ", 200),
        ("patch-3.MPQ", 300),
    ] {
        let path = data_dir.join(name);
        if path.exists() {
            chain
                .add_archive(&path, prio)
                .with_context(|| format!("opening {}", path.display()))?;
            found += 1;
        }
    }
    if found == 0 {
        bail!(
            "no MPQs in {} (point --dump-collision at the client's Data/ dir)",
            data_dir.display()
        );
    }
    Ok(chain)
}

/// MMDX/MWMO name blob order → byte offset of each name (names are nul-terminated back to
/// back; MMID/MWID store OFFSETS, placements index the offset list).
fn offset_to_index(names: &[String]) -> BTreeMap<u32, usize> {
    let mut map = BTreeMap::new();
    let mut off = 0u32;
    for (i, n) in names.iter().enumerate() {
        map.insert(off, i);
        off += n.len() as u32 + 1;
    }
    map
}

fn resolve_name<'a>(
    names: &'a [String],
    offsets: &BTreeMap<u32, usize>,
    indices: &[u32],
    name_id: u32,
) -> Result<&'a str> {
    let off = *indices
        .get(name_id as usize)
        .with_context(|| format!("name_id {name_id} out of range ({} indices)", indices.len()))?;
    let idx = *offsets
        .get(&off)
        .with_context(|| format!("MMID/MWID offset {off} doesn't start a name"))?;
    Ok(&names[idx])
}

/// A WMO's collision-relevant totals across all its group files.
struct WmoStats {
    groups: u32,
    tris: usize,
    collision_only_tris: usize, // MOPY material_id == 0xFF (invisible collision geometry)
    detail_tris: usize,         // MOPY flag 0x04 (vmap extractors treat these specially)
}

fn read_wmo(chain: &mut PatchChain, name: &str) -> Result<WmoStats> {
    let root_bytes = chain
        .read_file(name)
        .with_context(|| format!("reading WMO root {name}"))?;
    let wow_wmo::ParsedWmo::Root(root) = wow_wmo::parse_wmo(&mut Cursor::new(&root_bytes))
        .with_context(|| format!("parsing WMO root {name}"))?
    else {
        bail!("{name} parsed as a group file, expected root");
    };
    let stem = name.strip_suffix(".wmo").unwrap_or(name);
    let mut stats = WmoStats {
        groups: root.n_groups,
        tris: 0,
        collision_only_tris: 0,
        detail_tris: 0,
    };
    for g in 0..root.n_groups {
        let gname = format!("{stem}_{g:03}.wmo");
        let bytes = chain
            .read_file(&gname)
            .with_context(|| format!("reading WMO group {gname}"))?;
        let wow_wmo::ParsedWmo::Group(group) = wow_wmo::parse_wmo(&mut Cursor::new(&bytes))
            .with_context(|| format!("parsing WMO group {gname}"))?
        else {
            bail!("{gname} parsed as a root file, expected group");
        };
        stats.tris += group.vertex_indices.len() / 3;
        for m in &group.material_info {
            if m.material_id == 0xFF {
                stats.collision_only_tris += 1;
            }
            if m.flags & 0x04 != 0 {
                stats.detail_tris += 1;
            }
        }
    }
    Ok(stats)
}

/// M2 bounding (collision) mesh triangle count. Vanilla MMDX names end `.mdx`/`.mdl` but the
/// archives store `.m2` — swap the extension.
fn read_m2_bounding_tris(chain: &mut PatchChain, name: &str) -> Result<u32> {
    let m2_name = name
        .rsplit_once('.')
        .map(|(stem, _)| format!("{stem}.m2"))
        .unwrap_or_else(|| name.to_string());
    let bytes = chain
        .read_file(&m2_name)
        .with_context(|| format!("reading M2 {m2_name}"))?;
    let model = match wow_m2::parse_m2(&mut Cursor::new(&bytes))
        .with_context(|| format!("parsing M2 {m2_name}"))?
    {
        wow_m2::M2Format::Legacy(m) | wow_m2::M2Format::Chunked(m) => m,
    };
    Ok(model.header.bounding_triangles.count / 3)
}

pub(crate) fn run(args: &crate::Args) -> Result<()> {
    let data_dir = Path::new(args.dump_collision.as_ref().expect("caller checked"));
    let mut chain = open_geometry_chain(data_dir)?;
    let map_name = crate::terrain::map_dir(args.map as u32)?;

    // The one tile containing --center (16 cells per tile; same math as terrain.rs).
    let (cx, cy) = (args.center.0 as f32, args.center.1 as f32);
    let (Some(cell_x), Some(cell_y)) = (cell_index(cx), cell_index(cy)) else {
        bail!("--center ({cx},{cy}) outside the map square");
    };
    let (tx, ty) = (cell_x as i32 / 16, cell_y as i32 / 16);

    // Same content-arbitrated candidate trick as terrain.rs (filename axis order is folkloric).
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
            bail!("{name} is not a root terrain ADT");
        };
        if crate::terrain::tile_matches(&root.mcnk_chunks, tx, ty) {
            accepted = Some((root, name.clone()));
            break;
        }
    }
    let Some((root, tile_name)) = accepted else {
        bail!("no ADT tile ({tx},{ty}) found for --center ({cx},{cy}) on map {map_name}");
    };

    println!(
        "collision spike: {tile_name} — {} WMO placement(s), {} doodad placement(s)",
        root.wmo_placements.len(),
        root.doodad_placements.len()
    );

    // --- WMOs (MODF) ---
    let wmo_offsets = offset_to_index(&root.wmos);
    let mut wmo_total_tris = 0usize;
    let mut seen: BTreeMap<String, WmoStats> = BTreeMap::new();
    for p in &root.wmo_placements {
        let name =
            resolve_name(&root.wmos, &wmo_offsets, &root.wmo_indices, p.name_id)?.to_string();
        if !seen.contains_key(&name) {
            let stats = read_wmo(&mut chain, &name)?;
            seen.insert(name.clone(), stats);
        }
        let s = &seen[&name];
        println!(
            "  WMO  {name}  pos=({:.1},{:.1},{:.1})  groups={} tris={} collision_only={} detail={}",
            p.position[0],
            p.position[1],
            p.position[2],
            s.groups,
            s.tris,
            s.collision_only_tris,
            s.detail_tris
        );
        wmo_total_tris += s.tris;
    }

    // --- M2 doodads (MDDF) ---
    let m2_offsets = offset_to_index(&root.models);
    let mut m2_total_tris = 0u32;
    let mut m2_seen: BTreeMap<String, Option<u32>> = BTreeMap::new(); // None = parse failed
    let mut m2_zero = 0u32;
    for p in &root.doodad_placements {
        let name =
            resolve_name(&root.models, &m2_offsets, &root.model_indices, p.name_id)?.to_string();
        let tris = *m2_seen
            .entry(name.clone())
            .or_insert_with(|| read_m2_bounding_tris(&mut chain, &name).ok());
        match tris {
            Some(0) | None => m2_zero += 1,
            Some(t) => m2_total_tris += t,
        }
    }
    // Doodads are numerous (trees); print unique models, not every placement.
    for (name, tris) in &m2_seen {
        match tris {
            Some(t) => println!("  M2   {name}  bounding_tris={t}"),
            None => println!("  M2   {name}  PARSE FAILED"),
        }
    }
    let m2_failed = m2_seen.values().filter(|t| t.is_none()).count();

    println!(
        "totals: {} unique WMO ({wmo_total_tris} placed tris), {} unique M2 across {} placements \
         ({m2_total_tris} placed bounding tris, {m2_zero} placements tri-less/unparsed, \
         {m2_failed} unique M2 PARSE FAILURES)",
        seen.len(),
        m2_seen.len(),
        root.doodad_placements.len()
    );
    if seen.values().all(|s| s.tris == 0) && !seen.is_empty() {
        bail!("every WMO parsed to zero triangles — group-file read path is broken, spike FAILS");
    }
    Ok(())
}
