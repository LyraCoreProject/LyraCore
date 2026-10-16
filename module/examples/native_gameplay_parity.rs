use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use lyracore_test_support::Standalone;
use serde_json::{json, Value};

#[path = "support/calls.rs"]
mod calls;
#[path = "support/metrics.rs"]
mod metrics;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let (reference_pair, creatures, samples, warmups) = match args.get(4).map(String::as_str) {
        None if args.len() == 4 => (false, 16, 1, 0),
        Some("--reference-pair") if args.len() == 5 => (true, 16, 1, 0),
        Some("--benchmark") if args.len() == 7 && args[2] == "creature_cycle" => {
            let creatures: u32 = args[5].parse()?;
            let samples: u32 = args[6].parse()?;
            if !(2..=5_000).contains(&creatures) || !(5..=100).contains(&samples) {
                return Err("benchmark requires 2..=5000 creatures and 5..=100 samples".into());
            }
            if cfg!(debug_assertions) {
                return Err("benchmarks require a release build".into());
            }
            (false, creatures, samples, 3)
        }
        _ => return Err("usage: native_gameplay_parity <wasm> <library> <scenario> <output.jsonl> [--reference-pair | --benchmark <creatures> <samples>]".into()),
    };
    assert!(std::env::var_os("LYRACORE_NATIVE_DATABASE_MANIFEST").is_none());
    let wasm = std::fs::read(&args[0])?;
    let library = std::fs::canonicalize(&args[1])?;
    let manifest =
        std::env::temp_dir().join(format!("native-gameplay-{}.json", std::process::id()));
    let mut manifest = Manifest::create(manifest, &library, &wasm)?;
    let mut output = File::create_new(&args[3])?;
    let ticks = Command::new("getconf").arg("CLK_TCK").output()?;
    assert!(ticks.status.success());
    writeln!(
        output,
        "{}",
        json!({
            "kind": "metadata", "commit": git(&["rev-parse", "HEAD"]),
            "dirty": !git(&["status", "--porcelain"]).is_empty(),
            "wasm_blake3": hash(&wasm), "native_blake3": hash(&std::fs::read(&library)?),
            "server_blake3": hash(&std::fs::read(std::env::var_os("LYRACORE_NATIVE_DATABASE_SERVER").unwrap())?),
            "driver_blake3": hash(&std::fs::read(std::env::current_exe()?)?),
            "scenario": args[2], "creatures": creatures, "samples": samples, "warmups": warmups,
            "candidate_implementation": if reference_pair { "module" } else { "native_database" },
            "clock": "fixed rule time; real transaction; synthetic caller context inside fixture",
            "comparison": "all application tables except game_operator, with complete rows and no numeric tolerance",
            "scope": "one complete gameplay step per durable transaction; BSATN encoding, binary WebSocket acknowledgment and confirmed CLI subscription output; excludes setup and parity comparisons",
            "storage": "disk", "cpu_ticks_per_second": String::from_utf8(ticks.stdout)?.trim().parse::<u64>()?,
            "cpu_scope": "runtime and driver during request and publication wait; excludes the separate CLI subscriber process",
            "subscription_bytes": "CLI JSON bytes after decoding; not WebSocket wire bytes",
            "affinity": std::fs::read_to_string("/proc/self/status")?.lines().find(|line| line.starts_with("Cpus_allowed_list:")).unwrap_or("unknown"),
        })
    )?;
    let mut reference = Standalone::start_persistent("native-gameplay-reference");
    reference.publish_module_bytes(&wasm);
    if !reference_pair {
        std::env::set_var("LYRACORE_NATIVE_DATABASE_MANIFEST", &manifest.path);
    }
    let mut native = Standalone::start_persistent("native-gameplay-native");
    std::env::remove_var("LYRACORE_NATIVE_DATABASE_MANIFEST");
    native.publish_module_bytes(&wasm);
    let (tables, schedules) = schema_tables(&reference);
    assert_eq!((tables.clone(), schedules.clone()), schema_tables(&native));
    for shard in [&reference, &native] {
        shard.assert_call("claim_operator", &[]);
    }
    let pair = GameplayPair {
        calls: [calls::Calls::new(&reference), calls::Calls::new(&native)],
        shards: [reference, native],
        tables,
        schedules,
        scenario: args[2].clone(),
        creatures,
        native: !reference_pair,
    };
    for sample in 0..samples + warmups {
        pair.replay(sample, sample < warmups, &mut output)?;
    }
    verify_execution(&pair.shards[0], false);
    verify_execution(&pair.shards[1], pair.native);
    writeln!(
        output,
        "{}",
        json!({"kind": "complete", "samples": samples, "warmups": warmups, "tables": pair.tables.len(), "native_libraries": pair.shards.each_ref().map(|shard| metrics::native_libraries(shard.process_id()))})
    )?;
    output.flush()?;
    manifest.remove()?;
    Ok(())
}

