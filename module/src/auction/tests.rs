use super::*;

fn policy() -> AuctionHousePolicy {
    AuctionHousePolicy {
        id: 1,
        deposit_rate: 5,
        consignment_rate: 5,
    }
}

#[test]
fn seller_proceeds_apply_the_imported_cut_and_return_the_deposit_checked() {
    assert_eq!(seller_proceeds(100, 10, 5), Some(105));
    assert_eq!(seller_proceeds(100, 10, 15), Some(95));
    assert_eq!(seller_proceeds(19, 1, 5), Some(20));
    assert_eq!(seller_proceeds(20, 1, 5), Some(20));
    assert_eq!(seller_proceeds(u32::MAX, u32::MAX, 5), None);
    assert_eq!(seller_proceeds(100, 10, 101), None);
}

#[test]
fn the_subject_is_the_item_the_random_property_and_the_mail_auction_answers_code() {
    assert_eq!(
        auction_mail_subject(1234, 56, AuctionMailAction::Outbid),
        "1234:56:0"
    );
    assert_eq!(
        auction_mail_subject(1234, 56, AuctionMailAction::Won),
        "1234:56:1"
    );
    assert_eq!(
        auction_mail_subject(1234, 56, AuctionMailAction::Successful),
        "1234:56:2"
    );
    assert_eq!(
        auction_mail_subject(1234, 56, AuctionMailAction::Expired),
        "1234:56:3"
    );
    assert_eq!(
        auction_mail_subject(1234, 56, AuctionMailAction::CancelledToBidder),
        "1234:56:4"
    );
    assert_eq!(
        auction_mail_subject(1234, 0, AuctionMailAction::Cancelled),
        "1234:0:5",
        "a plain item's random property id is 0"
    );
}

fn mail_for_body(action: AuctionMailAction) -> AuctionMail {
    AuctionMail {
        recipient_guid: 1,
        house: 1,
        action,
        item_entry: 1234,
        random_property_id: 56,
        money: 0,
        attached_item: crate::items::ItemSnapshot::default(),
        counterparty_guid: 0,
        bid: 0,
        buyout: 0,
        deposit: 0,
        cut: 0,
    }
}

#[test]
fn the_won_and_successful_bodies_are_the_padded_invoice_cmangos_sends() {
    let won = AuctionMail {
        counterparty_guid: 9,
        bid: 100,
        buyout: 500,
        ..mail_for_body(AuctionMailAction::Won)
    };
    assert_eq!(
        auction_mail_body(&won),
        format!("{}9:100:500", " ".repeat(15)),
        "guid 9 is one hex digit, padded with 15 leading spaces to a 16-wide field"
    );

    let successful = AuctionMail {
        counterparty_guid: 0x123,
        bid: 250,
        buyout: 0,
        deposit: 12,
        cut: 6,
        ..mail_for_body(AuctionMailAction::Successful)
    };
    assert_eq!(
        auction_mail_body(&successful),
        format!("{}123:250:0:12:6", " ".repeat(13)),
        "guid 0x123 is three hex digits, padded with 13 leading spaces"
    );
}

#[test]
fn outbid_expired_and_cancelled_mail_carries_no_body() {
    for action in [
        AuctionMailAction::Outbid,
        AuctionMailAction::Expired,
        AuctionMailAction::CancelledToBidder,
        AuctionMailAction::Cancelled,
    ] {
        assert_eq!(auction_mail_body(&mail_for_body(action)), "");
    }
}

#[test]
fn outbid_notice_fires_only_for_a_real_displaced_bidder() {
    let item = crate::items::ItemSnapshot {
        entry: 1234,
        random_property_id: 56,
        ..crate::items::ItemSnapshot::default()
    };
    assert_eq!(
        outbid_notice(1, 41, item, 0, 8, 0),
        None,
        "a fresh listing's first bid displaces nobody"
    );
    assert_eq!(
        outbid_notice(1, 41, item, 9, 8, 100),
        Some(AuctionNoticeDraft {
            recipient_guid: 9,
            kind: auction_notice::OUTBID,
            house: 1,
            auction_id: 41,
            item_entry: 1234,
            random_property_id: 56,
            bid: 100,
            out_bid: 5,
            bidder_guid: 9,
        })
    );
}

