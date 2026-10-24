//! Test discovery records source presence, not execution or passing results.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::Path;
use syn::visit::{self, Visit};

pub fn inventory(root: &Path) -> Result<Value> {
    let mut behaviors: Value = serde_json::from_str(include_str!("behaviors.json"))?;
    let rows = behaviors.as_array_mut().context("behavior list")?;
    let mut ids = BTreeSet::new();
    for behavior in rows {
        let id = behavior["id"].as_str().context("behavior id")?;
        if !ids.insert(id.to_owned()) {
            bail!("duplicate behavior id: {id}");
        }
        for test in behavior["tests"].as_array_mut().context("behavior tests")? {
            let path = test["path"].as_str().context("test path")?;
            let name = test["function"].as_str().context("test function")?;
            let source = std::fs::read_to_string(root.join(path))
                .with_context(|| format!("read behavior evidence {path}"))?;
            let ignored = test_evidence(&source, name)
                .with_context(|| format!("behavior evidence {path}::{name}"))?;
            test["source_present"] = json!(true);
            test["ignored_by_default"] = json!(ignored);
            test["execution"] = json!("not_run_by_inventory");
        }
    }
    Ok(behaviors)
}

pub fn matching_behaviors(message: &Value, behaviors: &Value) -> Value {
    let field = if message["direction"] == "client_to_module" {
        "requests"
    } else {
        "replies"
    };
    json!(behaviors
        .as_array()
        .into_iter()
        .flatten()
        .filter(|behavior| behavior[field]
            .as_array()
            .is_some_and(|opcodes| opcodes.contains(&message["opcode"])))
        .map(|behavior| &behavior["id"])
        .collect::<Vec<_>>())
}

fn test_evidence(source: &str, name: &str) -> Result<bool> {
    struct Tests<'a> {
        name: &'a str,
        matches: Vec<bool>,
    }
    impl<'ast> Visit<'ast> for Tests<'_> {
        fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
            let has_attribute = |name: &str| {
                item.attrs.iter().any(|attribute| {
                    attribute
                        .path()
                        .segments
                        .last()
                        .is_some_and(|segment| segment.ident == name)
                })
            };
            if item.sig.ident == self.name && has_attribute("test") {
                self.matches.push(has_attribute("ignore"));
            }
            visit::visit_item_fn(self, item);
        }
    }
    let mut tests = Tests {
        name,
        matches: Vec::new(),
    };
    tests.visit_file(&syn::parse_file(source)?);
    match tests.matches.as_slice() {
        [ignored] => Ok(*ignored),
        [] => bail!("named test is absent: {name}"),
        _ => bail!("named test is ambiguous: {name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_requires_a_test_declaration() {
        let source = r##"
            // #[test] fn missing() {}
            fn ordinary() { let _ = "#[test] fn missing() {}"; }
            #[cfg(test)] mod tests {
                #[test] fn scenario() {}
                #[tokio::test] #[ignore = "private standalone"] async fn durable() {}
            }
        "##;
        assert!(test_evidence(source, "missing").is_err());
        assert!(test_evidence(source, "ordinary").is_err());
        assert!(!test_evidence(source, "scenario").unwrap());
        assert!(test_evidence(source, "durable").unwrap());
    }

    #[test]
    fn evidence_refuses_ambiguous_test_names() {
        let source = "mod a { #[test] fn same() {} } mod b { #[test] fn same() {} }";
        assert!(test_evidence(source, "same").is_err());
    }

    #[test]
    fn behavior_links_respect_opcode_direction() {
        let behaviors = json!([
            {"id": "request", "requests": ["0x0040"], "replies": []},
            {"id": "reply", "requests": [], "replies": ["0x0040"]}
        ]);
        assert_eq!(
            matching_behaviors(
                &json!({"direction": "module_to_client", "opcode": "0x0040"}),
                &behaviors
            ),
            json!(["reply"])
        );
        assert_eq!(
            matching_behaviors(
                &json!({"direction": "client_to_module", "opcode": "0xFFFF"}),
                &behaviors
            ),
            json!([])
        );
    }
}