struct GameplayPair {
    calls: [calls::Calls; 2],
    shards: [Standalone; 2],
    tables: Vec<String>,
    schedules: Vec<String>,
    scenario: String,
    creatures: u32,
    native: bool,
}

impl GameplayPair {
    fn replay(&self, sample: u32, warmup: bool, output: &mut File) -> std::io::Result<()> {
        for (shard, calls) in self.shards.iter().zip(&self.calls) {
            shard.assert_sql("DELETE FROM game_native_gameplay_state");
            calls
                .call(
                    "debug_native_gameplay_setup",
                    &(
                        &self.scenario,
                        self.creatures,
                        &self.tables,
                        &self.schedules,
                    ),
                )
                .assert_accepted();
        }
        let queries: Vec<_> = self
            .tables
            .iter()
            .map(|name| format!("SELECT * FROM {name}"))
            .collect();
        let queries: Vec<_> = queries.iter().map(String::as_str).collect();
        let (reference_subscription, reference_initial) = self.shards[0].subscribe(&queries);
        let (native_subscription, native_initial) = self.shards[1].subscribe(&queries);
        compare(
            &reference_initial.value,
            &native_initial.value,
            &format!("sample {sample} initial"),
            output,
        )?;
        let subscriptions = [reference_subscription, native_subscription];
        let steps: u64 = if self.scenario == "periodic_deaths" {
            2
        } else {
            12
        };
        let order = if sample.is_multiple_of(2) {
            [0, 1]
        } else {
            [1, 0]
        };
        for sequence in 1..=steps {
            let mut updates = [Value::Null, Value::Null];
            for implementation in order {
                let shard = &self.shards[implementation];
                let before = metrics::process_usage(shard.process_id());
                let driver_before = metrics::process_usage(std::process::id());
                let start = Instant::now();
                let outcome =
                    self.calls[implementation].call("debug_native_gameplay_step", &(sequence,));
                let acknowledgment_ns = start.elapsed().as_nanos();
                outcome.assert_accepted();
                let update = subscriptions[implementation].receive(Duration::from_secs(30));
                let publication_ns = update.received_at.duration_since(start).as_nanos();
                let after = metrics::process_usage(shard.process_id());
                let driver_after = metrics::process_usage(std::process::id());
                assert_eq!(
                    update.value["game_native_gameplay_state"]["inserts"][0]["sequence"],
                    sequence
                );
                writeln!(
                    output,
                    "{}",
                    json!({
                        "kind": "step", "implementation": match implementation { 0 => "module", _ if self.native => "native_database", _ => "module_repeat" },
                        "sample": sample, "warmup": warmup, "native_first": order[0] == 1,
                        "sequence": sequence, "elapsed_ns": acknowledgment_ns.max(publication_ns),
                        "acknowledgment_ns": acknowledgment_ns, "publication_ns": publication_ns,
                        "request_bsatn_bytes": outcome.request_bytes, "request_encode_ns": outcome.encode_ns,
                        "round_trip_ns": outcome.round_trip_ns, "subscription_json_bytes": update.json_bytes,
                        "server_cpu_ticks": after.cpu_ticks - before.cpu_ticks,
                        "driver_cpu_ticks": driver_after.cpu_ticks - driver_before.cpu_ticks,
                        "server_rss_kib": after.rss_kib, "server_peak_rss_kib": after.peak_rss_kib,
                    })
                )?;
                updates[implementation] = update.value;
            }
            compare(
                &updates[0],
                &updates[1],
                &format!("sample {sample} step {sequence}"),
                output,
            )?;
        }
        drop(subscriptions);
        let (_, expected) = self.shards[0].subscribe(&queries);
        let (_, actual) = self.shards[1].subscribe(&queries);
        compare(
            &expected.value,
            &actual.value,
            &format!("sample {sample} final durable state"),
            output,
        )?;
        assert_effects(&actual.value, &self.scenario, self.creatures);
        output.flush()
    }
}

