//! Lexical helpers for architecture tests. These scans do not prove runtime control flow.

use std::ops::Range;

use rustc_lexer::{tokenize, TokenKind};

fn is_comment(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::LineComment | TokenKind::BlockComment { .. }
    )
}

fn masked(src: &str, omit: impl Fn(TokenKind) -> bool) -> String {
    let mut bytes = src.as_bytes().to_vec();
    let mut start = 0;
    for token in tokenize(src) {
        let end = start + token.len;
        if omit(token.kind) {
            for byte in &mut bytes[start..end] {
                if !matches!(*byte, b'\n' | b'\r') {
                    *byte = b' ';
                }
            }
        }
        start = end;
    }
    String::from_utf8(bytes).expect("masking whole tokens preserves UTF-8")
}

/// Mask comments, literals, and test-only items or blocks, preserving byte offsets.
/// Handles outer `#[cfg(...)]` predicates. Inner `#![cfg(...)]` attributes remain in the scan.
/// This does not expand macros or `cfg_attr`.
pub fn code_only(src: &str) -> String {
    without_test_code(&masked(src, |kind| {
        is_comment(kind) || matches!(kind, TokenKind::Literal { .. })
    }))
}

// Unknown configuration predicates can hold in production. Only remove code when
// setting `test = false` proves its cfg false.
fn cfg_without_test(tokens: &[(&str, Range<usize>)]) -> Option<bool> {
    if tokens.len() == 1 && tokens[0].0 == "test" {
        return Some(false);
    }
    if tokens.len() < 3 || tokens[1].0 != "(" || tokens.last()?.0 != ")" {
        return None;
    }
    let mut args = Vec::new();
    let mut start = 2;
    let mut i = start;
    while i < tokens.len() - 1 {
        if tokens[i].0 == "," {
            args.push(cfg_without_test(&tokens[start..i]));
            start = i + 1;
        } else if matches!(tokens[i].0, "(" | "[") {
            i = group_end(tokens, i);
        }
        i += 1;
    }
    if start < i {
        args.push(cfg_without_test(&tokens[start..i]));
    }
    match tokens[0].0 {
        "all" if args.contains(&Some(false)) => Some(false),
        "all" if args.iter().all(|arg| *arg == Some(true)) => Some(true),
        "any" if args.contains(&Some(true)) => Some(true),
        "any" if args.iter().all(|arg| *arg == Some(false)) => Some(false),
        "not" if args.len() == 1 => args[0].map(|value| !value),
        _ => None,
    }
}

