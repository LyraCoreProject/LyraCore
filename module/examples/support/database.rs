use std::path::PathBuf;
use std::time::Duration;

use lyracore_module::native_prototype::benchmark::{workload, SCENARIOS};
use lyracore_test_support::Standalone;
use serde_json::{json, Value};

use super::support;

pub struct Database {
    pub shard: Standalone,
    pub artifacts: Value,
    manifest: PathBuf,
}

impl Database {
    pub fn start(wasm: &str, library: &std::ffi::OsStr) -> Self {
        assert!(std::env::var_os("LYRACORE_NATIVE_DATABASE_MANIFEST").is_none());
        let library = std::fs::canonicalize(library).unwrap();
        let server = std::env::var_os("LYRACORE_NATIVE_DATABASE_SERVER")
            .expect("set LYRACORE_NATIVE_DATABASE_SERVER to the experimental standalone binary");
        let hash = |path: &std::path::Path| {
            blake3::hash(&std::fs::read(path).unwrap())
                .to_hex()
                .to_string()
        };
        let manifest = std::env::temp_dir().join(format!(
            "native-database-manifest-{}.json",
            std::process::id()
        ));
        let contents = json!({
            "library": library,
            "native_blake3": hash(&library),
            "wasm_blake3": hash(&PathBuf::from(wasm)),
        });
        let mut file = std::fs::File::create_new(&manifest).unwrap();
        serde_json::to_writer(&mut file, &contents).unwrap();
        std::env::set_var("LYRACORE_NATIVE_DATABASE_MANIFEST", &manifest);
        let shard = support::start();
        std::env::remove_var("LYRACORE_NATIVE_DATABASE_MANIFEST");
        Self {
            shard,
            artifacts: json!({
                "server_blake3": hash(&PathBuf::from(server)),
                "module_native_blake3": contents["native_blake3"],
                "server_revision": "8ecd3c3bac427a49a9d9b13671bf83324afa59ba",
            }),
            manifest,
        }
    }

