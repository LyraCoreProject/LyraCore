//! Inventory source evidence by numeric world opcode and direction. This does not certify behavior.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use syn::visit::{self, Visit};

type References = BTreeMap<String, BTreeSet<String>>;

#[derive(Default)]
struct SourceScan {
    references: References,
    dispatch: References,
    ignored: BTreeSet<String>,
    client_variants: BTreeSet<String>,
    file: String,
    function: String,
}

fn test_only(attributes: &[syn::Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        if attribute.path().is_ident("test") {
            return true;
        }
        let syn::Meta::List(meta) = &attribute.meta else {
            return false;
        };
        if !meta.path.is_ident("cfg") {
            return false;
        }
        let cfg = meta.tokens.to_string();
        cfg == "test" || cfg.starts_with("all (test ,") || cfg.starts_with("all (test,")
    })
}

impl SourceScan {
    fn record_pattern(&mut self, pat: &syn::Pat, ignored: bool) {
        let mut pattern = SourceScan::default();
        pattern.visit_pat(pat);
        for name in pattern.client_variants {
            let location = self.location();
            self.dispatch
                .entry(name.clone())
                .or_default()
                .insert(location);
            if ignored {
                self.ignored.insert(name);
            }
        }
    }

    fn location(&self) -> String {
        format!("{}::{}", self.file, self.function)
    }

    fn scan(&mut self, path: &Path, root: &Path) -> Result<()> {
        self.file = path.strip_prefix(root)?.to_string_lossy().into_owned();
        let source = std::fs::read_to_string(path)?;
        let syntax =
            syn::parse_file(&source).with_context(|| format!("parse {}", path.display()))?;
        self.visit_file(&syntax);
        Ok(())
    }
}

impl<'ast> Visit<'ast> for SourceScan {
    fn visit_item_use(&mut self, _: &'ast syn::ItemUse) {}
    fn visit_attribute(&mut self, _: &'ast syn::Attribute) {}
    fn visit_item_const(&mut self, _: &'ast syn::ItemConst) {}

    fn visit_macro(&mut self, item: &'ast syn::Macro) {
        if !item.path.is_ident("matches") {
            return;
        }
        // Benilla lists movement relay opcodes in a matches! pattern.
        let parse = |input: syn::parse::ParseStream<'_>| {
            let value: syn::Expr = input.parse()?;
            input.parse::<syn::Token![,]>()?;
            let pattern = syn::Pat::parse_multi_with_leading_vert(input)?;
            let guard = if input.peek(syn::Token![if]) {
                input.parse::<syn::Token![if]>()?;
                Some(input.parse::<syn::Expr>()?)
            } else {
                None
            };
            if input.peek(syn::Token![,]) {
                input.parse::<syn::Token![,]>()?;
            }
            Ok((value, pattern, guard))
        };
        if let Ok((value, pattern, guard)) = syn::parse::Parser::parse2(parse, item.tokens.clone())
        {
            self.visit_expr(&value);
            self.record_pattern(&pattern, false);
            self.visit_pat(&pattern);
            if let Some(guard) = guard {
                self.visit_expr(&guard);
            }
        }
    }

    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        if !test_only(&item.attrs) {
            visit::visit_item_mod(self, item);
        }
    }

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if !test_only(&item.attrs) {
            let previous = std::mem::replace(&mut self.function, item.sig.ident.to_string());
            self.visit_block(&item.block);
            self.function = previous;
        }
    }

    fn visit_item_impl(&mut self, item: &'ast syn::ItemImpl) {
        if !test_only(&item.attrs) {
            visit::visit_item_impl(self, item);
        }
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if !test_only(&item.attrs) {
            let previous = std::mem::replace(&mut self.function, item.sig.ident.to_string());
            self.visit_block(&item.block);
            self.function = previous;
        }
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        let client_variant = path
            .segments
            .iter()
            .any(|segment| segment.ident == "ClientOpcodeMessage");
        for segment in &path.segments {
            let name = segment.ident.to_string();
            if ["CMSG_", "SMSG_", "MSG_"]
                .iter()
                .any(|prefix| name.starts_with(prefix))
            {
                let location = self.location();
                self.references
                    .entry(name.clone())
                    .or_default()
                    .insert(location);
                if client_variant {
                    self.client_variants.insert(name);
                }
            }
        }
        visit::visit_path(self, path);
    }

    fn visit_arm(&mut self, arm: &'ast syn::Arm) {
        let ignored =
            matches!(arm.body.as_ref(), syn::Expr::Block(block) if block.block.stmts.is_empty());
        self.record_pattern(&arm.pat, ignored);
        visit::visit_arm(self, arm);
    }

    fn visit_local(&mut self, local: &'ast syn::Local) {
        self.record_pattern(&local.pat, false);
        visit::visit_local(self, local);
    }

    fn visit_expr_let(&mut self, expr: &'ast syn::ExprLet) {
        self.record_pattern(&expr.pat, false);
        visit::visit_expr_let(self, expr);
    }
}

