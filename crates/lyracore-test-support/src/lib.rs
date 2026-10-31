mod rng;
pub use rng::Rng;

mod module_wasm;
mod reducer_connection;
pub mod source_scan;
mod subscription;

pub use module_wasm::{module_bytes, module_bytes_with_features};
pub use reducer_connection::{ReducerConnection, ReducerRows};
pub use subscription::TableSubscription;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub fn core_root() -> &'static Path {
    static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        let path = std::env::var_os("LYRACORE_TEST_CORE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."));
        let path = path
            .canonicalize()
            .expect("the test Core checkout is missing");
        assert!(
            path.join("module/Cargo.toml").is_file(),
            "not a Core checkout: {path:?}"
        );
        path
    })
}

const POLL_INTERVAL: Duration = Duration::from_millis(50);

pub const POLL_TIMEOUT: Duration = Duration::from_secs(30);

const PUBLISH_ATTEMPTS: usize = 3;

pub fn log_dir() -> PathBuf {
    std::env::temp_dir().join("lyracore-standalone-logs")
}

// Wasmtime keys entries by Module and compiler, and its cache tolerates concurrent processes.
fn share_compile_cache(data_dir: &Path) {
    let shared = std::env::temp_dir().join("lyracore-wasmtime-cache");
    fs::create_dir_all(&shared).expect("failed to create the shared Wasm compile cache");
    let cache = data_dir.join("cache");
    fs::create_dir(&cache).expect("failed to create the standalone cache directory");
    std::os::unix::fs::symlink(&shared, cache.join("wasmtime"))
        .expect("failed to link the shared Wasm compile cache");
}

// A synchronous probe must return on its own; the deadline cannot interrupt it.
pub fn poll_until(timeout: Duration, mut probe: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return false;
        }

        let succeeded = probe();
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        if succeeded {
            return true;
        }

        thread::sleep(POLL_INTERVAL.min(deadline.duration_since(now)));
    }
}

struct SigningKeys {
    directory: PathBuf,
    startup: Mutex<()>,
}