#[test]
fn a_bidder_who_raises_their_own_bid_gets_no_outbid_notice() {
    let item = crate::items::ItemSnapshot {
        entry: 1234,
        random_property_id: 56,
        ..crate::items::ItemSnapshot::default()
    };
    assert_eq!(
        outbid_notice(1, 41, item, 9, 9, 100),
        None,
        "the displaced bidder and the new bidder are the same Character"
    );
}

#[test]
fn new_bid_and_expired_notices_address_the_owner() {
    let item = crate::items::ItemSnapshot {
        entry: 1234,
        random_property_id: 56,
        ..crate::items::ItemSnapshot::default()
    };
    assert_eq!(
        new_bid_notice(1, 41, item, 7, 9, 107),
        AuctionNoticeDraft {
            recipient_guid: 7,
            kind: auction_notice::NEW_BID,
            house: 1,
            auction_id: 41,
            item_entry: 1234,
            random_property_id: 56,
            bid: 107,
            out_bid: 6,
            bidder_guid: 9,
        }
    );
    assert_eq!(
        expired_notice(1, 41, item, 7),
        AuctionNoticeDraft {
            recipient_guid: 7,
            kind: auction_notice::EXPIRED,
            house: 1,
            auction_id: 41,
            item_entry: 1234,
            random_property_id: 56,
            bid: 0,
            out_bid: 1,
            bidder_guid: 0,
        }
    );
}

#[test]
fn settlement_notices_are_won_to_the_buyer_and_sold_to_the_seller() {
    let item = crate::items::ItemSnapshot {
        entry: 1234,
        random_property_id: 56,
        ..crate::items::ItemSnapshot::default()
    };
    let [won, sold] = settlement_notices(1, 41, item.entry, item.random_property_id, 7, 9, 500);
    assert_eq!(
        won,
        AuctionNoticeDraft {
            recipient_guid: 9,
            kind: auction_notice::WON,
            house: 1,
            auction_id: 41,
            item_entry: 1234,
            random_property_id: 56,
            bid: 0,
            out_bid: 25,
            bidder_guid: 9,
        }
    );
    assert_eq!(
        sold,
        AuctionNoticeDraft {
            recipient_guid: 7,
            kind: auction_notice::SOLD,
            house: 1,
            auction_id: 41,
            item_entry: 1234,
            random_property_id: 56,
            bid: 500,
            out_bid: 25,
            bidder_guid: 0,
        }
    );
}

#[test]
fn every_legacy_auction_subject_maps_to_its_action_and_an_unknown_one_does_not() {
    assert_eq!(
        legacy_auction_action("Auction outbid"),
        Some(AuctionMailAction::Outbid)
    );
    assert_eq!(
        legacy_auction_action("Auction won"),
        Some(AuctionMailAction::Won)
    );
    assert_eq!(
        legacy_auction_action("Auction sold"),
        Some(AuctionMailAction::Successful)
    );
    assert_eq!(
        legacy_auction_action("Auction expired"),
        Some(AuctionMailAction::Expired)
    );
    assert_eq!(
        legacy_auction_action("Auction listing refused"),
        Some(AuctionMailAction::Cancelled)
    );
    assert_eq!(
        legacy_auction_action("Auction bid refund"),
        Some(AuctionMailAction::Outbid),
        "the deferred bid refund has no vanilla twin and renders as Outbid"
    );
    assert_eq!(legacy_auction_action("Auction House of Cards"), None);
    assert_eq!(legacy_auction_action("meet me at the gate"), None);
}

