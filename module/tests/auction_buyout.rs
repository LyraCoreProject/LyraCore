mod support;

use support::Standalone;

/// Runs only when requested because it builds and publishes the Wasm module to its own standalone.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn real_realm_reducer_commits_exact_buyout_mail_before_the_next_transaction() {
    let mut standalone = Standalone::start("auction-buyout");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    // Auction Notices are a one-shot, TTL-reaped relay (see gc.rs). Disarming the shared reaper
    // schedule before staging keeps every notice this test writes around for as long as the test
    // needs it, so verification never has to race the reaper or depend on call order (the
    // playerbots durable tests disarm the same schedule for the same reason).
    standalone.assert_sql("DELETE FROM game_event_reaper_schedule");
    for (reducer, args) in [
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
        (
            "realm_auction_decide_bid",
            // The RemainActive fixture: no buyout, no prior bidder, so this offer settles
            // nothing and displaces nobody — exactly the New Bid notice path.
            &[
                "5090056",
                r#"{"guid":5090058,"ownership":null}"#,
                "5090056",
                "1",
                "60",
            ][..],
        ),
        ("debug_verify_auction_buyout_fixture", &[][..]),
    ] {
        standalone.assert_call(reducer, args);
    }
}
