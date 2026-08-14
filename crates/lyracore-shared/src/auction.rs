//! Shared auction-house identities used by durable state and the vanilla wire adapter.

/// Vanilla's Stormwind auction-house identifier (`AuctionHouse::Stormwind`).
pub const STORMWIND_HOUSE_ID: u32 = 1;

/// Maximum squared 3-D distance for a player to use a named auctioneer: 10 yards.
pub const INTERACTION_RANGE_SQ: f32 = 100.0;

/// Stable reducer-boundary result tags for listing gameplay refusals.
pub mod result {
    pub const DATABASE: &str = "AUCTION_DATABASE";
    pub const ITEM_NOT_FOUND: &str = "AUCTION_ITEM_NOT_FOUND";
    pub const NOT_ENOUGH_MONEY: &str = "AUCTION_NOT_ENOUGH_MONEY";
}

/// Stable terminal outcome codes shared by bid Hold and decision rows.
pub mod bid_outcome {
    pub const PENDING: u8 = 0;
    pub const ACCEPTED: u8 = 1;
    pub const ITEM_NOT_FOUND: u8 = 2;
    pub const HIGHER_BID: u8 = 3;
    pub const BID_INCREMENT: u8 = 4;
    pub const BID_OWN: u8 = 5;
    pub const DATABASE: u8 = 6;
}