#[test]
fn listing_deposit_uses_the_imported_rate_and_supported_duration_ladder() {
    assert_eq!(listing_deposit(100, 2, 120, 5), Some(10));
    assert_eq!(listing_deposit(100, 2, 480, 5), Some(40));
    assert_eq!(listing_deposit(100, 2, 1_440, 5), Some(120));
    assert_eq!(listing_deposit(100, 2, 120, 25), Some(50));
    assert_eq!(listing_deposit(1, 1, 120, 5), Some(1));
    assert_eq!(
        listing_deposit(100, 1, 60, 5),
        None,
        "no client offers a 1-hour listing"
    );
    assert_eq!(
        listing_deposit(100, 1, 720, 5),
        None,
        "720 is the TBC 12-hour duration; no 1.12 client sends it"
    );
    assert_eq!(
        listing_deposit(100, 1, 2_880, 5),
        None,
        "2_880 is the TBC 48-hour duration; no 1.12 client sends it"
    );
    assert_eq!(listing_deposit(u32::MAX, u32::MAX, 1_440, 5), None);
    assert_eq!(listing_deposit(100, 1, 120, 101), None);
}

fn item(slot: u8) -> ListingItem {
    ListingItem {
        guid: 70,
        owner_guid: 7,
        slot,
        mailable: true,
        snapshot: crate::items::ItemSnapshot {
            entry: 25,
            stack_count: 2,
            durability: 17,
            enchant_id: 9,
            soulbound: false,
            random_property_id: 117,
            item_text_id: 0,
        },
        sell_price: 100,
    }
}

fn active_bid_auction() -> BidAuction {
    let snapshot = item(23).snapshot;
    BidAuction {
        id: 41,
        house: 1,
        owner_guid: 7,
        item: snapshot,
        highest_bidder_guid: 0,
        highest_bid: 0,
        start_bid: 100,
        buyout: 0,
        deposit: 10,
        consignment_rate: 5,
        expires_micros: 2_000,
        revision: 3,
    }
}

fn accepted_active(
    price: u32,
    revision: u64,
    displaced_bidder_guid: u64,
    displaced_bid: u32,
) -> BidDecision {
    BidDecision::Accepted(BidAcceptance {
        price,
        effect: AuctionBidEffect::RemainActive { revision },
        displaced_bidder_guid,
        displaced_bid,
    })
}

fn accepted_buyout(price: u32, displaced_bidder_guid: u64, displaced_bid: u32) -> BidDecision {
    BidDecision::Accepted(BidAcceptance {
        price,
        effect: AuctionBidEffect::SettleBuyout,
        displaced_bidder_guid,
        displaced_bid,
    })
}

fn terms() -> ListingTerms {
    ListingTerms {
        start_bid: 10,
        buyout: 20,
        duration_minutes: 120,
    }
}

#[test]
fn listing_accepts_only_one_owned_mailable_bag_stack_and_valid_terms() {
    assert_eq!(
        prepare_listing(Some(&item(23)), 7, 10, terms(), policy()),
        Ok(10)
    );
    assert_eq!(
        prepare_listing(Some(&item(120)), 7, 10, terms(), policy()),
        Ok(10)
    );

    for slot in [15, 19, 39, 63, 119, 192] {
        assert_eq!(
            prepare_listing(Some(&item(slot)), 7, 10, terms(), policy()),
            Err(AuctionRefusal::ItemNotFound),
            "slot {slot} must not be auctionable"
        );
    }

    let mut foreign = item(23);
    foreign.owner_guid = 8;
    assert_eq!(
        prepare_listing(Some(&foreign), 7, 10, terms(), policy()),
        Err(AuctionRefusal::ItemNotFound)
    );

    let mut soulbound = item(23);
    soulbound.snapshot.soulbound = true;
    assert_eq!(
        prepare_listing(Some(&soulbound), 7, 10, terms(), policy()),
        Err(AuctionRefusal::ItemNotFound)
    );

    let mut not_mailable = item(120);
    not_mailable.mailable = false;
    assert_eq!(
        prepare_listing(Some(&not_mailable), 7, 10, terms(), policy()),
        Err(AuctionRefusal::ItemNotFound)
    );

    let mut letter = item(23);
    letter.snapshot.item_text_id = 8;
    assert_eq!(
        prepare_listing(Some(&letter), 7, 10, terms(), policy()),
        Ok(10),
        "a Letter Copy's Plain Letter lists, and its text id travels in the snapshot"
    );

    assert_eq!(
        prepare_listing(None, 7, 10, terms(), policy()),
        Err(AuctionRefusal::ItemNotFound)
    );
    assert_eq!(
        prepare_listing(Some(&item(23)), 7, 9, terms(), policy()),
        Err(AuctionRefusal::NotEnoughMoney)
    );

    for invalid in [
        ListingTerms {
            start_bid: 0,
            ..terms()
        },
        ListingTerms {
            start_bid: 20,
            buyout: 19,
            ..terms()
        },
        ListingTerms {
            duration_minutes: 60,
            ..terms()
        },
        ListingTerms {
            duration_minutes: 720,
            ..terms()
        },
        ListingTerms {
            duration_minutes: 2_880,
            ..terms()
        },
    ] {
        assert_eq!(
            prepare_listing(Some(&item(23)), 7, 10, invalid, policy()),
            Err(AuctionRefusal::InvalidTerms),
            "{invalid:?} must not list: the 1.12 client never sends this duration"
        );
    }
}

