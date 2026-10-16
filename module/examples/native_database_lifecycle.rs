use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

use lyracore_test_support::Standalone;
use serde_json::{json, Value};

#[path = "support/metrics.rs"]
mod metrics;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() < 3
        || args[3..]
            .iter()
            .any(|arg| !matches!(arg.as_str(), "--wasm" | "--versions-only"))
    {
        return Err(
            "usage: native_database_lifecycle <wasm> <library> <output.jsonl> [--wasm] [--versions-only]".into(),
        );
    }
    let wasm_only = args[3..].iter().any(|arg| arg == "--wasm");
    let updates = if args[3..].iter().any(|arg| arg == "--versions-only") {
        0
    } else {
        16
    };
    assert!(std::env::var_os("LYRACORE_NATIVE_DATABASE_MANIFEST").is_none());
    let wasm = std::fs::read(&args[0])?;
    let library = std::fs::canonicalize(&args[1])?;
    let manifest_path =
        std::env::temp_dir().join(format!("native-lifecycle-{}.json", std::process::id()));
    let manifest = Manifest(manifest_path);
    let artifacts = json!({
        "library": library, "native_blake3": hash(&std::fs::read(&library)?),
        "wasm_blake3": hash(&wasm), "execution": "reducers",
    });
    serde_json::to_writer(File::create_new(&manifest.0)?, &artifacts)?;
    if !wasm_only {
        std::env::set_var("LYRACORE_NATIVE_DATABASE_MANIFEST", &manifest.0);
    }
    let mut shard = Standalone::start_persistent("native-module-updates");
    shard.publish_module_bytes(&wasm);
    shard.assert_call("claim_operator", &[]);
    let schema = shard.describe();
    let mut queries = Vec::new();
    for table in schema["tables"].as_array().unwrap() {
        let name = table["name"].as_str().unwrap();
        if table["schedule"].get("some").is_some() {
            shard.assert_sql(&format!("DELETE FROM {name}"));
        }
        queries.push(format!("SELECT * FROM {name}"));
    }
    shard.assert_sql("UPDATE game_character SET money = 12345 WHERE guid = 1");
    shard.assert_call("set_realm_address", &["\"example.invalid:8085\""]);
    let queries: Vec<_> = queries.iter().map(String::as_str).collect();
    let initial = snapshot(&shard, &queries);
    let mut output = File::create_new(&args[2])?;
    writeln!(
        output,
        "{}",
        json!({
            "kind": "metadata", "implementation": if wasm_only { "module" } else { "native_database" },
            "commit": git(&["rev-parse", "HEAD"]), "dirty": !git(&["status", "--porcelain"]).is_empty(),
            "wasm_blake3": artifacts["wasm_blake3"], "module_native_blake3": artifacts["native_blake3"],
            "server_blake3": hash(&std::fs::read(std::env::var_os("LYRACORE_NATIVE_DATABASE_SERVER").unwrap())?),
            "scope": "repeated publication of the same Module, preserving application rows",
        })
    )?;
    let before = usage(shard.process_id());
    assert_eq!(before["libraries"] == 0, wasm_only);
    writeln!(
        output,
        "{}",
        json!({"kind": "usage", "updates": 0, "usage": before})
    )?;
    for generation in 1..=updates {
        shard.publish_module_bytes(&wasm);
        shard.assert_call("set_realm_address", &["\"example.invalid:8085\""]);
        let character = shard.query_rows("SELECT money FROM game_character WHERE guid = 1");
        assert_eq!(character[0]["money"], "12345");
        writeln!(
            output,
            "{}",
            json!({
                "kind": "usage", "updates": generation, "usage": usage(shard.process_id()),
            })
        )?;
        output.flush()?;
    }
    assert_eq!(
        initial,
        snapshot(&shard, &queries),
        "Module updates changed application rows"
    );
    let runtime_log = lyracore_test_support::log_dir().join(format!("{}.log", shard.shard_name()));
    let native_calls = std::fs::read_to_string(runtime_log)?
        .lines()
        .filter(|line| line.contains("LYRACORE_NATIVE_DATABASE_V1 reducer=set_realm_address"))
        .count();
    assert_eq!(native_calls, if wasm_only { 0 } else { updates + 1 });
    let after = usage(shard.process_id());
    let bounded = after["libraries"].as_u64().unwrap() <= before["libraries"].as_u64().unwrap() + 2;
    writeln!(
        output,
        "{}",
        json!({
            "kind": "result", "application_state_preserved": true,
            "native_calls": native_calls, "bounded_library_count": bounded,
        })
    )?;
    if !bounded {
        return Err("Module updates retain native libraries; see the usage records".into());
    }
    if !wasm_only {
        verify_artifacts(
            &mut shard,
            &wasm,
            &manifest,
            &artifacts,
            &queries,
            &mut output,
        )?;
    }
    verify_program_versions(
        &mut shard,
        &wasm,
        &queries,
        &initial,
        wasm_only,
        &mut output,
    )?;
    Ok(())
}

