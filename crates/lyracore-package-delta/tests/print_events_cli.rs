use std::process::{Command, Output};

use lyracore_package_delta::HOOK_EVENT_NAMES;

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lyracore-delta-check"))
        .args(args)
        .output()
        .expect("spawn lyracore-delta-check")
}

#[test]
fn print_events_lists_every_catalogue_name_in_order_one_per_line() {
    let output = run(&["--print-events"]);
    assert!(output.status.success());

    let expected: String = HOOK_EVENT_NAMES
        .iter()
        .map(|event| format!("{event}\n"))
        .collect();
    assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
}

#[test]
fn print_events_writes_nothing_to_stderr() {
    let output = run(&["--print-events"]);
    assert!(output.stderr.is_empty());
}

#[test]
fn print_events_combined_with_an_artifact_path_is_refused() {
    let output = run(&["--print-events", "some-artifact.json"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--print-events"));
}

#[test]
fn existing_no_argument_usage_is_unchanged() {
    let output = run(&[]);
    assert!(!output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("lyracore-delta-check <artifact.json>..."));
}

#[test]
fn existing_help_flag_is_unchanged() {
    let output = run(&["--help"]);
    assert!(!output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Claim Conflict"));
}
