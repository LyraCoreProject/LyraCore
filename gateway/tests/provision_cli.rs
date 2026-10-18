use std::io::Write;
use std::process::{Command, Output, Stdio};

fn run_gateway(args: &[&str], stdin: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lyracore-gateway"))
        .args(args)
        .env_remove("LYRACORE_COORDINATOR_TOKEN")
        .env_remove("LYRACORE_DATABASE")
        .env_remove("LYRACORE_SPACETIMEDB_URL")
        .env_remove("LYRACORE_SHARD_MAP")
        .env_remove("LYRACORE_SHARD_MAP_FILE")
        .env_remove("LYRACORE_REALM_CORE")
        .env_remove("RUST_LOG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn gateway");
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    child.wait_with_output().expect("wait for gateway")
}

fn run_gateway_against_unreachable_loopback(args: &[&str], stdin: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lyracore-gateway"))
        .args(args)
        .env("LYRACORE_COORDINATOR_TOKEN", "functional-test-token")
        .env("LYRACORE_DATABASE", "spacetime-core")
        .env("LYRACORE_SPACETIMEDB_URL", "http://127.0.0.1:0")
        .env_remove("LYRACORE_SHARD_MAP")
        .env_remove("LYRACORE_SHARD_MAP_FILE")
        .env_remove("LYRACORE_REALM_CORE")
        .env_remove("RUST_LOG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn gateway");
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    child.wait_with_output().expect("wait for gateway")
}

#[test]
fn legacy_positional_password_is_rejected_before_any_connection_attempt() {
    let secret = "positional-secret";
    let output = run_gateway(&["provision", "TEST", secret], b"");
    assert!(!output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("gateway provision <USERNAME> --password-stdin"));
    assert!(!stderr.contains(secret));
    assert!(!stderr.contains("connect"));
}

#[test]
fn password_stdin_errors_do_not_echo_the_password() {
    let secret = b"1234567890abcdefg\n";
    let output = run_gateway(&["provision", "TEST", "--password-stdin"], secret);
    assert!(!output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("input exceeds 16 bytes"));
    assert!(!stderr.contains("1234567890abcdefg"));
    assert!(!stderr.contains("connect"));
}

#[test]
fn username_is_validated_before_connecting() {
    let secret = "stdin-secret";
    let output = run_gateway(
        &["provision", "NOT\u{00c4}SCII", "--password-stdin"],
        format!("{secret}\n").as_bytes(),
    );
    assert!(!output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("invalid username"));
    assert!(!stderr.contains(secret));
    assert!(!stderr.contains("connect"));
}

#[test]
fn valid_password_stdin_reaches_provisioning_without_disclosing_the_password() {
    let secret = "valid pass";
    let output = run_gateway_against_unreachable_loopback(
        &["provision", "TEST", "--password-stdin"],
        format!("{secret}\n").as_bytes(),
    );
    assert!(!output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.to_ascii_lowercase().contains("connect"),
        "valid stdin should reach the deliberately unreachable coordinator: {stderr}"
    );
    assert!(!stderr.contains(secret));
    assert!(!stderr.contains("functional-test-token"));
    assert!(!stderr.contains("invalid password"));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn provision_writes_the_account_to_the_world_shard_and_realm_core() {
    use lyracore_test_support::{module_bytes, Standalone};
    const REALM: &str = "provision-realm";

    let mut standalone = Standalone::start_persistent("provision-world");
    standalone.publish_module();
    standalone.publish_named_module_bytes(REALM, module_bytes());
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call_database(REALM, "claim_operator", &[]);

    let provision = |username: &str, password: &[u8], create_only: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lyracore-gateway"));
        command.args(["provision", username, "--password-stdin"]);
        if create_only {
            command.arg("--create-only");
        }
        let mut child = command
            .env("LYRACORE_COORDINATOR_TOKEN", standalone.owner_token())
            .env("LYRACORE_DATABASE", standalone.shard_name())
            .env("LYRACORE_SPACETIMEDB_URL", standalone.server())
            .env("LYRACORE_REALM_CORE", REALM)
            .env_remove("LYRACORE_SHARD_MAP")
            .env_remove("LYRACORE_SHARD_MAP_FILE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn gateway");
        child.stdin.take().unwrap().write_all(password).unwrap();
        child.wait_with_output().expect("wait for gateway")
    };
    let output = provision("NEWHERO", b"hero pass\n", true);
    assert!(
        output.status.success(),
        "provision failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let query = "SELECT * FROM game_account WHERE username = 'NEWHERO'";
    let databases = [standalone.shard_name().to_owned(), REALM.to_owned()];
    let original: Vec<_> = databases
        .iter()
        .map(|database| standalone.query_database_rows(database, query))
        .collect();
    for rows in &original {
        assert_eq!(rows.len(), 1);
    }

    let output = provision("newhero", b"different pass\n", true);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("account already exists"));
    for (database, accounts) in databases.iter().zip(&original) {
        assert_eq!(standalone.query_database_rows(database, query), *accounts);
    }

    let output = provision("newhero", b"rotated pass\n", false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for (database, accounts) in databases.iter().zip(&original) {
        let rotated = standalone.query_database_rows(database, query);
        assert_eq!(rotated.len(), 1);
        assert_eq!(rotated[0]["id"], accounts[0]["id"]);
        assert_ne!(rotated[0]["salt"], accounts[0]["salt"]);
        assert_ne!(rotated[0]["verifier"], accounts[0]["verifier"]);
    }
}
