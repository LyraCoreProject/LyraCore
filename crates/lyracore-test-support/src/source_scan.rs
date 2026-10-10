//! Rust source-scan primitives for Architecture Tests and the Package API's `package_test` root.

/// Isolate the brace-matched body that follows the first occurrence of `signature`. Panics if the
/// signature or a balanced `{...}` is missing, because a scan that cannot find its target has lost
/// its pin.
pub fn body_of(src: &str, signature: &str) -> String {
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

/// Strip a `//` comment from `line`, respecting double-quoted string literals. Raw strings and
/// block comments are not understood.
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

/// [`body_of`] with every comment removed: comment-only lines dropped, trailing comments cut.
pub fn code_of(src: &str, signature: &str) -> String {
    body_of(src, signature)
        .lines()
        .map(strip_line_comment)
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// [`code_of`] with every whitespace run collapsed to one space, for an equality comparison.
pub fn shape_of(src: &str, signature: &str) -> String {
    code_of(src, signature)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_of_strips_a_trailing_comment_and_keeps_the_live_half() {
        let src = "fn f() {\n    let _ = ctx; // bump(ctx, g);\n    // bump in prose\n}";
        let code = code_of(src, "fn f(");
        assert!(!code.contains("bump"), "{code}");
        assert!(code.contains("let _ = ctx;"), "{code}");
    }

    #[test]
    fn code_of_keeps_a_slash_slash_inside_a_string_literal() {
        let src = "fn f() {\n    Err(\"see https://example.com/docs\")?;\n}";
        assert!(code_of(src, "fn f(").contains("https://example.com/docs"));
    }

    #[test]
    fn code_of_handles_an_escaped_quote_inside_a_string_literal() {
        let src = "fn f() {\n    let s = \"a \\\"q\\\" // kept\";\n    let t = 1; // cut\n}";
        let code = code_of(src, "fn f(");
        assert!(code.contains("// kept"), "{code}");
        assert!(!code.contains("cut"), "{code}");
    }

    #[test]
    fn shape_of_collapses_whitespace_for_exact_equality() {
        let src = "fn f() {\n    let   x =\n        1;\n}";
        assert_eq!(shape_of(src, "fn f("), "{ let x = 1; }");
    }

    #[test]
    #[should_panic(expected = "no longer exists")]
    fn body_of_panics_when_the_signature_is_gone() {
        body_of("fn g() {}", "fn f(");
    }
}
