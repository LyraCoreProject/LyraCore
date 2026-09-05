//! Shared source-scan primitives for this crate's `#[cfg(test)]` tripwires. Test-only (there is no
//! `ReducerContext` harness in this crate by design — playbook §7 — so a scan of the reducer's own
//! source text is how a chokepoint's PRESENCE gets pinned; the SENSE of a decision should be pulled
//! into a pure function and asserted on directly instead, wherever that is reachable — see
//! `helpers::gate_in_transit` for the pattern).
//!
//! This used to be six near-identical copies (`auth.rs`, `chat.rs`, `group.rs`,
//! `instance.rs`, `world.rs`, `transfer.rs`), and they had already drifted — two of them stripped a
//! `// trailing comment` off an otherwise-live line, the other four only dropped a line that was
//! ENTIRELY a comment. A needle planted in a trailing comment (`let _ = ctx; // the real call used
//! to be here`) satisfied every `.contains()` scan built on the weak four, while doing nothing. One
//! implementation, hardened, used everywhere in this crate.

use std::io::Write;

/// Isolate a fn (or struct/const/etc.) body by brace-matching from the first byte offset where
/// `signature` appears. Panics loudly — never silently matches nothing — if the signature or a
/// balanced `{...}` cannot be found, because a scan that can't find its target has lost its
/// pin, not passed it.
pub(crate) fn body_of(src: &str, signature: &str) -> String {
    let start = src
        .find(signature)
        .unwrap_or_else(|| panic!("`{signature}` no longer exists in this source"));
    let rest = &src[start..];
    let open = rest.find('{').expect("fn has a body");
    let mut depth = 0i32;
    for (i, c) in rest[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return rest[open..=open + i].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("unterminated body for `{signature}`");
}

/// Strip a Rust line comment from `line`, respecting simple double-quoted string literals so a
/// `//` inside one (an error message containing a URL, say) is never mistaken for a comment start.
/// Does not understand raw strings or block comments — nothing this crate scans uses either, and a
/// miss here fails LOUD (the caller's assertion breaks), never silently.
fn strip_line_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_string => i += 1, // skip whatever the backslash escapes, incl. `\"`
            b'"' => in_string = !in_string,
            b'/' if !in_string && bytes.get(i + 1) == Some(&b'/') => return &line[..i],
            _ => {}
        }
        i += 1;
    }
    line
}

/// [`body_of`] with every comment gone — a whole comment-only line dropped entirely, a trailing
/// comment on an otherwise-live line truncated at its `//`. Never `.contains()` a raw `body_of`
/// result; always go through this (or [`shape_of`], for a still-stronger equality check).
pub(crate) fn code_of(src: &str, signature: &str) -> String {
    body_of(src, signature)
        .lines()
        .map(strip_line_comment)
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// [`code_of`] with every whitespace run collapsed to one space, so a body can be compared for
/// EQUALITY (or an exact leading slice, for a larger function) instead of `.contains()`. Equality
/// is what actually distinguishes "this body IS exactly this" from "this text appears somewhere in
/// this body" — the distinction a `.contains()` scan can never make, and the one every comment- or
/// dead-branch-based defeat in this codebase's history has slipped through.
pub(crate) fn shape_of(src: &str, signature: &str) -> String {
    code_of(src, signature)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

// ================================================================================================
//  Shared engine for a "raw table read outside its chokepoint" tripwire.
// ================================================================================================
//
// `partition_discipline_tripwire::raw_scans` (spatial whole-table `.iter()`/`.count()`) and
// `character_fence_tripwire::raw_lookups` (raw `game_character` point lookups) used to be ~70-line
// near-clones of this — identical bound-handle walk-back, handle dedup and comment-line filtering
// — and had already drifted from each other in small ways neither review caught. The exact drift
// pattern consolidated once already, one layer up (`body_of`/`code_of`, above). One
// engine now; the two callers differ only in which accessor(s) they watch and what counts as
// "opens a read" they care about.

/// `byte_idx` sits on a `//` line, i.e. it is prose rather than code.
pub(crate) fn on_comment_line(content: &str, byte_idx: usize) -> bool {
    let line_start = content[..byte_idx].rfind('\n').map(|i| i + 1).unwrap_or(0);
    content[line_start..byte_idx].trim_start().starts_with("//")
}

/// `byte_idx` sits inside an ordinary double-quoted string literal on its line. Source scanners
/// must not treat assertion needles as live table reads.
pub(crate) fn in_string_literal(content: &str, byte_idx: usize) -> bool {
    let line_start = content[..byte_idx].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let bytes = &content.as_bytes()[line_start..byte_idx];
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_string => i += 1,
            b'"' => in_string = !in_string,
            _ => {}
        }
        i += 1;
    }
    in_string
}

