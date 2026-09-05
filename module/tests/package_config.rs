//! `game_package_config` at the reducer boundary: `debug_seed_package_config` and
//! `set_package_config` against a real Shard, through the `by_package_key` lookup and the actual
//! row write.
//!
//! `decide_config_write` / `unknown_key_message` / `should_seed_default` are pure and covered
//! where they live (`module/src/package_config.rs`); this drives the rung those cannot reach.

mod support;

use std::collections::BTreeMap;

use support::Standalone;

const PACKAGE: &str = "example.package_config";

fn arg(value: &str) -> String {
    serde_json::to_string(value).expect("a string encodes as JSON")
}

fn config_rows(standalone: &Standalone, package: &str) -> Vec<BTreeMap<String, String>> {
    let mut rows = standalone.query_rows(&format!(
        "SELECT * FROM game_package_config WHERE package_name = '{package}'"
    ));
    rows.sort_by(|a, b| a["key"].cmp(&b["key"]));
    rows
}

fn config_value(rows: &[BTreeMap<String, String>], key: &str) -> Option<String> {
    rows.iter()
        .find(|row| row["key"] == key)
        .map(|row| row["value"].clone())
}

/// Seeding creates the row once, and re-seeding with the package's own default never touches a
/// value already sitting there — whether that value came from the prior seed or from an
/// Operator's own edit.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn debug_seed_package_config_seeds_once_and_never_clobbers_a_live_value_on_reseed() {
    let mut standalone = Standalone::start("package-config-seed");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);

    standalone.assert_call(
        "debug_seed_package_config",
        &[&arg(PACKAGE), &arg("max_bots"), &arg("10")],
    );
    let rows = config_rows(&standalone, PACKAGE);
    assert_eq!(
        rows.len(),
        1,
        "the seed must create exactly one row: {rows:?}"
    );
    assert_eq!(config_value(&rows, "max_bots"), Some("10".to_string()));

    // An Operator's own edit sitting on the row.
    standalone.assert_call(
        "set_package_config",
        &[&arg(PACKAGE), &arg("max_bots"), &arg("99"), "false"],
    );

    // Re-seeding with the package's ORIGINAL default must not touch the Operator's edit.
    standalone.assert_call(
        "debug_seed_package_config",
        &[&arg(PACKAGE), &arg("max_bots"), &arg("10")],
    );
    let rows = config_rows(&standalone, PACKAGE);
    assert_eq!(
        rows.len(),
        1,
        "re-seeding an existing key must not duplicate the row: {rows:?}"
    );
    assert_eq!(
        config_value(&rows, "max_bots"),
        Some("99".to_string()),
        "re-seeding must never clobber a live value"
    );
}

/// `set_package_config` on an already-seeded key updates the row in place.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn set_package_config_updates_an_existing_keys_value() {
    let mut standalone = Standalone::start("package-config-update");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call(
        "debug_seed_package_config",
        &[&arg(PACKAGE), &arg("max_bots"), &arg("10")],
    );

    standalone.assert_call(
        "set_package_config",
        &[&arg(PACKAGE), &arg("max_bots"), &arg("25"), "false"],
    );

    let rows = config_rows(&standalone, PACKAGE);
    assert_eq!(rows.len(), 1, "an update must not add a row: {rows:?}");
    assert_eq!(config_value(&rows, "max_bots"), Some("25".to_string()));
}

/// An unknown `(package_name, key)` without `allow_new` is refused, and the refusal names the
/// package's existing keys so a typo reads as a loud error rather than a silently ignored write.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn set_package_config_refuses_an_unknown_key_and_names_the_packages_known_keys() {
    let mut standalone = Standalone::start("package-config-refusal");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call(
        "debug_seed_package_config",
        &[&arg(PACKAGE), &arg("max_bots"), &arg("10")],
    );
    standalone.assert_call(
        "debug_seed_package_config",
        &[&arg(PACKAGE), &arg("spawn_rate"), &arg("5")],
    );

    let refused = standalone.call(
        "set_package_config",
        &[&arg(PACKAGE), &arg("totally_unknown"), &arg("1"), "false"],
    );
    assert!(
        !refused.status.success(),
        "an unknown key without allow_new must be refused"
    );
    let message = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(message.contains("totally_unknown"), "{message}");
    assert!(message.contains("max_bots"), "{message}");
    assert!(message.contains("spawn_rate"), "{message}");

    assert_eq!(
        config_rows(&standalone, PACKAGE).len(),
        2,
        "a refused write must not create a row"
    );
}

/// The same unknown key, with `allow_new`, is accepted and creates the row.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn set_package_config_with_allow_new_creates_an_unknown_key() {
    let mut standalone = Standalone::start("package-config-allow-new");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call(
        "debug_seed_package_config",
        &[&arg(PACKAGE), &arg("max_bots"), &arg("10")],
    );

    standalone.assert_call(
        "set_package_config",
        &[&arg(PACKAGE), &arg("brand_new_key"), &arg("42"), "true"],
    );

    let rows = config_rows(&standalone, PACKAGE);
    assert_eq!(
        rows.len(),
        2,
        "allow_new must add exactly one new row: {rows:?}"
    );
    assert_eq!(config_value(&rows, "brand_new_key"), Some("42".to_string()));
    assert_eq!(config_value(&rows, "max_bots"), Some("10".to_string()));
}
