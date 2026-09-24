mod support;

use std::collections::BTreeMap;

use support::{actor, Standalone};

/// The staged listings (`debug_stage_auction_cancel_fixture`): a bid of 1000 at a 15% cut, no bid,
/// and a second bid of 1000.
const BID_LISTING: &str = "5090086";
const UNBID_LISTING: &str = "5090088";
const STALE_LISTING: &str = "5090089";
const BIDDER: &str = "5090087";
/// A seller with no Character in the world: Realm-core decides without reading any purse.
const REALM_SELLER: &str = "5090090";
/// The seeded Character, who pays the cut from a live purse in the single-database path.
const LOCAL_SELLER: &str = "1";
const VENDOR_ENTRY: &str = "51004";
const HOUSE: &str = "7";
/// 15% of the bid of 1000, truncated (`cm:AuctionHouseMgr.cpp:733-736`).
const CUT: &str = "150";

fn rows(standalone: &Standalone, query: &str) -> Vec<BTreeMap<String, String>> {
    standalone.query_rows(query)
}

fn auction_mail(standalone: &Standalone, recipient: &str) -> Vec<BTreeMap<String, String>> {
    let mut mail = rows(
        standalone,
        &format!(
            "SELECT subject, money, item_entry, item_stack_count, sender_kind, sender_entry, \
             check_flags, cod FROM game_mail WHERE recipient_guid = {recipient}"
        ),
    );
    mail.retain(|row| row["sender_kind"] == "2");
    mail.sort_by(|a, b| a["subject"].cmp(&b["subject"]));
    mail
}

fn listing_is_active(standalone: &Standalone, auction_id: &str) -> bool {
    let listed = !rows(
        standalone,
        &format!("SELECT id FROM game_auction WHERE id = {auction_id}"),
    )
    .is_empty();
    let scheduled = !rows(
        standalone,
        &format!("SELECT auction_id FROM game_auction_expiry WHERE auction_id = {auction_id}"),
    )
    .is_empty();
    assert_eq!(
        listed, scheduled,
        "listing {auction_id} and its expiry go together"
    );
    listed
}

fn decision(standalone: &Standalone, operation_id: &str) -> BTreeMap<String, String> {
    let mut found = rows(
        standalone,
        &format!(
            "SELECT outcome, operation, offer, accepted_price, result_bidder_guid, result_bid \
             FROM game_auction_bid_decision WHERE operation_id = {operation_id}"
        ),
    );
    assert_eq!(found.len(), 1, "one decision for {operation_id}");
    found.remove(0)
}

fn decide_cancel(
    standalone: &Standalone,
    operation_id: &str,
    seller: &str,
    auction_id: &str,
    house: &str,
    cut: &str,
) {
    standalone.assert_call(
        "realm_auction_decide_cancel",
        &[operation_id, &actor(seller), auction_id, house, cut],
    );
}

fn refused(standalone: &Standalone, reducer: &str, args: &[&str], tag: &str) {
    let result = standalone.call(reducer, args);
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !result.status.success() && output.contains(tag),
        "{reducer}: expected {tag}: {output}"
    );
}

fn purse(standalone: &Standalone) -> String {
    rows(
        standalone,
        &format!("SELECT money FROM game_world_entity WHERE guid = {LOCAL_SELLER}"),
    )[0]["money"]
        .clone()
}

