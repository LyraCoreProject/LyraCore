//! The auction rows of each Character, kept from the same row callbacks that fill the coordinator
//! cache. The SDK cache indexes these tables by primary key only, and the Hold and receipt tables
//! keep every finished row, so a per-request lookup by Character would otherwise scan them.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, RwLock, RwLockWriteGuard};

use lyracore_shared::auction::bid_outcome;
use spacetimedb_sdk::{Table, TableWithPrimaryKey};

use super::bindings::*;

/// A Hold still owes a phase: Realm-core has not decided it, or its refund waits for mail.
pub(crate) fn hold_is_unfinished(hold: &AuctionBidHold) -> bool {
    hold.outcome == bid_outcome::PENDING || hold.deferred_refund != 0
}

/// Primary keys of one table's rows, by the Character each row belongs to.
#[derive(Default)]
pub(crate) struct KeysByCharacter {
    by_character: HashMap<u64, BTreeSet<u64>>,
}

impl KeysByCharacter {
    fn insert(&mut self, character_guid: u64, key: u64) {
        self.by_character
            .entry(character_guid)
            .or_default()
            .insert(key);
    }

    fn remove(&mut self, character_guid: u64, key: u64) {
        if let Some(keys) = self.by_character.get_mut(&character_guid) {
            keys.remove(&key);
            if keys.is_empty() {
                self.by_character.remove(&character_guid);
            }
        }
    }

    /// The keys of `character_guid`'s rows, lowest first. A caller reads each row back by primary
    /// key, so a stale key costs one missed lookup and nothing else.
    pub(crate) fn keys_of(&self, character_guid: u64) -> Vec<u64> {
        self.by_character
            .get(&character_guid)
            .map(|keys| keys.iter().copied().collect())
            .unwrap_or_default()
    }
}

/// Every per-Character auction index one coordinator keeps.
#[derive(Default)]
pub(crate) struct AuctionIndex {
    /// Unfinished bid and Cancellation Holds, by bidder.
    pub(crate) unfinished_bid_holds: KeysByCharacter,
    /// Listing Holds, by seller. Settle and release delete a listing Hold, so each one is
    /// unfinished.
    pub(crate) listing_holds: KeysByCharacter,
    /// Listing receipts, by seller.
    pub(crate) listing_receipts: KeysByCharacter,
    /// Auctions, by seller and by highest bidder.
    pub(crate) auctions: KeysByCharacter,
}

impl AuctionIndex {
    fn add_bid_hold(&mut self, row: &AuctionBidHold) {
        if hold_is_unfinished(row) {
            self.unfinished_bid_holds
                .insert(row.bidder_guid, row.operation_id);
        }
    }

    fn remove_bid_hold(&mut self, row: &AuctionBidHold) {
        self.unfinished_bid_holds
            .remove(row.bidder_guid, row.operation_id);
    }

    fn add_auction(&mut self, row: &Auction) {
        for character_guid in [row.owner_guid, row.highest_bidder_guid] {
            if character_guid != 0 {
                self.auctions.insert(character_guid, u64::from(row.id));
            }
        }
    }

    fn remove_auction(&mut self, row: &Auction) {
        for character_guid in [row.owner_guid, row.highest_bidder_guid] {
            self.auctions.remove(character_guid, u64::from(row.id));
        }
    }
}

