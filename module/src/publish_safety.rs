//! Deploy-safety tripwire (issue #223): the repo's hardest rule — **never `spacetime publish -c`**
//! — had exactly ONE runtime enforcement (the sanctioned deploy script's argv flag-rejection loop)
//! and otherwise lived in prose. Prose does not fail a build. Nothing would have gone red if `-c`
//! were pasted onto the sanctioned deploy script's publish line, or onto any other script that
//! embeds a publish command.
//!
//! `-c` clears the database: every account, character, item and quest row is destroyed, and login
//! stays broken afterwards until the operator identity is re-claimed. It is the one unrecoverable
//! mistake available in this repo's tooling, so it gets a scan of its own.
//!
//! Two things are pinned here:
//!
//! 1. No shell or Python file under `scripts/`, `tools/`, `adapters/` or `importer/` names
//!    `spacetime publish` on a line that also carries a destructive flag.
//! 2. The one sanctioned deploy script still rejects flag-shaped
//!    arguments, and still passes both flags it exists to guarantee.
//!
//! Following `test_scan`'s principle: a scan that cannot find its target has lost its pin, not
//! passed it. Every scan here therefore carries a sanity FLOOR, and the line-level extractor is
//! separately proven against a synthetic fixture that contains a known-bad line — so this module
//! can never go green by matching nothing.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::test_scan::repo_root;

/// The literal we treat as "this line names a publish invocation".
///
/// Ceiling: a line that reaches the CLI some other way (`$SPACETIME publish`, `spacetime` with two
/// spaces before `publish`, an `xargs`-assembled command) is invisible to a text scan. The tripwire's
/// job is to make the LAZY path — copy the command from a neighbouring script, add a flag — fail,
/// not to be a sound analysis of shell.
const PUBLISH_NEEDLE: &str = "spacetime publish";

/// Flags that make `spacetime publish` data-destructive.
///
/// `spacetime publish --help` is not reachable from an offline test (and this module must never
/// shell out to the CLI), so the list is literal and deliberately over-broad rather than derived:
/// - `-c` is the short form documented all over this repo (`docs/danger-zones.md` and the
///   sanctioned deploy script's own header) as THE destructive wipe.
/// - `--clear-database` is its long form.
/// - `--delete-data` is the adjacent spelling the CLI has used across versions; scanned so a
///   version bump that renames the flag cannot quietly re-open the hole.
///
/// Short flags are matched as a CLUSTER (`-cy` is `-c -y` to any clap-based CLI), so a bundle
/// cannot smuggle the wipe past a scan looking for a bare `-c`.
const DESTRUCTIVE_LONG_FLAGS: &[&str] = &["--clear-database", "--delete-data"];

/// Strip a `#`-comment from one line of shell (or Python), respecting single- and double-quoted
/// strings so a `#` inside one survives.
///
/// `test_scan::strip_line_comment` understands Rust's `//` and is no use here. Two shell rules this
/// must get right, both load-bearing on the real files:
/// - `#` only opens a comment at the start of a WORD. `[ ${#dbs[@]} -eq 0 ]` in
///   the sanctioned deploy script is code, not a comment, and a naive scanner truncates that script
///   at line 30 — losing the publish line entirely and passing vacuously.
/// - inside `'...'` or `"..."` a `#` is literal, so a documented command in an `echo` keeps its text.
///
/// Backslash escapes the next byte everywhere except inside single quotes, matching sh. Python's
/// triple-quoted strings are not modelled; the worst case there is truncating a docstring early,
/// which can only remove text from the scan's view on lines that are documentation anyway.
fn strip_shell_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if quote != Some(b'\'') => i += 1, // skip whatever the backslash escapes
            c @ (b'"' | b'\'') => match quote {
                None => quote = Some(c),
                Some(q) if q == c => quote = None,
                _ => {}
            },
            b'#' if quote.is_none()
                && (i == 0 || bytes[i - 1].is_ascii_whitespace() || bytes[i - 1] == b';') =>
            {
                return &line[..i];
            }
            _ => {}
        }
        i += 1;
    }
    line
}

