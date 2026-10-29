#[path = "../support/ratchet.rs"]
mod ratchet;

use std::path::Path;
use std::{fs::OpenOptions, io::Write};

use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use wire_client::{catalogue, compare::Comparison, trace::Sha256Digest};

#[derive(Deserialize)]
struct Tier1Result {
    schema_version: u32,
    tier: String,
    core_commit: String,
    harness_revision: String,
    scenarios: Vec<ScenarioResult>,
}

#[derive(Deserialize)]
struct ScenarioResult {
    id: String,
    behavior: String,
    scenario_sha256: Sha256Digest,
    comparison: Comparison,
}

fn baseline_from_result(
    result: Tier1Result,
    current_commit: &str,
    requested: Option<&str>,
) -> Result<ratchet::Baseline> {
    ensure!(
        result.schema_version == 2 && result.tier == "tier1",
        "expected a schema 2 tier 1 result"
    );
    ensure!(
        result.core_commit == current_commit,
        "tier 1 result is from another commit"
    );
    let pin = include_str!("../../.wire-harness-rev")
        .split_whitespace()
        .last()
        .context("harness pin")?;
    ensure!(
        result.harness_revision == pin,
        "tier 1 result uses another harness revision"
    );
    let mut scenarios = Vec::new();
    for entry in result.scenarios {
        let golden = catalogue::get(&entry.id)?;
        ensure!(
            entry.behavior == golden.scenario().behavior,
            "{} names another Behavior",
            entry.id
        );
        ensure!(
            entry.scenario_sha256 == golden.trace().header.scenario_sha256,
            "{} uses another scenario revision",
            entry.id
        );
        scenarios.push(ratchet::ScenarioBaseline::from_comparison(
            &entry.id,
            &entry.behavior,
            &result.harness_revision,
            &entry.scenario_sha256,
            &entry.comparison,
        ));
    }
    scenarios.sort_by(|left, right| left.scenario.cmp(&right.scenario));
    let mut expected: Vec<_> = match requested {
        Some(id) => {
            catalogue::get(id)?;
            vec![id]
        }
        None => catalogue::ENTRIES.iter().map(|entry| entry.id).collect(),
    };
    expected.sort_unstable();
    let actual: Vec<_> = scenarios
        .iter()
        .map(|entry| entry.scenario.as_str())
        .collect();
    ensure!(
        actual == expected,
        "tier 1 result must contain the requested scenarios exactly once: {actual:?}"
    );
    Ok(ratchet::Baseline {
        schema_version: 1,
        scenarios,
    })
}

fn replace_scenario(
    mut committed: ratchet::Baseline,
    current: ratchet::ScenarioBaseline,
) -> Result<ratchet::Baseline> {
    let mut expected: Vec<_> = catalogue::ENTRIES.iter().map(|entry| entry.id).collect();
    expected.sort_unstable();
    let actual: Vec<_> = committed
        .scenarios
        .iter()
        .map(|entry| entry.scenario.as_str())
        .collect();
    ensure!(
        actual == expected,
        "committed tier 1 baseline is incomplete"
    );
    let entry = committed
        .scenarios
        .iter_mut()
        .find(|entry| entry.scenario == current.scenario)
        .context("scenario is missing from the committed tier 1 baseline")?;
    ensure!(
        entry.behavior == current.behavior,
        "Behavior changed for {}",
        current.scenario
    );
    *entry = current;
    Ok(committed)
}