fn verify_artifacts(
    shard: &mut Standalone,
    wasm: &[u8],
    manifest: &Manifest,
    artifacts: &Value,
    queries: &[&str],
    output: &mut File,
) -> std::io::Result<()> {
    let initial = snapshot(shard, queries);
    let mut wrong_hash = artifacts.clone();
    wrong_hash["native_blake3"] = json!("0".repeat(64));
    serde_json::to_writer(File::create(&manifest.0)?, &wrong_hash)?;
    let error = shard.try_publish_module_bytes(wasm).unwrap_err();
    assert!(error.contains("native artifact hash mismatch"), "{error}");
    assert_eq!(snapshot(shard, queries), initial);

    let other_schema = renamed_fixture_table(wasm);
    let mut wrong_schema = artifacts.clone();
    wrong_schema["wasm_blake3"] = json!(hash(&other_schema));
    serde_json::to_writer(File::create(&manifest.0)?, &wrong_schema)?;
    let error = shard.try_publish_module_bytes(&other_schema).unwrap_err();
    assert!(
        error.contains("native and Wasm Module descriptions differ"),
        "{error}"
    );
    assert_eq!(snapshot(shard, queries), initial);

    serde_json::to_writer(File::create(&manifest.0)?, artifacts)?;
    shard.assert_call("set_realm_address", &["\"example.invalid:8085\""]);
    assert_eq!(snapshot(shard, queries), initial);
    writeln!(
        output,
        "{}",
        json!({
            "kind": "artifacts", "wrong_native_hash_refused": true,
            "wrong_module_description_refused": true, "application_state_preserved": true,
            "mismatched_schema_wasm_blake3": hash(&other_schema),
        })
    )
}

fn renamed_fixture_table(wasm: &[u8]) -> Vec<u8> {
    fn length(wasm: &[u8], offset: &mut usize) -> usize {
        let mut value = 0;
        for shift in (0..35).step_by(7) {
            let byte = wasm[*offset];
            *offset += 1;
            value |= usize::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return value;
            }
        }
        panic!("invalid Wasm section length");
    }

    assert!(wasm.starts_with(b"\0asm\x01\0\0\0"));
    let mut changed = wasm.to_vec();
    let mut offset = 8;
    let mut replacements = 0;
    while offset < wasm.len() {
        let section = wasm[offset];
        offset += 1;
        let size = length(wasm, &mut offset);
        let end = offset + size;
        // Section 11 holds data; leave registration export names and instructions intact.
        if section == 11 {
            let name = b"game_native_gameplay_state";
            for start in offset..end.saturating_sub(name.len()) + 1 {
                if &wasm[start..start + name.len()] == name {
                    changed[start + name.len() - 1] = b'x';
                    replacements += 1;
                }
            }
        }
        offset = end;
    }
    assert!(replacements > 0);
    changed
}