fn rust_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let path = entry?.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if name.contains("test") || name == "bindings" {
            continue;
        }
        if path.is_dir() {
            files.extend(rust_files(&path)?);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn metadata(root: &Path) -> Result<Value> {
    let output = Command::new("cargo")
        .args(["metadata", "--locked", "--offline", "--format-version", "1"])
        .current_dir(root)
        .output()?;
    if !output.status.success() {
        bail!(
            "cargo metadata: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn benilla_package(metadata: &Value) -> Result<&Value> {
    metadata["packages"]
        .as_array()
        .context("Cargo package list")?
        .iter()
        .find(|package| package["name"] == "benilla-protocol")
        .context("benilla-protocol must be resolved in Cargo.lock")
}

fn benilla_opcodes(root: &Path) -> Result<BTreeMap<String, u16>> {
    let source = std::fs::read_to_string(root.join("src/messages/opcode.rs"))?;
    let mut names = BTreeMap::new();
    for item in syn::parse_file(&source)?.items {
        if let syn::Item::Const(item) = item {
            if let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Int(value),
                ..
            }) = *item.expr
            {
                names.insert(item.ident.to_string(), value.base10_parse()?);
            }
        }
    }
    Ok(names)
}

fn row(
    direction: &str,
    opcode: u16,
    names: &[String],
    benilla: &SourceScan,
    lyra: &SourceScan,
) -> Value {
    let lyra_name = wow_world_messages::vanilla::opcode_to_name(u32::from(opcode));
    let mut evidence = BTreeSet::new();
    for name in names {
        evidence.extend(benilla.references.get(name).into_iter().flatten().cloned());
    }
    let dispatch = lyra_name.and_then(|name| lyra.dispatch.get(name));
    let status = if direction == "gateway_to_client" {
        "source_evidence_only"
    } else if lyra_name.is_some_and(|name| lyra.ignored.contains(name)) {
        "explicitly_ignored"
    } else if dispatch.is_some() {
        "dispatch_pattern_present"
    } else if matches!(
        opcode,
        0x005a
            | 0x00f9
            | 0x0125
            | 0x01ed
            | 0x0200
            | 0x0205
            | 0x0207
            | 0x023e
            | 0x0258
            | 0x0312
            | 0x0317
            | 0x0318
    ) {
        "raw_route_review_required"
    } else {
        "no_dispatch_pattern_found"
    };
    json!({
        "direction": direction,
        "opcode": format!("0x{opcode:04X}"),
        "benilla_names": names,
        "lyracore_name": lyra_name,
        "benilla_evidence": evidence,
        "benilla_decoder_present": if direction == "gateway_to_client" { Some(!names.is_empty()) } else { None },
        "lyracore_references": lyra_name.and_then(|name| lyra.references.get(name)),
        "lyracore_dispatch": dispatch,
        "handling": status,
    })
}

fn inventory() -> Result<Value> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("workspace root")?;
    let metadata = metadata(root)?;
    let package = benilla_package(&metadata)?;
    let benilla_root = Path::new(
        package["manifest_path"]
            .as_str()
            .context("Benilla manifest")?,
    )
    .parent()
    .context("Benilla crate directory")?;
    let benilla_repo = benilla_root
        .parent()
        .and_then(Path::parent)
        .context("Benilla checkout")?;
    let names = benilla_opcodes(benilla_root)?;

    let mut sends = SourceScan::default();
    for path in rust_files(&benilla_root.join("src/world"))? {
        sends.scan(&path, benilla_repo)?;
    }
    // Movement sends select their opcodes outside WorldWriter::send_movement.
    let mut movement = SourceScan::default();
    movement.scan(&benilla_root.join("src/messages/movement.rs"), benilla_repo)?;
    movement.scan(
        &benilla_repo.join("crates/benilla-app/src/net.rs"),
        benilla_repo,
    )?;
    for (name, locations) in movement.references {
        let selectors: BTreeSet<_> = locations
            .into_iter()
            .filter(|location| location.ends_with("::ack_opcode") || location.ends_with("::opcode"))
            .collect();
        if !selectors.is_empty() {
            sends.references.entry(name).or_default().extend(selectors);
        }
    }
    let mut receives = SourceScan::default();
    receives.scan(&benilla_root.join("src/messages/parse.rs"), benilla_repo)?;
    let mut lyra = SourceScan::default();
    for path in rust_files(&root.join("gateway/src"))? {
        lyra.scan(&path, root)?;
    }

    let mut rows = Vec::new();
    for (direction, source) in [
        ("client_to_gateway", &sends),
        ("gateway_to_client", &receives),
    ] {
        let mut by_number: BTreeMap<u16, Vec<String>> = BTreeMap::new();
        for name in source.references.keys() {
            if (direction == "client_to_gateway" && name.starts_with("SMSG_"))
                || (direction == "gateway_to_client" && name.starts_with("CMSG_"))
            {
                continue;
            }
            if let Some(&number) = names.get(name) {
                by_number.entry(number).or_default().push(name.clone());
            }
        }
        if direction == "gateway_to_client" {
            for number in 0..=u16::MAX {
                if let Some(name) = wow_world_messages::vanilla::opcode_to_name(u32::from(number)) {
                    if name.starts_with("SMSG_") && lyra.references.contains_key(name) {
                        by_number.entry(number).or_default();
                    }
                }
            }
        }
        for (number, names) in by_number {
            rows.push(row(direction, number, &names, source, &lyra));
        }
    }
    Ok(json!({
        "benilla_source": package["source"],
        "scope": "World messages. Source evidence is not scenario verification. Raw numeric sends and macros other than matches! require manual inspection. No decoder reference is itself a requirement to emit a message.",
        "messages": rows,
    }))
}

