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

/// A Cancellation Hold crosses a Shard Boundary with its Character and settles on the new Home
/// Shard, so a Gateway stop before the decision never strands the cut on the old one.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn an_unfinished_cancellation_hold_travels_with_its_seller_and_settles_on_the_new_home_shard() {
    let mut source = Standalone::start("auction-cancel-source");
    source.publish_module();
    source.assert_call("claim_operator", &[]);
    source.assert_call("install_guid_range", &["0"]);
    source.assert_call("debug_seed_scenario_fixtures", &[]);
    source.assert_call("debug_spawn_player_entity", &[LOCAL_SELLER]);
    source.assert_call("debug_set_money", &[LOCAL_SELLER, "1000"]);
    source.assert_call("debug_spawn_at_feet", &[LOCAL_SELLER, VENDOR_ENTRY, "1"]);
    let vendor = rows(
        &source,
        &format!("SELECT guid FROM game_world_entity WHERE entry = {VENDOR_ENTRY}"),
    )[0]["guid"]
        .clone();
    source.assert_call(
        "debug_stage_auction_cancel_fixture",
        &[LOCAL_SELLER, &vendor],
    );
    let seller = actor(LOCAL_SELLER);

    // Phase 1 only: the Gateway stopped before Realm-core decided.
    source.assert_call(
        "gw_auction_hold_cancel",
        &["5090092", &seller, &vendor, BID_LISTING, HOUSE, CUT],
    );
    assert_eq!(purse(&source), "850");
    source.assert_call(
        "begin_transfer",
        &["5090093", &seller, "0", "0", "0", "0", "0", "0", "true"],
    );
    let out = rows(
        &source,
        "SELECT blob FROM game_transfer_out WHERE transfer_id = 5090093",
    );
    let blob = serde_json::to_string(out[0]["blob"].strip_prefix("0x").unwrap()).unwrap();

    let mut destination = Standalone::start("auction-cancel-destination");
    destination.publish_module();
    destination.assert_call("claim_operator", &[]);
    destination.assert_call("install_guid_range", &["1000000000"]);
    let system = actor("0");
    destination.assert_call("import_character_blob", &["5090093", &blob, &system]);
    source.assert_call("confirm_import", &["5090093", &system]);
    source.assert_call("finish_transfer", &["5090093", &system]);
    destination.assert_call("release_transfer", &["5090093", &system]);

    let hold_query =
        "SELECT outcome, operation, offer, deferred_refund FROM game_auction_bid_hold \
                      WHERE operation_id = 5090092";
    assert!(
        rows(&source, hold_query).is_empty(),
        "the Hold left with its seller"
    );
    let arrived = rows(&destination, hold_query);
    assert_eq!(arrived.len(), 1);
    assert_eq!(
        [
            &arrived[0]["outcome"],
            &arrived[0]["operation"],
            &arrived[0]["offer"],
        ],
        ["0", "1", CUT]
    );

    // The source plays Realm-core and decides; the new Home Shard spends the Hold once.
    source.assert_call(
        "realm_auction_decide_cancel",
        &["5090092", &seller, BID_LISTING, HOUSE, CUT],
    );
    assert_eq!(decision(&source, "5090092")["outcome"], "7");
    destination.assert_call("debug_spawn_player_entity", &[LOCAL_SELLER]);
    let finish: [&str; 11] = [
        "5090092",
        &seller,
        BID_LISTING,
        HOUSE,
        CUT,
        "7",
        "0",
        BIDDER,
        "1000",
        "0",
        CUT,
    ];
    destination.assert_call("gw_auction_finish_bid", &finish);
    destination.assert_call("gw_auction_finish_bid", &finish);
    assert_eq!(purse(&destination), "850", "the cut is spent once");
    let settled = rows(&destination, hold_query);
    assert_eq!(
        [&settled[0]["outcome"], &settled[0]["deferred_refund"]],
        ["7", "0"]
    );
}

