#![cfg(feature = "native_simulation_prototype")]

use lyracore_module::native_prototype::{
    compare, scenario, Action, Request, CREATURE_GUID, SCENARIOS, TRACE_PREFIX,
};
use serde_json::{json, Value};

fn replay(name: &str) -> Vec<Value> {
    let case = scenario(name).unwrap();
    let mut simulation = case.simulation();
    let mut frames = vec![simulation.snapshot()];
    for request in &case.requests {
        frames.push(simulation.step(request).unwrap());
    }
    frames
}

#[test]
fn idle_movement_keeps_four_yards_unpublished_and_persists_arrival() {
    let frames = replay("idle_drift");
    assert_eq!(frames[3]["entities"][0]["x"], 10.0);
    assert_eq!(frames[3]["entities"][0]["last_move_ms"], 60_000);
    assert_eq!(frames[4]["entities"][0]["x"], 16.0);
    assert_eq!(frames[4]["entities"][0]["last_move_ms"], 61_500);
    assert_eq!(frames[6]["entities"][0]["x"], 22.0);
    assert_eq!(frames[6]["splines"], json!([]));
}

#[test]
fn crossing_a_cell_persists_even_a_two_yard_step() {
    let frames = replay("grid_crossing");
    let before = &frames[0]["entities"][0];
    let after = &frames[2]["entities"][0];
    assert_eq!(after["x"], 18.0);
    assert_ne!(before["grid_x"], after["grid_x"]);
    assert_ne!(before["cell"], after["cell"]);
}

#[test]
fn combat_movement_persists_each_advance() {
    let frames = replay("combat_advance");
    assert_eq!(frames[2]["entities"][0]["x"], 12.0);
    assert_eq!(frames[3]["entities"][0]["x"], 14.0);
}

#[test]
fn stationary_target_is_approached_to_four_yards() {
    let frames = replay("stopped_target");
    assert_eq!(frames[1]["splines"][0]["dx"], 26.0);
    assert_eq!(frames.last().unwrap()["entities"][0]["x"], 26.0);
    assert_eq!(frames.last().unwrap()["splines"], json!([]));
}

#[test]
fn a_moving_target_gets_an_eight_yard_lead_and_a_turn_replaces_the_leg() {
    let frames = replay("moving_target_turn_stop");
    assert_eq!(frames[1]["splines"][0]["dx"], 38.0);
    assert_eq!(frames[1]["splines"][0]["dy"], 0.0);
    assert!(frames[4]["splines"][0]["dy"].as_f64().unwrap() > 20.0);
    assert_eq!(frames[4]["splines"][0]["spline_id"], 61_000);
    assert!(frames[6]["splines"][0]["facing"].as_bool().unwrap());
}

#[test]
fn immobilization_stops_at_the_rendered_position() {
    let frames = replay("immobilized");
    assert_eq!(frames[3]["entities"][0]["x"], 14.0);
    assert_eq!(frames[3]["splines"][0]["dur_ms"], 0);
    assert_eq!(frames[3]["splines"][0]["sx"], 14.0);
    assert_eq!(frames[3]["splines"][0]["dx"], 14.0);
    assert_eq!(frames[4]["entities"][0]["x"], 14.0);
    assert_eq!(frames[6]["splines"], json!([]));
}

#[test]
fn a_missing_mover_drops_its_spline() {
    let frames = replay("missing_mover");
    assert_eq!(frames[2]["entities"], json!([]));
    assert_eq!(frames[2]["splines"][0]["guid"], CREATURE_GUID);
    assert_eq!(frames[3]["splines"], json!([]));
}

#[test]
fn the_millisecond_clock_wrap_does_not_reverse_a_spline() {
    let frames = replay("clock_wrap");
    assert_eq!(frames[4]["entities"][0]["x"], 16.0);
    assert_eq!(frames[4]["entities"][0]["last_move_ms"], 999);
    assert_eq!(frames[6]["entities"][0]["x"], 22.0);
}

#[test]
fn refused_input_keeps_rows_clock_and_sequence_unchanged() {
    let case = scenario("moving_target_turn_stop").unwrap();
    let mut simulation = case.simulation();
    let valid = &case.requests[0];
    simulation.step(valid).unwrap();
    let before = simulation.snapshot();
    let next = Request {
        sequence: 2,
        at_micros: valid.at_micros,
        action: Action::Tick,
    };
    let cases = [
        valid.clone(),
        Request {
            sequence: 3,
            ..next.clone()
        },
        Request {
            at_micros: valid.at_micros - 1,
            ..next.clone()
        },
        Request {
            at_micros: u64::MAX,
            ..next.clone()
        },
        Request {
            action: Action::MoveTarget {
                x: f32::NAN,
                y: 0.0,
                z: 0.0,
                orientation: 0.0,
                movement_flags: 0,
            },
            ..next.clone()
        },
        Request {
            action: Action::MoveTarget {
                x: 0.0,
                y: 0.0,
                z: 1.0,
                orientation: 0.0,
                movement_flags: 0,
            },
            ..next.clone()
        },
        Request {
            action: Action::MoveTarget {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                orientation: 0.0,
                movement_flags: 0x2000,
            },
            ..next.clone()
        },
    ];
    for request in cases {
        assert!(simulation.step(&request).is_err(), "{request:?}");
        assert_eq!(simulation.snapshot(), before, "{request:?}");
    }
    simulation.step(&next).unwrap();
}

