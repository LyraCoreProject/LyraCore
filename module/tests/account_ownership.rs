mod support;

use support::Standalone;

fn token(generation: u64, nonce: u128) -> String {
    format!(r#"{{"account_id":1,"generation":{generation},"request_nonce":{nonce}}}"#)
}

fn actor(token: &str) -> String {
    format!(r#"{{"guid":1,"ownership":{{"some":{token}}}}}"#)
}

fn claim(shard: &Standalone, nonce: &str) {
    shard.assert_call("claim_account", &["1", "1", nonce]);
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
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn claim_replay_recovers_a_lost_reply_without_reopening_closed_ownership() {
    let mut shard = Standalone::start("account-claim-replay");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    let first = token(1, 101);
    claim(&shard, "101");
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
    shard.assert_call("release_account_claim", &[&first]);
    refused(
        &shard,
        "claim_account",
        &["1", "1", "101"],
        "STALE_WORLD_SESSION",
    );
    refused(
        &shard,
        "renew_account_claim",
        &[&first],
        "STALE_WORLD_SESSION",
    );
    let second = token(2, 102);
    claim(&shard, "102");
    shard.assert_call("release_account_claim", &[&first]);
    assert_eq!(
        shard.query_rows("SELECT generation,closed FROM game_account_claim")[0]["closed"],
        "false"
    );
    shard.assert_call("release_account_claim", &[&second]);
    shard.assert_call("delete_character", &["1", &support::actor("1")]);
    assert!(shard
        .query_rows("SELECT guid FROM game_character WHERE guid = 1")
        .is_empty());
    let rows = shard.query_rows("SELECT generation,closed FROM game_account_claim");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["generation"], "2");
    assert_eq!(rows[0]["closed"], "true");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn party_requests_answer_only_to_the_current_account_claim() {
    let mut realm = Standalone::start("account-realm-party");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    realm.assert_call("install_guid_range", &["0"]);
    claim(&realm, "401");
    let first = actor(&token(1, 401));
    realm.assert_call("realm_group_op", &["0", &first, "2", "0", "0", "0"]);
    let invites = realm.query_rows("SELECT * FROM game_group_invite");
    assert_eq!(invites.len(), 1);
    realm.assert_sql("UPDATE game_account_claim SET expires_micros = 0");
    claim(&realm, "402");
    refused(
        &realm,
        "realm_group_op",
        &["0", &first, "3", "0", "0", "0"],
        "STALE_WORLD_SESSION",
    );
    assert_eq!(realm.query_rows("SELECT * FROM game_group_invite"), invites);
    let current = actor(&token(2, 402));
    realm.assert_call("realm_group_op", &["0", &current, "3", "0", "0", "0"]);
    assert_eq!(realm.query_rows("SELECT * FROM game_group_invite").len(), 2);
}
