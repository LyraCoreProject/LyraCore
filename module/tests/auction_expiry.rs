mod support;

use support::Standalone;

/// Runs only when requested because it builds and publishes the Wasm module to its own standalone.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn scheduled_bid_expiry_settles_once_and_a_callback_replay_is_a_no_op() {
    let mut standalone = Standalone::start("auction-expiry");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_stage_auction_expiry_fixture", &[]);

    standalone.wait_until_call_succeeds("debug_verify_auction_expiry_fixture", &[]);

    for reducer in [
        "debug_replay_auction_expiry_fixture",
        "debug_verify_auction_expiry_fixture",
        "debug_verify_auction_expiry_fixture",
    ] {
        standalone.assert_call(reducer, &[]);
    }

    // A pre-T5 realm's auction mail is plain Character mail. Stage one such row, then prove the
    // post-publish repair re-tags it to the vanilla AuctionHouse sender, and does so exactly once
    // across a repeated repair pass.
    standalone.assert_call("debug_stage_legacy_auction_mail_fixture", &[]);

    standalone.publish_module();
    for reducer in [
        "debug_repair_after_publish",
        "debug_verify_auction_expiry_fixture",
        "debug_verify_legacy_auction_mail_repaired",
        "debug_repair_after_publish",
        "debug_verify_legacy_auction_mail_repaired",
    ] {
        standalone.assert_call(reducer, &[]);
    }
}
