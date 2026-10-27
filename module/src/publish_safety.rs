use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::test_scan::repo_root;

const PUBLISH_NEEDLE: &str = "spacetime publish";

const DESTRUCTIVE_LONG_FLAGS: &[&str] = &["--clear-database", "--delete-data"];

// Shell # opens a comment only at a word start; parameter-length expressions remain code.
fn strip_shell_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if quote != Some(b'\'') => i += 1,
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

fn is_destructive_flag(raw: &str) -> bool {
    let token = raw.trim_matches(|c: char| c == '"' || c == '\'' || c == ';' || c == ',');
    if let Some(long) = token.strip_prefix("--") {
        let name = long.split('=').next().unwrap_or(long);
        return DESTRUCTIVE_LONG_FLAGS
            .iter()
            .any(|f| f.trim_start_matches('-') == name);
    }
    match token.strip_prefix('-') {
        // Check clusters for c so combined flags cannot bypass destructive-publish checks.
        Some(short) => {
            !short.is_empty()
                && short.chars().all(|c| c.is_ascii_alphabetic())
                && short.contains('c')
        }
        None => false,
    }
}

fn publish_lines(src: &str) -> Vec<usize> {
    src.lines()
        .enumerate()
        .filter(|(_, raw)| strip_shell_comment(raw).contains(PUBLISH_NEEDLE))
        .map(|(i, _)| i + 1)
        .collect()
}

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

const SCAN_ROOTS: &[(&str, usize)] = &[("scripts", 1), ("importer", 5)];

fn scanned_roots() -> Vec<(&'static str, Vec<PathBuf>)> {
    let root = repo_root();
    let mut out = Vec::new();
    for (dir, _) in SCAN_ROOTS {
        let path = root.join(dir);
        assert!(
            path.is_dir(),
            "the destructive-publish scan cannot find {dir}/, so it would pass by looking at nothing"
        );
        let mut files = Vec::new();
        collect_scripts(&path, &mut files);
        files.sort();
        out.push((*dir, files));
    }
    out
}

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

#[derive(Debug, PartialEq, Eq)]
struct SnakeCaseCollision {
    table: String,
    column: String,
    fields: Vec<String>,
}

fn publish_column_name(field: &str) -> String {
    let mut out = String::new();
    let mut previous_was_word = false;
    let mut previous_was_underscore = false;
    for ch in field.trim_start_matches("r#").chars() {
        if ch == '_' {
            if !out.is_empty() && !previous_was_underscore {
                out.push('_');
            }
            previous_was_word = false;
            previous_was_underscore = true;
            continue;
        }
        if previous_was_word && (ch.is_ascii_uppercase() || ch.is_ascii_digit()) {
            out.push('_');
        }
        out.push(ch.to_ascii_lowercase());
        previous_was_word = ch.is_ascii_alphabetic();
        previous_was_underscore = false;
    }
    out.trim_end_matches('_').to_string()
}

fn table_field_collisions(src: &str) -> Vec<SnakeCaseCollision> {
    let mut awaiting_table = false;
    let mut table_attribute_depth = 0i32;
    let mut table: Option<String> = None;
    let mut fields: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut collisions = Vec::new();

    for raw in src.lines() {
        let line = raw.trim();
        if table.is_none() {
            if line.starts_with("#[table") {
                table_attribute_depth =
                    line.matches('[').count() as i32 - line.matches(']').count() as i32;
                awaiting_table = table_attribute_depth == 0;
                continue;
            }
            if table_attribute_depth > 0 {
                table_attribute_depth +=
                    line.matches('[').count() as i32 - line.matches(']').count() as i32;
                awaiting_table = table_attribute_depth == 0;
                continue;
            }
            if awaiting_table {
                if let Some(rest) = line.strip_prefix("pub struct ") {
                    let name = rest
                        .split(|c: char| c == '{' || c.is_ascii_whitespace())
                        .next()
                        .unwrap_or("");
                    table = Some(name.to_string());
                    fields.clear();
                    awaiting_table = false;
                } else if !line.is_empty() && !line.starts_with("#[") {
                    awaiting_table = false;
                }
            }
            continue;
        }

        if line.starts_with('}') {
            let table_name = table.take().expect("table is present");
            for (column, names) in &fields {
                if names.len() > 1 {
                    collisions.push(SnakeCaseCollision {
                        table: table_name.clone(),
                        column: column.clone(),
                        fields: names.clone(),
                    });
                }
            }
            fields.clear();
            continue;
        }
        let Some(rest) = line.strip_prefix("pub ") else {
            continue;
        };
        let Some((name, _)) = rest.split_once(':') else {
            continue;
        };
        let name = name.trim().to_string();
        fields
            .entry(publish_column_name(&name))
            .or_default()
            .push(name);
    }
    collisions
}

