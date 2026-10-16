use std::fs::File;
use std::io::Write;
use std::process::Command;
use std::time::{Duration, Instant};

use lyracore_module::native_prototype::benchmark::{digest, workload, SCENARIOS};
use lyracore_module::native_prototype::commit::STATE_TABLE;
use lyracore_module::native_prototype::TableSimulation;
use lyracore_test_support::Standalone;
use serde_json::{json, Value};

#[path = "support/database.rs"]
mod database;
#[path = "support/committed.rs"]
mod support;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if cfg!(debug_assertions) {
        return Err("benchmarks require a release build".into());
    }
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        return Err(
            "usage: native_committed_benchmark <wasm> <output.jsonl> [creatures ...]".into(),
        );
    }
    let counts = if args.len() == 2 {
        vec![100, 1_000, 5_000]
    } else {
        args[2..]
            .iter()
            .map(|count| count.parse())
            .collect::<Result<Vec<u32>, _>>()?
    };
    let samples: u32 = std::env::var("LYRACORE_BENCHMARK_SAMPLES")
        .unwrap_or_else(|_| "20".into())
        .parse()?;
    if !(5..=100).contains(&samples) {
        return Err("LYRACORE_BENCHMARK_SAMPLES must be in 5..=100".into());
    }
    for &count in &counts {
        workload(SCENARIOS[0], count)?;
    }
    std::env::set_var("LYRACORE_NATIVE_REFERENCE_WASM", &args[0]);
    let mut output = File::create_new(&args[1])?;
    let mut shard = support::start();
    let mut database = std::env::var_os("LYRACORE_NATIVE_DATABASE_LIBRARY")
        .map(|library| database::Database::start(&args[0], &library));
    let mut metadata = metadata(&args[0], &counts, samples)?;
    if let Some(database) = &mut database {
        metadata["native_implementation"] = json!("native_database");
        metadata["scope"] = json!("one tick per durable transaction in the existing database engine; BSATN request encoding, binary WebSocket acknowledgment and confirmed CLI subscription output included; setup and parity queries excluded");
        database.verify(&mut shard);
        metadata["database_artifacts"] = database.artifacts.clone();
    } else {
        metadata["native_implementation"] = json!("native_host");
    }
    writeln!(output, "{metadata}")?;
    let calls = support::Calls::new(&shard);
    let database_calls = database
        .as_ref()
        .map(|database| support::Calls::new(&database.shard));
    for &creatures in &counts {
        for &scenario in SCENARIOS {
            eprintln!("Committed comparison: {scenario}, {creatures} creatures");
            for sample in 0..samples + 3 {
                let order = if sample % 2 == 0 {
                    [false, true]
                } else {
                    [true, false]
                };
                let mut previous = None;
                for native in order {
                    let series = if native {
                        if let Some(database) = &database {
                            run_series(
                                &database.shard,
                                database_calls.as_ref().unwrap(),
                                scenario,
                                creatures,
                                false,
                            )
                        } else {
                            run_series(&shard, &calls, scenario, creatures, true)
                        }
                    } else {
                        run_series(&shard, &calls, scenario, creatures, false)
                    };
                    if let Some((updates, digest)) = &previous {
                        assert_eq!(&series.updates, updates, "{scenario}: subscription parity");
                        assert_eq!(&series.digest, digest, "{scenario}: committed state parity");
                    } else {
                        previous = Some((series.updates, series.digest.clone()));
                    }
                    for (tick, timing) in series.timings.into_iter().enumerate() {
                        writeln!(
                            output,
                            "{}",
                            json!({
                                "kind": "tick", "scenario": scenario, "creatures": creatures,
                                "sample": sample, "warmup": sample < 3, "sequence": tick + 1,
                                "implementation": if native { metadata["native_implementation"].as_str().unwrap() } else { "module" },
                                "native_first": order[0], "timing": timing,
                                "final_digest": series.digest,
                            })
                        )?;
                    }
                    output.flush()?;
                }
            }
        }
    }
    writeln!(output, "{}", json!({"kind": "complete"}))?;
    Ok(())
}

struct Series {
    updates: Vec<String>,
    timings: Vec<Value>,
    digest: Value,
}