impl Drop for SigningKeys {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn signing_keys() -> Arc<SigningKeys> {
    static KEYS: Mutex<Weak<SigningKeys>> = Mutex::new(Weak::new());
    let mut shared = KEYS.lock().unwrap();
    if let Some(keys) = shared.upgrade() {
        return keys;
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "lyracore-fixture-keys-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("failed to create private fixture signing-key directory");
    let keys = Arc::new(SigningKeys {
        directory,
        startup: Mutex::new(()),
    });
    *shared = Arc::downgrade(&keys);
    keys
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Storage {
    Memory,
    Disk,
}

pub struct Standalone {
    child: Child,
    signing_keys: Arc<SigningKeys>,
    cli_config: PathBuf,
    address: String,
    data_dir: PathBuf,
    log_path: PathBuf,
    published: bool,
    published_databases: BTreeSet<String>,
    spacetime: OsString,
    server: String,
    database: String,
    storage: Storage,
}

impl Standalone {
    pub fn start(test_name: &str) -> Self {
        Self::start_with_storage(test_name, Storage::Memory)
    }

    pub fn start_persistent(test_name: &str) -> Self {
        Self::start_with_storage(test_name, Storage::Disk)
    }

    fn start_with_storage(test_name: &str, storage: Storage) -> Self {
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
        share_compile_cache(&data_dir);
        let cli_config = data_dir.join("cli.toml");

        let log_dir = log_dir();
        fs::create_dir_all(&log_dir).expect("failed to create the standalone log directory");
        let log_path = log_dir.join(format!("{name}.log"));

        let spacetime = std::env::var_os("SPACETIME_BIN").unwrap_or_else(|| "spacetime".into());
        let address = format!("127.0.0.1:{port}");
        let server = format!("http://{address}");
        // The first node must finish creating shared signing keys before the next reads.
        let signing_keys = signing_keys();
        let startup = signing_keys
            .startup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let child = spawn_node(
            &spacetime,
            &cli_config,
            &address,
            &data_dir,
            &log_path,
            &signing_keys.directory,
            storage,
        );
        let mut standalone = Self {
            child,
            signing_keys: Arc::clone(&signing_keys),
            cli_config,
            address,
            data_dir,
            log_path,
            published: false,
            published_databases: BTreeSet::new(),
            spacetime,
            server,
            database: name,
            storage,
        };
        standalone.wait_for_server();
        drop(startup);
        standalone
    }

    pub fn server(&self) -> &str {
        &self.server
    }

    pub fn shard_name(&self) -> &str {
        &self.database
    }

    pub fn module_logs(&self, lines: usize) -> Output {
        self.command()
            .args([
                "logs",
                "--server",
                &self.server,
                "--format",
                "json",
                "--num-lines",
                &lines.to_string(),
                &self.database,
            ])
            .output()
            .expect("failed to read the test Module logs")
    }

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
        self.publish_module_bytes(module_bytes());
    }

    pub fn publish_module_bytes(&mut self, wasm: &[u8]) {
        self.publish_bytes(wasm, &[]);
    }

    pub fn publish_module_anonymous(&mut self) {
        self.publish_bytes(module_bytes(), &["--anonymous"]);
    }

    fn publish_bytes(&mut self, wasm: &[u8], extra: &[&str]) {
        let path = self.data_dir.join("published-module.wasm");
        fs::write(&path, wasm).expect("failed to copy private Wasm artifact");
        let database = self.database.clone();
        self.publish(&database, &["--bin-path", path.to_str().unwrap()], extra);
    }

    pub fn publish_named_module_bytes(&mut self, database: &str, wasm: &[u8]) {
        assert!(
            self.storage == Storage::Disk,
            "named databases need durable storage"
        );
        let path = self.data_dir.join("published-named-module.wasm");
        fs::write(&path, wasm).expect("failed to copy private named Wasm artifact");
        self.publish(database, &["--bin-path", path.to_str().unwrap()], &[]);
    }

    pub fn describe(&self) -> serde_json::Value {
        let output = self
            .command()
            .args(["describe", "-s", &self.server, "--json", &self.database])
            .output()
            .expect("failed to describe the database");
        self.assert_ok(&output);
        serde_json::from_slice(&output.stdout).expect("the schema is not JSON")
    }

    pub fn call(&self, reducer: &str, args: &[&str]) -> Output {
        self.call_database(&self.database, reducer, args)
    }

    pub fn call_database(&self, database: &str, reducer: &str, args: &[&str]) -> Output {
        let mut command = self.command();
        command.args(["call", "-s", &self.server, database, reducer, "--"]);
        command.args(args);
        command.output().expect("failed to call reducer")
    }

    pub fn connect_reducers(&self, queries: &[&str]) -> ReducerConnection {
        ReducerConnection::connect(
            &self.address,
            &self.database,
            Some(&self.owner_token()),
            queries,
        )
        .expect("failed to connect to fixture as Operator")
    }

    pub fn connect_reducers_anonymous(&self, queries: &[&str]) -> ReducerConnection {
        ReducerConnection::connect(&self.address, &self.database, None, queries)
            .expect("failed to connect anonymously to fixture")
    }

    pub fn subscribe(&self, queries: &[&str]) -> TableSubscription {
        TableSubscription::start(self.command(), &self.server, &self.database, queries)
    }

    pub fn capture_updates(
        &self,
        query: &str,
        count: u32,
        action: impl FnOnce(),
    ) -> Vec<serde_json::Value> {
        struct Subscription(Child);
        impl Drop for Subscription {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut child = Subscription(
            self.command()
                .args([
                    "subscribe",
                    "-s",
                    &self.server,
                    "--print-initial-update",
                    "--timeout",
                    "45",
                    &self.database,
                    query,
                ])
                .stdout(Stdio::piped())
                .spawn()
                .expect("failed to subscribe to fixture"),
        );
        let mut reader = BufReader::new(child.0.stdout.take().unwrap());
        let mut initial = String::new();
        assert!(reader.read_line(&mut initial).unwrap() > 0);
        serde_json::from_str::<serde_json::Value>(&initial).expect("initial subscription result");
        action();
        // Keep the CLI alive until its asynchronous stdout write reaches the pipe.
        let updates: Vec<_> = reader
            .lines()
            .take(count as usize)
            .map(|line| serde_json::from_str(&line.unwrap()).expect("transaction update"))
            .collect();
        assert_eq!(updates.len(), count as usize, "subscription ended early");
        updates
    }

    pub fn assert_call(&self, reducer: &str, args: &[&str]) {
        self.assert_ok(&self.call(reducer, args));
    }

    pub fn assert_call_database(&self, database: &str, reducer: &str, args: &[&str]) {
        self.assert_ok(&self.call_database(database, reducer, args));
    }

    pub fn call_anonymous(&self, reducer: &str, args: &[&str]) -> Output {
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
        command.output().expect("failed to call reducer")
    }

    pub fn assert_call_anonymous(&self, reducer: &str, args: &[&str]) {
        self.assert_ok(&self.call_anonymous(reducer, args));
    }

    pub fn clear_schedule(&self, table: &str) {
        self.assert_call("debug_clear_schedule", &[&format!("\"{table}\"")]);
    }

    pub fn assert_sql(&self, query: &str) {
        self.assert_sql_database(&self.database, query);
    }

    pub fn assert_sql_database(&self, database: &str, query: &str) {
        self.assert_ok(&self.sql_database(database, query));
    }

    // Baseline fixtures need their published columns, without current migration defaults.
    pub fn assert_sql_exact(&self, query: &str) {
        self.assert_ok(&self.sql_database_exact(&self.database, query));
    }

    pub fn query_rows(&self, query: &str) -> Vec<BTreeMap<String, String>> {
        self.query_database_rows(&self.database, query)
    }

    pub fn query_database_rows(
        &self,
        database: &str,
        query: &str,
    ) -> Vec<BTreeMap<String, String>> {
        let output = self.sql_database(database, query);
        self.assert_ok(&output);
        parse_text_rows(&String::from_utf8(output.stdout).expect("SQL output was not UTF-8"))
    }

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

    // SpacetimeDB 2.7.1 can segfault at launch; retry only for a new database.
    fn publish(&mut self, database: &str, source: &[&str], extra: &[&str]) {
        let workspace = core_root();
        let mut attempts = Vec::new();
        let allowed_attempts = if self.published_databases.contains(database) {
            1
        } else {
            PUBLISH_ATTEMPTS
        };
        for attempt in 0..allowed_attempts {
            let mut command = self.command();
            command
                .current_dir(workspace)
                .args(["publish", "-s", &self.server]);
            command.args(source);
            command.args(extra);
            command.args(["-y", database]);
            let output = command.output().expect("failed to start spacetime publish");
            if output.status.success() {
                self.published = true;
                self.published_databases.insert(database.to_owned());
                return;
            }
            attempts.push(describe(&output));
            let node_is_running = self
                .child
                .try_wait()
                .expect("failed to poll standalone")
                .is_none();
            if node_is_running || attempt + 1 == allowed_attempts {
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

    pub fn process_id(&self) -> u32 {
        self.child.id()
    }

    pub fn restart_persistent(&mut self) {
        assert!(self.storage == Storage::Disk, "restart needs disk storage");
        assert!(self.published, "restart needs a published Module");
        self.child.kill().expect("failed to stop owned standalone");
        self.restart();
    }

    fn restart(&mut self) {
        let _ = self.child.wait();
        self.child = spawn_node(
            &self.spacetime,
            &self.cli_config,
            &self.address,
            &self.data_dir,
            &self.log_path,
            &self.signing_keys.directory,
            self.storage,
        );
        self.wait_for_server();
    }

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

    fn sql_database(&self, database: &str, query: &str) -> Output {
        let query = lyracore_shared::import_sql::insert_defaults(query);
        self.sql_database_exact(database, &query)
    }

    fn sql_database_exact(&self, database: &str, query: &str) -> Output {
        self.command()
            .args([
                "sql",
                "-s",
                &self.server,
                "--format",
                "text",
                database,
                query,
            ])
            .output()
            .expect("failed to run SQL query")
    }

    pub fn sql_anonymous(&self, query: &str) -> Output {
        self.command()
            .args([
                "sql",
                "-s",
                &self.server,
                "--anonymous",
                "--format",
                "text",
                &self.database,
                query,
            ])
            .output()
            .expect("failed to run anonymous SQL query")
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.spacetime);
        command.args(["--config-path", self.cli_config.to_str().unwrap()]);
        command
    }

    fn assert_ok(&self, output: &Output) {
        self.assert_output_success(output, &format!("command failed with {}", output.status));
    }

    pub fn assert_output_success(&self, output: &Output, context: &str) {
        assert!(
            output.status.success(),
            "{context}\n{}{}",
            describe(output),
            self.log_tail(),
        );
    }

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

fn spawn_node(
    spacetime: &OsString,
    cli_config: &Path,
    address: &str,
    data_dir: &Path,
    log_path: &Path,
    signing_key_dir: &Path,
    storage: Storage,
) -> Child {
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .expect("failed to open the standalone log");
    let log_err = log.try_clone().expect("failed to share the standalone log");
    let mut command = Command::new(spacetime);
    command
        .args(["--config-path", cli_config.to_str().unwrap()])
        .args([
            "start",
            "--listen-addr",
            address,
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--jwt-key-dir",
            signing_key_dir.to_str().unwrap(),
            "--non-interactive",
        ]);
    if storage == Storage::Memory {
        command.arg("--in-memory");
    }
    command
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
            let values = text_row_cells(line);
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

fn text_row_cells(line: &str) -> Vec<&str> {
    let mut cells = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (index, character) in line.char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                quoted = false;
            }
        } else if character == '"' {
            quoted = true;
        } else if character == '|' {
            cells.push(line[start..index].trim());
            start = index + 1;
        }
    }
    assert!(!quoted, "unterminated quoted SQL cell: {line}");
    cells.push(line[start..].trim());
    cells
}

fn unquote(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(value)
        .to_string()
}

pub fn actor(guid: &str) -> String {
    let guid: u64 = guid.parse().expect("fixture Character guid");
    format!(r#"{{"guid":{guid},"ownership":null}}"#)
}

// `describe` returns a V10 module definition: a list of sections keyed by kind, such as "Reducers".
pub fn schema_section<'a>(schema: &'a serde_json::Value, kind: &str) -> &'a serde_json::Value {
    schema["sections"]
        .as_array()
        .expect("Module definition sections")
        .iter()
        .find_map(|section| section.get(kind))
        .unwrap_or_else(|| panic!("the Module definition has no {kind} section"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn text_rows_preserve_the_addon_reply_separator() {
        let rows = parse_text_rows(
            "id | cmd | payload\n---+-----+--------\n1 | \"example.order.result\" | \"1|Applied\"\n",
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].len(), 3);
        assert_eq!(rows[0]["id"], "1");
        assert_eq!(rows[0]["cmd"], "example.order.result");
        assert_eq!(rows[0]["payload"], "1|Applied");
    }

    #[test]
    fn text_rows_preserve_quoted_separators_inside_structured_values() {
        let rows = parse_text_rows(
            r#"id | payload | state
---+---------+-------
1 | "before \"|\" \\ after" | (some = (message = "A|B"))
"#,
        );
        assert_eq!(rows[0]["payload"], r#"before \"|\" \\ after"#);
        assert_eq!(rows[0]["state"], r#"(some = (message = "A|B"))"#);
    }

    #[test]
    #[should_panic(expected = "unterminated quoted SQL cell")]
    fn text_rows_refuse_an_unterminated_quoted_cell() {
        parse_text_rows("id | payload\n---+--------\n1 | \"1|Applied\n");
    }

    #[test]
    fn an_immediate_success_finishes_on_the_first_probe() {
        let calls = Cell::new(0);

        assert!(poll_until(Duration::from_secs(1), || {
            calls.set(calls.get() + 1);
            true
        }));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn success_after_the_deadline_is_a_timeout() {
        assert!(!poll_until(Duration::from_millis(1), || {
            thread::sleep(Duration::from_millis(10));
            true
        }));
    }

    #[test]
    fn an_expired_budget_starts_no_further_probe() {
        let calls = Cell::new(0);

        assert!(!poll_until(Duration::from_millis(25), || {
            calls.set(calls.get() + 1);
            false
        }));
        assert_eq!(calls.get(), 1);
    }
}
