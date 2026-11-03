use super::*;
use crate::mail_text::game_item_text;

pub(crate) fn reserve_listing(
    ctx: &ReducerContext,
    operation_id: u64,
    seller_guid: u64,
    auctioneer_guid: u64,
    item_guid: u64,
    terms: ListingTerms,
) -> Result<AuctionOperationReceipt, AuctionRefusal> {
    let house = auction_house_for_interaction(ctx, seller_guid, auctioneer_guid)
        .ok_or(AuctionRefusal::Database)?;
    let listing = prepare_from_source(
        &CtxSource { ctx },
        ListingRequest {
            operation_id,
            seller_guid,
            item_guid,
            house,
            terms,
        },
    )?;
    if listing.snapshot.item_text_id != 0
        && ctx
            .db
            .game_item_text()
            .id()
            .find(listing.snapshot.item_text_id)
            .is_none()
    {
        return Err(AuctionRefusal::Database);
    }
    consume_listing_value(ctx, &listing);
    Ok(receipt_from_listing(listing, 0))
}

pub(crate) fn accept_listing(ctx: &ReducerContext, receipt: AuctionOperationReceipt) -> u32 {
    let listing = listing_from_receipt(receipt).listing;
    let id = insert_active_auction(ctx, &listing);
    ctx.db
        .game_auction_operation_receipt()
        .insert(receipt_from_listing(listing, id));
    id
}

pub(crate) fn return_listing(ctx: &ReducerContext, receipt: AuctionOperationReceipt) {
    let listing = listing_from_receipt(receipt).listing;
    insert_auction_mail(
        ctx,
        AuctionMail {
            recipient_guid: listing.request.seller_guid,
            house: listing.request.house.id,
            action: AuctionMailAction::Cancelled,
            item_entry: listing.snapshot.entry,
            random_property_id: listing.snapshot.random_property_id,
            money: 0,
            attached_item: listing.snapshot,
            counterparty_guid: 0,
            bid: 0,
            buyout: 0,
            deposit: 0,
            cut: 0,
        },
    );
}

pub(crate) fn bid(
    ctx: &ReducerContext,
    operation: u64,
    guid: u64,
    house: u32,
    id: u32,
    offer: u32,
) -> Result<u32, native::BidRefusal> {
    let mut market = CtxBidMarket { ctx };
    let request = bid_request(BidOperation::Bid, operation, guid, id, house, offer);
    let auction = market.auction(id);
    let decision = decide_bid(auction, request, market.now_micros());
    let price = native::accepted_price(decision)?;
    market.apply_decision(request, auction, decision)?;
    Ok(price)
}

pub(crate) fn cancellation_cut(
    ctx: &ReducerContext,
    guid: u64,
    house: u32,
    id: u32,
) -> Result<u32, AuctionRefusal> {
    let market = CtxBidMarket { ctx };
    let row = market
        .auction(id)
        .filter(|row| {
            row.owner_guid == guid
                && row.expires_micros > market.now_micros()
                && lyracore_shared::auction::market_of(row.house)
                    == lyracore_shared::auction::market_of(house)
        })
        .ok_or(AuctionRefusal::ItemNotFound)?;
    auction_cut(row.highest_bid, row.consignment_rate).ok_or(AuctionRefusal::Database)
}

pub(crate) fn cancel(
    ctx: &ReducerContext,
    operation: u64,
    guid: u64,
    house: u32,
    id: u32,
    paid: u32,
) -> Result<(), AuctionRefusal> {
    let mut market = CtxBidMarket { ctx };
    let row = market.auction(id);
    let request = bid_request(BidOperation::Cancel, operation, guid, id, house, paid);
    let decision = decide_cancel(row, request, market.now_micros());
    if !matches!(decision, BidDecision::Cancelled { .. }) {
        return Err(AuctionRefusal::Database);
    }
    market.apply_decision(request, row, decision)
}

pub(crate) fn import(
    ctx: &ReducerContext,
    mut row: Auction,
    receipt: AuctionOperationReceipt,
) -> Result<(), String> {
    if row.listing_operation_id != receipt.operation_id
        || row.id != receipt.auction_id
        || row.owner_guid != receipt.actor_guid
        || ctx
            .db
            .game_auction_operation_receipt()
            .operation_id()
            .find(receipt.operation_id)
            .is_some()
    {
        return Err("Listing import conflicts with a retained receipt".into());
    }
    row.id = 0;
    let row = ctx.db.game_auction().insert(row);
    ctx.db
        .game_auction_operation_receipt()
        .insert(AuctionOperationReceipt {
            auction_id: row.id,
            ..receipt
        });
    ctx.db.game_auction_expiry().insert(AuctionExpiry {
        scheduled_id: 0,
        scheduled_at: crate::time::schedule_time(ctx, row.expires_at),
        auction_id: row.id,
    });
    Ok(())
}