/// The item back to the seller and the bid back to the bidder, as cmangos writes them
/// (`cm:AuctionHouseHandler.cpp:167-189,450-456`): subject `entry:random_property:action`, action 5
/// for the seller and 4 for the bidder, from house 7, COPIED (0x04), and no deposit anywhere.
fn assert_cancellation_mail(standalone: &Standalone, seller: &str) {
    let seller_mail = auction_mail(standalone, seller);
    assert_eq!(seller_mail.len(), 2, "{seller_mail:?}");
    for (row, subject, entry) in [
        (&seller_mail[0], "5090086:117:5", BID_LISTING),
        (&seller_mail[1], "5090088:117:5", UNBID_LISTING),
    ] {
        assert_eq!(row["subject"], subject);
        assert_eq!(row["money"], "0", "the house keeps the deposit");
        assert_eq!(row["item_entry"], entry);
        assert_eq!(row["item_stack_count"], "3");
        assert_eq!(row["sender_entry"], HOUSE);
        assert_eq!(row["check_flags"], "4");
        assert_eq!(row["cod"], "0");
    }
    let bidder_mail = auction_mail(standalone, BIDDER);
    assert_eq!(bidder_mail.len(), 1, "{bidder_mail:?}");
    assert_eq!(bidder_mail[0]["subject"], "5090086:117:4");
    assert_eq!(bidder_mail[0]["money"], "1000");
    assert_eq!(bidder_mail[0]["item_entry"], "0");
    assert_eq!(bidder_mail[0]["sender_entry"], HOUSE);
    assert_eq!(bidder_mail[0]["check_flags"], "4");

    let notices = rows(
        standalone,
        &format!(
            "SELECT kind, auction_id, item_entry, random_property_id FROM game_auction_notice \
             WHERE recipient_guid = {BIDDER}"
        ),
    );
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0]["kind"], "5", "Removed");
    assert_eq!(notices[0]["auction_id"], BID_LISTING);
    assert_eq!(notices[0]["item_entry"], BID_LISTING);
    assert_eq!(notices[0]["random_property_id"], "117");
    assert!(rows(
        standalone,
        &format!("SELECT id FROM game_auction_notice WHERE recipient_guid = {seller}")
    )
    .is_empty());
}

