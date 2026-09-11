//! Imported-hour placement uses the installed navigation grid before any Character starts.

mod support;

use lyracore_shared::terrain::{cell_index, cell_key};
use lyracore_shared::vmap::{encode, TriClass, VmapTri};
use std::collections::BTreeMap;
use support::Standalone;

const SOURCE: &str = "1111111111111111111111111111111111111111";
const DUMP: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const BLOCKED_CELL: (u16, u16) = (780, 515);
const BLOCKED_SUBCELL: (usize, usize) = (33, 37);
const GENERATION: u64 = 5_012_001;

#[derive(Clone, Copy)]
enum NavigationFixture {
    Candidate19Blocked,
    FirstFiveOpen,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn install_generation(node: &Standalone) {
    let blob = encode(&[VmapTri {
        verts: [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 4.0]],
        class: TriClass::M2,
    }]);
    let (cell_x, cell_y) = (cell_index(0.0).unwrap(), cell_index(0.0).unwrap());
    let key = cell_key(0, cell_x, cell_y);
    let mut digest = blake3::Hasher::new();
    digest.update(b"lyracore-vmap-manifest-v1");
    digest.update(&key.to_le_bytes());
    digest.update(&0u32.to_le_bytes());
    digest.update(&(blob.len() as u32).to_le_bytes());
    digest.update(&blob);
    node.assert_call(
        "stage_vmap_generation",
        &[
            &GENERATION.to_string(),
            "0",
            "1",
            &blob.len().to_string(),
            &serde_json::to_string(&digest.finalize().to_hex().to_string()).unwrap(),
            &serde_json::to_string("imported-placement-fixture").unwrap(),
            &serde_json::to_string("map=0;northshire-start").unwrap(),
        ],
    );
    let packed = format!("0,0,{},{},{}", cell_x, cell_y, hex(&blob));
    node.assert_call(
        "append_vmap_generation_chunks",
        &[
            &GENERATION.to_string(),
            &serde_json::to_string(&packed).unwrap(),
        ],
    );
    node.assert_call("verify_vmap_generation", &[&GENERATION.to_string()]);
    let coverage = serde_json::json!([{"cell_x": cell_x, "cell_y": cell_y}]).to_string();
    node.assert_call(
        "prepare_vmap_nav_coverage",
        &[&GENERATION.to_string(), &coverage],
    );
    node.assert_call("finalize_vmap_nav_coverage", &[&GENERATION.to_string()]);
    node.assert_call("activate_vmap_generation", &[&GENERATION.to_string()]);
}

fn install_floor_and_navigation(node: &Standalone, fixture: NavigationFixture) {
    let heights = std::iter::repeat_n("83.5312", 145)
        .collect::<Vec<_>>()
        .join(":");
    let terrain = [515, 516]
        .map(|cell_y| format!("0,780,{cell_y},0,false,0,12,{heights}"))
        .join(";");
    node.assert_call(
        "import_terrain_chunks",
        &[&serde_json::to_string(&terrain).unwrap()],
    );

    let initial = match fixture {
        NavigationFixture::Candidate19Blocked => 0xff,
        NavigationFixture::FirstFiveOpen => 0,
    };
    let mut first = vec![initial; lyracore_shared::nav::WALK_BYTES];
    let mut second = vec![initial; lyracore_shared::nav::WALK_BYTES];
    match fixture {
        NavigationFixture::Candidate19Blocked => {
            lyracore_shared::nav::walk_set(&mut first, BLOCKED_SUBCELL.0, BLOCKED_SUBCELL.1, false);
        }
        NavigationFixture::FirstFiveOpen => {
            for (cell_y, sub_x, sub_y) in [
                (515, 26, 62),
                (515, 37, 56),
                (516, 31, 8),
                (515, 25, 53),
                (516, 44, 0),
            ] {
                let walk = if cell_y == 515 {
                    &mut first
                } else {
                    &mut second
                };
                lyracore_shared::nav::walk_set(walk, sub_x, sub_y, true);
            }
        }
    }
    let navigation = format!(
        "0,780,515,83.5312,{},;0,780,516,83.5312,{},",
        hex(&first),
        hex(&second)
    );
    node.assert_call(
        "import_nav_chunks",
        &[&serde_json::to_string(&navigation).unwrap()],
    );
}

fn prepare(fixture: NavigationFixture) -> Standalone {
    let mut node = Standalone::start("playerbots-imported-placement");
    node.publish_module();
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_sql("DELETE FROM game_start_position");
    for (race_class, class) in [(257, 1), (261, 5), (264, 8)] {
        node.assert_sql(&format!(
            "INSERT INTO game_start_position (race_class, race, class, map_id, zone_id, x, y, z, orientation, display_id) \
             VALUES ({race_class}, 1, {class}, 0, 12, -8949.95, -132.493, 83.5312, 0, 49)"
        ));
    }
    for family in [
        "creatures",
        "items",
        "loot",
        "gossip",
        "quests",
        "gameobjects",
        "trainers",
        "casts",
        "creature-ai",
        "globals",
        "spellmeta",
    ] {
        node.assert_call(
            "stamp_import_meta",
            &[
                &serde_json::to_string(family).unwrap(),
                &serde_json::to_string(SOURCE).unwrap(),
                &serde_json::to_string(DUMP).unwrap(),
                "1",
            ],
        );
    }
    install_floor_and_navigation(&node, fixture);
    install_generation(&node);
    node.assert_call("debug_set_xp_rate", &["1"]);
    node.assert_call("debug_set_nav_enabled", &["true"]);
    node.assert_call("debug_set_vmap_enabled", &["true"]);
    node.assert_call("debug_set_nav_coverage_enabled", &["true"]);
    node
}