fn write_baseline(path: &Path, current: ratchet::Baseline, requested: Option<&str>) -> Result<()> {
    let lock_path = path.with_extension("json.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("open {}", lock_path.display()))?;
    lock.lock()?;
    let baseline = match requested {
        Some(_) => replace_scenario(ratchet::Baseline::read(path)?, current.scenarios[0].clone())?,
        None => current,
    };
    let parent = path.parent().context("baseline parent directory")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&baseline.canonical_bytes()?)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path)?;
    #[cfg(unix)]
    std::fs::File::open(parent)?.sync_all()?;
    let saved = ratchet::Baseline::read(path)?;
    for scenario in &baseline.scenarios {
        saved.check(scenario)?;
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(
        args.first().map(String::as_str) == Some("regenerate")
            && (args.len() == 1 || args.len() == 3),
        "usage: baseline regenerate [scenario result-path]"
    );
    let requested = args.get(1).map(String::as_str);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("core root")?;
    let result_path = args
        .get(2)
        .map(Path::new)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| root.join(".lyracore/conformance/tier1.json"));
    let result: Tier1Result = serde_json::from_slice(&std::fs::read(result_path)?)?;
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()?;
    ensure!(output.status.success(), "git rev-parse HEAD failed");
    let commit = String::from_utf8(output.stdout)?;
    let current = baseline_from_result(result, commit.trim(), requested)?;
    let path = root.join("conformance/tier1-baseline.json");
    write_baseline(&path, current, requested)?;
    println!("wrote {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Arc, Barrier};
    use std::time::Duration;

    fn entry(id: &str) -> ratchet::ScenarioBaseline {
        ratchet::ScenarioBaseline::from_comparison(
            id,
            id,
            "revision",
            &Sha256Digest::of(id.as_bytes()),
            &Comparison {
                differences: vec![],
            },
        )
    }

    #[test]
    fn replacing_one_scenario_keeps_other_entry_bytes() -> Result<()> {
        let mut scenarios: Vec<_> = catalogue::ENTRIES
            .iter()
            .map(|item| entry(item.id))
            .collect();
        scenarios.sort_by(|left, right| left.scenario.cmp(&right.scenario));
        let original = ratchet::Baseline {
            schema_version: 1,
            scenarios,
        };
        let before = original.canonical_bytes()?;
        let mut changed = entry("taxi_single_hop");
        changed.harness_revision = "new revision".into();
        let after = replace_scenario(original, changed)?.canonical_bytes()?;
        fn outside(bytes: &[u8]) -> Result<(&[u8], &[u8])> {
            let marker = b"    {\n      \"scenario\": \"taxi_single_hop\"";
            let start = bytes
                .windows(marker.len())
                .position(|window| window == marker)
                .context("taxi entry start")?;
            let end_marker = b"\n    }";
            let end = bytes[start..]
                .windows(end_marker.len())
                .position(|window| window == end_marker)
                .context("taxi entry end")?
                + start
                + end_marker.len();
            Ok((&bytes[..start], &bytes[end..]))
        }
        assert_eq!(outside(&before)?, outside(&after)?);
        assert_ne!(before, after);
        Ok(())
    }

    #[test]
    fn incomplete_result_cannot_replace_scenario() {
        let result = Tier1Result {
            schema_version: 2,
            tier: "tier1".into(),
            core_commit: "commit".into(),
            harness_revision: include_str!("../../.wire-harness-rev")
                .split_whitespace()
                .last()
                .unwrap()
                .into(),
            scenarios: vec![],
        };
        assert!(baseline_from_result(result, "commit", Some("taxi_single_hop")).is_err());
    }

    #[test]
    fn concurrent_scenario_updates_preserve_both_entries() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("tier1-baseline.json");
        let mut scenarios: Vec<_> = catalogue::ENTRIES
            .iter()
            .map(|item| entry(item.id))
            .collect();
        scenarios.sort_by(|left, right| left.scenario.cmp(&right.scenario));
        let original = ratchet::Baseline {
            schema_version: 1,
            scenarios,
        };
        std::fs::write(&path, original.canonical_bytes()?)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path.with_extension("json.lock"))?;
        lock.lock()?;

        let barrier = Arc::new(Barrier::new(3));
        let (done, completed) = mpsc::channel();
        let threads: Vec<_> = ["connection_identity", "taxi_single_hop"]
            .into_iter()
            .map(|id| {
                let path = path.clone();
                let barrier = Arc::clone(&barrier);
                let done = done.clone();
                std::thread::spawn(move || {
                    let mut changed = entry(id);
                    changed.harness_revision = format!("changed {id}");
                    barrier.wait();
                    let result = write_baseline(
                        &path,
                        ratchet::Baseline {
                            schema_version: 1,
                            scenarios: vec![changed],
                        },
                        Some(id),
                    );
                    done.send(result).unwrap();
                })
            })
            .collect();
        barrier.wait();
        let wrote_without_lock = completed.recv_timeout(Duration::from_secs(1)).is_ok();
        drop(lock);
        for thread in threads {
            thread.join().unwrap();
        }
        drop(done);
        assert!(!wrote_without_lock, "baseline update skipped the lock");
        for result in completed {
            result?;
        }
        let saved = ratchet::Baseline::read(&path)?;
        for id in ["connection_identity", "taxi_single_hop"] {
            assert_eq!(
                saved
                    .scenarios
                    .iter()
                    .find(|entry| entry.scenario == id)
                    .unwrap()
                    .harness_revision,
                format!("changed {id}")
            );
        }
        Ok(())
    }
}
