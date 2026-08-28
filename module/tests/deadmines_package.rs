mod support;

use support::Standalone;

/// Drives the installed `dungeons` Package's Deadmines choreography through the production paths:
/// the kill path opens the three boss doors and ejects Sneed from his shredder, the damage path
/// fires Mr. Smite's 66%/33% stands (yell, chest run, weapon swap), and the gameobject use path
/// breaches the Iron Clad Door with the Defias Cannon. Every stage reducer verifies its own
/// durable outcome; no stage is timer-driven, so the sequence runs without waits.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI, the Wasm toolchain, and the dungeons Package installed (lyracore packages add dungeons)"]
fn deadmines_choreography_reaches_package_owned_outcomes() {
    let standalone = Standalone::start("deadmines-package");
    standalone.publish_module_anonymous();
    standalone.assert_call_anonymous("debug_deadmines_begin", &[]);
    standalone.assert_call_anonymous("debug_deadmines_rhahkzor_falls", &[]);
    standalone.assert_call_anonymous("debug_deadmines_shredder_ejects_sneed", &[]);
    standalone.assert_call_anonymous("debug_deadmines_sneed_falls", &[]);
    standalone.assert_call_anonymous("debug_deadmines_gilnid_falls", &[]);
    standalone.assert_call_anonymous("debug_deadmines_smite_improvises", &[]);
    standalone.assert_call_anonymous("debug_deadmines_smite_gets_angry", &[]);
    standalone.assert_call_anonymous("debug_deadmines_cannon_breaches", &[]);
    standalone.assert_call_anonymous("debug_deadmines_smite_falls", &[]);
}
