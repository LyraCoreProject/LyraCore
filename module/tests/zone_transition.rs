mod support;

use std::collections::BTreeMap;

use lyracore_shared::spatial::grid_cell;
use lyracore_shared::terrain::cell_index;
use support::Standalone;

const PLAYER_GUID: u64 = 1;
const MAP_ID: u32 = 0;
const HEARTBEAT: u32 = lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT;

/// Elwynn Forest, one of its subzones, and Westfall — a top-level zone, a subzone whose parent is
/// that zone (the one-hop chase), and a second top-level zone to cross into.
const ELWYNN: u32 = 12;
const GOLDSHIRE: u32 = 87;
const WESTFALL: u32 = 40;

/// A zone the terrain never resolves, written straight onto the durable row so a later read proves
/// whether the character row was written at all.
const SENTINEL_ZONE: u32 = 999;

/// Three positions on one line, 100 yd apart: far enough that each sits in its own MCNK terrain cell
/// AND its own 50 yd grid cell, so a heartbeat between any two crosses the area-resolution boundary.
const IN_GOLDSHIRE: (f32, f32, f32) = (-8900.0, -132.0, 60.0);
const IN_ELWYNN: (f32, f32, f32) = (-9000.0, -132.0, 60.0);
const IN_WESTFALL: (f32, f32, f32) = (-9100.0, -132.0, 60.0);

type SqlRow = BTreeMap<String, String>;

/// Runs only when requested because it builds and publishes the Wasm module to its own standalone.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn accepted_movement_across_a_zone_boundary_updates_the_live_and_durable_zone_once() {
    assert_distinct_cells();

    let standalone = Standalone::start("zone-transition");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    import_areas(&standalone);
    import_terrain(&standalone);

    // Start from "no zone resolved yet" so the first crossing is a visible transition rather than a
    // re-statement of what the seeded character already claimed.
    standalone.assert_sql(&format!(
        "UPDATE game_character SET zone_id = 0 WHERE guid = {PLAYER_GUID}"
    ));
    standalone.assert_call("debug_spawn_player_entity", &[&PLAYER_GUID.to_string()]);
    assert_eq!(
        entity_zone(&standalone),
        0,
        "the spawn position is off the imported terrain, so world entry resolves no zone"
    );

    // A subzone resolves to its parent zone: the crossing writes Elwynn, not Goldshire.
    move_to(&standalone, IN_GOLDSHIRE, 1000);
    assert_eq!(entity_zone(&standalone), ELWYNN);
    assert_eq!(character_zone(&standalone), ELWYNN);

    // Walking on INSIDE Elwynn is not a transition. The sentinel survives, which is the durable
    // row's way of saying it was never written.
    standalone.assert_sql(&format!(
        "UPDATE game_character SET zone_id = {SENTINEL_ZONE} WHERE guid = {PLAYER_GUID}"
    ));
    move_to(&standalone, IN_ELWYNN, 2000);
    assert_eq!(entity_zone(&standalone), ELWYNN);
    assert_eq!(
        character_zone(&standalone),
        SENTINEL_ZONE,
        "movement inside one zone must write neither row"
    );

    // Crossing into Westfall writes both rows.
    move_to(&standalone, IN_WESTFALL, 3000);
    assert_eq!(entity_zone(&standalone), WESTFALL);
    assert_eq!(character_zone(&standalone), WESTFALL);
}

/// The fixture's own precondition: each position must sit in its own terrain cell and its own grid
/// cell, or the test would pass for the wrong reason (no crossing, no lookup).
fn assert_distinct_cells() {
    let cells: Vec<_> = [IN_GOLDSHIRE, IN_ELWYNN, IN_WESTFALL]
        .iter()
        .map(|(x, y, _)| (terrain_cell(*x, *y), grid_cell(*x, *y)))
        .collect();
    for (left, right) in [(0, 1), (1, 2), (0, 2)] {
        assert_ne!(
            cells[left], cells[right],
            "fixture positions {left} and {right} share a cell"
        );
    }
}

fn terrain_cell(x: f32, y: f32) -> (u16, u16) {
    (
        cell_index(x).expect("fixture x is on the map"),
        cell_index(y).expect("fixture y is on the map"),
    )
}

/// `AreaTable.dbc` rows in the shape the importer writes: Goldshire is a subzone of Elwynn,
/// Elwynn and Westfall are top-level zones. `exploration_level` 0 keeps discovery XP out of the way.
fn import_areas(standalone: &Standalone) {
    standalone.assert_sql(&format!(
        "INSERT INTO game_area (id,map_id,parent_area_id,area_bit,flags,exploration_level,faction_group,name) \
         VALUES ({ELWYNN},{MAP_ID},0,1,0,0,2,'Elwynn Forest'),\
                ({GOLDSHIRE},{MAP_ID},{ELWYNN},2,0,0,2,'Goldshire'),\
                ({WESTFALL},{MAP_ID},0,3,0,0,2,'Westfall')"
    ));
}

/// One flat MCNK cell under each fixture position, carrying the area id the client would record.
fn import_terrain(standalone: &Standalone) {
    let packed = [
        (IN_GOLDSHIRE, GOLDSHIRE),
        (IN_ELWYNN, ELWYNN),
        (IN_WESTFALL, WESTFALL),
    ]
    .iter()
    .map(|((x, y, _), area_id)| {
        let (cell_x, cell_y) = terrain_cell(*x, *y);
        let heights = vec!["0"; 145].join(":");
        format!("{MAP_ID},{cell_x},{cell_y},0,0,0,{area_id},{heights}")
    })
    .collect::<Vec<_>>()
    .join(";");
    standalone.assert_call("import_terrain_chunks", &[&packed]);
}

fn move_to(standalone: &Standalone, (x, y, z): (f32, f32, f32), move_time_ms: u32) {
    // `--` first: the world coordinates are negative, which the CLI would otherwise read as flags.
    let args = [
        "--".to_string(),
        PLAYER_GUID.to_string(),
        HEARTBEAT.to_string(),
        "[]".to_string(),
        x.to_string(),
        y.to_string(),
        z.to_string(),
        "0.0".to_string(),
        move_time_ms.to_string(),
    ];
    standalone.assert_call(
        "gw_movement_update",
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
    );
}

fn entity_zone(standalone: &Standalone) -> u32 {
    zone_of(standalone.query_rows(&format!(
        "SELECT * FROM game_world_entity WHERE guid = {PLAYER_GUID}"
    )))
}

fn character_zone(standalone: &Standalone) -> u32 {
    zone_of(standalone.query_rows(&format!(
        "SELECT * FROM game_character WHERE guid = {PLAYER_GUID}"
    )))
}

fn zone_of(rows: Vec<SqlRow>) -> u32 {
    assert_eq!(rows.len(), 1, "expected exactly one row for the player");
    rows[0]["zone_id"].parse().expect("zone_id is a u32")
}
