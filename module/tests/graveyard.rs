mod support;

use lyracore_shared::terrain::cell_index;
use support::Standalone;

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn area_graveyards_take_priority_and_unusable_links_fall_back_to_the_zone() {
    let mut shard = Standalone::start("area-graveyards");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_sql("INSERT INTO game_area (id,map_id,parent_area_id,area_bit,flags,exploration_level,faction_group,name) VALUES (5099000,0,0,0,0,0,0,'Zone'),(5099001,0,5099000,0,0,0,0,'Area')");
    shard.assert_sql("INSERT INTO game_graveyard (id,map_id,x,y,z,o,name) VALUES (5099000,0,110,200,30,0,'Zone graveyard'),(5099001,0,900,900,30,0,'Area graveyard')");
    shard.assert_sql("INSERT INTO game_graveyard_zone (row_id,safe_loc_id,zone_id,faction) VALUES (5099000,5099000,5099000,0),(5099001,5099001,5099001,469)");
    let (cell_x, cell_y) = (cell_index(100.0).unwrap(), cell_index(200.0).unwrap());
    let heights = vec!["30"; 145].join(":");
    shard.assert_call(
        "import_terrain_chunks",
        &[&format!("0,{cell_x},{cell_y},0,0,0,5099001,{heights}")],
    );
    shard.assert_call("debug_spawn_player_entity", &["1"]);

    for (change, expected) in [
        (None, (900.0, 900.0)),
        (
            Some("UPDATE game_graveyard_zone SET faction = 67 WHERE row_id = 5099001"),
            (110.0, 200.0),
        ),
        (
            Some("UPDATE game_graveyard_zone SET faction = 0 WHERE row_id = 5099001"),
            (900.0, 900.0),
        ),
        (
            Some("UPDATE game_graveyard SET map_id = 1 WHERE id = 5099001"),
            (110.0, 200.0),
        ),
        (
            Some("DELETE FROM game_graveyard_zone WHERE row_id = 5099001"),
            (110.0, 200.0),
        ),
    ] {
        if let Some(sql) = change {
            shard.assert_sql(sql);
        }
        shard.assert_call("debug_teleport", &["1", "0", "100", "200", "30", "0"]);
        shard.assert_call("debug_set_health", &["1", "0"]);
        shard.assert_call("debug_repop", &["1"]);
        let ghost = shard
            .query_rows("SELECT map_id,x,y FROM game_character WHERE guid = 1")
            .remove(0);
        assert_eq!(ghost["map_id"], "0");
        assert_eq!(
            (
                ghost["x"].parse::<f32>().unwrap(),
                ghost["y"].parse::<f32>().unwrap()
            ),
            expected
        );
        shard.assert_call("debug_spirit_healer_res", &["1"]);
    }
}