/// One whitespace-token off a command line is a destructive publish flag.
///
/// Surrounding quote/punctuation is trimmed first so `"-c"` and `-c;` are not a way through.
fn is_destructive_flag(raw: &str) -> bool {
    let token = raw.trim_matches(|c: char| c == '"' || c == '\'' || c == ';' || c == ',');
    if let Some(long) = token.strip_prefix("--") {
        let name = long.split('=').next().unwrap_or(long);
        return DESTRUCTIVE_LONG_FLAGS
            .iter()
            .any(|f| f.trim_start_matches('-') == name);
    }
    match token.strip_prefix('-') {
        // A short-flag cluster containing `c`. Over-broad ON PURPOSE: a future flag spelled with a
        // `c` would fail this test rather than slip past it, and a false RED on a deploy-safety
        // tripwire costs one comment, while a false GREEN costs the database.
        Some(short) => {
            !short.is_empty()
                && short.chars().all(|c| c.is_ascii_alphabetic())
                && short.contains('c')
        }
        None => false,
    }
}

/// Every 1-based line number in `src` whose code (comments stripped) names a publish invocation.
fn publish_lines(src: &str) -> Vec<usize> {
    src.lines()
        .enumerate()
        .filter(|(_, raw)| strip_shell_comment(raw).contains(PUBLISH_NEEDLE))
        .map(|(i, _)| i + 1)
        .collect()
}

/// Every `(line number, code)` in `src` where a publish invocation carries a destructive flag.
///
/// Deliberately does NOT try to tell an executed command from one quoted inside an `echo` or a
/// Python string — several files legitimately DOCUMENT the publish command that way
/// (`importer/scripts/import-world.sh` and other internal tooling). Telling
/// the two apart needs a shell parser and would be the fragile part of this test; instead the rule
/// is simply "no destructive flag next to a publish, anywhere, in any form". Documented commands
/// must not carry `-c` either — somebody will paste them.
fn destructive_publish_lines(src: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (i, raw) in src.lines().enumerate() {
        let code = strip_shell_comment(raw);
        let Some(pos) = code.find(PUBLISH_NEEDLE) else {
            continue;
        };
        let tail = &code[pos + PUBLISH_NEEDLE.len()..];
        if tail.split_whitespace().any(is_destructive_flag) {
            out.push((i + 1, code.trim().to_string()));
        }
    }
    out
}