fn group_end(tokens: &[(&str, Range<usize>)], start: usize) -> usize {
    let mut depth = 0;
    for (i, (token, _)) in tokens.iter().enumerate().skip(start) {
        match *token {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    panic!("unterminated group in source scan");
}

fn without_test_code(src: &str) -> String {
    let mut offset = 0;
    let tokens: Vec<_> = tokenize(src)
        .filter_map(|token| {
            let range = offset..offset + token.len;
            offset = range.end;
            (!is_comment(token.kind) && token.kind != TokenKind::Whitespace)
                .then(|| (&src[range.clone()], range))
        })
        .collect();
    let mut bytes = src.as_bytes().to_vec();
    let mut i = 0;
    while i + 2 < tokens.len() {
        if tokens[i].0 != "#" || tokens[i + 1].0 != "[" || tokens[i + 2].0 != "cfg" {
            i += 1;
            continue;
        }
        let attr_end = group_end(&tokens, i + 1);
        if cfg_without_test(&tokens[i + 4..attr_end - 1]) != Some(false) {
            i = attr_end + 1;
            continue;
        }
        let start = tokens[i].1.start;
        i = attr_end + 1;
        let mut generic_depth = 0usize;
        let mut function = false;
        let mut field_type = false;
        let mut unmatched_close = false;
        while i < tokens.len() {
            match tokens[i].0 {
                ";" => break,
                "}" | ")" | "]" => {
                    unmatched_close = true;
                    break;
                }
                "," if generic_depth == 0 && !function => break,
                "fn" if !field_type => function = true,
                ":" if !function => field_type = true,
                "<" => generic_depth += 1,
                ">" if tokens[i - 1].0 != "-" => {
                    generic_depth = generic_depth.saturating_sub(1);
                }
                "{" if generic_depth == 0 => {
                    i = group_end(&tokens, i);
                    break;
                }
                "(" | "[" | "{" => i = group_end(&tokens, i),
                _ => {}
            }
            i += 1;
        }
        assert!(i < tokens.len(), "test-only item has no end");
        let end = if unmatched_close {
            tokens[i].1.start
        } else {
            tokens[i].1.end
        };
        for byte in &mut bytes[start..end] {
            if !matches!(*byte, b'\n' | b'\r') {
                *byte = b' ';
            }
        }
        i += 1;
    }
    String::from_utf8(bytes).expect("masking whole items preserves UTF-8")
}

fn token_at(src: &str, byte_idx: usize) -> TokenKind {
    assert!(
        byte_idx < src.len(),
        "token offset must be inside the source"
    );
    let mut end = 0;
    for token in tokenize(src) {
        end += token.len;
        if byte_idx < end {
            return token.kind;
        }
    }
    unreachable!("the lexer covers every source byte")
}

/// Whether this byte belongs to a line or block comment.
pub fn in_comment(src: &str, byte_idx: usize) -> bool {
    is_comment(token_at(src, byte_idx))
}

fn body_range(src: &str, signature: &str) -> Range<usize> {
    let code = masked(src, |kind| {
        is_comment(kind) || matches!(kind, TokenKind::Literal { .. })
    });
    let uncommented = masked(src, is_comment);
    let start = uncommented
        .match_indices(signature)
        .find(|(start, _)| !matches!(token_at(src, *start), TokenKind::Literal { .. }))
        .map(|(start, _)| start)
        .unwrap_or_else(|| panic!("`{signature}` no longer exists in this source"));
    let open = start + code[start..].find('{').expect("item has a body");
    let mut depth = 0;
    for (offset, byte) in code.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return open..open + offset + 1;
                }
            }
            _ => {}
        }
    }
    panic!("unterminated body for `{signature}`");
}

/// Find a body without matching signatures or braces inside comments and literals.
/// Returns the original body, including comments. Use `code_of` for content checks.
pub fn body_of(src: &str, signature: &str) -> String {
    src[body_range(src, signature)].to_string()
}

/// Extract a body with comments removed, preserving literal values and test-only code.
/// Use `code_only` when a check must exclude non-production tokens.
pub fn code_of(src: &str, signature: &str) -> String {
    compact(&masked(&body_of(src, signature), is_comment))
}