fn stage(node: &Standalone) -> std::process::Output {
    node.call(
        "playerbots_imported_stage",
        &[
            "1",
            &serde_json::to_string(SOURCE).unwrap(),
            &serde_json::to_string(DUMP).unwrap(),
        ],
    )
}

fn rows_by(
    rows: Vec<BTreeMap<String, String>>,
    key: &str,
) -> BTreeMap<String, BTreeMap<String, String>> {
    rows.into_iter()
        .map(|row| (row[key].clone(), row))
        .collect()
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_imported_stage_skips_a_blocked_scatter_point() {
    let node = prepare(NavigationFixture::Candidate19Blocked);
    let staged = stage(&node);
    assert!(
        staged.status.success(),
        "{}",
        String::from_utf8_lossy(&staged.stderr)
    );
    node.assert_call("playerbots_fixture_imported_staged", &[]);

    let bots = rows_by(
        node.query_rows("SELECT character_guid, class, role, controller FROM pkg_playerbots_bot"),
        "character_guid",
    );
    let entities = rows_by(
        node.query_rows(
            "SELECT guid, map_id, instance_id, x, y, z, level, xp FROM game_world_entity WHERE guid >= 1000000 AND guid < 2000000",
        ),
        "guid",
    );
    assert_eq!(bots.len(), 25);
    assert_eq!(entities.len(), 25);
    assert_eq!(
        bots.keys().collect::<Vec<_>>(),
        entities.keys().collect::<Vec<_>>()
    );

    let runners = node.query_rows(
        "SELECT character_guid, objective, foreground, last_outcome FROM pkg_playerbots_runner",
    );
    assert_eq!(runners.len(), 25);
    for runner in &runners {
        assert!(runner["objective"].contains("none"));
        assert!(runner["foreground"].contains("none"));
        assert_eq!(runner["last_outcome"], "(frozen = ())");
    }

    let roles: BTreeMap<(u8, u8), usize> =
        bots.values().fold(BTreeMap::new(), |mut counts, bot| {
            assert_eq!(bot["controller"], "(frozen = ())");
            *counts
                .entry((bot["class"].parse().unwrap(), bot["role"].parse().unwrap()))
                .or_default() += 1;
            counts
        });
    assert_eq!(roles.get(&(1, 0)), Some(&9));
    assert_eq!(roles.get(&(5, 1)), Some(&8));
    assert_eq!(roles.get(&(8, 2)), Some(&8));

    let mut positions = Vec::new();
    for entity in entities.values() {
        assert_eq!(entity["map_id"], "0");
        assert_eq!(entity["instance_id"], "0");
        assert_eq!(entity["level"], "1");
        assert_eq!(entity["xp"], "0");
        let x: f32 = entity["x"].parse().unwrap();
        let y: f32 = entity["y"].parse().unwrap();
        let z: f32 = entity["z"].parse().unwrap();
        assert!(z.is_finite() && (z - 83.5312).abs() < 1e-4);
        assert!(positions
            .iter()
            .all(|(prior_x, prior_y)| (prior_x - x).hypot(prior_y - y) > 1.5));
        positions.push((x, y));
        let cell = (
            lyracore_shared::terrain::cell_index(x).unwrap(),
            lyracore_shared::terrain::cell_index(y).unwrap(),
        );
        let subcell = (
            lyracore_shared::nav::sub_index(x, cell.0, lyracore_shared::nav::WALK_DIM).unwrap(),
            lyracore_shared::nav::sub_index(y, cell.1, lyracore_shared::nav::WALK_DIM).unwrap(),
        );
        assert_ne!((cell, subcell), (BLOCKED_CELL, BLOCKED_SUBCELL));
    }

    let manifests = node.query_rows(
        "SELECT seed, source_revision, dump_sha256, started_micros FROM pkg_playerbots_imported_fixture",
    );
    assert_eq!(manifests.len(), 1);
    assert_eq!(manifests[0]["seed"], "1");
    assert_eq!(manifests[0]["source_revision"], SOURCE);
    assert_eq!(manifests[0]["dump_sha256"], DUMP);
    assert!(manifests[0]["started_micros"].contains("none"));
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_imported_stage_refuses_an_exhausted_sparse_search_atomically() {
    let node = prepare(NavigationFixture::FirstFiveOpen);
    let refused = stage(&node);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr)
        .contains("imported bot 5 found no distinct walkable start in 100 bounded candidates"));
    assert!(node
        .query_rows("SELECT character_guid FROM pkg_playerbots_bot")
        .is_empty());
    assert!(node
        .query_rows("SELECT id FROM pkg_playerbots_imported_fixture")
        .is_empty());
    assert!(node
        .query_rows("SELECT guid FROM game_world_entity WHERE guid >= 1000000 AND guid < 2000000")
        .is_empty());
}
