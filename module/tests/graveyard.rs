mod support;

use lyracore_shared::terrain::cell_index;
use support::Standalone;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn area_graveyards_take_priority_and_unusable_links_fall_back_to_the_zone() {
    let mut shard = Standalone::start("area-graveyards");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.insert_rows(
        "game_area",
        &[
            &[("id", &5099000), ("name", &"Zone")],
            &[
                ("id", &5099001),
                ("parent_area_id", &5099000),
                ("name", &"Area"),
            ],
        ],
    );
    shard.insert_rows(
        "game_graveyard",
        &[
            &[
                ("id", &5099000),
                ("x", &110),
                ("y", &200),
                ("z", &30),
                ("name", &"Zone graveyard"),
            ],
            &[
                ("id", &5099001),
                ("x", &900),
                ("y", &900),
                ("z", &30),
                ("name", &"Area graveyard"),
            ],
        ],
    );
    shard.insert_rows(
        "game_graveyard_zone",
        &[
            &[
                ("row_id", &5099000),
                ("safe_loc_id", &5099000),
                ("zone_id", &5099000),
            ],
            &[
                ("row_id", &5099001),
                ("safe_loc_id", &5099001),
                ("zone_id", &5099001),
                ("faction", &469),
            ],
        ],
    );
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