#[derive(Clone)]
struct FakeLocal {
    money: Option<u32>,
    item: Option<ListingItem>,
    now_micros: i64,
    next_auction_id: u32,
    committed: Option<(PreparedListing, u32)>,
}

impl ListingSource for FakeLocal {
    fn seller_money(&self, _seller_guid: u64) -> Option<u32> {
        self.money
    }

    fn item(&self, _item_guid: u64) -> Option<ListingItem> {
        self.item.clone()
    }

    fn now_micros(&self) -> i64 {
        self.now_micros
    }
}

impl LocalListingSink for FakeLocal {
    fn receipt(&self, operation_id: u64) -> Option<ListingReceipt> {
        self.committed
            .as_ref()
            .filter(|(listing, _)| listing.request.operation_id == operation_id)
            .map(|(listing, auction_id)| ListingReceipt {
                listing: listing.clone(),
                auction_id: *auction_id,
            })
    }

    fn commit_local(&mut self, listing: PreparedListing) -> u32 {
        self.money = self.money.map(|money| money - listing.deposit);
        self.item = None;
        let auction_id = self.next_auction_id;
        self.next_auction_id += 1;
        self.committed = Some((listing, auction_id));
        auction_id
    }
}

fn request() -> ListingRequest {
    ListingRequest {
        operation_id: 900,
        seller_guid: 7,
        item_guid: 70,
        house: policy(),
        terms: terms(),
    }
}

fn local() -> FakeLocal {
    FakeLocal {
        money: Some(50),
        item: Some(item(23)),
        now_micros: 1_000,
        next_auction_id: 41,
        committed: None,
    }
}

#[test]
fn local_listing_atomically_moves_the_exact_item_and_deposit_into_one_scheduled_auction() {
    let mut store = local();

    assert_eq!(create_local_listing(&mut store, request()), Ok(41));

    assert_eq!(store.money, Some(40));
    assert!(store.item.is_none());
    let (listing, auction_id) = store.committed.as_ref().expect("one committed Auction");
    assert_eq!(*auction_id, 41);
    assert_eq!(listing.snapshot, item(23).snapshot);
    assert_eq!(listing.deposit, 10);
    assert_eq!(listing.created_micros, 1_000);
    assert_eq!(listing.expires_micros, 7_200_001_000);

    assert_eq!(create_local_listing(&mut store, request()), Ok(41));
    assert_eq!(store.money, Some(40), "replay must not charge again");
    assert_eq!(store.next_auction_id, 42, "replay must not mint another id");
}

