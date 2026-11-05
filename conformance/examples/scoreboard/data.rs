use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result};
use lyracore_conformance::FeatureLedger;
use serde::Serialize;
use serde_json::Value;

pub fn is_data_gap(path: &str) -> bool {
    Path::new(path).starts_with("importer")
}

#[derive(Default, Serialize)]
pub struct GapCounts {
    pub behavior: usize,
    pub data_import: usize,
}

#[derive(Default, Serialize)]
pub struct Coverage {
    pub gaps: GapCounts,
    pub systems: BTreeMap<String, SystemCoverage>,
}

#[derive(Default, Serialize)]
pub struct SystemCoverage {
    pub gaps: GapCounts,
    pub by_source: BTreeMap<String, GapCounts>,
    pub port_census: BTreeMap<String, Census>,
}

#[derive(Serialize)]
pub struct Census {
    pub planned_import_rows_by_table: BTreeMap<String, u64>,
    pub manifest: Value,
}

fn census(source: &str) -> Result<Census> {
    let rows = source
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split('|').map(str::trim).collect();
            if fields.len() == 5 && fields[1].starts_with("game_") {
                Some(
                    fields[3]
                        .parse::<u64>()
                        .map(|rows| (fields[1].to_owned(), rows))
                        .map_err(Into::into),
                )
            } else {
                None
            }
        })
        .collect::<Result<_>>()?;
    let manifest = source
        .split_once("## Compatibility Manifest")
        .and_then(|(_, section)| section.split_once("```json\n"))
        .and_then(|(_, section)| section.split_once("```"))
        .context("port census Compatibility Manifest")?
        .0;
    Ok(Census {
        planned_import_rows_by_table: rows,
        manifest: serde_json::from_str(manifest)?,
    })
}

pub fn coverage(ledger: &FeatureLedger) -> Result<Coverage> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("Core root")?;
    let mut result = Coverage::default();
    for behavior in ledger.behaviors() {
        let system = result.systems.entry(behavior.system.clone()).or_default();
        let mut reports = BTreeSet::new();
        for gap in &behavior.known_gaps {
            let data = is_data_gap(&gap.path);
            let counts = system.by_source.entry(gap.path.clone()).or_default();
            if data {
                counts.data_import += 1;
                system.gaps.data_import += 1;
                result.gaps.data_import += 1;
                for word in gap.evidence.split_whitespace() {
                    let path = word.trim_end_matches([';', '.', ',']);
                    if let Some(name) = path.strip_prefix("conformance/vmangos-port-census/") {
                        if !name.contains('/') && name.ends_with(".md") {
                            reports.insert(path.to_owned());
                        }
                    }
                }
            } else {
                counts.behavior += 1;
                system.gaps.behavior += 1;
                result.gaps.behavior += 1;
            }
        }
        for path in reports {
            if let Entry::Vacant(entry) = system.port_census.entry(path) {
                let source = std::fs::read_to_string(root.join(entry.key()))?;
                entry.insert(census(&source)?);
            }
        }
    }
    Ok(result)
}

#[test]
fn coverage_keeps_ledger_gaps_and_source_occurrences_separate() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ledger = FeatureLedger::load(&root.join("features"))?;
    let coverage = coverage(&ledger)?;
    assert_eq!(coverage.gaps.data_import, 2334);
    assert_eq!(coverage.gaps.behavior, 89);
    assert_eq!(
        coverage.systems["spells"].by_source["module/src/creatures/cycle/mod.rs"].behavior,
        1
    );
    assert_eq!(
        coverage.systems["inventory"].by_source["module/src/items/inventory.rs"].behavior,
        1
    );
    let creatures = &coverage.systems["creatures"];
    assert_eq!(
        creatures.by_source["importer/src/world_import_creatures.rs"].data_import,
        2252
    );
    let quests =
        &coverage.systems["quests"].port_census["conformance/vmangos-port-census/quests.md"];
    assert_eq!(
        quests.planned_import_rows_by_table["game_quest_template"],
        4253
    );
    let breadcrumb = quests.manifest["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["code"] == "quests.breadcrumb")
        .unwrap();
    assert_eq!(breadcrumb["classification"], "unsupported");
    assert_eq!(breadcrumb["occurrences"], 23);
    Ok(())
}
