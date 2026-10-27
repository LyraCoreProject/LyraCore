use anyhow::{anyhow, bail, Context, Result};

#[derive(Debug)]
pub(crate) struct Edit {
    pub(crate) source: String,
    pub(crate) path: String,
    pub(crate) anchor: Anchor,
    pub(crate) insert: String,
}

#[derive(Debug)]
pub(crate) enum Anchor {
    Before(String),
    After(String),
    Replace(String),
}

impl Anchor {
    pub(crate) fn text(&self) -> &str {
        match self {
            Anchor::Before(s) | Anchor::After(s) | Anchor::Replace(s) => s,
        }
    }

    fn tag(&self) -> u8 {
        match self {
            Anchor::Before(_) => 0,
            Anchor::After(_) => 1,
            Anchor::Replace(_) => 2,
        }
    }
}

pub(crate) fn parse(source: &str, json: &str) -> Result<Vec<Edit>> {
    let value: serde_json::Value = serde_json::from_str(json)
        .with_context(|| format!("{source}: ui-transforms.json is not valid JSON"))?;
    let entries = value
        .as_array()
        .ok_or_else(|| anyhow!("{source}: ui-transforms.json must be a JSON array"))?;

    let mut edits = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let obj = entry.as_object().ok_or_else(|| {
            anyhow!("{source}: ui-transforms.json entry {index} is not a JSON object")
        })?;

        let raw_path = obj
            .get("path")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow!("{source}: ui-transforms.json entry {index} has no \"path\""))?;
        let path = raw_path.replace('/', "\\");
        validate_path(source, &path)?;

        let insert = obj
            .get("insert")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow!("{source}: {path}: has no \"insert\""))?
            .to_string();

        let anchor_keys: Vec<(&str, &serde_json::Value)> = ["before", "after", "replace"]
            .into_iter()
            .filter_map(|key| obj.get(key).map(|v| (key, v)))
            .collect();
        if anchor_keys.len() != 1 {
            let found = anchor_keys.len();
            bail!("{source}: {path}: must set exactly one of before/after/replace, found {found}");
        }
        let (kind, raw_anchor) = anchor_keys[0];
        let text = raw_anchor
            .as_str()
            .ok_or_else(|| anyhow!("{source}: {path}: \"{kind}\" anchor must be a string"))?;
        if text.is_empty() {
            bail!("{source}: {path}: \"{kind}\" anchor is empty");
        }
        let anchor = match kind {
            "before" => Anchor::Before(text.to_string()),
            "after" => Anchor::After(text.to_string()),
            "replace" => Anchor::Replace(text.to_string()),
            _ => unreachable!("anchor_keys only ever holds before/after/replace"),
        };

        edits.push(Edit {
            source: source.to_string(),
            path,
            anchor,
            insert,
        });
    }
    Ok(edits)
}

fn validate_path(source: &str, path: &str) -> Result<()> {
    let lower = path.to_ascii_lowercase();
    if !lower.starts_with(r"interface\framexml\") && !lower.starts_with(r"interface\gluexml\") {
        bail!("{source}: {path} is outside Interface\\FrameXML\\ and Interface\\GlueXML\\");
    }
    if !(lower.ends_with(".lua") || lower.ends_with(".xml") || lower.ends_with(".toc")) {
        bail!("{source}: {path} must end in .lua, .xml or .toc");
    }
    Ok(())
}

