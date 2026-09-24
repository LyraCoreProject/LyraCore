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

/// The Auction Cut: the house's share of a bid, truncated (`cm:AuctionHouseMgr.cpp:733-736`). The
/// seller pays it out of the proceeds at Settlement and out of the purse at Cancellation. `None` when
/// the rate is not a percentage.
pub fn auction_cut(bid: u32, consignment_rate: u32) -> Option<u32> {
    if consignment_rate > 100 {
        return None;
    }
    u32::try_from(u64::from(bid) * u64::from(consignment_rate) / 100).ok()
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

/// `FactionTemplate.dbc` group bit for a template's own team (`cm:Server/DBCEnums.h:92-95`).
const FACTION_MASK_ALLIANCE: u32 = 2;
const FACTION_MASK_HORDE: u32 = 4;

/// Vanilla's fixed auctioneer-template-to-house table (`cm:AuctionHouseMgr.cpp:461-518`). A
/// template outside the table falls back to its faction group's own house, then to the neutral
/// house (Booty Bay/Gadgetzan/Everlook, house 7).
pub fn house_for_faction_template(template_id: u32, faction_group_mask: u32) -> u32 {
    match template_id {
        12 => 1,
        55 | 534 => 2,
        80 => 3,
        68 => 4,
        104 => 5,
        29 => 6,
        120 | 474 | 855 => 7,
        _ if faction_group_mask & FACTION_MASK_ALLIANCE != 0 => 1,
        _ if faction_group_mask & FACTION_MASK_HORDE != 0 => 6,
        _ => 7,
    }
}

/// The listing pool a house belongs to: Alliance houses 1-3 and Horde houses 4-6 each pool their
/// listings, and house 7 stands alone as the neutral market
/// (`cm:AuctionHouseMgr.cpp:51-63,444-459`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuctionMarket {
    Alliance,
    Horde,
    Neutral,
}

/// The market a house's listings pool into. An id outside 1-7 is not an imported house; it pools
/// as neutral rather than joining a team's listings.
pub fn market_of(house_id: u32) -> AuctionMarket {
    match house_id {
        1..=3 => AuctionMarket::Alliance,
        4..=6 => AuctionMarket::Horde,
        _ => AuctionMarket::Neutral,
    }
}

/// Stable terminal outcome codes shared by bid Hold and decision rows. `CANCELLED` is the one
/// accepting outcome of a Cancellation Hold. `ITEM_NOT_FOUND` and `DATABASE` refuse either operation.
pub mod bid_outcome {
    pub const PENDING: u8 = 0;
    pub const ACCEPTED: u8 = 1;
    pub const ITEM_NOT_FOUND: u8 = 2;
    pub const HIGHER_BID: u8 = 3;
    pub const BID_INCREMENT: u8 = 4;
    pub const BID_OWN: u8 = 5;
    pub const DATABASE: u8 = 6;
    pub const CANCELLED: u8 = 7;
}

/// Stable `operation` codes of the bid Hold and decision rows. Every row written before
/// Cancellation existed reads as `BID`.
pub mod hold_operation {
    pub const BID: u8 = 0;
    pub const CANCEL: u8 = 1;
}

