//! Shared auction-house protocol rules.

/// Maximum squared 3-D distance for a player to use a named auctioneer: 10 yards.
pub const INTERACTION_RANGE_SQ: f32 = 100.0;

/// Vanilla's minimum raise: five percent of the current bid, rounded up.
pub fn bid_increment(current_bid: u32) -> u32 {
    if current_bid == 0 {
        0
    } else {
        u32::try_from(u64::from(current_bid).div_ceil(20))
            .unwrap_or(u32::MAX)
            .max(1)
    }
}

/// Full next offer required by the auction protocol row and the bid validator.
pub fn minimum_next_bid(start_bid: u32, current_bid: u32) -> Option<u32> {
    if current_bid == 0 {
        Some(start_bid)
    } else {
        current_bid.checked_add(bid_increment(current_bid))
    }
}

/// Why the Module refused an auction Durable Request. The tag is the whole reducer error text,
/// so neither tier matches on human prose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuctionRefusal {
    ItemNotFound,
    NotEnoughMoney,
    InvalidTerms,
    Database,
}

impl AuctionRefusal {
    pub const ALL: [Self; 4] = [
        Self::ItemNotFound,
        Self::NotEnoughMoney,
        Self::InvalidTerms,
        Self::Database,
    ];

    pub fn as_tag(self) -> &'static str {
        match self {
            Self::ItemNotFound => "auction:item_not_found",
            Self::NotEnoughMoney => "auction:not_enough_money",
            Self::InvalidTerms => "auction:invalid_terms",
            Self::Database => "auction:database",
        }
    }

    pub fn parse_tag(tag: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|refusal| refusal.as_tag() == tag)
    }
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

#[cfg(test)]
mod tests {
    use super::AuctionRefusal;

    #[test]
    fn every_refusal_tag_round_trips() {
        for refusal in AuctionRefusal::ALL {
            assert_eq!(AuctionRefusal::parse_tag(refusal.as_tag()), Some(refusal));
        }
        assert_eq!(AuctionRefusal::parse_tag("auction:"), None);
        assert_eq!(
            AuctionRefusal::parse_tag("gw_auction_hold_bid reducer timed out after 10s"),
            None
        );
    }

    #[test]
    fn bid_increment_is_five_percent_rounded_up() {
        assert_eq!(super::bid_increment(0), 0);
        assert_eq!(super::bid_increment(1), 1);
        assert_eq!(super::bid_increment(20), 1);
        assert_eq!(super::bid_increment(21), 2);
        assert_eq!(super::bid_increment(u32::MAX), 214_748_365);
    }

    #[test]
    fn minimum_next_bid_is_the_full_required_offer() {
        assert_eq!(super::minimum_next_bid(100, 0), Some(100));
        assert_eq!(super::minimum_next_bid(100, 201), Some(212));
        assert_eq!(super::minimum_next_bid(100, u32::MAX), None);
    }
}
