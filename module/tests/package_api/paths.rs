//! Every crate-root path a Package source file names, read lexically from stripped source.

/// One crate-root path as the Package wrote it.
#[derive(Clone, Debug, Default)]
pub struct RootedPath {
    /// The segments after the crate root, raw identifiers normalized and `self` dropped. A glob
    /// ends in `*`.
    pub segments: Vec<String>,
    /// The path as written, from its root keyword (`crate`, `$crate` or `super::..`).
    pub written: String,
    /// The name a `use` binds with `as`, when it renames.
    pub alias: Option<String>,
    /// Byte offset of the last segment in the stripped source.
    offset: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedSyntax {
    WholeCrateAlias,
    PathAttribute,
    IncludeMacro,
}

/// What one Package file names beyond its own modules.
pub struct FilePaths {
    /// Every crate-root path, one per use-tree leaf, as (1-based line, path).
    pub paths: Vec<(usize, RootedPath)>,
    /// Syntax whose meaning this scan cannot track, as (1-based line, kind, spelling).
    pub unsupported: Vec<(usize, UnsupportedSyntax, String)>,
}

#[derive(Clone, Copy)]
pub struct Token<'a> {
    pub text: &'a str,
    start: usize,
}

/// The Rust tokens this lint needs, with byte positions for diagnostics. This is deliberately much
/// smaller than a parser: identifiers, `::`, and punctuation stay distinct; whitespace vanishes.
pub fn tokens(stripped: &str) -> Vec<Token<'_>> {
    let bytes = stripped.as_bytes();
    let ident_byte = |byte: &u8| byte.is_ascii_alphanumeric() || *byte == b'_';
    let mut tokens = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        if bytes[at].is_ascii_whitespace() {
            at += 1;
            continue;
        }
        let start = at;
        let raw_ident = bytes[at] == b'r'
            && bytes.get(at + 1) == Some(&b'#')
            && bytes
                .get(at + 2)
                .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_');
        if raw_ident || bytes[at].is_ascii_alphabetic() || bytes[at] == b'_' {
            at += if raw_ident { 3 } else { 1 };
            while bytes.get(at).is_some_and(ident_byte) {
                at += 1;
            }
        } else if bytes[at] == b':' && bytes.get(at + 1) == Some(&b':') {
            at += 2;
        } else {
            at += stripped[at..]
                .chars()
                .next()
                .expect("at is before the end of source")
                .len_utf8();
        }
        tokens.push(Token {
            text: &stripped[start..at],
            start,
        });
    }
    tokens
}

pub fn is_ident(token: Token<'_>) -> bool {
    token
        .text
        .as_bytes()
        .first()
        .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
}

fn ident_name(token: Token<'_>) -> Option<&str> {
    is_ident(token).then(|| token.text.strip_prefix("r#").unwrap_or(token.text))
}

fn line_at(stripped: &str, offset: usize) -> usize {
    stripped[..offset].matches('\n').count() + 1
}