fn schema_tables(shard: &Standalone) -> (Vec<String>, Vec<String>) {
    let schema = shard.describe();
    let mut tables = Vec::new();
    let mut schedules = Vec::new();
    for table in schema["tables"].as_array().unwrap() {
        let name = table["name"].as_str().unwrap();
        assert!(
            name.starts_with("game_"),
            "unexpected application table {name}"
        );
        if name == "game_operator" {
            continue;
        }
        tables.push(name.to_owned());
        if table["schedule"].get("some").is_some() {
            schedules.push(name.to_owned());
        }
    }
    tables.sort();
    schedules.sort();
    assert!(tables.len() > 100 && schedules.len() > 10);
    (tables, schedules)
}

fn canonical(update: &Value) -> BTreeMap<&str, Value> {
    update
        .as_object()
        .unwrap()
        .iter()
        .filter_map(|(name, table)| {
            let mut table = table.clone();
            for operation in ["inserts", "deletes"] {
                table[operation]
                    .as_array_mut()
                    .unwrap()
                    .sort_by_cached_key(Value::to_string);
            }
            (!table["inserts"].as_array().unwrap().is_empty()
                || !table["deletes"].as_array().unwrap().is_empty())
            .then_some((name.as_str(), table))
        })
        .collect()
}

fn compare(
    reference: &Value,
    native: &Value,
    frame: &str,
    output: &mut File,
) -> std::io::Result<()> {
    let reference = canonical(reference);
    let native = canonical(native);
    if reference != native {
        let tables: std::collections::BTreeSet<_> =
            reference.keys().chain(native.keys()).copied().collect();
        let mismatched: Vec<_> = tables
            .into_iter()
            .filter(|name| reference.get(name) != native.get(name))
            .map(|name| {
                json!({
                    "table": name,
                    "module": reference.get(name),
                    "candidate": native.get(name),
                })
            })
            .collect();
        writeln!(
            output,
            "{}",
            json!({"kind": "mismatch", "frame": frame, "tables": mismatched})
        )?;
        output.flush()?;
        return Err(std::io::Error::other(format!(
            "{frame}: committed gameplay differs; see the output artifact"
        )));
    }
    writeln!(
        output,
        "{}",
        json!({"kind": "parity", "frame": frame,
        "digest": hash(&serde_json::to_vec(&reference)?), "nonempty_tables": reference.len()})
    )
}

