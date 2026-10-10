//! Lexical helpers shared by `module/build.rs` and the Package API lint test target.

use std::fs;
use std::path::{Path, PathBuf};

/// The whole-file gate of a source file compiled only with `debug_reducers`.
pub const DEBUG_REDUCERS_FILE_CFG: &str = "#![cfg(feature = \"debug_reducers\")]";
/// The whole-file gate of a source file compiled only in a test build.
pub const TEST_FILE_CFG: &str = "#![cfg(test)]";

pub struct StrippedSource {
    /// The source with comments and literals blanked, keeping every line break.
    pub code: String,
    /// Every line comment, as (1-based line, comment text from `//`).
    pub line_comments: Vec<(usize, String)>,
}

/// Blank out comments (line + nested block), string literals (plain, byte, raw), and char
/// literals, PRESERVING newlines and byte-for-char positions, so a scan sees only real code and
/// line numbers stay true. Lifetimes (`'a`) are left intact (only a real char literal, quote,
/// optional escape, closing quote, is blanked). This is what makes a commented-out marker or path
/// inert and lets doc comments show real syntax.
///
/// Line comments are returned beside the code, so text inside a string can never pose as one.
pub fn strip_source(src: &str) -> StrippedSource {
    let b: Vec<char> = src.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(b.len());
    let mut line_comments = Vec::new();
    let blank = |c: char| if c == '\n' { '\n' } else { ' ' };
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        // Line comment (also covers /// and //!): blank to end of line.
        if c == '/' && b.get(i + 1) == Some(&'/') {
            let comment_start = i;
            while i < b.len() && b[i] != '\n' {
                out.push(' ');
                i += 1;
            }
            let line = b[..comment_start].iter().filter(|c| **c == '\n').count() + 1;
            line_comments.push((line, b[comment_start..i].iter().collect()));
            continue;
        }
        // Block comment, nested per Rust.
        if c == '/' && b.get(i + 1) == Some(&'*') {
            let mut depth = 0usize;
            while i < b.len() {
                if b[i] == '/' && b.get(i + 1) == Some(&'*') {
                    depth += 1;
                    out.push(' ');
                    out.push(' ');
                    i += 2;
                } else if b[i] == '*' && b.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    out.push(' ');
                    out.push(' ');
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    out.push(blank(b[i]));
                    i += 1;
                }
            }
            continue;
        }
        // Raw string r"..." / r#"..."# / br#"..."#, only when the r/b starts a token (previous
        // char is not part of an identifier), so `for` / `attr` never false-trigger.
        let prev_is_ident = i > 0 && (b[i - 1].is_alphanumeric() || b[i - 1] == '_');
        if !prev_is_ident && (c == 'r' || (c == 'b' && b.get(i + 1) == Some(&'r'))) {
            let r_at = if c == 'b' { i + 1 } else { i };
            let mut j = r_at + 1;
            while b.get(j) == Some(&'#') {
                j += 1;
            }
            if b.get(j) == Some(&'"') {
                let hashes = j - (r_at + 1);
                // Emit the opener as blanks, then scan for `"` + hashes.
                for c in &b[i..=j] {
                    out.push(blank(*c));
                }
                i = j + 1;
                'raw: while i < b.len() {
                    if b[i] == '"' {
                        let mut h = 0usize;
                        while h < hashes && b.get(i + 1 + h) == Some(&'#') {
                            h += 1;
                        }
                        if h == hashes {
                            for c in &b[i..=(i + hashes)] {
                                out.push(blank(*c));
                            }
                            i += hashes + 1;
                            break 'raw;
                        }
                    }
                    out.push(blank(b[i]));
                    i += 1;
                }
                continue;
            }
            // not a raw string: fall through to emit `c` normally below
        }
        // Plain / byte string literal with escapes.
        if c == '"' || (!prev_is_ident && c == 'b' && b.get(i + 1) == Some(&'"')) {
            if c == 'b' {
                out.push(' ');
                i += 1;
            }
            out.push(' '); // the opening quote
            i += 1;
            while i < b.len() {
                if b[i] == '\\' {
                    out.push(' ');
                    if i + 1 < b.len() {
                        out.push(blank(b[i + 1]));
                    }
                    i += 2;
                    continue;
                }
                if b[i] == '"' {
                    out.push(' ');
                    i += 1;
                    break;
                }
                out.push(blank(b[i]));
                i += 1;
            }
            continue;
        }
        // Char literal vs lifetime: a char literal is `'` + (escape | one char) + `'`.
        if c == '\'' {
            let is_char_lit = match b.get(i + 1) {
                Some('\\') => true,
                Some(_) => b.get(i + 2) == Some(&'\''),
                None => false,
            };
            if is_char_lit {
                out.push(' ');
                i += 1;
                if b.get(i) == Some(&'\\') {
                    // Consume the backslash AND its escaped char first: for '\'' the escaped char
                    // IS a quote, and a terminator scan alone would stop on it one char early. The
                    // tail loop then covers multi-char escapes ('\u{...}').
                    out.push(' ');
                    i += 1;
                    if i < b.len() {
                        out.push(blank(b[i]));
                        i += 1;
                    }
                    while i < b.len() && b[i] != '\'' {
                        out.push(blank(b[i]));
                        i += 1;
                    }
                } else {
                    out.push(blank(b[i]));
                    i += 1;
                }
                if b.get(i) == Some(&'\'') {
                    out.push(' ');
                    i += 1;
                }
                continue;
            }
            // lifetime: keep the quote so positions stay aligned
        }
        out.push(c);
        i += 1;
    }
    StrippedSource {
        code: out.into_iter().collect(),
        line_comments,
    }
}

/// The file's first non-blank line, trimmed: the whole-file gate when it is one.
///
/// Both consumers read one exact leading inner attribute instead of interpreting general `cfg`
/// expressions, so they decide a file's gate without reconstructing the module tree.
pub fn file_gate(source: &str) -> Option<&str> {
    source.lines().map(str::trim).find(|line| !line.is_empty())
}

/// Every `.rs` file beneath `dir`, in no particular order.
pub fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot read dir {}: {e}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("cannot read dir entry in {}: {e}", dir.display()))
            .path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}
