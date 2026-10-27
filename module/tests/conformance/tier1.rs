use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{ensure, Context, Result};
use lyracore_test_support::{core_root, Standalone};
use serde_json::json;
use wire_client::runner::Transport;
use wire_client::{catalogue, compare, runner};

use super::transport::OperatorTransport;

static REPORT: Mutex<()> = Mutex::new(());

fn git(arguments: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(core_root())
        .output()?;
    ensure!(output.status.success(), "git {arguments:?} failed");
    Ok(String::from_utf8(output.stdout)?.trim().into())
}

fn update_result(
    path: &Path,
    golden: &wire_client::trace::GoldenTrace,
    comparison: Option<&compare::Comparison>,
    now: u64,
) -> Result<()> {
    let _lock = REPORT.lock().unwrap();
    let pin = include_str!("../../../.wire-harness-rev");
    let revision = pin
        .lines()
        .filter(|line| !line.starts_with('#'))
        .flat_map(str::split_whitespace)
        .nth(1)
        .context("harness SHA")?;
    let mut result = json!({
        "schema_version": 1,
        "tier": "tier1",
        "core_commit": git(&["rev-parse", "HEAD"] )?,
        "working_tree_dirty": !git(&["status", "--porcelain", "--untracked-files=normal"] )?.is_empty(),
        "harness_revision": revision,
        "recorded_at_unix_ms": now,
        "scenarios": [],
    });
    match std::fs::read(path) {
        Ok(bytes) => {
            let old: serde_json::Value = serde_json::from_slice(&bytes)?;
            if [
                "schema_version",
                "tier",
                "core_commit",
                "working_tree_dirty",
                "harness_revision",
            ]
            .iter()
            .all(|field| old[field] == result[field])
            {
                result["scenarios"] = old["scenarios"].clone();
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    let scenarios = result["scenarios"]
        .as_array_mut()
        .context("result scenarios")?;
    scenarios.retain(|entry| entry["id"] != golden.scenario().id);
    if let Some(comparison) = comparison {
        scenarios.push(json!({
            "id": golden.scenario().id,
            "behavior": golden.scenario().behavior,
            "scenario_sha256": golden.trace().header.scenario_sha256,
            "comparison": comparison,
        }));
    }
    scenarios.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let temporary = path.with_extension("json.pending");
    std::fs::write(&temporary, serde_json::to_vec_pretty(&result)?)?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

fn run_scenario(id: &str, setup: impl FnOnce(&Standalone) -> Result<()>) -> Result<()> {
    let path = core_root().join(".lyracore/conformance/tier1.json");
    std::fs::create_dir_all(path.parent().unwrap())?;
    let golden = catalogue::get(id)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .try_into()?;
    update_result(&path, &golden, None, now)?;
    let mut shard = Standalone::start(&format!("tier1-{}", id.replace('_', "-")));
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    setup(&shard)?;
    let mut transport = OperatorTransport::prepare(&shard, golden.scenario())?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .try_into()?;
    let observations = runner::run_on(golden.scenario(), &mut transport)?;
    ensure!(
        observations
            .iter()
            .any(|observation| observation.checkpoint == "opening"),
        "world entry must remain in the opening"
    );
    let comparison = compare::compare_character_observations(
        &golden,
        &observations,
        transport.character_guid(),
        now,
        &[],
    )?;
    update_result(&path, &golden, Some(&comparison), now)?;
    let report = serde_json::to_string_pretty(&comparison)?;
    println!("{id}: {report}");
    assert!(
        comparison.is_match(),
        "{id} differs from its Golden Trace:\n{report}"
    );
    Ok(())
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn connection_identity_matches_its_golden_trace() -> Result<()> {
    run_scenario("connection_identity", |_| Ok(()))
}

#[test]
#[ignore = "taxi divergence: repeated splines, missing combat/mount/landing replies and unit flags; requires imported data"]
fn taxi_single_hop_matches_its_golden_trace() -> Result<()> {
    run_scenario("taxi_single_hop", |shard| {
        let path = std::env::var_os("LYRACORE_TEST_TAXI_SQL").context(
            "set LYRACORE_TEST_TAXI_SQL to the minimal import described in conformance/README.md",
        )?;
        for statement in std::fs::read_to_string(path)?.split(";\n") {
            if !statement.trim().is_empty() {
                shard.assert_sql(statement.trim());
            }
        }
        ensure!(
            shard
                .query_rows("SELECT id FROM game_taxi_path_node WHERE path_id = 6")
                .len()
                == 28,
            "route 6 requires all 28 imported vertices"
        );
        Ok(())
    })
}

#[test]
fn result_updates_preserve_other_scenarios_and_clear_failed_setup() -> Result<()> {
    let directory = std::env::temp_dir().join(format!(
        "conformance-result-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    std::fs::create_dir(&directory)?;
    let path = directory.join("tier1.json");
    let identity = catalogue::get("connection_identity")?;
    let taxi = catalogue::get("taxi_single_hop")?;
    let comparison = compare::compare(&identity, identity.trace(), &[])?;
    update_result(&path, &identity, Some(&comparison), 1)?;
    update_result(&path, &taxi, Some(&comparison), 2)?;
    update_result(&path, &taxi, None, 3)?;
    let result: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    assert_eq!(result["scenarios"].as_array().unwrap().len(), 1);
    assert_eq!(result["scenarios"][0]["id"], "connection_identity");
    assert_eq!(result["recorded_at_unix_ms"], 3);
    std::fs::remove_dir_all(directory)?;
    Ok(())
}

#[test]
fn golden_spline_coordinates_keep_their_wire_precision() -> Result<()> {
    use wire_client::trace::RecordedField;

    let golden = catalogue::get("taxi_single_hop")?;
    let spline = golden
        .trace()
        .packets
        .iter()
        .find(|packet| packet.checkpoint == "flight" && packet.opcode == 0x00DD)
        .context("Golden Trace passenger spline")?;
    let RecordedField::List(points) = &spline.fields["points"] else {
        anyhow::bail!("spline points must be a list");
    };
    for (index, axis, value) in [(10, "z", 52.476253509521484), (21, "y", 986.2713623046875)] {
        let RecordedField::Object(point) = &points[index] else {
            anyhow::bail!("spline point must be an object");
        };
        assert_eq!(point[axis], RecordedField::Float(value));
    }
    Ok(())
}
