use std::io::Write;
use std::process::{Command, Stdio};

fn gateway() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lyracore-gateway"));
    command
        .env_clear()
        .env("LYRACORE_SPACETIMEDB_URL", "http://127.0.0.1:0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

#[test]
fn malformed_admission_limits_fail_before_connecting() {
    for name in ["LYRACORE_MAX_SESSIONS", "LYRACORE_ADMIT_CONCURRENCY"] {
        let output = gateway().env(name, "1,000").output().unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains(name), "missing config failure: {stderr}");
        assert!(stderr.contains("1,000"), "missing invalid value: {stderr}");
        assert!(!stderr.to_ascii_lowercase().contains("connect"), "{stderr}");
    }
}

#[test]
fn provisioning_does_not_read_serving_admission_limits() {
    let mut child = gateway()
        .args(["provision", "TEST", "--password-stdin"])
        .env("LYRACORE_MAX_SESSIONS", "invalid")
        .env("LYRACORE_ADMIT_CONCURRENCY", "invalid")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"test pass\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.to_ascii_lowercase().contains("connect"), "{stderr}");
    assert!(!stderr.contains("LYRACORE_MAX_SESSIONS"), "{stderr}");
    assert!(!stderr.contains("LYRACORE_ADMIT_CONCURRENCY"), "{stderr}");
}
