use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

pub fn read(root: &Path) -> Result<BTreeMap<String, u16>> {
    let rustc = Command::new("rustc").arg("-vV").output()?;
    ensure!(rustc.status.success(), "rustc -vV failed");
    let rustc = String::from_utf8(rustc.stdout)?;
    let host = rustc
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .context("Rust host triple")?;
    let output = Command::new("cargo")
        .current_dir(root)
        .args([
            "metadata",
            "--locked",
            "--format-version",
            "1",
            "--filter-platform",
            host,
        ])
        .output()?;
    ensure!(
        output.status.success(),
        "cargo metadata: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Value = serde_json::from_slice(&output.stdout)?;
    let packages: Vec<_> = metadata["packages"]
        .as_array()
        .context("Cargo packages")?
        .iter()
        .filter(|package| package["name"] == "wow_world_messages")
        .collect();
    ensure!(packages.len() == 1, "expected one core opcode catalogue");
    let manifest = Path::new(
        packages[0]["manifest_path"]
            .as_str()
            .context("opcode manifest")?,
    );
    let source = std::fs::read_to_string(
        manifest
            .parent()
            .context("opcode crate")?
            .join("src/world/vanilla/opcodes.rs"),
    )?;
    let names = client_variants(&source)?;
    let opcodes: BTreeMap<_, _> = (0..=u16::MAX)
        .filter_map(|opcode| {
            wow_world_messages::vanilla::opcode_to_name(opcode.into())
                .map(|name| (name.to_owned(), opcode))
        })
        .collect();
    names
        .into_iter()
        .map(|name| {
            let opcode = *opcodes
                .get(&name)
                .with_context(|| format!("no opcode for {name}"))?;
            Ok((name, opcode))
        })
        .collect()
}

pub fn read_inventory(root: &Path) -> Result<BTreeMap<String, u16>> {
    parse_inventory(&std::fs::read_to_string(
        root.join("conformance/client-requests-5875.tsv"),
    )?)
}

fn parse_inventory(source: &str) -> Result<BTreeMap<String, u16>> {
    let mut names = BTreeMap::new();
    let mut values = BTreeSet::new();
    for line in source.lines() {
        let (value, name) = line
            .split_once(' ')
            .context("invalid client opcode inventory row")?;
        ensure!(
            value.len() == 6
                && value.starts_with("0x")
                && value[2..]
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_lowercase()),
            "invalid client opcode number: {value}"
        );
        ensure!(
            (name.starts_with("CMSG_") || name.starts_with("MSG_"))
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_'),
            "invalid client opcode name: {name}"
        );
        let value = u16::from_str_radix(&value[2..], 16)?;
        ensure!(
            values.insert(value),
            "duplicate client opcode number: {value:#06X}"
        );
        ensure!(
            names.insert(name.to_owned(), value).is_none(),
            "duplicate client opcode name: {name}"
        );
    }
    ensure!(!names.is_empty(), "empty client opcode inventory");
    Ok(names)
}

fn client_variants(source: &str) -> Result<BTreeSet<String>> {
    let syntax = syn::parse_file(source)?;
    let enums: Vec<_> = syntax
        .items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Enum(item) if item.ident == "ClientOpcodeMessage" => Some(item),
            _ => None,
        })
        .collect();
    ensure!(enums.len() == 1, "expected one ClientOpcodeMessage enum");
    let names: BTreeSet<_> = enums[0]
        .variants
        .iter()
        .map(|variant| variant.ident.to_string())
        .collect();
    ensure!(!names.is_empty(), "empty ClientOpcodeMessage enum");
    Ok(names)
}

#[test]
fn enum_scanner_excludes_comments_and_other_items() {
    assert_eq!(
        client_variants("enum ClientOpcodeMessage { CMSG_ONE, CMSG_TWO(u8) } // CMSG_THREE")
            .unwrap(),
        ["CMSG_ONE".into(), "CMSG_TWO".into()].into()
    );
    assert!(client_variants("// enum ClientOpcodeMessage { CMSG_ONE }").is_err());
}

#[test]
fn inventory_refuses_duplicate_values_and_invalid_names() {
    assert!(parse_inventory("0x0001 CMSG_ONE\n0x0002 MSG_TWO\n").is_ok());
    assert!(parse_inventory("0x0001 CMSG_ONE\n0x0001 MSG_TWO\n").is_err());
    assert!(parse_inventory("0x0001 SMSG_ONE\n").is_err());
}
