use super::super::*;
use crate::world::Actor;

#[derive(Default)]
pub(crate) struct AuctionState {
    /// The auctioneer's house and faction verdict returned to the focused auction seam.
    pub(crate) auction_interaction: Option<AuctionInteraction>,
}

impl AuctionActionStore for WorldFake {
    fn auction_interaction(
        &self,
        _actor: Actor,
        _auctioneer_guid: u64,
    ) -> Result<Option<AuctionInteraction>> {
        self.rec("auction_interaction");
        Ok(self.auction.auction_interaction)
    }

    fn create_auction(
        &self,
        _request: crate::world::handlers::CreateAuctionRequest,
    ) -> Result<crate::world::handlers::CreateAuctionOutcome> {
        Ok(crate::world::handlers::CreateAuctionOutcome::Database)
    }

    fn place_bid(
        &self,
        _request: crate::world::handlers::PlaceBidRequest,
    ) -> Result<crate::world::handlers::PlaceBidOutcome> {
        Ok(crate::world::handlers::PlaceBidOutcome::Database)
    }

    fn cancel_auction(
        &self,
        _request: crate::world::handlers::CancelAuctionRequest,
    ) -> Result<crate::world::handlers::CancelAuctionOutcome> {
        Ok(crate::world::handlers::CancelAuctionOutcome::Stale)
    }

    fn resume_auction_holds(&self, _actor: Actor) -> Result<()> {
        Ok(())
    }

    fn auction_query(
        &self,
        _actor: Actor,
        _house_id: u32,
        _query: crate::world::handlers::AuctionQuery,
    ) -> Result<crate::world::handlers::AuctionPage> {
        Ok(crate::world::handlers::AuctionPage {
            rows: Vec::new(),
            total: 0,
            now_micros: 0,
        })
    }
}
