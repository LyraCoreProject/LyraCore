//! Shared auction-house identities used by durable state and the vanilla wire adapter.

/// Vanilla's Stormwind auction-house identifier (`AuctionHouse::Stormwind`).
pub const STORMWIND_HOUSE_ID: u32 = 1;

/// Maximum squared 3-D distance for a player to use a named auctioneer: 10 yards.
pub const INTERACTION_RANGE_SQ: f32 = 100.0;
