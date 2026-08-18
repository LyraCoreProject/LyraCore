//! `--go-models <client Data/ dir>` — resolves every DOOR/BUTTON
//! `gameobject_template.display_id` to its M2's bounding mesh and emits one `game_go_model` row
//! per entry. Used TOGETHER with `--dump` (the template rows come from the cmangos dump; the
//! model bytes come from the client's own MPQs).
//!
//! Local-space, not world-space: `nav::m2_tris` already loads the bounding mesh model-local, and
//! the only transform applied here is the MDDF axis shuffle (the same one `vmap.rs::apply_full`
//! pre-applies for placed doodads) — quaternion/scale/translate are per-spawn data the MODULE
//! holds in `game_go_collider`, so baking a world transform here would break for instance copies
//! (no source-guid backref on a `GO_COPY_BAND` row). Same licensing firewall as `--vmap`/`--dbc`:
//! in-memory only, nothing written to disk. A display resolving to a `.wmo` is skipped with a
//! warning — WMO door parsing is out of scope.

use anyhow::{bail, Context, Result};
use lyracore_shared::vmap::{encode, TriClass, VmapTri};
use std::path::Path;
use wow_dbc::vanilla_tables::game_object_display_info::{
    GameObjectDisplayInfo, GameObjectDisplayInfoKey,
};
use wow_dbc::Indexable;

use crate::nav::Tri;
use crate::{field, got, parse_table, GO_BUTTON, GO_DOOR};

/// MDDF axis shuffle: placement-local (Y-up) → world-axis (Z-up) orientation, applied BEFORE any
/// rotation/scale/translate — same convention `vmap.rs::apply_full` uses for placed doodads.
/// Pre-applying it here means the module's per-spawn transform is pure quaternion+scale+
/// translate, with no shuffle of its own to get wrong at ray-query time.
fn shuffle(v: [f32; 3]) -> [f32; 3] {
    [-v[2], -v[0], v[1]]
}

fn shuffle_tri(t: Tri) -> Tri {
    [shuffle(t[0]), shuffle(t[1]), shuffle(t[2])]
}

/// Local bounding-sphere radius: center of the mesh's AABB, radius = farthest vertex from that
/// center. Not minimal-enclosing, but conservative (contains every vertex) — the cheap
/// segment-reject the ray merge needs before decoding and transforming the full triangle blob.
fn bounding_radius(tris: &[Tri]) -> f32 {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for t in tris {
        for v in t {
            for k in 0..3 {
                lo[k] = lo[k].min(v[k]);
                hi[k] = hi[k].max(v[k]);
            }
        }
    }
    let center = [
        (lo[0] + hi[0]) / 2.0,
        (lo[1] + hi[1]) / 2.0,
        (lo[2] + hi[2]) / 2.0,
    ];
    tris.iter()
        .flat_map(|t| t.iter())
        .map(|v| {
            let d = [v[0] - center[0], v[1] - center[1], v[2] - center[2]];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
        })
        .fold(0.0f32, f32::max)
}

/// Hard cap on one `game_go_model` row's packed byte size — door meshes are tens of triangles, so
/// hitting this means something is wrong (a WMO-scale mesh slipping through, a corrupt parse), not
/// a legitimate case to shard like `vmap.rs` does for dense terrain cells.
const MAX_ROW_TRI_BYTES: usize = 20_000;

/// Encode a model's shuffled local-space mesh, erroring (NOT sharding) past `MAX_ROW_TRI_BYTES`.
/// `TriClass::M2` is a placeholder the codec requires — the ray merge decides participation per
/// GO row, not per class.
fn encode_capped(entry: u64, model_name: &str, tris: &[Tri]) -> Result<Vec<u8>> {
    let packed: Vec<VmapTri> = tris
        .iter()
        .map(|&verts| VmapTri {
            verts,
            class: TriClass::M2,
        })
        .collect();
    let blob = encode(&packed);
    if blob.len() > MAX_ROW_TRI_BYTES {
        bail!(
            "go-models: entry {entry} model {model_name} packs to {} bytes, over MAX_ROW_TRI_BYTES \
             ({MAX_ROW_TRI_BYTES}) — door meshes should be tiny; this looks wrong, not shardable",
            blob.len()
        );
    }
    Ok(blob)
}

/// One classified DOOR/BUTTON template row worth resolving a model for.
struct DoorTemplate {
    entry: u64,
    display_id: u32,
    size: f32,
}

