//! `lyracore-delta-check <file>...` over a MIXED set of artifacts.
//!
//! A Package ships every artifact kind it has into one generated directory, so the set
//! `lyracore packages build` hands this binary holds Package Deltas and Script Artifacts together.
//! Each file is routed on its own `kind` member, each kind is traced with its own tracer, and the
//! exit status covers both.
//!
//! These run the real binary: the routing lives in its argument handling, not in the library.

use std::process::{Command, Output};

const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn run(paths: &[&std::path::Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lyracore-delta-check"))
        .args(paths)
        .output()
        .expect("spawn lyracore-delta-check")
}

fn write(dir: &std::path::Path, name: &str, json: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, json).expect("write artifact");
    path
}

fn script_artifact(package: &str, script_id: u32, name: &str, event: &str) -> String {
    format!(
        r#"{{"kind":"script","version":1,"package":"{package}","source_hash":"{HASH}","scripts":[
           {{"script_id":{script_id},"name":"{name}","event":"{event}","priority":0,
             "enabled":true,"source":"return 1"}}]}}"#
    )
}

fn delta_artifact(package: &str, spell_id: u32) -> String {
    format!(
        r#"{{"version":1,"package":"{package}","source_hash":"{HASH}","claims":[
           {{"table":"game_spell","key":{{"spell_id":{spell_id}}},"operation":"update",
             "fields":{{"cooldown_ms":{{"type":"u32","value":1500}}}}}}]}}"#
    )
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lyracore-delta-check-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn a_script_artifact_and_a_package_delta_check_out_together() {
    let dir = temp_dir("mixed");
    let delta = write(&dir, "spell.json", &delta_artifact("example.bolt", 133));
    let script = write(
        &dir,
        "bolt.script.json",
        &script_artifact("example.bolt", 100_001, "bolt.greet", "on_login"),
    );

    let output = run(&[&delta, &script]);

    assert!(output.status.success());
    let report = String::from_utf8(output.stdout).unwrap();
    assert!(report.contains("=== Package Deltas ==="), "{report}");
    assert!(report.contains("=== Runtime Scripts ==="), "{report}");
    assert!(report.contains("bolt.greet"), "{report}");
}

/// A Package Event of the artifact's OWN Package is bindable; nothing has to teach this binary
/// about it, because the parser already knows whose event it is.
#[test]
fn a_package_event_of_the_artifacts_own_package_checks_out() {
    let dir = temp_dir("package-event");
    let script = write(
        &dir,
        "bots.script.json",
        &script_artifact(
            "playerbots",
            100_100,
            "playerbots.flee",
            "playerbots.flee_at",
        ),
    );

    assert!(run(&[&script]).status.success());
}

#[test]
fn an_event_outside_the_catalogue_refuses_the_run() {
    let dir = temp_dir("unknown-event");
    let script = write(
        &dir,
        "bolt.script.json",
        &script_artifact("example.bolt", 100_001, "bolt.greet", "on_sneeze"),
    );

    let output = run(&[&script]);

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("on_sneeze"), "{stderr}");
    assert!(
        stderr.contains(script.to_string_lossy().as_ref()),
        "{stderr}"
    );
}

/// The collision the tracer exists for: two Packages claiming one Runtime Script identity. It has
/// to fail the run exactly the way a Claim Conflict does.
#[test]
fn two_packages_shipping_one_script_id_fail_the_run() {
    let dir = temp_dir("collision");
    let first = write(
        &dir,
        "first.script.json",
        &script_artifact("example.first", 100_001, "first.greet", "on_login"),
    );
    let second = write(
        &dir,
        "second.script.json",
        &script_artifact("example.second", 100_001, "second.greet", "on_login"),
    );

    let output = run(&[&first, &second]);

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("collision"), "{stderr}");
}

/// A kind this build does not read still meets the Package Delta parser, which names it rather than
/// complaining about a missing `claims` member.
#[test]
fn an_artifact_of_an_unknown_kind_is_refused_by_name() {
    let dir = temp_dir("unknown-kind");
    let path = write(&dir, "weather.json", r#"{"kind":"weather","version":1}"#);

    let output = run(&[&path]);

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("weather"), "{stderr}");
}
