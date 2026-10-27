use syn::{Expr, Item, Lit, Meta};

fn debug_module_is_gated(source: &str) -> bool {
    let file = syn::parse_file(source).unwrap();
    let modules: Vec<_> = file
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Mod(module) if module.ident == "debug" => Some(module),
            _ => None,
        })
        .collect();
    if modules.len() != 1 {
        return false;
    }
    modules[0].attrs.iter().any(|attr| {
        if !attr.path().is_ident("cfg") {
            return false;
        }
        let Ok(Meta::NameValue(meta)) = attr.parse_args::<Meta>() else {
            return false;
        };
        meta.path.is_ident("feature") && matches!(meta.value,
            Expr::Lit(syn::ExprLit { lit: Lit::Str(value), .. }) if value.value() == "debug_reducers")
    })
}

#[test]
fn native_debug_entry_is_compiled_only_with_debug_reducers() {
    assert!(debug_module_is_gated(include_str!("../src/native/mod.rs")));
}

#[test]
fn the_architecture_check_detects_a_missing_or_ineffective_feature_gate() {
    assert!(debug_module_is_gated(
        "#[cfg(feature = \"debug_reducers\")] mod debug;"
    ));
    for source in [
        "mod debug;",
        "// #[cfg(feature = \"debug_reducers\")]\nmod debug;",
        "#[cfg(test)] mod debug;",
        "#[cfg(any(feature = \"debug_reducers\", unix))] mod debug;",
        "#[cfg(feature = \"debug_reducers\")] fn unrelated() {} mod debug;",
    ] {
        assert!(!debug_module_is_gated(source), "{source}");
    }
}
