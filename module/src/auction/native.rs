use super::*;

#[derive(Debug)]
pub(crate) enum BidRefusal {
    Auction(AuctionRefusal),
    HigherBid {
        bidder_guid: u64,
        current_bid: u32,
        minimum_increment: u32,
    },
    Increment,
    Own,
}

impl From<AuctionRefusal> for BidRefusal {
    fn from(refusal: AuctionRefusal) -> Self {
        Self::Auction(refusal)
    }
}

pub(crate) fn list(
    ctx: &ReducerContext,
    seller_guid: u64,
    auctioneer_guid: u64,
    item_guid: u64,
    terms: ListingTerms,
) -> Result<u32, AuctionRefusal> {
    let house = auction_house_for_interaction(ctx, seller_guid, auctioneer_guid)
        .ok_or(AuctionRefusal::Database)?;
    if ctx.db.game_item_instance().guid().find(item_guid).is_none() {
        if let Some(receipt) = ctx
            .db
            .game_auction_operation_receipt()
            .by_actor()
            .filter(seller_guid)
            .find(|receipt| {
                receipt.item_guid == item_guid
                    && receipt.house == house.id
                    && receipt.start_bid == terms.start_bid
                    && receipt.buyout == terms.buyout
                    && receipt.duration_minutes == terms.duration_minutes
                    && ctx
                        .db
                        .game_auction()
                        .id()
                        .find(receipt.auction_id)
                        .is_some_and(|auction| auction.listing_operation_id == receipt.operation_id)
            })
        {
            return Ok(receipt.auction_id);
        }
    }
    let operation_id = *crate::auth::reserve_guids(ctx, 1, u64::MAX)
        .map_err(|_| AuctionRefusal::Database)?
        .start();
    create_local_listing(
        &mut CtxSource { ctx },
        ListingRequest {
            operation_id,
            seller_guid,
            item_guid,
            house,
            terms,
        },
    )
}

// Returns the accepted price, which can be below the offer at buyout.
pub(crate) fn bid(
    ctx: &ReducerContext,
    bidder_guid: u64,
    auctioneer_guid: u64,
    auction_id: u32,
    offer: u32,
) -> Result<u32, BidRefusal> {
    let house = auction_house_for_interaction(ctx, bidder_guid, auctioneer_guid)
        .ok_or(AuctionRefusal::Database)?;
    let mut bidder = crate::helpers::acting_entity_by_guid(ctx, bidder_guid)
        .ok_or(AuctionRefusal::NotEnoughMoney)?;
    if offer == 0 || auction_id == 0 {
        return Err(AuctionRefusal::Database.into());
    }
    if bidder.money < offer {
        return Err(AuctionRefusal::NotEnoughMoney.into());
    }
    let operation_id = *crate::auth::reserve_guids(ctx, 1, u64::MAX)
        .map_err(|_| AuctionRefusal::Database)?
        .start();
    let request = bid_request(
        BidOperation::Bid,
        operation_id,
        bidder_guid,
        auction_id,
        house.id,
        offer,
    );
    let mut market = CtxBidMarket { ctx };
    let auction = market.auction(auction_id);
    let decision = decide_bid(auction, request, market.now_micros());
    let price = match decision {
        BidDecision::Accepted(accepted) => accepted.price,
        BidDecision::ItemNotFound => return Err(AuctionRefusal::ItemNotFound.into()),
        BidDecision::HigherBid {
            bidder_guid,
            current_bid,
            minimum_increment,
        } => {
            return Err(BidRefusal::HigherBid {
                bidder_guid,
                current_bid,
                minimum_increment,
            })
        }
        BidDecision::BidIncrement => return Err(BidRefusal::Increment),
        BidDecision::BidOwn => return Err(BidRefusal::Own),
        BidDecision::Database | BidDecision::Cancelled { .. } => {
            return Err(AuctionRefusal::Database.into())
        }
    };
    bidder.money = bidder
        .money
        .checked_sub(price)
        .ok_or(AuctionRefusal::Database)?;
    bidder.update(ctx);
    market.apply_decision(request, auction, decision)?;
    Ok(price)
}

pub(crate) fn cancel(
    ctx: &ReducerContext,
    seller_guid: u64,
    auctioneer_guid: u64,
    auction_id: u32,
) -> Result<(), AuctionRefusal> {
    let house = auction_house_for_interaction(ctx, seller_guid, auctioneer_guid)
        .ok_or(AuctionRefusal::Database)?;
    let mut market = CtxBidMarket { ctx };
    let auction = market
        .auction(auction_id)
        .filter(|auction| {
            auction.owner_guid == seller_guid
                && auction.expires_micros > market.now_micros()
                && lyracore_shared::auction::market_of(auction.house)
                    == lyracore_shared::auction::market_of(house.id)
        })
        .ok_or(AuctionRefusal::ItemNotFound)?;
    let cut = auction_cut(auction.highest_bid, auction.consignment_rate)
        .ok_or(AuctionRefusal::Database)?;
    let mut seller = crate::helpers::acting_entity_by_guid(ctx, seller_guid)
        .ok_or(AuctionRefusal::NotEnoughMoney)?;
    if seller.money < cut {
        return Err(AuctionRefusal::NotEnoughMoney);
    }
    let operation_id = *crate::auth::reserve_guids(ctx, 1, u64::MAX)
        .map_err(|_| AuctionRefusal::Database)?
        .start();
    let request = bid_request(
        BidOperation::Cancel,
        operation_id,
        seller_guid,
        auction_id,
        house.id,
        cut,
    );
    let decision = decide_cancel(Some(auction), request, market.now_micros());
    if !matches!(decision, BidDecision::Cancelled { .. }) {
        return Err(AuctionRefusal::Database);
    }
    seller.money -= cut;
    seller.update(ctx);
    market.apply_decision(request, Some(auction), decision)
}