fn main() -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&inventory()?)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_benilla_requests_have_a_dispatch_pattern_or_known_raw_route() {
        let inventory = inventory().unwrap();
        let missing: Vec<_> = inventory["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["handling"] == "no_dispatch_pattern_found")
            .map(|row| (&row["opcode"], &row["benilla_names"]))
            .collect();
        assert!(
            missing.is_empty(),
            "requests need protocol review: {missing:?}"
        );
    }

    #[test]
    fn inventory_finds_movement_relay_selector_patterns() {
        let syntax = syn::parse_file(
            r#"
            const fn is_movement_relay(opcode: u16) -> bool {
                matches!(opcode, opcode::MSG_MOVE_START_FORWARD | opcode::MSG_MOVE_HEARTBEAT)
            }
        "#,
        )
        .unwrap();
        let mut scan = SourceScan::default();
        scan.visit_file(&syntax);
        assert_eq!(
            scan.references
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["MSG_MOVE_HEARTBEAT", "MSG_MOVE_START_FORWARD"]
        );
        assert!(scan.dispatch.is_empty());
    }

    #[test]
    fn inventory_excludes_imports_comments_strings_and_test_code() {
        let syntax = syn::parse_file(
            r#"
            use codec::CMSG_IMPORT;
            // CMSG_COMMENT
            fn dispatch(msg: Message) {
                let _ = "CMSG_STRING";
                match msg {
                    ClientOpcodeMessage::CMSG_PING(ping) => send(ping),
                    ClientOpcodeMessage::CMSG_FORCE_RUN_SPEED_CHANGE_ACK(_) => {},
                    _ => {},
                }
            }
            #[cfg(test)] mod tests { fn test() { send(CMSG_TEST); } }
            #[cfg(test)] fn helper() { send(CMSG_HELPER); }
        "#,
        )
        .unwrap();
        let mut scan = SourceScan::default();
        scan.visit_file(&syntax);
        assert_eq!(
            scan.references
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["CMSG_FORCE_RUN_SPEED_CHANGE_ACK", "CMSG_PING"]
        );
        assert_eq!(scan.dispatch.len(), 2);
        assert_eq!(
            scan.ignored,
            BTreeSet::from(["CMSG_FORCE_RUN_SPEED_CHANGE_ACK".into()])
        );
    }

    #[test]
    fn inventory_matches_different_names_for_the_same_numeric_opcode() {
        let mut source = SourceScan::default();
        source.references.insert(
            "CMSG_MEETINGSTONE_STATUS_QUERY".into(),
            BTreeSet::from(["writer::meeting_stone".into()]),
        );
        let mut lyra = SourceScan::default();
        lyra.dispatch.insert(
            "CMSG_MEETINGSTONE_INFO".into(),
            BTreeSet::from(["dispatch_meeting_stone_action".into()]),
        );
        let found = row(
            "client_to_gateway",
            0x0296,
            &["CMSG_MEETINGSTONE_STATUS_QUERY".into()],
            &source,
            &lyra,
        );
        assert_eq!(found["lyracore_name"], "CMSG_MEETINGSTONE_INFO");
        assert_eq!(found["handling"], "dispatch_pattern_present");
    }

    #[test]
    fn inventory_finds_let_else_and_alternative_dispatch_patterns() {
        let syntax = syn::parse_file(r#"
            fn handle(msg: Message) {
                let (ClientOpcodeMessage::CMSG_REQUEST_PARTY_MEMBER_STATS(request), Some(guid)) =
                    (&msg, character) else { return };
                match msg {
                    ClientOpcodeMessage::CMSG_BUSY_TRADE | ClientOpcodeMessage::CMSG_IGNORE_TRADE => decline(),
                    _ => {},
                }
            }
        "#).unwrap();
        let mut scan = SourceScan::default();
        scan.visit_file(&syntax);
        assert_eq!(
            scan.dispatch.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "CMSG_BUSY_TRADE",
                "CMSG_IGNORE_TRADE",
                "CMSG_REQUEST_PARTY_MEMBER_STATS"
            ]
        );
    }
}