/// Collapse whitespace in `code_of` for the Package API's source comparisons.
pub fn shape_of(src: &str, signature: &str) -> String {
    code_of(src, signature)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn compact(src: &str) -> String {
    src.lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commented_calls_do_not_satisfy_presence_checks() {
        let source = "fn update() { require_operator(ctx)?; publish(ctx, &row); }";
        for call in ["require_operator(ctx)?;", "publish(ctx, &row);"] {
            assert!(code_only(source).contains(call));
            for comment in [
                format!("/* {call} */"),
                format!("// {call}\n"),
                format!("let _ = r#\"{call}\"#;"),
                format!("#[cfg(test)] {{ {call} }}"),
            ] {
                let changed = source.replace(call, &comment);
                assert!(!code_only(&changed).contains(call));
            }
        }
    }

    #[test]
    fn commented_and_quoted_signatures_do_not_select_the_body() {
        let source = r###"
            // fn target() { wrong_line(); }
            /* fn target() { wrong_block(); } */
            const DECOY: &str = r#"fn target() { wrong_string(); }"#;
            #[cfg(test)] fn target() { actual(); }
        "###;
        assert_eq!(body_of(source, "fn target("), "{ actual(); }");
    }

    #[test]
    fn nested_comments_cannot_hide_the_end_of_a_body() {
        let source = "fn f() { before(); /* } /* nested { */ } */ after(); } fn g() {}";
        let body = code_of(source, "fn f(");
        assert!(body.contains("before();"));
        assert!(body.contains("after();"));
        assert!(!body.contains("nested"));
        assert!(!body.contains("fn g"));
    }

    #[test]
    fn braces_and_comment_markers_in_literals_are_preserved() {
        let source = r###"fn f<'a>(s: &'a str) -> [u8; 3] {
            let url = "https://example.com/} /* literal */";
            let escaped = "\" } // literal";
            let raw = br##"\" } /* raw */ // literal"##;
            let ch = b'}';
            let unicode = 'é';
            /* remove this { */ after();
        } fn g() {}"###;
        let body = code_of(source, "fn f<'a>(s: &'a str) -> [u8; 3]");
        assert!(body.contains("https://example.com/} /* literal */"));
        assert!(body.contains(r###"br##"\" } /* raw */ // literal"##"###));
        assert!(body.contains("let ch = b'}';"));
        assert!(body.contains("let unicode = 'é';"));
        assert!(body.contains("after();"));
        assert!(!body.contains("remove this"));
        assert!(!body.contains("fn g"));
    }

    #[test]
    fn masking_preserves_offsets_and_distinguishes_lifetimes_from_characters() {
        let source = "fn f<'a>() { /* é\ncomment */ let x = 'é'; read(); }";
        let code = code_only(source);
        assert_eq!(code.len(), source.len());
        assert_eq!(code.find("read()"), source.find("read()"));
        assert_eq!(code.find('\n'), source.find('\n'));
        assert!(code.contains("fn f<'a>()"));
        assert!(!code.contains('é'));
        assert!(in_comment(source, source.find("comment").unwrap()));
        assert!(!in_comment(source, source.find("read").unwrap()));
    }

    #[test]
    fn production_scans_exclude_test_configuration_and_resume_after_items() {
        let source = r#"
            #[cfg(all(unix, test))] #[allow(dead_code)] fn hidden() { hidden_call(); }
            #[cfg(test)] mod external;
            #[cfg(test)] fn generic<S, St>() { hidden_call(); }
            #[cfg(test)] fn fallible() -> Result<(), ()> { hidden_call(); Ok(()) }
            #[cfg(test)] fn callable<F: Fn() -> (), T>() { hidden_call(); }
            #[cfg(test)] fn bounded<S, St>() where S: Read, St: Store { hidden_call(); }
            #[cfg(test)] fn constant<const N: usize = { 1 + 2 }>() { hidden_call(); }
            #[cfg(not(not(test)))] fn also_hidden() { hidden_call(); }
            #[cfg(any(test, unix))] fn possible() { possible_call(); }
            #[cfg(not(test))] fn production() { production_call(); }
            fn last() { #[cfg(test)] { hidden_call(); } last_call(); }
            fn local() { #[cfg(test)] fn helper() { hidden_call(); } after_helper(); }
            struct S { #[cfg(test)] a: u8 }
            fn after_field() { field_successor(); }
            enum E { #[cfg(test)] A }
            fn after_variant() { variant_successor(); }
            fn matched() { match n { #[cfg(test)] 1 => hidden_call() } }
            fn after_match() { match_successor(); }
        "#;
        let code = code_only(source);
        assert!(!code.contains("hidden_call"));
        assert!(!code.contains("external"));
        assert!(body_of(&code, "fn last(").contains("last_call()"));
        assert!(body_of(&code, "fn local(").contains("after_helper()"));
        for call in [
            "possible_call()",
            "production_call()",
            "last_call()",
            "field_successor()",
            "variant_successor()",
            "match_successor()",
        ] {
            assert!(code.contains(call));
            assert_eq!(code.find(call), source.find(call));
        }
    }

    #[test]
    fn shape_of_collapses_whitespace_for_exact_equality() {
        let src = "fn f() {\n    let   x =\n        1;\n}";
        assert_eq!(shape_of(src, "fn f("), "{ let x = 1; }");
    }

    #[test]
    #[should_panic(expected = "no longer exists")]
    fn a_commented_out_function_is_missing() {
        code_of("/* fn gone() {} */", "fn gone(");
    }

    #[test]
    #[should_panic(expected = "unterminated body")]
    fn a_brace_in_a_comment_cannot_close_a_function() {
        body_of("fn broken() { /* } */", "fn broken(");
    }
}
