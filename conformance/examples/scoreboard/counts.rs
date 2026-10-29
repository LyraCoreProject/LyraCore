use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use syn::visit::{self, Visit};

#[path = "../../support/opcodes.rs"]
mod opcodes;

pub struct Counts {
    pub variants: BTreeSet<u16>,
    pub enum_variants: BTreeSet<u16>,
    pub unavailable: BTreeSet<u16>,
    pub out_of_scope: BTreeSet<u16>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Exclusions {
    schema_version: u32,
    opcode: Vec<Exclusion>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Exclusion {
    name: String,
    value: String,
    reason: String,
}

pub fn read(root: &Path) -> Result<Counts> {
    let opcodes = opcodes::read(root)?;
    let inventory = read_inventory(&std::fs::read_to_string(
        root.join("conformance/client-requests-5875.tsv"),
    )?)?;
    for (name, value) in &opcodes {
        ensure!(
            inventory.get(name) == Some(value),
            "core opcode differs from build 5875: {name}"
        );
    }
    let variants = inventory.values().copied().collect();
    let all_opcodes = (0..=u16::MAX)
        .filter_map(|opcode| {
            wow_world_messages::vanilla::opcode_to_name(opcode.into())
                .map(|name| (name.to_owned(), opcode))
        })
        .collect();
    let unavailable = unavailable_requests(
        &std::fs::read_to_string(root.join("module/src/native/unavailable.rs"))?,
        &all_opcodes,
    )?;
    let exclusions: Exclusions = toml::from_str(&std::fs::read_to_string(
        root.join("conformance/out-of-scope-opcodes.toml"),
    )?)?;
    ensure!(
        exclusions.schema_version == 1,
        "unsupported opcode exclusion schema"
    );
    let mut out_of_scope = BTreeSet::new();
    for exclusion in exclusions.opcode {
        ensure!(
            !exclusion.reason.trim().is_empty(),
            "missing reason for {}",
            exclusion.name
        );
        let value = exclusion
            .value
            .strip_prefix("0x")
            .filter(|value| {
                value.len() == 4
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_lowercase())
            })
            .context("out-of-scope opcode must use 0x and four uppercase hexadecimal digits")?;
        let value = u16::from_str_radix(value, 16)?;
        ensure!(
            inventory.get(&exclusion.name) == Some(&value),
            "out-of-scope opcode name/value mismatch: {}",
            exclusion.name
        );
        ensure!(
            out_of_scope.insert(value),
            "duplicate out-of-scope opcode: {}",
            exclusion.name
        );
    }
    Ok(Counts {
        variants,
        enum_variants: opcodes.values().copied().collect(),
        unavailable,
        out_of_scope,
    })
}

fn read_inventory(source: &str) -> Result<BTreeMap<String, u16>> {
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

#[derive(Default)]
struct Requests {
    names: BTreeSet<String>,
    numbers: BTreeSet<u16>,
    selector: String,
    matches: usize,
}

impl<'ast> Visit<'ast> for Requests {
    fn visit_expr_match(&mut self, expression: &'ast syn::ExprMatch) {
        if matches!(expression.expr.as_ref(), syn::Expr::Path(path) if path.path.is_ident(&self.selector))
        {
            self.matches += 1;
            for arm in &expression.arms {
                if !empty_response(&arm.body) {
                    self.pattern(&arm.pat);
                }
            }
        }
        visit::visit_expr_match(self, expression);
    }
}

impl Requests {
    fn pattern(&mut self, pattern: &syn::Pat) {
        let path = match pattern {
            syn::Pat::Path(path) => Some(&path.path),
            syn::Pat::TupleStruct(pat) => Some(&pat.path),
            syn::Pat::Or(pattern) => {
                for case in &pattern.cases {
                    self.pattern(case);
                }
                None
            }
            syn::Pat::Lit(syn::ExprLit {
                lit: syn::Lit::Int(value),
                ..
            }) => {
                if let Ok(opcode) = value.base10_parse() {
                    self.numbers.insert(opcode);
                }
                None
            }
            _ => None,
        };
        if let Some(path) = path {
            if path
                .segments
                .iter()
                .any(|part| part.ident == "ClientOpcodeMessage")
            {
                self.names
                    .insert(path.segments.last().unwrap().ident.to_string());
            }
        }
    }
}

fn empty_response(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::Call(call) if call.args.len() == 1 => empty_response(&call.args[0]),
        syn::Expr::Macro(expr) => expr.mac.path.is_ident("vec") && expr.mac.tokens.is_empty(),
        _ => false,
    }
}

fn unavailable_requests(source: &str, opcodes: &BTreeMap<String, u16>) -> Result<BTreeSet<u16>> {
    let syntax = syn::parse_file(source)?;
    let mut requests = Requests::default();
    let mut functions = BTreeSet::new();
    for item in &syntax.items {
        if let syn::Item::Fn(function) = item {
            let name = function.sig.ident.to_string();
            if matches!(name.as_str(), "responses" | "raw_responses") {
                ensure!(
                    functions.insert(name.clone()),
                    "duplicate unavailable response function"
                );
                requests.selector = if name == "responses" { "msg" } else { "opcode" }.into();
                let before = requests.matches;
                requests.visit_block(&function.block);
                ensure!(requests.matches > before, "missing request match in {name}");
            }
        }
    }
    ensure!(
        functions.len() == 2,
        "missing unavailable response function"
    );
    for name in requests.names {
        requests.numbers.insert(
            *opcodes
                .get(&name)
                .with_context(|| format!("no opcode for {name}"))?,
        );
    }
    Ok(requests.numbers)
}

#[test]
fn counts_only_unavailable_request_patterns() {
    let source = r#"
        use ClientOpcodeMessage::CMSG_UNUSED;
        fn responses(msg: &ClientOpcodeMessage) {
            match msg {
                ClientOpcodeMessage::CMSG_ONE(_) | ClientOpcodeMessage::CMSG_TWO => {
                    match unrelated { 77 => reply(), _ => () }
                },
                _ => (),
            }
            let text = "ClientOpcodeMessage::CMSG_TEXT";
        }
        fn raw_responses(opcode: u32) {
            match opcode { 3 => reply(), 4 | 5 => Ok(Some(vec![])), _ => () }
        }
        fn is_control_receipt(msg: ClientOpcodeMessage) {
            match msg { ClientOpcodeMessage::CMSG_RECEIPT => (), _ => () }
        }
        #[cfg(test)] mod tests { fn example() { ClientOpcodeMessage::CMSG_TEST; } }
    "#;
    let opcodes = [("CMSG_ONE".into(), 1), ("CMSG_TWO".into(), 2)].into();
    assert_eq!(
        unavailable_requests(source, &opcodes).unwrap(),
        [1, 2, 3].into()
    );
    assert!(unavailable_requests("fn unrelated() {}", &opcodes).is_err());
}

#[test]
fn inventory_refuses_duplicate_values_and_invalid_names() {
    assert!(read_inventory("0x0001 CMSG_ONE\n0x0002 MSG_TWO\n").is_ok());
    assert!(read_inventory("0x0001 CMSG_ONE\n0x0001 MSG_TWO\n").is_err());
    assert!(read_inventory("0x0001 SMSG_ONE\n").is_err());
}