/// A listing Hold crosses a Shard Boundary with its seller and lists from the new Home Shard, so a
/// Gateway stop between the Hold and Realm-core's commit never strands the item or the deposit on
/// the old one. The Hold carries a Letter Copy's text id with the item.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn an_unfinished_listing_hold_travels_with_its_seller_and_lists_from_the_new_home_shard() {
    let mut source = Standalone::start("auction-listing-source");
    source.publish_module();
    source.assert_call("claim_operator", &[]);
    source.assert_call("install_guid_range", &["0"]);
    source.assert_call("debug_seed_scenario_fixtures", &[]);
    source.assert_call("debug_spawn_player_entity", &[LOCAL_SELLER]);
    source.assert_call("debug_set_money", &[LOCAL_SELLER, "1000"]);
    source.assert_call("debug_spawn_at_feet", &[LOCAL_SELLER, VENDOR_ENTRY, "1"]);
    let vendor = rows(
        &source,
        &format!("SELECT guid FROM game_world_entity WHERE entry = {VENDOR_ENTRY}"),
    )[0]["guid"]
        .clone();
    source.assert_call(
        "debug_stage_auction_cancel_fixture",
        &[LOCAL_SELLER, &vendor],
    );
    source.assert_sql("DELETE FROM game_item_instance WHERE owner_guid = 1");
    // Five Tough Jerky (sell price 2), marked as a Letter Copy's letter so the text id rides along.
    source.assert_call("debug_grant_item", &[LOCAL_SELLER, "5090052", "5"]);
    source.assert_sql("UPDATE game_item_instance SET item_text_id = 41 WHERE owner_guid = 1");
    let item = rows(
        &source,
        "SELECT guid FROM game_item_instance WHERE owner_guid = 1",
    )[0]["guid"]
        .clone();
    let seller = actor(LOCAL_SELLER);

    // Phase 1 only: the Gateway stopped before Realm-core committed the listing.
    source.assert_call(
        "gw_auction_hold_listing",
        &["5090094", &seller, &item, &vendor, HOUSE, "100", "0", "720"],
    );
    let hold_query = "SELECT * FROM game_auction_hold WHERE operation_id = 5090094";
    let held = rows(&source, hold_query);
    assert_eq!(held.len(), 1);
    assert_eq!(
        [
            &held[0]["item_entry"],
            &held[0]["item_stack_count"],
            &held[0]["item_text_id"],
        ],
        ["5090052", "5", "41"]
    );
    let purse_after_deposit = purse(&source);
    source.assert_call(
        "begin_transfer",
        &["5090095", &seller, "0", "0", "0", "0", "0", "0", "true"],
    );
    let out = rows(
        &source,
        "SELECT blob FROM game_transfer_out WHERE transfer_id = 5090095",
    );
    let blob = serde_json::to_string(out[0]["blob"].strip_prefix("0x").unwrap()).unwrap();

    let mut destination = Standalone::start("auction-listing-destination");
    destination.publish_module();
    destination.assert_call("claim_operator", &[]);
    destination.assert_call("install_guid_range", &["1000000000"]);
    let system = actor("0");
    destination.assert_call("import_character_blob", &["5090095", &blob, &system]);
    source.assert_call("confirm_import", &["5090095", &system]);
    source.assert_call("finish_transfer", &["5090095", &system]);
    destination.assert_call("release_transfer", &["5090095", &system]);

    assert!(
        rows(&source, hold_query).is_empty(),
        "the Hold left with its seller"
    );
    assert_eq!(
        rows(&destination, hold_query),
        held,
        "the Hold arrives with every column"
    );

    // The source plays Realm-core and commits the held listing; the new Home Shard settles it.
    let hold = &held[0];
    let commit: Vec<&str> = [
        "operation_id",
        "",
        "item_guid",
        "item_entry",
        "item_stack_count",
        "item_durability",
        "item_enchant_id",
        "item_soulbound",
        "random_property_id",
        "house",
        "deposit_rate",
        "consignment_rate",
        "start_bid",
        "buyout",
        "duration_minutes",
        "deposit",
        "created_micros",
        "expires_micros",
        "item_text_id",
    ]
    .iter()
    .map(|column| {
        if column.is_empty() {
            seller.as_str()
        } else {
            hold[*column].as_str()
        }
    })
    .collect();
    source.assert_call("realm_auction_commit_listing", &commit);
    let listed = rows(
        &source,
        "SELECT id, item_entry, item_text_id FROM game_auction WHERE listing_operation_id = 5090094",
    );
    assert_eq!(
        [&listed[0]["item_entry"], &listed[0]["item_text_id"]],
        ["5090052", "41"],
        "the Auction carries the letter's text id"
    );
    destination.assert_call(
        "realm_auction_confirm_listing",
        &["5090094", &listed[0]["id"], &seller],
    );
    destination.assert_call("realm_auction_settle_listing", &["5090094", &seller]);
    assert!(rows(&destination, hold_query).is_empty(), "settled once");
    destination.assert_call("debug_spawn_player_entity", &[LOCAL_SELLER]);
    assert_eq!(
        purse(&destination),
        purse_after_deposit,
        "the deposit was taken once, before the Transfer"
    );
}