/// Runs only when requested because it builds and publishes the Wasm module to its own standalone.
/// One database plays the Home Shard and Realm-core.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_cancellation_returns_the_item_refunds_the_bidder_and_charges_the_cut_once() {
    let mut standalone = Standalone::start("auction-cancel");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    // Auction Notices are reaped about a second after insert. Without the reaper schedule every
    // notice this test writes stays until the test reads it.
    standalone.assert_sql("DELETE FROM game_event_reaper_schedule");

    // Realm-core's decision, as the sharded driver calls it.
    standalone.assert_call("debug_stage_auction_cancel_fixture", &[REALM_SELLER, "0"]);
    decide_cancel(
        &standalone,
        "5090086",
        REALM_SELLER,
        BID_LISTING,
        HOUSE,
        CUT,
    );
    decide_cancel(
        &standalone,
        "5090088",
        REALM_SELLER,
        UNBID_LISTING,
        HOUSE,
        "0",
    );
    // A bid after the read changed the cut; another player's listing; another market (house 1).
    decide_cancel(
        &standalone,
        "5090089",
        REALM_SELLER,
        STALE_LISTING,
        HOUSE,
        "149",
    );
    decide_cancel(&standalone, "5090090", BIDDER, STALE_LISTING, HOUSE, CUT);
    decide_cancel(
        &standalone,
        "5090091",
        REALM_SELLER,
        STALE_LISTING,
        "1",
        CUT,
    );

    assert!(!listing_is_active(&standalone, BID_LISTING));
    assert!(!listing_is_active(&standalone, UNBID_LISTING));
    assert!(listing_is_active(&standalone, STALE_LISTING));
    assert_cancellation_mail(&standalone, REALM_SELLER);
    let cancelled = decision(&standalone, "5090086");
    assert_eq!(
        [
            &cancelled["outcome"],
            &cancelled["operation"],
            &cancelled["offer"],
            &cancelled["accepted_price"],
            &cancelled["result_bidder_guid"],
            &cancelled["result_bid"],
        ],
        ["7", "1", CUT, CUT, BIDDER, "1000"]
    );
    let unbid = decision(&standalone, "5090088");
    assert_eq!(
        [
            &unbid["outcome"],
            &unbid["accepted_price"],
            &unbid["result_bid"]
        ],
        ["7", "0", "0"]
    );
    assert_eq!(decision(&standalone, "5090089")["outcome"], "6", "Database");
    assert_eq!(
        decision(&standalone, "5090090")["outcome"],
        "2",
        "not found"
    );
    assert_eq!(
        decision(&standalone, "5090091")["outcome"],
        "2",
        "not found"
    );

    // A replay changes nothing. A crossed payload or a bid under the same id is refused.
    decide_cancel(
        &standalone,
        "5090086",
        REALM_SELLER,
        BID_LISTING,
        HOUSE,
        CUT,
    );
    refused(
        &standalone,
        "realm_auction_decide_cancel",
        &["5090086", &actor(REALM_SELLER), BID_LISTING, HOUSE, "151"],
        "auction:database",
    );
    refused(
        &standalone,
        "realm_auction_decide_bid",
        &["5090086", &actor(REALM_SELLER), BID_LISTING, HOUSE, CUT],
        "auction:database",
    );
    assert_cancellation_mail(&standalone, REALM_SELLER);
    assert_eq!(
        rows(
            &standalone,
            "SELECT operation_id FROM game_auction_bid_decision"
        )
        .len(),
        5
    );

    // The single-database path: the seeded Character at an auctioneer, with a live purse.
    standalone.assert_call("debug_seed_scenario_fixtures", &[]);
    standalone.assert_call("debug_spawn_player_entity", &[LOCAL_SELLER]);
    standalone.assert_call("debug_spawn_at_feet", &[LOCAL_SELLER, VENDOR_ENTRY, "1"]);
    let vendor = rows(
        &standalone,
        &format!("SELECT guid FROM game_world_entity WHERE entry = {VENDOR_ENTRY}"),
    )[0]["guid"]
        .clone();
    standalone.assert_call(
        "debug_stage_auction_cancel_fixture",
        &[LOCAL_SELLER, &vendor],
    );
    let seller = actor(LOCAL_SELLER);
    let cancel_local = |operation_id: &str, auctioneer: &str, auction_id: &str, cut: &str| {
        [
            operation_id.to_string(),
            seller.clone(),
            auctioneer.to_string(),
            auction_id.to_string(),
            HOUSE.to_string(),
            cut.to_string(),
        ]
    };
    let call_local = |args: [String; 6]| {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        standalone.assert_call("gw_auction_cancel_local", &args);
    };

    // A seller who cannot pay the cut gets nothing: no Hold, no change.
    standalone.assert_call("debug_set_money", &[LOCAL_SELLER, "100"]);
    let poor = cancel_local("5090092", &vendor, BID_LISTING, CUT);
    refused(
        &standalone,
        "gw_auction_cancel_local",
        &poor.iter().map(String::as_str).collect::<Vec<_>>(),
        "auction:not_enough_money",
    );
    assert_eq!(purse(&standalone), "100");
    assert!(listing_is_active(&standalone, BID_LISTING));
    assert!(rows(
        &standalone,
        "SELECT operation_id FROM game_auction_bid_hold"
    )
    .is_empty());

    standalone.assert_call("debug_set_money", &[LOCAL_SELLER, "1000"]);
    call_local(cancel_local("5090092", &vendor, BID_LISTING, CUT));
    call_local(cancel_local("5090092", &vendor, BID_LISTING, CUT));
    call_local(cancel_local("5090093", &vendor, UNBID_LISTING, "0"));
    assert_eq!(purse(&standalone), "850", "the cut is charged once");
    assert!(!listing_is_active(&standalone, BID_LISTING));
    assert!(!listing_is_active(&standalone, UNBID_LISTING));
    assert_cancellation_mail(&standalone, LOCAL_SELLER);
    let hold = rows(
        &standalone,
        "SELECT outcome, operation, offer, deferred_refund FROM game_auction_bid_hold \
         WHERE operation_id = 5090092",
    );
    assert_eq!(
        [
            &hold[0]["outcome"],
            &hold[0]["operation"],
            &hold[0]["offer"],
            &hold[0]["deferred_refund"],
        ],
        ["7", "1", CUT, "0"]
    );

    // A stale cut refunds in the same transaction; an absent auctioneer holds nothing.
    call_local(cancel_local("5090094", &vendor, STALE_LISTING, "149"));
    assert_eq!(purse(&standalone), "850");
    assert!(listing_is_active(&standalone, STALE_LISTING));
    assert_eq!(decision(&standalone, "5090094")["outcome"], "6");
    let away = cancel_local("5090095", "5090099", STALE_LISTING, CUT);
    refused(
        &standalone,
        "gw_auction_cancel_local",
        &away.iter().map(String::as_str).collect::<Vec<_>>(),
        "auction:database",
    );
    assert_eq!(purse(&standalone), "850");
    assert!(rows(
        &standalone,
        "SELECT operation_id FROM game_auction_bid_hold WHERE operation_id = 5090095"
    )
    .is_empty());
}
