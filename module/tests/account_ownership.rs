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
    shard.assert_call("install_guid_range", &["0"]);
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
    shard.assert_call("close_account_fence", &[&second]);
    shard.assert_call("release_account_claim", &[&second]);
    shard.assert_call("delete_character", &["1", &support::actor("1")]);
    assert!(shard
        .query_rows("SELECT guid FROM game_character WHERE guid = 1")
        .is_empty());
    for table in ["game_account_claim", "game_account_fence"] {
        let rows = shard.query_rows(&format!("SELECT generation,closed FROM {table}"));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["generation"], "2");
        assert_eq!(rows[0]["closed"], "true");
    }
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_new_generation_completes_partial_admission_and_fences_transfer_completion() {
    let mut source = Standalone::start("account-transfer-source");
    source.publish_module();
    source.assert_call("claim_operator", &[]);
    source.assert_call("install_guid_range", &["0"]);
    let mut destination = Standalone::start("account-transfer-destination");
    destination.publish_module();
    destination.assert_call("claim_operator", &[]);
    destination.assert_call("install_guid_range", &["1000000000"]);
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
    for shard in [&source, &destination] {
        shard.assert_call("provision_account", &["\"OTHER\"", "[]", "[]"]);
    }
    let other_account =
        source.query_rows("SELECT id FROM game_account WHERE username = 'OTHER'")[0]["id"].clone();
    source.assert_call("claim_account", &[&other_account, "2", "203"]);
    let other_deadline = source.query_rows(&format!(
        "SELECT expires_micros FROM game_account_claim WHERE account_id = {other_account}"
    ))[0]["expires_micros"]
        .clone();
    let other_token =
        format!(r#"{{"account_id":{other_account},"generation":1,"request_nonce":203}}"#);
    let other = format!(r#"{{"guid":2,"ownership":{{"some":{other_token}}}}}"#);
    for shard in [&source, &destination] {
        shard.assert_call(
            "fence_account",
            &[&other_token, "\"OTHER\"", "2", &other_deadline],
        );
    }
    source.assert_call(
        "begin_transfer",
        &["1", &current, "0", "0", "100", "200", "20", "0", "true"],
    );
    let out = source.query_rows("SELECT blob FROM game_transfer_out WHERE transfer_id = 1");
    let blob = serde_json::to_string(out[0]["blob"].strip_prefix("0x").unwrap()).unwrap();
    destination.assert_call("import_character_blob", &["1", &blob, &current]);
    let arrival = destination.query_rows("SELECT * FROM game_transfer_in");
    for unrelated in [&other[..], r#"{"guid":0,"ownership":null}"#] {
        refused(
            &destination,
            "import_character_blob",
            &["1", &blob, unrelated],
            "STALE_WORLD_SESSION",
        );
        refused(
            &destination,
            "release_transfer",
            &["1", unrelated],
            "STALE_WORLD_SESSION",
        );
        for reducer in ["confirm_import", "finish_transfer"] {
            refused(&source, reducer, &["1", unrelated], "STALE_WORLD_SESSION");
        }
        refused(
            &source,
            "set_character_shard",
            &["1", "0", "0", unrelated],
            "STALE_WORLD_SESSION",
        );
    }
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
    shard.assert_call("install_guid_range", &["0"]);
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
    refused(
        &shard,
        "gw_ack_taxi_reply",
        &[&actor(&first), "1"],
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

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn realm_core_party_requests_use_the_account_claim_without_a_world_shard_fence() {
    let mut realm = Standalone::start("account-realm-party");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    realm.assert_call("install_guid_range", &["0"]);
    claim(&realm, "401");
    let first = actor(&token(1, 401));
    realm.assert_call("realm_group_op", &["0", &first, "2", "0", "0"]);
    let invites = realm.query_rows("SELECT * FROM game_group_invite");
    assert_eq!(invites.len(), 1);
    realm.assert_sql("UPDATE game_account_claim SET expires_micros = 0");
    claim(&realm, "402");
    refused(
        &realm,
        "realm_group_op",
        &["0", &first, "3", "0", "0"],
        "STALE_WORLD_SESSION",
    );
    assert_eq!(realm.query_rows("SELECT * FROM game_group_invite"), invites);
    let current = actor(&token(2, 402));
    realm.assert_call("realm_group_op", &["0", &current, "3", "0", "0"]);
    assert_eq!(realm.query_rows("SELECT * FROM game_group_invite").len(), 2);
    assert!(realm
        .query_rows("SELECT * FROM game_account_fence")
        .is_empty());
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn first_admission_cleans_each_legacy_character_with_a_shared_identity() {
    let mut shard = Standalone::start("account-legacy-identity");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call(
        "create_character",
        &["1", "\"Legacy\"", "1", "1", "0", "0", "0", "0", "0", "0"],
    );
    let second = shard.query_rows("SELECT guid FROM game_character WHERE name = 'Legacy'")[0]
        ["guid"]
        .clone();
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call("debug_spawn_player_entity", &[&second]);
    let owners =
        shard.query_rows("SELECT owner_identity FROM game_world_entity WHERE account_id = 1");
    assert_eq!(owners.len(), 2);
    assert_eq!(owners[0], owners[1]);
    let mut other_entities =
        shard.query_rows("SELECT guid FROM game_world_entity WHERE account_id = 0");
    other_entities.sort();
    assert!(!other_entities.is_empty());
    shard.assert_sql("UPDATE game_world_entity SET money = 123 WHERE guid = 1");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET money = 456 WHERE guid = {second}"
    ));
    let deadline = claim(&shard, "501");
    fence(&shard, &token(1, 501), &deadline);
    assert!(shard
        .query_rows("SELECT guid FROM game_world_entity WHERE account_id = 1")
        .is_empty());
    let mut remaining = shard.query_rows("SELECT guid FROM game_world_entity WHERE account_id = 0");
    remaining.sort();
    assert_eq!(remaining, other_entities);
    assert_eq!(
        shard.query_rows("SELECT money FROM game_character WHERE guid = 1")[0]["money"],
        "123"
    );
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT money FROM game_character WHERE guid = {second}"
        ))[0]["money"],
        "456"
    );
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn expired_fences_make_progress_in_bounded_batches() {
    let mut shard = Standalone::start("account-fence-batches");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    let mut rows: Vec<_> = (1..=65)
        .map(|id| format!("({id},'TEST',1,1,{id},0,false)"))
        .collect();
    rows.push("(66,'TEST',1,1,66,9223372036854775807,false)".into());
    let updates = shard.capture_updates(
        "SELECT * FROM game_account_fence WHERE closed = true",
        2,
        || {
            shard.assert_sql(&format!(
                "INSERT INTO game_account_fence (account_id,account_name,generation,request_nonce,character_guid,expires_micros,closed) VALUES {}",
                rows.join(",")
            ));
        },
    );
    let batch_sizes: Vec<_> = updates
        .iter()
        .map(|update| {
            update["game_account_fence"]["inserts"]
                .as_array()
                .unwrap()
                .len()
        })
        .collect();
    assert_eq!(batch_sizes, [64, 1]);
    assert_eq!(
        shard
            .query_rows("SELECT account_id FROM game_account_fence WHERE closed = true")
            .len(),
        65
    );
    assert!(shard
        .query_rows("SELECT guid FROM game_world_entity WHERE guid = 1")
        .is_empty());
    assert_eq!(
        shard.query_rows("SELECT account_id FROM game_account_fence WHERE closed = false")[0]
            ["account_id"],
        "66"
    );
}