// Resolve every span against the original baseline before applying edits.
pub(crate) fn compose(path: &str, baseline: &str, edits: &[&Edit]) -> Result<String> {
    struct Located<'a> {
        edit: &'a Edit,
        start: usize,
        end: usize,
    }

    let mut located = Vec::with_capacity(edits.len());
    for edit in edits {
        let anchor = edit.anchor.text();
        let count = baseline.matches(anchor).count();
        if count == 0 {
            let source = &edit.source;
            bail!("{source}: {path}: anchor not found: {anchor:?}");
        }
        if count > 1 {
            let source = &edit.source;
            bail!("{source}: {path}: ambiguous anchor (matches {count} times): {anchor:?}");
        }
        let start = baseline
            .find(anchor)
            .expect("matches() counted exactly one occurrence above");
        located.push(Located {
            edit,
            start,
            end: start + anchor.len(),
        });
    }

    for i in 0..located.len() {
        for j in (i + 1)..located.len() {
            let (a, b) = (&located[i], &located[j]);
            if a.start < b.end && b.start < a.end {
                let (source_a, source_b) = (&a.edit.source, &b.edit.source);
                let (anchor_a, anchor_b) = (a.edit.anchor.text(), b.edit.anchor.text());
                bail!(
                    "{source_a} and {source_b}: {path}: overlapping edits at anchors {anchor_a:?} and {anchor_b:?}"
                );
            }
        }
    }

    located.sort_by_key(|l| l.start);

    let mut out = String::with_capacity(baseline.len());
    let mut cursor = 0;
    for l in &located {
        out.push_str(&baseline[cursor..l.start]);
        match l.edit.anchor {
            Anchor::Before(_) => {
                out.push_str(&l.edit.insert);
                out.push_str(&baseline[l.start..l.end]);
            }
            Anchor::After(_) => {
                out.push_str(&baseline[l.start..l.end]);
                out.push_str(&l.edit.insert);
            }
            Anchor::Replace(_) => {
                out.push_str(&l.edit.insert);
            }
        }
        cursor = l.end;
    }
    out.push_str(&baseline[cursor..]);
    Ok(out)
}

pub(crate) fn record_header(path: &str, baseline_hash: &str, transforms_hash: &str) -> String {
    let body = format!(
        "Generated by LyraCore from the operator's client. baseline={baseline_hash} transforms={transforms_hash}"
    );
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".lua") {
        format!("-- {body}\n")
    } else if lower.ends_with(".toc") {
        format!("# {body}\n")
    } else if lower.ends_with(".xml") {
        format!("<!-- {body} -->\n")
    } else {
        unreachable!("record_header requires a path already validated by parse: {path}")
    }
}

// Domain separation prevents collisions with a future canonical rendering.
const TRANSFORMS_HASH_DOMAIN: &[u8] = b"lyracore-ui-transform-v1";

