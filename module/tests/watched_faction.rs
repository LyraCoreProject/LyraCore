//! Watched reputation selection through the durable Gateway Verb.

mod support;

use std::process::Output;

use support::{actor, number, Standalone};

const CHARACTER: &str = "1";

fn start(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &[CHARACTER]);
    shard
}

fn watched(shard: &Standalone, guid: &str) -> i32 {
    number(
        &shard.query_rows(&format!(
            "SELECT watched_faction_index FROM game_character WHERE guid = {guid}"
        ))[0],
        "watched_faction_index",
    )
}

fn select(shard: &Standalone, reputation_index: i32) {
    shard.assert_call(
        "gw_set_watched_faction",
        &[&actor(CHARACTER), &reputation_index.to_string()],
    );
}

fn assert_refusal(output: Output, reason: &str) {
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success() && text.contains(reason), "{text}");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn seeded_and_created_characters_start_without_a_watched_faction() {
    let shard = start("watched-faction-fresh");
    assert_eq!(watched(&shard, CHARACTER), -1);
    shard.assert_call(
        "create_character",
        &[
            "1",
            "\"Freshwatch\"",
            "1",
            "1",
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
        ],
    );
    let guid =
        &shard.query_rows("SELECT guid FROM game_character WHERE name = 'Freshwatch'")[0]["guid"];
    assert_eq!(watched(&shard, guid), -1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn selecting_and_clearing_preserve_standing_and_at_war_state() {
    let shard = start("watched-faction-select");
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("gw_set_faction_at_war", &[&actor(CHARACTER), "60", "true"]);
    let standing =
        shard.query_rows("SELECT * FROM game_player_reputation WHERE character_guid = 1");
    assert!(!standing.is_empty());
    let factions = shard.query_rows("SELECT * FROM game_faction");
    for reputation_index in [0, 19, 63, -1] {
        select(&shard, reputation_index);
        assert_eq!(watched(&shard, CHARACTER), reputation_index);
        assert_eq!(
            shard.query_rows("SELECT * FROM game_player_reputation WHERE character_guid = 1"),
            standing
        );
        assert_eq!(shard.query_rows("SELECT * FROM game_faction"), factions);
    }
    select(&shard, 19);
    for reputation_index in [-2, 64, i32::MIN, i32::MAX] {
        assert_refusal(
            shard.call(
                "gw_set_watched_faction",
                &[&actor(CHARACTER), &reputation_index.to_string()],
            ),
            "invalid watched reputation index",
        );
        assert_eq!(watched(&shard, CHARACTER), 19);
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn watched_selection_requires_operator_current_actor_and_live_character() {
    let shard = start("watched-faction-gates");
    select(&shard, 19);
    assert_refusal(
        shard.call_anonymous("gw_set_watched_faction", &[&actor(CHARACTER), "0"]),
        "operator only",
    );
    assert_eq!(watched(&shard, CHARACTER), 19);
    assert_refusal(
        shard.call("gw_set_watched_faction", &[&actor("999999"), "0"]),
        "mover not in world",
    );
    assert_eq!(watched(&shard, CHARACTER), 19);
    shard.assert_call("claim_account", &["1", CHARACTER, "650"]);
    assert_refusal(
        shard.call("gw_set_watched_faction", &[&actor(CHARACTER), "0"]),
        "STALE_WORLD_SESSION",
    );
    assert_eq!(watched(&shard, CHARACTER), 19);
    let current =
        r#"{"guid":1,"ownership":{"some":{"account_id":1,"generation":1,"request_nonce":650}}}"#;
    shard.assert_call("gw_set_watched_faction", &[current, "63"]);
    shard.assert_call(
        "begin_transfer",
        &["1", current, "0", "0", "1200", "1200", "50", "0", "false"],
    );
    assert_refusal(
        shard.call("gw_set_watched_faction", &[current, "0"]),
        "mover not in world",
    );
    assert_eq!(watched(&shard, CHARACTER), 63);
}

fn enter_world(shard: &Standalone) {
    let key = serde_json::to_string(&vec![7u8; 40]).unwrap();
    shard.assert_call(
        "establish_session",
        &["1", &key, r#"{"__identity__":"0x1"}"#],
    );
    shard.assert_call("gw_heartbeat", &[]);
    shard.assert_call("gw_player_login", &["1", &actor(CHARACTER)]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn selection_and_clear_survive_logout_and_login() {
    let shard = start("watched-faction-relog");
    enter_world(&shard);
    for reputation_index in [19, -1] {
        select(&shard, reputation_index);
        shard.assert_call("debug_logout_character", &[CHARACTER]);
        assert_eq!(watched(&shard, CHARACTER), reputation_index);
        shard.assert_call("gw_player_login", &["1", &actor(CHARACTER)]);
        assert_eq!(watched(&shard, CHARACTER), reputation_index);
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn selection_and_clear_survive_a_map_change_and_world_port() {
    let shard = start("watched-faction-map-change");
    enter_world(&shard);
    for (transfer_id, reputation_index, map_id) in [(1, 19, 1), (2, -1, 0)] {
        select(&shard, reputation_index);
        let transfer_id = transfer_id.to_string();
        shard.assert_call(
            "begin_transfer",
            &[
                &transfer_id,
                &actor(CHARACTER),
                &map_id.to_string(),
                "0",
                "1200",
                "1200",
                "50",
                "0",
                "false",
            ],
        );
        shard.assert_call("import_character", &[&transfer_id]);
        shard.assert_call("finish_transfer", &[&transfer_id, &actor(CHARACTER)]);
        shard.assert_call("gw_player_world_port", &["1", &actor(CHARACTER)]);
        assert_eq!(watched(&shard, CHARACTER), reputation_index);
        assert_eq!(
            shard.query_rows("SELECT map_id FROM game_world_entity WHERE guid = 1")[0]["map_id"],
            map_id.to_string()
        );
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_character_can_choose_the_watched_faction_during_a_taxi_flight() {
    let shard = start("watched-faction-taxi");
    shard.assert_sql(
        "INSERT INTO game_active_taxi_flight \
         (character_guid,path_id,source_node_id,destination_node_id,mount_display_id,fare,\
          current_node_index,started_micros) \
         VALUES (1,5090102,5090100,5090101,1147,25,0,1)",
    );
    select(&shard, 19);
    assert_eq!(watched(&shard, CHARACTER), 19);
    select(&shard, -1);
    assert_eq!(watched(&shard, CHARACTER), -1);
}
