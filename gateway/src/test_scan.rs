//! Shared source-scan primitives for this crate's `#[cfg(test)]` wiring tripwires — a gateway-local
//! port of `module/src/test_scan.rs` (issue #64: one hardened implementation instead of near-identical
//! copies that drift, comment-strip inconsistently, or fall for a needle planted in a trailing
//! comment). Ported rather than shared because the module and gateway are separate crates and
//! `module/src/test_scan.rs`'s functions are `pub(crate)` to that crate; the algorithm here is
//! otherwise identical.
//!
//! Only test-only (see the module doc there for why a source scan, not a mock, is this crate's own
//! answer to the same gap — `ReducerContext` doesn't apply here, but `DbConnection`/`Coordinator`
//! wiring that only a live SpacetimeDB node would exercise is the same shape of untestable-by-mock).
//! Declared behind `#[cfg(test)]` at the `mod` site in `main.rs`, matching `module/src/lib.rs`'s own
//! `#[cfg(test)] mod test_scan;`.

/// Isolate a fn (or struct/const/etc.) body by brace-matching from the first byte offset where
/// `signature` appears. Panics loudly — never silently matches nothing — if the signature or a
/// balanced `{...}` cannot be found, because a scan that can't find its target has lost its pin, not
/// passed it.
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

/// [`strip_line_comment`] for scanners that walk a whole file line by line rather than one extracted
/// body (`main.rs`'s `boundary_panic_tripwire`) — same stripper, so the two can never disagree about
/// what counts as a comment.
pub(crate) fn strip_comment(line: &str) -> &str {
    strip_line_comment(line)
}

/// Strip a Rust line comment from `line`, respecting simple double-quoted string literals so a `//`
/// inside one is never mistaken for a comment start. Does not understand raw strings or block
/// comments — nothing this crate's tripwires scan uses either, and a miss here fails LOUD (the
/// caller's assertion breaks), never silently.
fn strip_line_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_string => i += 1,
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
/// result; always go through this.
pub(crate) fn code_of(src: &str, signature: &str) -> String {
    body_of(src, signature)
        .lines()
        .map(strip_line_comment)
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}