    fn verify_movement(
        &mut self,
        reference: &Standalone,
        reference_calls: &support::Calls,
        native_calls: &support::Calls,
    ) {
        verify_movement_trace(reference, reference_calls);
        verify_movement_trace(&self.shard, native_calls);
        self.artifacts["movement_reference_frames"] = json!(60);
        eprintln!("Native database: both implementations match the 60-frame movement reference");
        for &scenario in SCENARIOS {
            let case = workload(scenario, 4).unwrap();
            let expected_run = support::setup(reference, reference_calls, scenario, 4, false);
            let native_run = support::setup(&self.shard, native_calls, scenario, 4, false);
            let expected_updates = support::subscribe(reference);
            let native_updates = support::subscribe(&self.shard);
            for request in &case.requests {
                for (calls, run) in [(reference_calls, expected_run), (native_calls, native_run)] {
                    calls
                        .call(
                            "debug_native_committed_step",
                            &(run, request.sequence, request.at_micros),
                        )
                        .assert_accepted();
                }
                assert_eq!(
                    support::gameplay_update(
                        expected_updates.receive(Duration::from_secs(30)).value
                    ),
                    support::gameplay_update(native_updates.receive(Duration::from_secs(30)).value),
                    "{scenario}, tick {}: subscription parity",
                    request.sequence,
                );
                let expected = support::snapshot(
                    reference,
                    reference_calls,
                    expected_run,
                    request.sequence,
                    true,
                );
                let actual = support::snapshot(
                    &self.shard,
                    native_calls,
                    native_run,
                    request.sequence,
                    true,
                );
                assert_eq!(
                    expected["frame"], actual["frame"],
                    "{scenario}: committed frame parity"
                );
                assert_eq!(expected["digest"], actual["digest"]);
            }
        }
        let logs = self.shard.module_logs(200);
        assert!(logs.status.success());
        let native_reducers: std::collections::BTreeSet<String> = String::from_utf8(logs.stdout)
            .unwrap()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|record| record["message"] == "LYRACORE_NATIVE_DATABASE_V1")
            .map(|record| record["function"].as_str().unwrap().to_owned())
            .collect();
        for reducer in [
            "debug_native_simulation_reference",
            "debug_native_committed_step",
        ] {
            assert!(
                native_reducers.contains(reducer),
                "no native evidence for {reducer}"
            );
        }
        eprintln!("Native database: 21 committed frames and subscription updates match Wasm");
    }

    pub fn verify(&mut self, reference: &mut Standalone) {
        let reference_calls = support::Calls::new(reference);
        let mut native_calls = support::Calls::new(&self.shard);
        self.verify_movement(reference, &reference_calls, &native_calls);
        let loaded_libraries = native_libraries(self.shard.process_id());
        assert!(loaded_libraries > 0);
        self.artifacts["native_database_libraries_before_trap"] = json!(loaded_libraries);

        let case = workload("combat_advance", 4).unwrap();
        let expected_run = support::setup(reference, &reference_calls, case.name, 4, false);
        let native_run = support::setup(&self.shard, &native_calls, case.name, 4, false);
        for (shard, calls, run) in [
            (&*reference, &reference_calls, expected_run),
            (&self.shard, &native_calls, native_run),
        ] {
            let before = support::snapshot(shard, calls, run, 0, true);
            let request = &case.requests[0];
            for (args, message) in [
                (
                    (run + 1, request.sequence, request.at_micros),
                    "benchmark run or execution mode is stale",
                ),
                (
                    (run, request.sequence + 1, request.at_micros),
                    "benchmark commits must arrive in sequence",
                ),
            ] {
                let outcome = calls.call("debug_native_committed_step", &args);
                assert!(outcome.refusal);
                assert_eq!(outcome.message, message);
            }
            let anonymous = shard.call_anonymous(
                "debug_native_committed_step",
                &[
                    &run.to_string(),
                    &request.sequence.to_string(),
                    &request.at_micros.to_string(),
                ],
            );
            assert!(
                !anonymous.status.success(),
                "anonymous movement must be refused"
            );
            assert!(
                format!(
                    "{}{}",
                    String::from_utf8_lossy(&anonymous.stdout),
                    String::from_utf8_lossy(&anonymous.stderr)
                )
                .contains("operator only"),
                "anonymous movement must return the authorization Refusal"
            );
            for trap in [false, true] {
                let outcome = calls.call("debug_native_database_fault", &(trap,));
                assert!(!outcome.accepted);
                assert_eq!(outcome.refusal, !trap);
                if !trap {
                    assert_eq!(outcome.message, "native database rollback fixture");
                }
                let after = support::snapshot(shard, calls, run, 0, true);
                assert_eq!(before["frame"], after["frame"]);
            }
            calls
                .call(
                    "debug_native_committed_step",
                    &(run, request.sequence, request.at_micros),
                )
                .assert_accepted();
        }
        for (label, shard, calls, run) in [
            ("module", &*reference, &reference_calls, expected_run),
            ("native_database", &self.shard, &native_calls, native_run),
        ] {
            let before = support::snapshot(shard, calls, run, 1, true);
            let mut rss = vec![super::process_usage(shard.process_id()).rss_kib];
            let mut libraries = vec![native_libraries(shard.process_id())];
            for _ in 0..2 {
                for _ in 0..8 {
                    let outcome = calls.call("debug_native_database_fault", &(true,));
                    assert!(!outcome.accepted && !outcome.refusal);
                }
                assert_eq!(
                    before["frame"],
                    support::snapshot(shard, calls, run, 1, true)["frame"]
                );
                rss.push(super::process_usage(shard.process_id()).rss_kib);
                libraries.push(native_libraries(shard.process_id()));
            }
            self.artifacts[format!("{label}_rss_kib_after_0_8_16_traps")] = json!(rss);
            self.artifacts[format!("{label}_libraries_after_0_8_16_traps")] = json!(libraries);
            let expected = if label == "native_database" {
                loaded_libraries
            } else {
                0
            };
            assert_eq!(libraries, vec![expected; 3]);
            eprintln!("{label} resident KiB after 0, 8 and 16 traps: {rss:?}");
        }
        let logs = self.shard.module_logs(160);
        assert!(logs.status.success());
        assert!(String::from_utf8(logs.stdout)
            .unwrap()
            .contains("LYRACORE_NATIVE_DATABASE_FALLBACK"));
        let before = support::snapshot(&self.shard, &native_calls, native_run, 1, true);
        drop(native_calls);
        std::env::set_var("LYRACORE_NATIVE_DATABASE_MANIFEST", &self.manifest);
        self.shard.restart_persistent();
        std::env::remove_var("LYRACORE_NATIVE_DATABASE_MANIFEST");
        native_calls = support::Calls::new(&self.shard);
        assert_eq!(
            before["frame"],
            support::snapshot(&self.shard, &native_calls, native_run, 1, true)["frame"]
        );
        for request in &case.requests[1..] {
            reference_calls
                .call(
                    "debug_native_committed_step",
                    &(expected_run, request.sequence, request.at_micros),
                )
                .assert_accepted();
            native_calls
                .call(
                    "debug_native_committed_step",
                    &(native_run, request.sequence, request.at_micros),
                )
                .assert_accepted();
        }
        let final_sequence = case.requests.last().unwrap().sequence;
        assert_eq!(
            support::snapshot(
                reference,
                &reference_calls,
                expected_run,
                final_sequence,
                true
            )["frame"],
            support::snapshot(&self.shard, &native_calls, native_run, final_sequence, true)
                ["frame"],
        );
        eprintln!("Native database: authorization, stale input, Refusal, panic rollback and restart checks pass");
        drop(native_calls);
        drop(reference_calls);
        reference.restart_persistent();
        std::env::set_var("LYRACORE_NATIVE_DATABASE_MANIFEST", &self.manifest);
        self.shard.restart_persistent();
        std::env::remove_var("LYRACORE_NATIVE_DATABASE_MANIFEST");
    }
}

