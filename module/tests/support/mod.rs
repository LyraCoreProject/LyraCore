use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct Standalone {
    child: Child,
    cli_config: PathBuf,
    data_dir: PathBuf,
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
        let data_dir = std::env::temp_dir().join(format!(
            "lyracore-{test_name}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&data_dir).expect("failed to create standalone data directory");
        let cli_config = data_dir.join("cli.toml");

        let spacetime = std::env::var_os("SPACETIME_BIN").unwrap_or_else(|| "spacetime".into());
        let address = format!("127.0.0.1:{port}");
        let server = format!("http://{address}");
        let child = Command::new(&spacetime)
            .args(["--config-path", cli_config.to_str().unwrap()])
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
        let database = format!("{test_name}-{}-{nonce}", std::process::id());
        let mut standalone = Self {
            child,
            cli_config,
            data_dir,
            spacetime,
            server,
            database,
        };
        standalone.wait_for_server(&address);
        standalone
    }

    pub fn publish_module(&self) {
        let module_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let workspace = module_dir.parent().unwrap();
        assert_success(self.command().current_dir(workspace).args([
            "publish",
            "-s",
            &self.server,
            "--module-path",
            module_dir.to_str().unwrap(),
            "--build-options=--features=debug_reducers",
            "-y",
            &self.database,
        ]));
    }

    /// Copy built Wasm bytes into this standalone's private directory and publish that copy.
    pub fn publish_module_bytes(&self, wasm: &[u8]) {
        let path = self.data_dir.join("published-module.wasm");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("failed to create private Wasm artifact");
        file.write_all(wasm)
            .expect("failed to copy private Wasm artifact");
        file.sync_all()
            .expect("failed to sync private Wasm artifact");
        let mut permissions = file
            .metadata()
            .expect("failed to inspect private Wasm artifact")
            .permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions)
            .expect("failed to make private Wasm artifact read-only");
        drop(file);

        assert_success(self.command().args([
            "publish",
            "-s",
            &self.server,
            "--bin-path",
            path.to_str().unwrap(),
            "-y",
            &self.database,
        ]));
    }

    #[allow(dead_code)] // Used when a developer's cached token is not valid for an isolated server.
    pub fn publish_module_anonymous(&self) {
        let module_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let workspace = module_dir.parent().unwrap();
        assert_success(self.command().current_dir(workspace).args([
            "publish",
            "-s",
            &self.server,
            "--module-path",
            module_dir.to_str().unwrap(),
            "--build-options=--features=debug_reducers",
            "--anonymous",
            "-y",
            &self.database,
        ]));
    }

    pub fn call(&self, reducer: &str, args: &[&str]) -> Output {
        let mut command = self.command();
        command.args(["call", "-s", &self.server, &self.database, reducer]);
        command.args(args);
        command.output().expect("failed to call reducer")
    }

    pub fn assert_call(&self, reducer: &str, args: &[&str]) {
        assert_output_success(self.call(reducer, args));
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
        assert_output_success(command.output().expect("failed to call reducer"));
    }

    #[allow(dead_code)] // Used by integration targets that inspect committed table state.
    pub fn assert_sql(&self, query: &str) {
        assert_output_success(self.sql(query));
    }

    #[allow(dead_code)] // Used by integration targets that inspect committed table state.
    pub fn query_rows(&self, query: &str) -> Vec<BTreeMap<String, String>> {
        let output = self.sql(query);
        assert_output_ok(&output);
        parse_text_rows(&String::from_utf8(output.stdout).expect("SQL output was not UTF-8"))
    }

    #[allow(dead_code)] // This shared method is used only by the expiry integration target.
    pub fn wait_until_call_succeeds(&self, reducer: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let output = self.call(reducer, &[]);
            if output.status.success() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{reducer} did not succeed in time\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn wait_for_server(&mut self, address: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if TcpStream::connect(address).is_ok() {
                return;
            }
            if let Some(status) = self.child.try_wait().expect("failed to poll standalone") {
                panic!("standalone exited before accepting connections: {status}");
            }
            assert!(
                Instant::now() < deadline,
                "standalone did not start in time"
            );
            thread::sleep(Duration::from_millis(50));
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
}

impl Drop for Standalone {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.data_dir);
    }
}

fn assert_success(command: &mut Command) {
    assert_output_success(command.output().expect("failed to start command"));
}

fn assert_output_success(output: Output) {
    assert_output_ok(&output);
}

fn assert_output_ok(output: &Output) {
    assert!(
        output.status.success(),
        "command failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
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
                .map(|(name, value)| ((*name).to_string(), value.to_string()))
                .collect()
        })
        .collect()
}
