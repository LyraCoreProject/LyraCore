mod support;

use support::Standalone;

const ACTOR: &str =
    r#"{"guid":1,"ownership":{"some":{"account_id":1,"generation":1,"request_nonce":101}}}"#;

fn fixture(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call("claim_account", &["1", "1", "101"]);
    shard
}

fn command(shard: &Standalone, actor: &str, command: &str, payload: &str) {
    shard.assert_call(
        "gw_client_command",
        &[
            actor,
            &serde_json::to_string(command).unwrap(),
            &serde_json::to_string(payload).unwrap(),
        ],
    );
}

fn refused(shard: &Standalone, actor: &str, command: &str, payload: &str, reason: &str) {
    let before = shard.query_rows("SELECT * FROM game_addon_message");
    let result = shard.call(
        "gw_client_command",
        &[
            actor,
            &serde_json::to_string(command).unwrap(),
            &serde_json::to_string(payload).unwrap(),
        ],
    );
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !result.status.success() && output.contains(reason),
        "{output}"
    );
    assert_eq!(shard.query_rows("SELECT * FROM game_addon_message"), before);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn solo_and_nonleader_package_commands_act_as_the_authenticated_character() {
    let shard = fixture("addon-actor");
    command(&shard, ACTOR, "package.fixture.status", "999");
    let owner = shard.query_rows("SELECT owner_identity FROM game_character WHERE guid = 1")[0]
        ["owner_identity"]
        .clone();
    shard.assert_sql("INSERT INTO game_group (group_id, leader_guid, loot_method, loot_threshold, rr_cursor, master_looter_guid, group_type) VALUES (5099000, 999, 3, 2, 0, 0, 0)");
    shard.assert_sql(&format!("INSERT INTO game_group_member (id, group_id, character_guid, owner_identity, raid_slot) VALUES (5099000, 5099000, 1, {owner}, 0)"));
    command(&shard, ACTOR, "package.fixture.status", "999");
    let replies =
        shard.query_rows("SELECT recipient_identity, cmd, payload FROM game_addon_message");
    assert_eq!(replies.len(), 2);
    for reply in replies {
        assert_eq!(reply["recipient_identity"], owner);
        assert_eq!(reply["cmd"], "package.fixture.status.result");
        assert_eq!(reply["payload"], "1");
    }
    assert!(shard
        .query_rows("SELECT * FROM game_party_command_intent")
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn package_commands_refuse_forged_and_missing_world_session_tokens() {
    let shard = fixture("addon-token");
    for actor in [
        ACTOR.replace("\"guid\":1", "\"guid\":2"),
        ACTOR.replace("\"generation\":1", "\"generation\":2"),
        ACTOR.replace("\"request_nonce\":101", "\"request_nonce\":102"),
        support::actor("1"),
    ] {
        refused(
            &shard,
            &actor,
            "package.fixture.status",
            "",
            "STALE_WORLD_SESSION",
        );
    }
    shard.assert_call(
        "release_account_claim",
        &[r#"{"account_id":1,"generation":1,"request_nonce":101}"#],
    );
    refused(
        &shard,
        &support::actor("1"),
        "package.fixture.status",
        "",
        "World Session Token",
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn package_refusals_roll_back_and_leave_bot_command_dispatch_intact() {
    let shard = fixture("addon-refusal");
    for (command, payload, reason) in [
        ("package.fixture.refuse", "".into(), "FixtureRefusal"),
        (
            "package.fixture.oversized",
            "".into(),
            "InvalidPackageReply",
        ),
        ("package.fixture.unknown", "".into(), "UnknownAction"),
        ("package.absent.status", "".into(), "UnknownCommand"),
        ("package.fixture.status.extra", "".into(), "Malformed"),
        ("package.fixture.status", "x".repeat(1025), "Malformed"),
    ] {
        refused(&shard, ACTOR, command, &payload, reason);
    }
    command(&shard, ACTOR, "fixture.command", "77");
    let intents = shard.query_rows("SELECT issuer_guid, pending FROM game_party_command_intent");
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0]["issuer_guid"], "1");
    assert_eq!(intents[0]["pending"], "true");
}
