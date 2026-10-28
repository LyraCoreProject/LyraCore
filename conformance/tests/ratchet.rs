#[path = "../support/ratchet.rs"]
mod ratchet;

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use lyracore_conformance::FeatureLedger;
use wire_client::catalogue;

#[test]
fn every_scenario_has_one_baseline() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ledger = FeatureLedger::load(&root.join("features"))?;
    let baseline = ratchet::Baseline::read(&root.join("tier1-baseline.json"))?;
    let expected: BTreeMap<_, _> = ledger
        .behaviors()
        .iter()
        .flat_map(|behavior| {
            behavior
                .scenarios
                .iter()
                .map(|id| (id.as_str(), behavior.id.as_str()))
        })
        .collect();
    let actual: BTreeMap<_, _> = baseline
        .scenarios
        .iter()
        .map(|entry| (entry.scenario.as_str(), entry.behavior.as_str()))
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(actual.len(), baseline.scenarios.len());
    assert_eq!(actual.len(), catalogue::ENTRIES.len());
    Ok(())
}
