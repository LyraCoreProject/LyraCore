use std::io::Write;

pub(crate) use lyracore_test_support::source_scan::in_comment as on_comment_line;

pub(crate) fn line_of(content: &str, byte_idx: usize) -> usize {
    content[..byte_idx].matches('\n').count() + 1
}

fn is_standalone_ident(content: &str, byte_idx: usize, name: &str) -> bool {
    let before_ok = content[..byte_idx]
        .chars()
        .next_back()
        .map(|c| !(c.is_alphanumeric() || c == '_' || c == '.'))
        .unwrap_or(true);
    let after_ok = content[byte_idx + name.len()..]
        .chars()
        .next()
        .map(|c| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(true);
    before_ok && after_ok
}

pub(crate) fn raw_table_reads(
    content: &str,
    accessors: &[&'static str],
    opens: impl Fn(&str, usize) -> bool,
) -> Vec<(usize, &'static str)> {
    let code = lyracore_test_support::source_scan::code_only(content);
    let content = code.as_str();
    let mut out = Vec::new();
    let mut handles: Vec<(String, &'static str)> = Vec::new();
    for accessor in accessors {
        let call = format!("{accessor}()");
        for (idx, _) in content.match_indices(&call) {
            if opens(content, idx + call.len()) {
                out.push((line_of(content, idx), *accessor));
            }
        }
        let bind = format!("ctx.db.{accessor}();");
        for (idx, _) in content.match_indices(&bind) {
            let line_start = content[..idx].rfind('\n').map(|i| i + 1).unwrap_or(0);
            let Some(head) = content[line_start..idx].trim_end().strip_suffix('=') else {
                continue;
            };
            let mut words = head.split_whitespace();
            if words.next() != Some("let") {
                continue;
            }
            let Some(name) = words.next_back() else {
                continue;
            };
            if !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            if !handles.iter().any(|(n, _)| n == name) {
                handles.push((name.to_string(), *accessor));
            }
        }
    }
    for (name, accessor) in &handles {
        for (idx, _) in content.match_indices(name.as_str()) {
            if !is_standalone_ident(content, idx, name) {
                continue;
            }
            if opens(content, idx + name.len()) {
                out.push((line_of(content, idx), *accessor));
            }
        }
    }
    out.sort();
    out
}

pub(crate) fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("module/ has a parent (the repo root)")
        .to_path_buf()
}

// A missing file is optional only when its entire owning Package or private tooling root is absent.
fn optional_owner(rel: &str) -> Option<String> {
    let mut segs = rel.split('/');
    match segs.next()? {
        "packages" => segs.next().map(|pkg| format!("packages/{pkg}")),
        root @ ("scripts" | "tools" | "adapters" | "client-patch") => Some(root.to_string()),
        _ => None,
    }
}

pub(crate) fn is_installed(rel: &str) -> bool {
    optional_owner(rel).is_none_or(|dir| repo_root().join(dir).is_dir())
}

pub(crate) fn read_scanned(rel: &str) -> Option<String> {
    match std::fs::read_to_string(repo_root().join(rel)) {
        Ok(src) => Some(src),
        Err(_) if !is_installed(rel) => {
            let dir = optional_owner(rel).unwrap_or_default();
            let _ = writeln!(
                std::io::stderr(),
                "note: skipping the scan of {rel} — {dir}/ is not installed in this checkout"
            );
            None
        }
        Err(e) => panic!(
            "cannot read {rel}: {e}. Its directory IS present in this checkout, so this is a path \
             typo, not an uninstalled drop-in — a scan that cannot find its target has lost its \
             pin, not passed it. Fix the path (or, if the file really moved, move the pin with it)."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_table_reads_ignores_comments_and_literals_but_keeps_live_reads() {
        let src = r###"fn f() {
    assert!(!tick.contains("game_world_entity().iter()"));
    /* game_world_entity().iter(); /* nested */ */
    let example = r#"game_world_entity().iter()"#;
    ctx.db.game_world_entity().iter();
    #[cfg(test)] { ctx.db.game_world_entity().iter(); }
}"###;
        let found = raw_table_reads(src, &["game_world_entity"], |content, byte_idx| {
            content[byte_idx..].starts_with(".iter()")
        });
        assert_eq!(found, [(5, "game_world_entity")]);
    }

    #[test]
    fn optional_owner_scopes_a_package_to_its_own_directory() {
        assert_eq!(
            optional_owner("packages/sample_package/src/mod.rs").as_deref(),
            Some("packages/sample_package"),
            "a package's absence is per-package: `packages/` itself carries a checked-in README, so \
             keying the skip on `packages/` would wave through a typo'd file inside an INSTALLED \
             package"
        );
        assert_eq!(
            optional_owner("scripts/publish-module.sh").as_deref(),
            Some("scripts")
        );
        assert_eq!(optional_owner("module/src/world.rs"), None);
        assert_eq!(optional_owner("native-host/src/mod.rs"), None);
        assert_eq!(optional_owner("crates/lyracore-shared/src/lib.rs"), None);
    }

    #[test]
    fn read_scanned_reads_a_shipped_file() {
        let src = read_scanned("module/src/test_scan.rs")
            .expect("module/ is never optional, so this can never be skipped");
        assert_eq!(
            src,
            std::fs::read_to_string(repo_root().join("module/src/test_scan.rs")).unwrap()
        );
    }

    #[test]
    #[should_panic(expected = "path typo")]
    fn read_scanned_panics_on_a_missing_file_whose_directory_is_installed() {
        let _ = read_scanned("module/src/no-such-file-exists.rs");
    }

    #[test]
    fn read_scanned_skips_a_package_that_is_not_installed() {
        assert!(
            read_scanned("packages/definitely-not-installed/src/mod.rs").is_none(),
            "an uninstalled drop-in package must be skipped, not panicked on — a checkout with no \
             packages is a designed state (module/build.rs discovers them dynamically)"
        );
    }
}