/// `src` with comments gone and every whitespace run collapsed to one space, so a block of shell
/// can be compared for EQUALITY (or exact containment) instead of by loose `.contains()` on raw
/// text. Same rationale as `test_scan::shape_of`, which is for Rust bodies and cannot be reused:
/// equality is what distinguishes "this guard IS exactly this" from "these words appear somewhere".
fn shell_shape(src: &str) -> String {
    src.lines()
        .map(strip_shell_comment)
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The scan roots, each with its own file FLOOR: `(dir, minimum *.sh/*.py files)`.
///
/// `adapters/` joined the list with #246, when the wire harness moved to its own repository: the
/// live-test entrypoint that survived here (`adapters/lyracore/run-suite.sh`) shells out against a
/// running stack, which is exactly the shape of script this scan exists to police. `importer/`
/// joined it with #323, when the world-import ETL moved out of `scripts/` and into
/// `importer/scripts/` — it is the same seven scripts, and one of them DOCUMENTS a publish command
/// in its abort message. A root that is not scanned is a root where a `spacetime publish -c` can
/// sit unnoticed.
///
/// The floors are PER ROOT (they used to be one number, 25, across all four) so that one root
/// going blind can no longer be masked by the other three growing. They still sum to 25.
///
/// Three of these four roots are PRIVATE-ONLY: the public mirror's allowlist filter ships
/// `importer/` and none of `scripts/`, `tools/`, `adapters/`, so a cold clone of the mirror has to
/// test green with only the last one. A root that is ABSENT is therefore skipped (with a note);
/// a root that is PRESENT is scanned and held to its floor exactly as before. `importer/` is a
/// workspace member and always there, which is what keeps this scan from ever looking at nothing.
///
/// Ceiling: renaming a private-only root wholesale now goes quiet instead of red. Renaming a FILE
/// under one does not — `read_repo` still panics on a missing file inside a present root.
const SCAN_ROOTS: &[(&str, usize)] = &[
    ("scripts", 10),
    ("tools", 9),
    ("adapters", 1),
    ("importer", 5),
];

/// The scan roots this checkout actually has, with their `*.sh` / `*.py` files, sorted for a
/// deterministic report.
fn scanned_roots() -> Vec<(&'static str, Vec<PathBuf>)> {
    let root = repo_root();
    let mut out = Vec::new();
    for (dir, _) in SCAN_ROOTS {
        let path = root.join(dir);
        if !path.is_dir() {
            assert!(
                !crate::test_scan::is_installed(dir),
                "the destructive-publish scan cannot find {dir}/ — the scan root is wrong, so this \
                 test would pass by looking at nothing while a `spacetime publish -c` sat in the \
                 repo waiting to wipe every account and character row"
            );
            let _ = writeln!(
                std::io::stderr(),
                "note: skipping the destructive-publish scan of {dir}/ — it is not present in this \
                 checkout (the public mirror filters it out)"
            );
            continue;
        }
        let mut files = Vec::new();
        collect_scripts(&path, &mut files);
        files.sort();
        out.push((*dir, files));
    }
    out
}

/// Every scanned script across every present root, flattened.
fn scanned_scripts() -> Vec<PathBuf> {
    scanned_roots().into_iter().flat_map(|(_, f)| f).collect()
}

fn collect_scripts(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("readable {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("readable dir entry").path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if matches!(
            name,
            "target" | "node_modules" | ".venv" | "__pycache__" | ".git"
        ) {
            continue;
        }
        if path.is_dir() {
            collect_scripts(&path, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("sh") | Some("py")
        ) {
            out.push(path);
        }
    }
}

/// Read a pinned repo file, or `None` when its whole PRIVATE-ONLY tree is absent from this
/// checkout (a cold clone of the public mirror). A file missing from a tree that IS here still
/// panics: this file is the deploy path's only runtime guard against a database wipe, and if it
/// moved the pin must MOVE WITH IT rather than be deleted.
fn read_repo(rel: &str) -> Option<String> {
    crate::test_scan::read_scanned(rel)
}

/// The publish-module.sh flag-rejection loop, verbatim, whitespace-collapsed. Pinned by exact shape
/// rather than by `.contains("-*)")` so that rewording, re-scoping or gutting the guard fails here
/// instead of silently narrowing what it refuses. If you changed this block on purpose, update the
/// constant AND satisfy yourself that a `-c` argument still cannot reach `spacetime publish`.
const FLAG_REJECTION_LOOP: &str = "for db in \"${dbs[@]}\"; do \
     case \"$db\" in \
     -*) echo \"publish-module.sh takes database NAMES, not flags (got '$db'). Refusing.\" >&2; \
     exit 2 ;; \
     esac \
     done";

#[cfg(test)]
mod tests {
    use super::*;

    // ---- the shell comment stripper's own self-tests -------------------------------------------

    #[test]
    fn strip_shell_comment_drops_a_whole_line_comment() {
        assert_eq!(
            strip_shell_comment("# NEVER -c: spacetime publish -c wipes it"),
            ""
        );
    }

    #[test]
    fn strip_shell_comment_truncates_a_trailing_comment_and_keeps_the_live_half() {
        let line = "spacetime publish --yes lyracore   # not -c, ever";
        assert_eq!(
            strip_shell_comment(line).trim_end(),
            "spacetime publish --yes lyracore",
            "the live half of a line carrying a trailing comment must survive; eating it would \
             hide real publish invocations from the destructive-flag scan"
        );
    }

    #[test]
    fn strip_shell_comment_keeps_a_hash_inside_a_double_quoted_string() {
        let line = "echo \"pass #1 of 5 shards: spacetime publish -c\"";
        assert_eq!(
            strip_shell_comment(line),
            line,
            "a `#` inside a quoted string is literal text, not a comment. Truncating there would \
             blind the scan to a destructive publish sitting later on the same line"
        );
    }

    #[test]
    fn strip_shell_comment_keeps_a_hash_inside_single_quotes() {
        let line = "spacetime publish --build-options='--cfg feature#x' lyracore";
        assert_eq!(strip_shell_comment(line), line);
    }

    /// The sanctioned deploy script's line 30 is `[ ${#dbs[@]} -eq 0 ] && dbs=(lyracore)`. A stripper
    /// that treats any `#` as a comment truncates the file there and never sees the publish command
    /// four lines later — the exact way this scan could pass while enforcing nothing.
    #[test]
    fn strip_shell_comment_keeps_a_parameter_expansion_hash() {
        let line = "[ ${#dbs[@]} -eq 0 ] && dbs=(lyracore)";
        assert_eq!(
            strip_shell_comment(line),
            line,
            "`${{#var[@]}}` is an array-length expansion, not a comment"
        );
    }

    #[test]
    fn strip_shell_comment_handles_an_escaped_quote() {
        let line = "echo \"a \\\"quoted\\\" word\" # real comment";
        assert_eq!(
            strip_shell_comment(line).trim_end(),
            "echo \"a \\\"quoted\\\" word\"",
            "an escaped quote must not desync the quote tracker and swallow the trailing comment rule"
        );
    }

    // ---- the extractor, proven against a synthetic fixture -------------------------------------

    /// The guard against a scanner that matches nothing: run the real line-scanner over an inline
    /// fixture holding BOTH a safe publish and a destructive one, and require it to separate them.
    /// Without this, every assertion below could go green because the extractor was broken.
    #[test]
    fn the_line_scanner_flags_a_destructive_publish_and_spares_the_safe_one() {
        let fixture = "\
#!/usr/bin/env bash
# a comment mentioning spacetime publish -c must NOT count
echo \"docs say: spacetime publish -s local --yes lyracore\"
spacetime publish -s local -p module --build-options='--features=debug_reducers' --yes \"$db\"
spacetime publish -c -s local -p module lyracore
spacetime publish --clear-database lyracore
spacetime publish -cy lyracore
";
        let found = destructive_publish_lines(fixture);
        let lines: Vec<usize> = found.iter().map(|(l, _)| *l).collect();
        assert_eq!(
            lines,
            vec![5, 6, 7],
            "the destructive-publish extractor must flag the bare `-c`, the `--clear-database` long \
             form and the `-cy` short-flag bundle, and must flag NEITHER the safe wrapper \
             invocation NOR the two comment/echo mentions. Got: {found:?}"
        );
        assert_eq!(
            publish_lines(fixture),
            vec![3, 4, 5, 6, 7],
            "the publish-line counter must see every non-comment publish mention (its job is to \
             stop the real scan below from passing vacuously)"
        );
    }

    // ---- the real scan ------------------------------------------------------------------------

    /// Sanity FLOOR. A scan rooted at the wrong directory finds zero files, flags zero violations
    /// and reports success — so the floor is what makes a green result mean something.
    ///
    /// The floor was 40 until #246. It is 25 now, and the drop is a REAL loss of surface, not a
    /// relaxed standard: 47 of the files this used to scan were the wire harness's live-stack
    /// orchestrators under `tools/wire-client/`, and they are in another repository now. What is
    /// left (26 in `scripts/` + `tools/` + `importer/scripts/`, plus `adapters/lyracore/run-suite.sh`)
    /// is every script this repo still ships. #323 moved seven of those files from `scripts/` to
    /// `importer/scripts/` and added that root to the scan in the same change, so the count did not
    /// move — a relocation must never be allowed to shrink this number by leaving a root behind.
    /// Lower it again only alongside a comparable structural move, and never to make a red run
    /// green — the whole point of a floor is that it notices the scan going blind.
    ///
    /// The 25 is spread over `SCAN_ROOTS` per root now, so it is checked root by root: this is
    /// STRICTER than the old single sum (one root emptying itself can no longer be masked by
    /// another growing) and it is what lets a checkout without the private-only roots — the public
    /// mirror — still hold the roots it does have to their own floors.
    #[test]
    fn the_publish_scan_actually_reaches_the_repo_scripts() {
        let roots = scanned_roots();
        for (dir, files) in &roots {
            let floor = SCAN_ROOTS
                .iter()
                .find(|(d, _)| d == dir)
                .map(|(_, f)| *f)
                .expect("scanned_roots only yields SCAN_ROOTS entries");
            assert!(
                files.len() >= floor,
                "found only {} shell/python file(s) under {dir}/ — expected at least {floor}. The \
                 destructive-publish scan is looking at the wrong place, so its green result means \
                 nothing and a `spacetime publish -c` could reach the deploy path unchallenged",
                files.len()
            );
        }

        // The rest of this test bottoms out on `scripts/publish-module.sh`, the ONE sanctioned deploy path — private-only tooling that a filtered checkout does not have at all. mirror-check: allow-private-ref
        // When scripts/ IS here so is every other root (only the mirror filter removes them), so the
        // publish-site floor below still spans all four exactly as it did before.
        if !roots.iter().any(|(d, _)| *d == "scripts") {
            let _ = writeln!(
                std::io::stderr(),
                "note: skipping the publish-site floor — scripts/ is not present in this checkout"
            );
            return;
        }
        let files = scanned_scripts();

        let root = repo_root();
        let mut publish_sites = 0usize;
        let mut saw_publish_module_sh = false;
        for file in &files {
            let src = std::fs::read_to_string(file).unwrap_or_else(|e| {
                panic!("cannot read {}: {e}", file.display());
            });
            let hits = publish_lines(&src).len();
            publish_sites += hits;
            if hits > 0
                && file
                    .strip_prefix(&root)
                    .is_ok_and(|r| r == Path::new("scripts/publish-module.sh"))
            {
                saw_publish_module_sh = true;
            }
        }
        assert!(
            publish_sites >= 3,
            "found only {publish_sites} live `spacetime publish` occurrence(s) across scripts/ + \
             tools/ + adapters/ + importer/ — expected at least 3. Either the comment stripper is \
             eating real code or the \
             command spelling changed; either way the destructive-flag scan below is now guarding \
             nothing"
        );
        assert!(
            saw_publish_module_sh,
            "the scan did not see a live `spacetime publish` in scripts/publish-module.sh, the ONE \
             sanctioned deploy path. That file is the primary thing this tripwire exists to watch; \
             if the scan cannot see it, it cannot catch a `-c` added to it"
        );
    }

    /// The tripwire proper.
    #[test]
    fn no_script_names_a_destructive_spacetime_publish() {
        let root = repo_root();
        let mut violations = Vec::new();
        for file in scanned_scripts() {
            let rel = file
                .strip_prefix(&root)
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/");
            let src =
                std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("cannot read {rel}: {e}"));
            for (line, code) in destructive_publish_lines(&src) {
                violations.push(format!("{rel}:{line}: {code}"));
            }
        }
        assert!(
            violations.is_empty(),
            "a destructive `spacetime publish` flag reached the repo's tooling:\n  {}\n\n\
             `-c` / `--clear-database` CLEARS the database: every account, character, item, quest \
             and reputation row is destroyed, and login stays broken afterwards until the operator \
             identity is re-claimed. There is no undo and no backup in this deployment. Plain \
             `spacetime publish` already auto-migrates an additive schema change, or aborts safely \
             if it cannot — use `bash scripts/publish-module.sh <db>...` and, if publish refuses a \
             genuinely breaking change, take the wipe as a deliberate operator decision typed by \
             hand, never as a flag committed to a script. See docs/danger-zones.md §1.",
            violations.join("\n  "),
        );
    }

    // ---- publish-module.sh's own guards --------------------------------------------------------

    /// The argv guard: this script's arguments are database NAMES, and forwarding a `-c` straight
    /// through to `spacetime publish` is the one unrecoverable mistake it exists to prevent.
    /// Deleting the loop must turn this red.
    #[test]
    fn publish_module_sh_still_refuses_flag_shaped_arguments() {
        // Skipped ONLY when scripts/ is absent wholesale (a mirror cold clone); a scripts/ that is
        // here without publish-module.sh in it panics inside `read_repo`.
        let Some(src) = read_repo("scripts/publish-module.sh") else {
            return;
        };
        let shape = shell_shape(&src);
        assert!(
            shape.contains(FLAG_REJECTION_LOOP),
            "scripts/publish-module.sh no longer carries its argv flag-rejection loop in the pinned \
             shape. Without it, `bash scripts/publish-module.sh -c` forwards `-c` to `spacetime \
             publish` and wipes the database — every account and character gone, login broken. \
             Expected (whitespace-collapsed):\n  {FLAG_REJECTION_LOOP}\nActual file shape:\n  {shape}"
        );
    }

    /// The two flags the wrapper exists to guarantee. Both fail QUIETLY when missing:
    /// `--features=debug_reducers` omitted makes publish report a FALSE "Removed table" breaking
    /// change and abort (a bare publish omits `--features=debug_reducers` and false-aborts);
    /// `--yes` omitted makes the disconnect prompt EOF-abort under a `/dev/null`
    /// stdin, even for a safe additive migration. Both present as "the publish just didn't work",
    /// and the historical consequence was a shard silently left behind on an old schema — which
    /// surfaces as the gateway refusing logons on that shard, not as a publish error.
    #[test]
    fn publish_module_sh_publish_line_keeps_both_required_flags() {
        // Same rule as its sibling above: absent scripts/ skips, present-but-missing panics.
        let Some(src) = read_repo("scripts/publish-module.sh") else {
            return;
        };
        let invocations: Vec<&str> = src
            .lines()
            .map(strip_shell_comment)
            .filter(|code| code.contains(PUBLISH_NEEDLE))
            .collect();
        assert_eq!(
            invocations.len(),
            1,
            "expected exactly one live `spacetime publish` invocation in scripts/publish-module.sh; \
             found {}. A second one is either dead code or an unguarded deploy path — both need a \
             look before this pin is widened. Got: {invocations:?}",
            invocations.len()
        );

        let shape = invocations[0]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            shape.contains("--build-options='--features=debug_reducers'"),
            "scripts/publish-module.sh's publish invocation lost \
             `--build-options='--features=debug_reducers'`. The debug module is behind that Cargo \
             feature, so a build without it does not match the deployed schema and publish \
             FALSE-ABORTS with a bogus \"Removed table\" breaking-change error — which reads as a \
             broken migration and tempts an operator toward the destructive `-c`. Got: {shape}"
        );
        assert!(
            shape.split_whitespace().any(|t| t == "--yes"),
            "scripts/publish-module.sh's publish invocation lost `--yes`. SpacetimeDB prompts \
             \"All clients will be disconnected\" for ANY schema change, including an additive \
             END-appended defaulted column; without `--yes` the prompt hits EOF on a non-tty stdin \
             and the publish aborts, silently leaving that shard on the old schema while its \
             siblings move on — the gateway then refuses logons on it. Got: {shape}"
        );
        assert!(
            !shape.split_whitespace().any(is_destructive_flag),
            "scripts/publish-module.sh's publish invocation carries a destructive flag. This is \
             THE deploy path every operator and script uses: a `-c` here wipes the world database \
             on the next routine deploy. Got: {shape}"
        );
    }
}