fn verify_movement_trace(shard: &Standalone, calls: &support::Calls) {
    use lyracore_module::native_prototype::{compare, SCENARIOS, TRACE_PREFIX};

    let mut expected: Vec<Value> =
        include_str!("../../prototypes/native-simulation/reference.jsonl")
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    for name in SCENARIOS {
        calls
            .call("debug_native_simulation_reference", &(name,))
            .assert_accepted();
    }
    let mut actual: Vec<Value> = Vec::new();
    assert!(lyracore_test_support::poll_until(
        Duration::from_secs(5),
        || {
            let logs = shard.module_logs(200);
            assert!(logs.status.success());
            actual = String::from_utf8(logs.stdout)
                .unwrap()
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .filter_map(|record| {
                    record["message"]
                        .as_str()?
                        .strip_prefix(TRACE_PREFIX)
                        .map(|frame| serde_json::from_str(frame).unwrap())
                })
                .collect();
            actual.len() >= expected.len()
        }
    ));
    let key = |frame: &Value| {
        (
            frame["scenario"].as_str().unwrap().to_owned(),
            frame["frame"]["sequence"].as_u64().unwrap(),
        )
    };
    expected.sort_by_key(&key);
    actual.sort_by_key(key);
    assert_eq!(expected.len(), 60);
    assert_eq!(expected.len(), actual.len());
    for (expected, actual) in expected.iter().zip(&actual) {
        compare(expected, actual).unwrap_or_else(|difference| panic!("{difference}"));
    }
}

fn native_libraries(pid: u32) -> usize {
    let mappings = std::fs::read_to_string(format!("/proc/{pid}/maps")).unwrap();
    mappings
        .lines()
        .filter_map(|line| line.split_whitespace().nth(5))
        .filter(|path| {
            path.rsplit('/')
                .next()
                .unwrap()
                .starts_with("lyracore-native-")
        })
        .collect::<std::collections::BTreeSet<_>>()
        .len()
}

impl Drop for Database {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.manifest);
    }
}
