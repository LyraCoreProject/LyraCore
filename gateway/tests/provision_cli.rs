use std::io::Write;
use std::process::{Command, Output, Stdio};

fn run_gateway(args: &[&str], stdin: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lyracore-gateway"))
        .args(args)
        // Keep the functional test isolated from any production/sharded environment inherited by
        // the test runner. Individual tests can add the loopback-only values they need below.
        // (These were `GW_*` until the rebrand — the stale names made every removal a no-op, so a
        // runner with a sharded environment exported leaked it into the child unnoticed.)
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
        // Rebrand fix: these were `GW_*`, which the gateway stopped reading — the URI override was
        // dead, the child fell back to the DEFAULT http://127.0.0.1:3000, and the "unreachable"
        // premise held only while nothing listened there (a live local dev node broke it).
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
