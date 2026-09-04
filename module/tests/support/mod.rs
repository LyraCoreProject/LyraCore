use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How often every wait in this suite re-checks its condition.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The deadline every wait uses unless it says otherwise. Deliberately generous: a wait that gives
/// up early is a flake, and a condition that is already true costs one poll either way.
pub const POLL_TIMEOUT: Duration = Duration::from_secs(30);

/// How many times a publish may be retried past a node that died launching the module.
const PUBLISH_ATTEMPTS: usize = 3;

/// Where each standalone's own stdout and stderr land. Outside the data directory on purpose — the
/// data directory goes away with the node, and a crash is exactly when the log is worth reading.
/// CI collects this directory as an artifact.
pub fn log_dir() -> PathBuf {
    std::env::temp_dir().join("lyracore-standalone-logs")
}

/// Poll `probe` every [`POLL_INTERVAL`] until it answers `true`. Returns `false` when `timeout`
/// passes first, so the caller reports what it was waiting for rather than a bare timeout.
pub fn poll_until(timeout: Duration, mut probe: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if probe() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(POLL_INTERVAL);
    }
}

pub struct Standalone {
    child: Child,
    cli_config: PathBuf,
    address: String,
    data_dir: PathBuf,
    log_path: PathBuf,
    spacetime: OsString,
    server: String,
    database: String,
}

impl Standalone {
    pub fn start(test_name: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("failed to reserve a local port");
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let name = format!("{test_name}-{}-{nonce}", std::process::id());
        let data_dir = std::env::temp_dir().join(format!("lyracore-{name}"));
        fs::create_dir(&data_dir).expect("failed to create standalone data directory");
        let cli_config = data_dir.join("cli.toml");

        let log_dir = log_dir();
        fs::create_dir_all(&log_dir).expect("failed to create the standalone log directory");
        let log_path = log_dir.join(format!("{name}.log"));

        let spacetime = std::env::var_os("SPACETIME_BIN").unwrap_or_else(|| "spacetime".into());
        let address = format!("127.0.0.1:{port}");
        let server = format!("http://{address}");
        let child = spawn_node(&spacetime, &cli_config, &address, &data_dir, &log_path);
        let mut standalone = Self {
            child,
            cli_config,
            address,
            data_dir,
            log_path,
            spacetime,
            server,
            database: name,
        };
        standalone.wait_for_server();
        standalone
    }

    #[allow(dead_code)] // Used by Gateway tests that connect through the SDK.
    pub fn server(&self) -> &str {
        &self.server
    }

    #[allow(dead_code)] // Used by Gateway tests that connect through the SDK.
    pub fn shard_name(&self) -> &str {
        &self.database
    }

