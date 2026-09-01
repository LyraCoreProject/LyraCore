mod support;

use support::Standalone;

/// Runs the canonical Loot Tag paths against live durable rows in an isolated standalone.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn loot_tag_owns_threat_lifetime_rewards_and_corpse_eligibility() {
    let standalone = Standalone::start("loot-tag");
    standalone.publish_module_anonymous();
    standalone.assert_call_anonymous("debug_verify_loot_tag_fixture", &[]);
}
