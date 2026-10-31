mod support;

use support::Standalone;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn raid_quest_credit_keeps_the_raid_gate_and_party_parity() {
    let mut standalone = Standalone::start("raid-quest-credit");
    standalone.publish_module();
    for (reducer, args) in [
        ("claim_operator", &[][..]),
        ("install_guid_range", &["0"][..]),
        ("debug_spawn_player_entity", &["1"][..]),
        ("debug_verify_raid_quest_credit_fixture", &[][..]),
    ] {
        standalone.assert_call(reducer, args);
    }
}
