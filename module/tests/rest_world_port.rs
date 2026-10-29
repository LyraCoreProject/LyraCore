mod support;

use support::{actor, Standalone};

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn world_port_keeps_rested_state_below_the_login_threshold() {
    let mut shard = Standalone::start("rest-world-port");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_sql("UPDATE game_character SET rested_xp = 5, rest_state = 1 WHERE guid = 1");
    let key = serde_json::to_string(&vec![7u8; 40]).unwrap();
    shard.assert_call(
        "establish_session",
        &["1", &key, r#"{"__identity__":"0x1"}"#],
    );
    shard.assert_call(
        "begin_transfer",
        &[
            "1",
            &actor("1"),
            "1",
            "0",
            "1200",
            "1200",
            "50",
            "0",
            "false",
        ],
    );
    shard.assert_call("import_character", &["1"]);
    shard.assert_call("finish_transfer", &["1", &actor("1")]);

    shard.assert_call("gw_player_world_port", &["1", &actor("1")]);

    let character =
        &shard.query_rows("SELECT rest_state, rested_xp FROM game_character WHERE guid = 1")[0];
    assert_eq!(character["rest_state"], "1");
    assert_eq!(character["rested_xp"], "5");
    let entity =
        &shard.query_rows("SELECT player_bytes_2 FROM game_world_entity WHERE guid = 1")[0];
    assert_eq!(entity["player_bytes_2"].parse::<u32>().unwrap() >> 24, 1);
    assert_eq!(
        shard.query_rows("SELECT map_id FROM game_world_entity WHERE guid = 1")[0]["map_id"],
        "1"
    );
}
