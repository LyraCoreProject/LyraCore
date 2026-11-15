use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::OnceLock;

use anyhow::{ensure, Result};
use lyracore_conformance::FeatureLedger;
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[path = "../support/opcodes.rs"]
mod opcodes;

#[path = "../support/scratch.rs"]
mod scratch;

const EXCLUSIONS: &str = "conformance/opcodes-out-of-scope.toml";
const INVENTORY_COUNT: usize = 465;
const INVENTORY_SHA256: &str = "2ac5acc43e5e0f8eea113ec75626640557e801194324b6af08d561e5a9d8a216";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Exclusions {
    out_of_scope: BTreeMap<String, Exclusion>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Exclusion {
    reason: String,
    source: String,
}

fn scope_source(root: &Path, opcode: &str, source: &str) -> bool {
    let path = Path::new(source);
    if source.starts_with("docs/adr/")
        && source.ends_with(".md")
        && path
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
    {
        return std::fs::read_to_string(root.join(path)).is_ok_and(|text| {
            text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .any(|word| word == opcode)
        });
    }
    let Some(reference) = source.strip_prefix("https://github.com/vmangos/core/blob/") else {
        return false;
    };
    let Some((sha, path)) = reference.split_once('/') else {
        return false;
    };
    pinned_source_commits().contains(sha)
        && path.starts_with("src/")
        && Path::new(path)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

fn pinned_source_commits() -> &'static BTreeSet<String> {
    static COMMITS: OnceLock<BTreeSet<String>> = OnceLock::new();
    COMMITS.get_or_init(|| {
        let commits: Option<BTreeSet<_>> = wire_client::catalogue::ENTRIES
            .iter()
            .map(|entry| {
                let golden = wire_client::catalogue::get(entry.id).ok()?;
                match &golden.trace().header.origin {
                    wire_client::trace::Origin::Golden { provenance } => {
                        Some(provenance.source_commit.clone())
                    }
                    _ => None,
                }
            })
            .collect();
        commits
            .filter(|commits| commits.len() == 1)
            .unwrap_or_default()
    })
}

fn inventory_matches_pin(variants: &BTreeMap<String, u16>) -> Result<()> {
    let mut rows: Vec<_> = variants
        .iter()
        .map(|(name, value)| (*value, name))
        .collect();
    rows.sort_unstable();
    let canonical = rows
        .iter()
        .map(|(value, name)| format!("0x{value:04X} {name}\n"))
        .collect::<String>();
    ensure!(
        rows.len() == INVENTORY_COUNT
            && format!("{:x}", Sha256::digest(canonical.as_bytes())) == INVENTORY_SHA256,
        "client request inventory differs from pinned vmangos Opcodes_1_12_1.h"
    );
    Ok(())
}

fn scope_sources_are_pinned(root: &Path, excluded: &BTreeMap<String, Exclusion>) -> Result<()> {
    for (name, entry) in excluded {
        ensure!(
            scope_source(root, name, &entry.source),
            "out-of-scope opcode {name} needs an ADR or vmangos source at a pinned SHA, not {}",
            entry.source
        );
    }
    Ok(())
}

fn covers(
    variants: &BTreeMap<String, u16>,
    claimed: &BTreeSet<u16>,
    excluded: &BTreeMap<String, Exclusion>,
) -> Result<()> {
    for (name, entry) in excluded {
        ensure!(
            variants.contains_key(name),
            "unknown out-of-scope opcode {name}"
        );
        ensure!(
            !entry.reason.trim().is_empty(),
            "{name} needs an out-of-scope reason"
        );
        ensure!(
            !claimed.contains(&variants[name]),
            "{name} is both claimed and out of scope"
        );
    }
    let missing: Vec<_> = variants
        .iter()
        .filter(|(name, opcode)| !claimed.contains(opcode) && !excluded.contains_key(*name))
        .map(|(name, _)| name)
        .collect();
    ensure!(
        missing.is_empty(),
        "opcodes need a Behavior or out-of-scope reason: {missing:?}"
    );
    Ok(())
}

#[test]
fn every_client_request_has_a_behavior_or_scope_decision() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let variants = opcodes::read_inventory(root)?;
    inventory_matches_pin(&variants)?;
    for (name, value) in opcodes::read(root)? {
        ensure!(
            variants.get(&name) == Some(&value),
            "core opcode differs from build 5875: {name}"
        );
    }
    let ledger = FeatureLedger::load(&root.join("conformance/features"))?;
    let claimed = ledger
        .behaviors()
        .iter()
        .flat_map(|behavior| &behavior.requests)
        .map(|opcode| u16::from_str_radix(&opcode[2..], 16))
        .collect::<std::result::Result<_, _>>()?;
    let excluded: Exclusions = toml::from_str(&std::fs::read_to_string(root.join(EXCLUSIONS))?)?;
    covers(&variants, &claimed, &excluded.out_of_scope)?;
    scope_sources_are_pinned(root, &excluded.out_of_scope)?;
    println!(
        "{} client requests: {} claimed, {} out of scope",
        variants.len(),
        variants
            .values()
            .filter(|opcode| claimed.contains(opcode))
            .count(),
        excluded.out_of_scope.len()
    );
    Ok(())
}

