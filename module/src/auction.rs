//! Durable Stormwind auctions, value-preserving bid transport, and atomic buyout settlement.

use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table, Timestamp};

use lyracore_shared::auction::bid_outcome::{
    ACCEPTED as BID_ACCEPTED, BID_INCREMENT, BID_OWN, DATABASE as BID_DATABASE,
    HIGHER_BID as BID_HIGHER, ITEM_NOT_FOUND as BID_ITEM_NOT_FOUND, PENDING as BID_PENDING,
};

use crate::{game_item_instance, game_item_template, game_world_entity};
#[cfg(feature = "debug_reducers")]
use crate::mail::game_mail;

/// One active listing in the shared Stormwind market. The complete item-instance snapshot is the
/// item while it is listed; no inventory row exists until ordinary mail returns or delivers it.
#[table(
    accessor = game_auction,
    public,
    index(accessor = by_owner, btree(columns = [owner_guid])),
    index(accessor = by_highest_bidder, btree(columns = [highest_bidder_guid]))
)]
pub struct Auction {
    #[primary_key]
    #[auto_inc]
    pub id: u32,
    pub listing_operation_id: u64,
    pub house: u32,
    pub owner_guid: u64,
    pub item_guid: u64,
    pub item_entry: u32,
    pub item_stack_count: u32,
    pub item_durability: u32,
    pub item_enchant_id: u32,
    pub item_soulbound: bool,
    pub start_bid: u32,
    pub buyout: u32,
    pub highest_bidder_guid: u64,
    pub highest_bid: u32,
    pub deposit: u32,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    pub revision: u64,
}

/// Source-shard value reserved by a sharded listing operation. A matching operation receipt makes
/// the row recovery evidence rather than spendable value; it is deleted only after that evidence
/// is durably copied back to the source.
#[table(
    accessor = game_auction_hold,
    index(accessor = by_seller, btree(columns = [seller_guid]))
)]
pub struct AuctionHold {
    #[primary_key]
    pub operation_id: u64,
    pub seller_guid: u64,
    pub item_guid: u64,
    pub item_entry: u32,
    pub item_stack_count: u32,
    pub item_durability: u32,
    pub item_enchant_id: u32,
    pub item_soulbound: bool,
    pub start_bid: u32,
    pub buyout: u32,
    pub duration_minutes: u32,
    pub deposit: u32,
    pub created_micros: i64,
    pub expires_micros: i64,
}

/// Durable idempotency receipt. The full listing payload makes identical replay distinguishable
/// from conflicting reuse even after the source Hold has been deleted.
#[table(
    accessor = game_auction_operation_receipt,
    index(accessor = by_actor, btree(columns = [actor_guid]))
)]
pub struct AuctionOperationReceipt {
    #[primary_key]
    pub operation_id: u64,
    pub auction_id: u32,
    pub actor_guid: u64,
    pub item_guid: u64,
    pub item_entry: u32,
    pub item_stack_count: u32,
    pub item_durability: u32,
    pub item_enchant_id: u32,
    pub item_soulbound: bool,
    pub start_bid: u32,
    pub buyout: u32,
    pub duration_minutes: u32,
    pub deposit: u32,
    pub created_micros: i64,
    pub expires_micros: i64,
}

/// Source-shard copper fence for one caller-identified bid. `outcome == 0` is pending; every
/// nonzero outcome is terminal. `accepted_price` records realm-core's normalized charge, while
/// `deferred_refund` retains any remainder that could not fit back in the bidder's purse.
#[table(
    accessor = game_auction_bid_hold,
    index(accessor = by_bidder, btree(columns = [bidder_guid]))
)]
pub struct AuctionBidHold {
    #[primary_key]
    pub operation_id: u64,
    pub bidder_guid: u64,
    pub auction_id: u32,
    pub offer: u32,
    pub outcome: u8,
    pub revision: u64,
    pub result_bidder_guid: u64,
    pub result_bid: u32,
    pub minimum_increment: u32,
    #[default(0)]
    pub deferred_refund: u32,
    #[default(0)]
    pub accepted_price: u32,
}

/// Realm-core's terminal serialized decision for one bid payload. Auction changes, buyout mail,
/// displaced mail, and any later source-refund mail are exact-once updates recorded on this row.
#[table(accessor = game_auction_bid_decision)]
pub struct AuctionBidDecision {
    #[primary_key]
    pub operation_id: u64,
    pub bidder_guid: u64,
    pub auction_id: u32,
    pub offer: u32,
    pub outcome: u8,
    pub revision: u64,
    pub result_bidder_guid: u64,
    pub result_bid: u32,
    pub minimum_increment: u32,
    #[default(0)]
    pub deferred_refund: u32,
    #[default(0)]
    pub accepted_price: u32,
}

/// One one-shot scheduler row for each active Auction.
#[table(
    accessor = game_auction_expiry,
    scheduled(expire_auction)
)]
pub struct AuctionExpiry {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    #[unique]
    pub auction_id: u32,
}

// Auction durability belongs to the listing protocol, not character transport. Active Auction or
// Hold value blocks character deletion, and every row stays on the database that owns its protocol
// phase rather than entering the character movement manifest.

fn duration_multiplier(duration_minutes: u32) -> Option<u64> {
    match duration_minutes {
        720 => Some(1),
        1_440 => Some(2),
        2_880 => Some(4),
        _ => None,
    }
}

fn listing_deposit(sell_price: u32, stack_count: u32, duration_minutes: u32) -> Option<u32> {
    let multiplier = duration_multiplier(duration_minutes)?;
    let deposit = u64::from(sell_price)
        .checked_mul(u64::from(stack_count))?
        .checked_mul(5)?
        .checked_mul(multiplier)?
        / 100;
    u32::try_from(deposit.max(1)).ok()
}

fn seller_proceeds(winning_price: u32, deposit: u32) -> Option<u32> {
    let cut = u64::from(winning_price).checked_mul(5)? / 100;
    let after_cut = u64::from(winning_price).checked_sub(cut)?;
    u32::try_from(after_cut.checked_add(u64::from(deposit))?).ok()
}

