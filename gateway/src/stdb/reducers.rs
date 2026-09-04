//! Reducer-call wrapper methods on `Coordinator`: each fires a `gw_*` module reducer over the
//! shared coordinator call pipe (the module attributes the caller by the `actor_guid` argument)
//! and blocks on its completion via the `call_reducer!` macro. Cache reads live in `reads.rs`.

use anyhow::{anyhow, Result};
use spacetimedb_sdk::{Identity, Table};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::bindings::*;
use super::connection::{call_reducer, recv_reducer_on, reducer_refusal_reason, Coordinator};
use super::views::entity_view;
use crate::world::{
    ItemActionResult, LootActionStatus, LootWindowRefusal, LootWindowRequestStatus,
};
use lyracore_shared::auction::AuctionRefusal;
use lyracore_shared::item::ItemRefusal;
use lyracore_shared::loot::{LootBoundaryFailure, LootRefusal};
use lyracore_shared::trainer::TrainerRefusal;

static NEXT_TAXI_REQUEST_ID: OnceLock<AtomicU64> = OnceLock::new();

fn next_taxi_request_id() -> u64 {
    // Seed from this process start's wall-clock nanoseconds. The reply table survives a gateway
    // restart, so restarting the old `1, 2, ...` sequence could make the cache's pre-restart row
    // look like the just-committed reply before its replacement subscription delta arrived.
    let next = NEXT_TAXI_REQUEST_ID.get_or_init(|| {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        AtomicU64::new(seed.max(1))
    });
    let id = next.fetch_add(1, Ordering::Relaxed);
    if id == 0 {
        next.fetch_add(1, Ordering::Relaxed)
    } else {
        id
    }
}

fn taxi_reply_matches(
    reply: &TaxiServiceReply,
    character_guid: u64,
    npc_guid: u64,
    operation: u8,
) -> bool {
    reply.character_guid == character_guid
        && reply.operation == operation
        && reply.npc_guid == npc_guid
}

impl Coordinator {
    /// Delete one subscribed bot invite intent on this World Shard. A missing row is the expected
    /// result for every losing Gateway callback, while transport and other Module failures remain
    /// caller-visible.
    pub fn claim_bot_invite_intent(&self, intent_id: u64) -> Result<bool> {
        match call_reducer!(
            self.0.call_pipe().conn.reducers,
            "claim_bot_invite_intent",
            claim_bot_invite_intent_then(intent_id)
        ) {
            Ok(()) => Ok(true),
            Err(error)
                if reducer_refusal_reason(&error)
                    == Some(lyracore_shared::group::err::BOT_INVITE_INTENT_ALREADY_CLAIMED) =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    fn await_taxi_reply(
        &self,
        character_guid: u64,
        npc_guid: u64,
        request_id: u64,
        operation: u8,
    ) -> Result<Option<TaxiServiceReply>> {
        // Reducer completion and subscription propagation travel on the same SDK connection but
        // are separate events. Poll by the caller-chosen id so an older cached reply can never be
        // mistaken for this operation's result.
        for _ in 0..100 {
            let reply = self
                .0
                .coord()
                .conn
                .db
                .game_taxi_service_reply()
                .request_id()
                .find(&request_id);
            if let Some(reply) =
                reply.filter(|reply| taxi_reply_matches(reply, character_guid, npc_guid, operation))
            {
                // Copy the cache row before acknowledging it. The ack deletes only this request's
                // module row; a failed ack is infrastructure loss and remains session-fatal.
                call_reducer!(
                    self.0.call_pipe().conn.reducers,
                    "gw_ack_taxi_reply",
                    gw_ack_taxi_reply_then(character_guid, request_id)
                )?;
                if !reply.accepted {
                    log::debug!(
                        "taxi operation {operation} refused for character {character_guid}: {}",
                        reply.refusal
                    );
                }
                return Ok(Some(reply));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Err(anyhow!(
            "taxi operation {operation} committed but reply {request_id} was not visible within 1s"
        ))
    }

    /// Cohesive status request: module resolution + policy runs before this returns one known bit.
    pub fn taxi_node_status(
        &self,
        character_guid: u64,
        npc_guid: u64,
    ) -> Result<Option<crate::codec::TaxiNodeStatusView>> {
        if character_guid == 0 {
            return Ok(None);
        }
        let request_id = next_taxi_request_id();
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_taxi_node_status",
            gw_taxi_node_status_then(character_guid, npc_guid, request_id)
        )?;
        Ok(self
            .await_taxi_reply(
                character_guid,
                npc_guid,
                request_id,
                lyracore_shared::constants::taxi_protocol::REPLY_STATUS,
            )?
            .filter(|reply| reply.accepted)
            .map(|reply| crate::codec::TaxiNodeStatusView {
                npc_guid: reply.npc_guid,
                known: reply.known,
            }))
    }

    /// Cohesive open request: source discovery and direct-route filtering commit together, then
    /// this projects the module's client node ids without re-reading raw taxi tables.
    pub fn open_taxi(
        &self,
        character_guid: u64,
        npc_guid: u64,
    ) -> Result<Option<crate::codec::TaxiMapView>> {
        if character_guid == 0 {
            return Ok(None);
        }
        let request_id = next_taxi_request_id();
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_open_taxi",
            gw_open_taxi_then(character_guid, npc_guid, request_id)
        )?;
        Ok(self
            .await_taxi_reply(
                character_guid,
                npc_guid,
                request_id,
                lyracore_shared::constants::taxi_protocol::REPLY_OPEN,
            )?
            .filter(|reply| reply.accepted)
            .map(|reply| crate::codec::TaxiMapView {
                npc_guid: reply.npc_guid,
                source_client_node_id: reply.source_client_node_id,
                available_client_node_ids: reply.available_client_node_ids,
            }))
    }

    /// Cohesive direct-flight activation. The module commits every gameplay outcome as a stable
    /// result code; only reducer transport/timeout failures escape as `Err` and end the session.
    pub fn activate_taxi(
        &self,
        character_guid: u64,
        npc_guid: u64,
        source_client_node_id: u32,
        destination_client_node_id: u32,
    ) -> Result<crate::codec::TaxiActivationResult> {
        if character_guid == 0 {
            return Ok(crate::codec::TaxiActivationResult {
                result_code:
                    lyracore_shared::constants::taxi_protocol::ACTIVATE_UNSPECIFIED_SERVER_ERROR,
            });
        }
        let request_id = next_taxi_request_id();
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_activate_taxi",
            gw_activate_taxi_then(
                character_guid,
                npc_guid,
                source_client_node_id,
                destination_client_node_id,
                request_id
            )
        )?;
        let reply = self
            .await_taxi_reply(
                character_guid,
                npc_guid,
                request_id,
                lyracore_shared::constants::taxi_protocol::REPLY_ACTIVATE,
            )?
            .ok_or_else(|| anyhow!("taxi activation reply {request_id} disappeared"))?;
        Ok(crate::codec::TaxiActivationResult {
            result_code: reply.result_code,
        })
    }

    pub fn arm_taxi_flight(&self, character_guid: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_arm_taxi_flight",
            gw_arm_taxi_flight_then(character_guid)
        )
    }

    /// Create one auction listing. Single-database deployments use one atomic reducer; sharded
    /// deployments drive the durable Hold -> Auction -> receipt -> settle protocol.
    pub(crate) fn create_auction(
        &self,
        request: crate::world::CreateAuctionRequest,
    ) -> Result<crate::world::CreateAuctionOutcome> {
        use crate::world::CreateAuctionOutcome;

        let item_is_present = self
            .0
            .coord()
            .conn
            .db
            .game_item_instance()
            .guid()
            .find(&request.item_guid)
            .is_some();
        // A successful listing removes the source item in the same transaction that records its
        // Hold or receipt. Presence therefore proves this is a new request even when a returned
        // item later reuses the same guid and terms.
        if !item_is_present {
            if let Some(receipt) = self.matching_active_auction_receipt(request)? {
                if self
                    .0
                    .coord()
                    .conn
                    .db
                    .game_auction_hold()
                    .operation_id()
                    .find(&receipt.operation_id)
                    .is_some()
                {
                    self.auction_settle_listing(receipt.operation_id)?;
                }
                return Ok(CreateAuctionOutcome::Created {
                    auction_id: receipt.auction_id,
                });
            }
        }

        let operation_id = if item_is_present {
            next_auction_operation_id()?
        } else {
            match self.matching_auction_hold(request) {
                Some(hold) => hold.operation_id,
                None => next_auction_operation_id()?,
            }
        };

        let result = if self.is_sharded() {
            self.drive_sharded_auction_listing(operation_id, request)
        } else {
            self.auction_list_local(operation_id, request)
                .and_then(|()| self.wait_for_auction_receipt(operation_id))
                .map(|receipt| receipt.auction_id)
        };

        match result {
            Ok(auction_id) => Ok(CreateAuctionOutcome::Created { auction_id }),
            Err(error) => match auction_refusal(&error) {
                Some(refusal) => Ok(refusal.into()),
                None => Err(error),
            },
        }
    }

    fn drive_sharded_auction_listing(
        &self,
        operation_id: u64,
        request: crate::world::CreateAuctionRequest,
    ) -> Result<u32> {
        self.auction_hold_listing(operation_id, request)?;
        let hold = self.wait_for_auction_hold(operation_id)?;
        let realm = self.realm_core()?;
        if let Err(error) = realm.auction_commit_listing(&hold) {
            // Only a Refusal proves realm-core took nothing. A timeout or transport failure
            // leaves the Hold for the next replay, which commits idempotently.
            if reducer_refusal_reason(&error).is_some()
                && !realm.auction_receipt_is_visible(operation_id)
            {
                // Mail is authoritative on Realm-core. Commit its idempotent refund receipt and
                // the exact returned value there before deleting the source Hold. If either call
                // is interrupted, the Hold remains recovery evidence and the next replay resumes
                // from the same operation id.
                realm.auction_refund_listing(&hold)?;
                self.auction_release_listing_hold(&hold)?;
            }
            return Err(error);
        }
        let receipt = realm.wait_for_auction_receipt(operation_id)?;
        self.auction_confirm_listing(operation_id, receipt.auction_id)?;
        self.wait_for_auction_receipt(operation_id)?;
        self.auction_settle_listing(operation_id)?;
        Ok(receipt.auction_id)
    }

