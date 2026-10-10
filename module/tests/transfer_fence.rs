//! A Character in Escrow stays frozen on its source Shard: login, the background passes and the
//! Instance reaper leave it alone until the Transfer settles.

mod support;

use std::time::Duration;

use support::{actor, poll_until, Row, Standalone};

const ESCROWED: &str = "1";
const BYSTANDER: &str = "2";

/// The rested pass fires once per 30 s window.
const REST_WINDOW_WAIT: Duration = Duration::from_secs(45);

/// Character 1 in the world, and Character 2 on the same account when `bystander` is set.
fn start(name: &str, bystander: bool) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &[ESCROWED]);
    if bystander {
        shard.assert_call(
            "create_character",
            &["1", "\"Bystander\"", "1", "1", "0", "0", "0", "0", "0", "0"],
        );
        let guid = &shard.query_rows("SELECT guid FROM game_character WHERE name = 'Bystander'")[0]
            ["guid"];
        assert_eq!(guid, BYSTANDER);
        shard.assert_call("debug_spawn_player_entity", &[BYSTANDER]);
    }
    shard
}

fn begin_transfer(shard: &Standalone, guid: &str) {
    shard.assert_call(
        "begin_transfer",
        &[
            guid,
            &actor(guid),
            "0",
            "0",
            "1200",
            "1200",
            "50",
            "0",
            "true",
        ],
    );
}

fn refused(shard: &Standalone, reducer: &str, args: &[&str], tag: &str) {
    let output = shard.call(reducer, args);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success() && text.contains(tag),
        "{reducer} should refuse with {tag}: {text}"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn login_refuses_a_character_in_escrow() {
    let shard = start("transfer-fence-login", false);
    let key = serde_json::to_string(&vec![7u8; 40]).unwrap();
    shard.assert_call(
        "establish_session",
        &["1", &key, r#"{"__identity__":"0x1"}"#],
    );
    shard.assert_call("gw_heartbeat", &[]);
    shard.assert_call("gw_player_login", &["1", &actor(ESCROWED)]);

    begin_transfer(&shard, ESCROWED);
    refused(
        &shard,
        "gw_player_login",
        &["1", &actor(ESCROWED)],
        "in transit",
    );
    assert!(shard
        .query_rows("SELECT guid FROM game_world_entity WHERE guid = 1")
        .is_empty());
}

/// The timed-quest and rested passes scan durable rows, not live entities, so each carries its
/// own fence. The Bystander shows both passes ran over the same rows.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn background_passes_leave_a_character_in_escrow_unchanged() {
    let shard = start("transfer-fence-background", true);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    for guid in [ESCROWED, BYSTANDER] {
        shard.assert_call("debug_grant_quest", &[guid, "50900"]);
    }
    begin_transfer(&shard, ESCROWED);
    shard.assert_sql("UPDATE game_character_quest SET deadline_micros = 1");
    shard.assert_sql("UPDATE game_character SET resting = true, rested_since_micros = 1");

    let passes_ran = poll_until(REST_WINDOW_WAIT, || {
        quest(&shard, BYSTANDER)["failed"] == "true" && rest(&shard, BYSTANDER)["rested_xp"] != "0"
    });
    assert!(
        passes_ran,
        "the Bystander's quest {:?} and rest {:?}",
        quest(&shard, BYSTANDER),
        rest(&shard, BYSTANDER)
    );
    let escrowed_quest = quest(&shard, ESCROWED);
    assert_eq!(escrowed_quest["failed"], "false");
    assert_eq!(escrowed_quest["deadline_micros"], "1");
    let escrowed_rest = rest(&shard, ESCROWED);
    assert_eq!(escrowed_rest["rested_xp"], "0");
    assert_eq!(escrowed_rest["rested_since_micros"], "1");
}

fn quest(shard: &Standalone, guid: &str) -> Row {
    shard.query_rows(&format!(
        "SELECT failed, deadline_micros FROM game_character_quest WHERE character_guid = {guid}"
    ))[0]
        .clone()
}

fn rest(shard: &Standalone, guid: &str) -> Row {
    shard.query_rows(&format!(
        "SELECT rested_xp, rested_since_micros FROM game_character WHERE guid = {guid}"
    ))[0]
        .clone()
}

/// `debug_reap_instance` reads the same occupancy as the scheduled reaper, without its 30 min
/// empty window.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn the_instance_reaper_holds_an_instance_claimed_by_escrow() {
    let shard = start("transfer-fence-reaper", true);
    for guid in [ESCROWED, BYSTANDER] {
        shard.assert_call("debug_create_fixture_instance", &[guid]);
    }
    let claimed = instance_of(&shard, ESCROWED);
    let empty = instance_of(&shard, BYSTANDER);
    assert_ne!(claimed, empty);
    shard.assert_call("debug_enter_instance", &[BYSTANDER, "0"]);
    begin_transfer(&shard, ESCROWED);

    shard.assert_call("debug_reap_instance", &[&empty]);
    refused(
        &shard,
        "debug_reap_instance",
        &[&claimed],
        "occupied or claimed",
    );
    let instances: Vec<String> = shard
        .query_rows("SELECT instance_id FROM game_instance")
        .into_iter()
        .map(|row| row["instance_id"].clone())
        .collect();
    assert_eq!(instances, [claimed]);
}

fn instance_of(shard: &Standalone, guid: &str) -> String {
    shard.query_rows(&format!(
        "SELECT instance_id FROM game_world_entity WHERE guid = {guid}"
    ))[0]["instance_id"]
        .clone()
}
