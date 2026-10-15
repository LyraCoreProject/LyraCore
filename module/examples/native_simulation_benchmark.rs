use std::fs::File;
use std::io::Write;
use std::time::Instant;

use lyracore_module::native_prototype::benchmark::{
    digest, workload, RESULT_PREFIX, SCENARIOS, TIMER_PREFIX,
};
use lyracore_module::native_prototype::compare;
use lyracore_test_support::Standalone;
use serde_json::{json, Value};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if cfg!(debug_assertions) {
        return Err("benchmarks require a release build".into());
    }
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        return Err(
            "usage: native_simulation_benchmark <wasm> <output.jsonl> [creatures ...]".into(),
        );
    }
    let counts: Vec<u32> = if args.len() == 2 {
        vec![100, 1_000, 5_000]
    } else {
        args[2..]
            .iter()
            .map(|n| n.parse())
            .collect::<Result<_, _>>()?
    };
    let samples: u32 = std::env::var("LYRACORE_BENCHMARK_SAMPLES")
        .unwrap_or_else(|_| "20".into())
        .parse()?;
    if !(5..=100).contains(&samples) {
        return Err("LYRACORE_BENCHMARK_SAMPLES must be in 5..=100".into());
    }
    for &creatures in &counts {
        workload(SCENARIOS[0], creatures)?;
    }
    let wasm = std::fs::read(&args[0])?;
    let mut output = File::create_new(&args[1])?;
    writeln!(output, "{}", metadata(&wasm, samples, &counts)?)?;
    let mut shard = Standalone::start("native-simulation-benchmark");
    shard.publish_module_bytes(&wasm);
    shard.assert_call("claim_operator", &[]);
    for name in SCENARIOS {
        check_scaled_parity(&shard, name);
    }
    writeln!(
        output,
        "{}",
        json!({"kind": "parity", "creatures": 4, "scenarios": SCENARIOS})
    )?;
    for &creatures in &counts {
        for name in SCENARIOS {
            eprintln!("Benchmarking {name}, {creatures} creatures");
            for sample in 0..samples + 3 {
                let (native, module) = if sample % 2 == 0 {
                    let native = native_sample(name, creatures);
                    (native, module_sample(&shard, name, creatures, sample))
                } else {
                    let module = module_sample(&shard, name, creatures, sample);
                    (native_sample(name, creatures), module)
                };
                assert_eq!(
                    native["digest"], module["digest"],
                    "{name}, {creatures} creatures, sample {sample}"
                );
                assert_eq!(native["ticks"], module["ticks"]);
                writeln!(
                    output,
                    "{}",
                    json!({"kind": "sample", "scenario": name,
                    "creatures": creatures, "sample": sample, "warmup": sample < 3,
                    "native": native, "module": module})
                )?;
                output.flush()?;
            }
        }
    }
    writeln!(output, "{}", json!({"kind": "complete"}))?;
    Ok(())
}

fn check_scaled_parity(shard: &Standalone, name: &str) {
    shard.assert_call(
        "debug_native_simulation_benchmark",
        &[&format!("\"{name}\""), "4", "0", "true"],
    );
    let frames: Vec<Value> = messages(shard)
        .iter()
        .filter_map(|message| message.strip_prefix(RESULT_PREFIX))
        .map(|message| serde_json::from_str::<Value>(message).unwrap())
        .filter(|result| result["scenario"] == name && result["creatures"] == 4)
        .filter_map(|mut result| result.as_object_mut().unwrap().remove("frame"))
        .collect();
    let case = workload(name, 4).unwrap();
    assert_eq!(
        frames.len(),
        case.requests.len(),
        "wrong scaled parity frame count for {name}"
    );
    let mut simulation = case.simulation();
    for (request, expected) in case.requests.iter().zip(&frames) {
        let actual = simulation.step(request).unwrap();
        compare(expected, &actual).unwrap_or_else(|difference| {
            panic!("{name}, command {}: {difference}", request.sequence)
        });
    }
}

fn native_sample(name: &str, creatures: u32) -> Value {
    let case = workload(name, creatures).unwrap();
    let mut simulation = case.simulation();
    let started = Instant::now();
    for request in std::hint::black_box(&case.requests) {
        simulation.apply(request).unwrap();
    }
    std::hint::black_box(&simulation);
    let elapsed_ns = started.elapsed().as_nanos();
    let snapshot = simulation.snapshot();
    json!({"elapsed_ns": elapsed_ns as u64, "digest": digest(&snapshot), "ticks": case.requests.len()})
}

fn module_sample(shard: &Standalone, name: &str, creatures: u32, sample: u32) -> Value {
    shard.assert_call(
        "debug_native_simulation_benchmark",
        &[
            &format!("\"{name}\""),
            &creatures.to_string(),
            &sample.to_string(),
            "false",
        ],
    );
    let messages = messages(shard);
    let timer_name = format!("{TIMER_PREFIX}/{name}/{creatures}/{sample}");
    let timer = messages
        .iter()
        .find(|message| message.contains(&timer_name))
        .expect("Module stopwatch output is missing");
    let mut result = messages
        .iter()
        .rev()
        .filter_map(|message| message.strip_prefix(RESULT_PREFIX))
        .map(|message| serde_json::from_str::<Value>(message).unwrap())
        .find(|value| {
            value["scenario"] == name
                && value["creatures"] == creatures
                && value["sample"] == sample
        })
        .expect("Module benchmark result is missing");
    result["timer"] = json!(timer);
    result["timer_name"] = json!(timer_name);
    result
}

fn messages(shard: &Standalone) -> Vec<String> {
    let logs = shard.module_logs(100);
    assert!(logs.status.success(), "cannot read Module logs");
    String::from_utf8(logs.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|line| line["message"].as_str().map(str::to_owned))
        .collect()
}

fn metadata(
    wasm: &[u8],
    samples: u32,
    counts: &[u32],
) -> Result<Value, Box<dyn std::error::Error>> {
    let cpu = std::fs::read_to_string("/proc/cpuinfo")?;
    let status = std::fs::read_to_string("/proc/self/status")?;
    let executable = std::fs::read(std::env::current_exe()?)?;
    let spacetime = command(
        &std::env::var("SPACETIME_BIN").unwrap_or_else(|_| "spacetime".into()),
        &["--version"],
    );
    let version = spacetime
        .lines()
        .find(|line| line.starts_with("spacetimedb tool version"))
        .ok_or("cannot identify the SpacetimeDB version")?;
    Ok(
        json!({"kind": "metadata", "samples": samples, "warmups": 3, "counts": counts,
        "scenarios": SCENARIOS, "storage": "memory", "profile": "release",
        "scope": "seven movement ticks in one transaction; setup, snapshots, cleanup and commit excluded",
        "cpu": cpu.lines().find(|line| line.starts_with("model name")),
        "affinity": status.lines().find(|line| line.starts_with("Cpus_allowed_list:")),
        "architecture": std::env::consts::ARCH,
        "commit": command("git", &["rev-parse", "HEAD"]),
        "dirty": !command("git", &["status", "--porcelain"]).is_empty(),
        "rustc": command("rustc", &["--version"]),
        "spacetime": version,
        "unix_seconds": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs(),
        "wasm_blake3": blake3::hash(wasm).to_hex().to_string(),
        "native_blake3": blake3::hash(&executable).to_hex().to_string()}),
    )
}

fn command(program: &str, args: &[&str]) -> String {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{program} failed");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}
