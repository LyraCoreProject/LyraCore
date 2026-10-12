//! Source populations stay dormant until an Instance creates independent spawn and patrol copies.

mod support;

use std::time::Duration;

use lyracore_shared::instance::{BATTLEGROUND_MAPS, DUNGEON_MAPS};
use support::{actor, poll_until, Standalone};

fn source_guid(map_id: u32) -> u64 {
    (0xF130u64 << 48) | (51000u64 << 24) | (1_000_000 + u64::from(map_id))
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn legacy_pools_choose_one_position_and_keep_instance_lives_independent() {
    let mut shard = Standalone::start("legacy-creature-pools");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    let mut packed = Vec::new();
    for (pool, map, count, event) in [(1069, 0, 8, ""), (46330, 230, 4, ""), (1163, 0, 3, "27")] {
        let population = (2_u64 << 32) + pool;
        shard.assert_sql(&format!("INSERT INTO game_creature_population (population_id,max_count,world_state_condition,world_state_expression,flags,entries,is_pool) VALUES ({population},1,0,0,0,'',true)"));
        for index in 0..count {
            let guid = (0xF130u64 << 48) | (51000u64 << 24) | (pool * 100 + index);
            shard.assert_sql(&format!("INSERT INTO game_creature_population_member (source_guid,population_id,entry,choices,chance_bp,event_ids) VALUES ({guid},{population},51000,'',0,'{event}')"));
            packed.push(format!("{guid},51000,{map},10,20,30,0,0,1"));
        }
    }
    shard.assert_call("import_creature_spawns", &[&packed.join(";")]);
    assert_eq!(
        shard
            .query_rows("SELECT guid FROM game_creature_spawn WHERE source_guid = 0")
            .len(),
        15
    );
    assert!(poll_until(Duration::from_secs(10), || shard
        .query_rows("SELECT guid FROM game_world_entity WHERE entry = 51000")
        .len()
        == 1));
    let mut copies = Vec::new();
    for instance in [901, 902] {
        shard.assert_call(
            "ensure_instance",
            &[&instance.to_string(), "230", "0", &actor("1")],
        );
        let rows = shard.query_rows(&format!(
            "SELECT guid FROM game_creature_spawn WHERE instance_id = {instance}"
        ));
        assert_eq!(rows.len(), 1);
        copies.push(rows[0]["guid"].clone());
    }
    assert_ne!(copies[0], copies[1]);
    shard.assert_call("debug_kill_creature", &["1", &copies[0]]);
    assert!(poll_until(Duration::from_secs(90), || {
        let survivor = shard.query_rows(&format!(
            "SELECT dead FROM game_world_entity WHERE guid = {}",
            copies[1]
        ));
        assert_eq!(survivor[0]["dead"], "false");
        shard.query_rows(&format!(
            "SELECT life_seq FROM game_creature_spawn WHERE guid = {}",
            copies[0]
        ))[0]["life_seq"]
            == "2"
    }));
    shard.assert_call("debug_reap_instance", &["901"]);
    assert!(shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE guid = {}",
            copies[0]
        ))
        .is_empty());
    assert_eq!(
        shard
            .query_rows(&format!(
                "SELECT guid FROM game_world_entity WHERE guid = {}",
                copies[1]
            ))
            .len(),
        1
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn all_dungeon_populations_have_independent_spawn_and_patrol_lifecycles() {
    let mut shard = Standalone::start("full-world-population");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    let maps: Vec<u32> = DUNGEON_MAPS
        .iter()
        .chain(BATTLEGROUND_MAPS)
        .copied()
        .chain([0, 1, 369, 449, 450])
        .collect();
    let packed = maps
        .iter()
        .map(|&map_id| format!("{},51000,{map_id},10,20,30,0,2,1", source_guid(map_id)))
        .collect::<Vec<_>>()
        .join(";");
    shard.assert_call(
        "import_creature_spawns",
        &[&serde_json::to_string(&packed).unwrap()],
    );
    let mut imported_maps: Vec<u32> = shard
        .query_rows("SELECT map_id FROM game_creature_spawn_map")
        .iter()
        .map(|row| row["map_id"].parse().unwrap())
        .collect();
    imported_maps.sort_unstable();
    let mut expected_maps = maps.clone();
    expected_maps.sort_unstable();
    assert_eq!(imported_maps, expected_maps);
    assert!(poll_until(Duration::from_secs(10), || {
        shard
            .query_rows("SELECT guid FROM game_world_entity WHERE entry = 51000")
            .len()
            == 5
    }));
    for map_id in DUNGEON_MAPS.iter().chain(BATTLEGROUND_MAPS) {
        assert!(
            shard
                .query_rows(&format!(
                    "SELECT guid FROM game_world_entity WHERE guid = {}",
                    source_guid(*map_id)
                ))
                .is_empty(),
            "map {map_id} source became an instance-zero entity"
        );
    }
    for (index, &map_id) in DUNGEON_MAPS.iter().enumerate() {
        let waypoint_id = 5_090_000 + index * 1000;
        shard.assert_sql(&format!(
            "INSERT INTO game_creature_waypoint (id, creature_guid, x, y, z) VALUES ({waypoint_id}, {}, 11, 20, 30)",
            source_guid(map_id)
        ));
        let instance_id = 1000 + index;
        shard.assert_call(
            "ensure_instance",
            &[
                &instance_id.to_string(),
                &map_id.to_string(),
                "0",
                &actor("1"),
            ],
        );
        let copies = shard.query_rows(&format!(
            "SELECT guid, instance_id, respawn_secs FROM game_creature_spawn WHERE instance_id = {instance_id}"
        ));
        assert_eq!(copies.len(), 1, "map {map_id}: {copies:?}");
        let guid = &copies[0]["guid"];
        assert_ne!(guid, &source_guid(map_id).to_string());
        assert_eq!(copies[0]["respawn_secs"], "1");
        assert_eq!(
            shard
                .query_rows(&format!(
                    "SELECT id FROM game_creature_waypoint WHERE creature_guid = {guid}"
                ))
                .len(),
            1
        );
    }
    let instance_id = 1000
        + DUNGEON_MAPS
            .iter()
            .position(|map_id| *map_id == 533)
            .unwrap();
    let query = format!("SELECT guid, instance_id, life_seq FROM game_creature_spawn WHERE instance_id = {instance_id}");
    let copy = shard.query_rows(&query).remove(0);
    let guid = &copy["guid"];
    shard.assert_call("debug_kill_creature", &["1", guid]);
    assert!(
        poll_until(Duration::from_secs(90), || {
            shard
                .query_rows(&query)
                .first()
                .is_some_and(|row| row["life_seq"] == "2")
        }),
        "Instance creature did not respawn"
    );
    let live = shard.query_rows(&format!(
        "SELECT instance_id, dead FROM game_world_entity WHERE guid = {guid}"
    ));
    assert_eq!(live[0]["instance_id"], instance_id.to_string());
    assert_eq!(live[0]["dead"], "false");
    shard.assert_call("debug_reap_instance", &[&instance_id.to_string()]);
    assert!(shard.query_rows(&query).is_empty());
    assert!(shard
        .query_rows(&format!(
            "SELECT id FROM game_creature_waypoint WHERE creature_guid = {guid}"
        ))
        .is_empty());
    assert_eq!(
        shard
            .query_rows(&format!(
                "SELECT guid FROM game_creature_spawn WHERE guid = {}",
                source_guid(533)
            ))
            .len(),
        1
    );
    shard.assert_sql("INSERT INTO game_area (id,map_id,parent_area_id,area_bit,flags,exploration_level,faction_group,name) VALUES (5099000,33,0,0,0,0,0,'Fixture dungeon')");
    shard.assert_sql("INSERT INTO game_area_trigger (id,map_id,x,y,z,radius,box_length,box_width,box_height,box_yaw) VALUES (5099000,33,10,20,30,1,0,0,0,0)");
    shard.assert_sql("INSERT INTO game_areatrigger_teleport (trigger_id,target_map,x,y,z,o,name) VALUES (5099000,0,100,200,30,0,'Fixture exit')");
    shard.assert_sql("INSERT INTO game_graveyard (id,map_id,x,y,z,o,name) VALUES (5099001,0,10,20,30,0,'Near dungeon coordinates'),(5099002,0,110,200,30,0,'Near outside entrance')");
    shard.assert_sql("INSERT INTO game_graveyard_zone (row_id,safe_loc_id,zone_id,faction) VALUES (5099001,5099001,5099000,0),(5099002,5099002,5099000,0)");
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call("debug_teleport", &["1", "33", "10", "20", "30", "0"]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call("debug_set_health", &["1", "0"]);
    shard.assert_call("debug_repop", &["1"]);
    let ghost = shard
        .query_rows("SELECT map_id,x,y FROM game_character WHERE guid = 1")
        .remove(0);
    assert_eq!(ghost["map_id"], "0");
    assert_eq!(ghost["x"].parse::<f32>().unwrap(), 110.0);
    assert_eq!(ghost["y"].parse::<f32>().unwrap(), 200.0);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_selected_world_entry_keeps_its_source_and_uses_its_own_guid_and_patrol() {
    let mut shard = Standalone::start("world-source-choice");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    let source = source_guid(0);
    let selected = (source & !(0xFF_FFFFu64 << 24)) | (51001u64 << 24);
    shard.assert_sql("INSERT INTO game_creature_population (population_id,max_count,world_state_condition,world_state_expression,flags,entries,is_pool) VALUES (5099000,1,0,0,0,'',false)");
    shard.assert_sql(&format!("INSERT INTO game_creature_population_member (source_guid,population_id,entry,choices,chance_bp,event_ids) VALUES ({source},5099000,0,'51001',0,'')"));
    shard.assert_sql(&format!("INSERT INTO game_creature_waypoint (id,creature_guid,x,y,z) VALUES (5099000,{source},11,20,30)"));
    let packed = serde_json::to_string(&format!("{source},51000,0,10,20,30,0,2,1")).unwrap();
    for _ in 0..2 {
        shard.assert_call("import_creature_spawns", &[&packed]);
        assert!(poll_until(Duration::from_secs(10), || {
            !shard
                .query_rows(&format!(
                    "SELECT guid FROM game_world_entity WHERE guid = {selected}"
                ))
                .is_empty()
        }));
        assert!(shard
            .query_rows(&format!(
                "SELECT guid FROM game_world_entity WHERE guid = {source}"
            ))
            .is_empty());
        let rows = shard.query_rows(&format!(
            "SELECT entry,source_guid FROM game_creature_spawn WHERE guid = {selected}"
        ));
        assert_eq!(rows[0]["entry"], "51001");
        assert_eq!(rows[0]["source_guid"], source.to_string());
        assert_eq!(
            shard
                .query_rows(&format!(
                    "SELECT guid FROM game_creature_spawn WHERE guid = {source}"
                ))
                .len(),
            1
        );
        assert_eq!(
            shard
                .query_rows(&format!(
                    "SELECT id FROM game_creature_waypoint WHERE creature_guid = {selected}"
                ))
                .len(),
            1
        );
        assert_eq!(
            shard
                .query_rows(&format!(
                    "SELECT id FROM game_creature_waypoint WHERE creature_guid = {source}"
                ))
                .len(),
            1
        );
    }
    let extra =
        serde_json::to_string(&format!("{},51000,533,10,20,30,0,0,1", source_guid(533))).unwrap();
    shard.assert_call("import_creature_spawns_append", &[&extra]);
    let mut maps: Vec<u32> = shard
        .query_rows("SELECT map_id FROM game_creature_spawn_map")
        .iter()
        .map(|row| row["map_id"].parse().unwrap())
        .collect();
    maps.sort_unstable();
    assert_eq!(maps, [0, 533]);
    shard.assert_call("import_creature_spawns", &[&packed]);
    let maps = shard.query_rows("SELECT map_id FROM game_creature_spawn_map");
    assert_eq!(maps.len(), 1);
    assert_eq!(maps[0]["map_id"], "0");
}
