mod support;

use std::thread;
use std::time::Duration;

use support::Standalone;

/// Exercises EventAI's production encounter notification boundary against the installed `dungeons`
/// Package. The verifier covers every binding and signal, including refusal outside the owning
/// map and outside an instance.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI, the Wasm toolchain, and the dungeons Package installed (lyracore packages add dungeons)"]
fn eventai_instance_signals_reach_package_owned_outcomes() {
    let standalone = Standalone::start("eventai-instance-packages");
    standalone.publish_module_anonymous();
    standalone.assert_call_anonymous("debug_verify_eventai_instance_packages", &[]);
    thread::sleep(Duration::from_secs(14));
    standalone.assert_call_anonymous("debug_verify_shadowfang_choreography", &[]);
    standalone.assert_call_anonymous("debug_verify_wailing_escort_and_begin_awakening", &[]);
    thread::sleep(Duration::from_millis(500));
    standalone.assert_call_anonymous("debug_verify_wailing_first_corner_dialogue", &[]);
    thread::sleep(Duration::from_millis(2_500));
    standalone.assert_call_anonymous("debug_verify_wailing_awakening", &[]);
    standalone.assert_call_anonymous("debug_prepare_wailing_exit", &[]);
    thread::sleep(Duration::from_secs(3));
    standalone.assert_call_anonymous("debug_verify_wailing_first_corner_continue", &[]);
    standalone.assert_call_anonymous("debug_verify_wailing_exit_and_prepare_cleanup", &[]);
    thread::sleep(Duration::from_secs(14));
    standalone.assert_call_anonymous(
        "debug_verify_shadowfang_dark_offering_and_prepare_restart",
        &[],
    );
    standalone.assert_call_anonymous("debug_verify_wailing_cleanup", &[]);
    thread::sleep(Duration::from_secs(1));
    standalone.assert_call_anonymous("debug_verify_shadowfang_restart_recovery", &[]);
    standalone.assert_call_anonymous("debug_begin_tomb_round_scheduler", &[]);
    thread::sleep(Duration::from_secs(35));
    standalone.assert_call_anonymous("debug_verify_tomb_round_scheduler", &[]);
}
