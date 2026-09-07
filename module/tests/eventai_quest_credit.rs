mod support;

use support::Standalone;

/// Runs EventAI quest authority against live durable rows. The verifier calls the production
/// `apply_eventai_credit` path for the tap roster, fallback, distance, duplicate, missing-objective,
/// atomic-recipient, and event-completion cases.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn eventai_quest_credit_keeps_pinned_recipient_and_completion_contracts() {
    let mut standalone = Standalone::start("eventai-quest-credit");
    standalone.publish_module();
    for (reducer, args) in [
        ("claim_operator", &[][..]),
        ("debug_spawn_player_entity", &["1"][..]),
        ("debug_verify_eventai_quest_credit_fixture", &[][..]),
    ] {
        standalone.assert_call(reducer, args);
    }
}
