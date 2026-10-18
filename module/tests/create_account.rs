mod support;

use support::Standalone;

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn create_only_preserves_existing_credentials_authority_and_session() {
    let mut shard = Standalone::start("create-account");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("create_account", &[r#""NewHero""#, "[1,2]", "[3,4]"]);
    let accounts = shard.query_rows("SELECT * FROM game_account WHERE username = 'NEWHERO'");
    assert_eq!(accounts.len(), 1);
    let id = &accounts[0]["id"];
    shard.assert_sql(&format!(
        "UPDATE game_account SET banned = true WHERE id = {id}"
    ));
    shard.assert_call("revoke_alpha_test_tools", &[r#""NEWHERO""#]);
    shard.assert_call(
        "establish_session",
        &[id, "[9,8,7]", r#"{"__identity__":"0x1"}"#],
    );
    let accounts = shard.query_rows("SELECT * FROM game_account");
    let sessions = shard.query_rows("SELECT * FROM game_session");

    let refusal = shard.call("create_account", &[r#""newhero""#, "[5,6]", "[7,8]"]);
    assert!(!refusal.status.success());
    assert!(String::from_utf8_lossy(&refusal.stderr).contains("account already exists"));
    assert_eq!(shard.query_rows("SELECT * FROM game_account"), accounts);
    assert_eq!(shard.query_rows("SELECT * FROM game_session"), sessions);

    let refusal = shard.call_anonymous("create_account", &[r#""OTHER""#, "[]", "[]"]);
    assert!(!refusal.status.success());
    assert_eq!(shard.query_rows("SELECT * FROM game_account"), accounts);

    shard.assert_call("provision_account", &[r#""NEWHERO""#, "[5,6]", "[7,8]"]);
    let rotated = shard.query_rows("SELECT * FROM game_account");
    let mut expected = accounts;
    let account = expected
        .iter_mut()
        .find(|account| account["username"] == "NEWHERO")
        .unwrap();
    account.insert("salt".to_owned(), "0x0506".to_owned());
    account.insert("verifier".to_owned(), "0x0708".to_owned());
    assert_eq!(rotated, expected);
    assert_eq!(shard.query_rows("SELECT * FROM game_session"), sessions);
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn concurrent_creates_have_one_winner_for_a_normalized_name() {
    let mut shard = Standalone::start("create-account-race");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    let (first, second) = std::thread::scope(|scope| {
        let first = scope.spawn(|| shard.call("create_account", &[r#""Hero""#, "[1]", "[2]"]));
        let second = scope.spawn(|| shard.call("create_account", &[r#""hERO""#, "[3]", "[4]"]));
        (first.join().unwrap(), second.join().unwrap())
    });
    assert_ne!(first.status.success(), second.status.success());
    let accounts = shard.query_rows("SELECT * FROM game_account WHERE username = 'HERO'");
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0]["username"], "HERO");
    let expected = if first.status.success() {
        ("0x01", "0x02")
    } else {
        ("0x03", "0x04")
    };
    assert_eq!(accounts[0]["salt"], expected.0);
    assert_eq!(accounts[0]["verifier"], expected.1);
}