fn verify_program_versions(
    shard: &mut Standalone,
    wasm: &[u8],
    queries: &[&str],
    initial: &Value,
    wasm_only: bool,
    output: &mut File,
) -> std::io::Result<()> {
    let mut other_version = wasm.to_vec();
    let name = b"lyracore-native-version-test";
    // A custom section changes the artifact identity without changing its schema or instructions.
    other_version.extend([0, (name.len() + 1) as u8, name.len() as u8]);
    other_version.extend(name);
    assert_ne!(hash(wasm), hash(&other_version));
    let native_before = native_address_calls(shard, shard.shard_name());
    shard.publish_module_bytes(&other_version);
    shard.assert_call("set_realm_address", &["\"example.invalid:8085\""]);
    assert_eq!(
        native_address_calls(shard, shard.shard_name()),
        native_before
    );
    assert_eq!(&snapshot(shard, queries), initial);

    let sibling = format!("native-lifecycle-sibling-{}", std::process::id());
    shard.publish_named_module_bytes(&sibling, wasm);
    shard.assert_call_database(&sibling, "claim_operator", &[]);
    shard.assert_sql_database(
        &sibling,
        "UPDATE game_character SET money = 67890 WHERE guid = 1",
    );
    shard.restart_persistent();
    shard.assert_call("set_realm_address", &["\"example.invalid:8085\""]);
    assert_eq!(
        native_address_calls(shard, shard.shard_name()),
        native_before
    );
    assert_eq!(&snapshot(shard, queries), initial);
    assert_eq!(
        shard.query_database_rows(&sibling, "SELECT money FROM game_character WHERE guid = 1")[0]
            ["money"],
        "67890"
    );
    shard.assert_call_database(&sibling, "set_realm_address", &["\"example.invalid:8085\""]);
    let sibling_native_calls = native_address_calls(shard, &sibling);
    assert_eq!(sibling_native_calls, usize::from(!wasm_only));

    shard.publish_module_bytes(wasm);
    shard.assert_call("set_realm_address", &["\"example.invalid:8085\""]);
    assert_eq!(
        native_address_calls(shard, shard.shard_name()),
        native_before + usize::from(!wasm_only)
    );
    assert_eq!(&snapshot(shard, queries), initial);
    writeln!(
        output,
        "{}",
        json!({
            "kind": "versions", "unmatched_program_blake3": hash(&other_version),
            "wasm_update_preserved_state": true, "mixed_program_restart": true,
            "matching_program_restored": true, "matching_shard_native_calls": sibling_native_calls,
        })
    )
}

fn native_address_calls(shard: &Standalone, name: &str) -> usize {
    let logs = shard.module_logs_database(name, 10_000);
    assert!(logs.status.success());
    let logs = String::from_utf8(logs.stdout).unwrap();
    assert!(!logs.contains("LYRACORE_NATIVE_DATABASE_FALLBACK"));
    logs.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|line| {
            line["message"] == "LYRACORE_NATIVE_DATABASE_V1"
                && line["function"] == "set_realm_address"
        })
        .count()
}

fn snapshot(shard: &Standalone, queries: &[&str]) -> Value {
    let (_, initial) = shard.subscribe(queries);
    let mut frame = initial.value;
    for table in frame.as_object_mut().unwrap().values_mut() {
        for operation in ["inserts", "deletes"] {
            table[operation]
                .as_array_mut()
                .unwrap()
                .sort_by_cached_key(Value::to_string);
        }
    }
    frame
}

fn usage(pid: u32) -> Value {
    let usage = metrics::process_usage(pid);
    json!({
        "rss_kib": usage.rss_kib, "peak_rss_kib": usage.peak_rss_kib,
        "cpu_ticks": usage.cpu_ticks, "libraries": metrics::native_libraries(pid),
    })
}

fn hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn git(args: &[&str]) -> String {
    let result = std::process::Command::new("git")
        .args(args)
        .output()
        .unwrap();
    assert!(result.status.success());
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

struct Manifest(PathBuf);

impl Drop for Manifest {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