fn locked(index: &RwLock<AuctionIndex>) -> RwLockWriteGuard<'_, AuctionIndex> {
    index
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Keep an [`AuctionIndex`] current from the auction row callbacks. Initial subscription rows
/// arrive as inserts.
pub(crate) fn watch_auctions(conn: &DbConnection) -> Arc<RwLock<AuctionIndex>> {
    let index = Arc::new(RwLock::new(AuctionIndex::default()));

    let i = index.clone();
    conn.db
        .game_auction_bid_hold()
        .on_insert(move |_ctx, row| locked(&i).add_bid_hold(row));
    let i = index.clone();
    conn.db
        .game_auction_bid_hold()
        .on_delete(move |_ctx, row| locked(&i).remove_bid_hold(row));
    let i = index.clone();
    conn.db
        .game_auction_bid_hold()
        .on_update(move |_ctx, old, new| {
            let mut index = locked(&i);
            index.remove_bid_hold(old);
            index.add_bid_hold(new);
        });

    let i = index.clone();
    conn.db.game_auction_hold().on_insert(move |_ctx, row| {
        locked(&i)
            .listing_holds
            .insert(row.seller_guid, row.operation_id)
    });
    let i = index.clone();
    conn.db.game_auction_hold().on_delete(move |_ctx, row| {
        locked(&i)
            .listing_holds
            .remove(row.seller_guid, row.operation_id)
    });

    let i = index.clone();
    conn.db
        .game_auction_operation_receipt()
        .on_insert(move |_ctx, row| {
            locked(&i)
                .listing_receipts
                .insert(row.actor_guid, row.operation_id)
        });
    let i = index.clone();
    conn.db
        .game_auction_operation_receipt()
        .on_delete(move |_ctx, row| {
            locked(&i)
                .listing_receipts
                .remove(row.actor_guid, row.operation_id)
        });

    let i = index.clone();
    conn.db
        .game_auction()
        .on_insert(move |_ctx, row| locked(&i).add_auction(row));
    let i = index.clone();
    conn.db
        .game_auction()
        .on_delete(move |_ctx, row| locked(&i).remove_auction(row));
    let i = index.clone();
    conn.db.game_auction().on_update(move |_ctx, old, new| {
        let mut index = locked(&i);
        index.remove_auction(old);
        index.add_auction(new);
    });
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hold(
        operation_id: u64,
        bidder_guid: u64,
        outcome: u8,
        deferred_refund: u32,
    ) -> AuctionBidHold {
        AuctionBidHold {
            operation_id,
            bidder_guid,
            auction_id: 41,
            offer: 10,
            outcome,
            revision: 0,
            result_bidder_guid: 0,
            result_bid: 0,
            minimum_increment: 0,
            deferred_refund,
            accepted_price: 0,
            house: 1,
            operation: lyracore_shared::auction::hold_operation::CANCEL,
        }
    }

    #[test]
    fn a_hold_leaves_the_index_once_it_is_decided_and_refunded() {
        let mut index = AuctionIndex::default();
        let pending = hold(5, 7, 0, 0);
        index.add_bid_hold(&pending);
        index.add_bid_hold(&hold(3, 7, 0, 0));
        index.add_bid_hold(&hold(9, 8, 0, 0));
        assert_eq!(index.unfinished_bid_holds.keys_of(7), vec![3, 5]);

        // A decision whose refund overflowed the purse still owes the refund phases.
        let overflowed = hold(5, 7, 2, 10);
        index.remove_bid_hold(&pending);
        index.add_bid_hold(&overflowed);
        assert_eq!(index.unfinished_bid_holds.keys_of(7), vec![3, 5]);

        index.remove_bid_hold(&overflowed);
        index.add_bid_hold(&hold(5, 7, 2, 0));
        assert_eq!(index.unfinished_bid_holds.keys_of(7), vec![3]);
        index.remove_bid_hold(&hold(3, 7, 0, 0));
        assert!(index.unfinished_bid_holds.keys_of(7).is_empty());
        assert_eq!(index.unfinished_bid_holds.keys_of(8), vec![9]);
    }

    #[test]
    fn a_finished_hold_never_enters_the_index() {
        let mut index = AuctionIndex::default();
        index.add_bid_hold(&hold(5, 7, 7, 0));
        index.add_bid_hold(&hold(6, 7, 1, 0));
        assert!(index.unfinished_bid_holds.keys_of(7).is_empty());
    }

    fn auction(id: u32, owner_guid: u64, highest_bidder_guid: u64) -> Auction {
        Auction {
            id,
            listing_operation_id: 1,
            house: 7,
            owner_guid,
            item_guid: 2,
            item_entry: 25,
            item_stack_count: 1,
            item_durability: 0,
            item_enchant_id: 0,
            item_soulbound: false,
            start_bid: 100,
            buyout: 0,
            highest_bidder_guid,
            highest_bid: if highest_bidder_guid == 0 { 0 } else { 100 },
            deposit: 5,
            created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
            expires_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
            revision: 0,
            deposit_rate: 5,
            consignment_rate: 5,
            random_property_id: 0,
            item_text_id: 0,
        }
    }

    #[test]
    fn an_auction_is_found_by_its_seller_and_its_highest_bidder_only() {
        let mut index = AuctionIndex::default();
        let unbid = auction(41, 7, 0);
        index.add_auction(&unbid);
        assert_eq!(index.auctions.keys_of(7), vec![41]);
        assert!(index.auctions.keys_of(0).is_empty());

        let outbid = auction(41, 7, 8);
        index.remove_auction(&unbid);
        index.add_auction(&outbid);
        let raised = auction(41, 7, 9);
        index.remove_auction(&outbid);
        index.add_auction(&raised);
        assert_eq!(index.auctions.keys_of(9), vec![41]);
        assert!(
            index.auctions.keys_of(8).is_empty(),
            "a displaced bidder no longer has the auction"
        );

        index.remove_auction(&raised);
        assert!(index.auctions.keys_of(7).is_empty());
        assert!(index.auctions.keys_of(9).is_empty());
    }
}
