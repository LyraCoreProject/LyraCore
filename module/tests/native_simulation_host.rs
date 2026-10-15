#![cfg(all(feature = "native_simulation_host", not(target_arch = "wasm32")))]

use lyracore_module::native_prototype::{
    compare, scenario, Action, Request, TableSimulation, SCENARIOS,
};
use serde_json::Value;

#[test]
fn actual_module_rules_in_native_memory_match_all_saved_wasm_frames() {
    let reference: Vec<Value> = include_str!("../prototypes/native-simulation/reference.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for name in SCENARIOS {
        let case = scenario(name).unwrap();
        let mut simulation = TableSimulation::new(&case).unwrap();
        let frames: Vec<_> = reference
            .iter()
            .filter(|frame| frame["scenario"] == *name)
            .collect();
        assert_eq!(frames.len(), case.requests.len() + 1);
        compare(&frames[0]["frame"], &simulation.snapshot().unwrap()).unwrap();
        for (request, frame) in case.requests.iter().zip(&frames[1..]) {
            compare(&frame["frame"], &simulation.step(request).unwrap())
                .unwrap_or_else(|e| panic!("{name} at {}: {e}", request.sequence));
        }
    }
}

#[test]
fn scaled_real_module_rules_match_the_prototype_after_each_tick() {
    use lyracore_module::native_prototype::benchmark::{workload, SCENARIOS};
    for name in SCENARIOS {
        let case = workload(name, 4).unwrap();
        let mut original = case.simulation();
        let mut native = TableSimulation::new(&case).unwrap();
        for request in &case.requests {
            compare(
                &original.step(request).unwrap(),
                &native.step(request).unwrap(),
            )
            .unwrap();
        }
    }
}

#[test]
fn a_refused_batch_restores_rows_and_request_sequence() {
    let case = scenario("idle_drift").unwrap();
    let mut native = TableSimulation::new(&case).unwrap();
    let before = native.snapshot().unwrap();
    let mut requests = case.requests.clone();
    requests.push(Request {
        sequence: requests.len() as u64 + 1,
        at_micros: requests.last().unwrap().at_micros,
        action: Action::MoveTarget {
            x: 1.0,
            y: 0.0,
            z: 0.0,
            orientation: 0.0,
            movement_flags: 0,
        },
    });
    assert!(native.apply_batch(&requests).is_err());
    compare(&before, &native.snapshot().unwrap()).unwrap();
    compare(
        &case.simulation().step(&case.requests[0]).unwrap(),
        &native.step(&case.requests[0]).unwrap(),
    )
    .unwrap();
}