/// Stable `game_auction_notice.kind` codes, in the vanilla notice table's own order
/// (`cm:AuctionHouseHandler.cpp`/`AuctionHouseMgr.cpp`). Distinct from an Auction Mail's
/// `MailAuctionAnswers` subject code: a live notice has two kinds (New bid, Removed) with no mail
/// twin, and a mail action (Successful) with no live notice.
pub mod auction_notice {
    pub const OUTBID: u8 = 0;
    pub const WON: u8 = 1;
    pub const SOLD: u8 = 2;
    pub const EXPIRED: u8 = 3;
    pub const NEW_BID: u8 = 4;
    /// To the displaced bidder when the seller cancels (`cm:AuctionHouseHandler.cpp:167-189`).
    pub const REMOVED: u8 = 5;
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
    fn auction_cut_is_the_rate_of_the_bid_truncated() {
        assert_eq!(super::auction_cut(100, 5), Some(5));
        assert_eq!(super::auction_cut(19, 5), Some(0), "0.95 truncates to 0");
        assert_eq!(
            super::auction_cut(201, 5),
            Some(10),
            "10.05 truncates to 10"
        );
        assert_eq!(
            super::auction_cut(0, 15),
            Some(0),
            "an unbid listing costs nothing"
        );
        assert_eq!(super::auction_cut(u32::MAX, 100), Some(u32::MAX));
        assert_eq!(super::auction_cut(100, 101), None, "not a percentage");
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

    // House ids and template rows: `cm:AuctionHouseMgr.cpp:461-518`. The DBC names each house
    // Stormwind (1), Alliance (2), Darnassus (3), Undercity (4), Thunder Bluff (5), Horde (6) and
    // Blackwater (7).
    #[test]
    fn house_for_faction_template_resolves_the_stormwind_row() {
        assert_eq!(super::house_for_faction_template(12, 0), 1);
    }

    #[test]
    fn house_for_faction_template_resolves_the_alliance_row_and_its_alternate() {
        assert_eq!(super::house_for_faction_template(55, 0), 2);
        assert_eq!(super::house_for_faction_template(534, 0), 2);
    }

    #[test]
    fn house_for_faction_template_resolves_the_darnassus_row() {
        assert_eq!(super::house_for_faction_template(80, 0), 3);
    }

    #[test]
    fn house_for_faction_template_resolves_the_undercity_row() {
        assert_eq!(super::house_for_faction_template(68, 0), 4);
    }

    #[test]
    fn house_for_faction_template_resolves_the_thunder_bluff_row() {
        assert_eq!(super::house_for_faction_template(104, 0), 5);
    }

    #[test]
    fn house_for_faction_template_resolves_the_horde_row() {
        assert_eq!(super::house_for_faction_template(29, 0), 6);
    }

    #[test]
    fn house_for_faction_template_resolves_the_blackwater_row_and_its_alternates() {
        assert_eq!(super::house_for_faction_template(120, 0), 7);
        assert_eq!(super::house_for_faction_template(474, 0), 7);
        assert_eq!(super::house_for_faction_template(855, 0), 7);
    }

    #[test]
    fn house_for_faction_template_falls_back_to_stormwind_for_an_unlisted_alliance_template() {
        assert_eq!(super::house_for_faction_template(999, 2), 1);
    }

    #[test]
    fn house_for_faction_template_falls_back_to_the_horde_house_for_an_unlisted_horde_template() {
        assert_eq!(super::house_for_faction_template(999, 4), 6);
    }

    #[test]
    fn house_for_faction_template_falls_back_to_blackwater_for_an_unlisted_or_unknown_template() {
        assert_eq!(super::house_for_faction_template(999, 0), 7);
        // A Monster-only group belongs to neither team.
        assert_eq!(super::house_for_faction_template(999, 8), 7);
    }

    // Market pooling: `cm:AuctionHouseMgr.cpp:51-63,444-459`.
    #[test]
    fn market_of_pools_stormwind_alliance_and_darnassus_together() {
        for house in [1, 2, 3] {
            assert_eq!(super::market_of(house), super::AuctionMarket::Alliance);
        }
    }

    #[test]
    fn market_of_pools_undercity_thunder_bluff_and_horde_together() {
        for house in [4, 5, 6] {
            assert_eq!(super::market_of(house), super::AuctionMarket::Horde);
        }
    }

    #[test]
    fn market_of_keeps_blackwater_and_any_unknown_house_neutral() {
        assert_eq!(super::market_of(7), super::AuctionMarket::Neutral);
        assert_eq!(super::market_of(0), super::AuctionMarket::Neutral);
        assert_eq!(super::market_of(8), super::AuctionMarket::Neutral);
    }

    #[test]
    fn every_auction_notice_kind_has_a_distinct_code() {
        use super::auction_notice::{EXPIRED, NEW_BID, OUTBID, REMOVED, SOLD, WON};
        let codes = [OUTBID, WON, SOLD, EXPIRED, NEW_BID, REMOVED];
        for (i, a) in codes.iter().enumerate() {
            for b in &codes[i + 1..] {
                assert_ne!(a, b, "auction_notice kind codes must be pairwise distinct");
            }
        }
        assert_eq!(
            [OUTBID, WON, SOLD, EXPIRED, NEW_BID, REMOVED],
            [0, 1, 2, 3, 4, 5]
        );
    }
}
