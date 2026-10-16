use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use lyracore_module::native_prototype::commit::{GAMEPLAY_TABLES, SNAPSHOT_PREFIX, STATE_TABLE};
use lyracore_test_support::{poll_until, Standalone, TableSubscription};
use serde_json::Value;

#[path = "calls.rs"]
mod calls;
pub use calls::Calls;

pub fn start() -> Standalone {
    let artifact = std::env::var_os("LYRACORE_NATIVE_REFERENCE_WASM")
        .expect("set LYRACORE_NATIVE_REFERENCE_WASM to the prototype Module artifact");
    let mut shard = Standalone::start_persistent("native-committed");
    shard.publish_module_bytes(&std::fs::read(artifact).unwrap());
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("debug_clear_creatures", &["0"]);
    shard
}

pub fn setup(shard: &Standalone, calls: &Calls, scenario: &str, creatures: u32) -> u64 {
    calls
        .call(
            "debug_native_committed_setup",
            &(scenario.to_owned(), creatures),
        )
        .assert_accepted();
    let rows = shard.query_rows(&format!("SELECT run FROM {STATE_TABLE}"));
    rows[0]["run"].parse().unwrap()
}

pub fn subscribe(shard: &Standalone) -> TableSubscription {
    let queries: Vec<_> = GAMEPLAY_TABLES
        .iter()
        .chain([&STATE_TABLE])
        .map(|table| format!("SELECT * FROM {table}"))
        .collect();
    shard
        .subscribe(&queries.iter().map(String::as_str).collect::<Vec<_>>())
        .0
}

pub fn gameplay_update(mut update: Value) -> Value {
    let tables = update
        .as_object_mut()
        .expect("subscription update is a table map");
    tables.remove(STATE_TABLE);
    tables.retain(|name, table| {
        assert!(
            GAMEPLAY_TABLES.contains(&name.as_str()),
            "unexpected subscribed table: {name}"
        );
        for operation in ["inserts", "deletes"] {
            table[operation]
                .as_array_mut()
                .unwrap()
                .sort_by_cached_key(|row| row.to_string());
        }
        !table["inserts"].as_array().unwrap().is_empty()
            || !table["deletes"].as_array().unwrap().is_empty()
    });
    update
}

pub fn snapshot(shard: &Standalone, calls: &Calls, run: u64, sequence: u64, trace: bool) -> Value {
    static NEXT_NONCE: AtomicU64 = AtomicU64::new(1);
    let nonce = NEXT_NONCE.fetch_add(1, Ordering::Relaxed);
    calls
        .call("debug_native_committed_snapshot", &(trace, nonce))
        .assert_accepted();
    let mut found = None;
    assert!(
        poll_until(Duration::from_secs(5), || {
            let logs = shard.module_logs(80);
            assert!(logs.status.success());
            found = String::from_utf8(logs.stdout)
                .unwrap()
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .filter_map(|line| {
                    line["message"]
                        .as_str()?
                        .strip_prefix(SNAPSHOT_PREFIX)
                        .map(|message| serde_json::from_str::<Value>(message).unwrap())
                })
                .find(|frame| frame["nonce"] == nonce);
            found.is_some()
        }),
        "committed snapshot did not arrive"
    );
    let found = found.unwrap();
    assert_eq!(found["run"], run);
    assert_eq!(found["sequence"], sequence);
    found
}