/// Every `gameobject_template` row that classifies as DOOR/BUTTON (`classify_go_type` 0/1) — every
/// other type (CHEST, GOOBER, GATHER, ...) is out of scope for this slice's collider geometry.
fn door_templates(dump: &str) -> Vec<DoorTemplate> {
    parse_table(dump, "gameobject_template")
        .iter()
        .filter_map(|row| {
            let entry: u64 = field(row, got::ENTRY).parse().ok()?;
            let raw_type: u32 = field(row, got::TYPE).parse().ok()?;
            let stored_type = crate::classify_go_type(entry, raw_type)?;
            (stored_type == GO_DOOR || stored_type == GO_BUTTON).then(|| DoorTemplate {
                entry,
                display_id: field(row, got::DISPLAY_ID).parse().unwrap_or(0),
                size: field(row, got::SIZE).parse().unwrap_or(0.0),
            })
        })
        .collect()
}

/// Reducer batch ceiling (bytes of packed payload) — same ballpark the other importers respect.
const BATCH_BYTES: usize = 28_000;

pub(crate) fn run(args: &crate::Args) -> Result<()> {
    let data_dir = Path::new(args.go_models.as_ref().expect("caller checked"));
    let dump_path = args
        .dump
        .as_ref()
        .context("--go-models needs --dump too (gameobject_template is the template source)")?;
    let dump = crate::read_dump(dump_path)?;
    let templates = door_templates(&dump);
    if templates.is_empty() {
        println!("go-models: no DOOR/BUTTON gameobject_template rows in this dump");
        return Ok(());
    }

    let mut dbc_chain = crate::dbc::open_chain(data_dir)?;
    let display_info: GameObjectDisplayInfo = crate::dbc::read_table(&mut dbc_chain)?;
    let mut geo_chain = crate::collision::open_geometry_chain(data_dir)?;

    let mut rows: Vec<String> = Vec::new();
    let (mut skipped_missing_display, mut skipped_wmo, mut skipped_empty) = (0u32, 0u32, 0u32);
    for t in &templates {
        let Some(display) = display_info.get(GameObjectDisplayInfoKey::new(t.display_id)) else {
            eprintln!(
                "go-models: WARN entry {} display {} not found, skipping",
                t.entry, t.display_id
            );
            skipped_missing_display += 1;
            continue;
        };
        let model_name = display.model_name.as_str();
        if model_name.to_ascii_lowercase().ends_with(".wmo") {
            eprintln!(
                "go-models: WARN entry {} display {} resolves to a WMO ({model_name}), skipping \
                 (M2-only slice — WMO door parsing is out of scope)",
                t.entry, t.display_id
            );
            skipped_wmo += 1;
            continue;
        }
        let local_tris = crate::nav::m2_tris(&mut geo_chain, model_name);
        if local_tris.is_empty() {
            eprintln!(
                "go-models: WARN entry {} model {model_name} produced zero bounding tris, skipping",
                t.entry
            );
            skipped_empty += 1;
            continue;
        }
        let shuffled: Vec<Tri> = local_tris.into_iter().map(shuffle_tri).collect();
        let radius = bounding_radius(&shuffled);
        let blob = encode_capped(t.entry, model_name, &shuffled)?;
        println!(
            "go-models: entry {} model {model_name} — {} tri(s), radius {radius:.2}",
            t.entry,
            shuffled.len()
        );
        let hex: String = blob.iter().map(|b| format!("{b:02x}")).collect();
        rows.push(format!("{},{},{radius},{hex}", t.entry, t.size));
    }

    println!(
        "go-models: {}/{} DOOR/BUTTON template(s) resolved ({skipped_missing_display} missing \
         display, {skipped_wmo} WMO skipped, {skipped_empty} empty mesh)",
        rows.len(),
        templates.len()
    );

    if !args.apply {
        println!("-- DRY RUN: would import {} game_go_model row(s)", rows.len());
        return Ok(());
    }
    if rows.is_empty() {
        bail!("go-models: nothing resolved to import — check the warnings above");
    }

    // Batch + apply (byte-budgeted, same convention as nav.rs/vmap.rs): first batch clears +
    // loads, the rest append.
    let mut batches: Vec<String> = Vec::new();
    let mut current = String::new();
    for row in &rows {
        if !current.is_empty() && current.len() + row.len() + 1 > BATCH_BYTES {
            batches.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(';');
        }
        current.push_str(row);
    }
    if !current.is_empty() {
        batches.push(current);
    }
    for (i, batch) in batches.iter().enumerate() {
        let reducer = if i == 0 {
            "import_go_models"
        } else {
            "import_go_models_append"
        };
        crate::call_reducer(args, reducer, batch)?;
    }
    println!("go-models: applied {} row(s) across {} batch(es).", rows.len(), batches.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube(half: f32, z: f32) -> Vec<Tri> {
        // Two triangles: enough to exercise the AABB/radius math without a real M2.
        vec![
            [[-half, -half, z], [half, -half, z], [-half, half, z]],
            [[half, -half, z], [half, half, z], [-half, half, z]],
        ]
    }

    // -------------------------------------------------------------------------------------
    //  Classification gating (only DOOR/BUTTON survive `door_templates`).
    // -------------------------------------------------------------------------------------

    #[test]
    fn door_templates_keeps_only_door_and_button_rows() {
        let dump = "INSERT INTO `gameobject_template` VALUES \
            (1,0,10,'Door',0,0,0,1.0,0,0),\
            (2,1,20,'Button',0,0,0,1.0,0,0),\
            (3,3,30,'Chest',0,0,0,1.0,0,0),\
            (4,10,40,'Goober',0,0,0,1.0,0,0);";
        let templates = door_templates(dump);
        let entries: Vec<u64> = templates.iter().map(|t| t.entry).collect();
        assert_eq!(entries, vec![1, 2], "only the DOOR (type 0) and BUTTON (type 1) rows survive");
        assert_eq!(templates[0].display_id, 10);
        assert_eq!(templates[1].display_id, 20);
    }

    #[test]
    fn door_templates_drops_a_row_with_no_classification() {
        // Real cmangos type 25 (FISHINGHOLE) collides with the synthetic GATHER marker and is
        // always dropped by `classify_go_type` — must not appear as a DOOR/BUTTON either.
        let dump = "INSERT INTO `gameobject_template` VALUES (5,25,50,'Fishing Hole',0,0,0,1.0,0,0);";
        assert!(door_templates(dump).is_empty());
    }

    // -------------------------------------------------------------------------------------
    //  Transform pipeline: shuffle, bounding radius, codec round trip.
    // -------------------------------------------------------------------------------------

    #[test]
    fn shuffle_applies_the_mddf_axis_convention() {
        // [-v2, -v0, v1]
        assert_eq!(shuffle([1.0, 2.0, 3.0]), [-3.0, -1.0, 2.0]);
        assert_eq!(shuffle([0.0, 0.0, 0.0]), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn bounding_radius_covers_every_shuffled_vertex() {
        let tris: Vec<Tri> = cube(2.0, 5.0).into_iter().map(shuffle_tri).collect();
        let radius = bounding_radius(&tris);
        // Every shuffled vertex must lie within `radius` of the reported center.
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for t in &tris {
            for v in t {
                for k in 0..3 {
                    lo[k] = lo[k].min(v[k]);
                    hi[k] = hi[k].max(v[k]);
                }
            }
        }
        let center = [
            (lo[0] + hi[0]) / 2.0,
            (lo[1] + hi[1]) / 2.0,
            (lo[2] + hi[2]) / 2.0,
        ];
        for t in &tris {
            for v in t {
                let d = ((v[0] - center[0]).powi(2)
                    + (v[1] - center[1]).powi(2)
                    + (v[2] - center[2]).powi(2))
                .sqrt();
                assert!(d <= radius + 1e-4, "vertex {v:?} lies outside radius {radius}");
            }
        }
        assert!(radius > 0.0);
    }

    #[test]
    fn encode_capped_round_trips_through_the_vmap_codec() {
        let tris = cube(1.0, 0.0);
        let blob = encode_capped(1, "test.m2", &tris).unwrap();
        let decoded = lyracore_shared::vmap::decode(&blob).expect("round trip");
        assert_eq!(decoded.len(), tris.len());
        for (d, t) in decoded.iter().zip(&tris) {
            assert_eq!(d.verts, *t);
        }
    }

    // -------------------------------------------------------------------------------------
    //  Byte-cap error — oversized rows fail loudly, they are never sharded.
    // -------------------------------------------------------------------------------------

    #[test]
    fn encode_capped_errors_instead_of_sharding_an_oversized_mesh() {
        // Each triangle costs lyracore_shared::vmap::TRI_BYTES; pick enough to blow the cap.
        let per_tri = lyracore_shared::vmap::TRI_BYTES;
        let n = MAX_ROW_TRI_BYTES / per_tri + 10;
        let tris: Vec<Tri> = (0..n).map(|i| cube(1.0, i as f32)[0]).collect();
        let err = encode_capped(42, "oversized.m2", &tris).unwrap_err();
        assert!(
            err.to_string().contains("MAX_ROW_TRI_BYTES"),
            "error should name the cap, not silently shard: {err}"
        );
    }

    #[test]
    fn encode_capped_accepts_a_typical_tiny_door_mesh() {
        let tris = cube(1.0, 0.0);
        assert!(encode_capped(1, "door.m2", &tris).is_ok());
    }
}