fn listing_proceeds_are_representable(terms: ListingTerms, deposit: u32) -> bool {
    seller_proceeds(terms.start_bid, deposit).is_some()
        && (terms.buyout == 0 || seller_proceeds(terms.buyout, deposit).is_some())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ListingItem {
    guid: u64,
    owner_guid: u64,
    slot: u8,
    mailable: bool,
    snapshot: crate::items::ItemSnapshot,
    sell_price: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ListingTerms {
    start_bid: u32,
    buyout: u32,
    duration_minutes: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListingRefusal {
    ItemNotFound,
    NotEnoughMoney,
    InvalidTerms,
}

const FIRST_BACKPACK_SLOT: u8 = 23;
const MICROS_PER_MINUTE: i64 = 60_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ListingRequest {
    operation_id: u64,
    seller_guid: u64,
    item_guid: u64,
    terms: ListingTerms,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PreparedListing {
    request: ListingRequest,
    snapshot: crate::items::ItemSnapshot,
    deposit: u32,
    created_micros: i64,
    expires_micros: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ListingReceipt {
    listing: PreparedListing,
    auction_id: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ListingHold {
    listing: PreparedListing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OperationMatch {
    Replay(u32),
    Conflict,
    Fresh,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BidRequest {
    operation_id: u64,
    bidder_guid: u64,
    auction_id: u32,
    offer: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BidAuction {
    id: u32,
    owner_guid: u64,
    item: crate::items::ItemSnapshot,
    highest_bidder_guid: u64,
    highest_bid: u32,
    start_bid: u32,
    buyout: u32,
    deposit: u32,
    expires_micros: i64,
    revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuctionBidEffect {
    RemainActive { revision: u64 },
    SettleBuyout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BidAcceptance {
    price: u32,
    effect: AuctionBidEffect,
    displaced_bidder_guid: u64,
    displaced_bid: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BidDecision {
    Accepted(BidAcceptance),
    ItemNotFound,
    HigherBid {
        bidder_guid: u64,
        current_bid: u32,
        minimum_increment: u32,
    },
    BidIncrement,
    BidOwn,
    Database,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AuctionMail {
    recipient_guid: u64,
    sender_guid: u64,
    subject: &'static str,
    money: u32,
    item: crate::items::ItemSnapshot,
}

fn displaced_bid_refund_mail(bidder_guid: u64, bid: u32) -> Option<AuctionMail> {
    (bidder_guid != 0).then_some(AuctionMail {
        recipient_guid: bidder_guid,
        sender_guid: 0,
        subject: "Auction outbid",
        money: bid,
        item: crate::items::ItemSnapshot::default(),
    })
}

fn sale_settlement_mail(
    owner_guid: u64,
    item: crate::items::ItemSnapshot,
    winner_guid: u64,
    winning_price: u32,
    deposit: u32,
) -> Option<[AuctionMail; 2]> {
    let proceeds = seller_proceeds(winning_price, deposit)?;
    Some([
        AuctionMail {
            recipient_guid: winner_guid,
            sender_guid: owner_guid,
            subject: "Auction won",
            money: 0,
            item,
        },
        AuctionMail {
            recipient_guid: owner_guid,
            sender_guid: winner_guid,
            subject: "Auction sold",
            money: proceeds,
            item: crate::items::ItemSnapshot::default(),
        },
    ])
}

fn buyout_settlement_mail(
    auction: BidAuction,
    winner_guid: u64,
    price: u32,
) -> Option<[AuctionMail; 2]> {
    if auction.buyout == 0 || price != auction.buyout {
        return None;
    }
    sale_settlement_mail(
        auction.owner_guid,
        auction.item,
        winner_guid,
        price,
        auction.deposit,
    )
}

fn insert_auction_mail(ctx: &ReducerContext, mail: AuctionMail) {
    crate::mail::insert_mail(
        ctx,
        mail.recipient_guid,
        mail.sender_guid,
        mail.subject.to_string(),
        String::new(),
        mail.money,
        0,
        &mail.item,
    );
}

#[derive(Clone, Copy)]
struct BidDecisionFields {
    outcome: u8,
    revision: u64,
    result_bidder_guid: u64,
    result_bid: u32,
    minimum_increment: u32,
    accepted_price: u32,
}

fn bid_decision_fields(decision: BidDecision) -> BidDecisionFields {
    let mut fields = BidDecisionFields {
        outcome: BID_DATABASE,
        revision: 0,
        result_bidder_guid: 0,
        result_bid: 0,
        minimum_increment: 0,
        accepted_price: 0,
    };
    match decision {
        BidDecision::Accepted(BidAcceptance {
            price,
            effect,
            displaced_bidder_guid,
            displaced_bid,
        }) => {
            fields.outcome = BID_ACCEPTED;
            fields.revision = match effect {
                AuctionBidEffect::RemainActive { revision } => revision,
                AuctionBidEffect::SettleBuyout => 0,
            };
            fields.result_bidder_guid = displaced_bidder_guid;
            fields.result_bid = displaced_bid;
            fields.accepted_price = price;
        }
        BidDecision::ItemNotFound => fields.outcome = BID_ITEM_NOT_FOUND,
        BidDecision::HigherBid {
            bidder_guid,
            current_bid,
            minimum_increment,
        } => {
            fields.outcome = BID_HIGHER;
            fields.result_bidder_guid = bidder_guid;
            fields.result_bid = current_bid;
            fields.minimum_increment = minimum_increment;
        }
        BidDecision::BidIncrement => fields.outcome = BID_INCREMENT,
        BidDecision::BidOwn => fields.outcome = BID_OWN,
        BidDecision::Database => {}
    }
    fields
}

fn bid_decision_from_fields(fields: BidDecisionFields, legacy_offer: u32) -> Option<BidDecision> {
    match fields.outcome {
        BID_PENDING => None,
        BID_ACCEPTED => Some(BidDecision::Accepted(BidAcceptance {
            price: if fields.accepted_price == 0 {
                legacy_offer
            } else {
                fields.accepted_price
            },
            effect: if fields.revision == 0 {
                AuctionBidEffect::SettleBuyout
            } else {
                AuctionBidEffect::RemainActive {
                    revision: fields.revision,
                }
            },
            displaced_bidder_guid: fields.result_bidder_guid,
            displaced_bid: fields.result_bid,
        })),
        BID_ITEM_NOT_FOUND => Some(BidDecision::ItemNotFound),
        BID_HIGHER => Some(BidDecision::HigherBid {
            bidder_guid: fields.result_bidder_guid,
            current_bid: fields.result_bid,
            minimum_increment: fields.minimum_increment,
        }),
        BID_INCREMENT => Some(BidDecision::BidIncrement),
        BID_OWN => Some(BidDecision::BidOwn),
        _ => Some(BidDecision::Database),
    }
}

fn held_bid_decision(row: &AuctionBidHold) -> Option<BidDecision> {
    bid_decision_from_fields(
        BidDecisionFields {
            outcome: row.outcome,
            revision: row.revision,
            result_bidder_guid: row.result_bidder_guid,
            result_bid: row.result_bid,
            minimum_increment: row.minimum_increment,
            accepted_price: row.accepted_price,
        },
        row.offer,
    )
}

fn realm_bid_decision(row: &AuctionBidDecision) -> Option<BidDecision> {
    bid_decision_from_fields(
        BidDecisionFields {
            outcome: row.outcome,
            revision: row.revision,
            result_bidder_guid: row.result_bidder_guid,
            result_bid: row.result_bid,
            minimum_increment: row.minimum_increment,
            accepted_price: row.accepted_price,
        },
        row.offer,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BidRefusal {
    NotEnoughMoney,
    Database,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HeldBid {
    request: BidRequest,
    decision: Option<BidDecision>,
    deferred_refund: u32,
}

trait BidSource {
    fn money(&self, bidder_guid: u64) -> Option<u32>;
    fn hold(&self, operation_id: u64) -> Option<HeldBid>;
    fn create_hold(&mut self, request: BidRequest) -> Result<(), BidRefusal>;
    fn finish_hold(
        &mut self,
        request: BidRequest,
        decision: BidDecision,
    ) -> Result<(), BidRefusal>;
    fn confirm_refund(&mut self, request: BidRequest) -> Result<(), BidRefusal>;
}

fn fence_bid<S: BidSource>(source: &mut S, request: BidRequest) -> Result<(), BidRefusal> {
    if request.operation_id == 0
        || request.bidder_guid == 0
        || request.auction_id == 0
        || request.offer == 0
    {
        return Err(BidRefusal::Database);
    }
    if let Some(hold) = source.hold(request.operation_id) {
        return if hold.request == request {
            Ok(())
        } else {
            Err(BidRefusal::Database)
        };
    }
    if source
        .money(request.bidder_guid)
        .is_none_or(|money| money < request.offer)
    {
        return Err(BidRefusal::NotEnoughMoney);
    }
    source.create_hold(request)?;
    Ok(())
}

fn finish_bid<S: BidSource>(
    source: &mut S,
    request: BidRequest,
    decision: BidDecision,
) -> Result<BidDecision, BidRefusal> {
    let hold = source
        .hold(request.operation_id)
        .ok_or(BidRefusal::Database)?;
    if hold.request != request {
        return Err(BidRefusal::Database);
    }
    if let Some(existing) = hold.decision {
        return if existing == decision {
            Ok(existing)
        } else {
            Err(BidRefusal::Database)
        };
    }
    source.finish_hold(request, decision)?;
    Ok(decision)
}

fn split_bid_refund(purse: u32, refund: u32) -> (u32, u32) {
    let purse_credit = refund.min(u32::MAX - purse);
    (purse + purse_credit, refund - purse_credit)
}

fn refundable_bid_value(decision: BidDecision, offer: u32) -> Option<u32> {
    match decision {
        BidDecision::Accepted(BidAcceptance { price, .. }) if price != 0 && price <= offer => {
            offer.checked_sub(price)
        }
        BidDecision::Accepted(_) => None,
        _ => Some(offer),
    }
}

fn minimum_next_bid(auction: BidAuction) -> Result<u32, BidDecision> {
    lyracore_shared::auction::minimum_next_bid(auction.start_bid, auction.highest_bid)
        .ok_or(BidDecision::Database)
}

fn decide_bid(auction: Option<BidAuction>, request: BidRequest, now_micros: i64) -> BidDecision {
    if request.operation_id == 0
        || request.bidder_guid == 0
        || request.auction_id == 0
        || request.offer == 0
    {
        return BidDecision::Database;
    }
    let Some(auction) = auction.filter(|auction| {
        auction.id == request.auction_id && auction.expires_micros > now_micros
    }) else {
        return BidDecision::ItemNotFound;
    };
    if auction.owner_guid == request.bidder_guid {
        return BidDecision::BidOwn;
    }
    let is_buyout = auction.buyout != 0 && request.offer >= auction.buyout;
    let minimum_increment = lyracore_shared::auction::bid_increment(auction.highest_bid);
    let minimum = if is_buyout {
        None
    } else {
        let Ok(minimum) = minimum_next_bid(auction) else {
            return BidDecision::Database;
        };
        Some(minimum)
    };
    if auction.highest_bid != 0 && request.offer <= auction.highest_bid {
        return BidDecision::HigherBid {
            bidder_guid: auction.highest_bidder_guid,
            current_bid: auction.highest_bid,
            minimum_increment,
        };
    }
    if is_buyout {
        if seller_proceeds(auction.buyout, auction.deposit).is_none() {
            return BidDecision::Database;
        }
        return BidDecision::Accepted(BidAcceptance {
            price: auction.buyout,
            effect: AuctionBidEffect::SettleBuyout,
            displaced_bidder_guid: auction.highest_bidder_guid,
            displaced_bid: auction.highest_bid,
        });
    }
    let Some(minimum) = minimum else {
        return BidDecision::Database;
    };
    if request.offer < minimum {
        return BidDecision::BidIncrement;
    }
    if seller_proceeds(request.offer, auction.deposit).is_none() {
        return BidDecision::Database;
    }
    let Some(revision) = auction.revision.checked_add(1) else {
        return BidDecision::Database;
    };
    BidDecision::Accepted(BidAcceptance {
        price: request.offer,
        effect: AuctionBidEffect::RemainActive { revision },
        displaced_bidder_guid: auction.highest_bidder_guid,
        displaced_bid: auction.highest_bid,
    })
}

trait BidMarket {
    fn decision(&self, operation_id: u64) -> Option<(BidRequest, BidDecision)>;
    fn auction(&self, auction_id: u32) -> Option<BidAuction>;
    fn now_micros(&self) -> i64;
    fn commit_decision(
        &mut self,
        request: BidRequest,
        auction: Option<BidAuction>,
        decision: BidDecision,
    ) -> Result<(), BidRefusal>;
}

trait BidRefundSink {
    fn refund_decision(
        &self,
        operation_id: u64,
    ) -> Option<(BidRequest, BidDecision, u32)>;
    fn commit_refund(
        &mut self,
        request: BidRequest,
        amount: u32,
    ) -> Result<(), BidRefusal>;
}

fn relay_bid_refund<S: BidRefundSink>(
    sink: &mut S,
    request: BidRequest,
    amount: u32,
) -> Result<(), BidRefusal> {
    if amount == 0 {
        return Ok(());
    }
    let (existing_request, decision, recorded) = sink
        .refund_decision(request.operation_id)
        .ok_or(BidRefusal::Database)?;
    if existing_request != request
        || refundable_bid_value(decision, request.offer).is_none_or(|limit| amount > limit)
    {
        return Err(BidRefusal::Database);
    }
    if recorded != 0 {
        return if recorded == amount {
            Ok(())
        } else {
            Err(BidRefusal::Database)
        };
    }
    sink.commit_refund(request, amount)
}

fn confirm_bid_refund<S: BidSource>(
    source: &mut S,
    request: BidRequest,
    amount: u32,
) -> Result<(), BidRefusal> {
    if amount == 0 {
        return Err(BidRefusal::Database);
    }
    let hold = source
        .hold(request.operation_id)
        .ok_or(BidRefusal::Database)?;
    let decision = hold.decision.ok_or(BidRefusal::Database)?;
    if hold.request != request
        || refundable_bid_value(decision, request.offer).is_none_or(|limit| amount > limit)
    {
        return Err(BidRefusal::Database);
    }
    if hold.deferred_refund == 0 {
        return Ok(());
    }
    if hold.deferred_refund != amount {
        return Err(BidRefusal::Database);
    }
    source.confirm_refund(request)
}

fn resolve_bid<S: BidMarket>(
    market: &mut S,
    request: BidRequest,
) -> Result<BidDecision, BidRefusal> {
    if let Some((existing_request, decision)) = market.decision(request.operation_id) {
        return if existing_request == request {
            Ok(decision)
        } else {
            Err(BidRefusal::Database)
        };
    }
    let auction = market.auction(request.auction_id);
    let decision = decide_bid(auction, request, market.now_micros());
    market.commit_decision(request, auction, decision)?;
    Ok(decision)
}

fn drive_bid<S: BidSource, M: BidMarket>(
    source: &mut S,
    market: &mut M,
    request: BidRequest,
) -> Result<BidDecision, BidRefusal> {
    fence_bid(source, request)?;
    if let Some(decision) = source
        .hold(request.operation_id)
        .and_then(|hold| hold.decision)
    {
        return Ok(decision);
    }
    let decision = resolve_bid(market, request)?;
    finish_bid(source, request, decision)
}

trait ListingSource {
    fn seller_money(&self, seller_guid: u64) -> Option<u32>;
    fn item(&self, item_guid: u64) -> Option<ListingItem>;
    fn now_micros(&self) -> i64;
}

trait LocalListingSink: ListingSource {
    fn receipt(&self, operation_id: u64) -> Option<ListingReceipt>;
    fn commit_local(&mut self, listing: PreparedListing) -> u32;
}

fn operation_match<S: LocalListingSink>(sink: &S, request: ListingRequest) -> OperationMatch {
    match sink.receipt(request.operation_id) {
        None => OperationMatch::Fresh,
        Some(receipt) if receipt.listing.request == request => {
            OperationMatch::Replay(receipt.auction_id)
        }
        Some(_) => OperationMatch::Conflict,
    }
}

fn prepare_from_source<S: ListingSource>(
    sink: &S,
    request: ListingRequest,
) -> Result<PreparedListing, ListingRefusal> {
    if request.operation_id == 0 {
        return Err(ListingRefusal::InvalidTerms);
    }
    let item = sink.item(request.item_guid);
    let seller_money = sink
        .seller_money(request.seller_guid)
        .ok_or(ListingRefusal::ItemNotFound)?;
    let deposit = prepare_listing(
        item.as_ref(),
        request.seller_guid,
        seller_money,
        request.terms,
    )?;
    let created_micros = sink.now_micros();
    let expires_micros = i64::from(request.terms.duration_minutes)
        .checked_mul(MICROS_PER_MINUTE)
        .and_then(|duration| created_micros.checked_add(duration))
        .ok_or(ListingRefusal::InvalidTerms)?;
    Ok(PreparedListing {
        request,
        snapshot: item.expect("validated listing item is present").snapshot,
        deposit,
        created_micros,
        expires_micros,
    })
}

fn create_local_listing<S: LocalListingSink>(
    sink: &mut S,
    request: ListingRequest,
) -> Result<u32, ListingRefusal> {
    match operation_match(sink, request) {
        OperationMatch::Replay(auction_id) => return Ok(auction_id),
        OperationMatch::Conflict => return Err(ListingRefusal::InvalidTerms),
        OperationMatch::Fresh => {}
    }
    let listing = prepare_from_source(sink, request)?;
    Ok(sink.commit_local(listing))
}

trait HoldSink: ListingSource {
    fn hold(&self, operation_id: u64) -> Option<ListingHold>;
    fn receipt(&self, operation_id: u64) -> Option<ListingReceipt>;
    fn commit_hold(&mut self, hold: ListingHold);
    fn confirm_hold(&mut self, receipt: ListingReceipt);
    fn delete_hold(&mut self, operation_id: u64);
}

trait MarketSink {
    fn receipt(&self, operation_id: u64) -> Option<ListingReceipt>;
    fn commit_market(&mut self, listing: PreparedListing) -> u32;
}

fn fence_listing<S: HoldSink>(sink: &mut S, request: ListingRequest) -> Result<(), ListingRefusal> {
    if let Some(receipt) = sink.receipt(request.operation_id) {
        return if receipt.listing.request == request {
            Ok(())
        } else {
            Err(ListingRefusal::InvalidTerms)
        };
    }
    if let Some(hold) = sink.hold(request.operation_id) {
        return if hold.listing.request == request {
            Ok(())
        } else {
            Err(ListingRefusal::InvalidTerms)
        };
    }
    let listing = prepare_from_source(sink, request)?;
    sink.commit_hold(ListingHold { listing });
    Ok(())
}

fn commit_held_listing<S: MarketSink>(
    sink: &mut S,
    listing: PreparedListing,
) -> Result<u32, ListingRefusal> {
    if let Some(receipt) = sink.receipt(listing.request.operation_id) {
        return if receipt.listing == listing {
            Ok(receipt.auction_id)
        } else {
            Err(ListingRefusal::InvalidTerms)
        };
    }
    Ok(sink.commit_market(listing))
}

fn confirm_listing<S: HoldSink>(
    sink: &mut S,
    receipt: ListingReceipt,
) -> Result<(), ListingRefusal> {
    if let Some(existing) = sink.receipt(receipt.listing.request.operation_id) {
        return if existing == receipt {
            Ok(())
        } else {
            Err(ListingRefusal::InvalidTerms)
        };
    }
    let hold = sink
        .hold(receipt.listing.request.operation_id)
        .ok_or(ListingRefusal::InvalidTerms)?;
    if hold.listing != receipt.listing {
        return Err(ListingRefusal::InvalidTerms);
    }
    sink.confirm_hold(receipt);
    Ok(())
}

fn settle_listing<S: HoldSink>(sink: &mut S, operation_id: u64) -> Result<(), ListingRefusal> {
    let receipt = sink
        .receipt(operation_id)
        .ok_or(ListingRefusal::InvalidTerms)?;
    match sink.hold(operation_id) {
        None => Ok(()),
        Some(hold) if hold.listing == receipt.listing => {
            sink.delete_hold(operation_id);
            Ok(())
        }
        Some(_) => Err(ListingRefusal::InvalidTerms),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ActiveAuction {
    id: u32,
    listing: PreparedListing,
    highest_bidder_guid: u64,
    highest_bid: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ExpiryCompletion {
    Unsold(AuctionMail),
    Sold([AuctionMail; 2]),
}

trait ExpirySink {
    fn auction(&self, auction_id: u32) -> Result<Option<ActiveAuction>, String>;
    fn complete_expiry(&mut self, auction: ActiveAuction, completion: ExpiryCompletion);
}

fn expiry_completion(auction: &ActiveAuction) -> Result<ExpiryCompletion, String> {
    match (auction.highest_bidder_guid, auction.highest_bid) {
        (0, 0) => Ok(ExpiryCompletion::Unsold(AuctionMail {
            recipient_guid: auction.listing.request.seller_guid,
            sender_guid: 0,
            subject: "Auction expired",
            money: 0,
            item: auction.listing.snapshot,
        })),
        (winner_guid, winning_price) if winner_guid != 0 && winning_price != 0 => {
            sale_settlement_mail(
                auction.listing.request.seller_guid,
                auction.listing.snapshot,
                winner_guid,
                winning_price,
                auction.listing.deposit,
            )
            .map(ExpiryCompletion::Sold)
            .ok_or_else(|| format!("auction {} sale settlement overflow", auction.id))
        }
        _ => Err(format!(
            "auction {} has inconsistent highest-bid state; preserving it for repair",
            auction.id
        )),
    }
}

fn expire_active<S: ExpirySink>(sink: &mut S, auction_id: u32) -> Result<(), String> {
    let Some(auction) = sink.auction(auction_id)? else {
        return Ok(());
    };
    let completion = expiry_completion(&auction)?;
    sink.complete_expiry(auction, completion);
    Ok(())
}

fn tagged(refusal: ListingRefusal, detail: &str) -> String {
    let tag = match refusal {
        ListingRefusal::ItemNotFound => lyracore_shared::auction::result::ITEM_NOT_FOUND,
        ListingRefusal::NotEnoughMoney => lyracore_shared::auction::result::NOT_ENOUGH_MONEY,
        ListingRefusal::InvalidTerms => lyracore_shared::auction::result::DATABASE,
    };
    format!("[{tag}] {detail}")
}

fn listing_from_hold(row: AuctionHold) -> PreparedListing {
    PreparedListing {
        request: ListingRequest {
            operation_id: row.operation_id,
            seller_guid: row.seller_guid,
            item_guid: row.item_guid,
            terms: ListingTerms {
                start_bid: row.start_bid,
                buyout: row.buyout,
                duration_minutes: row.duration_minutes,
            },
        },
        snapshot: crate::items::ItemSnapshot {
            entry: row.item_entry,
            stack_count: row.item_stack_count,
            durability: row.item_durability,
            enchant_id: row.item_enchant_id,
            soulbound: row.item_soulbound,
        },
        deposit: row.deposit,
        created_micros: row.created_micros,
        expires_micros: row.expires_micros,
    }
}

fn hold_from_listing(listing: PreparedListing) -> AuctionHold {
    AuctionHold {
        operation_id: listing.request.operation_id,
        seller_guid: listing.request.seller_guid,
        item_guid: listing.request.item_guid,
        item_entry: listing.snapshot.entry,
        item_stack_count: listing.snapshot.stack_count,
        item_durability: listing.snapshot.durability,
        item_enchant_id: listing.snapshot.enchant_id,
        item_soulbound: listing.snapshot.soulbound,
        start_bid: listing.request.terms.start_bid,
        buyout: listing.request.terms.buyout,
        duration_minutes: listing.request.terms.duration_minutes,
        deposit: listing.deposit,
        created_micros: listing.created_micros,
        expires_micros: listing.expires_micros,
    }
}

fn listing_from_receipt(row: AuctionOperationReceipt) -> ListingReceipt {
    ListingReceipt {
        listing: PreparedListing {
            request: ListingRequest {
                operation_id: row.operation_id,
                seller_guid: row.actor_guid,
                item_guid: row.item_guid,
                terms: ListingTerms {
                    start_bid: row.start_bid,
                    buyout: row.buyout,
                    duration_minutes: row.duration_minutes,
                },
            },
            snapshot: crate::items::ItemSnapshot {
                entry: row.item_entry,
                stack_count: row.item_stack_count,
                durability: row.item_durability,
                enchant_id: row.item_enchant_id,
                soulbound: row.item_soulbound,
            },
            deposit: row.deposit,
            created_micros: row.created_micros,
            expires_micros: row.expires_micros,
        },
        auction_id: row.auction_id,
    }
}

fn receipt_from_listing(listing: PreparedListing, auction_id: u32) -> AuctionOperationReceipt {
    AuctionOperationReceipt {
        operation_id: listing.request.operation_id,
        auction_id,
        actor_guid: listing.request.seller_guid,
        item_guid: listing.request.item_guid,
        item_entry: listing.snapshot.entry,
        item_stack_count: listing.snapshot.stack_count,
        item_durability: listing.snapshot.durability,
        item_enchant_id: listing.snapshot.enchant_id,
        item_soulbound: listing.snapshot.soulbound,
        start_bid: listing.request.terms.start_bid,
        buyout: listing.request.terms.buyout,
        duration_minutes: listing.request.terms.duration_minutes,
        deposit: listing.deposit,
        created_micros: listing.created_micros,
        expires_micros: listing.expires_micros,
    }
}

struct CtxSource<'a> {
    ctx: &'a ReducerContext,
}

impl ListingSource for CtxSource<'_> {
    fn seller_money(&self, seller_guid: u64) -> Option<u32> {
        crate::helpers::acting_entity_by_guid(self.ctx, seller_guid).map(|seller| seller.money)
    }

    fn item(&self, item_guid: u64) -> Option<ListingItem> {
        let item = self.ctx.db.game_item_instance().guid().find(item_guid)?;
        let template = self.ctx.db.game_item_template().entry().find(item.entry)?;
        Some(ListingItem {
            guid: item.guid,
            owner_guid: item.owner_guid,
            slot: item.slot,
            mailable: crate::items::validate_bag_dest_slot(self.ctx, item.owner_guid, item.slot)
                .is_ok(),
            snapshot: crate::items::ItemSnapshot::from(&item),
            sell_price: template.sell_price,
        })
    }

    fn now_micros(&self) -> i64 {
        self.ctx.timestamp.to_micros_since_unix_epoch()
    }
}

fn insert_active_auction(ctx: &ReducerContext, listing: &PreparedListing) -> u32 {
    let created_at = Timestamp::from_micros_since_unix_epoch(listing.created_micros);
    let expires_at = Timestamp::from_micros_since_unix_epoch(listing.expires_micros);
    let auction = ctx.db.game_auction().insert(Auction {
        id: 0,
        listing_operation_id: listing.request.operation_id,
        house: lyracore_shared::auction::STORMWIND_HOUSE_ID,
        owner_guid: listing.request.seller_guid,
        item_guid: listing.request.item_guid,
        item_entry: listing.snapshot.entry,
        item_stack_count: listing.snapshot.stack_count,
        item_durability: listing.snapshot.durability,
        item_enchant_id: listing.snapshot.enchant_id,
        item_soulbound: listing.snapshot.soulbound,
        start_bid: listing.request.terms.start_bid,
        buyout: listing.request.terms.buyout,
        highest_bidder_guid: 0,
        highest_bid: 0,
        deposit: listing.deposit,
        created_at,
        expires_at,
        revision: 0,
    });
    ctx.db.game_auction_expiry().insert(AuctionExpiry {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Time(expires_at),
        auction_id: auction.id,
    });
    auction.id
}

fn consume_listing_value(ctx: &ReducerContext, listing: &PreparedListing) {
    let mut seller = crate::helpers::acting_entity_by_guid(ctx, listing.request.seller_guid)
        .expect("validated listing seller remains present in the reducer transaction");
    seller.money -= listing.deposit;
    ctx.db.game_world_entity().guid().update(seller);
    ctx.db
        .game_item_instance()
        .guid()
        .delete(listing.request.item_guid);
}

impl LocalListingSink for CtxSource<'_> {
    fn receipt(&self, operation_id: u64) -> Option<ListingReceipt> {
        self.ctx
            .db
            .game_auction_operation_receipt()
            .operation_id()
            .find(operation_id)
            .map(listing_from_receipt)
    }

    fn commit_local(&mut self, listing: PreparedListing) -> u32 {
        consume_listing_value(self.ctx, &listing);
        let auction_id = insert_active_auction(self.ctx, &listing);
        self.ctx
            .db
            .game_auction_operation_receipt()
            .insert(receipt_from_listing(listing, auction_id));
        auction_id
    }
}

impl HoldSink for CtxSource<'_> {
    fn hold(&self, operation_id: u64) -> Option<ListingHold> {
        self.ctx
            .db
            .game_auction_hold()
            .operation_id()
            .find(operation_id)
            .map(|row| ListingHold {
                listing: listing_from_hold(row),
            })
    }

    fn receipt(&self, operation_id: u64) -> Option<ListingReceipt> {
        <Self as LocalListingSink>::receipt(self, operation_id)
    }

    fn commit_hold(&mut self, hold: ListingHold) {
        consume_listing_value(self.ctx, &hold.listing);
        self.ctx
            .db
            .game_auction_hold()
            .insert(hold_from_listing(hold.listing));
    }

    fn confirm_hold(&mut self, receipt: ListingReceipt) {
        self.ctx
            .db
            .game_auction_operation_receipt()
            .insert(receipt_from_listing(receipt.listing, receipt.auction_id));
    }

    fn delete_hold(&mut self, operation_id: u64) {
        self.ctx
            .db
            .game_auction_hold()
            .operation_id()
            .delete(operation_id);
    }
}

struct CtxMarket<'a> {
    ctx: &'a ReducerContext,
}

impl MarketSink for CtxMarket<'_> {
    fn receipt(&self, operation_id: u64) -> Option<ListingReceipt> {
        self.ctx
            .db
            .game_auction_operation_receipt()
            .operation_id()
            .find(operation_id)
            .map(listing_from_receipt)
    }

    fn commit_market(&mut self, listing: PreparedListing) -> u32 {
        let auction_id = insert_active_auction(self.ctx, &listing);
        self.ctx
            .db
            .game_auction_operation_receipt()
            .insert(receipt_from_listing(listing, auction_id));
        auction_id
    }
}

struct CtxBidSource<'a> {
    ctx: &'a ReducerContext,
}

impl BidSource for CtxBidSource<'_> {
    fn money(&self, bidder_guid: u64) -> Option<u32> {
        crate::helpers::acting_entity_by_guid(self.ctx, bidder_guid).map(|bidder| bidder.money)
    }

    fn hold(&self, operation_id: u64) -> Option<HeldBid> {
        self.ctx
            .db
            .game_auction_bid_hold()
            .operation_id()
            .find(operation_id)
            .map(|row| HeldBid {
                request: BidRequest {
                    operation_id: row.operation_id,
                    bidder_guid: row.bidder_guid,
                    auction_id: row.auction_id,
                    offer: row.offer,
                },
                decision: held_bid_decision(&row),
                deferred_refund: row.deferred_refund,
            })
    }

    fn create_hold(&mut self, request: BidRequest) -> Result<(), BidRefusal> {
        let mut bidder = crate::helpers::acting_entity_by_guid(self.ctx, request.bidder_guid)
            .ok_or(BidRefusal::NotEnoughMoney)?;
        bidder.money = bidder
            .money
            .checked_sub(request.offer)
            .ok_or(BidRefusal::NotEnoughMoney)?;
        self.ctx.db.game_world_entity().guid().update(bidder);
        self.ctx.db.game_auction_bid_hold().insert(AuctionBidHold {
            operation_id: request.operation_id,
            bidder_guid: request.bidder_guid,
            auction_id: request.auction_id,
            offer: request.offer,
            outcome: BID_PENDING,
            revision: 0,
            result_bidder_guid: 0,
            result_bid: 0,
            minimum_increment: 0,
            accepted_price: 0,
            deferred_refund: 0,
        });
        Ok(())
    }

    fn finish_hold(
        &mut self,
        request: BidRequest,
        decision: BidDecision,
    ) -> Result<(), BidRefusal> {
        let refund = refundable_bid_value(decision, request.offer).ok_or(BidRefusal::Database)?;
        let deferred_refund = if refund != 0 {
            let mut bidder = crate::helpers::acting_entity_by_guid(self.ctx, request.bidder_guid)
                .ok_or(BidRefusal::Database)?;
            let (money, deferred_refund) = split_bid_refund(bidder.money, refund);
            bidder.money = money;
            self.ctx.db.game_world_entity().guid().update(bidder);
            deferred_refund
        } else {
            0
        };
        let fields = bid_decision_fields(decision);
        self.ctx
            .db
            .game_auction_bid_hold()
            .operation_id()
            .update(AuctionBidHold {
                operation_id: request.operation_id,
                bidder_guid: request.bidder_guid,
                auction_id: request.auction_id,
                offer: request.offer,
                outcome: fields.outcome,
                revision: fields.revision,
                result_bidder_guid: fields.result_bidder_guid,
                result_bid: fields.result_bid,
                minimum_increment: fields.minimum_increment,
                accepted_price: fields.accepted_price,
                deferred_refund,
            });
        Ok(())
    }

    fn confirm_refund(&mut self, request: BidRequest) -> Result<(), BidRefusal> {
        let mut row = self
            .ctx
            .db
            .game_auction_bid_hold()
            .operation_id()
            .find(request.operation_id)
            .ok_or(BidRefusal::Database)?;
        if row.bidder_guid != request.bidder_guid
            || row.auction_id != request.auction_id
            || row.offer != request.offer
        {
            return Err(BidRefusal::Database);
        }
        row.deferred_refund = 0;
        self.ctx
            .db
            .game_auction_bid_hold()
            .operation_id()
            .update(row);
        Ok(())
    }
}

struct CtxBidMarket<'a> {
    ctx: &'a ReducerContext,
}

impl BidMarket for CtxBidMarket<'_> {
    fn decision(&self, operation_id: u64) -> Option<(BidRequest, BidDecision)> {
        let row = self
            .ctx
            .db
            .game_auction_bid_decision()
            .operation_id()
            .find(operation_id)?;
        let decision = realm_bid_decision(&row)?;
        Some((
            BidRequest {
                operation_id: row.operation_id,
                bidder_guid: row.bidder_guid,
                auction_id: row.auction_id,
                offer: row.offer,
            },
            decision,
        ))
    }

    fn auction(&self, auction_id: u32) -> Option<BidAuction> {
        self.ctx
            .db
            .game_auction()
            .id()
            .find(auction_id)
            .filter(|auction| auction.house == lyracore_shared::auction::STORMWIND_HOUSE_ID)
            .map(|auction| BidAuction {
                id: auction.id,
                owner_guid: auction.owner_guid,
                item: crate::items::ItemSnapshot {
                    entry: auction.item_entry,
                    stack_count: auction.item_stack_count,
                    durability: auction.item_durability,
                    enchant_id: auction.item_enchant_id,
                    soulbound: auction.item_soulbound,
                },
                highest_bidder_guid: auction.highest_bidder_guid,
                highest_bid: auction.highest_bid,
                start_bid: auction.start_bid,
                buyout: auction.buyout,
                deposit: auction.deposit,
                expires_micros: auction.expires_at.to_micros_since_unix_epoch(),
                revision: auction.revision,
            })
    }

    fn now_micros(&self) -> i64 {
        self.ctx.timestamp.to_micros_since_unix_epoch()
    }

    fn commit_decision(
        &mut self,
        request: BidRequest,
        auction: Option<BidAuction>,
        decision: BidDecision,
    ) -> Result<(), BidRefusal> {
        if let BidDecision::Accepted(accepted) = decision {
            let expected = auction.ok_or(BidRefusal::Database)?;
            let mut row = self
                .ctx
                .db
                .game_auction()
                .id()
                .find(request.auction_id)
                .ok_or(BidRefusal::Database)?;
            if row.revision != expected.revision
                || row.highest_bidder_guid != expected.highest_bidder_guid
                || row.highest_bid != expected.highest_bid
            {
                return Err(BidRefusal::Database);
            }
            let refund_mail =
                displaced_bid_refund_mail(accepted.displaced_bidder_guid, accepted.displaced_bid);
            match accepted.effect {
                AuctionBidEffect::SettleBuyout => {
                    let sale_mail =
                        buyout_settlement_mail(expected, request.bidder_guid, accepted.price)
                            .ok_or(BidRefusal::Database)?;
                    self.ctx
                        .db
                        .game_auction_expiry()
                        .auction_id()
                        .delete(row.id);
                    self.ctx.db.game_auction().id().delete(row.id);
                    refund_mail
                        .into_iter()
                        .chain(sale_mail)
                        .for_each(|mail| insert_auction_mail(self.ctx, mail));
                }
                AuctionBidEffect::RemainActive { revision } => {
                    row.highest_bidder_guid = request.bidder_guid;
                    row.highest_bid = accepted.price;
                    row.revision = revision;
                    self.ctx.db.game_auction().id().update(row);
                    refund_mail
                        .into_iter()
                        .for_each(|mail| insert_auction_mail(self.ctx, mail));
                }
            }
        }
        let fields = bid_decision_fields(decision);
        self.ctx
            .db
            .game_auction_bid_decision()
            .insert(AuctionBidDecision {
                operation_id: request.operation_id,
                bidder_guid: request.bidder_guid,
                auction_id: request.auction_id,
                offer: request.offer,
                outcome: fields.outcome,
                revision: fields.revision,
                result_bidder_guid: fields.result_bidder_guid,
                result_bid: fields.result_bid,
                minimum_increment: fields.minimum_increment,
                accepted_price: fields.accepted_price,
                deferred_refund: 0,
            });
        Ok(())
    }
}

impl BidRefundSink for CtxBidMarket<'_> {
    fn refund_decision(
        &self,
        operation_id: u64,
    ) -> Option<(BidRequest, BidDecision, u32)> {
        let row = self
            .ctx
            .db
            .game_auction_bid_decision()
            .operation_id()
            .find(operation_id)?;
        let decision = realm_bid_decision(&row)?;
        Some((
            bid_request(row.operation_id, row.bidder_guid, row.auction_id, row.offer),
            decision,
            row.deferred_refund,
        ))
    }

    fn commit_refund(
        &mut self,
        request: BidRequest,
        amount: u32,
    ) -> Result<(), BidRefusal> {
        let mut row = self
            .ctx
            .db
            .game_auction_bid_decision()
            .operation_id()
            .find(request.operation_id)
            .ok_or(BidRefusal::Database)?;
        if row.bidder_guid != request.bidder_guid
            || row.auction_id != request.auction_id
            || row.offer != request.offer
            || row.deferred_refund != 0
        {
            return Err(BidRefusal::Database);
        }
        crate::mail::insert_mail(
            self.ctx,
            request.bidder_guid,
            0,
            "Auction bid refund".to_string(),
            String::new(),
            amount,
            0,
            &crate::items::ItemSnapshot::default(),
        );
        row.deferred_refund = amount;
        self.ctx
            .db
            .game_auction_bid_decision()
            .operation_id()
            .update(row);
        Ok(())
    }
}

fn bid_request(operation_id: u64, bidder_guid: u64, auction_id: u32, offer: u32) -> BidRequest {
    BidRequest {
        operation_id,
        bidder_guid,
        auction_id,
        offer,
    }
}

fn tagged_bid(refusal: BidRefusal, detail: &str) -> String {
    let tag = match refusal {
        BidRefusal::NotEnoughMoney => lyracore_shared::auction::result::NOT_ENOUGH_MONEY,
        BidRefusal::Database => lyracore_shared::auction::result::DATABASE,
    };
    format!("[{tag}] {detail}")
}

fn validate_market_listing(ctx: &ReducerContext, listing: &PreparedListing) -> Result<(), String> {
    if listing.request.operation_id == 0
        || listing.snapshot.stack_count == 0
        || listing.snapshot.soulbound
        || listing.request.terms.start_bid == 0
        || (listing.request.terms.buyout != 0
            && listing.request.terms.buyout < listing.request.terms.start_bid)
    {
        return Err(tagged(ListingRefusal::InvalidTerms, "invalid held listing"));
    }
    let template = ctx
        .db
        .game_item_template()
        .entry()
        .find(listing.snapshot.entry)
        .ok_or_else(|| tagged(ListingRefusal::InvalidTerms, "item template missing"))?;
    let expected_deposit = listing_deposit(
        template.sell_price,
        listing.snapshot.stack_count,
        listing.request.terms.duration_minutes,
    )
    .ok_or_else(|| tagged(ListingRefusal::InvalidTerms, "invalid listing arithmetic"))?;
    let expected_expiry = i64::from(listing.request.terms.duration_minutes)
        .checked_mul(MICROS_PER_MINUTE)
        .and_then(|duration| listing.created_micros.checked_add(duration))
        .ok_or_else(|| tagged(ListingRefusal::InvalidTerms, "auction expiry overflow"))?;
    if listing.deposit != expected_deposit
        || !listing_proceeds_are_representable(listing.request.terms, expected_deposit)
        || listing.expires_micros != expected_expiry
    {
        return Err(tagged(
            ListingRefusal::InvalidTerms,
            "held listing payload changed",
        ));
    }
    Ok(())
}

struct CtxExpiry<'a> {
    ctx: &'a ReducerContext,
}

impl ExpirySink for CtxExpiry<'_> {
    fn auction(&self, auction_id: u32) -> Result<Option<ActiveAuction>, String> {
        let Some(auction) = self.ctx.db.game_auction().id().find(auction_id) else {
            return Ok(None);
        };
        let receipt = self
            .ctx
            .db
            .game_auction_operation_receipt()
            .operation_id()
            .find(auction.listing_operation_id)
            .map(listing_from_receipt)
            .ok_or_else(|| {
                format!("auction {auction_id} has no operation receipt; preserving it for repair")
            })?;
        Ok(Some(ActiveAuction {
            id: auction.id,
            listing: receipt.listing,
            highest_bidder_guid: auction.highest_bidder_guid,
            highest_bid: auction.highest_bid,
        }))
    }

    fn complete_expiry(&mut self, auction: ActiveAuction, completion: ExpiryCompletion) {
        match completion {
            ExpiryCompletion::Unsold(mail) => insert_auction_mail(self.ctx, mail),
            ExpiryCompletion::Sold(mail) => mail
                .into_iter()
                .for_each(|mail| insert_auction_mail(self.ctx, mail)),
        }
        self.ctx
            .db
            .game_auction_expiry()
            .auction_id()
            .delete(auction.id);
        self.ctx.db.game_auction().id().delete(auction.id);
    }
}

fn listing_request(
    operation_id: u64,
    seller_guid: u64,
    item_guid: u64,
    start_bid: u32,
    buyout: u32,
    duration_minutes: u32,
) -> ListingRequest {
    ListingRequest {
        operation_id,
        seller_guid,
        item_guid,
        terms: ListingTerms {
            start_bid,
            buyout,
            duration_minutes,
        },
    }
}

/// Single-database listing: item, deposit, Auction, receipt, and expiry are one transaction.
#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn gw_auction_list_local(
    ctx: &ReducerContext,
    operation_id: u64,
    seller_guid: u64,
    item_guid: u64,
    start_bid: u32,
    buyout: u32,
    duration_minutes: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    create_local_listing(
        &mut CtxSource { ctx },
        listing_request(
            operation_id,
            seller_guid,
            item_guid,
            start_bid,
            buyout,
            duration_minutes,
        ),
    )
    .map(|_| ())
    .map_err(|refusal| tagged(refusal, "listing rejected"))
}

/// Sharded listing phase 1: atomically move the source value into a caller-identified Hold.
#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn gw_auction_hold_listing(
    ctx: &ReducerContext,
    operation_id: u64,
    seller_guid: u64,
    item_guid: u64,
    start_bid: u32,
    buyout: u32,
    duration_minutes: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    fence_listing(
        &mut CtxSource { ctx },
        listing_request(
            operation_id,
            seller_guid,
            item_guid,
            start_bid,
            buyout,
            duration_minutes,
        ),
    )
    .map_err(|refusal| tagged(refusal, "listing Hold rejected"))
}

/// Sharded listing phase 2: create the realm Auction and idempotency receipt from a held payload.
#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn realm_auction_commit_listing(
    ctx: &ReducerContext,
    operation_id: u64,
    seller_guid: u64,
    item_guid: u64,
    item_entry: u32,
    item_stack_count: u32,
    item_durability: u32,
    item_enchant_id: u32,
    item_soulbound: bool,
    start_bid: u32,
    buyout: u32,
    duration_minutes: u32,
    deposit: u32,
    created_micros: i64,
    expires_micros: i64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let listing = PreparedListing {
        request: listing_request(
            operation_id,
            seller_guid,
            item_guid,
            start_bid,
            buyout,
            duration_minutes,
        ),
        snapshot: crate::items::ItemSnapshot {
            entry: item_entry,
            stack_count: item_stack_count,
            durability: item_durability,
            enchant_id: item_enchant_id,
            soulbound: item_soulbound,
        },
        deposit,
        created_micros,
        expires_micros,
    };
    let mut market = CtxMarket { ctx };
    if market.receipt(operation_id).is_none() {
        validate_market_listing(ctx, &listing)?;
    }
    commit_held_listing(&mut market, listing)
        .map(|_| ())
        .map_err(|refusal| tagged(refusal, "listing operation id conflict"))
}

/// Sharded listing phase 3: copy the matching realm receipt onto the source shard.
#[reducer]
pub fn realm_auction_confirm_listing(
    ctx: &ReducerContext,
    operation_id: u64,
    auction_id: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let listing = CtxSource { ctx }
        .hold(operation_id)
        .map(|hold| hold.listing)
        .or_else(|| {
            <CtxSource<'_> as HoldSink>::receipt(&CtxSource { ctx }, operation_id)
                .map(|receipt| receipt.listing)
        })
        .ok_or_else(|| tagged(ListingRefusal::InvalidTerms, "listing Hold missing"))?;
    confirm_listing(
        &mut CtxSource { ctx },
        ListingReceipt {
            listing,
            auction_id,
        },
    )
    .map_err(|refusal| tagged(refusal, "listing receipt conflict"))
}

/// Sharded listing phase 4: delete the Hold only after the source has matching receipt evidence.
#[reducer]
pub fn realm_auction_settle_listing(ctx: &ReducerContext, operation_id: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    settle_listing(&mut CtxSource { ctx }, operation_id)
        .map_err(|refusal| tagged(refusal, "listing Hold is not confirmed"))
}

/// Single-database bid: full-offer Hold, realm decision, Auction update or buyout settlement,
/// ordinary mail, and terminal source outcome commit atomically.
#[reducer]
pub fn gw_auction_bid_local(
    ctx: &ReducerContext,
    operation_id: u64,
    bidder_guid: u64,
    auction_id: u32,
    offer: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let request = bid_request(operation_id, bidder_guid, auction_id, offer);
    drive_bid(
        &mut CtxBidSource { ctx },
        &mut CtxBidMarket { ctx },
        request,
    )
    .map_err(|refusal| tagged_bid(refusal, "local bid rejected"))?;
    let deferred_refund = CtxBidSource { ctx }
        .hold(operation_id)
        .ok_or_else(|| tagged_bid(BidRefusal::Database, "local bid Hold missing"))?
        .deferred_refund;
    if deferred_refund != 0 {
        relay_bid_refund(&mut CtxBidMarket { ctx }, request, deferred_refund)
            .map_err(|refusal| tagged_bid(refusal, "local bid refund conflict"))?;
        confirm_bid_refund(&mut CtxBidSource { ctx }, request, deferred_refund)
            .map_err(|refusal| tagged_bid(refusal, "local bid refund confirmation conflict"))?;
    }
    Ok(())
}

/// Sharded bid phase 1: move the complete offer into a source-shard Hold before realm-core decides.
#[reducer]
pub fn gw_auction_hold_bid(
    ctx: &ReducerContext,
    operation_id: u64,
    bidder_guid: u64,
    auction_id: u32,
    offer: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    fence_bid(
        &mut CtxBidSource { ctx },
        bid_request(operation_id, bidder_guid, auction_id, offer),
    )
    .map_err(|refusal| tagged_bid(refusal, "bid Hold rejected"))
}

/// Sharded bid phase 2: serialize against the realm Auction and persist one terminal decision.
#[reducer]
pub fn realm_auction_decide_bid(
    ctx: &ReducerContext,
    operation_id: u64,
    bidder_guid: u64,
    auction_id: u32,
    offer: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    resolve_bid(
        &mut CtxBidMarket { ctx },
        bid_request(operation_id, bidder_guid, auction_id, offer),
    )
    .map(|_| ())
    .map_err(|refusal| tagged_bid(refusal, "bid decision conflict"))
}

/// Sharded bid phase 3: consume the normalized accepted price or restore refused value exactly once.
#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn gw_auction_finish_bid(
    ctx: &ReducerContext,
    operation_id: u64,
    bidder_guid: u64,
    auction_id: u32,
    offer: u32,
    outcome: u8,
    revision: u64,
    result_bidder_guid: u64,
    result_bid: u32,
    minimum_increment: u32,
    accepted_price: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let decision = bid_decision_from_fields(
        BidDecisionFields {
            outcome,
            revision,
            result_bidder_guid,
            result_bid,
            minimum_increment,
            accepted_price,
        },
        offer,
    )
    .ok_or_else(|| tagged_bid(BidRefusal::Database, "bid decision is pending"))?;
    finish_bid(
        &mut CtxBidSource { ctx },
        bid_request(operation_id, bidder_guid, auction_id, offer),
        decision,
    )
    .map(|_| ())
    .map_err(|refusal| tagged_bid(refusal, "bid outcome conflict"))
}

/// Sharded bid phase 4: place an unrepresentable purse refund in realm-core mail exactly once.
#[reducer]
pub fn realm_auction_refund_bid(
    ctx: &ReducerContext,
    operation_id: u64,
    bidder_guid: u64,
    auction_id: u32,
    offer: u32,
    deferred_refund: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    relay_bid_refund(
        &mut CtxBidMarket { ctx },
        bid_request(operation_id, bidder_guid, auction_id, offer),
        deferred_refund,
    )
    .map_err(|refusal| tagged_bid(refusal, "bid refund conflict"))
}

/// Sharded bid phase 5: record on the source that realm-core durably accepted the refund mail.
#[reducer]
pub fn gw_auction_confirm_bid_refund(
    ctx: &ReducerContext,
    operation_id: u64,
    bidder_guid: u64,
    auction_id: u32,
    offer: u32,
    deferred_refund: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    confirm_bid_refund(
        &mut CtxBidSource { ctx },
        bid_request(operation_id, bidder_guid, auction_id, offer),
        deferred_refund,
    )
    .map_err(|refusal| tagged_bid(refusal, "bid refund confirmation conflict"))
}

/// Scheduler-only one-shot expiry. Replays see no active Auction and therefore create no mail.
#[reducer]
pub fn expire_auction(ctx: &ReducerContext, schedule: AuctionExpiry) -> Result<(), String> {
    if ctx.sender() != ctx.database_identity() {
        return Err("scheduler only".to_string());
    }
    expire_active(&mut CtxExpiry { ctx }, schedule.auction_id)
}

#[cfg(feature = "debug_reducers")]
const BUYOUT_FIXTURE_AUCTION_ID: u32 = 509_0050;
#[cfg(feature = "debug_reducers")]
const BUYOUT_FIXTURE_OPERATION_ID: u64 = 509_0050;
#[cfg(feature = "debug_reducers")]
const BUYOUT_FIXTURE_SELLER_GUID: u64 = 509_0050;
#[cfg(feature = "debug_reducers")]
const BUYOUT_FIXTURE_WINNER_GUID: u64 = 509_0051;
#[cfg(feature = "debug_reducers")]
const BUYOUT_FIXTURE_DISPLACED_GUID: u64 = 509_0052;

/// Stage one reserved Auction row for the standalone buyout integration test.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_stage_auction_buyout_fixture(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;

    ctx.db
        .game_auction_expiry()
        .auction_id()
        .delete(BUYOUT_FIXTURE_AUCTION_ID);
    ctx.db
        .game_auction()
        .id()
        .delete(BUYOUT_FIXTURE_AUCTION_ID);
    ctx.db
        .game_auction_bid_decision()
        .operation_id()
        .delete(BUYOUT_FIXTURE_OPERATION_ID);

    let mails = ctx.db.game_mail();
    for recipient_guid in [
        BUYOUT_FIXTURE_SELLER_GUID,
        BUYOUT_FIXTURE_WINNER_GUID,
        BUYOUT_FIXTURE_DISPLACED_GUID,
    ] {
        let stale: Vec<u64> = mails
            .by_recipient()
            .filter(&recipient_guid)
            .filter(|mail| {
                matches!(
                    mail.subject.as_str(),
                    "Auction outbid" | "Auction won" | "Auction sold"
                )
            })
            .map(|mail| mail.id)
            .collect();
        for id in stale {
            mails.id().delete(id);
        }
    }

    let expires_micros = ctx
        .timestamp
        .to_micros_since_unix_epoch()
        .checked_add(3_600_000_000)
        .ok_or_else(|| "auction buyout fixture expiry overflow".to_string())?;
    let expires_at = Timestamp::from_micros_since_unix_epoch(expires_micros);
    ctx.db.game_auction().insert(Auction {
        id: BUYOUT_FIXTURE_AUCTION_ID,
        listing_operation_id: BUYOUT_FIXTURE_OPERATION_ID - 1,
        house: lyracore_shared::auction::STORMWIND_HOUSE_ID,
        owner_guid: BUYOUT_FIXTURE_SELLER_GUID,
        item_guid: 509_0053,
        item_entry: 509_0050,
        item_stack_count: 2,
        item_durability: 17,
        item_enchant_id: 9,
        item_soulbound: false,
        start_bid: 100,
        buyout: 500,
        highest_bidder_guid: BUYOUT_FIXTURE_DISPLACED_GUID,
        highest_bid: 201,
        deposit: 10,
        created_at: ctx.timestamp,
        expires_at,
        revision: 3,
    });
    ctx.db.game_auction_expiry().insert(AuctionExpiry {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Time(expires_at),
        auction_id: BUYOUT_FIXTURE_AUCTION_ID,
    });
    Ok(())
}

#[cfg(feature = "debug_reducers")]
fn auction_fixture_mail(
    ctx: &ReducerContext,
    recipient_guid: u64,
    subject: &str,
) -> Result<crate::Mail, String> {
    let mut matches: Vec<_> = ctx
        .db
        .game_mail()
        .by_recipient()
        .filter(&recipient_guid)
        .filter(|mail| mail.subject == subject)
        .collect();
    if matches.len() != 1 {
        return Err(format!(
            "expected one {subject:?} mail for {recipient_guid}, found {}",
            matches.len()
        ));
    }
    Ok(matches.remove(0))
}

/// Verify the real realm reducer committed exact settlement rows in a prior transaction.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_verify_auction_buyout_fixture(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if ctx
        .db
        .game_auction()
        .id()
        .find(BUYOUT_FIXTURE_AUCTION_ID)
        .is_some()
        || ctx
            .db
            .game_auction_expiry()
            .auction_id()
            .find(BUYOUT_FIXTURE_AUCTION_ID)
            .is_some()
    {
        return Err("settled Auction or expiry schedule is still active".to_string());
    }

    let decision = ctx
        .db
        .game_auction_bid_decision()
        .operation_id()
        .find(BUYOUT_FIXTURE_OPERATION_ID)
        .ok_or_else(|| "buyout decision was not committed".to_string())?;
    if decision.bidder_guid != BUYOUT_FIXTURE_WINNER_GUID
        || decision.auction_id != BUYOUT_FIXTURE_AUCTION_ID
        || decision.offer != 900
        || decision.outcome != BID_ACCEPTED
        || decision.revision != 0
        || decision.result_bidder_guid != BUYOUT_FIXTURE_DISPLACED_GUID
        || decision.result_bid != 201
        || decision.accepted_price != 500
    {
        return Err("buyout decision payload changed".to_string());
    }

    let refund = auction_fixture_mail(
        ctx,
        BUYOUT_FIXTURE_DISPLACED_GUID,
        "Auction outbid",
    )?;
    if refund.sender_guid != 0
        || refund.money != 201
        || refund.cod != 0
        || refund.was_read
        || !refund.body.is_empty()
        || !refund.snapshot().is_empty()
    {
        return Err("displaced-bidder refund mail changed".to_string());
    }

    let winner = auction_fixture_mail(ctx, BUYOUT_FIXTURE_WINNER_GUID, "Auction won")?;
    if winner.sender_guid != BUYOUT_FIXTURE_SELLER_GUID
        || winner.money != 0
        || winner.cod != 0
        || winner.was_read
        || !winner.body.is_empty()
        || winner.snapshot()
            != (crate::items::ItemSnapshot {
                entry: 509_0050,
                stack_count: 2,
                durability: 17,
                enchant_id: 9,
                soulbound: false,
            })
    {
        return Err("winner item mail changed".to_string());
    }

    let seller = auction_fixture_mail(ctx, BUYOUT_FIXTURE_SELLER_GUID, "Auction sold")?;
    if seller.sender_guid != BUYOUT_FIXTURE_WINNER_GUID
        || seller.money != 485
        || seller.cod != 0
        || seller.was_read
        || !seller.body.is_empty()
        || !seller.snapshot().is_empty()
    {
        return Err("seller proceeds mail changed".to_string());
    }
    Ok(())
}