#[test]
fn local_listing_refusals_leave_item_purse_and_market_unchanged() {
    let cases = [
        {
            let mut store = local();
            store.money = Some(9);
            (store, request())
        },
        {
            let mut bad = request();
            bad.terms.duration_minutes = 60;
            (local(), bad)
        },
        {
            let mut bad = request();
            bad.operation_id = 0;
            (local(), bad)
        },
    ];

    for (mut store, request) in cases {
        let before = store.clone();
        assert!(create_local_listing(&mut store, request).is_err());
        assert_eq!(store.money, before.money);
        assert_eq!(store.item, before.item);
        assert_eq!(store.next_auction_id, before.next_auction_id);
        assert!(store.committed.is_none());
    }
}

#[test]
fn listing_rejects_overflowing_possible_proceeds_before_moving_value() {
    for terms in [
        ListingTerms {
            start_bid: u32::MAX,
            buyout: 0,
            duration_minutes: 120,
        },
        ListingTerms {
            start_bid: 10,
            buyout: u32::MAX,
            duration_minutes: 120,
        },
    ] {
        let mut local = local();
        local.money = Some(u32::MAX);
        local.item.as_mut().unwrap().sell_price = u32::MAX;
        let before = local.clone();
        let request = ListingRequest { terms, ..request() };

        assert_eq!(
            create_local_listing(&mut local, request),
            Err(AuctionRefusal::InvalidTerms)
        );
        assert_eq!(local.money, before.money);
        assert_eq!(local.item, before.item);
        assert!(local.committed.is_none());
    }
}

#[derive(Clone)]
struct FakeExpiry {
    auction: Option<ActiveAuction>,
    schedule_count: usize,
    mail: Vec<AuctionMail>,
    notices: Vec<AuctionNoticeDraft>,
    returned_items: Vec<crate::items::ItemSnapshot>,
    refunded_copper: u32,
}

impl ExpirySink for FakeExpiry {
    fn auction(&self, auction_id: u32) -> Result<Option<ActiveAuction>, String> {
        Ok(self
            .auction
            .as_ref()
            .filter(|auction| auction.id == auction_id)
            .cloned())
    }

    fn complete_expiry(&mut self, auction: ActiveAuction, completion: ExpiryCompletion) {
        let house = auction.listing.request.house.id;
        let item = auction.listing.snapshot;
        let owner_guid = auction.listing.request.seller_guid;
        match completion {
            ExpiryCompletion::Unsold(mail) => {
                self.returned_items.push(mail.attached_item);
                self.notices
                    .push(expired_notice(house, auction.id, item, owner_guid));
                self.mail.push(mail);
            }
            ExpiryCompletion::Sold(mail) => {
                self.notices.extend(settlement_notices(
                    house,
                    auction.id,
                    item.entry,
                    item.random_property_id,
                    owner_guid,
                    auction.highest_bidder_guid,
                    auction.highest_bid,
                ));
                self.mail.extend(mail);
            }
        }
        self.auction = None;
        self.schedule_count = 0;
    }
}

#[test]
fn replayed_unbid_expiry_returns_the_exact_item_once_and_forfeits_the_deposit() {
    let listing = PreparedListing {
        request: request(),
        snapshot: item(23).snapshot,
        deposit: 10,
        created_micros: 1_000,
        expires_micros: 7_200_001_000,
    };
    let mut store = FakeExpiry {
        auction: Some(ActiveAuction {
            id: 41,
            listing,
            highest_bidder_guid: 0,
            highest_bid: 0,
        }),
        schedule_count: 1,
        mail: Vec::new(),
        notices: Vec::new(),
        returned_items: Vec::new(),
        refunded_copper: 0,
    };

    assert_eq!(expire_active(&mut store, 41), Ok(()));
    assert_eq!(expire_active(&mut store, 41), Ok(()));

    assert!(store.auction.is_none());
    assert_eq!(store.schedule_count, 0);
    assert_eq!(store.returned_items, vec![item(23).snapshot]);
    assert_eq!(store.refunded_copper, 0, "the listing deposit is forfeited");
}

