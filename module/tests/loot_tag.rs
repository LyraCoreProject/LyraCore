mod support;

use support::Standalone;

/// Runs the canonical Loot Tag paths against live durable rows in an isolated standalone.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn loot_tag_owns_threat_lifetime_rewards_and_corpse_eligibility() {
    let mut standalone = Standalone::start("loot-tag");
    standalone.publish_module_anonymous();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call_anonymous("debug_verify_loot_tag_fixture", &[]);
}
