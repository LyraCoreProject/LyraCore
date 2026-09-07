mod support;

use support::Standalone;

fn token(generation: u64, nonce: u128) -> String {
    format!(r#"{{"account_id":1,"generation":{generation},"request_nonce":{nonce}}}"#)
}

fn actor(token: &str) -> String {
    format!(r#"{{"guid":1,"ownership":{{"some":{token}}}}}"#)
}

fn claim(shard: &Standalone, nonce: &str) -> String {
    shard.assert_call("claim_account", &["1", "1", nonce]);
    shard.query_rows("SELECT expires_micros FROM game_account_claim WHERE account_id = 1")[0]
        ["expires_micros"]
        .clone()
}

fn fence(shard: &Standalone, token: &str, deadline: &str) {
    shard.assert_call("fence_account", &[token, "\"TEST\"", "1", deadline]);
}

fn refused(shard: &Standalone, reducer: &str, args: &[&str], reason: &str) {
    let result = shard.call(reducer, args);
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !result.status.success() && output.contains(reason),
        "{reducer}: {output}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn claim_replay_recovers_a_lost_reply_without_reopening_closed_ownership() {
    let mut shard = Standalone::start("account-claim-replay");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    let first = token(1, 101);
    let deadline = claim(&shard, "101");
    let receipt = shard.query_rows("SELECT * FROM game_account_claim");
    shard.assert_call("claim_account", &["1", "1", "101"]);
    assert_eq!(
        shard.query_rows("SELECT * FROM game_account_claim"),
        receipt
    );
    refused(
        &shard,
        "claim_account",
        &["1", "1", "102"],
        "ACCOUNT_IN_USE",
    );
    fence(&shard, &first, &deadline);
    fence(&shard, &first, &deadline);
    shard.assert_call("close_account_fence", &[&first]);
    shard.assert_call("release_account_claim", &[&first]);
    refused(
        &shard,
        "claim_account",
        &["1", "1", "101"],
        "STALE_WORLD_SESSION",
    );
    refused(
        &shard,
        "fence_account",
        &[&first, "\"TEST\"", "1", &deadline],
        "STALE_WORLD_SESSION",
    );
    refused(
        &shard,
        "renew_account_claim",
        &[&first],
        "STALE_WORLD_SESSION",
    );
    refused(
        &shard,
        "renew_account_fence",
        &[&first, &deadline],
        "STALE_WORLD_SESSION",
    );
    let second = token(2, 102);
    let deadline = claim(&shard, "102");
    fence(&shard, &second, &deadline);
    shard.assert_call("close_account_fence", &[&first]);
    shard.assert_call("release_account_claim", &[&first]);
    assert_eq!(
        shard.query_rows("SELECT generation,closed FROM game_account_claim")[0]["generation"],
        "2"
    );
    assert_eq!(
        shard.query_rows("SELECT generation,closed FROM game_account_fence")[0]["closed"],
        "false"
    );
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_new_generation_completes_partial_admission_and_fences_transfer_completion() {
    let mut source = Standalone::start("account-transfer-source");
    source.publish_module();
    source.assert_call("claim_operator", &[]);
    let mut destination = Standalone::start("account-transfer-destination");
    destination.publish_module();
    destination.assert_call("claim_operator", &[]);
    let first = token(1, 201);
    let deadline = claim(&source, "201");
    fence(&source, &first, &deadline);
    // The first Gateway stopped before reaching the destination. No renewal keeps its claim alive.
    assert!(destination
        .query_rows("SELECT * FROM game_account_fence")
        .is_empty());
    source.assert_sql("UPDATE game_account_claim SET expires_micros = 0");
    let second = token(2, 202);
    let deadline = claim(&source, "202");
    fence(&source, &second, &deadline);
    fence(&destination, &second, &deadline);
    let old = actor(&first);
    let current = actor(&second);
    source.assert_call(
        "begin_transfer",
        &["1", &current, "0", "0", "100", "200", "20", "0", "true"],
    );
    let out = source.query_rows("SELECT blob FROM game_transfer_out WHERE transfer_id = 1");
    let blob = serde_json::to_string(out[0]["blob"].strip_prefix("0x").unwrap()).unwrap();
    destination.assert_call("import_character_blob", &["1", &blob, &current]);
    let arrival = destination.query_rows("SELECT * FROM game_transfer_in");
    refused(
        &destination,
        "import_character_blob",
        &["1", &blob, &old],
        "STALE_WORLD_SESSION",
    );
    refused(
        &destination,
        "release_transfer",
        &["1", &old],
        "STALE_WORLD_SESSION",
    );
    for reducer in ["confirm_import", "finish_transfer"] {
        refused(&source, reducer, &["1", &old], "STALE_WORLD_SESSION");
    }
    assert_eq!(
        destination.query_rows("SELECT * FROM game_transfer_in"),
        arrival
    );
    assert_eq!(
        source.query_rows("SELECT blob FROM game_transfer_out WHERE transfer_id = 1"),
        out
    );
    source.assert_call("confirm_import", &["1", &current]);
    source.assert_call("finish_transfer", &["1", &current]);
    destination.assert_call("release_transfer", &["1", &current]);
    source.assert_call("close_account_fence", &[&first]);
    destination.assert_call("close_account_fence", &[&first]);
    assert!(source
        .query_rows("SELECT * FROM game_character WHERE guid = 1")
        .is_empty());
    assert_eq!(
        destination
            .query_rows("SELECT * FROM game_character WHERE guid = 1")
            .len(),
        1
    );
    assert_eq!(
        source.query_rows("SELECT generation FROM game_account_fence")[0]["generation"],
        "2"
    );
    assert_eq!(
        destination.query_rows("SELECT generation FROM game_account_fence")[0]["generation"],
        "2"
    );
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn expired_account_ownership_is_reaped_while_another_gateway_keeps_its_lease_alive() {
    let mut shard = Standalone::start("account-owner-crash");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("gw_heartbeat", &[]);
    let first = token(1, 301);
    let deadline = claim(&shard, "301");
    fence(&shard, &first, &deadline);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    assert_eq!(
        shard
            .query_rows("SELECT guid FROM game_world_entity WHERE guid = 1")
            .len(),
        1
    );
    shard.assert_sql("UPDATE game_account_claim SET expires_micros = 0");
    shard.assert_sql("UPDATE game_account_fence SET expires_micros = 0");
    refused(
        &shard,
        "gw_stop_attack",
        &[&actor(&first)],
        "STALE_WORLD_SESSION",
    );
    shard.assert_call("gw_heartbeat", &[]);
    assert!(support::poll_until(
        std::time::Duration::from_secs(20),
        || shard
            .query_rows("SELECT guid FROM game_world_entity WHERE guid = 1")
            .is_empty()
    ));
    assert_eq!(
        shard.query_rows("SELECT closed FROM game_account_fence")[0]["closed"],
        "true"
    );
    // Retained, expired generations protect old requests without preventing a fresh admission.
    let second = token(2, 302);
    let deadline = claim(&shard, "302");
    fence(&shard, &second, &deadline);
}
