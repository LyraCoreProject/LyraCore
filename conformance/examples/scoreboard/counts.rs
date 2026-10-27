use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use anyhow::{ensure, Context, Result};
use serde_json::Value;
use syn::visit::{self, Visit};

pub struct Counts {
    pub variants: BTreeSet<u16>,
    pub unavailable: BTreeSet<u16>,
}

pub fn read(root: &Path) -> Result<Counts> {
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
            "--offline",
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
    let variants = names
        .iter()
        .map(|name| {
            opcodes
                .get(name)
                .copied()
                .with_context(|| format!("no opcode for {name}"))
        })
        .collect::<Result<_>>()?;
    let unavailable = unavailable_requests(
        &std::fs::read_to_string(root.join("module/src/native/unavailable.rs"))?,
        &opcodes,
    )?;
    Ok(Counts {
        variants,
        unavailable,
    })
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
fn counts_enum_variants_and_only_unavailable_request_patterns() {
    assert_eq!(
        client_variants("enum ClientOpcodeMessage { CMSG_ONE, CMSG_TWO(u8) } // CMSG_THREE")
            .unwrap()
            .len(),
        2
    );
    assert!(client_variants("// enum ClientOpcodeMessage { CMSG_ONE }").is_err());
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