#[cfg(feature = "debug_reducers")]
const EXPIRY_FIXTURE_AUCTION_ID: u32 = 509_0060;
#[cfg(feature = "debug_reducers")]
const EXPIRY_FIXTURE_OPERATION_ID: u64 = 509_0060;
#[cfg(feature = "debug_reducers")]
const EXPIRY_FIXTURE_SELLER_GUID: u64 = 509_0060;
#[cfg(feature = "debug_reducers")]
const EXPIRY_FIXTURE_WINNER_GUID: u64 = 509_0061;
#[cfg(feature = "debug_reducers")]
const EXPIRY_FIXTURE_ITEM: crate::items::ItemSnapshot = crate::items::ItemSnapshot {
    entry: 509_0060,
    stack_count: 2,
    durability: 17,
    enchant_id: 9,
    soulbound: false,
};

/// Stage a valid winning-bid Auction whose one-shot schedule fires shortly after this transaction.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_stage_auction_expiry_fixture(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;

    ctx.db
        .game_auction_expiry()
        .auction_id()
        .delete(EXPIRY_FIXTURE_AUCTION_ID);
    ctx.db
        .game_auction()
        .id()
        .delete(EXPIRY_FIXTURE_AUCTION_ID);
    ctx.db
        .game_auction_operation_receipt()
        .operation_id()
        .delete(EXPIRY_FIXTURE_OPERATION_ID);

    let mails = ctx.db.game_mail();
    for recipient_guid in [EXPIRY_FIXTURE_SELLER_GUID, EXPIRY_FIXTURE_WINNER_GUID] {
        let stale: Vec<u64> = mails
            .by_recipient()
            .filter(&recipient_guid)
            .filter(|mail| matches!(mail.subject.as_str(), "Auction won" | "Auction sold"))
            .map(|mail| mail.id)
            .collect();
        for id in stale {
            mails.id().delete(id);
        }
    }

    let expires_micros = ctx
        .timestamp
        .to_micros_since_unix_epoch()
        .checked_add(500_000)
        .ok_or_else(|| "auction expiry fixture deadline overflow".to_string())?;
    let created_micros = expires_micros
        .checked_sub(43_200_000_000)
        .ok_or_else(|| "auction expiry fixture creation time underflow".to_string())?;
    let expires_at = Timestamp::from_micros_since_unix_epoch(expires_micros);
    let created_at = Timestamp::from_micros_since_unix_epoch(created_micros);
    ctx.db
        .game_auction_operation_receipt()
        .insert(AuctionOperationReceipt {
            operation_id: EXPIRY_FIXTURE_OPERATION_ID,
            auction_id: EXPIRY_FIXTURE_AUCTION_ID,
            actor_guid: EXPIRY_FIXTURE_SELLER_GUID,
            item_guid: 509_0063,
            item_entry: EXPIRY_FIXTURE_ITEM.entry,
            item_stack_count: EXPIRY_FIXTURE_ITEM.stack_count,
            item_durability: EXPIRY_FIXTURE_ITEM.durability,
            item_enchant_id: EXPIRY_FIXTURE_ITEM.enchant_id,
            item_soulbound: EXPIRY_FIXTURE_ITEM.soulbound,
            start_bid: 100,
            buyout: 500,
            duration_minutes: 720,
            deposit: 10,
            created_micros,
            expires_micros,
        });
    ctx.db.game_auction().insert(Auction {
        id: EXPIRY_FIXTURE_AUCTION_ID,
        listing_operation_id: EXPIRY_FIXTURE_OPERATION_ID,
        house: lyracore_shared::auction::STORMWIND_HOUSE_ID,
        owner_guid: EXPIRY_FIXTURE_SELLER_GUID,
        item_guid: 509_0063,
        item_entry: EXPIRY_FIXTURE_ITEM.entry,
        item_stack_count: EXPIRY_FIXTURE_ITEM.stack_count,
        item_durability: EXPIRY_FIXTURE_ITEM.durability,
        item_enchant_id: EXPIRY_FIXTURE_ITEM.enchant_id,
        item_soulbound: EXPIRY_FIXTURE_ITEM.soulbound,
        start_bid: 100,
        buyout: 500,
        highest_bidder_guid: EXPIRY_FIXTURE_WINNER_GUID,
        highest_bid: 201,
        deposit: 10,
        created_at,
        expires_at,
        revision: 3,
    });
    ctx.db.game_auction_expiry().insert(AuctionExpiry {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Time(expires_at),
        auction_id: EXPIRY_FIXTURE_AUCTION_ID,
    });
    Ok(())
}

