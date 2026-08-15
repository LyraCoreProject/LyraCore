use std::fs;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
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

fn run(command: &mut Command) -> Output {
    let output = command.output().expect("failed to start command");
    assert!(
        output.status.success(),
        "command failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    output
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

fn call(spacetime: &Path, server: &str, database: &str, reducer: &str) -> Output {
    Command::new(spacetime)
        .args(["call", "-s", server, database, reducer])
        .output()
        .expect("failed to call reducer")
}

/// Runs only when requested because it builds and publishes the Wasm module to its own standalone.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn scheduled_bid_expiry_settles_once_and_a_callback_replay_is_a_no_op() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to reserve a local port");
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let data_dir = std::env::temp_dir().join(format!(
        "lyracore-auction-expiry-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&data_dir).expect("failed to create standalone data directory");

    let spacetime =
        PathBuf::from(std::env::var_os("SPACETIME_BIN").unwrap_or_else(|| "spacetime".into()));
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
    let database = format!("auction-expiry-{}-{nonce}", std::process::id());

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
    for reducer in ["claim_operator", "debug_stage_auction_expiry_fixture"] {
        let mut command = Command::new(&spacetime);
        command.args(["call", "-s", &server, &database, reducer]);
        run(&mut command);
    }

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let output = call(
            &spacetime,
            &server,
            &database,
            "debug_verify_auction_expiry_fixture",
        );
        if output.status.success() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "scheduled expiry did not settle in time\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        thread::sleep(Duration::from_millis(50));
    }

    for reducer in [
        "debug_replay_auction_expiry_fixture",
        "debug_verify_auction_expiry_fixture",
        "debug_verify_auction_expiry_fixture",
    ] {
        let mut command = Command::new(&spacetime);
        command.args(["call", "-s", &server, &database, reducer]);
        run(&mut command);
    }

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
    for reducer in [
        "debug_repair_after_publish",
        "debug_verify_auction_expiry_fixture",
    ] {
        let mut command = Command::new(&spacetime);
        command.args(["call", "-s", &server, &database, reducer]);
        run(&mut command);
    }
}