    fn auction_list_local(
        &self,
        operation_id: u64,
        request: crate::world::CreateAuctionRequest,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_auction_list_local",
            gw_auction_list_local_then(
                operation_id,
                request.actor_guid,
                request.item_guid,
                request.auctioneer_guid,
                request.house_id,
                request.start_bid,
                request.buyout,
                request.duration_minutes
            )
        )
    }

    fn auction_hold_listing(
        &self,
        operation_id: u64,
        request: crate::world::CreateAuctionRequest,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_auction_hold_listing",
            gw_auction_hold_listing_then(
                operation_id,
                request.actor_guid,
                request.item_guid,
                request.auctioneer_guid,
                request.house_id,
                request.start_bid,
                request.buyout,
                request.duration_minutes
            )
        )
    }

    fn auction_commit_listing(&self, hold: &AuctionHold) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_auction_commit_listing",
            realm_auction_commit_listing_then(
                hold.operation_id,
                hold.seller_guid,
                hold.item_guid,
                hold.item_entry,
                hold.item_stack_count,
                hold.item_durability,
                hold.item_enchant_id,
                hold.item_soulbound,
                hold.house,
                hold.deposit_rate,
                hold.consignment_rate,
                hold.start_bid,
                hold.buyout,
                hold.duration_minutes,
                hold.deposit,
                hold.created_micros,
                hold.expires_micros
            )
        )
    }

    fn auction_confirm_listing(&self, operation_id: u64, auction_id: u32) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_auction_confirm_listing",
            realm_auction_confirm_listing_then(operation_id, auction_id)
        )
    }

    fn auction_settle_listing(&self, operation_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_auction_settle_listing",
            realm_auction_settle_listing_then(operation_id)
        )
    }

    fn auction_refund_listing(&self, hold: &AuctionHold) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_auction_refund_listing",
            realm_auction_refund_listing_then(
                hold.operation_id,
                hold.seller_guid,
                hold.item_guid,
                hold.item_entry,
                hold.item_stack_count,
                hold.item_durability,
                hold.item_enchant_id,
                hold.item_soulbound,
                hold.house,
                hold.deposit_rate,
                hold.consignment_rate,
                hold.start_bid,
                hold.buyout,
                hold.duration_minutes,
                hold.deposit,
                hold.created_micros,
                hold.expires_micros
            )
        )
    }

    fn auction_release_listing_hold(&self, hold: &AuctionHold) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_auction_release_listing_hold",
            gw_auction_release_listing_hold_then(hold.operation_id, hold.seller_guid)
        )
    }

    fn auction_receipt_is_visible(&self, operation_id: u64) -> bool {
        self.0
            .coord()
            .conn
            .db
            .game_auction_operation_receipt()
            .operation_id()
            .find(&operation_id)
            .is_some_and(|receipt| receipt.auction_id != 0)
    }

    fn matching_auction_hold(
        &self,
        request: crate::world::CreateAuctionRequest,
    ) -> Option<AuctionHold> {
        let guard = self.0.coord();
        let hold = guard
            .conn
            .db
            .game_auction_hold()
            .iter()
            .find(|hold| same_auction_request(hold, request));
        hold
    }

    fn matching_active_auction_receipt(
        &self,
        request: crate::world::CreateAuctionRequest,
    ) -> Result<Option<AuctionOperationReceipt>> {
        let candidates = {
            let guard = self.0.coord();
            guard
                .conn
                .db
                .game_auction_operation_receipt()
                .iter()
                .filter(|receipt| same_auction_request(receipt, request))
                .collect::<Vec<_>>()
        };
        if candidates.is_empty() {
            return Ok(None);
        }

        // Receipts outlive Auctions so an operation replay stays auditable. Only reuse a receipt
        // while its Auction is still active: returned items can later be granted the same item guid
        // and legitimately listed again with the same terms.
        let realm = self.realm_core()?;
        let guard = realm.0.coord();
        let receipt = candidates.into_iter().find(|receipt| {
            guard
                .conn
                .db
                .game_auction()
                .id()
                .find(&receipt.auction_id)
                .is_some_and(|auction| auction.listing_operation_id == receipt.operation_id)
        });
        Ok(receipt)
    }

    fn wait_for_auction_hold(&self, operation_id: u64) -> Result<AuctionHold> {
        wait_for_auction_cache_row(operation_id, "Hold", || {
            self.0
                .coord()
                .conn
                .db
                .game_auction_hold()
                .operation_id()
                .find(&operation_id)
        })
    }

    fn wait_for_auction_receipt(&self, operation_id: u64) -> Result<AuctionOperationReceipt> {
        wait_for_auction_cache_row(operation_id, "receipt", || {
            self.0
                .coord()
                .conn
                .db
                .game_auction_operation_receipt()
                .operation_id()
                .find(&operation_id)
                .filter(|receipt| receipt.auction_id != 0)
        })
    }

    /// Place one full-offer bid. The sharded path fences the bidder purse before realm-core makes
    /// its serialized Auction decision; both paths finish with the same normalized terminal Hold.
    pub(crate) fn place_bid(
        &self,
        request: crate::world::PlaceBidRequest,
    ) -> Result<crate::world::PlaceBidOutcome> {
        let operation_id = self
            .matching_unfinished_bid_hold(request)
            .map_or_else(next_auction_operation_id, |hold| Ok(hold.operation_id))?;
        let result = if self.is_sharded() {
            self.drive_sharded_auction_bid(operation_id, request)
        } else {
            self.auction_bid_local(operation_id, request)?;
            self.wait_for_terminal_bid_hold(operation_id)
        };
        match result {
            Ok(hold) => bid_outcome(&hold),
            Err(error) => match auction_refusal(&error) {
                Some(refusal) => Ok(refusal.into()),
                None => Err(error),
            },
        }
    }

    fn drive_sharded_auction_bid(
        &self,
        operation_id: u64,
        request: crate::world::PlaceBidRequest,
    ) -> Result<AuctionBidHold> {
        self.auction_hold_bid(operation_id, request)?;
        let mut hold = self.wait_for_auction_bid_hold(operation_id)?;
        let realm = self.realm_core()?;
        if hold.outcome == lyracore_shared::auction::bid_outcome::PENDING {
            realm.auction_decide_bid(&hold)?;
            let decision = realm.wait_for_auction_bid_decision(operation_id)?;
            self.auction_finish_bid(&hold, &decision)?;
            hold = self.wait_for_terminal_bid_hold(operation_id)?;
        }
        if hold.deferred_refund != 0 {
            realm.auction_refund_bid(&hold)?;
            realm.wait_for_bid_refund(&hold)?;
            self.auction_confirm_bid_refund(&hold)?;
            hold = self.wait_for_settled_bid_refund(hold.operation_id)?;
        }
        Ok(hold)
    }

    fn auction_bid_local(
        &self,
        operation_id: u64,
        request: crate::world::PlaceBidRequest,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_auction_bid_local",
            gw_auction_bid_local_then(
                operation_id,
                request.actor_guid,
                request.auctioneer_guid,
                request.auction_id,
                request.house_id,
                request.offer
            )
        )
    }

    fn auction_hold_bid(
        &self,
        operation_id: u64,
        request: crate::world::PlaceBidRequest,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_auction_hold_bid",
            gw_auction_hold_bid_then(
                operation_id,
                request.actor_guid,
                request.auctioneer_guid,
                request.auction_id,
                request.house_id,
                request.offer
            )
        )
    }

    fn auction_decide_bid(&self, hold: &AuctionBidHold) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_auction_decide_bid",
            realm_auction_decide_bid_then(
                hold.operation_id,
                hold.bidder_guid,
                hold.auction_id,
                hold.house,
                hold.offer
            )
        )
    }

    fn auction_finish_bid(
        &self,
        hold: &AuctionBidHold,
        decision: &AuctionBidDecision,
    ) -> Result<()> {
        if !bid_payload_matches(hold, decision) {
            return Err(anyhow!(
                "auction bid decision payload does not match its Hold"
            ));
        }
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_auction_finish_bid",
            gw_auction_finish_bid_then(
                hold.operation_id,
                hold.bidder_guid,
                hold.auction_id,
                hold.house,
                hold.offer,
                decision.outcome,
                decision.revision,
                decision.result_bidder_guid,
                decision.result_bid,
                decision.minimum_increment,
                decision.accepted_price
            )
        )
    }

    fn auction_refund_bid(&self, hold: &AuctionBidHold) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_auction_refund_bid",
            realm_auction_refund_bid_then(
                hold.operation_id,
                hold.bidder_guid,
                hold.auction_id,
                hold.house,
                hold.offer,
                hold.deferred_refund
            )
        )
    }

    fn auction_confirm_bid_refund(&self, hold: &AuctionBidHold) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_auction_confirm_bid_refund",
            gw_auction_confirm_bid_refund_then(
                hold.operation_id,
                hold.bidder_guid,
                hold.auction_id,
                hold.house,
                hold.offer,
                hold.deferred_refund
            )
        )
    }

    fn matching_unfinished_bid_hold(
        &self,
        request: crate::world::PlaceBidRequest,
    ) -> Option<AuctionBidHold> {
        self.0
            .coord()
            .conn
            .db
            .game_auction_bid_hold()
            .iter()
            .find(|hold| {
                (hold.outcome == lyracore_shared::auction::bid_outcome::PENDING
                    || hold.deferred_refund != 0)
                    && hold.bidder_guid == request.actor_guid
                    && hold.auction_id == request.auction_id
                    && hold.house == request.house_id
                    && hold.offer == request.offer
            })
    }

    fn wait_for_auction_bid_hold(&self, operation_id: u64) -> Result<AuctionBidHold> {
        wait_for_auction_cache_row(operation_id, "bid Hold", || {
            self.0
                .coord()
                .conn
                .db
                .game_auction_bid_hold()
                .operation_id()
                .find(&operation_id)
        })
    }

    fn wait_for_terminal_bid_hold(&self, operation_id: u64) -> Result<AuctionBidHold> {
        wait_for_auction_cache_row(operation_id, "terminal bid Hold", || {
            self.0
                .coord()
                .conn
                .db
                .game_auction_bid_hold()
                .operation_id()
                .find(&operation_id)
                .filter(|hold| hold.outcome != lyracore_shared::auction::bid_outcome::PENDING)
        })
    }

    fn wait_for_auction_bid_decision(&self, operation_id: u64) -> Result<AuctionBidDecision> {
        wait_for_auction_cache_row(operation_id, "bid decision", || {
            self.0
                .coord()
                .conn
                .db
                .game_auction_bid_decision()
                .operation_id()
                .find(&operation_id)
        })
    }

    fn wait_for_bid_refund(&self, hold: &AuctionBidHold) -> Result<AuctionBidDecision> {
        wait_for_auction_cache_row(hold.operation_id, "bid refund", || {
            self.0
                .coord()
                .conn
                .db
                .game_auction_bid_decision()
                .operation_id()
                .find(&hold.operation_id)
                .filter(|decision| bid_refund_is_recorded(hold, decision))
        })
    }

    fn wait_for_settled_bid_refund(&self, operation_id: u64) -> Result<AuctionBidHold> {
        wait_for_auction_cache_row(operation_id, "settled bid refund", || {
            self.0
                .coord()
                .conn
                .db
                .game_auction_bid_hold()
                .operation_id()
                .find(&operation_id)
                .filter(|hold| {
                    hold.outcome != lyracore_shared::auction::bid_outcome::PENDING
                        && hold.deferred_refund == 0
                })
        })
    }

    /// Enter the world (Phase 4): call the `player_login` reducer on the coordinator connection
    /// (so `ctx.sender` is the player's bound identity), then read the resulting
    /// `game_world_entity` row back through the privileged cache as an `EntityView`.
    pub fn player_login(
        &self,
        account_id: u64,
        character_guid: u64,
    ) -> Result<crate::codec::EntityView> {
        // Login rides `gw_player_login` on the COORDINATOR connection (module half: delegates to
        // apply_player_login with the account's bound identity as row owner, binds entity→lease,
        // fail-closed on either missing) — no per-player connection exists anywhere.
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_player_login",
            gw_player_login_then(account_id, character_guid)
        )?;

        // The reducer committed; the row propagates to the owner cache asynchronously. Poll
        // briefly until it appears (home_* ride along from the game_character row, and its
        // zone_id is the fallback for a live row the Module could not resolve a zone for).
        let char_row = self
            .0
            .coord()
            .conn
            .db
            .game_character()
            .guid()
            .find(&character_guid);
        let zone_id = char_row.as_ref().map(|c| c.zone_id).unwrap_or(0);
        let home_map = char_row.as_ref().map(|c| c.home_map).unwrap_or(0);
        let home_zone = char_row.as_ref().map(|c| c.home_zone).unwrap_or(0);
        let home_x = char_row.as_ref().map(|c| c.home_x).unwrap_or(0.0);
        let home_y = char_row.as_ref().map(|c| c.home_y).unwrap_or(0.0);
        let home_z = char_row.as_ref().map(|c| c.home_z).unwrap_or(0.0);
        // 15 s cap, 15 ms steps. Was 3 s — the cold-1000 measurement showed the reducer
        // COMMITTING while the coordinator stream lagged the login-burst tail past 3 s (writer at
        // 34.5%, so pure propagation, not CPU): 67/1000 logins died here with the entity already
        // live. The poll exits on first sight, so the longer cap costs nothing outside a burst.
        for _ in 0..1000 {
            if let Some(e) = self
                .0
                .coord()
                .conn
                .db
                .game_world_entity()
                .guid()
                .find(&character_guid)
            {
                let mut view = entity_view(e, zone_id);
                view.home_map = home_map;
                view.home_zone = home_zone;
                view.home_x = home_x;
                view.home_y = home_y;
                view.home_z = home_z;
                return Ok(view);
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        Err(anyhow!(
            "player_login committed but game_world_entity {character_guid} not visible in the \
             coordinator cache within 15s"
        ))
    }

    /// Heartbeat this gateway's `game_gateway_lease` row every 15 s on EVERY connected
    /// database's coordinator connection, forever. Every shard, not just the default:
    /// `gw_player_login` fail-closes on the lease of the database it runs ON, and a
    /// cross-database login (an instance entry resuming a transfer) runs on that destination
    /// shard — a default-only lease made every instance login die with "no lease for this
    /// gateway". Fire-and-forget per beat — a missed beat is harmless (the TTL tolerates
    /// several) and the loop must never stall on a slow call. Spawned from `main`.
    pub fn spawn_gateway_heartbeat(&self) {
        let coord = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(15));
            loop {
                tick.tick().await;
                // `world_shards()` is default-first; realm-core is appended when it is a
                // distinct database (unconfigured, it aliases the default handle).
                let mut shards = coord.world_shards();
                if let Ok(rc) = coord.realm_core() {
                    if !shards.iter().any(|(n, _)| n == rc.shard_name()) {
                        shards.push((rc.shard_name().to_string(), rc));
                    }
                }
                for (shard_name, shard) in shards {
                    let guard = shard.0.coord();
                    if let Err(e) = guard.conn.reducers.gw_heartbeat() {
                        log::warn!(
                            "gateway heartbeat send failed on {shard_name} (will retry next beat): {e}"
                        );
                    }
                }
            }
        });
    }

    /// Provision SRP6 credentials computed by the gateway (Phase 0 bring-up).
    pub fn provision_account(&self, username: &str, salt: &[u8], verifier: &[u8]) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "provision_account",
            provision_account_then(username.to_string(), salt.to_vec(), verifier.to_vec())
        )
    }

    /// Create a character via the `create_character` reducer (owner connection), mapping the
    /// reducer result to a game outcome. A distinguished `NAME_IN_USE` error → `NameInUse`; any
    /// other reducer/transport error → `Failed` (never propagated as a hard error, so a bad
    /// creation can't drop the world session).
    pub fn create_character(
        &self,
        account_id: u64,
        name: &str,
        race: u8,
        class: u8,
        gender: u8,
        appearance: crate::codec::Appearance,
    ) -> Result<crate::codec::CharCreateOutcome> {
        use crate::codec::CharCreateOutcome;
        // The SpacetimeDB-generated reducer binding takes the five appearance bytes positionally;
        // unbundle `Appearance` here, at the single generated-boundary call.
        let result = call_reducer!(
            self.0.call_pipe().conn.reducers,
            "create_character",
            create_character_then(
                account_id,
                name.to_string(),
                race,
                class,
                gender,
                appearance.skin,
                appearance.face,
                appearance.hair_style,
                appearance.hair_color,
                appearance.facial_hair
            )
        );
        Ok(match result {
            Ok(()) => CharCreateOutcome::Success,
            Err(e) if e.to_string().contains("NAME_IN_USE") => CharCreateOutcome::NameInUse,
            Err(e) if e.to_string().contains("SERVER_LIMIT") => CharCreateOutcome::ServerLimit,
            // The 5875 client has no code for "this database may not mint guids", so the outcome is
            // the generic failure — but the REASON must not be swallowed: the whole point of
            // guid-range licensing is that an unlicensed shard fails loudly instead of minting
            // into someone else's range.
            Err(e) => {
                log::warn!("create_character on {} failed: {e:#}", self.shard_name());
                CharCreateOutcome::Failed
            }
        })
    }

    /// Delete a character via the `delete_character` reducer (owner connection — the reducer is
    /// operator-gated, mirroring `create_character`). Ownership is enforced module-side (`NOT_OWNER`
    /// if `character_guid` isn't `account_id`'s), so a malicious/buggy client can't delete another
    /// account's character. Maps to a game outcome the same way `create_character` does: never
    /// propagated as a hard error, so a bad delete can't drop the world session.
    pub fn delete_character(
        &self,
        account_id: u64,
        character_guid: u64,
    ) -> Result<crate::codec::CharDeleteOutcome> {
        use crate::codec::CharDeleteOutcome;
        match self.character_has_auction_value(character_guid) {
            Ok(true) => return Ok(CharDeleteOutcome::Failed),
            Ok(false) => {}
            Err(error) => {
                log::warn!(
                    "delete_character: could not verify auction value for {character_guid}: {error:#}"
                );
                return Ok(CharDeleteOutcome::Failed);
            }
        }
        let result = call_reducer!(
            self.0.call_pipe().conn.reducers,
            "delete_character",
            delete_character_then(account_id, character_guid)
        );
        Ok(match result {
            Ok(()) => CharDeleteOutcome::Success,
            Err(_) => CharDeleteOutcome::Failed,
        })
    }

    fn character_has_auction_value(&self, character_guid: u64) -> Result<bool> {
        for (_, shard) in self.world_shards() {
            let guard = shard.0.coord();
            let has_hold = guard
                .conn
                .db
                .game_auction_hold()
                .iter()
                .any(|hold| hold.seller_guid == character_guid);
            if has_hold {
                return Ok(true);
            }
            let has_bid_hold = guard
                .conn
                .db
                .game_auction_bid_hold()
                .iter()
                .any(|hold| bid_hold_has_value(&hold, character_guid));
            if has_bid_hold {
                return Ok(true);
            }
        }

        let realm = self.realm_core()?;
        let guard = realm.0.coord();
        let has_auction = guard.conn.db.game_auction().iter().any(|auction| {
            auction.owner_guid == character_guid || auction.highest_bidder_guid == character_guid
        });
        Ok(has_auction)
    }

    /// Logon writes K + the bound per-account identity (Phase 1).
    pub fn establish_session(
        &self,
        account_id: u64,
        session_key: &[u8; 40],
        bound_identity: [u8; 32],
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "establish_session",
            establish_session_then(
                account_id,
                session_key.to_vec(),
                Identity::from_byte_array(bound_identity)
            )
        )
    }

    /// Publish `character_guid`'s location into this handle's character→shard index. Call it
    /// on the REALM-CORE handle: on a world shard the index is already maintained transactionally by
    /// `finish_transfer`. Operator-gated module-side (the index is a routing input).
    pub fn set_character_shard(
        &self,
        character_guid: u64,
        map_id: u32,
        instance_id: u64,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "set_character_shard",
            set_character_shard_then(character_guid, map_id, instance_id)
        )
    }

    /// Set the player's current target (`CMSG_SET_SELECTION`, Tier 2 / N3) over the coordinator
    /// connection so the module attributes it to the caller. `target_guid` 0 clears it.
    pub fn set_target(&self, _account_id: u64, actor_guid: u64, target_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("set_target: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_set_target",
            gw_set_target_then(actor_guid, target_guid)
        )
    }

    /// Validate a `CMSG_INSPECT` request (target is a real in-world player, on the caller's map, in
    /// range, friendly) over the coordinator connection so the module resolves the caller from
    /// `ctx.sender`. `Err` (out of range / hostile / no such target) → the caller ignores it.
    pub fn inspect(&self, _account_id: u64, actor_guid: u64, target_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("inspect: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_inspect",
            gw_inspect_then(actor_guid, target_guid)
        )
    }

    /// Use a gameobject (`CMSG_GAMEOBJ_USE`) — a chest rolls its loot into the corpse-loot table keyed
    /// on the GO guid, a quest-use object grants quest credit. The module gates range + type.
    /// Rides the coordinator connection as `gw_use_gameobject`.
    pub fn use_gameobject(
        &self,
        _account_id: u64,
        actor_guid: u64,
        go_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        if actor_guid == 0 {
            return Err(anyhow!("use_gameobject: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        legacy_loot_request_status(call_reducer!(
            coord.conn.reducers,
            "gw_use_gameobject",
            gw_use_gameobject_then(actor_guid, go_guid)
        ))
    }

    /// Enter an area trigger (`CMSG_AREATRIGGER`) — credit any active explore quest tied to `trigger_id`.
    pub fn enter_areatrigger(
        &self,
        _account_id: u64,
        actor_guid: u64,
        trigger_id: u32,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("enter_areatrigger: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_enter_areatrigger",
            gw_enter_areatrigger_then(actor_guid, trigger_id)
        )
    }

    /// Forward an addon-bridge command to the module's `client_command` dispatch.
    pub fn client_command(
        &self,
        _account_id: u64,
        actor_guid: u64,
        cmd: String,
        payload: String,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("client_command: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_client_command",
            gw_client_command_then(actor_guid, cmd, payload)
        )
    }

    /// Start the player's melee auto-attack on `target_guid` (`CMSG_ATTACKSWING`, combat C1) over
    /// the coordinator connection so the module attributes the swing to the caller.
    pub fn start_attack(&self, _account_id: u64, actor_guid: u64, target_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("start_attack: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_attack",
            gw_attack_then(actor_guid, target_guid)
        )
    }

    /// Relay a pet command-bar action (`CMSG_PET_ACTION`) over the coordinator connection so the module
    /// attributes it to the pet's owner. `data` is the raw packed action (flag<<24 | id); the module
    /// decodes stay/follow/attack/dismiss + passive/defensive/aggressive.
    pub fn pet_command(
        &self,
        _account_id: u64,
        actor_guid: u64,
        data: u32,
        target_guid: u64,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("pet_command: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_pet_command",
            gw_pet_command_then(actor_guid, data, target_guid)
        )
    }

    /// Start the player's RANGED auto-attack on `target_guid` with `spell_id` (75 Auto Shot / 5019 Shoot)
    /// over the coordinator connection so the module attributes the shot to the caller.
    /// Rides the coordinator connection as `gw_ranged_attack`.
    pub fn start_ranged_attack(
        &self,
        _account_id: u64,
        actor_guid: u64,
        target_guid: u64,
        spell_id: u32,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("start_ranged_attack: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_ranged_attack",
            gw_ranged_attack_then(actor_guid, target_guid, spell_id)
        )
    }

    /// Stop the player's melee auto-attack (`CMSG_ATTACKSTOP`, combat C1).
    pub fn stop_attack(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("stop_attack: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_stop_attack",
            gw_stop_attack_then(actor_guid)
        )
    }

    /// Draw or stow the player's weapons (`CMSG_SETSHEATHED`). [#101]
    pub fn set_sheathed(&self, _account_id: u64, actor_guid: u64, state: u8) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("set_sheathed: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_set_sheathed",
            gw_set_sheathed_then(actor_guid, state)
        )
    }

    /// Cast a spell (`CMSG_CAST_SPELL`, aura tracer) over the coordinator connection so the module
    /// attributes the cast to the caller. `target_guid` is the client's selected unit (0 = none/self →
    /// the module substitutes the caster), threaded so target-keyed effects see the real target.
    pub fn cast_spell(
        &self,
        _account_id: u64,
        actor_guid: u64,
        spell_id: u32,
        target_guid: u64,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("cast_spell: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_cast_spell",
            gw_cast_spell_then(actor_guid, spell_id, target_guid)
        )
    }

    /// Resolve an item-target cast through the module's generic item-effect seam.
    pub fn cast_item_target(
        &self,
        _account_id: u64,
        actor_guid: u64,
        spell_id: u32,
        slot: u8,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("cast_item_target: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_cast_item_target",
            gw_cast_item_target_then(actor_guid, spell_id, slot)
        )
    }

    /// Cast a GROUND-TARGETED spell at a clicked world point (`CMSG_CAST_SPELL` with a DEST_LOCATION —
    /// Flamestrike/Blizzard/Rain of Fire). Same per-account attribution as `cast_spell`; the `(x,y,z)` is
    /// the ground click so the module anchors the AoE/patch there.
    pub fn cast_spell_at(
        &self,
        _account_id: u64,
        actor_guid: u64,
        spell_id: u32,
        target_guid: u64,
        x: f32,
        y: f32,
        z: f32,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("cast_spell_at: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_cast_spell_at",
            gw_cast_spell_at_then(actor_guid, spell_id, target_guid, x, y, z)
        )
    }

    /// Cancel one of the caller's own auras by spell id (`CMSG_CANCEL_AURA`) over the coordinator
    /// connection so the module attributes the removal to the caller.
    pub fn cancel_aura(&self, _account_id: u64, actor_guid: u64, spell_id: u32) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("cancel_aura: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_cancel_aura",
            gw_cancel_aura_then(actor_guid, spell_id)
        )
    }

    /// Cancel the caller's in-progress cast (`CMSG_CANCEL_CAST`) over the coordinator connection so the
    /// module clears the caller's pending cast — no phantom completion GO.
    pub fn cancel_cast(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("cancel_cast: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_cancel_cast",
            gw_cancel_cast_then(actor_guid)
        )
    }

    pub fn send_chat(
        &self,
        _account_id: u64,
        actor_guid: u64,
        chat_type: u8,
        language: u8,
        message: String,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("send_chat: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_send_chat",
            gw_send_chat_then(actor_guid, chat_type, language, message)
        )
    }

    /// Join a chat channel (CMSG_JOIN_CHANNEL — the client auto-sends on zone-in).
    pub fn join_channel(&self, _account_id: u64, actor_guid: u64, channel: String) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("join_channel: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_join_channel",
            gw_join_channel_then(actor_guid, channel)
        )
    }

    /// Leave a chat channel (CMSG_LEAVE_CHANNEL).
    pub fn leave_channel(&self, _account_id: u64, actor_guid: u64, channel: String) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("leave_channel: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_leave_channel",
            gw_leave_channel_then(actor_guid, channel)
        )
    }

    /// Speak into a channel (the CMSG_MESSAGECHAT Channel arm).
    pub fn send_channel_message(
        &self,
        _account_id: u64,
        actor_guid: u64,
        channel: String,
        message: String,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("send_channel_message: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_send_channel_message",
            gw_send_channel_message_then(actor_guid, channel, message)
        )
    }

    pub fn send_emote(
        &self,
        _account_id: u64,
        actor_guid: u64,
        text_emote: u32,
        emote_anim: u32,
        target_guid: u64,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("send_emote: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_send_emote",
            gw_send_emote_then(actor_guid, text_emote, emote_anim, target_guid)
        )
    }

    pub fn send_roll(
        &self,
        _account_id: u64,
        actor_guid: u64,
        min_roll: u32,
        max_roll: u32,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("send_roll: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_send_roll",
            gw_send_roll_then(actor_guid, min_roll, max_roll)
        )
    }

    pub fn send_whisper(
        &self,
        _account_id: u64,
        actor_guid: u64,
        target_player: String,
        message: String,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("send_whisper: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_send_whisper",
            gw_send_whisper_then(actor_guid, target_player, message)
        )
    }

    /// `CMSG_MESSAGECHAT` Party (`/p`) — over the coordinator connection so the module
    /// attributes the line (and its group-membership check) to the caller.
    pub fn party_chat(&self, _account_id: u64, actor_guid: u64, message: String) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("party_chat: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_party_chat",
            gw_party_chat_then(actor_guid, message)
        )
    }

    /// GM playtest dot-command: resolve current Account authority on Realm-core, then convey it to
    /// the Home Shard through one Store operation.
    pub fn gm_command(&self, account_name: &str, actor_guid: u64, text: String) -> Result<()> {
        crate::realm_core::run_gm_command(self, account_name, actor_guid, text)
    }

    /// Send one classified command request to this Home Shard. The Module combines the conveyed
    /// Account authority with its own Character GM level and remains the final Gate.
    /// Deliberately does NOT use the `call_reducer!` macro: that macro wraps a module `Err` as
    /// `"{what} reducer failed: {e}"` (fine when a caller only substring-matches it, like `party_chat`'s
    /// `NOT_IN_GROUP` check), but the Say handler relays this `Err`'s text VERBATIM to the sender as a
    /// system chat line — a raw `"permission denied"` / `"unknown command: .foo"` must reach the client
    /// with no wrapper prefix.
    pub(crate) fn request_gm_command(
        &self,
        actor_guid: u64,
        alpha_test_tools: bool,
        text: String,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("gm_command: actor_guid unresolved"));
        }
        let (tx, rx) = std::sync::mpsc::channel::<
            std::result::Result<(), super::connection::ReducerCompletionFailure>,
        >();
        // Raw-module-message plumbing: the GM console renders the module's own rejection text
        // ("permission denied", parse errors) verbatim, no "reducer failed" wrapper.
        let coord = self.0.call_pipe();
        let completion = coord.reducer_completion.clone();
        let call_id = completion
            .register(tx)
            .map_err(|e| anyhow!("gm_command reducer transport disconnected: {e}"))?;
        let callback_completion = completion.clone();
        coord
            .conn
            .reducers
            .gw_gm_command_then(actor_guid, alpha_test_tools, text, move |_ctx, status| {
                callback_completion.finish(
                    call_id,
                    match status {
                        Ok(Ok(())) => Ok(()),
                        Ok(Err(reason)) => Err(
                            super::connection::ReducerCompletionFailure::Rejected(reason),
                        ),
                        Err(error) => {
                            Err(super::connection::ReducerCompletionFailure::Internal(error))
                        }
                    },
                );
            })
            .map_err(|e| {
                completion.cancel(call_id);
                super::connection::ReducerCallError::sdk_send("gw_gm_command", e)
            })?;
        match recv_reducer_on(rx, "gm_command", &completion, call_id) {
            Ok(()) => Ok(()),
            Err(e) if super::connection::is_reducer_refusal(&e) => Err(anyhow!(e
                .to_string()
                .trim_start_matches("gm_command reducer failed: ")
                .to_string())),
            Err(e) => Err(e),
        }
    }

    /// `CMSG_PUSHQUESTTOPARTY` — over the coordinator connection so the module
    /// attributes the sender + its grouped/on-quest gates to the caller.
    pub fn push_quest(&self, _account_id: u64, actor_guid: u64, quest_id: u32) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("push_quest: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_push_quest_to_party",
            gw_push_quest_to_party_then(actor_guid, quest_id)
        )
    }

    /// `CMSG_GROUP_INVITE` — `target_guid` is already resolved by the gateway.
    pub fn group_invite(&self, _account_id: u64, actor_guid: u64, target_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("group_invite: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_group_invite",
            gw_group_invite_then(actor_guid, target_guid)
        )
    }

    /// `CMSG_INITIATE_TRADE` — `target_guid` is the client's targeted player (#120).
    pub fn initiate_trade(
        &self,
        _account_id: u64,
        actor_guid: u64,
        target_guid: u64,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("initiate_trade: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_initiate_trade",
            gw_initiate_trade_then(actor_guid, target_guid)
        )
    }

    /// `CMSG_BEGIN_TRADE` (#120).
    pub fn begin_trade(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("begin_trade: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_begin_trade",
            gw_begin_trade_then(actor_guid)
        )
    }

    /// `CMSG_CANCEL_TRADE` (#120).
    pub fn cancel_trade(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("cancel_trade: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_cancel_trade",
            gw_cancel_trade_then(actor_guid)
        )
    }

    pub fn duel_accept(&self, _account_id: u64, actor_guid: u64, flag_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("duel_accept: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_duel_accept",
            gw_duel_accept_then(actor_guid, flag_guid)
        )
    }

    pub fn duel_cancel(&self, _account_id: u64, actor_guid: u64, flag_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("duel_cancel: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_duel_cancel",
            gw_duel_cancel_then(actor_guid, flag_guid)
        )
    }

    /// `CMSG_SET_TRADE_ITEM` (#121).
    pub fn set_trade_item(
        &self,
        _account_id: u64,
        actor_guid: u64,
        trade_slot: u8,
        inv_slot: u8,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("set_trade_item: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_set_trade_item",
            gw_set_trade_item_then(actor_guid, trade_slot, inv_slot)
        )
    }

    /// `CMSG_CLEAR_TRADE_ITEM` (#121).
    pub fn clear_trade_item(
        &self,
        _account_id: u64,
        actor_guid: u64,
        trade_slot: u8,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("clear_trade_item: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_clear_trade_item",
            gw_clear_trade_item_then(actor_guid, trade_slot)
        )
    }

    /// `CMSG_SET_TRADE_GOLD` (#121).
    pub fn set_trade_gold(&self, _account_id: u64, actor_guid: u64, copper: u32) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("set_trade_gold: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_set_trade_gold",
            gw_set_trade_gold_then(actor_guid, copper)
        )
    }

    /// `CMSG_ACCEPT_TRADE` (#122).
    pub fn accept_trade(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("accept_trade: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_accept_trade",
            gw_accept_trade_then(actor_guid)
        )
    }

    /// `CMSG_UNACCEPT_TRADE` (#122).
    pub fn unaccept_trade(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("unaccept_trade: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_unaccept_trade",
            gw_unaccept_trade_then(actor_guid)
        )
    }

    /// `CMSG_BUSY_TRADE` (#123).
    pub fn busy_trade(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("busy_trade: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_busy_trade",
            gw_busy_trade_then(actor_guid)
        )
    }

    /// `CMSG_IGNORE_TRADE` (#123).
    pub fn ignore_trade(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("ignore_trade: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_ignore_trade",
            gw_ignore_trade_then(actor_guid)
        )
    }

    /// `CMSG_GROUP_ACCEPT`. Rides the coordinator connection as
    /// `gw_accept_group_invite`.
    pub fn group_accept(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("group_accept: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_accept_group_invite",
            gw_accept_group_invite_then(actor_guid)
        )
    }

    /// `CMSG_GROUP_DECLINE`.
    pub fn group_decline(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("group_decline: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_group_decline",
            gw_group_decline_then(actor_guid)
        )
    }

    /// `CMSG_GROUP_DISBAND` — leave the caller's group.
    pub fn group_leave(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("group_leave: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_group_leave",
            gw_group_leave_then(actor_guid)
        )
    }

    /// `CMSG_GROUP_UNINVITE` — the leader kicks `target_guid`.
    pub fn group_uninvite(
        &self,
        _account_id: u64,
        actor_guid: u64,
        target_guid: u64,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("group_uninvite: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_group_uninvite",
            gw_group_uninvite_then(actor_guid, target_guid)
        )
    }

    /// `CMSG_LOOT_METHOD` — the leader sets the party's loot method/
    /// threshold/master. Echoed to every member via the existing `SMSG_GROUP_LIST` relay (the
    /// module's `group_loot_method` reducer re-renders the roster payload); no separate ack packet
    /// (vanilla sends none for this opcode either).
    pub fn group_loot_method(
        &self,
        _account_id: u64,
        actor_guid: u64,
        loot_setting: u8,
        master_guid: u64,
        loot_threshold: u8,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("group_loot_method: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_group_loot_method",
            gw_group_loot_method_then(actor_guid, loot_setting, master_guid, loot_threshold)
        )
    }

    /// `CMSG_GOSSIP_SELECT_OPTION` — the NOTIFY-ONLY module chokepoint. Fired
    /// best-effort BEFORE the gateway's own gossip behavior; a failure never blocks the reply.
    pub fn gossip_select(
        &self,
        _account_id: u64,
        actor_guid: u64,
        npc_guid: u64,
        option_id: u32,
        option_row_id: u32,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("gossip_select: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_gossip_select",
            gw_gossip_select_then(actor_guid, npc_guid, option_id, option_row_id)
        )
    }

    /// `CMSG_ADD_FRIEND` — `target_guid` is already resolved by the gateway.
    pub fn add_friend(&self, _account_id: u64, actor_guid: u64, target_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("add_friend: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_add_friend",
            gw_add_friend_then(actor_guid, target_guid)
        )
    }

    /// `CMSG_DEL_FRIEND`.
    pub fn del_friend(&self, _account_id: u64, actor_guid: u64, target_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("del_friend: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_del_friend",
            gw_del_friend_then(actor_guid, target_guid)
        )
    }

    /// `CMSG_ADD_IGNORE` — `target_guid` is already resolved by the gateway.
    pub fn add_ignore(&self, _account_id: u64, actor_guid: u64, target_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("add_ignore: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_add_ignore",
            gw_add_ignore_then(actor_guid, target_guid)
        )
    }

    /// `CMSG_DEL_IGNORE`.
    pub fn del_ignore(&self, _account_id: u64, actor_guid: u64, target_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("del_ignore: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_del_ignore",
            gw_del_ignore_then(actor_guid, target_guid)
        )
    }

    /// Take the money from a corpse (`CMSG_LOOT_MONEY`, slice 3) over the coordinator connection so
    /// the module attributes the loot to the caller (as `gw_loot_money`).
    pub fn loot_money(
        &self,
        _account_id: u64,
        actor_guid: u64,
        target_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        if actor_guid == 0 {
            return Err(anyhow!("loot_money: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        strict_loot_request_status(call_reducer!(
            coord.conn.reducers,
            "gw_loot_money",
            gw_loot_money_then(actor_guid, target_guid)
        ))
    }

    /// Authorize opening a creature corpse before the Gateway reads its loot rows.
    pub fn open_creature_loot(
        &self,
        _account_id: u64,
        actor_guid: u64,
        corpse_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        if actor_guid == 0 {
            return Err(anyhow!("open_creature_loot: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        strict_loot_request_status(call_reducer!(
            coord.conn.reducers,
            "gw_open_creature_loot",
            gw_open_creature_loot_then(actor_guid, corpse_guid)
        ))
    }

    /// Take one item from the open corpse into the backpack (`CMSG_AUTOSTORE_LOOT_ITEM`, slice 4) over
    /// the coordinator connection so the module attributes the loot to the caller. The module moves the
    /// item into a free slot + deletes the corpse-loot row (the inventory relay then shows it in the bag).
    /// Rides the coordinator connection as `gw_take_loot`.
    pub fn take_loot(
        &self,
        _account_id: u64,
        actor_guid: u64,
        corpse_guid: u64,
        loot_slot: u8,
    ) -> Result<LootWindowRequestStatus> {
        if actor_guid == 0 {
            return Err(anyhow!("take_loot: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        strict_loot_request_status(call_reducer!(
            coord.conn.reducers,
            "gw_take_loot",
            gw_take_loot_then(actor_guid, corpse_guid, loot_slot)
        ))
    }

    pub fn skin_corpse(
        &self,
        _account_id: u64,
        actor_guid: u64,
        corpse_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        if actor_guid == 0 {
            return Err(anyhow!("skin_corpse: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        legacy_loot_request_status(call_reducer!(
            coord.conn.reducers,
            "gw_skin",
            gw_skin_then(actor_guid, corpse_guid)
        ))
    }

    /// `CMSG_LOOT_ROLL` — record the caller's need/greed/pass vote on a
    /// live roll. Live votes/roll numbers relay to every eligible member via the `game_group_event`
    /// roll-kind rows (`stdb/subscriptions.rs`).
    pub fn loot_roll(
        &self,
        _account_id: u64,
        actor_guid: u64,
        corpse_guid: u64,
        loot_slot: u32,
        vote: u8,
    ) -> Result<LootActionStatus> {
        if actor_guid == 0 {
            return Err(anyhow!("loot_roll: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        loot_action_status(call_reducer!(
            coord.conn.reducers,
            "gw_loot_roll",
            gw_loot_roll_then(actor_guid, corpse_guid, loot_slot, vote)
        ))
    }

    /// `CMSG_LOOT_MASTER_GIVE` — the master looter assigns an above-
    /// threshold row to `target_guid`.
    pub fn loot_master_give(
        &self,
        _account_id: u64,
        actor_guid: u64,
        corpse_guid: u64,
        loot_slot: u8,
        target_guid: u64,
    ) -> Result<LootActionStatus> {
        if actor_guid == 0 {
            return Err(anyhow!("loot_master_give: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        loot_action_status(call_reducer!(
            coord.conn.reducers,
            "gw_loot_master_give",
            gw_loot_master_give_then(actor_guid, corpse_guid, loot_slot, target_guid)
        ))
    }

    pub fn disenchant_item(&self, _account_id: u64, actor_guid: u64, slot: u8) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("disenchant_item: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_disenchant",
            gw_disenchant_then(actor_guid, slot)
        )
    }

    pub fn enchant_item_on_slot(
        &self,
        _account_id: u64,
        actor_guid: u64,
        slot: u8,
        enchant_id: u32,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("enchant_item_on_slot: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_enchant_item",
            gw_enchant_item_then(actor_guid, slot, enchant_id)
        )
    }

    /// Buy `count` of `item_entry` from the vendor `vendor_guid` (`CMSG_BUY_ITEM`, Tier 2) over the
    /// coordinator connection so the module attributes the purchase to the caller. The module gates
    /// it on the vendor (stock + NPC flags + range) and debits the buyer's copper.
    pub fn buy_item(
        &self,
        _account_id: u64,
        actor_guid: u64,
        vendor_guid: u64,
        item_entry: u32,
        count: u32,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("buy_item: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_buy_item",
            gw_buy_item_then(actor_guid, vendor_guid, item_entry, count)
        )
    }

    /// Learn `spell_id` from trainer `trainer_guid` (`CMSG_TRAINER_BUY_SPELL`) over the coordinator
    /// connection. The module gates it (range / level / cost / not-already-known) and charges copper.
    /// A Refusal the Module tagged comes back as an outcome; anything else stays an error.
    /// Rides the coordinator connection as `gw_trainer_buy`.
    pub fn buy_trainer_spell(
        &self,
        _account_id: u64,
        actor_guid: u64,
        trainer_guid: u64,
        spell_id: u32,
    ) -> Result<crate::world::TrainerBuyOutcome> {
        if actor_guid == 0 {
            return Err(anyhow!("buy_trainer_spell: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        let result: Result<()> = call_reducer!(
            coord.conn.reducers,
            "gw_trainer_buy",
            gw_trainer_buy_then(actor_guid, trainer_guid, spell_id)
        );
        match result {
            Ok(()) => Ok(crate::world::TrainerBuyOutcome::Learned),
            Err(error) => match trainer_refusal(&error) {
                Some(refusal) => Ok(refusal.into()),
                None => Err(error),
            },
        }
    }

    /// Buy the next bank bag slot from `banker_guid` (`CMSG_BUY_BANK_SLOT`) over the coordinator
    /// connection. A refusal carries the module's `[N]` `SMSG_BUY_BANK_SLOT_RESULT` code tag.
    pub fn buy_bank_slot(&self, _account_id: u64, actor_guid: u64, banker_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("buy_bank_slot: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_buy_bank_slot",
            gw_buy_bank_slot_then(actor_guid, banker_guid)
        )
    }

    pub fn learn_talent(&self, _account_id: u64, actor_guid: u64, talent_id: u32) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("learn_talent: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_learn_talent",
            gw_learn_talent_then(actor_guid, talent_id)
        )
    }

    /// Respec at a trainer (the "I wish to unlearn my talents." gossip option, #516) — clears every
    /// learned talent for the calling player's escalating gold cost. Rides the coordinator
    /// connection as `gw_reset_talents` (#483 deleted the per-player sender path).
    pub fn reset_talents(
        &self,
        _account_id: u64,
        actor_guid: u64,
        trainer_guid: u64,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("reset_talents: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_reset_talents",
            gw_reset_talents_then(actor_guid, trainer_guid)
        )
    }

    /// Fishing cast: instant-resolve catch — the module's lenient alpha gate auto-learns the
    /// skill and grants the fish straight to the bag. Caller resolved via ctx.sender.
    pub fn fish(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("fish: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(coord.conn.reducers, "gw_fish", gw_fish_then(actor_guid))
    }

    /// Pick Lock: unlock the locked GameObject `go_guid` over the coordinator connection (so the
    /// module attributes the pick to the caller via ctx.sender). The module gates range / lock
    /// requirement / Lockpicking skill; on success it records the GO unlocked + climbs the skill.
    pub fn pick_lock(&self, _account_id: u64, actor_guid: u64, go_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("pick_lock: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_pick_lock",
            gw_pick_lock_then(actor_guid, go_guid)
        )
    }

    /// Persist one action-bar button (`CMSG_SET_ACTION_BUTTON`): upsert by (character, button);
    /// action 0 clears. Without this every bar drag was lost on relog (only creation seeds survived).
    pub fn set_action_button(
        &self,
        _account_id: u64,
        actor_guid: u64,
        button: u8,
        action: u32,
        action_type: u8,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("set_action_button: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_set_action_button",
            gw_set_action_button_then(actor_guid, button, action, action_type)
        )
    }

    /// Persist the rep pane's At-War checkbox (`CMSG_SET_FACTION_ATWAR`, 195 slice B): the wire's
    /// u16 is the client's 0..63 rep-array slot (ReputationListID — the gtker `Faction` field name
    /// lies, same as SET_FACTION_STANDING); the module reverse-resolves the faction and upserts.
    pub fn set_faction_at_war(
        &self,
        _account_id: u64,
        actor_guid: u64,
        reputation_index: u32,
        at_war: bool,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("set_faction_at_war: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_set_faction_at_war",
            gw_set_faction_at_war_then(actor_guid, reputation_index, at_war)
        )
    }

    /// Sell the item in inventory `slot` back to a vendor (`CMSG_SELL_ITEM`, Tier 2) over the
    /// coordinator connection. The gateway resolves the client's item-INSTANCE guid to the owning
    /// slot before calling (the reducer takes the slot); the module credits the seller's copper.
    /// Rides the coordinator connection as `gw_sell_item`.
    pub fn sell_item(
        &self,
        _account_id: u64,
        actor_guid: u64,
        vendor_guid: u64,
        slot: u8,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("sell_item: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_sell_item",
            gw_sell_item_then(actor_guid, vendor_guid, slot)
        )
    }

    pub fn buyback_item(
        &self,
        _account_id: u64,
        actor_guid: u64,
        vendor_guid: u64,
        slot: u8,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("buyback_item: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_buyback_item",
            gw_buyback_item_then(actor_guid, vendor_guid, slot)
        )
    }

    /// Repair the item in inventory `slot` at REPAIR-NPC `npc_guid` (`CMSG_REPAIR_ITEM`) over the
    /// coordinator connection. The module gates the NPC + charges copper; the player's item +
    /// purse replicate back via subscription.
    pub fn repair_item(
        &self,
        _account_id: u64,
        actor_guid: u64,
        npc_guid: u64,
        slot: u8,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("repair_item: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_repair_item",
            gw_repair_item_then(actor_guid, npc_guid, slot)
        )
    }

    /// Equip the item in main-inventory `from_slot` (`CMSG_AUTOEQUIP_ITEM`) over the coordinator
    /// connection. The module resolves the matching equipment slot and gates the required level.
    /// Rides the coordinator connection as `gw_equip_item`.
    pub fn equip_item(
        &self,
        _account_id: u64,
        actor_guid: u64,
        from_slot: u8,
    ) -> Result<ItemActionResult> {
        let Some(actor_guid) = resolved_item_actor("equip_item", actor_guid) else {
            return Ok(ItemRefusal::Internal.into());
        };
        let coord = self.0.call_pipe();
        item_action(call_reducer!(
            coord.conn.reducers,
            "gw_equip_item",
            gw_equip_item_then(actor_guid, from_slot)
        ))
    }

    /// Unequip the item in equipment `from_slot` to a free backpack slot (`CMSG_AUTOSTORE_BAG_ITEM`)
    /// over the coordinator connection. The module gates "is equipped" + "backpack has room".
    pub fn unequip_item(
        &self,
        _account_id: u64,
        actor_guid: u64,
        from_slot: u8,
    ) -> Result<ItemActionResult> {
        let Some(actor_guid) = resolved_item_actor("unequip_item", actor_guid) else {
            return Ok(ItemRefusal::Internal.into());
        };
        let coord = self.0.call_pipe();
        item_action(call_reducer!(
            coord.conn.reducers,
            "gw_unequip_item",
            gw_unequip_item_then(actor_guid, from_slot)
        ))
    }

    /// Use the consumable in main-inventory `slot` (`CMSG_USE_ITEM`) over the coordinator connection —
    /// eat/drink/potion/bandage. The module applies the on-use effect (flat heal for slice food) and
    /// decrements the stack; a gameplay `Err` (no item / not usable) is per-action.
    pub fn use_item(
        &self,
        _account_id: u64,
        actor_guid: u64,
        slot: u8,
    ) -> Result<ItemActionResult> {
        let Some(actor_guid) = resolved_item_actor("use_item", actor_guid) else {
            return Ok(ItemRefusal::Internal.into());
        };
        let coord = self.0.call_pipe();
        item_action(call_reducer!(
            coord.conn.reducers,
            "gw_use_item",
            gw_use_item_then(actor_guid, slot)
        ))
    }

    /// Bind the caller's hearthstone home to their current position (`CMSG_GOSSIP_SELECT_OPTION` on an
    /// innkeeper's "Make this inn your home.") over the coordinator connection so the module attributes
    /// it to the caller's entity. No args — `bind_home` resolves the caller via `ctx.sender`.
    pub fn bind_home(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("bind_home: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_bind_home",
            gw_bind_home_then(actor_guid)
        )
    }

    /// Move (or swap) main-inventory `from_slot` → `to_slot` (`CMSG_SWAP_INV_ITEM`/`CMSG_SWAP_ITEM`)
    /// over the coordinator connection. The module's move primitive validates equip-slot transitions.
    pub fn move_item(
        &self,
        _account_id: u64,
        actor_guid: u64,
        from_slot: u8,
        to_slot: u8,
    ) -> Result<ItemActionResult> {
        let Some(actor_guid) = resolved_item_actor("move_item", actor_guid) else {
            return Ok(ItemRefusal::Internal.into());
        };
        let coord = self.0.call_pipe();
        item_action(call_reducer!(
            coord.conn.reducers,
            "gw_move_item",
            gw_move_item_then(actor_guid, from_slot, to_slot)
        ))
    }

    /// Auto-bank/auto-store-bank the item in `slot` (`CMSG_AUTOBANK_ITEM`/`CMSG_AUTOSTORE_BANK_ITEM`)
    /// over the coordinator connection. The module infers deposit vs. withdraw from `slot` and
    /// resolves the receiving free slot itself.
    pub fn auto_bank_item(&self, _account_id: u64, actor_guid: u64, slot: u8) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("auto_bank_item: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_auto_bank_item",
            gw_auto_bank_item_then(actor_guid, slot)
        )
    }

    /// Accept quest `quest_id` from giver `giver_guid` (`CMSG_QUESTGIVER_ACCEPT_QUEST`) over the
    /// coordinator connection so the module attributes it to the caller. The module gates the accept
    /// (giver relation + range + level + not-already-held); a gameplay `Err` is per-action, not fatal.
    /// Rides the coordinator connection as `gw_accept_quest`.
    pub fn accept_quest(
        &self,
        _account_id: u64,
        actor_guid: u64,
        giver_guid: u64,
        quest_id: u32,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("accept_quest: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_accept_quest",
            gw_accept_quest_then(actor_guid, giver_guid, quest_id)
        )
    }

    /// Turn quest `quest_id` in to giver `giver_guid` (`CMSG_QUESTGIVER_CHOOSE_REWARD`). The module
    /// validates completion + grants the rewards (money/XP/items). `reward_index` is the player's
    /// pick-1-of-N choice slot; ignored when the quest has no choices. This call uses the subscribed
    /// visibility pipe so committed item relays are queued before success presentation is allowed.
    pub fn turn_in_quest(
        &self,
        _account_id: u64,
        actor_guid: u64,
        giver_guid: u64,
        quest_id: u32,
        reward_index: u32,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("turn_in_quest: actor_guid unresolved"));
        }
        let coord = self.0.visibility_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_turn_in_quest",
            gw_turn_in_quest_then(actor_guid, giver_guid, quest_id, reward_index)
        )
    }

    /// Abandon quest `quest_id` (`CMSG_QUESTLOG_REMOVE_QUEST`) over the coordinator connection. The
    /// module deletes the player's quest-log row; the quest-log relay then clears the slot.
    pub fn abandon_quest(&self, _account_id: u64, actor_guid: u64, quest_id: u32) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("abandon_quest: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_abandon_quest",
            gw_abandon_quest_then(actor_guid, quest_id)
        )
    }

    /// Revive the caller after death (`CMSG_REPOP_REQUEST`, slice 4) over the coordinator connection.
    /// Rides the coordinator connection as `gw_repop`.
    pub fn repop(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("repop: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(coord.conn.reducers, "gw_repop", gw_repop_then(actor_guid))
    }

    /// Reclaim the caller's corpse (`CMSG_RECLAIM_CORPSE`, slice 5) over the coordinator connection.
    pub fn reclaim_corpse(
        &self,
        _account_id: u64,
        actor_guid: u64,
        corpse_guid: u64,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("reclaim_corpse: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_reclaim_corpse",
            gw_reclaim_corpse_then(actor_guid, corpse_guid)
        )
    }

    /// Answer a pending resurrect offer (`CMSG_RESURRECT_RESPONSE`) over the coordinator connection.
    /// Rides the coordinator connection as `gw_respond_resurrect`.
    pub fn resurrect_response(
        &self,
        _account_id: u64,
        actor_guid: u64,
        accept: bool,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("resurrect_response: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_respond_resurrect",
            gw_respond_resurrect_then(actor_guid, accept)
        )
    }

    /// Spirit-Healer resurrect (`CMSG_SPIRIT_HEALER_ACTIVATE`) over the coordinator connection: the
    /// module res's the caller in place at 50% + applies Resurrection Sickness if it's a ghost.
    /// `gw_spirit_res` takes no `healer_guid`.
    pub fn spirit_healer_res(
        &self,
        _account_id: u64,
        actor_guid: u64,
        _healer_guid: u64,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("spirit_healer_res: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_spirit_res",
            gw_spirit_res_then(actor_guid)
        )
    }

    /// Explicit logout (Phase 7): call the `logout` reducer over the coordinator connection so the
    /// module removes the live `game_world_entity` row. That delete fires every in-range observer's
    /// `game_world_entity` on_delete → `SMSG_DESTROY_OBJECT`, so the peer vanishes. Required because
    /// the player's SDK connection is cached/reused and does NOT drop when the game client's TCP
    /// socket closes (so the module's `on_disconnect` would not otherwise fire).
    pub fn logout(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("logout: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_leave_world",
            gw_leave_world_then(actor_guid)
        )
    }

    // -------------------------------------------------------------------------------------
    // Cross-database transfer. ALL of these are operator-gated orchestration (`require_operator`),
    // and the
    // destination shard has no bound player identity until the character has arrived on it — which
    // is precisely what they exist to make happen.
    // -------------------------------------------------------------------------------------

    /// `begin_transfer` — freeze the character, serialize it (row + every manifest table's rows),
    /// and delete its live entity, in ONE transaction. Idempotent on `transfer_id`.
    pub fn begin_transfer(&self, plan: &crate::world::transfer::TransferPlan) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "begin_transfer",
            begin_transfer_then(
                plan.transfer_id,
                plan.character_guid,
                plan.dest_map_id,
                plan.dest_instance_id,
                plan.dest_x,
                plan.dest_y,
                plan.dest_z,
                plan.dest_o,
                true, // cross_database — this wrapper only ever drives a two-database move
            )
        )
    }

    /// `import_character_blob` — materialise the arrival copy at the destination from the blob the
    /// gateway carried. Idempotent on `transfer_id`.
    pub fn import_character_blob(&self, transfer_id: u64, blob: &[u8]) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "import_character_blob",
            import_character_blob_then(transfer_id, blob.to_vec())
        )
    }

    /// `confirm_import` — attest ON THE SOURCE that the destination copy committed. Called only
    /// after `import_character_blob` returned Ok; see `world::transfer::run_transfer`.
    pub fn confirm_import(&self, transfer_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "confirm_import",
            confirm_import_then(transfer_id)
        )
    }

    /// `finish_transfer` — delete-last: destroy the source copy and clear the escrow.
    pub fn finish_transfer(&self, transfer_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "finish_transfer",
            finish_transfer_then(transfer_id)
        )
    }

    /// `release_transfer` — drop the arrival copy's fence at the destination.
    pub fn release_transfer(&self, transfer_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "release_transfer",
            release_transfer_then(transfer_id)
        )
    }

    /// `ensure_instance` — mirror an instance id onto this shard (idempotent), spawning its
    /// population the first time.
    pub fn ensure_instance(&self, instance_id: u64, map_id: u32, party_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "ensure_instance",
            ensure_instance_then(instance_id, map_id, party_id)
        )
    }

    /// `evict_instance_population` — stop this shard ticking an instance whose run moved elsewhere.
    pub fn evict_instance_population(&self, instance_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "evict_instance_population",
            evict_instance_population_then(instance_id)
        )
    }

    /// `record_shard_load` — fired against THIS handle's connection. Callers hold the
    /// **realm-core** handle: `game_shard_load` is only ever read from there.
    pub fn record_shard_load(
        &self,
        shard: &str,
        writer_occupancy_pct: f32,
        sessions: u32,
        gateway_key: u64,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "record_shard_load",
            record_shard_load_then(
                shard.to_string(),
                writer_occupancy_pct,
                sessions,
                gateway_key
            )
        )
    }

    /// `realm_group_op` — one party op against the database THIS handle points at. The gateway
    /// calls it on the **realm-core** handle, where membership is authoritative.
    ///
    /// Through the COORDINATOR connection, not the player's: the reducer is operator-gated because
    /// it takes the acting character's guid as an argument (realm-core has no live entity to derive
    /// one from), so only the token that holds the operator identity may call it. The guid passed is
    /// the one this socket authenticated into the world with — see `world::party`.
    pub fn realm_group_op(
        &self,
        op: u8,
        actor_guid: u64,
        target_guid: u64,
        arg_a: u8,
        arg_b: u8,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_group_op",
            realm_group_op_then(op, actor_guid, target_guid, arg_a, arg_b)
        )
    }

    /// `realm_whisper` — deliver one whisper against the database THIS handle points at. The
    /// gateway calls it on the **realm-core** handle, the only database that can
    /// address both parties of a cross-shard whisper (a guid is realm-wide; an identity is not).
    ///
    /// Through the COORDINATOR connection, not the player's: the reducer is operator-gated because it
    /// takes the SENDING character's guid as an argument (realm-core has no live entity to derive one
    /// from), so only the token that holds the operator identity may call it. The guid passed is the
    /// one this socket authenticated into the world with — see `world::whisper`. `sender_is_ignored`
    /// is the target's ignore-list verdict, read from the shard that holds the target's contact rows.
    pub fn realm_whisper(
        &self,
        sender_guid: u64,
        target_guid: u64,
        message: String,
        sender_is_ignored: bool,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_whisper",
            realm_whisper_then(sender_guid, target_guid, message, sender_is_ignored)
        )
    }

    /// `realm_mail_mark_read` — flip a mail's read state against the database THIS handle points
    /// at. Same trust shape as `realm_whisper` above: operator-gated, `recipient_guid` passed
    /// explicitly (the plane may hold no live entity), called on whichever handle `world::mail`
    /// picked — realm-core when sharded, this shard's own database when not.
    pub fn mail_mark_read(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_mark_read",
            realm_mail_mark_read_then(recipient_guid, mail_id)
        )
    }

    /// `realm_mail_delete` — [`mail_mark_read`](Self::mail_mark_read)'s twin for delete.
    pub fn mail_delete(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_delete",
            realm_mail_delete_then(recipient_guid, mail_id)
        )
    }

    /// `realm_mail_return` — [`mail_delete`](Self::mail_delete)'s twin for return-to-sender: the row
    /// is re-addressed in place, on the database THIS handle points at. No sharded variant — the row
    /// never leaves the plane that already holds it.
    pub fn mail_return(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_return",
            realm_mail_return_then(recipient_guid, mail_id)
        )
    }

    /// `realm_mail_send` — write one sent letter against the database THIS handle points at, and
    /// charge the sender for it in the same transaction. Same trust shape as the two above, and the
    /// guid it carries is the one the socket authenticated: every gate deciding who may write to
    /// whom ran in `world::mail`, because realm-core can answer none of them.
    ///
    /// **The single-database plane only.** A sharded realm cannot have that one transaction and
    /// drives the escrow below instead.
    #[allow(clippy::too_many_arguments)]
    pub fn mail_send(
        &self,
        sender_guid: u64,
        recipient_guid: u64,
        subject: String,
        body: String,
        money: u32,
        cod: u32,
        item_guid: u64,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_send",
            realm_mail_send_then(
                sender_guid,
                recipient_guid,
                subject,
                body,
                money,
                cod,
                item_guid
            )
        )
    }

    /// `realm_mail_take_money` — credit a mail's copper to the recipient and empty the row, in one
    /// transaction. The single-database plane's whole take, for the same reason.
    pub fn mail_take_money(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_take_money",
            realm_mail_take_money_then(recipient_guid, mail_id)
        )
    }

    /// `realm_mail_take_item` — re-create a mail's attachment in the recipient's bags and empty the
    /// row, in one transaction. The single-database plane's whole item take.
    pub fn mail_take_item(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_take_item",
            realm_mail_take_item_then(recipient_guid, mail_id)
        )
    }

    /// `realm_mail_item_room` — the bag-space probe a sharded item take runs BEFORE it fences, on
    /// the taker's own handle. A read dressed as a reducer, because only the module can answer it
    /// without a second copy of the bag search.
    pub fn mail_item_room(&self, payee_guid: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_item_room",
            realm_mail_item_room_then(payee_guid)
        )
    }

    /// `realm_mail_fence` — step 1 of a sharded SEND, on the SENDER's own handle: the postage plus
    /// the attached coin leave the purse into an escrow row keyed by the caller-chosen `escrow_id`.
    #[allow(clippy::too_many_arguments)]
    pub fn mail_fence(
        &self,
        escrow_id: u64,
        sender_guid: u64,
        recipient_guid: u64,
        subject: String,
        body: String,
        money: u32,
        postage: u32,
        item_guid: u64,
        cod: u32,
        cod_source_mail_id: u64,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_fence",
            realm_mail_fence_then(
                escrow_id,
                sender_guid,
                recipient_guid,
                subject,
                body,
                money,
                postage,
                item_guid,
                cod,
                cod_source_mail_id
            )
        )
    }

    /// `realm_mail_commit` — step 2 of a sharded send, on the REALM handle: the mail row plus a
    /// receipt under the same `escrow_id`, so a replay writes one letter and not two.
    #[allow(clippy::too_many_arguments)]
    pub fn mail_commit(
        &self,
        escrow_id: u64,
        sender_guid: u64,
        recipient_guid: u64,
        subject: String,
        body: String,
        money: u32,
        item: crate::world::mail::AttachedItem,
        cod: u32,
        cod_source_mail_id: u64,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_commit",
            realm_mail_commit_then(
                escrow_id,
                sender_guid,
                recipient_guid,
                subject,
                body,
                money,
                item.entry,
                item.stack_count,
                item.durability,
                item.enchant_id,
                item.soulbound,
                cod,
                cod_source_mail_id
            )
        )
    }

    /// `realm_mail_take_money_fence` — step 1 of a sharded TAKE, on the handle that OWNS THE MAIL
    /// ROW: the copper leaves the row into an escrow there. The mirror of `mail_fence`.
    pub fn mail_take_money_fence(
        &self,
        escrow_id: u64,
        payee_guid: u64,
        mail_id: u64,
        expect_money: u32,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_take_money_fence",
            realm_mail_take_money_fence_then(escrow_id, payee_guid, mail_id, expect_money)
        )
    }

    /// `realm_mail_payout` — step 2 of a sharded take, on the TAKER's own handle: the purse plus a
    /// receipt under the same `escrow_id`. `mail_commit`'s twin.
    pub fn mail_payout(
        &self,
        escrow_id: u64,
        payee_guid: u64,
        mail_id: u64,
        amount: u32,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_payout",
            realm_mail_payout_then(escrow_id, payee_guid, mail_id, amount)
        )
    }

    /// `realm_mail_take_item_fence` — step 1 of a sharded ITEM take, on the handle that OWNS THE
    /// MAIL ROW: the attachment leaves the row into an escrow there.
    pub fn mail_take_item_fence(
        &self,
        escrow_id: u64,
        payee_guid: u64,
        mail_id: u64,
        expect_entry: u32,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_take_item_fence",
            realm_mail_take_item_fence_then(escrow_id, payee_guid, mail_id, expect_entry)
        )
    }

    /// `realm_mail_item_payout` — step 2 of a sharded item take, on the TAKER's own handle: the
    /// item plus a receipt under the same `escrow_id`. `mail_payout`'s twin.
    pub fn mail_item_payout(
        &self,
        escrow_id: u64,
        payee_guid: u64,
        mail_id: u64,
        item: crate::world::mail::AttachedItem,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_item_payout",
            realm_mail_item_payout_then(
                escrow_id,
                payee_guid,
                mail_id,
                item.entry,
                item.stack_count,
                item.durability,
                item.enchant_id,
                item.soulbound
            )
        )
    }

    /// `realm_mail_confirm_delivery` — step 3, on the handle that HOLDS THE FENCE. The attestation
    /// that the other database committed, and the only thing that licenses the settle.
    pub fn mail_confirm_delivery(&self, escrow_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_confirm_delivery",
            realm_mail_confirm_delivery_then(escrow_id)
        )
    }

    /// `realm_mail_settle` — step 4, on the handle that holds the fence: destroy it. Delete-last.
    pub fn mail_settle(&self, escrow_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_settle",
            realm_mail_settle_then(escrow_id)
        )
    }

    /// `sync_group_mirror` — replace THIS shard's mirror of one party with realm-core's roster.
    /// Operator-gated, coordinator connection, same reasoning as above; called
    /// on each WORLD shard after a party op and at world entry.
    pub fn sync_group_mirror(&self, roster: &crate::world::party::GroupRoster) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "sync_group_mirror",
            sync_group_mirror_then(
                roster.group_id,
                roster.leader_guid,
                roster.loot_method,
                roster.loot_threshold,
                roster.master_looter_guid,
                roster.members.clone(),
            )
        )
    }

    /// `realm_loot_op` — one loot-roll op against the database THIS handle points at. The
    /// gateway calls it on the **realm-core** handle: START promotes a world shard's staging roll,
    /// VOTE casts `CMSG_LOOT_ROLL`'s vote.
    ///
    /// Through the COORDINATOR connection, not the player's: the reducer is operator-gated because
    /// it acts on realm-core, which has no live entity to derive an actor from. VOTE's `actor_guid`
    /// is the guid this socket authenticated into the world with (`InWorld::self_guid`), never a
    /// literal a client supplies; START's `recipients` are the spatial snapshot a world shard already
    /// computed at kill time.
    #[allow(clippy::too_many_arguments)]
    pub fn realm_loot_op(
        &self,
        op: u8,
        corpse_guid: u64,
        slot: u8,
        item_entry: u32,
        actor_guid: u64,
        vote: u8,
        deadline_micros: i64,
        recipients: Vec<u64>,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_loot_op",
            realm_loot_op_then(
                op,
                corpse_guid,
                slot,
                item_entry,
                actor_guid,
                vote,
                deadline_micros,
                recipients
            )
        )
    }

    /// Cast a realm-core Loot Roll vote and classify only a known typed Refusal as an answer.
    pub fn realm_loot_vote(
        &self,
        corpse_guid: u64,
        slot: u8,
        actor_guid: u64,
        vote: u8,
    ) -> Result<LootActionStatus> {
        loot_action_status(call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_loot_op",
            realm_loot_op_then(
                lyracore_shared::loot_roll::loot_op::VOTE,
                corpse_guid,
                slot,
                0,
                actor_guid,
                vote,
                0,
                Vec::new()
            )
        ))
    }

    /// `settle_loot_roll` — grant a resolved roll's item on THIS world shard, if it holds the
    /// matching corpse row. Operator-gated, coordinator connection; the loot-roll relay calls
    /// it on every connected world shard after observing realm-core's `ROLL_WON` event — the
    /// module's own `withheld` guard makes a wrong-shard call a harmless no-op.
    pub fn settle_loot_roll(&self, corpse_guid: u64, slot: u8, winner_guid: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "settle_loot_roll",
            settle_loot_roll_then(corpse_guid, slot, winner_guid)
        )
    }

    /// `clear_promoted_loot_roll` — delete a staging roll's rows on THIS world shard, once the
    /// loot-roll relay has promoted it onto realm-core. Operator-gated, coordinator connection.
    pub fn clear_promoted_loot_roll(&self, roll_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "clear_promoted_loot_roll",
            clear_promoted_loot_roll_then(roll_id)
        )
    }
}

