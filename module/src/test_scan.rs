//! Shared source-scan primitives for this crate's `#[cfg(test)]` tripwires. Test-only (there is no
//! `ReducerContext` harness in this crate by design — playbook §7 — so a scan of the reducer's own
//! source text is how a chokepoint's PRESENCE gets pinned; the SENSE of a decision should be pulled
//! into a pure function and asserted on directly instead, wherever that is reachable — see
//! `helpers::gate_in_transit` for the pattern).
//!
//! Issue #64: this used to be six near-identical copies (`auth.rs`, `chat.rs`, `group.rs`,
//! `instance.rs`, `world.rs`, `transfer.rs`), and they had already drifted — two of them stripped a
//! `// trailing comment` off an otherwise-live line, the other four only dropped a line that was
//! ENTIRELY a comment. A needle planted in a trailing comment (`let _ = ctx; // the real call used
//! to be here`) satisfied every `.contains()` scan built on the weak four, while doing nothing. One
//! implementation, hardened, used everywhere in this crate.

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
/// private-only tooling trees below, which are optional WHOLESALE. In a maintainer's own checkout
/// `packages/` itself is always there (checked in, with its own README); on the public mirror
/// `packages/` is absent entirely, same as those four trees, and every path under it still resolves
/// correctly here because it maps to its OWN subpackage dir, which is then just as absent as the
/// parent it lives under.
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

/// Emit a one-line note about a skipped scan.
///
/// Deliberately NOT one of the std print macros. `spacetime generate` — which preflight runs to
/// extract the module schema — text-scans every file under `module/src/` for those macro names and
/// fails the build on a hit. That scan understands nothing: not `#[cfg]` (so the notes here and in
/// `publish_safety` tripped it even though both modules are `#[cfg(test)]` and reach no wasm), and
/// not comments (so naming the macros in THIS doc comment tripped it too — don't). The stderr
/// variant is no escape either; the scan matches it twice over. Writing to the stream directly says
/// the same thing and is invisible to a scan looking for macro names.
///
/// stderr, not stdout, because the test harness does not capture it — so these notes surface
/// without `--nocapture`, which is the whole point of emitting them.
pub(crate) fn note(message: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "{message}");
}

/// Read a repo-relative source file for a scan.
///
/// * `Some(contents)` — scan it, exactly as an `include_str!` would have.
/// * `None` — the file's whole optional directory is not installed in this checkout; a one-line
///   note is emitted so a skip is never silent in the test output.
///
/// Panics if the file is missing while its directory IS installed: that is the path-typo case, and
/// a scan that cannot find its target has lost its pin, not passed it.
pub(crate) fn read_scanned(rel: &str) -> Option<String> {
    match std::fs::read_to_string(repo_root().join(rel)) {
        Ok(src) => Some(src),
        Err(_) if !is_installed(rel) => {
            let dir = optional_owner(rel).unwrap_or_default();
            note(&format!(
                "note: skipping the scan of {rel} — {dir}/ is not installed in this checkout"
            ));
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

    /// The fixture the trailing-comment defeat looked like for real (round-1 review, #59): a `let
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