fn run_series(
    shard: &Standalone,
    calls: &support::Calls,
    name: &str,
    creatures: u32,
    native: bool,
) -> Series {
    let case = workload(name, creatures).unwrap();
    let run = support::setup(shard, calls, name, creatures, native);
    let subscription = support::subscribe(shard);
    let mut simulation = native.then(|| TableSimulation::new(&case).unwrap());
    let mut updates = Vec::new();
    let mut timings = Vec::new();
    for request in &case.requests {
        let server_before = process_usage(shard.process_id());
        let driver_before = process_usage(std::process::id());
        let start = Instant::now();
        let mut native_work_ns = 0;
        let outcome = if let Some(simulation) = &mut simulation {
            simulation.apply(request).unwrap();
            let batch = simulation.commit_batch(run);
            native_work_ns = start.elapsed().as_nanos();
            calls.commit(&batch)
        } else {
            calls.call(
                "debug_native_committed_step",
                &(run, request.sequence, request.at_micros),
            )
        };
        let acknowledgment_ns = start.elapsed().as_nanos();
        outcome.assert_accepted();
        let update = subscription.receive(Duration::from_secs(30));
        let publication_ns = update.received_at.duration_since(start).as_nanos();
        let server_after = process_usage(shard.process_id());
        let driver_after = process_usage(std::process::id());
        let progress = &update.value[STATE_TABLE]["inserts"];
        assert_eq!(progress.as_array().unwrap().len(), 1);
        assert_eq!(progress[0]["sequence"], request.sequence);
        let gameplay = support::gameplay_update(update.value);
        let rows: usize = gameplay
            .as_object()
            .unwrap()
            .values()
            .map(|table| {
                table["inserts"].as_array().unwrap().len()
                    + table["deletes"].as_array().unwrap().len()
            })
            .sum();
        let subscription_digest = digest(&gameplay);
        timings.push(json!({
            "acknowledgment_ns": acknowledgment_ns,
            "publication_ns": publication_ns,
            "elapsed_ns": acknowledgment_ns.max(publication_ns),
            "request_bsatn_bytes": outcome.request_bytes,
            "subscription_json_bytes": update.json_bytes,
            "changed_rows": rows,
            "subscription_digest": subscription_digest,
            "native_work_ns": native_work_ns,
            "request_encode_ns": outcome.encode_ns,
            "round_trip_ns": outcome.round_trip_ns,
            "server_cpu_ticks": server_after.cpu_ticks - server_before.cpu_ticks,
            "driver_cpu_ticks": driver_after.cpu_ticks - driver_before.cpu_ticks,
            "server_rss_kib": server_after.rss_kib,
            "server_peak_rss_kib": server_after.peak_rss_kib,
        }));
        updates.push(subscription_digest);
    }
    let final_frame = support::snapshot(
        shard,
        calls,
        run,
        case.requests.last().unwrap().sequence,
        false,
    );
    if let Some(simulation) = &mut simulation {
        assert_eq!(
            final_frame["digest"],
            digest(&simulation.snapshot().unwrap())
        );
    }
    Series {
        updates,
        timings,
        digest: final_frame["digest"].clone(),
    }
}

struct ProcessUsage {
    cpu_ticks: u64,
    rss_kib: u64,
    peak_rss_kib: u64,
}

fn process_usage(pid: u32) -> ProcessUsage {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let fields: Vec<_> = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .collect();
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    let memory = |label| -> u64 {
        status
            .lines()
            .find_map(|line| line.strip_prefix(label))
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap()
    };
    ProcessUsage {
        // Linux stat fields 14 and 15 count user and system CPU, including all process threads.
        cpu_ticks: fields[11].parse::<u64>().unwrap() + fields[12].parse::<u64>().unwrap(),
        rss_kib: memory("VmRSS:"),
        peak_rss_kib: memory("VmHWM:"),
    }
}

fn metadata(wasm: &str, counts: &[u32], samples: u32) -> Result<Value, Box<dyn std::error::Error>> {
    let command = |program: &str, args: &[&str]| -> String {
        let result = Command::new(program).args(args).output().unwrap();
        assert!(result.status.success());
        String::from_utf8(result.stdout).unwrap().trim().to_owned()
    };
    let ticks: std::collections::BTreeMap<_, _> = SCENARIOS
        .iter()
        .map(|&name| (name, workload(name, 1).unwrap().requests.len()))
        .collect();
    Ok(json!({
        "kind": "metadata", "commit": command("git", &["rev-parse", "HEAD"]),
        "dirty": !command("git", &["status", "--porcelain"]).is_empty(),
        "rustc": command("rustc", &["--version"]), "profile": "release", "storage": "disk",
        "wasm_blake3": blake3::hash(&std::fs::read(wasm)?).to_hex().to_string(),
        "native_blake3": blake3::hash(&std::fs::read(std::env::current_exe()?)?).to_hex().to_string(),
        "counts": counts, "samples": samples, "warmups": 3, "scenarios": SCENARIOS,
        "ticks_per_replay": ticks,
        "scope": "one tick per durable transaction; native work, BSATN request encoding, binary WebSocket acknowledgment and confirmed CLI subscription output included; setup and parity queries excluded",
        "request_bytes": "BSATN client message, excluding websocket framing",
        "cpu_ticks_per_second": command("getconf", &["CLK_TCK"]).parse::<u64>()?,
        "cpu_scope": "server and benchmark driver during request and publication wait; excludes the separate CLI subscriber process and setup",
        "subscription_bytes": "CLI JSON bytes after decoding; not websocket wire bytes",
        "affinity": std::fs::read_to_string("/proc/self/status")?.lines().find(|line| line.starts_with("Cpus_allowed_list:")).unwrap_or("unknown"),
        "unix_seconds": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs(),
    }))
}
