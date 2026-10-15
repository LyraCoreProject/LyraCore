use std::fs::File;
use std::io::Write;
use std::time::{Duration, Instant};

use lyracore_module::native_prototype::benchmark::{
    digest, workload, RESULT_PREFIX, SCENARIOS, TIMER_PREFIX,
};
use lyracore_module::native_prototype::compare;
use lyracore_test_support::{poll_until, Standalone};
use serde_json::{json, Value};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if cfg!(debug_assertions) {
        return Err("benchmarks require a release build".into());
    }
    let args: Vec<_> = std::env::args().skip(1).collect();
    #[cfg(feature = "native_simulation_host")]
    if args.first().is_some_and(|arg| arg == "--host-only") {
        return profile_host(&args[1..]);
    }
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
    shard.assert_call("debug_clear_creatures", &["0"]);
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
                writeln!(output, "{}", measure(&shard, name, creatures, sample))?;
                output.flush()?;
            }
        }
    }
    writeln!(output, "{}", json!({"kind": "complete"}))?;
    Ok(())
}

#[cfg(feature = "native_simulation_host")]
fn profile_host(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.len() != 3 {
        return Err("usage: --host-only <scenario> <creatures> <samples>".into());
    }
    let creatures = args[1].parse()?;
    let samples: u32 = args[2].parse()?;
    if !(1..=100).contains(&samples) {
        return Err("profiling samples must be in 1..=100".into());
    }
    let case = workload(&args[0], creatures)?;
    let mut reference = case.simulation();
    for request in &case.requests {
        reference.apply(request)?;
    }
    let expected = digest(&reference.snapshot());
    for sample in 0..samples {
        let result = native_host_sample(&args[0], creatures);
        assert_eq!(
            result["digest"], expected,
            "native profiling parity mismatch"
        );
        println!("{}", json!({"sample": sample, "native_host": result}));
    }
    Ok(())
}

fn measure(shard: &Standalone, name: &str, creatures: u32, sample: u32) -> Value {
    let mut implementations = vec!["native", "module"];
    if cfg!(feature = "native_simulation_host") {
        implementations.push("native_host");
    }
    let count = implementations.len();
    implementations.rotate_left(sample as usize % count);
    let mut result = json!({"kind": "sample", "scenario": name,
        "creatures": creatures, "sample": sample, "warmup": sample < 3,
        "order": implementations});
    for implementation in &implementations {
        let value = match *implementation {
            "native" => native_sample(name, creatures),
            "module" => module_sample(shard, name, creatures, sample),
            #[cfg(feature = "native_simulation_host")]
            "native_host" => native_host_sample(name, creatures),
            _ => unreachable!("only compiled implementations are measured"),
        };
        result[*implementation] = value;
    }
    for implementation in implementations {
        assert_eq!(
            result[implementation]["digest"], result["module"]["digest"],
            "{implementation}: {name}, {creatures} creatures, sample {sample}"
        );
        assert_eq!(result[implementation]["ticks"], result["module"]["ticks"]);
    }
    result
}

fn check_scaled_parity(shard: &Standalone, name: &str) {
    shard.assert_call(
        "debug_native_simulation_benchmark",
        &[&format!("\"{name}\""), "4", "0", "true"],
    );
    let case = workload(name, 4).unwrap();
    let mut frames = Vec::new();
    assert!(
        poll_until(Duration::from_secs(5), || {
            frames = messages(shard)
                .iter()
                .filter_map(|message| message.strip_prefix(RESULT_PREFIX))
                .map(|message| serde_json::from_str::<Value>(message).unwrap())
                .filter(|result| result["scenario"] == name && result["creatures"] == 4)
                .filter_map(|mut result| result.as_object_mut().unwrap().remove("frame"))
                .collect::<Vec<_>>();
            frames.len() >= case.requests.len()
        }),
        "scaled parity frames did not arrive for {name}"
    );
    assert_eq!(
        frames.len(),
        case.requests.len(),
        "wrong scaled parity frame count for {name}"
    );
    let mut simulation = case.simulation();
    #[cfg(feature = "native_simulation_host")]
    let mut native_host = lyracore_module::native_prototype::TableSimulation::new(&case).unwrap();
    for (request, expected) in case.requests.iter().zip(&frames) {
        let actual = simulation.step(request).unwrap();
        compare(expected, &actual).unwrap_or_else(|difference| {
            panic!("{name}, command {}: {difference}", request.sequence)
        });
        #[cfg(feature = "native_simulation_host")]
        compare(expected, &native_host.step(request).unwrap()).unwrap_or_else(|difference| {
            panic!(
                "native host {name}, command {}: {difference}",
                request.sequence
            )
        });
    }
}

#[cfg(feature = "native_simulation_host")]
fn native_host_sample(name: &str, creatures: u32) -> Value {
    let case = workload(name, creatures).unwrap();
    let mut simulation = lyracore_module::native_prototype::TableSimulation::new(&case).unwrap();
    let started = Instant::now();
    simulation
        .apply_batch(std::hint::black_box(&case.requests))
        .unwrap();
    std::hint::black_box(&simulation);
    let elapsed_ns = started.elapsed().as_nanos();
    let snapshot = simulation.snapshot().unwrap();
    json!({"elapsed_ns": elapsed_ns as u64, "digest": digest(&snapshot), "ticks": case.requests.len()})
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
    let timer_name = format!("{TIMER_PREFIX}/{name}/{creatures}/{sample}");
    let mut result = None;
    let complete = poll_until(Duration::from_secs(5), || {
        let messages = messages(shard);
        let Some(timer) = messages
            .iter()
            .find(|message| message.contains(&timer_name))
        else {
            return false;
        };
        let Some(mut sample) = messages
            .iter()
            .rev()
            .filter_map(|message| message.strip_prefix(RESULT_PREFIX))
            .map(|message| serde_json::from_str::<Value>(message).unwrap())
            .find(|value| {
                value["scenario"] == name
                    && value["creatures"] == creatures
                    && value["sample"] == sample
            })
        else {
            return false;
        };
        sample["timer"] = json!(timer);
        sample["timer_name"] = json!(timer_name);
        result = Some(sample);
        true
    });
    assert!(
        complete,
        "Module timing and state did not arrive for {timer_name}"
    );
    result.unwrap()
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
        "native_host": cfg!(feature = "native_simulation_host"),
        "native_host_scope": "existing Module rules and table operations; memory transaction and change collection included; durable commit and publication excluded",
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
