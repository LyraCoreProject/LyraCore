use anyhow::{bail, Context, Result};
use lyracore_shared::vmap::{encode, TriClass, VmapTri};
use std::path::Path;
use wow_dbc::vanilla_tables::game_object_display_info::{
    GameObjectDisplayInfo, GameObjectDisplayInfoKey,
};
use wow_dbc::Indexable;

use crate::dump::{got, parse_table};
use crate::nav::Tri;
use crate::world_import::{GO_BUTTON, GO_DOOR};

// Shuffle Y-up placement coordinates to Z-up world axes before rotation and scale.
fn shuffle(v: [f32; 3]) -> [f32; 3] {
    [-v[2], -v[0], v[1]]
}

fn shuffle_tri(t: Tri) -> Tri {
    [shuffle(t[0]), shuffle(t[1]), shuffle(t[2])]
}

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

const MAX_ROW_TRI_BYTES: usize = 20_000;

// M2 is a codec placeholder here; each GameObject row decides ray participation.
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

struct DoorTemplate {
    entry: u64,
    display_id: u32,
    size: f32,
}

fn door_templates(
    dump: &(impl crate::dump::TableRows + ?Sized),
) -> anyhow::Result<Vec<DoorTemplate>> {
    parse_table(dump, "gameobject_template")?
        .iter()
        .map(|row| -> anyhow::Result<_> {
            let entry: u64 = row.number(got::ENTRY)?;
            let raw_type: u32 = row.number(got::TYPE)?;
            let Some(stored_type) = crate::world_import::classify_go_type(entry, raw_type) else {
                return Ok(None);
            };
            if stored_type != GO_DOOR && stored_type != GO_BUTTON {
                return Ok(None);
            }
            Ok(Some(DoorTemplate {
                entry,
                display_id: row.number(got::DISPLAY_ID)?,
                size: row.number(got::SIZE)?,
            }))
        })
        .filter_map(Result::transpose)
        .collect::<anyhow::Result<_>>()
}

const BATCH_BYTES: usize = 28_000;

pub(crate) fn packed_rows(
    data_dir: &Path,
    dump: &(impl crate::dump::TableRows + ?Sized),
) -> Result<Vec<String>> {
    let templates = door_templates(dump)?;
    if templates.is_empty() {
        eprintln!("go-models: no DOOR/BUTTON gameobject_template rows in this dump");
        return Ok(Vec::new());
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
        let local_tris = crate::nav::m2_tris(&mut geo_chain, model_name)?;
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
        eprintln!(
            "go-models: entry {} model {model_name}, {} tri(s), radius {radius:.2}",
            t.entry,
            shuffled.len()
        );
        let hex: String = blob.iter().map(|b| format!("{b:02x}")).collect();
        rows.push(format!("{},{},{radius},{hex}", t.entry, t.size));
    }

    eprintln!(
        "go-models: {}/{} DOOR/BUTTON template(s) resolved ({skipped_missing_display} missing \
         display, {skipped_wmo} WMO skipped, {skipped_empty} empty mesh)",
        rows.len(),
        templates.len()
    );

    Ok(rows)
}

pub(crate) fn run(
    data_dir: &str,
    dump_path: Option<&str>,
    source: crate::world_source::SourceKind,
    apply: bool,
    shard: &crate::shard::Shard,
) -> Result<()> {
    let data_dir = Path::new(data_dir);
    let dump_path = dump_path
        .context("--go-models needs --dump too (gameobject_template is the template source)")?;
    let dump = crate::world_source::WorldSource::open(Path::new(dump_path), source)?;
    let rows = packed_rows(data_dir, &dump)?;
    if rows.is_empty() && door_templates(&dump)?.is_empty() {
        return Ok(());
    }

    if !apply {
        println!(
            "-- DRY RUN: would import {} game_go_model row(s)",
            rows.len()
        );
        return Ok(());
    }
    if rows.is_empty() {
        bail!("go-models: nothing resolved to import — check the warnings above");
    }

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
        shard.call_reducer(reducer, batch)?;
    }
    println!(
        "go-models: applied {} row(s) across {} batch(es).",
        rows.len(),
        batches.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube(half: f32, z: f32) -> Vec<Tri> {
        vec![
            [[-half, -half, z], [half, -half, z], [-half, half, z]],
            [[half, -half, z], [half, half, z], [-half, half, z]],
        ]
    }

    #[test]
    fn vmangos_door_models_use_selected_template_versions() {
        use crate::world_source::{sqlite_fixture, FixtureTable};
        use rusqlite::types::Value::Integer as N;
        let row = |patch, display| {
            [
                ("entry", N(7)),
                ("patch", N(patch)),
                ("type", N(0)),
                ("displayId", N(display)),
                ("size", N(1)),
            ]
        };
        let source = sqlite_fixture(&[FixtureTable {
            name: "gameobject_template",
            columns: &[
                ("size", "REAL"),
                ("displayId", "INTEGER"),
                ("type", "INTEGER"),
                ("patch", "INTEGER"),
                ("entry", "INTEGER"),
            ],
            key: &["entry", "patch"],
            rows: &[&row(0, 1), &row(10, 2), &row(11, 3)],
        }])
        .unwrap();
        let templates = door_templates(&source).unwrap();
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].entry, 7);
        assert_eq!(templates[0].display_id, 2);
        assert_eq!(templates[0].size, 1.0);
    }

    #[test]
    fn door_templates_keeps_only_door_and_button_rows() {
        let dump = "INSERT INTO `gameobject_template` (`entry`, `type`, `displayId`, `name`, `faction`, `flags`, `ExtraFlags`, `size`, `data0`, `data1`) VALUES \
            (1,0,10,'Door',0,0,0,1.0,0,0),\
            (2,1,20,'Button',0,0,0,1.0,0,0),\
            (3,3,30,'Chest',0,0,0,1.0,0,0),\
            (4,10,40,'Goober',0,0,0,1.0,0,0);";
        let templates = door_templates(dump).unwrap();
        let entries: Vec<u64> = templates.iter().map(|t| t.entry).collect();
        assert_eq!(
            entries,
            vec![1, 2],
            "only the DOOR (type 0) and BUTTON (type 1) rows survive"
        );
        assert_eq!(templates[0].display_id, 10);
        assert_eq!(templates[1].display_id, 20);
    }

    #[test]
    fn door_templates_drops_a_row_with_no_classification() {
        let dump =
            "INSERT INTO `gameobject_template` (`entry`, `type`, `displayId`, `name`, `faction`, `flags`, `ExtraFlags`, `size`, `data0`, `data1`) VALUES (5,25,50,'Fishing Hole',0,0,0,1.0,0,0);";
        assert!(door_templates(dump).unwrap().is_empty());
    }

    #[test]
    fn shuffle_applies_the_mddf_axis_convention() {
        assert_eq!(shuffle([1.0, 2.0, 3.0]), [-3.0, -1.0, 2.0]);
        assert_eq!(shuffle([0.0, 0.0, 0.0]), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn bounding_radius_covers_every_shuffled_vertex() {
        let tris: Vec<Tri> = cube(2.0, 5.0).into_iter().map(shuffle_tri).collect();
        let radius = bounding_radius(&tris);
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
                assert!(
                    d <= radius + 1e-4,
                    "vertex {v:?} lies outside radius {radius}"
                );
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

    #[test]
    fn encode_capped_errors_instead_of_sharding_an_oversized_mesh() {
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