#[test]
fn replayed_bid_expiry_mails_the_exact_item_and_imported_rate_proceeds_once() {
    let listing = PreparedListing {
        request: request(),
        snapshot: item(23).snapshot,
        deposit: 10,
        created_micros: 1_000,
        expires_micros: 7_200_001_000,
    };
    let mut store = FakeExpiry {
        auction: Some(ActiveAuction {
            id: 41,
            listing,
            highest_bidder_guid: 8,
            highest_bid: 201,
        }),
        schedule_count: 1,
        mail: Vec::new(),
        notices: Vec::new(),
        returned_items: Vec::new(),
        refunded_copper: 0,
    };

    assert_eq!(expire_active(&mut store, 41), Ok(()));
    assert_eq!(expire_active(&mut store, 41), Ok(()));

    assert!(store.auction.is_none());
    assert_eq!(store.schedule_count, 0);
    assert_eq!(store.returned_items, Vec::new());
    assert_eq!(
        store.mail,
        vec![
            AuctionMail {
                recipient_guid: 8,
                house: 1,
                action: AuctionMailAction::Won,
                item_entry: 25,
                random_property_id: 117,
                money: 0,
                attached_item: item(23).snapshot,
                counterparty_guid: 7,
                bid: 201,
                buyout: 20,
                deposit: 0,
                cut: 0,
            },
            AuctionMail {
                recipient_guid: 7,
                house: 1,
                action: AuctionMailAction::Successful,
                item_entry: 25,
                random_property_id: 117,
                money: 201,
                attached_item: crate::items::ItemSnapshot::default(),
                counterparty_guid: 8,
                bid: 201,
                buyout: 20,
                deposit: 10,
                cut: 10,
            },
        ],
        "one Sold mail settles at the LISTING's buyout term, not the expiring bid"
    );
    assert_eq!(
        store.notices,
        vec![
            AuctionNoticeDraft {
                recipient_guid: 8,
                kind: auction_notice::WON,
                house: 1,
                auction_id: 41,
                item_entry: 25,
                random_property_id: 117,
                bid: 0,
                out_bid: 11,
                bidder_guid: 8,
            },
            AuctionNoticeDraft {
                recipient_guid: 7,
                kind: auction_notice::SOLD,
                house: 1,
                auction_id: 41,
                item_entry: 25,
                random_property_id: 117,
                bid: 201,
                out_bid: 11,
                bidder_guid: 0,
            },
        ],
        "a replayed expiry callback must not write a second notice"
    );
}

#[test]
fn bid_decision_enforces_the_vanilla_price_ladder_and_revision() {
    let active = active_bid_auction();
    let request = BidRequest {
        operation: BidOperation::Bid,
        operation_id: 900,
        bidder_guid: 8,
        auction_id: 41,
        house: 1,
        offer: 100,
    };

    assert_eq!(
        decide_bid(Some(active), request, 1_000),
        accepted_active(100, 4, 0, 0)
    );

    let bid = BidAuction {
        highest_bidder_guid: 9,
        highest_bid: 101,
        ..active
    };
    assert_eq!(minimum_next_bid(bid), Ok(107));
    assert_eq!(
        decide_bid(
            Some(bid),
            BidRequest {
                offer: 101,
                ..request
            },
            1_000
        ),
        BidDecision::HigherBid {
            bidder_guid: 9,
            current_bid: 101,
            minimum_increment: 6,
        }
    );
    assert_eq!(
        decide_bid(
            Some(bid),
            BidRequest {
                offer: 106,
                ..request
            },
            1_000
        ),
        BidDecision::BidIncrement
    );
    assert_eq!(
        decide_bid(
            Some(active),
            BidRequest {
                offer: 99,
                ..request
            },
            1_000
        ),
        BidDecision::BidIncrement
    );
    assert_eq!(
        decide_bid(
            Some(BidAuction {
                owner_guid: 8,
                ..active
            }),
            request,
            1_000,
        ),
        BidDecision::BidOwn
    );
    assert_eq!(decide_bid(None, request, 1_000), BidDecision::ItemNotFound);
    assert_eq!(
        decide_bid(
            Some(active),
            BidRequest {
                house: 7,
                ..request
            },
            1_000,
        ),
        BidDecision::ItemNotFound
    );
    assert_eq!(
        decide_bid(
            Some(BidAuction {
                expires_micros: 1_000,
                ..active
            }),
            request,
            1_000,
        ),
        BidDecision::ItemNotFound
    );
    assert_eq!(
        decide_bid(
            Some(active),
            BidRequest {
                operation_id: 0,
                ..request
            },
            1_000
        ),
        BidDecision::Database
    );
    assert!(matches!(
        decide_bid(
            Some(BidAuction {
                highest_bid: u32::MAX,
                buyout: 0,
                ..bid
            }),
            request,
            1_000,
        ),
        BidDecision::Database
    ));
}

