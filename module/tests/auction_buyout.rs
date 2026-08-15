use std::fs;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Standalone {
    child: Child,
    data_dir: PathBuf,
}

impl Drop for Standalone {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.data_dir);
    }
}

fn run(command: &mut Command) {
    let output = command.output().expect("failed to start command");
    assert!(
        output.status.success(),
        "command failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn wait_for_server(child: &mut Child, address: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if TcpStream::connect(address).is_ok() {
            return;
        }
        if let Some(status) = child.try_wait().expect("failed to poll standalone") {
            panic!("standalone exited before accepting connections: {status}");
        }
        assert!(
            Instant::now() < deadline,
            "standalone did not start in time"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

/// Runs only when requested because it builds and publishes the Wasm module to its own standalone.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn real_realm_reducer_commits_exact_buyout_mail_before_the_next_transaction() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to reserve a local port");
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let data_dir = std::env::temp_dir().join(format!(
        "lyracore-auction-buyout-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&data_dir).expect("failed to create standalone data directory");

    let spacetime = std::env::var_os("SPACETIME_BIN").unwrap_or_else(|| "spacetime".into());
    let address = format!("127.0.0.1:{port}");
    let server = format!("http://{address}");
    let child = Command::new(&spacetime)
        .args([
            "start",
            "--listen-addr",
            &address,
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--in-memory",
            "--non-interactive",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to start standalone");
    let mut standalone = Standalone { child, data_dir };
    wait_for_server(&mut standalone.child, &address);

    let module_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = module_dir.parent().unwrap();
    let database = format!("auction-buyout-{}-{nonce}", std::process::id());

    run(Command::new(&spacetime).current_dir(workspace).args([
        "publish",
        "-s",
        &server,
        "--module-path",
        module_dir.to_str().unwrap(),
        "--build-options=--features=debug_reducers",
        "-y",
        &database,
    ]));
    for (reducer, args) in [
        ("claim_operator", &[][..]),
        ("debug_stage_auction_buyout_fixture", &[][..]),
        (
            "realm_auction_decide_bid",
            &["509050", "509051", "509050", "900"][..],
        ),
        ("debug_verify_auction_buyout_fixture", &[][..]),
    ] {
        let mut command = Command::new(&spacetime);
        command.args(["call", "-s", &server, &database, reducer]);
        command.args(args);
        run(&mut command);
    }
}
