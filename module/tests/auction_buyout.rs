mod support;

use support::Standalone;

/// Runs only when requested because it builds and publishes the Wasm module to its own standalone.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn real_realm_reducer_commits_exact_buyout_mail_before_the_next_transaction() {
    let standalone = Standalone::start("auction-buyout");
    standalone.publish_module();
    for (reducer, args) in [
        ("claim_operator", &[][..]),
        ("debug_stage_auction_buyout_fixture", &[][..]),
        (
            "realm_auction_decide_bid",
            &["5090050", "5090051", "5090050", "900"][..],
        ),
        ("debug_verify_auction_buyout_fixture", &[][..]),
    ] {
        standalone.assert_call(reducer, args);
    }
}