#[test]
fn bid_decision_accepts_a_bid_within_the_listings_market_and_refuses_outside_it() {
    let active = active_bid_auction();
    let request = BidRequest {
        operation: BidOperation::Bid,
        operation_id: 900,
        bidder_guid: 8,
        auction_id: 41,
        house: 2,
        offer: 100,
    };

    assert_eq!(
        decide_bid(Some(active), request, 1_000),
        accepted_active(100, 4, 0, 0)
    );
    assert_eq!(
        decide_bid(
            Some(active),
            BidRequest {
                house: 3,
                ..request
            },
            1_000
        ),
        accepted_active(100, 4, 0, 0)
    );
    for horde_house in [4, 5, 6] {
        assert_eq!(
            decide_bid(
                Some(active),
                BidRequest {
                    house: horde_house,
                    ..request
                },
                1_000
            ),
            BidDecision::ItemNotFound
        );
    }
    assert_eq!(
        decide_bid(
            Some(active),
            BidRequest {
                house: 7,
                ..request
            },
            1_000
        ),
        BidDecision::ItemNotFound
    );
}

#[test]
fn buyout_normalizes_the_offer_and_checks_settlement_arithmetic() {
    let active = BidAuction {
        highest_bidder_guid: 9,
        highest_bid: 201,
        buyout: 500,
        ..active_bid_auction()
    };
    let request = BidRequest {
        operation: BidOperation::Bid,
        operation_id: 905,
        bidder_guid: 8,
        auction_id: 41,
        house: 1,
        offer: 900,
    };

    for offer in [500, 900] {
        assert_eq!(
            decide_bid(Some(active), BidRequest { offer, ..request }, 1_000),
            accepted_buyout(500, 9, 201)
        );
    }
    assert_eq!(
        decide_bid(
            Some(BidAuction {
                deposit: u32::MAX,
                buyout: u32::MAX,
                ..active
            }),
            BidRequest {
                offer: u32::MAX,
                ..request
            },
            1_000,
        ),
        BidDecision::Database
    );
    assert!(matches!(
        decide_bid(
            Some(BidAuction {
                highest_bidder_guid: 9,
                highest_bid: u32::MAX - 1,
                buyout: u32::MAX,
                deposit: 1,
                ..active
            }),
            BidRequest {
                offer: u32::MAX,
                ..request
            },
            1_000,
        ),
        BidDecision::Accepted(BidAcceptance {
            price: u32::MAX,
            effect: AuctionBidEffect::SettleBuyout,
            ..
        })
    ));
}