fn assert_effects(snapshot: &Value, scenario: &str, creatures: u32) {
    let entities = snapshot["game_world_entity"]["inserts"].as_array().unwrap();
    assert_eq!(entities.len(), creatures as usize + 1);
    let state = &snapshot["game_native_gameplay_state"]["inserts"][0];
    if scenario == "periodic_deaths" {
        assert_eq!(
            entities.iter().filter(|row| row["dead"] == true).count(),
            16
        );
        assert!(entities
            .iter()
            .any(|row| row["money"].as_u64().unwrap() > 0));
        assert!(!snapshot["game_corpse_loot"]["inserts"]
            .as_array()
            .unwrap()
            .is_empty());
        let character = entities.iter().find(|row| row["guid"] == 1).unwrap();
        assert_eq!(character["level"], 2);
        assert_eq!(character["xp"], 400);
        let rewards = snapshot["game_xp_event"]["inserts"].as_array().unwrap();
        assert_eq!(rewards.len(), 16);
        assert!(rewards.iter().all(|reward| reward["total_exp"] == 50));
    } else {
        assert_eq!(state["awake"], creatures);
        assert!(state["phases"]
            .as_array()
            .unwrap()
            .iter()
            .any(|phase| phase == "eventai"));
        assert!(!snapshot["game_combat_event"]["inserts"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(entities
            .iter()
            .any(|row| row["health"].as_u64().unwrap() < row["max_health"].as_u64().unwrap()));
    }
}

fn verify_execution(shard: &Standalone, native: bool) {
    let path = lyracore_test_support::log_dir().join(format!("{}.log", shard.shard_name()));
    let logs = std::fs::read_to_string(path).unwrap();
    assert!(!logs.contains("LYRACORE_NATIVE_DATABASE_FALLBACK"));
    assert!(!logs.contains("LYRACORE_NATIVE_DATABASE_WASM"));
    for name in [
        "init",
        "debug_native_gameplay_setup",
        "debug_native_gameplay_step",
    ] {
        let marker = format!("LYRACORE_NATIVE_DATABASE_V1 reducer={name}");
        let found = logs.lines().any(|line| line.ends_with(&marker));
        assert_eq!(found, native, "native execution evidence for {name}");
    }
}

fn hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn git(args: &[&str]) -> String {
    let result = Command::new("git").args(args).output().unwrap();
    assert!(result.status.success());
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

struct Manifest {
    path: PathBuf,
}

impl Manifest {
    fn create(path: PathBuf, library: &Path, wasm: &[u8]) -> std::io::Result<Self> {
        serde_json::to_writer(
            File::create_new(&path)?,
            &json!({
                "library": library, "wasm_blake3": hash(wasm),
                "native_blake3": hash(&std::fs::read(library)?),
            }),
        )?;
        Ok(Self { path })
    }

    fn remove(&mut self) -> std::io::Result<()> {
        std::fs::remove_file(&self.path)
    }
}

impl Drop for Manifest {
    fn drop(&mut self) {
        let _ = self.remove();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence() -> (PathBuf, File) {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "native-gameplay-comparator-{}-{sequence}.jsonl",
            std::process::id()
        ));
        let file = File::create_new(&path).unwrap();
        (path, file)
    }

    #[test]
    fn committed_rows_ignore_order_but_preserve_every_field() {
        let reference = json!({"game_world_entity": {
            "inserts": [{"guid": 1, "money": 8, "x": 1.25}, {"guid": 2, "money": 9, "x": 2.5}],
            "deletes": []
        }});
        let mut reordered = reference.clone();
        reordered["game_world_entity"]["inserts"]
            .as_array_mut()
            .unwrap()
            .reverse();
        let (evidence, mut output) = evidence();
        compare(&reference, &reordered, "reordered rows", &mut output).unwrap();
        for (field, value) in [("money", json!(7)), ("x", json!(1.250001))] {
            let mut changed = reference.clone();
            changed["game_world_entity"]["inserts"][0][field] = value;
            assert!(compare(&reference, &changed, field, &mut output).is_err());
        }
        let mut missing = reference.clone();
        missing["game_world_entity"]["inserts"]
            .as_array_mut()
            .unwrap()
            .pop();
        assert!(compare(&reference, &missing, "missing row", &mut output).is_err());
        std::fs::remove_file(evidence).unwrap();
    }

    #[test]
    fn an_unexpected_table_records_both_sides_and_fails() {
        let reference = json!({});
        let native =
            json!({"game_corpse_loot": {"inserts": [{"id": 1, "count": 2}], "deletes": []}});
        let (evidence, mut output) = evidence();
        assert!(compare(&reference, &native, "death", &mut output).is_err());
        let record: Value = serde_json::from_reader(File::open(&evidence).unwrap()).unwrap();
        assert_eq!(record["kind"], "mismatch");
        assert_eq!(record["frame"], "death");
        assert_eq!(record["tables"][0]["table"], "game_corpse_loot");
        assert_eq!(record["tables"][0]["module"], Value::Null);
        assert_eq!(record["tables"][0]["candidate"], native["game_corpse_loot"]);
        std::fs::remove_file(evidence).unwrap();
    }
}