    #[allow(dead_code)] // Used by Gateway tests that need the private shard's Owner Token.
    pub fn owner_token(&self) -> String {
        let config = fs::read_to_string(&self.cli_config).expect("private CLI config is missing");
        config
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once('=')?;
                (key.trim() == "spacetimedb_token")
                    .then(|| value.trim().trim_matches('"').to_owned())
            })
            .expect("private Owner Token is missing")
    }

    pub fn publish_module(&mut self) {
        self.publish(&[]);
    }

    /// Copy built Wasm bytes into this standalone's private directory and publish that copy.
    #[allow(dead_code)] // Used by tests that build their own Wasm artifact.
    pub fn publish_module_bytes(&self, wasm: &[u8]) {
        let path = self.data_dir.join("published-module.wasm");
        fs::write(&path, wasm).expect("failed to copy private Wasm artifact");

        let output = self
            .command()
            .args([
            "publish",
            "-s",
            &self.server,
            "--bin-path",
            path.to_str().unwrap(),
            "-y",
            &self.database,
            ])
            .output()
            .expect("failed to start spacetime publish");
        self.assert_ok(&output);
    }

    #[allow(dead_code)] // Used when a developer's cached token is not valid for an isolated server.
    pub fn publish_module_anonymous(&mut self) {
        self.publish(&["--anonymous"]);
    }

    pub fn call(&self, reducer: &str, args: &[&str]) -> Output {
        let mut command = self.command();
        command.args(["call", "-s", &self.server, &self.database, reducer]);
        command.args(args);
        command.output().expect("failed to call reducer")
    }

    pub fn assert_call(&self, reducer: &str, args: &[&str]) {
        self.assert_ok(&self.call(reducer, args));
    }

    #[allow(dead_code)] // Paired with `publish_module_anonymous` for isolated local servers.
    pub fn assert_call_anonymous(&self, reducer: &str, args: &[&str]) {
        let mut command = self.command();
        command.args([
            "call",
            "-s",
            &self.server,
            "--anonymous",
            &self.database,
            reducer,
        ]);
        command.args(args);
        self.assert_ok(&command.output().expect("failed to call reducer"));
    }

    #[allow(dead_code)] // Used by integration targets that inspect committed table state.
    pub fn assert_sql(&self, query: &str) {
        self.assert_ok(&self.sql(query));
    }

    #[allow(dead_code)] // Used by integration targets that inspect committed table state.
    pub fn query_rows(&self, query: &str) -> Vec<BTreeMap<String, String>> {
        let output = self.sql(query);
        self.assert_ok(&output);
        parse_text_rows(&String::from_utf8(output.stdout).expect("SQL output was not UTF-8"))
    }

    /// Wait for a reducer that refuses until the durable state it checks has settled. The reducer
    /// itself is the condition, so the test waits exactly as long as the schedule behind it needs.
    #[allow(dead_code)] // Used by the integration targets that wait on a scheduled outcome.
    pub fn wait_until_call_succeeds(&self, reducer: &str, args: &[&str]) {
        let mut last = None;
        let settled = poll_until(POLL_TIMEOUT, || {
            let output = self.call(reducer, args);
            let ok = output.status.success();
            last = Some(output);
            ok
        });
        assert!(
            settled,
            "{reducer} did not succeed in time\n{}{}",
            describe(last.as_ref().expect("at least one attempt")),
            self.log_tail(),
        );
    }

    /// Publish the module, restarting the node when it dies mid-publish.
    ///
    /// `spacetimedb-standalone` 2.7.1 segfaults while launching this module roughly once in a dozen
    /// publishes (SIGSEGV, no log line past `launching module`). The node is `--in-memory`, so a
    /// restart before the first publish loses nothing. A node that survives, or a refusal the node
    /// itself returned, is reported on the spot rather than retried.
    fn publish(&mut self, extra: &[&str]) {
        let module_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let workspace = module_dir.parent().unwrap();
        let mut attempts = Vec::new();
        for _ in 0..PUBLISH_ATTEMPTS {
            let mut command = self.command();
            command.current_dir(workspace).args([
                "publish",
                "-s",
                &self.server,
                "--module-path",
                module_dir.to_str().unwrap(),
                "--build-options=--features=debug_reducers",
            ]);
            command.args(extra);
            command.args(["-y", &self.database]);
            let output = command.output().expect("failed to start spacetime publish");
            if output.status.success() {
                return;
            }
            attempts.push(describe(&output));
            if self
                .child
                .try_wait()
                .expect("failed to poll standalone")
                .is_none()
            {
                break;
            }
            self.restart();
        }
        panic!(
            "spacetime publish failed after {} attempt(s)\n{}{}",
            attempts.len(),
            attempts.join("\n"),
            self.log_tail(),
        );
    }

    /// Start a replacement node on the same address, appending to the same log.
    fn restart(&mut self) {
        let _ = self.child.wait();
        self.child = spawn_node(
            &self.spacetime,
            &self.cli_config,
            &self.address,
            &self.data_dir,
            &self.log_path,
        );
        self.wait_for_server();
    }

    /// Not `poll_until`: a node that has already exited never starts listening, so this one waits
    /// on two outcomes and reports the exit rather than burning the whole deadline on it.
    fn wait_for_server(&mut self) {
        let deadline = Instant::now() + POLL_TIMEOUT;
        loop {
            if TcpStream::connect(&self.address).is_ok() {
                return;
            }
            if let Some(status) = self.child.try_wait().expect("failed to poll standalone") {
                panic!(
                    "standalone exited before accepting connections: {status}\n{}",
                    self.log_tail()
                );
            }
            assert!(
                Instant::now() < deadline,
                "standalone did not start in time\n{}",
                self.log_tail()
            );
            thread::sleep(POLL_INTERVAL);
        }
    }

    fn sql(&self, query: &str) -> Output {
        self.command()
            .args([
                "sql",
                "-s",
                &self.server,
                "--format",
                "text",
                &self.database,
                query,
            ])
            .output()
            .expect("failed to run SQL query")
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.spacetime);
        command.args(["--config-path", self.cli_config.to_str().unwrap()]);
        command
    }

    fn assert_ok(&self, output: &Output) {
        assert!(
            output.status.success(),
            "command failed with {}\n{}{}",
            output.status,
            describe(output),
            self.log_tail(),
        );
    }

    /// The end of this node's own log, appended to every failure. A publish that reports only
    /// "connection closed" says nothing; the node's last lines say why it closed it.
    fn log_tail(&self) -> String {
        let Ok(log) = fs::read_to_string(&self.log_path) else {
            return String::new();
        };
        let lines: Vec<&str> = log.lines().collect();
        let tail = lines[lines.len().saturating_sub(40)..].join("\n");
        format!("standalone log ({}):\n{tail}\n", self.log_path.display())
    }
}

impl Drop for Standalone {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.data_dir);
    }
}

/// Start one standalone node, appending its stdout and stderr to `log_path`.
fn spawn_node(
    spacetime: &OsString,
    cli_config: &Path,
    address: &str,
    data_dir: &Path,
    log_path: &Path,
) -> Child {
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .expect("failed to open the standalone log");
    let log_err = log.try_clone().expect("failed to share the standalone log");
    Command::new(spacetime)
        .args(["--config-path", cli_config.to_str().unwrap()])
        .args([
            "start",
            "--listen-addr",
            address,
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--in-memory",
            "--non-interactive",
        ])
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        .spawn()
        .expect("failed to start standalone")
}

fn describe(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}\n",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

fn parse_text_rows(output: &str) -> Vec<BTreeMap<String, String>> {
    let mut lines = output.lines().filter(|line| !line.trim().is_empty());
    let headers: Vec<&str> = lines
        .next()
        .expect("SQL output had no header")
        .split('|')
        .map(str::trim)
        .collect();
    let separator = lines.next().expect("SQL output had no separator");
    assert!(
        separator.chars().all(|c| c == '-' || c == '+'),
        "unexpected SQL separator: {separator}"
    );

    lines
        .map(|line| {
            let values: Vec<&str> = line.split('|').map(str::trim).collect();
            assert_eq!(
                values.len(),
                headers.len(),
                "unexpected SQL row shape: {line}"
            );
            headers
                .iter()
                .zip(values)
                .map(|(name, value)| ((*name).to_string(), unquote(value)))
                .collect()
        })
        .collect()
}

/// `spacetime sql --format text` prints a string column inside double quotes. Callers compare
/// against the value, not against its rendering, so the quotes come off here once.
fn unquote(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(value)
        .to_string()
}
