#![cfg(all(feature = "native_simulation_host", not(target_arch = "wasm32")))]

use std::time::Duration;

use lyracore_module::native_prototype::{
    benchmark::{digest, workload, SCENARIOS},
    compare, TableSimulation,
};
use spacetimedb::sats::bsatn;

#[path = "../examples/support/committed.rs"]
mod support;

#[test]
#[ignore = "requires a disposable SpacetimeDB process"]
fn in_memory_capture_receives_a_committed_change() {
    use lyracore_module::native_prototype::commit::STATE_TABLE;
    use lyracore_test_support::Standalone;

    let artifact = std::env::var_os("LYRACORE_NATIVE_REFERENCE_WASM").unwrap();
    let mut shard = Standalone::start("native-memory-subscription");
    shard.publish_module_bytes(&std::fs::read(artifact).unwrap());
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("debug_clear_creatures", &["0"]);
    let updates = shard.capture_updates(&format!("SELECT * FROM {STATE_TABLE}"), 1, || {
        shard.assert_call(
            "debug_native_committed_setup",
            &["\"idle_drift\"", "1", "false"],
        );
    });
    assert_eq!(updates[0][STATE_TABLE]["inserts"][0]["creatures"], 1);
}

#[test]
#[ignore = "requires a disposable persistent SpacetimeDB process"]
fn committed_frames_and_subscription_rows_match_wasm_after_every_tick() {
    let shard = support::start();
    let calls = support::Calls::new(&shard);
    for &name in SCENARIOS {
        let case = workload(name, 4).unwrap();
        let mut expected = Vec::new();
        let run = support::setup(&shard, &calls, name, 4, false);
        {
            let subscription = support::subscribe(&shard);
            for request in &case.requests {
                calls
                    .call(
                        "debug_native_committed_step",
                        &(run, request.sequence, request.at_micros),
                    )
                    .assert_accepted();
                let update = subscription.receive(Duration::from_secs(10));
                let frame = support::snapshot(&shard, &calls, run, request.sequence, true);
                expected.push((
                    support::gameplay_update(update.value),
                    frame["frame"].clone(),
                ));
            }
        }
        let run = support::setup(&shard, &calls, name, 4, true);
        let subscription = support::subscribe(&shard);
        let mut simulation = TableSimulation::new(&case).unwrap();
        for (request, (expected_update, expected_frame)) in case.requests.iter().zip(expected) {
            simulation.apply(request).unwrap();
            let batch = simulation.commit_batch(run);
            calls.commit(&batch).assert_accepted();
            let update = subscription.receive(Duration::from_secs(10));
            assert_eq!(
                support::gameplay_update(update.value),
                expected_update,
                "{name}, tick {}",
                request.sequence
            );
            let frame = support::snapshot(&shard, &calls, run, request.sequence, true);
            compare(&expected_frame, &frame["frame"]).unwrap();
            assert_eq!(frame["digest"], digest(&simulation.snapshot().unwrap()));
        }
    }
}

#[test]
#[ignore = "requires a disposable persistent SpacetimeDB process"]
fn duplicate_commit_is_idempotent_and_changed_input_is_refused() {
    let shard = support::start();
    let calls = support::Calls::new(&shard);
    let case = workload("combat_advance", 4).unwrap();
    let run = support::setup(&shard, &calls, case.name, 4, true);
    let mut simulation = TableSimulation::new(&case).unwrap();
    simulation.apply(&case.requests[0]).unwrap();
    calls
        .commit(&simulation.commit_batch(run))
        .assert_accepted();
    simulation.apply(&case.requests[1]).unwrap();
    let batch = simulation.commit_batch(run);
    assert!(!batch.changes.is_empty());
    calls.commit(&batch).assert_accepted();
    let before = support::snapshot(&shard, &calls, run, 2, true);
    calls.commit(&batch).assert_accepted();
    let mut changed = batch.clone();
    changed.at_micros += 1;
    assert!(!calls.commit(&changed).accepted);
    let after = support::snapshot(&shard, &calls, run, 2, true);
    assert_eq!(before["frame"], after["frame"]);
}

