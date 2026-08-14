//! Read-only Stormwind market queries and their pure filter/pagination seam.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};
use spacetimedb_sdk::Table;

use super::super::bindings::*;
use super::super::connection::Coordinator;
use crate::world::{AuctionBrowseRequest, AuctionPage, AuctionQuery};

#[derive(Clone, Copy)]
struct BrowseFacts<'a> {
    name: &'a str,
    required_level: u8,
    inventory_type: u8,
    item_class: u8,
    item_subclass: u8,
    quality: u8,
}

fn browse_matches(
    request: &AuctionBrowseRequest,
    player_level: u8,
    player_class: u8,
    item: BrowseFacts<'_>,
) -> bool {
    (request.name.is_empty()
        || item
            .name
            .to_lowercase()
            .contains(&request.name.to_lowercase()))
        && request
            .minimum_level
            .is_none_or(|minimum| item.required_level >= minimum)
        && request
            .maximum_level
            .is_none_or(|maximum| item.required_level <= maximum)
        && request
            .inventory_type
            .is_none_or(|wanted| u32::from(item.inventory_type) == wanted)
        && request
            .item_class
            .is_none_or(|wanted| u32::from(item.item_class) == wanted)
        && request
            .item_subclass
            .is_none_or(|wanted| u32::from(item.item_subclass) == wanted)
        && request.quality.is_none_or(|wanted| item.quality == wanted)
        && (!request.usable_only
            || (item.required_level <= player_level
                && lyracore_shared::item::can_equip_proficiency(
                    player_class,
                    item.item_class,
                    item.item_subclass,
                )))
}

fn paginate(mut rows: Vec<Auction>, offset: u32) -> (Vec<Auction>, u32) {
    rows.sort_by_key(|row| row.id);
    let total = u32::try_from(rows.len()).unwrap_or(u32::MAX);
    let page = rows.into_iter().skip(offset as usize).take(50).collect();
    (page, total)
}

fn select_active_page(
    rows: impl IntoIterator<Item = Auction>,
    now_micros: i64,
    offset: u32,
    mut matches: impl FnMut(&Auction) -> bool,
) -> (Vec<Auction>, u32) {
    paginate(
        rows.into_iter()
            .filter(|row| {
                row.house == lyracore_shared::auction::STORMWIND_HOUSE_ID
                    && row.expires_at.to_micros_since_unix_epoch() > now_micros
                    && matches(row)
            })
            .collect(),
        offset,
    )
}

fn bidder_matches(row: &Auction, player_guid: u64, _outbid_auction_ids: &[u32]) -> bool {
    // Vanilla sends recently outbid ids as cache-invalidation hints. They never authorize a
    // displaced row back into the bidder tab; the authoritative winner is the realm Auction.
    row.highest_bidder_guid == player_guid
}

fn now_micros() -> i64 {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();
    i64::try_from(micros).unwrap_or(i64::MAX)
}

fn auction_view(row: Auction) -> crate::codec::AuctionView {
    crate::codec::AuctionView {
        id: row.id,
        item_entry: row.item_entry,
        item_stack_count: row.item_stack_count,
        item_enchant_id: row.item_enchant_id,
        owner_guid: row.owner_guid,
        start_bid: row.start_bid,
        buyout: row.buyout,
        highest_bidder_guid: row.highest_bidder_guid,
        highest_bid: row.highest_bid,
        expires_at_micros: row.expires_at.to_micros_since_unix_epoch(),
    }
}

