//! Durable Stormwind auction listings and value-preserving listing transport.

use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table, Timestamp};

use crate::{game_item_instance, game_item_template, game_world_entity};

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

trait ExpirySink {
    fn auction(&self, auction_id: u32) -> Result<Option<ActiveAuction>, String>;
    fn return_unsold(&mut self, auction: ActiveAuction);
}

fn expire_unbid<S: ExpirySink>(sink: &mut S, auction_id: u32) -> Result<(), String> {
    let Some(auction) = sink.auction(auction_id)? else {
        return Ok(());
    };
    if auction.highest_bidder_guid != 0 || auction.highest_bid != 0 {
        return Err(format!(
            "auction {auction_id} has a bid and requires sale settlement"
        ));
    }
    sink.return_unsold(auction);
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
    if listing.deposit != expected_deposit || listing.expires_micros != expected_expiry {
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

    fn return_unsold(&mut self, auction: ActiveAuction) {
        crate::mail::insert_mail(
            self.ctx,
            auction.listing.request.seller_guid,
            0,
            "Auction expired".to_string(),
            String::new(),
            0,
            0,
            &auction.listing.snapshot,
        );
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

/// Scheduler-only one-shot expiry. Replays see no active Auction and therefore create no mail.
#[reducer]
pub fn expire_auction(ctx: &ReducerContext, schedule: AuctionExpiry) -> Result<(), String> {
    if ctx.sender() != ctx.database_identity() {
        return Err("scheduler only".to_string());
    }
    expire_unbid(&mut CtxExpiry { ctx }, schedule.auction_id)
}

/// Character deletion must not destroy value held by or listed for that character.
pub(crate) fn character_has_auction_value(ctx: &ReducerContext, character_guid: u64) -> bool {
    ctx.db
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
    if seller_money < deposit {
        return Err(ListingRefusal::NotEnoughMoney);
    }
    Ok(deposit)
}

#[cfg(test)]
mod tests {
    use super::*;

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

        fn return_unsold(&mut self, auction: ActiveAuction) {
            self.returned_items.push(auction.listing.snapshot);
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
            returned_items: Vec::new(),
            refunded_copper: 0,
        };

        assert_eq!(expire_unbid(&mut store, 41), Ok(()));
        assert_eq!(expire_unbid(&mut store, 41), Ok(()));

        assert!(store.auction.is_none());
        assert_eq!(store.schedule_count, 0);
        assert_eq!(store.returned_items, vec![item(23).snapshot]);
        assert_eq!(store.refunded_copper, 0, "the listing deposit is forfeited");
    }

    #[test]
    fn local_and_sharded_listing_produce_the_same_auction_and_expiry_mail() {
        let mut local_store = local();
        create_local_listing(&mut local_store, request()).unwrap();
        let local_listing = local_store.committed.unwrap().0;

        let mut source = source();
        let mut market = market();
        drive_sharded(&mut source, &mut market, request()).unwrap();
        let sharded_listing = market.receipt.unwrap().listing;
        assert_eq!(local_listing, sharded_listing);

        let expire = |listing: PreparedListing| {
            let mut store = FakeExpiry {
                auction: Some(ActiveAuction {
                    id: 41,
                    listing,
                    highest_bidder_guid: 0,
                    highest_bid: 0,
                }),
                schedule_count: 1,
                returned_items: Vec::new(),
                refunded_copper: 0,
            };
            expire_unbid(&mut store, 41).unwrap();
            store
        };
        let local_expiry = expire(local_listing);
        let sharded_expiry = expire(sharded_listing);
        assert_eq!(local_expiry.returned_items, sharded_expiry.returned_items);
        assert_eq!(local_expiry.refunded_copper, sharded_expiry.refunded_copper);
        assert_eq!(local_expiry.schedule_count, sharded_expiry.schedule_count);
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