#[test]
fn removing_an_inventory_row_fails() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut variants = opcodes::read_inventory(root)?;
    inventory_matches_pin(&variants)?;
    variants.remove("CMSG_STABLE_REVIVE_PET");
    assert!(inventory_matches_pin(&variants).is_err());
    Ok(())
}

#[test]
fn coverage_detects_missing_claims_and_invalid_exclusions() {
    let variants = [("CMSG_ONE".into(), 1), ("CMSG_TWO".into(), 2)].into();
    let claimed = [1].into();
    assert!(covers(&variants, &[1, 2].into(), &BTreeMap::new()).is_ok());
    assert!(covers(&variants, &claimed, &BTreeMap::new()).is_err());
    let excluded = [("CMSG_TWO".into(), exclusion("Tool-only request"))].into();
    assert!(covers(&variants, &claimed, &excluded).is_ok());
    for exclusions in [
        [("CMSG_TWO".into(), exclusion(" "))].into(),
        [("CMSG_UNKNOWN".into(), exclusion("Tool-only"))].into(),
        [("CMSG_ONE".into(), exclusion("Tool-only"))].into(),
    ] {
        assert!(covers(&variants, &claimed, &exclusions).is_err());
    }
}

fn exclusion(reason: &str) -> Exclusion {
    Exclusion {
        reason: reason.into(),
        source: String::new(),
    }
}

#[test]
fn scope_entries_require_a_pinned_source() -> Result<()> {
    let directory = scratch::Scratch::new("scope-source")?;
    let root = &*directory;
    std::fs::create_dir_all(root.join("docs/adr"))?;
    std::fs::write(
        root.join("docs/adr/scope.md"),
        "CMSG_ONE is a tool-only opcode.",
    )?;
    std::fs::write(
        root.join("docs/adr/unrelated.md"),
        "CMSG_ONE_MORE does not name the opcode.",
    )?;
    let mut excluded = BTreeMap::from([("CMSG_ONE".into(), exclusion("Tool-only"))]);
    assert!(scope_sources_are_pinned(root, &excluded).is_err());
    for source in ["docs/adr/scope.md",
        "https://github.com/vmangos/core/blob/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Server/Opcodes.cpp"] {
        excluded.get_mut("CMSG_ONE").unwrap().source = source.into();
        assert!(scope_sources_are_pinned(root, &excluded).is_ok());
    }
    for source in [
        "docs/adr/missing.md",
        "docs/adr/unrelated.md",
        "https://github.com/vmangos/core/blob/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/src/game/Server/Opcodes.cpp",
        "docs/adr/../../Cargo.toml",
        "because",
        "https://github.com/vmangos/core/blob/main/src/game/Server/Opcodes.cpp",
    ] {
        excluded.get_mut("CMSG_ONE").unwrap().source = source.into();
        assert!(scope_sources_are_pinned(root, &excluded).is_err());
    }
    Ok(())
}
