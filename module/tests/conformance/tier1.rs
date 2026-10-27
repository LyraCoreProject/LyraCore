use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{ensure, Context, Result};
use lyracore_test_support::{core_root, Standalone};
use serde_json::json;
use wire_client::{catalogue, compare, runner};

use super::transport::OperatorTransport;

fn git(arguments: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(core_root())
        .output()?;
    ensure!(output.status.success(), "git {arguments:?} failed");
    Ok(String::from_utf8(output.stdout)?.trim().into())
}

fn write_result(
    path: &Path,
    golden: &wire_client::trace::GoldenTrace,
    comparison: &compare::Comparison,
    now: u64,
) -> Result<()> {
    let pin = include_str!("../../../.wire-harness-rev");
    let revision = pin
        .lines()
        .filter(|line| !line.starts_with('#'))
        .flat_map(str::split_whitespace)
        .nth(1)
        .context("harness SHA")?;
    let result = json!({
        "schema_version": 1,
        "tier": "tier1",
        "core_commit": git(&["rev-parse", "HEAD"] )?,
        "working_tree_dirty": !git(&["status", "--porcelain", "--untracked-files=normal"] )?.is_empty(),
        "harness_revision": revision,
        "recorded_at_unix_ms": now,
        "scenarios": [{
            "id": golden.scenario().id,
            "behavior": golden.scenario().behavior,
            "scenario_sha256": golden.trace().header.scenario_sha256,
            "comparison": comparison,
        }],
    });
    let temporary = path.with_extension("json.pending");
    std::fs::write(&temporary, serde_json::to_vec_pretty(&result)?)?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn connection_identity_matches_its_golden_trace() -> Result<()> {
    let path = core_root().join(".lyracore/conformance/tier1.json");
    std::fs::create_dir_all(path.parent().unwrap())?;
    match std::fs::remove_file(&path) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    let golden = catalogue::get("connection_identity")?;
    let mut shard = Standalone::start("tier1-connection-identity");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    let mut transport = OperatorTransport::prepare(&shard, golden.scenario())?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .try_into()?;
    let observations = runner::run_on(golden.scenario(), &mut transport)?;
    let comparison = compare::compare_observations(&golden, &observations, now, &[])?;
    write_result(&path, &golden, &comparison, now)?;
    println!("{}", serde_json::to_string_pretty(&comparison)?);
    assert!(
        comparison.is_match(),
        "connection_identity differs from its Golden Trace"
    );
    Ok(())
}
