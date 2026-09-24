mod support;

use support::Standalone;

/// Runs the raid quest-credit rule against live durable rows. The verifier stages a 12-member Raid
/// and a Party at reserved `509_297xxx` ids, then drives the production kill-credit,
/// quest-object-use, area-trigger-credit, quest-item-need and Loot Tag paths against both.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
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
