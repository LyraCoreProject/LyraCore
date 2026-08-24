mod support;

use support::Standalone;

/// Exercises EventAI's production encounter notification boundary against the installed dungeon
/// packages. The verifier covers every binding and signal, including refusal outside the owning
/// map and outside an instance.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn eventai_instance_signals_reach_package_owned_outcomes() {
    let standalone = Standalone::start("eventai-instance-packages");
    standalone.publish_module_anonymous();
    standalone.assert_call_anonymous("debug_verify_eventai_instance_packages", &[]);
}