#[test]
fn replay_is_deterministic_and_instances_do_not_share_state() {
    for name in SCENARIOS {
        assert_eq!(replay(name), replay(name), "{name}");
    }
    let case = scenario("idle_drift").unwrap();
    let mut first = case.simulation();
    let second = case.simulation();
    let before = second.snapshot();
    first
        .step(&Request {
            sequence: 1,
            at_micros: case.requests[0].at_micros,
            action: Action::RemoveCreature,
        })
        .unwrap();
    assert_eq!(second.snapshot(), before);
}

#[test]
fn comparison_catches_missing_rows_flags_time_and_float_drift() {
    let expected = json!({"entities": [{"guid": 9_007_199_254_740_993_u64, "x": 3.0, "dead": false}], "sequence": 1});
    for actual in [
        json!({"entities": [], "sequence": 1}),
        json!({"entities": [{"guid": 9_007_199_254_740_992_u64, "x": 3.0, "dead": false}], "sequence": 1}),
        json!({"entities": [{"guid": 9_007_199_254_740_993_u64, "x": 3.01, "dead": false}], "sequence": 1}),
        json!({"entities": [{"guid": 9_007_199_254_740_993_u64, "x": 3.0, "dead": true}], "sequence": 1}),
        json!({"entities": [{"guid": 9_007_199_254_740_993_u64, "x": 3.0, "dead": false}], "sequence": 2}),
    ] {
        assert!(compare(&expected, &actual).is_err(), "{actual}");
    }
    let mut close = expected.clone();
    close["entities"][0]["x"] = json!(3.000001);
    compare(&expected, &close).unwrap();
    close["entities"][0]["x"] = json!(4.0);
    assert!(compare(&expected, &close)
        .unwrap_err()
        .contains("entities[guid=9007199254740993].x"));
}

#[test]
fn recorded_module_trace_matches_without_starting_spacetimedb() {
    let traces = include_str!("../prototypes/native-simulation/reference.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect::<Vec<Value>>();
    assert_reference_matches(&traces);
}

fn assert_reference_matches(traces: &[Value]) {
    let expected_count: usize = SCENARIOS.iter().map(|name| replay(name).len()).sum();
    assert_eq!(
        traces.len(),
        expected_count,
        "unexpected reference frame count"
    );
    for name in SCENARIOS {
        let mut reference: Vec<_> = traces
            .iter()
            .filter(|t| t["scenario"] == *name)
            .map(|t| t["frame"].clone())
            .collect();
        reference.sort_by_key(|frame| frame["sequence"].as_u64().unwrap());
        let native = replay(name);
        assert_eq!(
            reference.len(),
            native.len(),
            "missing reference frames for {name}"
        );
        for (command, (expected, actual)) in reference.iter().zip(&native).enumerate() {
            compare(expected, actual)
                .unwrap_or_else(|difference| panic!("{name}, command {command}: {difference}"));
        }
    }
}

#[test]
#[ignore = "opt-in: requires a prototype Wasm artifact and one disposable SpacetimeDB process"]
fn table_backed_module_matches_the_native_trace() {
    let artifact = std::env::var_os("LYRACORE_NATIVE_REFERENCE_WASM")
        .expect("set LYRACORE_NATIVE_REFERENCE_WASM to the prototype Module artifact");
    let wasm = std::fs::read(artifact).unwrap();
    let mut shard = lyracore_test_support::Standalone::start("native-simulation-parity");
    shard.publish_module_bytes(&wasm);
    shard.assert_call("claim_operator", &[]);
    for name in SCENARIOS {
        shard.assert_call(
            "debug_native_simulation_reference",
            &[&format!("\"{name}\"")],
        );
    }
    let expected_count: usize = SCENARIOS.iter().map(|name| replay(name).len()).sum();
    let mut traces: Vec<Value> = Vec::new();
    assert!(lyracore_test_support::poll_until(
        std::time::Duration::from_secs(5),
        || {
            let logs = shard.module_logs(200);
            assert!(logs.status.success());
            traces = String::from_utf8(logs.stdout)
                .unwrap()
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .filter_map(|line| {
                    line["message"]
                        .as_str()?
                        .strip_prefix(TRACE_PREFIX)
                        .map(|message| serde_json::from_str(message).unwrap())
                })
                .collect();
            traces.len() >= expected_count
        }
    ));
    assert_reference_matches(&traces);
    if let Some(path) = std::env::var_os("LYRACORE_NATIVE_REFERENCE_TRACE") {
        let mut output = String::new();
        for trace in traces {
            output.push_str(&serde_json::to_string(&trace).unwrap());
            output.push('\n');
        }
        std::fs::write(path, output).unwrap();
    }
}
