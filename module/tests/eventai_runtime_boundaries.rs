mod support;

use support::Standalone;

/// Runs only when requested because it builds and publishes the Wasm module to its own standalone.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn empty_relay_catalogue_and_spell_guardian_cleanup_apply_through_real_reducers() {
    let standalone = Standalone::start("eventai-runtime-boundaries");
    standalone.publish_module_anonymous();
    standalone.assert_call_anonymous("debug_verify_eventai_spell_guardian_cleanup", &[]);
}