/// Every leaf of the use tree or plain path that starts at `tokens[at]`, the token after a root's
/// `::`, appended below `parent`. Returns the index of the first token after the tree.
pub fn tree_leaves(
    tokens: &[Token<'_>],
    at: usize,
    parent: &RootedPath,
    out: &mut Vec<RootedPath>,
) -> usize {
    let mut leaf = parent.clone();
    let mut index = at;
    while let Some(token) = tokens.get(index).copied() {
        if token.text == "{" {
            return group_leaves(tokens, index, &leaf, out);
        }
        if token.text == "*" || is_ident(token) {
            leaf.written.push_str("::");
            leaf.written.push_str(token.text);
            if token.text != "self" {
                let name = ident_name(token).unwrap_or(token.text);
                leaf.segments.push(name.to_string());
                leaf.offset = token.start;
            }
            index += 1;
            if token.text != "*" && tokens.get(index).is_some_and(|next| next.text == "::") {
                index += 1;
                continue;
            }
        }
        break;
    }
    if tokens.get(index).is_some_and(|token| token.text == "as") {
        if let Some(alias) = tokens.get(index + 1).copied().filter(|t| is_ident(*t)) {
            leaf.alias = Some(alias.text.to_string());
            index += 2;
        }
    }
    if leaf.segments.len() > parent.segments.len() || leaf.written.ends_with("::self") {
        out.push(leaf);
    }
    index
}

/// The leaves of one braced group whose `{` is `tokens[open]`.
fn group_leaves(
    tokens: &[Token<'_>],
    open: usize,
    parent: &RootedPath,
    out: &mut Vec<RootedPath>,
) -> usize {
    let mut index = open + 1;
    while let Some(token) = tokens.get(index) {
        match token.text {
            "}" => return index + 1,
            "," => index += 1,
            _ => {
                let next = tree_leaves(tokens, index, parent, out);
                index = next.max(index + 1);
            }
        }
    }
    index
}

/// Inline `mod name { .. }` bodies, as byte ranges. File position supplies the rest of a Package
/// module's depth.
fn inline_modules(tokens: &[Token<'_>]) -> Vec<(usize, usize)> {
    let mut open = Vec::new();
    let mut pairs = Vec::new();
    for token in tokens {
        match token.text {
            "{" => open.push(token.start),
            "}" => {
                if let Some(start) = open.pop() {
                    pairs.push((start, token.start));
                }
            }
            _ => {}
        }
    }
    tokens
        .windows(3)
        .filter(|w| w[0].text == "mod" && is_ident(w[1]) && w[2].text == "{")
        .filter_map(|w| pairs.iter().find(|(open, _)| *open == w[2].start).copied())
        .collect()
}

/// The crate-root paths and unsupported syntax in one Package file. `file_depth` is zero for
/// `src/mod.rs`, one for `src/foo.rs` or `src/foo/mod.rs`, and so on. Inline modules add to it.
///
/// A path is rooted at `crate`, `$crate`, or enough leading `super` segments to leave the Package.
pub fn file_paths(stripped: &str, file_depth: usize) -> FilePaths {
    let tokens = tokens(stripped);
    let modules = inline_modules(&tokens);
    let mut leaves = Vec::new();
    let mut index = 0usize;
    while index < tokens.len() {
        let token = tokens[index];
        let after = |offset: usize, text: &str| {
            tokens
                .get(index + offset)
                .is_some_and(|token| token.text == text)
        };
        let (root, separator) = if token.text == "$" && after(1, "crate") && after(2, "::") {
            ("$crate".to_string(), index + 2)
        } else if token.text == "crate"
            && after(1, "::")
            && (index == 0 || tokens[index - 1].text != "$")
        {
            ("crate".to_string(), index + 1)
        } else if token.text == "super" && (index == 0 || tokens[index - 1].text != "::") {
            let mut levels = 1usize;
            let mut end = index;
            while tokens.get(end + 1).is_some_and(|t| t.text == "::")
                && tokens.get(end + 2).is_some_and(|t| t.text == "super")
            {
                levels += 1;
                end += 2;
            }
            let depth = file_depth
                + modules
                    .iter()
                    .filter(|(open, close)| *open < token.start && token.start < *close)
                    .count();
            if levels != depth + 1 || tokens.get(end + 1).is_none_or(|t| t.text != "::") {
                index = end + 1;
                continue;
            }
            (vec!["super"; levels].join("::"), end + 1)
        } else {
            index += 1;
            continue;
        };
        let parent = RootedPath {
            written: root,
            ..RootedPath::default()
        };
        index = tree_leaves(&tokens, separator + 1, &parent, &mut leaves).max(separator + 1);
    }
    let paths = leaves
        .into_iter()
        .filter(|leaf| !leaf.segments.is_empty())
        .map(|leaf| (line_at(stripped, leaf.offset), leaf))
        .collect();
    FilePaths {
        paths,
        unsupported: unsupported_syntax(&tokens, stripped),
    }
}

/// Syntax whose meaning the lint cannot track reliably. It is refused where it is declared
/// instead of guessing at Rust name resolution or filesystem-to-module mapping.
fn unsupported_syntax(
    tokens: &[Token<'_>],
    stripped: &str,
) -> Vec<(usize, UnsupportedSyntax, String)> {
    let mut found = Vec::new();
    let mut alias = |start: usize, name: &str| {
        found.push((
            start,
            UnsupportedSyntax::WholeCrateAlias,
            format!("crate alias `{name}`"),
        ));
    };
    for (index, token) in tokens.iter().enumerate() {
        // `crate as core` and `crate::{self as core}` in any use tree.
        if token.text != "crate" || !in_use_statement(tokens, index) {
            continue;
        }
        let renames = |at: usize| {
            tokens.get(at).is_some_and(|t| t.text == "as")
                && tokens
                    .get(at + 1)
                    .is_some_and(|t| is_ident(*t) && t.text != "_")
        };
        if renames(index + 1) {
            alias(token.start, tokens[index + 2].text);
        } else if tokens.get(index + 1).is_some_and(|t| t.text == "::")
            && tokens.get(index + 2).is_some_and(|t| t.text == "{")
        {
            let mut depth = 0usize;
            let mut entry = true;
            for at in index + 3..tokens.len() {
                match tokens[at].text {
                    "{" => depth += 1,
                    "}" if depth == 0 => break,
                    "}" => depth -= 1,
                    "," if depth == 0 => entry = true,
                    "self" if depth == 0 && entry && renames(at + 1) => {
                        alias(token.start, tokens[at + 2].text);
                        entry = false;
                    }
                    _ if depth == 0 => entry = false,
                    _ => {}
                }
            }
        }
    }
    // `extern crate self as core` is the older spelling of the same file-local crate alias.
    for window in tokens.windows(5) {
        if window[0].text == "extern"
            && window[1].text == "crate"
            && window[2].text == "self"
            && window[3].text == "as"
            && is_ident(window[4])
            && window[4].text != "_"
        {
            alias(window[0].start, window[4].text);
        }
    }
    found.extend(path_attributes(tokens));
    // `include!` parses another file as Rust in this module, beyond `.rs` discovery. Data
    // inclusion (`include_str!`, `include_bytes!`) stays available.
    for window in tokens.windows(2) {
        if ident_name(window[0]) == Some("include") && window[1].text == "!" {
            found.push((
                window[0].start,
                UnsupportedSyntax::IncludeMacro,
                "`include!`".to_string(),
            ));
        }
    }
    found.sort_by_key(|finding| finding.0);
    found.dedup_by(|left, right| left.0 == right.0 && left.1 == right.1);
    found
        .into_iter()
        .map(|(start, kind, written)| (line_at(stripped, start), kind, written))
        .collect()
}

/// Whether `tokens[index]` sits inside a `use` statement.
fn in_use_statement(tokens: &[Token<'_>], index: usize) -> bool {
    tokens[..index]
        .iter()
        .rev()
        .take_while(|token| !matches!(token.text, ";" | "}"))
        .any(|token| token.text == "use")
}

/// `#[path = ..]` and `#[cfg_attr(.., path = ..)]`, which break Package source discovery.
fn path_attributes(tokens: &[Token<'_>]) -> Vec<(usize, UnsupportedSyntax, String)> {
    let mut found = Vec::new();
    for (index, window) in tokens.windows(2).enumerate() {
        if window[0].text != "#" || window[1].text != "[" {
            continue;
        }
        let Some(end) = tokens[index + 2..].iter().position(|t| t.text == "]") else {
            continue;
        };
        let attribute = &tokens[index + 2..index + 2 + end];
        let names_path =
            |pair: &[Token<'_>]| ident_name(pair[0]) == Some("path") && pair[1].text == "=";
        let written = match attribute.first().and_then(|t| ident_name(*t)) {
            Some("path") if attribute.len() > 1 && names_path(&attribute[..2]) => "`#[path]`",
            Some("cfg_attr") if attribute.windows(2).any(names_path) => {
                "`#[cfg_attr(..., path = ...)]`"
            }
            _ => continue,
        };
        found.push((
            window[0].start,
            UnsupportedSyntax::PathAttribute,
            written.to_string(),
        ));
    }
    found
}