fn table_declaration_count(src: &str) -> usize {
    src.lines()
        .filter(|line| line.trim().starts_with("#[table"))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_case_preflight_names_both_fields_in_a_collision() {
        let fixture = [
            "#[ta",
            "ble]\npub struct BrokenRotation {\n",
            "    pub rotation0: f32,\n",
            "    pub rotation_0: f32,\n",
            "}\n",
        ]
        .concat();
        assert_eq!(
            table_field_collisions(&fixture),
            vec![SnakeCaseCollision {
                table: "BrokenRotation".to_string(),
                column: "rotation_0".to_string(),
                fields: vec!["rotation0".to_string(), "rotation_0".to_string()],
            }]
        );
    }

    #[test]
    fn snake_case_preflight_parses_multiline_table_attributes() {
        let fixture = [
            "#[ta",
            "ble(\n",
            "    accessor = broken_rotation,\n",
            "    public\n",
            ")]\n",
            "pub struct BrokenRotation {\n",
            "    pub rotation0: f32,\n",
            "    pub rotation_0: f32,\n",
            "}\n",
        ]
        .concat();
        assert_eq!(table_field_collisions(&fixture).len(), 1);
    }

    #[test]
    fn module_tables_have_unique_published_column_names() {
        let src_root = repo_root().join("module/src");
        let mut rust_files = Vec::new();
        collect_rust_files(&src_root, &mut rust_files);
        assert!(
            rust_files.len() >= 50,
            "found only {} Rust files under module/src; the snake-case publish preflight is looking at the wrong tree",
            rust_files.len()
        );
        let mut failures = Vec::new();
        let mut table_count = 0;
        for path in rust_files {
            let src = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("readable {}: {e}", path.display()));
            table_count += table_declaration_count(&src);
            for collision in table_field_collisions(&src) {
                failures.push(format!(
                    "{}: table `{}` fields `{}` publish as duplicate column `{}`",
                    path.display(),
                    collision.table,
                    collision.fields.join("` and `"),
                    collision.column
                ));
            }
        }
        assert!(
            table_count >= 100,
            "found only {table_count} #[table] declarations; the snake-case preflight parser lost its production surface"
        );
        assert!(
            failures.is_empty(),
            "SpacetimeDB snake_case field collision(s); refusing publish:\n  {}",
            failures.join("\n  ")
        );
    }

    #[test]
    fn sanctioned_cli_runs_the_column_collision_guard_before_publish() {
        let cli = std::fs::read_to_string(repo_root().join("lyracore"))
            .expect("the sanctioned CLI wrapper must be readable");
        assert!(cli.contains("preflight | publish)"));
        assert!(cli.contains("module_tables_have_unique_published_column_names"));
        assert!(
            cli.find("module_tables_have_unique_published_column_names")
                .expect("collision guard invocation")
                < cli.find("exec \"$binary\"").expect("CLI handoff")
        );
    }

    fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in
            std::fs::read_dir(dir).unwrap_or_else(|e| panic!("readable {}: {e}", dir.display()))
        {
            let path = entry.expect("readable dir entry").path();
            if path.is_dir() {
                collect_rust_files(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }

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
    }

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
             if it cannot. Publish through `./lyracore publish`. If publish refuses a \
             genuinely breaking change, take the wipe as a deliberate operator decision typed by \
             hand, never as a flag committed to a script. See docs/danger-zones.md §1.",
            violations.join("\n  "),
        );
    }
}
