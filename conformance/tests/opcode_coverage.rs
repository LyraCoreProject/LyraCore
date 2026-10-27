use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use anyhow::{ensure, Context, Result};
use lyracore_conformance::FeatureLedger;
use serde::Deserialize;

#[path = "../support/opcodes.rs"]
mod opcodes;

const ALLOWLIST: &str = "conformance/opcodes-unclaimed.txt";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Exclusions {
    out_of_scope: BTreeMap<String, String>,
}

fn allowlist(source: &str) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    for name in source.lines() {
        ensure!(
            !name.is_empty() && name.trim() == name,
            "invalid allowlist entry {name:?}"
        );
        ensure!(
            names.insert(name.into()),
            "duplicate allowlist entry {name}"
        );
    }
    Ok(names)
}

fn covers(
    variants: &BTreeMap<String, u16>,
    claimed: &BTreeSet<u16>,
    excluded: &BTreeMap<String, String>,
    unclaimed: &BTreeSet<String>,
) -> Result<()> {
    for (name, reason) in excluded {
        ensure!(
            variants.contains_key(name),
            "unknown out-of-scope opcode {name}"
        );
        ensure!(
            !reason.trim().is_empty(),
            "{name} needs an out-of-scope reason"
        );
        ensure!(
            !claimed.contains(&variants[name]),
            "{name} is both claimed and out of scope"
        );
    }
    for name in unclaimed {
        ensure!(
            variants.contains_key(name),
            "unknown allowlisted opcode {name}"
        );
        ensure!(
            !claimed.contains(&variants[name]) && !excluded.contains_key(name),
            "remove {name} from the allowlist: it is claimed or out of scope"
        );
    }
    let missing: Vec<_> = variants
        .iter()
        .filter(|(name, opcode)| {
            !claimed.contains(opcode) && !excluded.contains_key(*name) && !unclaimed.contains(*name)
        })
        .map(|(name, _)| name)
        .collect();
    ensure!(
        missing.is_empty(),
        "opcodes need a Behavior or out-of-scope reason: {missing:?}"
    );
    Ok(())
}

fn only_shrinks(current: &BTreeSet<String>, previous: &BTreeSet<String>) -> Result<()> {
    let added: Vec<_> = current.difference(previous).collect();
    ensure!(added.is_empty(), "opcode allowlist grew: {added:?}");
    Ok(())
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git").current_dir(root).args(args).output()?;
    ensure!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?)
}

#[test]
fn every_client_variant_has_a_behavior_scope_decision_or_remaining_allowance() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let variants = opcodes::read(root)?;
    let ledger = FeatureLedger::load(&root.join("conformance/features"))?;
    let claimed = ledger
        .behaviors()
        .iter()
        .flat_map(|behavior| &behavior.requests)
        .map(|opcode| u16::from_str_radix(&opcode[2..], 16))
        .collect::<std::result::Result<_, _>>()?;
    let excluded: Exclusions = toml::from_str(&std::fs::read_to_string(
        root.join("conformance/opcodes-out-of-scope.toml"),
    )?)?;
    let unclaimed = allowlist(&std::fs::read_to_string(root.join(ALLOWLIST))?)?;
    covers(&variants, &claimed, &excluded.out_of_scope, &unclaimed)?;
    ensure!(
        git(root, &["rev-parse", "--is-shallow-repository"])?.trim() == "false",
        "opcode allowance history requires a full checkout; run git fetch --unshallow"
    );
    let history = git(
        root,
        &[
            "log",
            "--full-history",
            "--diff-filter=AM",
            "--format=%H",
            "HEAD",
            "--",
            ALLOWLIST,
        ],
    )?;
    for commit in history.lines() {
        let previous = allowlist(&git(root, &["show", &format!("{commit}:{ALLOWLIST}")])?)?;
        only_shrinks(&unclaimed, &previous).with_context(|| format!("allowance at {commit}"))?;
    }
    println!(
        "{} client variants: {} claimed, {} out of scope, {} allowlisted",
        variants.len(),
        variants
            .values()
            .filter(|opcode| claimed.contains(opcode))
            .count(),
        excluded.out_of_scope.len(),
        unclaimed.len()
    );
    Ok(())
}

#[test]
fn coverage_detects_missing_claims_stale_allowances_and_invalid_exclusions() {
    let variants = [("CMSG_ONE".into(), 1), ("CMSG_TWO".into(), 2)].into();
    let claimed = [1].into();
    let unclaimed = ["CMSG_TWO".into()].into();
    assert!(covers(&variants, &claimed, &BTreeMap::new(), &unclaimed).is_ok());
    assert!(covers(&variants, &BTreeSet::new(), &BTreeMap::new(), &unclaimed).is_err());
    assert!(covers(&variants, &[1, 2].into(), &BTreeMap::new(), &unclaimed).is_err());
    let excluded = [("CMSG_TWO".into(), "Tool-only request".into())].into();
    assert!(covers(&variants, &claimed, &excluded, &unclaimed).is_err());
    assert!(covers(&variants, &claimed, &excluded, &BTreeSet::new()).is_ok());
    for exclusions in [
        [("CMSG_TWO".into(), " ".into())].into(),
        [("CMSG_UNKNOWN".into(), "Tool-only".into())].into(),
        [("CMSG_ONE".into(), "Tool-only".into())].into(),
    ] {
        assert!(covers(&variants, &claimed, &exclusions, &unclaimed).is_err());
    }
    assert!(covers(
        &variants,
        &claimed,
        &BTreeMap::new(),
        &["CMSG_UNKNOWN".into()].into()
    )
    .is_err());
    assert!(allowlist("CMSG_ONE\nCMSG_ONE\n").is_err());
}

#[test]
fn allowances_cannot_grow_or_exchange_a_removed_opcode_for_another() {
    let previous = allowlist("CMSG_ONE\nCMSG_TWO\n").unwrap();
    assert!(only_shrinks(&allowlist("CMSG_ONE\n").unwrap(), &previous).is_ok());
    assert!(only_shrinks(&previous, &previous).is_ok());
    for current in ["CMSG_ONE\nCMSG_TWO\nCMSG_THREE\n", "CMSG_ONE\nCMSG_THREE\n"] {
        assert!(only_shrinks(&allowlist(current).unwrap(), &previous).is_err());
    }
    assert!(only_shrinks(&previous, &allowlist("CMSG_ONE\n").unwrap()).is_err());
}
