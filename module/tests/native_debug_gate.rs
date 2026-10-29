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
            Expr::Lit(syn::ExprLit { lit: Lit::Str(value), .. }) if value.value() == "conformance_test_fixture")
    })
}

#[test]
fn native_debug_entry_is_compiled_only_with_conformance_test_fixture() {
    assert!(debug_module_is_gated(include_str!("../src/native/mod.rs")));
}

#[test]
fn the_architecture_check_detects_a_missing_or_ineffective_feature_gate() {
    assert!(debug_module_is_gated(
        "#[cfg(feature = \"conformance_test_fixture\")] mod debug;"
    ));
    for source in [
        "mod debug;",
        "#[cfg(feature = \"debug_reducers\")] mod debug;",
        "// #[cfg(feature = \"conformance_test_fixture\")]\nmod debug;",
        "#[cfg(test)] mod debug;",
        "#[cfg(any(feature = \"conformance_test_fixture\", unix))] mod debug;",
        "#[cfg(feature = \"conformance_test_fixture\")] fn unrelated() {} mod debug;",
    ] {
        assert!(!debug_module_is_gated(source), "{source}");
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn debug_reducers_build_does_not_export_conformance_entry() {
    use lyracore_test_support::{module_bytes_with_features, Standalone};

    let wasm = module_bytes_with_features("debug_reducers");
    let mut shard = Standalone::start("native-debug-gate");
    shard.publish_module_bytes(&wasm);
    let schema = shard.describe();
    let reducers = schema["reducers"].as_array().expect("Module reducers");
    assert!(reducers
        .iter()
        .any(|reducer| reducer["name"] == "debug_learn_taxi_node"));
    assert!(!reducers
        .iter()
        .any(|reducer| reducer["name"] == "debug_native_enter_world"));
    for name in ["debug_start_virtual_clock", "debug_advance_virtual_clock"] {
        assert!(!reducers.iter().any(|reducer| reducer["name"] == name));
    }
    assert!(!schema["tables"]
        .as_array()
        .expect("Module tables")
        .iter()
        .any(|table| table["name"] == "game_conformance_clock"));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn default_build_does_not_export_taxi_discovery() {
    use lyracore_test_support::{module_bytes_with_features, Standalone};

    let wasm = module_bytes_with_features("");
    let mut shard = Standalone::start("taxi-debug-gate");
    shard.publish_module_bytes(&wasm);
    let schema = shard.describe();
    let reducers = schema["reducers"].as_array().expect("Module reducers");
    assert!(!reducers
        .iter()
        .any(|reducer| reducer["name"] == "debug_learn_taxi_node"));
}