#[cfg(test)]
mod visibility_receipt_tests {
    #[test]
    fn quest_completion_waits_for_the_subscribed_coordinator_receipt() {
        let source = include_str!("reducers.rs");
        let turn_in = crate::test_scan::code_of(source, "pub fn turn_in_quest(");
        let visibility_pipe = crate::test_scan::code_of(
            include_str!("connection.rs"),
            "pub(crate) fn visibility_pipe(",
        );

        assert!(
            turn_in.contains("self.0.visibility_pipe()"),
            "a successful turn-in may authorize QUEST_COMPLETE only after the coordinator has \
             applied the reward transaction and queued its inventory relays"
        );
        assert!(
            !turn_in.contains("self.0.call_pipe()"),
            "a reducer-only call pipe cannot receipt coordinator subscription visibility"
        );
        assert!(
            visibility_pipe.contains("self.coord()")
                && !visibility_pipe.contains("self.call_pipe()"),
            "the visibility pipe must be the connection that owns the relayed subscriptions"
        );
    }
}

#[cfg(test)]
mod taxi_reply_tests {
    use super::*;

    fn reply(
        request_id: u64,
        character_guid: u64,
        npc_guid: u64,
        operation: u8,
    ) -> TaxiServiceReply {
        TaxiServiceReply {
            request_id,
            character_guid,
            operation,
            npc_guid,
            accepted: true,
            known: false,
            source_client_node_id: 0,
            available_client_node_ids: Vec::new(),
            refusal: String::new(),
            created_micros: 0,
            result_code: lyracore_shared::constants::taxi_protocol::ACTIVATE_OK,
        }
    }