/// Re-drive the callback body after settlement to prove the missing Auction is a durable no-op.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_replay_auction_expiry_fixture(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    expire_active(&mut CtxExpiry { ctx }, EXPIRY_FIXTURE_AUCTION_ID)
}

/// Verify the scheduler committed the exact bid-expiry mail and removed only active state.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_verify_auction_expiry_fixture(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if ctx
        .db
        .game_auction()
        .id()
        .find(EXPIRY_FIXTURE_AUCTION_ID)
        .is_some()
        || ctx
            .db
            .game_auction_expiry()
            .auction_id()
            .find(EXPIRY_FIXTURE_AUCTION_ID)
            .is_some()
    {
        return Err("expired Auction or schedule is still active".to_string());
    }
    if ctx
        .db
        .game_auction_operation_receipt()
        .operation_id()
        .find(EXPIRY_FIXTURE_OPERATION_ID)
        .is_none()
    {
        return Err("expiry removed the durable listing receipt".to_string());
    }

    let winner = auction_fixture_mail(ctx, EXPIRY_FIXTURE_WINNER_GUID, "Auction won")?;
    if winner.sender_guid != EXPIRY_FIXTURE_SELLER_GUID
        || winner.money != 0
        || winner.cod != 0
        || winner.was_read
        || !winner.body.is_empty()
        || winner.snapshot() != EXPIRY_FIXTURE_ITEM
    {
        return Err("expiry winner item mail changed".to_string());
    }

    let seller = auction_fixture_mail(ctx, EXPIRY_FIXTURE_SELLER_GUID, "Auction sold")?;
    if seller.sender_guid != EXPIRY_FIXTURE_WINNER_GUID
        || seller.money != 201
        || seller.cod != 0
        || seller.was_read
        || !seller.body.is_empty()
        || !seller.snapshot().is_empty()
    {
        return Err("expiry seller proceeds mail changed".to_string());
    }
    Ok(())
}

