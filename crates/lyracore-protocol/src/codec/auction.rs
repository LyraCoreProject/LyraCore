use anyhow::{anyhow, Result};
use std::time::Duration;

use lyracore_shared::item_property::client_enchantment_id;

use wow_world_messages::{
    vanilla::{
        AuctionHouse, AuctionListItem, SMSG_AUCTION_BIDDER_LIST_RESULT,
        SMSG_AUCTION_BIDDER_NOTIFICATION, SMSG_AUCTION_LIST_RESULT, SMSG_AUCTION_OWNER_LIST_RESULT,
        SMSG_AUCTION_OWNER_NOTIFICATION, SMSG_AUCTION_REMOVED_NOTIFICATION,
    },
    Guid,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuctionBrowseRequest {
    pub auctioneer_guid: u64,
    pub offset: u32,
    pub name: String,
    pub minimum_level: Option<u8>,
    pub maximum_level: Option<u8>,
    pub inventory_type: Option<u32>,
    pub item_class: Option<u32>,
    pub item_subclass: Option<u32>,
    pub quality: Option<u8>,
    pub usable_only: bool,
}

pub const CMSG_AUCTION_LIST_ITEMS_OPCODE: u32 = 0x0258;

// The typed quality enum cannot represent the client's 0xffff_ffff sentinel.
pub fn decode_auction_browse(body: &[u8]) -> Result<AuctionBrowseRequest> {
    if !(32..=287).contains(&body.len()) {
        return Err(anyhow!(
            "invalid CMSG_AUCTION_LIST_ITEMS body size {}",
            body.len()
        ));
    }
    let auctioneer_guid = u64::from_le_bytes(body[0..8].try_into().unwrap());
    let offset = u32::from_le_bytes(body[8..12].try_into().unwrap());
    let name_end = body[12..]
        .iter()
        .position(|byte| *byte == 0)
        .map(|position| position + 12)
        .ok_or_else(|| anyhow!("CMSG_AUCTION_LIST_ITEMS name is not terminated"))?;
    let name = std::str::from_utf8(&body[12..name_end])
        .map_err(|_| anyhow!("CMSG_AUCTION_LIST_ITEMS name is not UTF-8"))?
        .to_owned();
    let fields = &body[name_end + 1..];
    if fields.len() != 19 {
        return Err(anyhow!(
            "invalid CMSG_AUCTION_LIST_ITEMS trailing size {}",
            fields.len()
        ));
    }
    let minimum_level = fields[0];
    let maximum_level = fields[1];
    let inventory_type = u32::from_le_bytes(fields[2..6].try_into().unwrap());
    let item_class = u32::from_le_bytes(fields[6..10].try_into().unwrap());
    let item_subclass = u32::from_le_bytes(fields[10..14].try_into().unwrap());
    let quality = match u32::from_le_bytes(fields[14..18].try_into().unwrap()) {
        u32::MAX => None,
        value @ 0..=6 => Some(value as u8),
        value => return Err(anyhow!("invalid auction quality {value:#x}")),
    };
    let optional_filter = |value| (value != u32::MAX).then_some(value);
    Ok(AuctionBrowseRequest {
        auctioneer_guid,
        offset,
        name,
        minimum_level: (minimum_level != 0).then_some(minimum_level),
        maximum_level: (maximum_level != 0).then_some(maximum_level),
        inventory_type: optional_filter(inventory_type),
        item_class: optional_filter(item_class),
        item_subclass: optional_filter(item_subclass),
        quality,
        usable_only: fields[18] != 0,
    })
}

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
        item_enchantment: client_enchantment_id(view.item_enchant_id),
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

/// The client reads nonzero `won` as outbid and zero as won.
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

/// The binding's `item` field carries the auction id.
pub fn build_auction_removed_notification(
    auction_id: u32,
    item_entry: u32,
    item_random_property_id: u32,
) -> SMSG_AUCTION_REMOVED_NOTIFICATION {
    SMSG_AUCTION_REMOVED_NOTIFICATION {
        item: auction_id,
        item_template: item_entry,
        random_property_id: item_random_property_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_browse(quality: u32) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&99_u64.to_le_bytes());
        body.extend_from_slice(&50_u32.to_le_bytes());
        body.extend_from_slice(b"Sword\0");
        body.extend_from_slice(&[10, 20]);
        body.extend_from_slice(&13_u32.to_le_bytes());
        body.extend_from_slice(&2_u32.to_le_bytes());
        body.extend_from_slice(&7_u32.to_le_bytes());
        body.extend_from_slice(&quality.to_le_bytes());
        body.push(1);
        body
    }

    #[test]
    fn raw_browse_preserves_sentinels_and_rejects_unknown_quality_values() {
        assert_eq!(
            decode_auction_browse(&raw_browse(u32::MAX)).unwrap(),
            AuctionBrowseRequest {
                auctioneer_guid: 99,
                offset: 50,
                name: "Sword".to_owned(),
                minimum_level: Some(10),
                maximum_level: Some(20),
                inventory_type: Some(13),
                item_class: Some(2),
                item_subclass: Some(7),
                quality: None,
                usable_only: true,
            }
        );
        assert!(decode_auction_browse(&raw_browse(0x100)).is_err());

        let mut empty = raw_browse(u32::MAX);
        empty[18] = 0;
        empty[19] = 0;
        empty[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
        empty[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
        empty[28..32].copy_from_slice(&u32::MAX.to_le_bytes());
        let decoded = decode_auction_browse(&empty).unwrap();
        assert_eq!(decoded.minimum_level, None);
        assert_eq!(decoded.maximum_level, None);
        assert_eq!(decoded.inventory_type, None);
        assert_eq!(decoded.item_class, None);
        assert_eq!(decoded.item_subclass, None);
    }

    #[test]
    fn browse_requires_a_bounded_terminated_name_and_complete_fields() {
        let valid = raw_browse(2);
        for length in 0..valid.len() {
            assert!(decode_auction_browse(&valid[..length]).is_err());
        }
        let mut trailing = valid.clone();
        trailing.push(0);
        assert!(decode_auction_browse(&trailing).is_err());
        assert!(decode_auction_browse(&[1; 287]).is_err());
        let mut invalid_utf8 = valid;
        invalid_utf8[12] = 0xff;
        assert!(decode_auction_browse(&invalid_utf8).is_err());
        let mut longest = raw_browse(u32::MAX);
        longest.splice(12..17, vec![b'a'; 255]);
        assert_eq!(decode_auction_browse(&longest).unwrap().name.len(), 255);
        longest.insert(12, b'a');
        assert!(decode_auction_browse(&longest).is_err());
    }

    #[test]
    fn the_removed_notification_encodes_to_the_exact_cmangos_wire_layout() {
        use wow_world_messages::Message;
        let packet = build_auction_removed_notification(41, 25, 117);
        let mut bytes = Vec::new();
        packet.write_into_vec(&mut bytes).unwrap();
        assert_eq!(bytes, [41, 0, 0, 0, 25, 0, 0, 0, 117, 0, 0, 0]);
    }

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
    fn a_compatibility_enchant_goes_out_as_its_client_enchantment() {
        for (stored, client) in [(7745, 823), (7748, 724)] {
            let mut row = view();
            row.item_enchant_id = stored;
            assert_eq!(
                build_auction_list_item(&row, 1_000_000).item_enchantment,
                client
            );
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

    #[test]
    fn the_bidder_notification_encodes_to_the_exact_cmangos_wire_layout() {
        use wow_world_messages::Message;
        let packet =
            build_auction_bidder_notification(AuctionHouse::Stormwind, 41, 9, 201, 11, 25, 117);
        let mut bytes = Vec::new();
        packet.write_into_vec(&mut bytes).unwrap();

        let mut expected = Vec::new();
        expected.extend_from_slice(&1u32.to_le_bytes());
        expected.extend_from_slice(&41u32.to_le_bytes());
        expected.extend_from_slice(&9u64.to_le_bytes());
        expected.extend_from_slice(&201u32.to_le_bytes());
        expected.extend_from_slice(&11u32.to_le_bytes());
        expected.extend_from_slice(&25u32.to_le_bytes());
        expected.extend_from_slice(&117u32.to_le_bytes());
        assert_eq!(bytes.len(), 32);
        assert_eq!(bytes, expected);
    }

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
