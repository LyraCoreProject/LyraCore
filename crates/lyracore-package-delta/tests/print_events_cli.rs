//! `lyracore-delta-check --print-events` — the Event Binding catalogue on stdout, nothing else.
//!
//! T2's TypeScript authoring surface is generated from this output, so the shape is the contract:
//! one event per line, in catalogue order, no artifact read, no file touched. These tests run the
//! real binary rather than calling a library function, because the binary's argument handling is
//! part of what T2's build invokes.

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

/// A generator re-running this twice must see the same bytes, or a diff-based "is this stale"
/// check (T2's `packages check`) would fail on a rebuild that changed nothing.
#[test]
fn print_events_output_is_byte_stable_across_runs() {
    let first = run(&["--print-events"]).stdout;
    let second = run(&["--print-events"]).stdout;
    assert_eq!(first, second);
}

#[test]
fn print_events_writes_nothing_to_stderr_and_touches_no_file() {
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