/// 1-based line number of `byte_idx`.
pub(crate) fn line_of(content: &str, byte_idx: usize) -> usize {
    content[..byte_idx].matches('\n').count() + 1
}

/// `name` at `byte_idx` is a standalone identifier, not a substring of a longer one and not a
/// field access (`self.entities` is somebody else's business).
pub(crate) fn is_standalone_ident(content: &str, byte_idx: usize, name: &str) -> bool {
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

/// Every `(line, accessor)` in `content` where one of `accessors` is read through a call matching
/// `opens` — either straight off the accessor (`ctx.db.<accessor>().<opens-shaped tail>`) or off a
/// local handle bound from it one line earlier (`let entities = ctx.db.<accessor>();` ...
/// `entities.<...>`, this codebase's DOMINANT binding idiom — ~120 bindings vs ~18 inline calls, so
/// a scanner that only matched the inline form would wave through most of the real reads).
/// `opens(content, byte_idx)` decides whether the text starting right where the accessor call or
/// handle name ends is a read the caller cares about: `.iter()`/`.count()` for a whole-table scan,
/// `.guid().find(` for a raw point lookup — the only thing that differs between the two tripwires
/// built on this.
///
/// Ceiling (inherited by both callers): still a text scan — a handle passed to another fn, or an
/// accessor name assembled by a macro, defeats it. That is a deliberate trade, not an oversight: the
/// job is to make the LAZY path (copy the code next to you) fail, not to be a sound analysis.
pub(crate) fn raw_table_reads(
    content: &str,
    accessors: &[&'static str],
    opens: impl Fn(&str, usize) -> bool,
) -> Vec<(usize, &'static str)> {
    let mut out = Vec::new();
    // `(local handle name, accessor it was bound from)`, deduped by name so a file that binds
    // `entities` a dozen times doesn't count each downstream read a dozen times.
    let mut handles: Vec<(String, &'static str)> = Vec::new();
    for accessor in accessors {
        let call = format!("{accessor}()");
        for (idx, _) in content.match_indices(&call) {
            if on_comment_line(content, idx) || in_string_literal(content, idx) {
                continue;
            }
            if opens(content, idx + call.len()) {
                out.push((line_of(content, idx), *accessor));
            }
        }
        let bind = format!("ctx.db.{accessor}();");
        for (idx, _) in content.match_indices(&bind) {
            if on_comment_line(content, idx) || in_string_literal(content, idx) {
                continue;
            }
            // Walk back over `let [mut] NAME =` on the same line to name the handle.
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
            if on_comment_line(content, idx)
                || in_string_literal(content, idx)
                || !is_standalone_ident(content, idx, name)
            {
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

// ================================================================================================
//  Resolving a scanned file that may legitimately not be in this checkout.
// ================================================================================================
//
// Not every tree this crate's tripwires scan is present in every checkout, and that is a DESIGNED
// state, not a broken one:
//
// * `packages/<name>/` is an optional drop-in. `module/build.rs` discovers packages at build time
//   and compiles in whatever it finds; a checkout with none builds a perfectly good module.
// * `scripts/`, `tools/`, `adapters/` and `client-patch/` are private-only tooling. The public
//   mirror (LyraCoreProject/LyraCore) is an allowlist-filtered history that ships `module/`,
//   `gateway/`, `importer/` and `crates/` and none of those four — so a cold clone of the mirror
//   has to compile and test green without them.
//
// A `include_str!` on such a path is a COMPILE-TIME hard requirement, which is how the mirror's
// cold clone stopped building at all. These helpers move the requirement to run time with one
// exact rule, so a tripwire never loses its teeth to make a filtered checkout happy:
//
//   skip ONLY when the whole optional directory is absent — if the directory is there and the
//   named file is not, that is a PATH TYPO, and the tripwire must fail on it.

/// Concatenate every file in `module/src/debug/` (the `#[cfg(feature = "debug_reducers")]`
/// directory module split out of the former single `debug.rs`) into one blob, so a text-scan
/// tripwire that used to `include_str!("debug.rs")` keeps seeing every debug reducer regardless of
/// which of the seven files it landed in. Directory-read order (unspecified) is fine — every caller
/// only substring/signature-searches, never anchors on cross-file position or file boundaries.
pub(crate) fn debug_dir_src() -> String {
    let dir = repo_root().join("module/src/debug");
    let mut out = String::new();
    for entry in
        std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
    {
        let path = entry.expect("readable dir entry").path();
        if path.extension().map(|e| e == "rs").unwrap_or(false) {
            out.push_str(
                &std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display())),
            );
            out.push('\n');
        }
    }
    out
}

/// Every `.rs` file under `module/src`, as repo-relative paths, sorted. For a CRATE-WIDE scan — the
/// "this call exists in exactly one place" shape, which a per-file `include_str!` cannot express
/// (a new file is exactly where a second call site would appear).
pub(crate) fn module_sources() -> Vec<String> {
    fn walk(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<String>) {
        let entries =
            std::fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
        for entry in entries {
            let path = entry.expect("readable dir entry").path();
            if path.is_dir() {
                walk(&path, root, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let rel = path
                    .strip_prefix(root)
                    .expect("walked from the repo root")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push(rel);
            }
        }
    }
    let root = repo_root();
    let mut out = Vec::new();
    walk(&root.join("module/src"), &root, &mut out);
    out.sort();
    out
}

/// The repo root: `module/`'s parent.
pub(crate) fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("module/ has a parent (the repo root)")
        .to_path_buf()
}

/// The optional directory a repo-relative path belongs to, if any — the ONLY directory whose
/// absence licenses skipping that path.
///
/// A drop-in package is optional ONE PACKAGE AT A TIME — a missing `packages/playerbots/src/goals.rs`
/// inside an installed `packages/playerbots/` is a typo, not an uninstalled package — unlike the
/// private-only tooling trees below, which are optional WHOLESALE. `packages/` itself is always
/// there, in every checkout including the public mirror (it ships the reference Package,
/// `packages/example/`), so only an individual OTHER package — `packages/<pkg>` — can be absent;
/// every path under it still resolves correctly here because it maps to its own subpackage dir.
fn optional_owner(rel: &str) -> Option<String> {
    let mut segs = rel.split('/');
    match segs.next()? {
        "packages" => segs.next().map(|pkg| format!("packages/{pkg}")),
        root @ ("scripts" | "tools" | "adapters" | "client-patch") => Some(root.to_string()),
        _ => None,
    }
}

/// True when `rel`'s owning optional directory exists in this checkout — i.e. whatever lives under
/// it is expected to be there. A path with no optional owner is always expected.
pub(crate) fn is_installed(rel: &str) -> bool {
    optional_owner(rel).is_none_or(|dir| repo_root().join(dir).is_dir())
}

/// Read a repo-relative source file for a scan.
///
/// * `Some(contents)` — scan it, exactly as an `include_str!` would have.
/// * `None` — the file's whole optional directory is not installed in this checkout; a one-line
///   note is printed so a skip is never silent in the test output.
///
/// Panics if the file is missing while its directory IS installed: that is the path-typo case, and
/// a scan that cannot find its target has lost its pin, not passed it.
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

    /// The fixture the trailing-comment defeat looked like for real (round-1 review): a `let
    /// _ = ctx;` no-op with the real call demoted to a trailing comment. The weak four-file version
    /// of `code_of` (`.filter(|l| !l.trim_start().starts_with("//"))`) left that comment's text
    /// intact, so `.contains("bump_guid_high_water")` still passed. This must now be gone.
    #[test]
    fn code_of_strips_a_trailing_comment_not_just_a_leading_one() {
        let src = "fn f() {\n    let _ = 1;\n    let _ = ctx; // crate::auth::bump_guid_high_water(ctx, g);\n    // a whole-line comment naming it too: bump_guid_high_water\n}";
        let code = code_of(src, "fn f(");
        assert!(
            !code.contains("bump_guid_high_water"),
            "a needle sitting in a trailing OR a leading comment must not survive code_of. Got:\n{code}"
        );
        // The live code on the trailing-comment line must survive — only the comment is cut.
        assert!(
            code.contains("let _ = ctx;"),
            "code_of must not eat the live half of a line. Got:\n{code}"
        );
    }

    /// The other half of the same acceptance criterion: a `//` that is part of a STRING LITERAL —
    /// an error message quoting a URL, say — must not be mistaken for a comment start and truncate
    /// real code.
    #[test]
    fn code_of_does_not_treat_a_slash_slash_inside_a_string_literal_as_a_comment() {
        let src = r#"fn f() {
    Err("see https://example.com/docs for details")?;
}"#;
        let code = code_of(src, "fn f(");
        assert!(
            code.contains("https://example.com/docs"),
            "a `//` inside a string literal must survive code_of intact — it is not a comment. Got:\n{code}"
        );
    }

    /// A comment containing an escaped quote inside its OWN string-literal-shaped text must not
    /// desync the in-string tracker for the rest of the line — degenerate but cheap to guard.
    #[test]
    fn code_of_handles_an_escaped_quote_inside_a_string_literal() {
        let src = r#"fn f() {
    let s = "a \"quoted\" word // not a comment";
    let t = 1; // this IS a comment
}"#;
        let code = code_of(src, "fn f(");
        assert!(
            code.contains(r#"a \"quoted\" word // not a comment"#),
            "escaped quotes desynced the string tracker. Got:\n{code}"
        );
        assert!(
            !code.contains("this IS a comment"),
            "the real trailing comment on the next line must still be stripped. Got:\n{code}"
        );
    }

    #[test]
    fn raw_table_reads_ignores_a_table_scan_named_inside_a_string_literal() {
        let src = r#"fn f() {
    assert!(!tick.contains("game_world_entity().iter()"));
}"#;
        let found = raw_table_reads(src, &["game_world_entity"], |content, byte_idx| {
            content[byte_idx..].starts_with(".iter()")
        });
        assert!(
            found.is_empty(),
            "an assertion's forbidden-pattern needle is not a live table read: {found:?}"
        );
    }

    #[test]
    fn shape_of_collapses_whitespace_for_exact_equality() {
        let src = "fn f() {\n    let   x =\n        1;\n}";
        assert_eq!(shape_of(src, "fn f("), "{ let x = 1; }");
    }

    // ---- the optional-tree resolver ------------------------------------------------------------

    /// A drop-in package is optional one PACKAGE at a time; the private-only tooling trees are
    /// optional wholesale; everything the mirror ships is never optional.
    #[test]
    fn optional_owner_scopes_a_package_to_its_own_directory() {
        assert_eq!(
            optional_owner("packages/playerbots/src/mod.rs").as_deref(),
            Some("packages/playerbots"),
            "a package's absence is per-package: `packages/` itself carries a checked-in README, so \
             keying the skip on `packages/` would wave through a typo'd file inside an INSTALLED \
             package"
        );
        assert_eq!(
            optional_owner("scripts/publish-module.sh").as_deref(),
            Some("scripts")
        );
        assert_eq!(optional_owner("module/src/world.rs"), None);
        assert_eq!(optional_owner("gateway/src/main.rs"), None);
        assert_eq!(optional_owner("crates/lyracore-shared/src/lib.rs"), None);
    }

    /// A file the mirror ships always reads — no skip path exists for it at all.
    #[test]
    fn read_scanned_reads_a_shipped_file() {
        let src = read_scanned("module/src/test_scan.rs")
            .expect("module/ is never optional, so this can never be skipped");
        assert!(src.contains("fn read_scanned("));
    }

    /// The typo case the tripwires must keep catching: the directory is installed, the named file
    /// is not. Skipping here would let a whitelist entry rot into a pin over nothing.
    #[test]
    #[should_panic(expected = "path typo")]
    fn read_scanned_panics_on_a_missing_file_whose_directory_is_installed() {
        let _ = read_scanned("module/src/no-such-file-exists.rs");
    }

    /// The skip case: an uninstalled optional tree. `packages/definitely-not-installed/` cannot
    /// exist (a real package folder would have to be a valid Rust identifier and be checked in), so
    /// this exercises the branch a mirror cold clone takes without depending on which packages the
    /// running checkout happens to have.
    #[test]
    fn read_scanned_skips_a_package_that_is_not_installed() {
        assert!(
            read_scanned("packages/definitely-not-installed/src/mod.rs").is_none(),
            "an uninstalled drop-in package must be skipped, not panicked on — a checkout with no \
             packages is a designed state (module/build.rs discovers them dynamically)"
        );
    }
}
