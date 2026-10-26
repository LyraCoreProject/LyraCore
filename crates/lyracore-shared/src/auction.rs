// Squared interaction distance in yards.
pub const INTERACTION_RANGE_SQ: f32 = 100.0;

pub fn bid_increment(current_bid: u32) -> u32 {
    if current_bid == 0 {
        0
    } else {
        u32::try_from(u64::from(current_bid).div_ceil(20))
            .unwrap_or(u32::MAX)
            .max(1)
    }
}

pub fn minimum_next_bid(start_bid: u32, current_bid: u32) -> Option<u32> {
    if current_bid == 0 {
        Some(start_bid)
    } else {
        current_bid.checked_add(bid_increment(current_bid))
    }
}

pub fn auction_cut(bid: u32, consignment_rate: u32) -> Option<u32> {
    if consignment_rate > 100 {
        return None;
    }
    u32::try_from(u64::from(bid) * u64::from(consignment_rate) / 100).ok()
}

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

// FactionTemplate.dbc group bits, cm:Server/DBCEnums.h:92-95.
const FACTION_MASK_ALLIANCE: u32 = 2;
const FACTION_MASK_HORDE: u32 = 4;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuctionMarket {
    Alliance,
    Horde,
    Neutral,
}

pub fn market_of(house_id: u32) -> AuctionMarket {
    match house_id {
        1..=3 => AuctionMarket::Alliance,
        4..=6 => AuctionMarket::Horde,
        _ => AuctionMarket::Neutral,
    }
}

// Notice kinds use a different numbering from Auction Mail subject codes.
pub mod auction_notice {
    pub const OUTBID: u8 = 0;
    pub const WON: u8 = 1;
    pub const SOLD: u8 = 2;
    pub const EXPIRED: u8 = 3;
    pub const NEW_BID: u8 = 4;
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
        assert_eq!(super::house_for_faction_template(999, 8), 7);
    }

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