impl Coordinator {
    pub(crate) fn auction_query(
        &self,
        player_guid: u64,
        query: AuctionQuery,
    ) -> Result<AuctionPage> {
        let (player_level, player_class) = {
            let guard = self.0.coord();
            let player = guard
                .conn
                .db
                .game_world_entity()
                .guid()
                .find(&player_guid)
                .ok_or_else(|| anyhow!("auction query actor {player_guid} is not in world"))?;
            (
                u8::try_from(player.level).unwrap_or(u8::MAX),
                ((player.unit_bytes_0 >> 8) & 0xff) as u8,
            )
        };
        let now_micros = now_micros();
        let market = self.realm_core()?;
        let guard = market.0.coord();
        let db = &guard.conn.db;
        let offset = match &query {
            AuctionQuery::Browse(request) => request.offset,
            AuctionQuery::Owner { offset } => *offset,
            AuctionQuery::Bidder { offset, .. } => *offset,
        };
        let (rows, total) = select_active_page(
            db.game_auction().iter(),
            now_micros,
            offset,
            |row| match &query {
                AuctionQuery::Browse(request) => db
                    .game_item_template()
                    .entry()
                    .find(&row.item_entry)
                    .is_some_and(|item| {
                        browse_matches(
                            request,
                            player_level,
                            player_class,
                            BrowseFacts {
                                name: &item.name,
                                required_level: item.required_level,
                                inventory_type: item.inventory_type,
                                item_class: item.class,
                                item_subclass: item.subclass,
                                quality: item.quality,
                            },
                        )
                    }),
                AuctionQuery::Owner { .. } => row.owner_guid == player_guid,
                AuctionQuery::Bidder {
                    outbid_auction_ids,
                    ..
                } => bidder_matches(row, player_guid, outbid_auction_ids),
            },
        );

        Ok(AuctionPage {
            rows: rows.into_iter().map(auction_view).collect(),
            total,
            now_micros,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> AuctionBrowseRequest {
        AuctionBrowseRequest {
            auctioneer_guid: 1,
            offset: 0,
            name: "SWORD".to_owned(),
            minimum_level: Some(10),
            maximum_level: Some(20),
            inventory_type: Some(13),
            item_class: Some(2),
            item_subclass: Some(7),
            quality: Some(2),
            usable_only: true,
        }
    }

    fn facts() -> BrowseFacts<'static> {
        BrowseFacts {
            name: "Solid Short Sword",
            required_level: 15,
            inventory_type: 13,
            item_class: 2,
            item_subclass: 7,
            quality: 2,
        }
    }

    #[test]
    fn browse_supports_every_exact_filter_and_the_existing_usable_model() {
        assert!(browse_matches(&request(), 15, 1, facts()));

        let mismatches = [
            AuctionBrowseRequest {
                name: "axe".to_owned(),
                ..request()
            },
            AuctionBrowseRequest {
                minimum_level: Some(16),
                ..request()
            },
            AuctionBrowseRequest {
                maximum_level: Some(14),
                ..request()
            },
            AuctionBrowseRequest {
                inventory_type: Some(17),
                ..request()
            },
            AuctionBrowseRequest {
                item_class: Some(4),
                ..request()
            },
            AuctionBrowseRequest {
                item_subclass: Some(8),
                ..request()
            },
            AuctionBrowseRequest {
                quality: Some(3),
                ..request()
            },
        ];
        for mismatch in mismatches {
            assert!(!browse_matches(&mismatch, 15, 1, facts()));
        }
        assert!(!browse_matches(&request(), 14, 1, facts()));
        assert!(!browse_matches(&request(), 15, 5, facts()));
        assert!(browse_matches(
            &AuctionBrowseRequest {
                usable_only: false,
                ..request()
            },
            15,
            5,
            facts(),
        ));
        assert!(browse_matches(
            &AuctionBrowseRequest {
                auctioneer_guid: 1,
                offset: 0,
                name: String::new(),
                minimum_level: None,
                maximum_level: None,
                inventory_type: None,
                item_class: None,
                item_subclass: None,
                quality: None,
                usable_only: false,
            },
            1,
            5,
            facts(),
        ));
    }

    fn auction(id: u32) -> Auction {
        Auction {
            id,
            listing_operation_id: u64::from(id),
            house: lyracore_shared::auction::STORMWIND_HOUSE_ID,
            owner_guid: 1,
            item_guid: u64::from(id),
            item_entry: 25,
            item_stack_count: 1,
            item_durability: 10,
            item_enchant_id: 0,
            item_soulbound: false,
            start_bid: 1,
            buyout: 0,
            highest_bidder_guid: 0,
            highest_bid: 0,
            deposit: 1,
            created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
            expires_at: spacetimedb_sdk::Timestamp::from_micros_since_unix_epoch(i64::MAX),
            revision: 0,
        }
    }

    #[test]
    fn pagination_sorts_before_taking_fifty_and_reports_the_full_total() {
        let rows = (1..=55).rev().map(auction).collect();
        let (first, total) = paginate(rows, 0);
        assert_eq!(total, 55);
        assert_eq!(first.len(), 50);
        assert_eq!(first.first().unwrap().id, 1);
        assert_eq!(first.last().unwrap().id, 50);

        let rows = (1..=55).rev().map(auction).collect();
        let (second, total) = paginate(rows, 50);
        assert_eq!(total, 55);
        assert_eq!(
            second.iter().map(|row| row.id).collect::<Vec<_>>(),
            (51..=55).collect::<Vec<_>>()
        );
    }

    #[test]
    fn active_selection_excludes_expired_other_house_and_non_owner_rows_before_totals() {
        let mut expired = auction(1);
        expired.expires_at = spacetimedb_sdk::Timestamp::from_micros_since_unix_epoch(10);
        let mut at_deadline = auction(2);
        at_deadline.expires_at = spacetimedb_sdk::Timestamp::from_micros_since_unix_epoch(20);
        let mut other_house = auction(3);
        other_house.house = 7;
        let mut other_owner = auction(4);
        other_owner.owner_guid = 2;
        let owned = auction(5);

        let rows = vec![expired, at_deadline, other_house, other_owner, owned];
        let (page, total) = select_active_page(rows, 20, 0, |row| row.owner_guid == 1);
        assert_eq!(total, 1);
        assert_eq!(page.iter().map(|row| row.id).collect::<Vec<_>>(), vec![5]);
    }

    #[test]
    fn bidder_selection_never_reintroduces_requested_outbid_auctions() {
        let mut highest = auction(5);
        highest.highest_bidder_guid = 8;
        highest.highest_bid = 107;
        let mut displaced = auction(19);
        displaced.highest_bidder_guid = 9;
        displaced.highest_bid = 113;

        let requested_outbid_ids = [19, 88];
        let (page, total) = select_active_page(
            vec![displaced, highest],
            20,
            0,
            |row| bidder_matches(row, 8, &requested_outbid_ids),
        );
        assert_eq!(total, 1);
        assert_eq!(page.iter().map(|row| row.id).collect::<Vec<_>>(), vec![5]);
    }
}
