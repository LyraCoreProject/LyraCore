use std::path::Path;

use lyracore_conformance::FeatureLedger;
use wire_client::catalogue;

#[test]
fn every_ledger_entry_parses_and_references_known_behaviors_and_scenarios() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ledger = FeatureLedger::load(&root.join("features")).unwrap();
    let scenarios: Vec<_> = catalogue::ENTRIES
        .iter()
        .map(|entry| {
            let golden = catalogue::get(entry.id).unwrap();
            assert_eq!(entry.id, golden.scenario().id);
            golden
        })
        .collect();
    ledger
        .validate_scenarios(scenarios.iter().map(|golden| {
            let scenario = golden.scenario();
            (scenario.id.as_str(), scenario.behavior.as_str())
        }))
        .unwrap();
    println!("validated {} Behavior files", ledger.behaviors().len());
}
