mod support;

use support::Standalone;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn eventai_quest_credit_keeps_pinned_recipient_and_completion_contracts() {
    let mut standalone = Standalone::start("eventai-quest-credit");
    standalone.publish_module();
    for (reducer, args) in [
        ("claim_operator", &[][..]),
        ("install_guid_range", &["0"][..]),
        ("debug_spawn_player_entity", &["1"][..]),
        ("debug_verify_eventai_quest_credit_fixture", &[][..]),
    ] {
        standalone.assert_call(reducer, args);
    }
}
