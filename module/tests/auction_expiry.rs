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
    // Auction Notices are a one-shot, TTL-reaped relay (see gc.rs), so check them once here, right
    // after the scheduled expiry settles — not after the slower steps below, where the reaper would
    // have already claimed the row on schedule.
    standalone.assert_call("debug_verify_auction_expiry_notices_fixture", &[]);

    for reducer in [
        "debug_replay_auction_expiry_fixture",
        "debug_verify_auction_expiry_fixture",
        "debug_verify_auction_expiry_fixture",
    ] {
        standalone.assert_call(reducer, &[]);
    }

    // Legacy auction mail (from before the vanilla Auction Mail format) is plain Character mail.
    // Stage one such row plus a real player's look-alike, then prove the post-publish repair
    // re-tags only the legacy row to the vanilla AuctionHouse sender, does so exactly once across
    // a repeated repair pass, and never touches the player's own mail.
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