pub(crate) fn transforms_hash(edits: &[&Edit]) -> String {
    const FIELD_SEP: char = '\u{1}';

    let mut rendered: Vec<String> = edits
        .iter()
        .map(|e| {
            format!(
                "{}{FIELD_SEP}{}{FIELD_SEP}{}{FIELD_SEP}{}",
                e.source,
                e.anchor.tag(),
                e.anchor.text(),
                e.insert
            )
        })
        .collect();
    rendered.sort();

    let mut hasher = blake3::Hasher::new();
    hasher.update(TRANSFORMS_HASH_DOMAIN);
    for line in &rendered {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    hasher.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const README_EXAMPLE: &str = r#"[
  { "path": "Interface/FrameXML/LootFrame.lua",
    "after": "function LootFrame_OnLoad()",
    "insert": "\tPkgLoot_OnLoad();\n" },
  { "path": "Interface/FrameXML/FrameXML.toc",
    "before": "LootFrame.xml",
    "insert": "PkgLoot.lua\nPkgLoot.xml\n" },
  { "path": "Interface/GlueXML/GlueXML.toc",
    "replace": "AccountLogin.xml",
    "insert": "AccountLogin.xml\nPkgGlue.lua\n" }
]"#;

    #[test]
    fn parse_accepts_the_readme_example() {
        let edits = parse("loot", README_EXAMPLE).expect("valid document");
        assert_eq!(edits.len(), 3);
        assert_eq!(edits[0].path, r"Interface\FrameXML\LootFrame.lua");
        assert_eq!(edits[0].anchor.text(), "function LootFrame_OnLoad()");
        assert_eq!(edits[0].insert, "\tPkgLoot_OnLoad();\n");
        assert!(matches!(edits[1].anchor, Anchor::Before(_)));
        assert!(matches!(edits[2].anchor, Anchor::Replace(_)));
        assert!(edits.iter().all(|e| e.source == "loot"));
    }

    #[test]
    fn parse_rejects_a_non_array_document() {
        let err = parse("loot", r#"{"path": "x"}"#).unwrap_err().to_string();
        assert!(err.contains("loot"), "{err}");
        assert!(err.contains("JSON array"), "{err}");
    }

    #[test]
    fn parse_rejects_an_entry_that_is_not_an_object() {
        let err = parse("loot", r#"["not an object"]"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("loot"), "{err}");
        assert!(err.contains("entry 0"), "{err}");
    }

    #[test]
    fn parse_rejects_zero_anchor_keys() {
        let json = r#"[{ "path": "Interface/FrameXML/LootFrame.lua", "insert": "x" }]"#;
        let err = parse("loot", json).unwrap_err().to_string();
        assert!(err.contains("loot"), "{err}");
        assert!(err.contains("exactly one of before/after/replace"), "{err}");
    }

    #[test]
    fn parse_rejects_two_anchor_keys() {
        let json = r#"[{ "path": "Interface/FrameXML/LootFrame.lua",
            "before": "a", "after": "b", "insert": "x" }]"#;
        let err = parse("loot", json).unwrap_err().to_string();
        assert!(err.contains("exactly one of before/after/replace"), "{err}");
    }

    #[test]
    fn parse_rejects_a_missing_path() {
        let json = r#"[{ "after": "a", "insert": "x" }]"#;
        let err = parse("loot", json).unwrap_err().to_string();
        assert!(err.contains("loot"), "{err}");
        assert!(err.contains("\"path\""), "{err}");
    }

    #[test]
    fn parse_rejects_a_missing_insert() {
        let json = r#"[{ "path": "Interface/FrameXML/LootFrame.lua", "after": "a" }]"#;
        let err = parse("loot", json).unwrap_err().to_string();
        assert!(err.contains("loot"), "{err}");
        assert!(err.contains("\"insert\""), "{err}");
    }

    #[test]
    fn parse_rejects_an_empty_anchor() {
        let json =
            r#"[{ "path": "Interface/FrameXML/LootFrame.lua", "after": "", "insert": "x" }]"#;
        let err = parse("loot", json).unwrap_err().to_string();
        assert!(err.contains("empty"), "{err}");
    }

    #[test]
    fn parse_rejects_a_path_outside_framexml_and_gluexml() {
        let json = r#"[{ "path": "Interface/AddOns/Foo/Foo.lua", "after": "a", "insert": "x" }]"#;
        let err = parse("loot", json).unwrap_err().to_string();
        assert!(err.contains("loot"), "{err}");
        assert!(err.contains("outside"), "{err}");
    }

    #[test]
    fn parse_rejects_an_unsupported_extension() {
        let json =
            r#"[{ "path": "Interface/FrameXML/LootFrame.blp", "after": "a", "insert": "x" }]"#;
        let err = parse("loot", json).unwrap_err().to_string();
        assert!(err.contains(".lua, .xml or .toc"), "{err}");
    }

    #[test]
    fn parse_normalises_forward_slashes_and_accepts_mixed_case_root() {
        let json =
            r#"[{ "path": "interface/FRAMEXML/LootFrame.lua", "after": "a", "insert": "x" }]"#;
        let edits = parse("loot", json).expect("mixed-case root under FrameXML is valid");
        assert_eq!(edits[0].path, "interface\\FRAMEXML\\LootFrame.lua");
    }

    #[test]
    fn compose_two_disjoint_edits_are_order_independent() {
        let baseline = "function A() end\nfunction B() end\n";
        let a = Edit {
            source: "pkg-a".into(),
            path: "p".into(),
            anchor: Anchor::After("function A() end".into()),
            insert: "-- a\n".into(),
        };
        let b = Edit {
            source: "pkg-b".into(),
            path: "p".into(),
            anchor: Anchor::Before("function B() end".into()),
            insert: "-- b\n".into(),
        };

        let forward = compose("p", baseline, &[&a, &b]).unwrap();
        let backward = compose("p", baseline, &[&b, &a]).unwrap();
        assert_eq!(forward, backward);
        assert_eq!(forward, "function A() end-- a\n\n-- b\nfunction B() end\n");
    }

    #[test]
    fn compose_replace_substitutes_only_the_anchor() {
        let baseline = "AccountLogin.xml\nOtherFile.xml\n";
        let edit = Edit {
            source: "glue".into(),
            path: "p".into(),
            anchor: Anchor::Replace("AccountLogin.xml".into()),
            insert: "AccountLogin.xml\nPkgGlue.lua\n".into(),
        };
        let out = compose("p", baseline, &[&edit]).unwrap();
        assert_eq!(out, "AccountLogin.xml\nPkgGlue.lua\n\nOtherFile.xml\n");
    }

    #[test]
    fn compose_refuses_a_missing_anchor_naming_package_and_path() {
        let edit = Edit {
            source: "pkg-a".into(),
            path: "Interface\\FrameXML\\LootFrame.lua".into(),
            anchor: Anchor::After("no such text".into()),
            insert: "x".into(),
        };
        let path = edit.path.clone();
        let err = compose(&path, "nothing to see here", &[&edit])
            .unwrap_err()
            .to_string();
        assert!(err.contains("pkg-a"), "{err}");
        assert!(err.contains("LootFrame.lua"), "{err}");
        assert!(err.contains("not found"), "{err}");
    }

    #[test]
    fn compose_refuses_an_ambiguous_anchor_naming_package_and_path() {
        let edit = Edit {
            source: "pkg-a".into(),
            path: "p".into(),
            anchor: Anchor::After("dup".into()),
            insert: "x".into(),
        };
        let err = compose("p", "dup dup", &[&edit]).unwrap_err().to_string();
        assert!(
            err.starts_with("pkg-a: p: ambiguous anchor (matches 2 times)"),
            "{err}"
        );
    }

    #[test]
    fn compose_refuses_overlapping_edits_naming_both_packages() {
        let a = Edit {
            source: "pkg-a".into(),
            path: "p".into(),
            anchor: Anchor::After("function A".into()),
            insert: "x".into(),
        };
        let b = Edit {
            source: "pkg-b".into(),
            path: "p".into(),
            anchor: Anchor::Before("A() end".into()),
            insert: "y".into(),
        };
        let err = compose("p", "function A() end", &[&a, &b])
            .unwrap_err()
            .to_string();
        assert!(err.contains("pkg-a"), "{err}");
        assert!(err.contains("pkg-b"), "{err}");
        assert!(err.contains("overlap"), "{err}");
    }

    #[test]
    fn compose_refuses_identical_anchors_as_an_overlap() {
        let a = Edit {
            source: "pkg-a".into(),
            path: "p".into(),
            anchor: Anchor::After("same".into()),
            insert: "x".into(),
        };
        let b = Edit {
            source: "pkg-b".into(),
            path: "p".into(),
            anchor: Anchor::After("same".into()),
            insert: "y".into(),
        };
        let err = compose("p", "same text here", &[&a, &b])
            .unwrap_err()
            .to_string();
        assert!(err.contains("overlap"), "{err}");
    }

    #[test]
    fn record_header_uses_each_files_own_comment_syntax() {
        assert_eq!(
            record_header("Interface\\FrameXML\\Loot.lua", "abc", "def"),
            "-- Generated by LyraCore from the operator's client. baseline=abc transforms=def\n"
        );
        assert_eq!(
            record_header("Interface\\FrameXML\\FrameXML.toc", "abc", "def"),
            "# Generated by LyraCore from the operator's client. baseline=abc transforms=def\n"
        );
        assert_eq!(
            record_header("Interface\\GlueXML\\Glue.xml", "abc", "def"),
            "<!-- Generated by LyraCore from the operator's client. baseline=abc transforms=def -->\n"
        );
    }

    #[test]
    fn transforms_hash_is_independent_of_input_order() {
        let a = Edit {
            source: "pkg-a".into(),
            path: "p".into(),
            anchor: Anchor::After("x".into()),
            insert: "1".into(),
        };
        let b = Edit {
            source: "pkg-b".into(),
            path: "p".into(),
            anchor: Anchor::Before("y".into()),
            insert: "2".into(),
        };
        assert_eq!(transforms_hash(&[&a, &b]), transforms_hash(&[&b, &a]));
    }

    #[test]
    fn transforms_hash_changes_when_an_edit_changes() {
        let a = Edit {
            source: "pkg-a".into(),
            path: "p".into(),
            anchor: Anchor::After("x".into()),
            insert: "1".into(),
        };
        let a_changed = Edit {
            insert: "2".into(),
            ..clone_edit(&a)
        };
        assert_ne!(transforms_hash(&[&a]), transforms_hash(&[&a_changed]));
    }

    fn clone_edit(e: &Edit) -> Edit {
        Edit {
            source: e.source.clone(),
            path: e.path.clone(),
            anchor: match &e.anchor {
                Anchor::Before(s) => Anchor::Before(s.clone()),
                Anchor::After(s) => Anchor::After(s.clone()),
                Anchor::Replace(s) => Anchor::Replace(s.clone()),
            },
            insert: e.insert.clone(),
        }
    }
}