    #[test]
    fn reply_selection_rejects_stale_character_operation_and_npc_rows() {
        let current = reply(
            22,
            7,
            90,
            lyracore_shared::constants::taxi_protocol::REPLY_OPEN,
        );
        assert!(taxi_reply_matches(
            &current,
            7,
            90,
            lyracore_shared::constants::taxi_protocol::REPLY_OPEN,
        ));
        assert!(!taxi_reply_matches(
            &current,
            8,
            90,
            lyracore_shared::constants::taxi_protocol::REPLY_OPEN,
        ));
        assert!(!taxi_reply_matches(
            &current,
            7,
            91,
            lyracore_shared::constants::taxi_protocol::REPLY_OPEN,
        ));
        assert!(!taxi_reply_matches(
            &current,
            7,
            90,
            lyracore_shared::constants::taxi_protocol::REPLY_STATUS,
        ));
    }

    #[test]
    fn reply_wait_uses_the_unique_request_id_accessor() {
        let source = include_str!("reducers.rs");
        assert!(source.contains(".request_id()\n                .find(&request_id)"));
        assert!(!source.contains(".character_guid()\n                .find(&character_guid)"));
        let wait = source
            .split("fn await_taxi_reply(")
            .nth(1)
            .and_then(|tail| tail.split("pub fn taxi_node_status(").next())
            .expect("taxi reply wait body");
        let observes = wait.find("taxi_reply_matches(").expect("validated reply");
        let acknowledges = wait
            .find("gw_ack_taxi_reply_then(character_guid, request_id)")
            .expect("reply acknowledgement");
        assert!(observes < acknowledges);
    }
}

