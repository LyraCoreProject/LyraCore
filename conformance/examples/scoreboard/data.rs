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
    let gaps: Vec<(&str, &str)> = ledger
        .behaviors()
        .iter()
        .flat_map(|behavior| {
            behavior
                .known_gaps
                .iter()
                .map(|gap| (behavior.system.as_str(), gap.path.as_str()))
        })
        .collect();
    let data = gaps.iter().filter(|(_, path)| is_data_gap(path)).count();
    assert!(
        0 < data && data < gaps.len(),
        "the ledger has both kinds of gap"
    );
    assert_eq!(coverage.gaps.data_import, data);
    assert_eq!(coverage.gaps.behavior, gaps.len() - data);

    let mut sources = BTreeSet::new();
    let mut parsed_rows = false;
    for (system, covered) in &coverage.systems {
        for (path, counts) in &covered.by_source {
            let source = (system.as_str(), path.as_str());
            let occurrences = gaps.iter().filter(|gap| **gap == source).count();
            let (own, other) = if is_data_gap(path) {
                (counts.data_import, counts.behavior)
            } else {
                (counts.behavior, counts.data_import)
            };
            assert_eq!((own, other), (occurrences, 0), "{system} {path}");
            sources.insert(source);
        }
        if covered.gaps.data_import == 0 {
            assert!(covered.port_census.is_empty(), "{system}");
        }
        for (family, census) in &covered.port_census {
            parsed_rows |= !census.planned_import_rows_by_table.is_empty();
            assert!(census.manifest["findings"].is_array(), "{system} {family}");
        }
    }
    assert!(parsed_rows, "a port census lists planned import rows");
    assert_eq!(sources, gaps.into_iter().collect::<BTreeSet<_>>());
    Ok(())
}

#[test]
fn only_importer_paths_are_data_import_gaps() {
    assert!(is_data_gap("importer/src/world_import_creatures.rs"));
    assert!(!is_data_gap("module/src/spell/cast/resolve.rs"));
}
