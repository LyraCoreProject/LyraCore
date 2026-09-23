use std::time::Duration;

use wow_world_messages::{
    vanilla::{
        AuctionHouse, AuctionListItem, SMSG_AUCTION_BIDDER_LIST_RESULT,
        SMSG_AUCTION_BIDDER_NOTIFICATION, SMSG_AUCTION_LIST_RESULT, SMSG_AUCTION_OWNER_LIST_RESULT,
        SMSG_AUCTION_OWNER_NOTIFICATION,
    },
    Guid,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuctionView {
    pub id: u32,
    pub item_entry: u32,
    pub item_stack_count: u32,
    pub item_enchant_id: u32,
    pub owner_guid: u64,
    pub start_bid: u32,
    pub buyout: u32,
    pub highest_bidder_guid: u64,
    pub highest_bid: u32,
    pub expires_at_micros: i64,
    pub random_property_id: u32,
}

pub fn build_auction_list_item(view: &AuctionView, now_micros: i64) -> AuctionListItem {
    let minimum_bid = lyracore_shared::auction::minimum_next_bid(view.start_bid, view.highest_bid)
        .unwrap_or(u32::MAX);
    let remaining_millis = view.expires_at_micros.saturating_sub(now_micros).max(0) / 1_000;
    AuctionListItem {
        id: view.id,
        item: view.item_entry,
        item_enchantment: view.item_enchant_id,
        item_random_property_id: view.random_property_id,
        item_suffix_factor: 0,
        item_count: view.item_stack_count,
        item_charges: 0,
        item_owner: Guid::new(view.owner_guid),
        start_bid: view.start_bid,
        minimum_bid,
        buyout_amount: view.buyout,
        time_left: Duration::from_millis(
            u64::try_from(remaining_millis)
                .unwrap_or(0)
                .min(u64::from(u32::MAX)),
        ),
        highest_bidder: Guid::new(view.highest_bidder_guid),
        highest_bid: view.highest_bid,
    }
}

fn build_items(rows: &[AuctionView], now_micros: i64) -> Vec<AuctionListItem> {
    rows.iter()
        .map(|row| build_auction_list_item(row, now_micros))
        .collect()
}

pub fn build_auction_list_result(
    rows: &[AuctionView],
    total: u32,
    now_micros: i64,
) -> SMSG_AUCTION_LIST_RESULT {
    SMSG_AUCTION_LIST_RESULT {
        auctions: build_items(rows, now_micros),
        total_amount_of_auctions: total,
    }
}

pub fn build_auction_owner_list_result(
    rows: &[AuctionView],
    total: u32,
    now_micros: i64,
) -> SMSG_AUCTION_OWNER_LIST_RESULT {
    SMSG_AUCTION_OWNER_LIST_RESULT {
        auctions: build_items(rows, now_micros),
        total_amount_of_auctions: total,
    }
}

pub fn build_auction_bidder_list_result(
    rows: &[AuctionView],
    total: u32,
    now_micros: i64,
) -> SMSG_AUCTION_BIDDER_LIST_RESULT {
    SMSG_AUCTION_BIDDER_LIST_RESULT {
        auctions: build_items(rows, now_micros),
        total_amount_of_auctions: total,
    }
}

/// Outbid or Won, to the bidder (`cm:AuctionHouseHandler.cpp:93-107,157-158`,
/// `cm:AuctionHouseMgr.cpp:146-147`). `won` is the displaced bid on Outbid, 0 on Won: the client
/// reads a nonzero value as "you were outbid" and zero as "you won".
pub fn build_auction_bidder_notification(
    auction_house: AuctionHouse,
    auction_id: u32,
    bidder_guid: u64,
    won: u32,
    out_bid: u32,
    item_entry: u32,
    item_random_property_id: u32,
) -> SMSG_AUCTION_BIDDER_NOTIFICATION {
    SMSG_AUCTION_BIDDER_NOTIFICATION {
        auction_house,
        auction_id,
        bidder: Guid::new(bidder_guid),
        won,
        out_bid,
        item_template: item_entry,
        item_random_property_id,
    }
}

/// Sold, Expired or New bid, to the owner (`cm:AuctionHouseMgr.cpp:195-199,231-232,802-806`,
/// `cm:AuctionHouseHandler.cpp:110-128`). `bidder` is 0 on Sold and Expired, and the new bidder on
/// New bid, which the client answers only by refreshing its list.
pub fn build_auction_owner_notification(
    auction_id: u32,
    bid: u32,
    out_bid: u32,
    bidder_guid: u64,
    item_entry: u32,
    item_random_property_id: u32,
) -> SMSG_AUCTION_OWNER_NOTIFICATION {
    SMSG_AUCTION_OWNER_NOTIFICATION {
        auction_id,
        bid,
        auction_out_bid: out_bid,
        bidder: Guid::new(bidder_guid),
        item: item_entry,
        item_random_property_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> AuctionView {
        AuctionView {
            id: 9,
            item_entry: 25,
            item_stack_count: 2,
            item_enchant_id: 7,
            owner_guid: 10,
            start_bid: 100,
            buyout: 500,
            highest_bidder_guid: 20,
            highest_bid: 201,
            expires_at_micros: 3_500_000,
            random_property_id: 509_0101,
        }
    }

    #[test]
    fn auction_row_maps_every_wire_field_and_clamps_derived_values() {
        let mapped = build_auction_list_item(&view(), 1_000_000);
        assert_eq!(mapped.id, 9);
        assert_eq!(mapped.item, 25);
        assert_eq!(mapped.item_enchantment, 7);
        assert_eq!(mapped.item_random_property_id, 509_0101);
        assert_eq!(mapped.item_suffix_factor, 0);
        assert_eq!(mapped.item_count, 2);
        assert_eq!(mapped.item_charges, 0);
        assert_eq!(mapped.item_owner.guid(), 10);
        assert_eq!(mapped.start_bid, 100);
        assert_eq!(mapped.minimum_bid, 212);
        assert_eq!(mapped.buyout_amount, 500);
        assert_eq!(mapped.time_left, Duration::from_millis(2_500));
        assert_eq!(mapped.highest_bidder.guid(), 20);
        assert_eq!(mapped.highest_bid, 201);

        let mut unbid = view();
        unbid.highest_bid = 0;
        assert_eq!(build_auction_list_item(&unbid, 1_000_000).minimum_bid, 100);

        assert_eq!(
            build_auction_list_item(&view(), 4_000_000).time_left,
            Duration::ZERO
        );
        assert_eq!(
            build_auction_list_item(&view(), i64::MIN)
                .time_left
                .as_millis(),
            u128::from(u32::MAX)
        );
    }

    #[test]
    fn browse_owner_and_bidder_packets_use_the_same_row_mapping_and_total() {
        let rows = [view()];
        let browse = build_auction_list_result(&rows, 51, 1_000_000);
        let owner = build_auction_owner_list_result(&rows, 51, 1_000_000);
        let bidder = build_auction_bidder_list_result(&rows, 51, 1_000_000);
        assert_eq!(browse.auctions, owner.auctions);
        assert_eq!(browse.auctions, bidder.auctions);
        assert_eq!(browse.total_amount_of_auctions, 51);
        assert_eq!(owner.total_amount_of_auctions, 51);
        assert_eq!(bidder.total_amount_of_auctions, 51);
    }

    #[test]
    fn the_bidder_notification_carries_every_field_at_its_own_wire_position() {
        let packet =
            build_auction_bidder_notification(AuctionHouse::Stormwind, 41, 9, 201, 11, 25, 117);
        assert_eq!(packet.auction_house, AuctionHouse::Stormwind);
        assert_eq!(packet.auction_id, 41);
        assert_eq!(packet.bidder.guid(), 9);
        assert_eq!(packet.won, 201, "nonzero won means outbid");
        assert_eq!(packet.out_bid, 11);
        assert_eq!(packet.item_template, 25);
        assert_eq!(packet.item_random_property_id, 117);
    }

    #[test]
    fn a_zero_won_field_reads_as_a_win_not_an_outbid() {
        let packet =
            build_auction_bidder_notification(AuctionHouse::Stormwind, 41, 8, 0, 10, 25, 117);
        assert_eq!(packet.won, 0);
    }

    #[test]
    fn the_owner_notification_carries_every_field_at_its_own_wire_position() {
        let packet = build_auction_owner_notification(41, 201, 11, 0, 25, 117);
        assert_eq!(packet.auction_id, 41);
        assert_eq!(packet.bid, 201);
        assert_eq!(packet.auction_out_bid, 11);
        assert_eq!(packet.bidder.guid(), 0, "Sold and Expired carry no bidder");
        assert_eq!(packet.item, 25);
        assert_eq!(packet.item_random_property_id, 117);
    }

    #[test]
    fn a_new_bid_owner_notification_names_the_new_bidder() {
        let packet = build_auction_owner_notification(41, 107, 6, 9, 25, 117);
        assert_eq!(packet.bidder.guid(), 9);
    }

    /// `cm:AuctionHouseHandler.cpp:93-107`: house, auction id, bidder guid (full 8 bytes, not
    /// packed), won, out_bid, item entry, random property id — 32 bytes, written out by hand.
    #[test]
    fn the_bidder_notification_encodes_to_the_exact_cmangos_wire_layout() {
        use wow_world_messages::Message;
        let packet =
            build_auction_bidder_notification(AuctionHouse::Stormwind, 41, 9, 201, 11, 25, 117);
        let mut bytes = Vec::new();
        packet.write_into_vec(&mut bytes).unwrap();

        let mut expected = Vec::new();
        expected.extend_from_slice(&1u32.to_le_bytes()); // AuctionHouse::Stormwind == 0x1
        expected.extend_from_slice(&41u32.to_le_bytes());
        expected.extend_from_slice(&9u64.to_le_bytes());
        expected.extend_from_slice(&201u32.to_le_bytes());
        expected.extend_from_slice(&11u32.to_le_bytes());
        expected.extend_from_slice(&25u32.to_le_bytes());
        expected.extend_from_slice(&117u32.to_le_bytes());
        assert_eq!(bytes.len(), 32);
        assert_eq!(bytes, expected);
    }

    /// `cm:AuctionHouseHandler.cpp:110-128`: auction id, bid, auction_out_bid, bidder guid (full 8
    /// bytes), item entry, random property id — 28 bytes, written out by hand.
    #[test]
    fn the_owner_notification_encodes_to_the_exact_cmangos_wire_layout() {
        use wow_world_messages::Message;
        let packet = build_auction_owner_notification(41, 500, 25, 9, 25, 117);
        let mut bytes = Vec::new();
        packet.write_into_vec(&mut bytes).unwrap();

        let mut expected = Vec::new();
        expected.extend_from_slice(&41u32.to_le_bytes());
        expected.extend_from_slice(&500u32.to_le_bytes());
        expected.extend_from_slice(&25u32.to_le_bytes());
        expected.extend_from_slice(&9u64.to_le_bytes());
        expected.extend_from_slice(&25u32.to_le_bytes());
        expected.extend_from_slice(&117u32.to_le_bytes());
        assert_eq!(bytes.len(), 28);
        assert_eq!(bytes, expected);
    }
}