#[test]
fn expiry_preserves_inconsistent_or_overflowing_sale_state_for_repair() {
    let listing = PreparedListing {
        request: request(),
        snapshot: item(23).snapshot,
        deposit: 10,
        created_micros: 1_000,
        expires_micros: 7_200_001_000,
    };
    for (highest_bidder_guid, highest_bid) in [(8, 0), (0, 201)] {
        let mut store = FakeExpiry {
            auction: Some(ActiveAuction {
                id: 41,
                listing: listing.clone(),
                highest_bidder_guid,
                highest_bid,
            }),
            schedule_count: 1,
            mail: Vec::new(),
            notices: Vec::new(),
            returned_items: Vec::new(),
            refunded_copper: 0,
        };
        assert!(expire_active(&mut store, 41).is_err());
        assert!(store.auction.is_some());
        assert_eq!(store.schedule_count, 1);
        assert!(store.mail.is_empty());
    }

    let mut overflowing = listing;
    overflowing.deposit = u32::MAX;
    let mut store = FakeExpiry {
        auction: Some(ActiveAuction {
            id: 41,
            listing: overflowing,
            highest_bidder_guid: 8,
            highest_bid: u32::MAX,
        }),
        schedule_count: 1,
        mail: Vec::new(),
        notices: Vec::new(),
        returned_items: Vec::new(),
        refunded_copper: 0,
    };
    assert!(expire_active(&mut store, 41).is_err());
    assert!(store.auction.is_some());
    assert_eq!(store.schedule_count, 1);
    assert!(store.mail.is_empty());
}

fn bid_listing() -> BidAuction {
    BidAuction {
        highest_bidder_guid: 9,
        highest_bid: 201,
        ..active_bid_auction()
    }
}

fn cancel_request(operation_id: u64, cut: u32) -> BidRequest {
    BidRequest {
        operation: BidOperation::Cancel,
        operation_id,
        bidder_guid: 7,
        auction_id: 41,
        house: 1,
        offer: cut,
    }
}

const CANCELLED_WITH_BID: BidDecision = BidDecision::Cancelled {
    cut: 10,
    displaced_bidder_guid: 9,
    displaced_bid: 201,
};

#[test]
fn cancellation_decision_checks_owner_market_expiry_and_the_held_cut() {
    let listing = Some(bid_listing());
    assert_eq!(
        decide_cancel(listing, cancel_request(950, 10), 1_000),
        CANCELLED_WITH_BID
    );
    assert_eq!(
        decide_cancel(Some(active_bid_auction()), cancel_request(950, 0), 1_000),
        BidDecision::Cancelled {
            cut: 0,
            displaced_bidder_guid: 0,
            displaced_bid: 0,
        },
        "an unbid listing costs nothing"
    );
    assert_eq!(
        decide_cancel(
            listing,
            BidRequest {
                house: 3,
                ..cancel_request(950, 10)
            },
            1_000
        ),
        CANCELLED_WITH_BID,
        "Darnassus shares the Alliance market with Stormwind"
    );

    for (cut, why) in [
        (11, "a bid raised the cut"),
        (0, "the listing gained a bid"),
    ] {
        assert_eq!(
            decide_cancel(listing, cancel_request(950, cut), 1_000),
            BidDecision::Database,
            "{why}"
        );
    }
    for (auction, request, now, why) in [
        (None, cancel_request(950, 10), 1_000, "missing"),
        (listing, cancel_request(950, 10), 2_000, "expired"),
        (
            listing,
            BidRequest {
                bidder_guid: 8,
                ..cancel_request(950, 10)
            },
            1_000,
            "another player's listing",
        ),
        (
            listing,
            BidRequest {
                house: 4,
                ..cancel_request(950, 10)
            },
            1_000,
            "the Horde market",
        ),
    ] {
        assert_eq!(
            decide_cancel(auction, request, now),
            BidDecision::ItemNotFound,
            "{why}"
        );
    }
    for (bidder, bid) in [(9, 0), (0, 201)] {
        let inconsistent = BidAuction {
            highest_bidder_guid: bidder,
            highest_bid: bid,
            ..active_bid_auction()
        };
        assert_eq!(
            decide_cancel(Some(inconsistent), cancel_request(950, 10), 1_000),
            BidDecision::Database
        );
    }
    let as_bid = BidRequest {
        operation: BidOperation::Bid,
        ..cancel_request(950, 10)
    };
    assert_eq!(decide_cancel(listing, as_bid, 1_000), BidDecision::Database);
    assert_eq!(
        decide_bid(listing, cancel_request(950, 300), 1_000),
        BidDecision::Database,
        "a Cancellation never buys"
    );
}
