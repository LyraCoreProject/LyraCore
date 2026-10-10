//! The Package API lint, an Architecture Test: every installed Package names only Package API
//! version 2. Core CI runs it with no Package installed; the official Packages job runs it with
//! the collection installed.

mod package_api;

use std::path::{Path, PathBuf};

use package_api::{check_inventory, check_source, Contract, Diagnostic, Finding};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn contract() -> Contract {
    Contract::read(&manifest_dir().join("src/tables.rs"))
}

#[test]
fn enabled_packages_use_package_api() {
    let packages = manifest_dir().join("../packages");
    if let Err(diagnostics) = check_inventory(&contract(), &packages) {
        let lines: Vec<String> = diagnostics.iter().map(Diagnostic::to_string).collect();
        panic!(
            "{} Package API finding(s):\n{}",
            lines.len(),
            lines.join("\n")
        );
    }
}

fn diagnostics_at(relative_path: &str, source: &str) -> Vec<Diagnostic> {
    check_source(&contract(), "sample", Path::new(relative_path), source)
}

/// Each finding as `line:path`, the path as written.
fn reported_at(relative_path: &str, source: &str) -> Vec<String> {
    diagnostics_at(relative_path, source)
        .into_iter()
        .map(|diagnostic| {
            let path = match diagnostic.finding {
                Finding::Outside { path, .. }
                | Finding::GatedChild { path, .. }
                | Finding::PackageModule { path } => path,
                Finding::Unsupported { written, .. } => written,
            };
            format!("{}:{path}", diagnostic.line)
        })
        .collect()
}

fn reported(source: &str) -> Vec<String> {
    reported_at("mod.rs", source)
}

#[test]
fn the_contract_reads_each_name_the_table_catalog_exports() {
    let contract = contract();
    for name in [
        "game_character",
        "Character",
        "game_package_config",
        "PartyPartitionState",
        "game_debug_readout",
    ] {
        assert!(contract.catalogs(name), "{name}");
    }
    for name in [
        "game_operation",
        "UnusedUpperCamel",
        "SessionActor",
        "spell",
    ] {
        assert!(!contract.catalogs(name), "{name}");
    }
}

#[test]
#[should_panic(expected = "names each table")]
fn a_glob_in_the_table_catalog_is_refused() {
    Contract::from_catalog("pub use crate::character::*;\n");
}

#[test]
fn the_fixture_inventory_reports_each_finding_with_its_location() {
    let fixtures = manifest_dir().join("tests/package_api/fixtures");
    let diagnostics = check_inventory(&contract(), &fixtures).expect_err("denied has findings");
    let lines: Vec<String> = diagnostics.iter().map(Diagnostic::to_string).collect();
    let outside = "outside Package API version 2 (docs/package-api.md).";
    let any_root = "Name a path under `crate::actor`, `crate::hooks`, `crate::package`, \
                    `crate::tables`, `crate::script_binding`, or write `// package-api: exempt \
                    <reason>` on that line and raise the gap with the maintainers.";
    let debug_gate = "#![cfg(feature = \"debug_reducers\")]";
    assert_eq!(
        lines,
        [
            format!(
                "Package `denied` names `super::super::spell::pending_cast` at \
                 packages/denied/src/goals.rs:3, {outside} Use `crate::actor::pending_cast`."
            ),
            format!(
                "Package `denied` names `super::super::spell::CastStart` at \
                 packages/denied/src/goals.rs:3, {outside} Use `crate::actor::CastStart`."
            ),
            "Package `denied` uses unsupported syntax crate alias `core` at \
             packages/denied/src/goals.rs:4. Whole-crate aliases can hide core paths across Rust \
             scopes. Spell each core path as `crate::<Package API root>` instead."
                .to_string(),
            format!(
                "Package `denied` names `crate::package::fixture::apply_damage` at \
                 packages/denied/src/goals.rs:7. `crate::package::fixture` is Package API only in \
                 a file whose first non-blank line is `{debug_gate}`."
            ),
            format!(
                "Package `denied` names `crate::helpers::require_operator` at \
                 packages/denied/src/mod.rs:3, {outside} Use `crate::package::require_operator`."
            ),
            format!(
                "Package `denied` names `crate::package::fixture::apply_damage` at \
                 packages/denied/src/mod.rs:4. `crate::package::fixture` is Package API only in \
                 a file whose first non-blank line is `{debug_gate}`."
            ),
            "Package `denied` names the `crate::package` module itself as `crate::package` at \
             packages/denied/src/mod.rs:5. Name `crate::package::<item>` instead, so `fixture` \
             and `test` cannot leave their file gates through the module."
                .to_string(),
            format!(
                "Package `denied` names `crate::UnusedUpperCamel` at \
                 packages/denied/src/mod.rs:7, {outside} {any_root}"
            ),
            format!(
                "Package `denied` names `crate::game_operation` at \
                 packages/denied/src/mod.rs:8, {outside} {any_root}"
            ),
        ]
    );
}

