//! The unfinished bid Holds of each Character, kept from the same row callbacks that fill the
//! coordinator cache. The SDK cache indexes `game_auction_bid_hold` by operation id only, and the
//! table keeps every finished Hold, so a per-request lookup by Character would otherwise scan it.

use std::collections::{BTreeSet, HashMap};

use lyracore_shared::auction::bid_outcome;

use super::bindings::AuctionBidHold;

/// A Hold still owes a phase: Realm-core has not decided it, or its refund waits for mail.
pub(crate) fn hold_is_unfinished(hold: &AuctionBidHold) -> bool {
    hold.outcome == bid_outcome::PENDING || hold.deferred_refund != 0
}

#[derive(Default)]
pub(crate) struct UnfinishedHoldIndex {
    by_actor: HashMap<u64, BTreeSet<u64>>,
}

impl UnfinishedHoldIndex {
    pub(crate) fn insert(&mut self, row: &AuctionBidHold) {
        if hold_is_unfinished(row) {
            self.by_actor
                .entry(row.bidder_guid)
                .or_default()
                .insert(row.operation_id);
        }
    }

    pub(crate) fn remove(&mut self, row: &AuctionBidHold) {
        if let Some(operations) = self.by_actor.get_mut(&row.bidder_guid) {
            operations.remove(&row.operation_id);
            if operations.is_empty() {
                self.by_actor.remove(&row.bidder_guid);
            }
        }
    }

    /// The operation ids of `actor_guid`'s unfinished Holds, oldest id first. A caller reads each
    /// row back by primary key, so a stale id costs one missed lookup and nothing else.
    pub(crate) fn operations_of(&self, actor_guid: u64) -> Vec<u64> {
        self.by_actor
            .get(&actor_guid)
            .map(|operations| operations.iter().copied().collect())
            .unwrap_or_default()
    }
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
        let mut index = UnfinishedHoldIndex::default();
        let pending = hold(5, 7, 0, 0);
        index.insert(&pending);
        index.insert(&hold(3, 7, 0, 0));
        index.insert(&hold(9, 8, 0, 0));
        assert_eq!(index.operations_of(7), vec![3, 5]);

        // A decision whose refund overflowed the purse still owes the refund phases.
        let overflowed = hold(5, 7, 2, 10);
        index.remove(&pending);
        index.insert(&overflowed);
        assert_eq!(index.operations_of(7), vec![3, 5]);

        index.remove(&overflowed);
        index.insert(&hold(5, 7, 2, 0));
        assert_eq!(index.operations_of(7), vec![3]);
        index.remove(&hold(3, 7, 0, 0));
        assert!(index.operations_of(7).is_empty());
        assert_eq!(index.operations_of(8), vec![9]);
    }

    #[test]
    fn a_finished_hold_never_enters_the_index() {
        let mut index = UnfinishedHoldIndex::default();
        index.insert(&hold(5, 7, 7, 0));
        index.insert(&hold(6, 7, 1, 0));
        assert!(index.operations_of(7).is_empty());
    }
}
