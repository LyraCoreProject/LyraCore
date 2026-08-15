use std::time::Duration;

use wow_world_messages::{
    vanilla::{
        AuctionListItem, SMSG_AUCTION_BIDDER_LIST_RESULT, SMSG_AUCTION_LIST_RESULT,
        SMSG_AUCTION_OWNER_LIST_RESULT,
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
}

pub fn build_auction_list_item(view: &AuctionView, now_micros: i64) -> AuctionListItem {
    let minimum_bid = lyracore_shared::auction::minimum_next_bid(view.start_bid, view.highest_bid)
        .unwrap_or(u32::MAX);
    let remaining_millis = view.expires_at_micros.saturating_sub(now_micros).max(0) / 1_000;
    AuctionListItem {
        id: view.id,
        item: view.item_entry,
        item_enchantment: view.item_enchant_id,
        item_random_property_id: 0,
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
        }
    }

    #[test]
    fn auction_row_maps_every_wire_field_and_clamps_derived_values() {
        let mapped = build_auction_list_item(&view(), 1_000_000);
        assert_eq!(mapped.id, 9);
        assert_eq!(mapped.item, 25);
        assert_eq!(mapped.item_enchantment, 7);
        assert_eq!(mapped.item_random_property_id, 0);
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
}