#[test]
#[ignore = "requires a disposable persistent SpacetimeDB process"]
fn stale_or_partial_commits_preserve_rows_and_progress() {
    let shard = support::start();
    let calls = support::Calls::new(&shard);
    let case = workload("combat_advance", 4).unwrap();
    let run = support::setup(&shard, &calls, case.name, 4, true);
    let mut simulation = TableSimulation::new(&case).unwrap();
    for request in &case.requests[..2] {
        simulation.apply(request).unwrap();
        if request.sequence == 1 {
            calls
                .commit(&simulation.commit_batch(run))
                .assert_accepted();
        }
    }
    let batch = simulation.commit_batch(run);
    assert!(batch.changes.len() > 1);
    let before = support::snapshot(&shard, &calls, run, 1, true);
    let mut stale = batch.clone();
    stale.run += 1;
    assert!(!calls.commit(&stale).accepted);
    let mut skipped = batch.clone();
    skipped.sequence += 1;
    assert!(!calls.commit(&skipped).accepted);
    let mut duplicate_row = batch.clone();
    duplicate_row.changes.push(duplicate_row.changes[0].clone());
    assert!(!calls.commit(&duplicate_row).accepted);
    let mut mismatched = batch.clone();
    let last = mismatched.changes.last_mut().unwrap();
    last.before = last.after.clone();
    assert!(!calls.commit(&mismatched).accepted);
    let mut partial = batch.clone();
    partial.changes.last_mut().unwrap().before = Some(vec![0xff]);
    assert!(!calls.commit(&partial).accepted);
    assert_eq!(
        before["frame"],
        support::snapshot(&shard, &calls, run, 1, true)["frame"]
    );
    let mut broken_insert = batch.clone();
    let last = broken_insert.changes.last_mut().unwrap();
    last.after = Some(last.after.as_ref().unwrap()[..8].to_vec());
    assert!(!calls.commit(&broken_insert).accepted);
    assert_eq!(
        before["frame"],
        support::snapshot(&shard, &calls, run, 1, true)["frame"]
    );
    let mut payload = bsatn::to_vec(&batch).unwrap();
    payload.push(0);
    assert!(
        !calls
            .call("debug_native_committed_apply", &(payload,))
            .accepted
    );
    calls.commit(&batch).assert_accepted();
    let run2 = support::setup(&shard, &calls, case.name, 4, true);
    assert_ne!(run2, run);
    assert!(!calls.commit(&batch).accepted);
    let frame = support::snapshot(&shard, &calls, run2, 0, true);
    assert_eq!(
        frame["digest"],
        digest(&TableSimulation::new(&case).unwrap().snapshot().unwrap())
    );
}

#[test]
#[ignore = "requires a disposable persistent SpacetimeDB process"]
fn restart_retains_rows_and_receipt_for_a_lost_acknowledgment() {
    let mut shard = support::start();
    let calls = support::Calls::new(&shard);
    let case = workload("combat_advance", 4).unwrap();
    let run = support::setup(&shard, &calls, case.name, 4, true);
    let subscription = support::subscribe(&shard);
    let mut simulation = TableSimulation::new(&case).unwrap();
    simulation.apply(&case.requests[0]).unwrap();
    calls
        .commit(&simulation.commit_batch(run))
        .assert_accepted();
    subscription.receive(Duration::from_secs(10));
    simulation.apply(&case.requests[1]).unwrap();
    let batch = simulation.commit_batch(run);
    assert!(!batch.changes.is_empty());
    calls.commit(&batch).assert_accepted();
    subscription.receive(Duration::from_secs(10));
    drop(subscription);
    let before = support::snapshot(&shard, &calls, run, 2, true);
    shard.restart_persistent();
    let calls = support::Calls::new(&shard);
    calls.commit(&batch).assert_accepted();
    assert_eq!(
        before["frame"],
        support::snapshot(&shard, &calls, run, 2, true)["frame"]
    );
    simulation.apply(&case.requests[2]).unwrap();
    calls
        .commit(&simulation.commit_batch(run))
        .assert_accepted();
    let after = support::snapshot(&shard, &calls, run, 3, true);
    assert_eq!(after["digest"], digest(&simulation.snapshot().unwrap()));
}
