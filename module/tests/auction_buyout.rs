mod support;

use support::Standalone;

/// Runs only when requested because it builds and publishes the Wasm module to its own standalone.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn real_realm_reducer_commits_exact_buyout_mail_before_the_next_transaction() {
    let mut standalone = Standalone::start("auction-buyout");
    standalone.publish_module();
    for (reducer, args) in [
        ("claim_operator", &[][..]),
        ("install_guid_range", &["0"][..]),
        ("debug_stage_auction_buyout_fixture", &[][..]),
        (
            "realm_auction_decide_bid",
            // operation_id, bidder_guid, auction_id, house, offer — the fixture lists in house 1.
            &[
                "5090050",
                r#"{"guid":5090051,"ownership":null}"#,
                "5090050",
                "1",
                "900",
            ][..],
        ),
    ] {
        standalone.assert_call(reducer, args);
    }
    // Auction Notices are a one-shot, TTL-reaped relay (see gc.rs), so check each leg's notices
    // immediately after the decide call that fires them — not after the next decide call below,
    // where the reaper could have already claimed the row on schedule.
    standalone.assert_call("debug_verify_auction_buyout_notices_fixture", &[]);

    standalone.assert_call(
        "realm_auction_decide_bid",
        // The RemainActive fixture: no buyout, no prior bidder, so this offer settles
        // nothing and displaces nobody — exactly the New Bid notice path.
        &[
            "5090056",
            r#"{"guid":5090058,"ownership":null}"#,
            "5090056",
            "1",
            "60",
        ],
    );
    standalone.assert_call("debug_verify_auction_buyout_new_bid_notice_fixture", &[]);
    standalone.assert_call("debug_verify_auction_buyout_fixture", &[]);
}