/// Character deletion must not destroy value held by or listed for that character.
pub(crate) fn character_has_auction_value(ctx: &ReducerContext, character_guid: u64) -> bool {
    ctx.db
        .game_auction_bid_hold()
        .by_bidder()
        .filter(character_guid)
        .any(|hold| hold.outcome == BID_PENDING || hold.deferred_refund != 0)
        || ctx
            .db
            .game_auction_hold()
            .by_seller()
            .filter(character_guid)
            .next()
            .is_some()
        || ctx
            .db
            .game_auction()
            .by_owner()
            .filter(character_guid)
            .next()
            .is_some()
        || ctx
            .db
            .game_auction()
            .by_highest_bidder()
            .filter(character_guid)
            .next()
            .is_some()
}

fn prepare_listing(
    item: Option<&ListingItem>,
    seller_guid: u64,
    seller_money: u32,
    terms: ListingTerms,
) -> Result<u32, ListingRefusal> {
    let Some(item) = item else {
        return Err(ListingRefusal::ItemNotFound);
    };
    if item.owner_guid != seller_guid
        || item.slot < FIRST_BACKPACK_SLOT
        || !crate::items::is_carried_slot(item.slot)
        || !item.mailable
        || item.snapshot.stack_count == 0
        || item.snapshot.soulbound
    {
        return Err(ListingRefusal::ItemNotFound);
    }
    if terms.start_bid == 0 || (terms.buyout != 0 && terms.buyout < terms.start_bid) {
        return Err(ListingRefusal::InvalidTerms);
    }
    let deposit = listing_deposit(
        item.sell_price,
        item.snapshot.stack_count,
        terms.duration_minutes,
    )
    .ok_or(ListingRefusal::InvalidTerms)?;
    if !listing_proceeds_are_representable(terms, deposit) {
        return Err(ListingRefusal::InvalidTerms);
    }
    if seller_money < deposit {
        return Err(ListingRefusal::NotEnoughMoney);
    }
    Ok(deposit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seller_proceeds_apply_the_stormwind_cut_and_return_the_deposit_checked() {
        assert_eq!(seller_proceeds(100, 10), Some(105));
        assert_eq!(seller_proceeds(19, 1), Some(20));
        assert_eq!(seller_proceeds(20, 1), Some(20));
        assert_eq!(seller_proceeds(u32::MAX, u32::MAX), None);
    }

    #[test]
    fn listing_deposit_uses_the_stormwind_rate_and_supported_duration_ladder() {
        assert_eq!(listing_deposit(100, 2, 720), Some(10));
        assert_eq!(listing_deposit(100, 2, 1_440), Some(20));
        assert_eq!(listing_deposit(100, 2, 2_880), Some(40));
        assert_eq!(listing_deposit(1, 1, 720), Some(1));
        assert_eq!(listing_deposit(100, 1, 60), None);
        assert_eq!(listing_deposit(u32::MAX, u32::MAX, 2_880), None);
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
            },
            sell_price: 100,
        }
    }

    fn active_bid_auction() -> BidAuction {
        let snapshot = item(23).snapshot;
        BidAuction {
            id: 41,
            owner_guid: 7,
            item: snapshot,
            highest_bidder_guid: 0,
            highest_bid: 0,
            start_bid: 100,
            buyout: 0,
            deposit: 10,
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
            duration_minutes: 720,
        }
    }

    #[test]
    fn listing_accepts_only_one_owned_mailable_bag_stack_and_valid_terms() {
        assert_eq!(prepare_listing(Some(&item(23)), 7, 10, terms()), Ok(10));
        assert_eq!(prepare_listing(Some(&item(120)), 7, 10, terms()), Ok(10));

        for slot in [15, 19, 39, 63, 119, 192] {
            assert_eq!(
                prepare_listing(Some(&item(slot)), 7, 10, terms()),
                Err(ListingRefusal::ItemNotFound),
                "slot {slot} must not be auctionable"
            );
        }

        let mut foreign = item(23);
        foreign.owner_guid = 8;
        assert_eq!(
            prepare_listing(Some(&foreign), 7, 10, terms()),
            Err(ListingRefusal::ItemNotFound)
        );

        let mut soulbound = item(23);
        soulbound.snapshot.soulbound = true;
        assert_eq!(
            prepare_listing(Some(&soulbound), 7, 10, terms()),
            Err(ListingRefusal::ItemNotFound)
        );

        let mut not_mailable = item(120);
        not_mailable.mailable = false;
        assert_eq!(
            prepare_listing(Some(&not_mailable), 7, 10, terms()),
            Err(ListingRefusal::ItemNotFound)
        );

        assert_eq!(
            prepare_listing(None, 7, 10, terms()),
            Err(ListingRefusal::ItemNotFound)
        );
        assert_eq!(
            prepare_listing(Some(&item(23)), 7, 9, terms()),
            Err(ListingRefusal::NotEnoughMoney)
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
        ] {
            assert_eq!(
                prepare_listing(Some(&item(23)), 7, 10, invalid),
                Err(ListingRefusal::InvalidTerms)
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
        assert_eq!(listing.expires_micros, 43_200_001_000);

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

    #[derive(Clone)]
    struct FakeSource {
        money: Option<u32>,
        item: Option<ListingItem>,
        now_micros: i64,
        hold: Option<ListingHold>,
        receipt: Option<ListingReceipt>,
    }

    impl ListingSource for FakeSource {
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

    impl HoldSink for FakeSource {
        fn hold(&self, operation_id: u64) -> Option<ListingHold> {
            self.hold
                .as_ref()
                .filter(|hold| hold.listing.request.operation_id == operation_id)
                .cloned()
        }

        fn receipt(&self, operation_id: u64) -> Option<ListingReceipt> {
            self.receipt
                .as_ref()
                .filter(|receipt| receipt.listing.request.operation_id == operation_id)
                .cloned()
        }

        fn commit_hold(&mut self, hold: ListingHold) {
            self.money = self.money.map(|money| money - hold.listing.deposit);
            self.item = None;
            self.hold = Some(hold);
        }

        fn confirm_hold(&mut self, receipt: ListingReceipt) {
            self.receipt = Some(receipt);
        }

        fn delete_hold(&mut self, operation_id: u64) {
            if self
                .hold
                .as_ref()
                .is_some_and(|hold| hold.listing.request.operation_id == operation_id)
            {
                self.hold = None;
            }
        }
    }

    #[derive(Clone)]
    struct FakeMarket {
        next_auction_id: u32,
        receipt: Option<ListingReceipt>,
        auction_count: usize,
    }

    impl MarketSink for FakeMarket {
        fn receipt(&self, operation_id: u64) -> Option<ListingReceipt> {
            self.receipt
                .as_ref()
                .filter(|receipt| receipt.listing.request.operation_id == operation_id)
                .cloned()
        }

        fn commit_market(&mut self, listing: PreparedListing) -> u32 {
            let auction_id = self.next_auction_id;
            self.next_auction_id += 1;
            self.auction_count += 1;
            self.receipt = Some(ListingReceipt {
                listing,
                auction_id,
            });
            auction_id
        }
    }

    fn source() -> FakeSource {
        FakeSource {
            money: Some(50),
            item: Some(item(23)),
            now_micros: 1_000,
            hold: None,
            receipt: None,
        }
    }

    #[test]
    fn listing_rejects_overflowing_possible_proceeds_before_moving_value() {
        for terms in [
            ListingTerms {
                start_bid: u32::MAX,
                buyout: 0,
                duration_minutes: 720,
            },
            ListingTerms {
                start_bid: 10,
                buyout: u32::MAX,
                duration_minutes: 720,
            },
        ] {
            let mut local = local();
            local.money = Some(u32::MAX);
            local.item.as_mut().unwrap().sell_price = u32::MAX;
            let before = local.clone();
            let request = ListingRequest { terms, ..request() };

            assert_eq!(
                create_local_listing(&mut local, request),
                Err(ListingRefusal::InvalidTerms)
            );
            assert_eq!(local.money, before.money);
            assert_eq!(local.item, before.item);
            assert!(local.committed.is_none());

            let mut source = source();
            source.money = Some(u32::MAX);
            source.item.as_mut().unwrap().sell_price = u32::MAX;
            let before = source.clone();

            assert_eq!(
                fence_listing(&mut source, request),
                Err(ListingRefusal::InvalidTerms)
            );
            assert_eq!(source.money, before.money);
            assert_eq!(source.item, before.item);
            assert!(source.hold.is_none());
            assert!(source.receipt.is_none());
        }
    }

    fn market() -> FakeMarket {
        FakeMarket {
            next_auction_id: 41,
            receipt: None,
            auction_count: 0,
        }
    }

    fn drive_sharded(
        source: &mut FakeSource,
        market: &mut FakeMarket,
        request: ListingRequest,
    ) -> Result<u32, ListingRefusal> {
        fence_listing(source, request)?;
        if let Some(receipt) = source.receipt(request.operation_id) {
            settle_listing(source, request.operation_id)?;
            return Ok(receipt.auction_id);
        }
        let hold = source
            .hold(request.operation_id)
            .expect("a successful fresh fence leaves a durable Hold");
        let auction_id = commit_held_listing(market, hold.listing.clone())?;
        confirm_listing(
            source,
            ListingReceipt {
                listing: hold.listing,
                auction_id,
            },
        )?;
        settle_listing(source, request.operation_id)?;
        Ok(auction_id)
    }

    #[test]
    fn sharded_listing_retries_every_interruption_without_losing_or_duplicating_value() {
        for killed_after in 0..=3 {
            let mut source = source();
            let mut market = market();
            let request = request();

            fence_listing(&mut source, request).expect("fenced");
            if killed_after >= 1 {
                let hold = source.hold(request.operation_id).unwrap();
                let auction_id = commit_held_listing(&mut market, hold.listing.clone()).unwrap();
                if killed_after >= 2 {
                    confirm_listing(
                        &mut source,
                        ListingReceipt {
                            listing: hold.listing,
                            auction_id,
                        },
                    )
                    .unwrap();
                    if killed_after >= 3 {
                        settle_listing(&mut source, request.operation_id).unwrap();
                    }
                }
            }

            assert_eq!(drive_sharded(&mut source, &mut market, request), Ok(41));
            assert_eq!(source.money, Some(40));
            assert!(source.item.is_none());
            assert!(source.hold.is_none(), "Hold is deleted last");
            assert_eq!(source.receipt.as_ref().unwrap().auction_id, 41);
            assert_eq!(market.auction_count, 1);
            assert_eq!(
                market.receipt.as_ref().unwrap().listing.snapshot,
                item(23).snapshot
            );

            assert_eq!(drive_sharded(&mut source, &mut market, request), Ok(41));
            assert_eq!(source.money, Some(40));
            assert_eq!(market.auction_count, 1);
        }
    }

    #[test]
    fn sharded_listing_accounts_for_the_item_and_deposit_at_every_durable_boundary() {
        let mut source = source();
        let mut market = market();
        let request = request();

        assert_eq!(source.money, Some(50));
        assert_eq!(
            source.item.as_ref().map(|item| item.snapshot),
            Some(item(23).snapshot)
        );
        assert!(source.hold.is_none());
        assert!(market.receipt.is_none());

        fence_listing(&mut source, request).unwrap();
        let hold = source.hold(request.operation_id).unwrap();
        assert_eq!(source.money, Some(40));
        assert!(source.item.is_none());
        assert_eq!(hold.listing.snapshot, item(23).snapshot);
        assert_eq!(40 + hold.listing.deposit, 50);
        assert!(market.receipt.is_none());

        let auction_id = commit_held_listing(&mut market, hold.listing.clone()).unwrap();
        let market_receipt = market.receipt.as_ref().unwrap();
        assert_eq!(auction_id, 41);
        assert_eq!(market_receipt.listing.snapshot, item(23).snapshot);
        assert_eq!(40 + market_receipt.listing.deposit, 50);
        assert_eq!(
            source.hold.as_ref().unwrap().listing.snapshot,
            market_receipt.listing.snapshot,
            "the source Hold is recovery evidence for the one market item"
        );

        confirm_listing(
            &mut source,
            ListingReceipt {
                listing: hold.listing,
                auction_id,
            },
        )
        .unwrap();
        assert_eq!(
            source.receipt.as_ref().unwrap(),
            market.receipt.as_ref().unwrap()
        );
        assert!(source.hold.is_some());

        settle_listing(&mut source, request.operation_id).unwrap();
        assert!(source.hold.is_none());
        assert_eq!(source.money, Some(40));
        assert_eq!(market.auction_count, 1);
        assert_eq!(
            market.receipt.as_ref().unwrap().listing.snapshot,
            item(23).snapshot
        );
        assert_eq!(40 + market.receipt.as_ref().unwrap().listing.deposit, 50);
    }

    #[test]
    fn conflicting_operation_id_reuse_fails_closed_on_both_planes() {
        let mut source = source();
        let mut market = market();
        let original = request();
        fence_listing(&mut source, original).unwrap();
        let hold = source.hold(original.operation_id).unwrap();
        commit_held_listing(&mut market, hold.listing).unwrap();

        let mut changed = original;
        changed.terms.buyout += 1;
        assert_eq!(
            fence_listing(&mut source, changed),
            Err(ListingRefusal::InvalidTerms)
        );

        let mut changed_payload = source.hold(original.operation_id).unwrap().listing;
        changed_payload.snapshot.durability += 1;
        assert_eq!(
            commit_held_listing(&mut market, changed_payload),
            Err(ListingRefusal::InvalidTerms)
        );
        assert_eq!(source.money, Some(40));
        assert_eq!(market.auction_count, 1);
    }

    #[derive(Clone)]
    struct FakeExpiry {
        auction: Option<ActiveAuction>,
        schedule_count: usize,
        mail: Vec<AuctionMail>,
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

        fn complete_expiry(&mut self, _auction: ActiveAuction, completion: ExpiryCompletion) {
            match completion {
                ExpiryCompletion::Unsold(mail) => {
                    self.returned_items.push(mail.item);
                    self.mail.push(mail);
                }
                ExpiryCompletion::Sold(mail) => self.mail.extend(mail),
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
            expires_micros: 43_200_001_000,
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
    fn local_and_sharded_listings_produce_the_same_unbid_and_bid_expiry_mail() {
        let mut local_store = local();
        create_local_listing(&mut local_store, request()).unwrap();
        let local_listing = local_store.committed.unwrap().0;

        let mut source = source();
        let mut market = market();
        drive_sharded(&mut source, &mut market, request()).unwrap();
        let sharded_listing = market.receipt.unwrap().listing;
        assert_eq!(local_listing, sharded_listing);

        let expire = |listing: PreparedListing, highest_bidder_guid, highest_bid| {
            let mut store = FakeExpiry {
                auction: Some(ActiveAuction {
                    id: 41,
                    listing,
                    highest_bidder_guid,
                    highest_bid,
                }),
                schedule_count: 1,
                mail: Vec::new(),
                returned_items: Vec::new(),
                refunded_copper: 0,
            };
            expire_active(&mut store, 41).unwrap();
            store
        };
        for (highest_bidder_guid, highest_bid) in [(0, 0), (8, 201)] {
            let local_expiry = expire(local_listing.clone(), highest_bidder_guid, highest_bid);
            let sharded_expiry = expire(sharded_listing.clone(), highest_bidder_guid, highest_bid);
            assert_eq!(local_expiry.mail, sharded_expiry.mail);
            assert_eq!(local_expiry.returned_items, sharded_expiry.returned_items);
            assert_eq!(local_expiry.refunded_copper, sharded_expiry.refunded_copper);
            assert_eq!(local_expiry.schedule_count, sharded_expiry.schedule_count);
        }
    }

    #[test]
    fn replayed_bid_expiry_mails_the_exact_item_and_stormwind_proceeds_once() {
        let listing = PreparedListing {
            request: request(),
            snapshot: item(23).snapshot,
            deposit: 10,
            created_micros: 1_000,
            expires_micros: 43_200_001_000,
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
                    sender_guid: 7,
                    subject: "Auction won",
                    money: 0,
                    item: item(23).snapshot,
                },
                AuctionMail {
                    recipient_guid: 7,
                    sender_guid: 8,
                    subject: "Auction sold",
                    money: 201,
                    item: crate::items::ItemSnapshot::default(),
                },
            ]
        );
    }

    #[test]
    fn realm_bid_decision_enforces_the_vanilla_price_ladder_and_revision() {
        let active = active_bid_auction();
        let request = BidRequest {
            operation_id: 900,
            bidder_guid: 8,
            auction_id: 41,
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
            decide_bid(Some(bid), BidRequest { offer: 101, ..request }, 1_000),
            BidDecision::HigherBid {
                bidder_guid: 9,
                current_bid: 101,
                minimum_increment: 6,
            }
        );
        assert_eq!(
            decide_bid(Some(bid), BidRequest { offer: 106, ..request }, 1_000),
            BidDecision::BidIncrement
        );
        assert_eq!(
            decide_bid(Some(active), BidRequest { offer: 99, ..request }, 1_000),
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
            decide_bid(Some(active), BidRequest { operation_id: 0, ..request }, 1_000),
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
    fn realm_buyout_normalizes_the_offer_and_checks_settlement_arithmetic() {
        let active = BidAuction {
            highest_bidder_guid: 9,
            highest_bid: 201,
            buyout: 500,
            ..active_bid_auction()
        };
        let request = BidRequest {
            operation_id: 905,
            bidder_guid: 8,
            auction_id: 41,
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

    #[derive(Clone, Copy)]
    struct FakeBidSource {
        money: u32,
        deferred_refund: u32,
        hold: Option<HeldBid>,
    }

    impl FakeBidSource {
        fn new(money: u32) -> Self {
            Self {
                money,
                deferred_refund: 0,
                hold: None,
            }
        }
    }

    impl BidSource for FakeBidSource {
        fn money(&self, _bidder_guid: u64) -> Option<u32> {
            Some(self.money)
        }

        fn hold(&self, operation_id: u64) -> Option<HeldBid> {
            self.hold
                .filter(|hold| hold.request.operation_id == operation_id)
        }

        fn create_hold(&mut self, request: BidRequest) -> Result<(), BidRefusal> {
            self.money -= request.offer;
            self.hold = Some(HeldBid {
                request,
                decision: None,
                deferred_refund: 0,
            });
            Ok(())
        }

        fn finish_hold(
            &mut self,
            request: BidRequest,
            decision: BidDecision,
        ) -> Result<(), BidRefusal> {
            let refund =
                refundable_bid_value(decision, request.offer).ok_or(BidRefusal::Database)?;
            if refund != 0 {
                let (money, deferred_refund) = split_bid_refund(self.money, refund);
                self.money = money;
                self.deferred_refund += deferred_refund;
            }
            self.hold = Some(HeldBid {
                request,
                decision: Some(decision),
                deferred_refund: self.deferred_refund,
            });
            Ok(())
        }

        fn confirm_refund(&mut self, request: BidRequest) -> Result<(), BidRefusal> {
            self.deferred_refund = 0;
            self.hold = self.hold.map(|mut hold| {
                assert_eq!(hold.request, request);
                hold.deferred_refund = 0;
                hold
            });
            Ok(())
        }
    }

    #[test]
    fn bid_hold_decisions_are_terminal_replay_safe_and_conserve_copper() {
        let request = BidRequest {
            operation_id: 901,
            bidder_guid: 8,
            auction_id: 41,
            offer: 107,
        };
        let rejected = BidDecision::BidIncrement;
        let accepted = accepted_active(107, 4, 9, 101);

        let mut rejection = FakeBidSource::new(200);
        assert_eq!(fence_bid(&mut rejection, request), Ok(()));
        assert_eq!(rejection.money, 93, "the full offer is held first");
        assert_eq!(finish_bid(&mut rejection, request, rejected), Ok(rejected));
        assert_eq!(rejection.money, 200, "a rejection restores the offer");
        assert_eq!(finish_bid(&mut rejection, request, rejected), Ok(rejected));
        assert_eq!(rejection.money, 200, "replay cannot restore twice");

        let mut overflowed_rejection = FakeBidSource::new(200);
        assert_eq!(fence_bid(&mut overflowed_rejection, request), Ok(()));
        overflowed_rejection.money = u32::MAX;
        assert_eq!(
            finish_bid(&mut overflowed_rejection, request, rejected),
            Ok(rejected),
            "an intervening purse credit cannot strand a rejected Hold"
        );
        assert_eq!(overflowed_rejection.money, u32::MAX);
        assert_eq!(overflowed_rejection.deferred_refund, request.offer);
        assert_eq!(
            finish_bid(&mut overflowed_rejection, request, rejected),
            Ok(rejected)
        );
        assert_eq!(
            overflowed_rejection.deferred_refund,
            request.offer,
            "replay cannot defer the refund twice"
        );
        assert_eq!(
            confirm_bid_refund(&mut overflowed_rejection, request, request.offer),
            Ok(())
        );
        assert_eq!(overflowed_rejection.deferred_refund, 0);
        assert_eq!(
            confirm_bid_refund(&mut overflowed_rejection, request, request.offer),
            Ok(()),
            "confirmation replay is idempotent"
        );

        let mut acceptance = FakeBidSource::new(200);
        assert_eq!(fence_bid(&mut acceptance, request), Ok(()));
        assert_eq!(finish_bid(&mut acceptance, request, accepted), Ok(accepted));
        assert_eq!(finish_bid(&mut acceptance, request, accepted), Ok(accepted));
        assert_eq!(acceptance.money, 93, "accepted value is consumed once");

        let normalized = accepted_active(100, 4, 9, 101);
        let mut normalized_acceptance = FakeBidSource::new(200);
        assert_eq!(fence_bid(&mut normalized_acceptance, request), Ok(()));
        assert_eq!(
            finish_bid(&mut normalized_acceptance, request, normalized),
            Ok(normalized)
        );
        assert_eq!(normalized_acceptance.money, 100);
        assert_eq!(
            finish_bid(&mut normalized_acceptance, request, normalized),
            Ok(normalized)
        );
        assert_eq!(normalized_acceptance.money, 100);

        let mut interrupted_normalization = FakeBidSource::new(200);
        fence_bid(&mut interrupted_normalization, request).unwrap();
        interrupted_normalization.money = u32::MAX;
        finish_bid(&mut interrupted_normalization, request, normalized).unwrap();
        assert_eq!(interrupted_normalization.deferred_refund, 7);
        assert_eq!(
            confirm_bid_refund(&mut interrupted_normalization, request, 7),
            Ok(())
        );
        assert_eq!(interrupted_normalization.deferred_refund, 0);

        assert_eq!(
            finish_bid(&mut acceptance, BidRequest { offer: 108, ..request }, accepted),
            Err(BidRefusal::Database),
            "changed-payload identifier reuse fails closed"
        );

        for malformed in [
            BidRequest {
                operation_id: 0,
                ..request
            },
            BidRequest {
                bidder_guid: 0,
                ..request
            },
            BidRequest {
                auction_id: 0,
                ..request
            },
            BidRequest {
                offer: 0,
                ..request
            },
        ] {
            let mut source = FakeBidSource::new(200);
            assert_eq!(fence_bid(&mut source, malformed), Err(BidRefusal::Database));
            assert_eq!(source.money, 200);
            assert!(source.hold.is_none());
        }
        let mut poor = FakeBidSource::new(106);
        assert_eq!(
            fence_bid(&mut poor, request),
            Err(BidRefusal::NotEnoughMoney)
        );
        assert_eq!(poor.money, 106);
        assert!(poor.hold.is_none());
    }

    #[derive(Clone)]
    struct FakeBidMarket {
        auction: Option<BidAuction>,
        decisions: Vec<(BidRequest, BidDecision)>,
        mail: Vec<AuctionMail>,
        expiry_armed: bool,
        now_micros: i64,
    }

    impl FakeBidMarket {
        fn mailbox(&self, recipient_guid: u64) -> Vec<AuctionMail> {
            self.mail
                .iter()
                .copied()
                .filter(|mail| mail.recipient_guid == recipient_guid)
                .collect()
        }
    }

    impl BidMarket for FakeBidMarket {
        fn decision(&self, operation_id: u64) -> Option<(BidRequest, BidDecision)> {
            self.decisions
                .iter()
                .copied()
                .find(|(request, _)| request.operation_id == operation_id)
        }

        fn auction(&self, auction_id: u32) -> Option<BidAuction> {
            self.auction.filter(|auction| auction.id == auction_id)
        }

        fn now_micros(&self) -> i64 {
            self.now_micros
        }

        fn commit_decision(
            &mut self,
            request: BidRequest,
            auction: Option<BidAuction>,
            decision: BidDecision,
        ) -> Result<(), BidRefusal> {
            if let BidDecision::Accepted(accepted) = decision {
                let mut auction = auction.expect("only an active Auction can accept a bid");
                self.mail.extend(displaced_bid_refund_mail(
                    accepted.displaced_bidder_guid,
                    accepted.displaced_bid,
                ));
                match accepted.effect {
                    AuctionBidEffect::SettleBuyout => {
                        self.expiry_armed = false;
                        self.mail.extend(
                            buyout_settlement_mail(auction, request.bidder_guid, accepted.price)
                                .expect("accepted buyout arithmetic was checked"),
                        );
                        self.auction = None;
                    }
                    AuctionBidEffect::RemainActive { revision } => {
                        auction.highest_bidder_guid = request.bidder_guid;
                        auction.highest_bid = accepted.price;
                        auction.revision = revision;
                        self.auction = Some(auction);
                    }
                }
            }
            self.decisions.push((request, decision));
            Ok(())
        }
    }

    struct BidMarketExpiry<'a> {
        market: &'a mut FakeBidMarket,
        listing: PreparedListing,
    }

    impl ExpirySink for BidMarketExpiry<'_> {
        fn auction(&self, auction_id: u32) -> Result<Option<ActiveAuction>, String> {
            Ok(self
                .market
                .auction
                .filter(|auction| auction.id == auction_id)
                .map(|auction| ActiveAuction {
                    id: auction.id,
                    listing: self.listing.clone(),
                    highest_bidder_guid: auction.highest_bidder_guid,
                    highest_bid: auction.highest_bid,
                }))
        }

        fn complete_expiry(&mut self, _auction: ActiveAuction, completion: ExpiryCompletion) {
            match completion {
                ExpiryCompletion::Unsold(mail) => self.market.mail.push(mail),
                ExpiryCompletion::Sold(mail) => self.market.mail.extend(mail),
            }
            self.market.auction = None;
            self.market.expiry_armed = false;
        }
    }

    fn expire_bid_market(
        market: &mut FakeBidMarket,
        listing: PreparedListing,
    ) -> Result<(), String> {
        let auction_id = market.auction.map_or(41, |auction| auction.id);
        expire_active(&mut BidMarketExpiry { market, listing }, auction_id)
    }

    struct FakeBidRefundSink {
        request: BidRequest,
        decision: BidDecision,
        recorded: u32,
        mails: Vec<(u64, u32)>,
    }

    impl BidRefundSink for FakeBidRefundSink {
        fn refund_decision(
            &self,
            operation_id: u64,
        ) -> Option<(BidRequest, BidDecision, u32)> {
            (self.request.operation_id == operation_id)
                .then_some((self.request, self.decision, self.recorded))
        }

        fn commit_refund(
            &mut self,
            request: BidRequest,
            amount: u32,
        ) -> Result<(), BidRefusal> {
            self.recorded = amount;
            self.mails.push((request.bidder_guid, amount));
            Ok(())
        }
    }

    #[test]
    fn deferred_bid_refund_relay_is_terminal_and_payload_safe() {
        let request = BidRequest {
            operation_id: 904,
            bidder_guid: 8,
            auction_id: 41,
            offer: 107,
        };
        let mut sink = FakeBidRefundSink {
            request,
            decision: BidDecision::BidIncrement,
            recorded: 0,
            mails: Vec::new(),
        };

        assert_eq!(relay_bid_refund(&mut sink, request, 7), Ok(()));
        assert_eq!(relay_bid_refund(&mut sink, request, 7), Ok(()));
        assert_eq!(sink.mails, vec![(8, 7)]);
        assert_eq!(
            relay_bid_refund(&mut sink, BidRequest { offer: 108, ..request }, 7),
            Err(BidRefusal::Database)
        );
        assert_eq!(
            relay_bid_refund(&mut sink, request, 8),
            Err(BidRefusal::Database)
        );

        sink.recorded = 0;
        sink.decision = accepted_active(100, 4, 9, 101);
        assert_eq!(relay_bid_refund(&mut sink, request, 7), Ok(()));
        assert_eq!(relay_bid_refund(&mut sink, request, 7), Ok(()));
        assert_eq!(sink.mails, vec![(8, 7), (8, 7)]);
        assert_eq!(
            relay_bid_refund(&mut sink, request, 8),
            Err(BidRefusal::Database)
        );
    }

    #[test]
    fn realm_bid_replay_updates_once_and_returns_the_displaced_bid_once() {
        let request = BidRequest {
            operation_id: 902,
            bidder_guid: 8,
            auction_id: 41,
            offer: 107,
        };
        let mut market = FakeBidMarket {
            auction: Some(BidAuction {
                highest_bidder_guid: 9,
                highest_bid: 101,
                ..active_bid_auction()
            }),
            decisions: Vec::new(),
            mail: Vec::new(),
            expiry_armed: true,
            now_micros: 1_000,
        };

        let expected = accepted_active(107, 4, 9, 101);
        assert_eq!(resolve_bid(&mut market, request), Ok(expected));
        assert_eq!(resolve_bid(&mut market, request), Ok(expected));
        assert_eq!(
            market.mail,
            vec![displaced_bid_refund_mail(9, 101).unwrap()]
        );
        assert_eq!(
            market.auction.map(|auction| (
                auction.highest_bidder_guid,
                auction.highest_bid,
                auction.revision,
            )),
            Some((8, 107, 4))
        );

        assert_eq!(
            resolve_bid(&mut market, BidRequest { offer: 108, ..request }),
            Err(BidRefusal::Database)
        );
        assert_eq!(
            market.mail,
            vec![displaced_bid_refund_mail(9, 101).unwrap()]
        );
    }

    #[test]
    fn local_and_interrupted_sharded_bids_and_buyouts_have_equivalent_state() {
        for (operation_id, offer) in [(903, 107), (904, 900)] {
            let request = BidRequest {
                operation_id,
                bidder_guid: 8,
                auction_id: 41,
                offer,
            };
            let source = FakeBidSource::new(1_000);
            let market = FakeBidMarket {
                auction: Some(BidAuction {
                    highest_bidder_guid: 9,
                    highest_bid: 101,
                    buyout: 500,
                    ..active_bid_auction()
                }),
                decisions: Vec::new(),
                mail: Vec::new(),
                expiry_armed: true,
                now_micros: 1_000,
            };

            let mut local_source = source;
            let mut local_market = market.clone();
            let expected = drive_bid(&mut local_source, &mut local_market, request).unwrap();

            for killed_after in 0..=2 {
                let mut sharded_source = source;
                let mut sharded_market = market.clone();
                if killed_after >= 1 {
                    fence_bid(&mut sharded_source, request).unwrap();
                }
                if killed_after >= 2 {
                    resolve_bid(&mut sharded_market, request).unwrap();
                }

                assert_eq!(
                    drive_bid(&mut sharded_source, &mut sharded_market, request),
                    Ok(expected)
                );
                assert_eq!(sharded_source.money, local_source.money);
                assert_eq!(sharded_source.hold, local_source.hold);
                assert_eq!(sharded_market.auction, local_market.auction);
                assert_eq!(sharded_market.decisions, local_market.decisions);
                assert_eq!(sharded_market.mail, local_market.mail);
                assert_eq!(sharded_market.expiry_armed, local_market.expiry_armed);
                assert_eq!(
                    drive_bid(&mut sharded_source, &mut sharded_market, request),
                    Ok(expected)
                );
                assert_eq!(sharded_market.decisions, local_market.decisions);
                assert_eq!(sharded_market.mail, local_market.mail);
            }
        }
    }

    fn listing_for_flow(sharded: bool, operation_id: u64) -> PreparedListing {
        let request = ListingRequest {
            operation_id,
            ..request()
        };
        if sharded {
            let mut source = source();
            let mut market = market();
            fence_listing(&mut source, request).unwrap();
            assert_eq!(drive_sharded(&mut source, &mut market, request), Ok(41));
            assert_eq!(drive_sharded(&mut source, &mut market, request), Ok(41));
            market.receipt.unwrap().listing
        } else {
            let mut store = local();
            assert_eq!(create_local_listing(&mut store, request), Ok(41));
            assert_eq!(create_local_listing(&mut store, request), Ok(41));
            store.committed.unwrap().0
        }
    }

    fn bid_market_for(listing: &PreparedListing) -> FakeBidMarket {
        FakeBidMarket {
            auction: Some(BidAuction {
                id: 41,
                owner_guid: listing.request.seller_guid,
                item: listing.snapshot,
                highest_bidder_guid: 0,
                highest_bid: 0,
                start_bid: listing.request.terms.start_bid,
                buyout: listing.request.terms.buyout,
                deposit: listing.deposit,
                expires_micros: listing.expires_micros,
                revision: 0,
            }),
            decisions: Vec::new(),
            mail: Vec::new(),
            expiry_armed: true,
            now_micros: listing.created_micros,
        }
    }

    fn bid_for_flow(
        sharded: bool,
        source: &mut FakeBidSource,
        market: &mut FakeBidMarket,
        request: BidRequest,
    ) -> BidDecision {
        if sharded {
            fence_bid(source, request).unwrap();
            resolve_bid(market, request).unwrap();
        }
        let decision = drive_bid(source, market, request).unwrap();
        assert_eq!(drive_bid(source, market, request), Ok(decision));
        decision
    }

    #[derive(Debug, PartialEq, Eq)]
    struct CompleteFlowOutcome {
        browse_search_rows: Vec<BidAuction>,
        decisions: Vec<BidDecision>,
        collected_mail: Vec<AuctionMail>,
        collected_copper: Vec<(u64, u32)>,
        collected_items: Vec<(u64, crate::items::ItemSnapshot)>,
        bidder_money: Vec<u32>,
    }

    fn collect_flow_mail(outcome: &mut CompleteFlowOutcome, mail: Vec<AuctionMail>) {
        for mail in mail {
            if mail.money != 0 {
                assert_eq!(
                    crate::mail::plan_take_money(
                        Some((mail.recipient_guid, mail.money)),
                        mail.recipient_guid,
                    ),
                    crate::mail::TakeMoney::Take(mail.money),
                );
                assert_eq!(
                    crate::mail::plan_take_money(
                        Some((mail.recipient_guid, 0)),
                        mail.recipient_guid,
                    ),
                    crate::mail::TakeMoney::NothingToTake,
                );
                outcome
                    .collected_copper
                    .push((mail.recipient_guid, crate::mail::credited(0, mail.money)));
            }
            if !mail.item.is_empty() {
                assert_eq!(
                    crate::mail::plan_take_item(
                        Some((mail.recipient_guid, mail.item.entry)),
                        mail.recipient_guid,
                    ),
                    crate::mail::TakeItem::Take,
                );
                assert_eq!(
                    crate::mail::plan_take_item(
                        Some((mail.recipient_guid, 0)),
                        mail.recipient_guid,
                    ),
                    crate::mail::TakeItem::NothingToTake,
                );
                outcome.collected_items.push((mail.recipient_guid, mail.item));
            }
            outcome.collected_mail.push(mail);
        }
    }

    fn complete_flow(sharded: bool) -> CompleteFlowOutcome {
        let buyout_listing = listing_for_flow(sharded, 910);
        let mut buyout_market = bid_market_for(&buyout_listing);
        // Browse/search renders the same authoritative Auction row that the later writes consume.
        let browse_search_rows = buyout_market
            .auction
            .into_iter()
            .filter(|auction| auction.item.entry == item(23).snapshot.entry)
            .collect();
        let bid = BidRequest {
            operation_id: 911,
            bidder_guid: 8,
            auction_id: 41,
            offer: 10,
        };
        let mut bidder = FakeBidSource::new(100);
        let bid_decision = bid_for_flow(sharded, &mut bidder, &mut buyout_market, bid);
        let buyout = BidRequest {
            operation_id: 912,
            bidder_guid: 9,
            auction_id: 41,
            offer: 50,
        };
        let mut buyer = FakeBidSource::new(100);
        let buyout_decision = bid_for_flow(sharded, &mut buyer, &mut buyout_market, buyout);
        expire_bid_market(&mut buyout_market, buyout_listing.clone()).unwrap();
        let mut outcome = CompleteFlowOutcome {
            browse_search_rows,
            decisions: vec![bid_decision, buyout_decision],
            collected_mail: Vec::new(),
            collected_copper: Vec::new(),
            collected_items: Vec::new(),
            bidder_money: vec![bidder.money, buyer.money],
        };
        collect_flow_mail(&mut outcome, std::mem::take(&mut buyout_market.mail));

        let unbid_listing = listing_for_flow(sharded, 913);
        let mut unbid_market = bid_market_for(&unbid_listing);
        expire_bid_market(&mut unbid_market, unbid_listing.clone()).unwrap();
        expire_bid_market(&mut unbid_market, unbid_listing).unwrap();
        collect_flow_mail(&mut outcome, std::mem::take(&mut unbid_market.mail));

        let bid_expiry_listing = listing_for_flow(sharded, 914);
        let mut bid_expiry_market = bid_market_for(&bid_expiry_listing);
        let expiry_bid = BidRequest {
            operation_id: 915,
            bidder_guid: 8,
            auction_id: 41,
            offer: 10,
        };
        let mut expiry_bidder = FakeBidSource::new(100);
        let expiry_bid_decision = bid_for_flow(
            sharded,
            &mut expiry_bidder,
            &mut bid_expiry_market,
            expiry_bid,
        );
        expire_bid_market(&mut bid_expiry_market, bid_expiry_listing.clone()).unwrap();
        expire_bid_market(&mut bid_expiry_market, bid_expiry_listing).unwrap();
        collect_flow_mail(&mut outcome, std::mem::take(&mut bid_expiry_market.mail));
        outcome.decisions.push(expiry_bid_decision);
        outcome.bidder_money.push(expiry_bidder.money);
        outcome
    }

    #[test]
    fn complete_flow_is_equivalent_across_local_and_interrupted_sharded_topologies() {
        let local = complete_flow(false);
        let sharded = complete_flow(true);
        assert_eq!(sharded, local);
        assert_eq!(local.browse_search_rows.len(), 1);
        assert_eq!(local.collected_mail.len(), 6);
        assert_eq!(
            local
                .collected_mail
                .iter()
                .map(|mail| (mail.recipient_guid, mail.subject, mail.money))
                .collect::<Vec<_>>(),
            vec![
                (8, "Auction outbid", 10),
                (9, "Auction won", 0),
                (7, "Auction sold", 29),
                (7, "Auction expired", 0),
                (8, "Auction won", 0),
                (7, "Auction sold", 20),
            ]
        );
        assert_eq!(
            local
                .collected_items,
            vec![
                (9, item(23).snapshot),
                (7, item(23).snapshot),
                (8, item(23).snapshot),
            ],
            "buyout, unbid expiry, and bid expiry each collect one exact item"
        );
        assert_eq!(local.collected_copper, vec![(8, 10), (7, 29), (7, 20)]);
        assert_eq!(local.bidder_money, vec![90, 80, 90]);
    }

    #[test]
    fn buyout_atomically_delivers_exact_mail_and_conserves_copper() {
        let request = BidRequest {
            operation_id: 906,
            bidder_guid: 8,
            auction_id: 41,
            offer: 900,
        };
        let mut source = FakeBidSource::new(1_000);
        let mut market = FakeBidMarket {
            auction: Some(BidAuction {
                highest_bidder_guid: 9,
                highest_bid: 201,
                buyout: 500,
                ..active_bid_auction()
            }),
            decisions: Vec::new(),
            mail: Vec::new(),
            expiry_armed: true,
            now_micros: 1_000,
        };

        assert_eq!(
            drive_bid(&mut source, &mut market, request),
            Ok(accepted_buyout(500, 9, 201))
        );
        assert_eq!(source.money, 500);
        assert!(market.auction.is_none());
        assert!(!market.expiry_armed);
        assert_eq!(
            market.mailbox(9),
            vec![displaced_bid_refund_mail(9, 201).unwrap()]
        );
        let winner_mail = market.mailbox(8);
        assert_eq!(winner_mail.len(), 1, "winner mail is visible immediately");
        assert_eq!(winner_mail[0].sender_guid, 7);
        assert_eq!(winner_mail[0].subject, "Auction won");
        assert_eq!(winner_mail[0].money, 0);
        assert_eq!(winner_mail[0].item, item(23).snapshot);
        let seller_mail = market.mailbox(7);
        assert_eq!(seller_mail.len(), 1, "seller mail is visible immediately");
        assert_eq!(seller_mail[0].sender_guid, 8);
        assert_eq!(seller_mail[0].subject, "Auction sold");
        assert_eq!(seller_mail[0].money, 485);
        assert!(seller_mail[0].item.is_empty());
        assert_eq!(market.mail.len(), 3);
        assert_eq!(1_000_u64 + 201 + 10, 500_u64 + 201 + 485 + 25);

        assert_eq!(
            drive_bid(&mut source, &mut market, request),
            Ok(accepted_buyout(500, 9, 201))
        );
        assert_eq!(source.money, 500);
        assert_eq!(market.mail.len(), 3);
    }

    #[test]
    fn concurrent_buyouts_have_one_winner_and_restore_the_loser_once() {
        let winner = BidRequest {
            operation_id: 907,
            bidder_guid: 8,
            auction_id: 41,
            offer: 900,
        };
        let loser = BidRequest {
            operation_id: 908,
            bidder_guid: 10,
            auction_id: 41,
            offer: 500,
        };
        let mut winner_source = FakeBidSource::new(1_000);
        let mut loser_source = FakeBidSource::new(700);
        let mut market = FakeBidMarket {
            auction: Some(BidAuction {
                buyout: 500,
                ..active_bid_auction()
            }),
            decisions: Vec::new(),
            mail: Vec::new(),
            expiry_armed: true,
            now_micros: 1_000,
        };

        fence_bid(&mut winner_source, winner).unwrap();
        fence_bid(&mut loser_source, loser).unwrap();
        let winner_decision = resolve_bid(&mut market, winner).unwrap();
        let loser_decision = resolve_bid(&mut market, loser).unwrap();
        assert!(matches!(
            winner_decision,
            BidDecision::Accepted(BidAcceptance {
                price: 500,
                effect: AuctionBidEffect::SettleBuyout,
                ..
            })
        ));
        assert_eq!(loser_decision, BidDecision::ItemNotFound);
        finish_bid(&mut winner_source, winner, winner_decision).unwrap();
        finish_bid(&mut loser_source, loser, loser_decision).unwrap();
        assert_eq!(winner_source.money, 500);
        assert_eq!(loser_source.money, 700);
        assert_eq!(market.mail.len(), 2);
        assert_eq!(market.decisions.len(), 2);

        assert_eq!(
            drive_bid(&mut loser_source, &mut market, loser),
            Ok(BidDecision::ItemNotFound)
        );
        assert_eq!(loser_source.money, 700);
        assert_eq!(market.mail.len(), 2);
        assert_eq!(market.decisions.len(), 2);
    }

    #[test]
    fn expiry_and_buyout_race_in_either_order_completes_once_and_conserves_value() {
        let listing = PreparedListing {
            request: request(),
            snapshot: item(23).snapshot,
            deposit: 10,
            created_micros: 1_000,
            expires_micros: 43_200_001_000,
        };
        let bid_auction = BidAuction {
            highest_bidder_guid: 9,
            highest_bid: 201,
            buyout: 500,
            ..active_bid_auction()
        };
        let buyout = BidRequest {
            operation_id: 909,
            bidder_guid: 8,
            auction_id: 41,
            offer: 500,
        };

        let mut buyout_source = FakeBidSource::new(700);
        let mut buyout_market = FakeBidMarket {
            auction: Some(bid_auction),
            decisions: Vec::new(),
            mail: Vec::new(),
            expiry_armed: true,
            now_micros: 1_000,
        };
        assert_eq!(
            drive_bid(&mut buyout_source, &mut buyout_market, buyout),
            Ok(accepted_buyout(500, 9, 201))
        );
        let buyout_mail = buyout_market.mail.clone();
        expire_bid_market(&mut buyout_market, listing.clone()).unwrap();
        expire_bid_market(&mut buyout_market, listing.clone()).unwrap();
        assert_eq!(buyout_market.mail, buyout_mail);
        assert_eq!(
            u64::from(buyout_source.money)
                + buyout_market.mail.iter().map(|mail| u64::from(mail.money)).sum::<u64>()
                + 25,
            700 + 201 + 10,
            "the post-buyout purse, refund, proceeds, and cut account for all copper"
        );
        assert_eq!(
            buyout_market
                .mail
                .iter()
                .filter(|mail| !mail.item.is_empty())
                .count(),
            1
        );

        let mut expiry_market = FakeBidMarket {
            auction: Some(bid_auction),
            decisions: Vec::new(),
            mail: Vec::new(),
            expiry_armed: true,
            now_micros: 1_000,
        };
        expire_bid_market(&mut expiry_market, listing.clone()).unwrap();
        expire_bid_market(&mut expiry_market, listing).unwrap();
        let expiry_mail = expiry_market.mail.clone();
        let mut late_buyout_source = FakeBidSource::new(700);
        assert_eq!(
            drive_bid(&mut late_buyout_source, &mut expiry_market, buyout),
            Ok(BidDecision::ItemNotFound)
        );
        assert_eq!(
            drive_bid(&mut late_buyout_source, &mut expiry_market, buyout),
            Ok(BidDecision::ItemNotFound)
        );
        assert_eq!(late_buyout_source.money, 700, "the losing Hold is restored once");
        assert_eq!(expiry_market.mail, expiry_mail);
        assert_eq!(
            u64::from(late_buyout_source.money)
                + expiry_mail.iter().map(|mail| u64::from(mail.money)).sum::<u64>()
                + 10,
            700 + 201 + 10,
            "the post-expiry purse, proceeds, and cut account for all copper"
        );
        assert_eq!(
            expiry_mail
                .iter()
                .filter(|mail| !mail.item.is_empty())
                .count(),
            1
        );
    }

    #[test]
    fn expiry_preserves_inconsistent_or_overflowing_sale_state_for_repair() {
        let listing = PreparedListing {
            request: request(),
            snapshot: item(23).snapshot,
            deposit: 10,
            created_micros: 1_000,
            expires_micros: 43_200_001_000,
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
            returned_items: Vec::new(),
            refunded_copper: 0,
        };
        assert!(expire_active(&mut store, 41).is_err());
        assert!(store.auction.is_some());
        assert_eq!(store.schedule_count, 1);
        assert!(store.mail.is_empty());
    }

    #[test]
    fn auction_write_reducers_gate_before_reading_caller_named_state() {
        use crate::test_scan::code_of;

        for signature in [
            "pub fn gw_auction_list_local(",
            "pub fn gw_auction_hold_listing(",
            "pub fn realm_auction_commit_listing(",
            "pub fn realm_auction_confirm_listing(",
            "pub fn realm_auction_settle_listing(",
            "pub fn gw_auction_bid_local(",
            "pub fn gw_auction_hold_bid(",
            "pub fn realm_auction_decide_bid(",
            "pub fn gw_auction_finish_bid(",
            "pub fn realm_auction_refund_bid(",
            "pub fn gw_auction_confirm_bid_refund(",
            "pub fn debug_stage_auction_buyout_fixture(",
            "pub fn debug_verify_auction_buyout_fixture(",
            "pub fn debug_stage_auction_expiry_fixture(",
            "pub fn debug_replay_auction_expiry_fixture(",
            "pub fn debug_verify_auction_expiry_fixture(",
        ] {
            let body = code_of(include_str!("auction.rs"), signature);
            let normalized = body.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                normalized.starts_with("{ crate::helpers::require_operator(ctx)?;"),
                "`{signature}` no longer opens with the operator gate. Body was:\n{body}"
            );
        }
    }

    #[test]
    fn auction_expiry_is_scheduler_only() {
        let body = crate::test_scan::code_of(include_str!("auction.rs"), "pub fn expire_auction(");
        let normalized = body.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(normalized.starts_with(
            "{ if ctx.sender() != ctx.database_identity() { return Err(\"scheduler only\".to_string());"
        ));
    }

    #[test]
    fn character_delete_refuses_auction_value_before_the_cascade() {
        let body = crate::test_scan::code_of(include_str!("auth.rs"), "pub fn delete_character(");
        let auction_gate = body
            .find("crate::auction::character_has_auction_value")
            .expect("character deletion must check Auction value");
        let cascade = body
            .find("crate::world::cascade_delete_character")
            .expect("character deletion still needs its normal cascade");
        assert!(auction_gate < cascade, "Auction value must be fenced before deletion");
    }
}