fn wait_for_auction_cache_row<T>(
    operation_id: u64,
    row_name: &str,
    mut read: impl FnMut() -> Option<T>,
) -> Result<T> {
    for _ in 0..100 {
        if let Some(row) = read() {
            return Ok(row);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(anyhow!(
        "auction {row_name} {operation_id} committed but is not visible in the coordinator cache"
    ))
}

trait AuctionRequestFields {
    fn actor_guid(&self) -> u64;
    fn item_guid(&self) -> u64;
    fn start_bid(&self) -> u32;
    fn buyout(&self) -> u32;
    fn duration_minutes(&self) -> u32;
    fn house_id(&self) -> u32;
}

impl AuctionRequestFields for AuctionHold {
    fn actor_guid(&self) -> u64 {
        self.seller_guid
    }
    fn item_guid(&self) -> u64 {
        self.item_guid
    }
    fn start_bid(&self) -> u32 {
        self.start_bid
    }
    fn buyout(&self) -> u32 {
        self.buyout
    }
    fn duration_minutes(&self) -> u32 {
        self.duration_minutes
    }
    fn house_id(&self) -> u32 {
        self.house
    }
}

impl AuctionRequestFields for AuctionOperationReceipt {
    fn actor_guid(&self) -> u64 {
        self.actor_guid
    }
    fn item_guid(&self) -> u64 {
        self.item_guid
    }
    fn start_bid(&self) -> u32 {
        self.start_bid
    }
    fn buyout(&self) -> u32 {
        self.buyout
    }
    fn duration_minutes(&self) -> u32 {
        self.duration_minutes
    }
    fn house_id(&self) -> u32 {
        self.house
    }
}

fn same_auction_request(
    row: &impl AuctionRequestFields,
    request: crate::world::CreateAuctionRequest,
) -> bool {
    row.actor_guid() == request.actor_guid
        && row.item_guid() == request.item_guid
        && row.start_bid() == request.start_bid
        && row.buyout() == request.buyout
        && row.duration_minutes() == request.duration_minutes
        && row.house_id() == request.house_id
}

fn next_auction_operation_id() -> Result<u64> {
    loop {
        let mut bytes = [0; 8];
        getrandom::fill(&mut bytes)
            .map_err(|error| anyhow!("OS randomness unavailable: {error}"))?;
        let operation_id = u64::from_le_bytes(bytes);
        if operation_id != 0 {
            return Ok(operation_id);
        }
    }
}

#[derive(Clone, Copy)]
enum UntaggedLootRejection {
    Fatal,
    LegacyUnanswered,
}

/// Classify a Durable Request from a core whose gameplay refusals all have loot tags.
fn strict_loot_request_status(result: Result<()>) -> Result<LootWindowRequestStatus> {
    loot_request_status(result, UntaggedLootRejection::Fatal)
}

/// Preserve silent gameplay refusals from the legacy GameObject and skinning cores. Boundary
/// failures and every tagged result remain explicit, so this compatibility cannot hide them.
fn legacy_loot_request_status(result: Result<()>) -> Result<LootWindowRequestStatus> {
    loot_request_status(result, UntaggedLootRejection::LegacyUnanswered)
}

fn loot_request_status(
    result: Result<()>,
    untagged: UntaggedLootRejection,
) -> Result<LootWindowRequestStatus> {
    match result {
        Ok(()) => Ok(LootWindowRequestStatus::Applied),
        Err(error) => match reducer_refusal_reason(&error) {
            Some(reason) => {
                if LootBoundaryFailure::parse_tag(reason).is_some() {
                    return Err(error);
                }
                if let Some(refusal) = LootRefusal::parse_tag(reason) {
                    log::debug!("stdb: loot Durable Request refused: {error:#}");
                    return Ok(LootWindowRequestStatus::Refused(refusal.into()));
                }
                if reason.starts_with("loot:") {
                    return Err(error);
                }
                match untagged {
                    UntaggedLootRejection::LegacyUnanswered => {
                        log::debug!("stdb: legacy loot Durable Request refused: {error:#}");
                        Ok(LootWindowRequestStatus::Refused(
                            LootWindowRefusal::Unanswered,
                        ))
                    }
                    UntaggedLootRejection::Fatal => Err(error),
                }
            }
            None => Err(error),
        },
    }
}

/// The typed answer for Loot Roll and master-loot requests. An unknown rejection or transport
/// failure stays an error with an unknown durable result.
fn loot_action_status(result: Result<()>) -> Result<LootActionStatus> {
    match result {
        Ok(()) => Ok(LootActionStatus::Applied),
        Err(error) => match reducer_refusal_reason(&error).and_then(LootRefusal::parse_tag) {
            Some(refusal) => {
                log::debug!("stdb: loot action refused: {error:#}");
                Ok(LootActionStatus::Refused(refusal))
            }
            None => Err(error),
        },
    }
}

/// The Module's typed auction Refusal. Only a reducer the Module rejected carries a tag; a timeout,
/// transport, or SDK failure stays an error with an unknown outcome.
fn auction_refusal(error: &anyhow::Error) -> Option<AuctionRefusal> {
    reducer_refusal_reason(error).and_then(AuctionRefusal::parse_tag)
}

/// The Module's typed trainer Refusal. Only a reducer the Module rejected carries a tag; a timeout,
/// transport, or SDK failure stays an error with an unknown outcome.
fn trainer_refusal(error: &anyhow::Error) -> Option<TrainerRefusal> {
    reducer_refusal_reason(error).and_then(TrainerRefusal::parse_tag)
}

/// The Module's typed item Refusal, on the same rule as the auction family: only a reducer the
/// Module rejected carries a tag, so a timeout or transport failure keeps its unknown outcome.
fn item_action(result: Result<()>) -> Result<ItemActionResult> {
    match result {
        Ok(()) => Ok(ItemActionResult::Done),
        Err(error) => match reducer_refusal_reason(&error).and_then(ItemRefusal::parse_tag) {
            Some(refusal) => Ok(refusal.into()),
            None => Err(error),
        },
    }
}

/// An item action needs the caller's own entity. Without one there is nothing to request, so the
/// client gets a Refusal rather than a dead session.
fn resolved_item_actor(operation: &str, actor_guid: u64) -> Option<u64> {
    if actor_guid == 0 {
        log::warn!("stdb: {operation} has no resolved actor");
        return None;
    }
    Some(actor_guid)
}

fn bid_payload_matches(hold: &AuctionBidHold, decision: &AuctionBidDecision) -> bool {
    hold.operation_id == decision.operation_id
        && hold.bidder_guid == decision.bidder_guid
        && hold.auction_id == decision.auction_id
        && hold.house == decision.house
        && hold.offer == decision.offer
}

fn bid_refund_is_recorded(hold: &AuctionBidHold, decision: &AuctionBidDecision) -> bool {
    hold.deferred_refund != 0
        && bid_payload_matches(hold, decision)
        && hold.deferred_refund == decision.deferred_refund
}

fn bid_hold_has_value(hold: &AuctionBidHold, bidder_guid: u64) -> bool {
    hold.bidder_guid == bidder_guid
        && (hold.outcome == lyracore_shared::auction::bid_outcome::PENDING
            || hold.deferred_refund != 0)
}

fn bid_outcome(hold: &AuctionBidHold) -> Result<crate::world::PlaceBidOutcome> {
    use crate::world::PlaceBidOutcome;
    use lyracore_shared::auction::bid_outcome;
    Ok(match hold.outcome {
        bid_outcome::ACCEPTED => PlaceBidOutcome::Accepted {
            minimum_increment: lyracore_shared::auction::bid_increment(
                if hold.accepted_price == 0 {
                    hold.offer
                } else {
                    hold.accepted_price
                },
            ),
        },
        bid_outcome::ITEM_NOT_FOUND => PlaceBidOutcome::ItemNotFound,
        bid_outcome::HIGHER_BID => PlaceBidOutcome::HigherBid {
            bidder_guid: hold.result_bidder_guid,
            current_bid: hold.result_bid,
            minimum_increment: hold.minimum_increment,
        },
        bid_outcome::BID_INCREMENT => PlaceBidOutcome::BidIncrement,
        bid_outcome::BID_OWN => PlaceBidOutcome::BidOwn,
        bid_outcome::DATABASE => PlaceBidOutcome::Database,
        outcome => {
            return Err(anyhow!(
                "auction bid Hold has non-terminal outcome {outcome}"
            ))
        }
    })
}

#[cfg(test)]
mod item_reducer_tests {
    use super::*;
    use crate::stdb::connection::ReducerCallError;

    #[test]
    fn only_a_rejected_reducer_carries_a_typed_refusal() {
        for refusal in ItemRefusal::ALL {
            let rejected = Err(anyhow::Error::from(ReducerCallError::Rejected {
                operation: "gw_move_item".to_string(),
                reason: refusal.as_tag().to_string(),
            })
            .context("move phase"));
            assert_eq!(
                item_action(rejected).unwrap(),
                ItemActionResult::Refused(refusal)
            );
        }

        assert_eq!(item_action(Ok(())).unwrap(), ItemActionResult::Done);

        let not_refusals = [
            anyhow::Error::from(ReducerCallError::fatal(
                "gw_move_item reducer timed out after 10s".to_string(),
            )),
            anyhow::Error::from(ReducerCallError::fatal(
                "gw_use_item reducer failed: transport disconnected".to_string(),
            )),
            anyhow::Error::from(ReducerCallError::Rejected {
                operation: "gw_equip_item".to_string(),
                reason: "operator only".to_string(),
            }),
            anyhow!(
                "wrapped text that mentions {}",
                ItemRefusal::Internal.as_tag()
            ),
        ];
        for error in not_refusals {
            let text = format!("{error:#}");
            assert!(item_action(Err(error)).is_err(), "{text}");
        }
    }
}

#[cfg(test)]
mod loot_reducer_tests {
    use super::*;
    use crate::stdb::connection::ReducerCallError;

    fn refusal_of(
        result: Result<()>,
        classify: fn(Result<()>) -> Result<LootWindowRequestStatus>,
    ) -> LootWindowRefusal {
        match classify(result) {
            Ok(LootWindowRequestStatus::Refused(refusal)) => refusal,
            Ok(LootWindowRequestStatus::Applied) => panic!("a Refusal was applied"),
            Err(error) => panic!("a Refusal ended the session: {error:#}"),
        }
    }

    fn rejected(reason: &str) -> Result<()> {
        Err(anyhow::Error::from(ReducerCallError::Rejected {
            operation: "gw_take_loot".to_string(),
            reason: reason.to_string(),
        })
        .context("loot window"))
    }

    #[test]
    fn every_module_refusal_tag_becomes_one_client_answer() {
        for refusal in LootRefusal::ALL {
            for classify in [
                strict_loot_request_status as fn(Result<()>) -> Result<LootWindowRequestStatus>,
                legacy_loot_request_status,
            ] {
                assert_eq!(
                    refusal_of(rejected(refusal.as_tag()), classify),
                    LootWindowRefusal::from(refusal),
                    "{refusal:?}"
                );
            }
        }
    }

    #[test]
    fn only_legacy_cores_keep_untagged_gameplay_refusals_unanswered() {
        for reason in ["it is locked", "not a beast", "inventory is full"] {
            assert_eq!(
                refusal_of(rejected(reason), legacy_loot_request_status),
                LootWindowRefusal::Unanswered,
                "{reason}"
            );
            assert!(
                strict_loot_request_status(rejected(reason)).is_err(),
                "{reason}"
            );
        }
    }

    #[test]
    fn boundary_failures_and_unknown_loot_tags_are_fatal() {
        let reasons = LootBoundaryFailure::ALL
            .into_iter()
            .map(LootBoundaryFailure::as_tag)
            .chain(["loot:newer_module_refusal"]);

        for reason in reasons {
            assert!(
                strict_loot_request_status(rejected(reason)).is_err(),
                "{reason}"
            );
            assert!(
                legacy_loot_request_status(rejected(reason)).is_err(),
                "{reason}"
            );
        }
    }

    #[test]
    fn a_timeout_is_not_answered_as_a_refusal() {
        let not_refusals = [
            anyhow::Error::from(ReducerCallError::fatal(
                "gw_take_loot reducer timed out after 10s".to_string(),
            )),
            anyhow::Error::from(ReducerCallError::fatal(
                "gw_loot_money reducer failed: transport disconnected".to_string(),
            )),
            anyhow!(
                "wrapped text that mentions {}",
                LootRefusal::LootTagIneligible.as_tag()
            ),
        ];
        for error in not_refusals {
            let text = format!("{error:#}");
            assert!(strict_loot_request_status(Err(error)).is_err(), "{text}");
        }
    }

    #[test]
    fn loot_action_status_keeps_unknown_results_as_errors() {
        for refusal in [
            LootRefusal::RollUnavailable,
            LootRefusal::NotMasterLooter,
            LootRefusal::RecipientUnavailable,
            LootRefusal::RecipientInventoryFull,
        ] {
            assert_eq!(
                loot_action_status(rejected(refusal.as_tag())).unwrap(),
                LootActionStatus::Refused(refusal)
            );
        }

        for error in [
            anyhow::Error::from(ReducerCallError::Rejected {
                operation: "gw_loot_roll".to_string(),
                reason: "loot:newer_module_refusal".to_string(),
            }),
            anyhow::Error::from(ReducerCallError::fatal(
                "gw_loot_roll reducer timed out after 10s".to_string(),
            )),
        ] {
            assert!(loot_action_status(Err(error)).is_err());
        }
    }
}

#[cfg(test)]
mod auction_reducer_tests {
    use super::*;
    use crate::stdb::connection::ReducerCallError;

    #[test]
    fn only_a_rejected_reducer_carries_a_typed_refusal() {
        for refusal in AuctionRefusal::ALL {
            let error = anyhow::Error::from(ReducerCallError::Rejected {
                operation: "gw_auction_hold_listing".to_string(),
                reason: refusal.as_tag().to_string(),
            })
            .context("listing phase 1");
            assert_eq!(auction_refusal(&error), Some(refusal));
        }

        let not_refusals = [
            anyhow::Error::from(ReducerCallError::fatal(
                "gw_auction_hold_listing reducer timed out after 10s".to_string(),
            )),
            anyhow::Error::from(ReducerCallError::fatal(
                "gw_auction_hold_bid reducer failed: transport disconnected".to_string(),
            )),
            anyhow::Error::from(ReducerCallError::Rejected {
                operation: "gw_auction_hold_bid".to_string(),
                reason: "operator only".to_string(),
            }),
            anyhow!(
                "wrapped text that mentions {}",
                AuctionRefusal::Database.as_tag()
            ),
        ];
        for error in not_refusals {
            assert_eq!(auction_refusal(&error), None, "{error:#}");
        }
    }

    #[test]
    fn deferred_refund_receipt_requires_the_full_hold_payload() {
        let hold = AuctionBidHold {
            operation_id: 7,
            bidder_guid: 8,
            auction_id: 9,
            house: 4,
            offer: 10,
            outcome: lyracore_shared::auction::bid_outcome::BID_INCREMENT,
            revision: 0,
            result_bidder_guid: 0,
            result_bid: 0,
            minimum_increment: 0,
            accepted_price: 0,
            deferred_refund: 3,
        };
        let decision = AuctionBidDecision {
            operation_id: hold.operation_id,
            bidder_guid: hold.bidder_guid,
            auction_id: hold.auction_id,
            house: hold.house,
            offer: hold.offer,
            outcome: hold.outcome,
            revision: hold.revision,
            result_bidder_guid: hold.result_bidder_guid,
            result_bid: hold.result_bid,
            minimum_increment: hold.minimum_increment,
            accepted_price: hold.accepted_price,
            deferred_refund: hold.deferred_refund,
        };

        assert!(bid_refund_is_recorded(&hold, &decision));
        assert!(!bid_refund_is_recorded(
            &hold,
            &AuctionBidDecision {
                offer: 11,
                ..decision.clone()
            }
        ));
        assert!(!bid_refund_is_recorded(
            &hold,
            &AuctionBidDecision {
                deferred_refund: 2,
                ..decision
            }
        ));

        let bidder_guid = hold.bidder_guid;
        assert!(bid_hold_has_value(&hold, bidder_guid));
        assert!(!bid_hold_has_value(&hold, bidder_guid + 1));
        assert!(!bid_hold_has_value(
            &AuctionBidHold {
                deferred_refund: 0,
                ..hold
            },
            bidder_guid
        ));
    }

    #[test]
    fn refused_listing_refund_commits_on_realm_core_before_the_home_hold_is_deleted() {
        let drive = crate::test_scan::code_of(
            include_str!("reducers.rs"),
            "fn drive_sharded_auction_listing(",
        );
        let refund = "realm.auction_refund_listing(&hold)?;";
        let release = "self.auction_release_listing_hold(&hold)?;";
        let refund_at = drive
            .find(refund)
            .expect("the Realm-core handle must commit the refused listing Mail");
        let release_at = drive
            .find(release)
            .expect("the Home Shard must delete the Hold after that commit");
        assert!(
            refund_at < release_at,
            "a source-Hold delete before Realm-core Mail commit loses the only listing value"
        );
        assert!(
            !drive.contains("self.auction_refund_listing(&hold)?;"),
            "the Home Shard does not own Mail in a sharded realm"
        );
    }

    #[test]
    fn refused_listing_binding_matches_the_generated_commit_listing_shape() {
        let expected = include_str!("bindings/realm_auction_commit_listing_reducer.rs")
            .replace("RealmAuctionCommitListing", "RealmAuctionRefundListing")
            .replace(
                "realm_auction_commit_listing",
                "realm_auction_refund_listing",
            );
        assert_eq!(
            include_str!("bindings/realm_auction_refund_listing_reducer.rs"),
            expected,
            "the hand-added reducer binding must remain generator-identical"
        );
    }

    #[test]
    fn normalized_buyout_price_drives_the_exact_success_command_value() {
        let hold = AuctionBidHold {
            operation_id: 7,
            bidder_guid: 8,
            auction_id: 9,
            house: 4,
            offer: 900,
            outcome: lyracore_shared::auction::bid_outcome::ACCEPTED,
            revision: 0,
            result_bidder_guid: 0,
            result_bid: 0,
            minimum_increment: 0,
            accepted_price: 500,
            deferred_refund: 0,
        };

        assert_eq!(
            bid_outcome(&hold).unwrap(),
            crate::world::PlaceBidOutcome::Accepted {
                minimum_increment: 25,
            }
        );
    }
}

#[cfg(test)]
mod trainer_reducer_tests {
    use super::*;
    use crate::stdb::connection::ReducerCallError;

    #[test]
    fn only_a_rejected_reducer_carries_a_typed_refusal() {
        for refusal in TrainerRefusal::ALL {
            let error = anyhow::Error::from(ReducerCallError::Rejected {
                operation: "gw_trainer_buy".to_string(),
                reason: refusal.as_tag().to_string(),
            })
            .context("trainer buy");
            assert_eq!(trainer_refusal(&error), Some(refusal));
        }

        let not_refusals = [
            anyhow::Error::from(ReducerCallError::fatal(
                "gw_trainer_buy reducer timed out after 10s".to_string(),
            )),
            anyhow::Error::from(ReducerCallError::Rejected {
                operation: "gw_trainer_buy".to_string(),
                reason: "operator only".to_string(),
            }),
            anyhow!(
                "wrapped text that mentions {}",
                TrainerRefusal::AlreadyKnown.as_tag()
            ),
        ];
        for error in not_refusals {
            assert_eq!(trainer_refusal(&error), None, "{error:#}");
        }
    }
}