#[test]
fn version_2_roots_tables_markers_and_generated_roots_are_on_the_surface() {
    let source = "use crate::actor::movement::route_step;\n\
                  use crate::tables::{game_spell, Spell};\n\
                  use crate::{game_character, Character, PartyPartitionState};\n\
                  use crate::package::{require_operator, character, encounter::{self, ENCOUNTER_DONE}};\n\
                  fn f(p: &crate::hooks::LevelupPayload) {\n\
                      crate::script_binding::ask();\n\
                      crate::game_tick_pass!(fn pass(ctx) {});\n\
                      crate::character_owned!(delete, fn sweep_delete_pkg_sample_row(ctx, guid) {});\n\
                      let _ = crate::CHARACTER_OWNED_TABLES;\n\
                      crate::pkg_sibling::shared();\n\
                  }\n";
    assert!(reported(source).is_empty(), "{:?}", reported(source));
}

#[test]
fn a_removed_family_root_names_its_version_2_replacement() {
    let diagnostics = diagnostics_at(
        "mod.rs",
        "let cast = crate::spell::pending_cast(ctx, guid);\n\
         let start = crate::spell::CastStart::Started;\n\
         crate::helpers::require_character(ctx);\n\
         use crate::encounter::{self, Unlisted};\n",
    );
    let replacements: Vec<_> = diagnostics
        .into_iter()
        .map(|diagnostic| match diagnostic.finding {
            Finding::Outside { path, replacement } => (path, replacement),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        replacements,
        [
            (
                "crate::spell::pending_cast".to_string(),
                Some("crate::actor::pending_cast")
            ),
            (
                "crate::spell::CastStart::Started".to_string(),
                Some("crate::actor::CastStart")
            ),
            ("crate::helpers::require_character".to_string(), None),
            (
                "crate::encounter::self".to_string(),
                Some("crate::package::encounter")
            ),
            (
                "crate::encounter::Unlisted".to_string(),
                Some("crate::package::encounter")
            ),
        ]
    );
}

#[test]
fn an_unused_upper_camel_name_is_outside_the_surface() {
    assert_eq!(
        reported("fn f(_: crate::UnusedUpperCamel) {}\n"),
        ["1:crate::UnusedUpperCamel"]
    );
}

#[test]
fn an_invented_game_operation_is_outside_the_surface() {
    assert_eq!(
        reported("crate::game_operation(ctx);\n"),
        ["1:crate::game_operation"]
    );
}

#[test]
fn a_gated_child_is_refused_from_an_ordinary_file() {
    // An ordinary build compiles these files; an item gate does not change what the lint sees.
    let debug_item = "#[cfg(feature = \"debug_reducers\")]\nfn f() {\n    crate::package::fixture::apply_damage();\n}\n";
    let test_module = "#[cfg(test)]\nmod tests {\n    use crate::package::test::shape_of;\n}\n";
    assert_eq!(
        reported(debug_item),
        ["3:crate::package::fixture::apply_damage"]
    );
    assert_eq!(reported(test_module), ["3:crate::package::test::shape_of"]);
}

#[test]
fn a_gated_child_is_on_the_surface_in_its_gated_file() {
    let debug_file = "\n#![cfg(feature = \"debug_reducers\")]\nuse crate::package::fixture::{apply_damage, top_threat_target};\nfn f() {\n    super::super::package::fixture::client_cast();\n}\n";
    let test_file = "#![cfg(test)]\nuse crate::package::test::{ask_offline, read_scanned};\nfn f() {\n    super::super::package::test::shape_of();\n}\n";
    assert!(reported_at("verify.rs", debug_file).is_empty());
    assert!(reported_at("tests.rs", test_file).is_empty());
}

#[test]
fn each_gated_child_needs_its_own_gate() {
    let test_file = "#![cfg(test)]\ncrate::package::fixture::apply_damage();\n";
    let debug_file =
        "#![cfg(feature = \"debug_reducers\")]\ncrate::package::test::ask_offline();\n";
    assert_eq!(
        reported(test_file),
        ["2:crate::package::fixture::apply_damage"]
    );
    assert_eq!(
        reported(debug_file),
        ["2:crate::package::test::ask_offline"]
    );
}

#[test]
fn an_alias_cannot_carry_a_gated_child_past_its_gate() {
    let source = "use crate::package as api;\n\
                  use crate::package;\n\
                  use crate::package::{self as api, require_operator};\n\
                  use crate::{package as api};\n\
                  use crate::package::*;\n\
                  use super::package;\n\
                  use crate::package::fixture as f;\n\
                  use crate::package::{fixture::{self as f}};\n";
    let kinds: Vec<String> = diagnostics_at("mod.rs", source)
        .into_iter()
        .map(|diagnostic| match diagnostic.finding {
            Finding::PackageModule { path } => format!("{}:module {path}", diagnostic.line),
            Finding::GatedChild { path, .. } => format!("{}:gated {path}", diagnostic.line),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "1:module crate::package",
            "2:module crate::package",
            "3:module crate::package::self",
            "4:module crate::package",
            "5:module crate::package::*",
            "6:module super::package",
            "7:gated crate::package::fixture",
            "8:gated crate::package::fixture::self",
        ]
    );
}

#[test]
fn an_exemption_cannot_clear_a_gated_child_or_the_package_module() {
    let source = "crate::package::fixture::apply_damage(); // package-api: exempt fixture damage\n\
                  use crate::package::test::read_scanned; // package-api: exempt scan Core\n\
                  use crate::package as api; // package-api: exempt shorter paths\n";
    assert_eq!(
        reported(source),
        [
            "1:crate::package::fixture::apply_damage",
            "2:crate::package::test::read_scanned",
            "3:crate::package",
        ]
    );
}

#[test]
fn an_exemption_clears_its_own_line_and_no_other() {
    let source = "fn f() {\n    crate::auth::create_character(); // package-api: exempt the Package owns its Characters\n    crate::auth::Account::default();\n}\n";
    assert_eq!(reported(source), ["3:crate::auth::Account::default"]);
}

#[test]
fn an_exemption_without_a_reason_does_not_clear() {
    assert_eq!(
        reported("crate::auth::Account; // package-api: exempt\n"),
        ["1:crate::auth::Account"]
    );
}

#[test]
fn marker_text_inside_a_string_is_not_an_exemption() {
    let source = "let note = \"// package-api: exempt not a comment\"; crate::auth::Account;\n";
    assert_eq!(reported(source), ["1:crate::auth::Account"]);
}

#[test]
fn a_path_in_a_comment_or_a_string_is_inert() {
    let source = "// crate::spell::pending_cast is Version 1\n\
                  /* crate::package::fixture::apply_damage */\n\
                  let s = \"crate::game_operation\";\n\
                  let r = r#\"crate::UnusedUpperCamel\"#;\n";
    assert!(reported(source).is_empty(), "{:?}", reported(source));
}

#[test]
fn a_nested_use_tree_is_read_leaf_by_leaf() {
    let source = "use crate::{\n    game_world_entity,\n    auth::Account,\n    actor::{movement::{route_path, RoutePoint}, quest_complete},\n    spell::{pending_cast, CastStart},\n    package::{character::build_player_entity, fixture::apply_damage},\n};\n";
    assert_eq!(
        reported(source),
        [
            "3:crate::auth::Account",
            "5:crate::spell::pending_cast",
            "5:crate::spell::CastStart",
            "6:crate::package::fixture::apply_damage",
        ]
    );
}

#[test]
fn another_crates_path_is_not_this_crates_path() {
    let source =
        "use lyracore_crate::auth::Account;\nuse lyracore_shared::spell::SPELL_ATTR_CHANNELED;\n";
    assert!(reported(source).is_empty(), "{:?}", reported(source));
}

#[test]
fn a_relative_path_that_leaves_the_package_is_checked() {
    assert_eq!(
        reported("use super::auth::Account;\n"),
        ["1:super::auth::Account"]
    );
    let nested = "mod tests {\n    use super::super::package_sibling;\n    use super::super::super::auth::Account;\n}\n";
    assert_eq!(
        reported_at("goals.rs", nested),
        ["3:super::super::super::auth::Account"]
    );
    let raw_module = "mod r#nested { use super::package_item; use super::super::auth::Account; }\n";
    assert_eq!(reported(raw_module), ["1:super::super::auth::Account"]);
}

#[test]
fn package_relative_siblings_and_submodules_stay_inside_the_package() {
    let source = "use super::sibling;\nmod tests {\n    use super::super::package_root_item;\n}\n";
    assert!(reported_at("goals.rs", source).is_empty());
    assert!(reported_at("runner/movement.rs", "use super::super::auth;\n").is_empty());
}

#[test]
fn raw_identifiers_are_normalized_for_the_package_api() {
    let source = "use crate::r#actor::live_entity;\nuse crate::r#auth::Account;\nuse crate::package::r#fixture::apply_damage;\n";
    assert_eq!(
        reported(source),
        [
            "2:crate::r#auth::Account",
            "3:crate::package::r#fixture::apply_damage"
        ]
    );
}

#[test]
fn dollar_crate_and_spaced_separators_do_not_bypass_the_lint() {
    let source = "macro_rules! hidden { () => { $crate::auth::Account } }\nuse crate /* gap */ :: test_scan;\n";
    assert_eq!(
        reported(source),
        ["1:$crate::auth::Account", "2:crate::test_scan"]
    );
}

#[test]
fn a_crate_root_glob_is_outside_the_surface() {
    assert_eq!(
        reported("use crate::*;\nuse super::*;\n"),
        ["1:crate::*", "2:super::*"]
    );
}

#[test]
fn every_whole_crate_alias_spelling_is_unsupported() {
    let source = "use {crate as first};\nuse crate::{self as second, actor};\nextern crate self as third;\nuse crate as r#fourth; // package-api: exempt legacy spelling\n";
    assert_eq!(
        reported(source),
        [
            "1:crate alias `first`",
            "2:crate alias `second`",
            "3:crate alias `third`",
            "4:crate alias `r#fourth`",
        ]
    );
}

#[test]
fn a_package_alias_named_core_is_not_the_crate() {
    let source =
        "mod owned { pub mod auth {} }\nfn f() { use self::owned as core; use core::auth; }\n";
    assert!(reported(source).is_empty(), "{:?}", reported(source));
}

#[test]
fn path_attributes_are_unsupported_even_when_conditional_or_exempted() {
    let source = "#[path = \"layout/hidden.rs\"] // package-api: exempt legacy layout\nmod hidden;\n#[cfg_attr(unix, path = \"unix.rs\")]\nmod platform;\n";
    assert_eq!(
        reported(source),
        ["1:`#[path]`", "3:`#[cfg_attr(..., path = ...)]`"]
    );
}

#[test]
fn include_is_unsupported_and_data_includes_are_not() {
    let source = "include /* gap */ ! (\"private.inc\");\nr#include!(\"hidden.inc\");\nlet text = include_str!(\"fixture.txt\");\nlet bytes = include_bytes!(\"fixture.bin\");\n";
    assert_eq!(reported(source), ["1:`include!`", "2:`include!`"]);
}
