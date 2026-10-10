//! `Coordinator`'s [`AuctionActionStore`] adapter.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};
use lyracore_shared::auction::AuctionRefusal;
use lyracore_shared::item::Proficiency;
use spacetimedb_sdk::Table;

use crate::stdb::bindings::*;
use crate::stdb::connection::{call_reducer, classify, DurableFailure};
use crate::stdb::reducers::{next_operation_id, wait_for_cache_row};
use crate::stdb::Coordinator;
use crate::world::{
    Actor, AuctionActionStore, AuctionBrowseRequest, AuctionHousePolicy, AuctionInteraction,
    AuctionPage, AuctionQuery, CancelAuctionOutcome, CancelAuctionRequest, CreateAuctionOutcome,
    CreateAuctionRequest, PlaceBidOutcome, PlaceBidRequest,
};

impl AuctionActionStore for crate::stdb::Coordinator {
    fn auction_interaction(
        &self,
        actor: Actor,
        auctioneer_guid: u64,
    ) -> Result<Option<AuctionInteraction>> {
        use crate::stdb::bindings::{
            GameAuctionHouseTableAccess, GameFactionTemplateTableAccess, GameWorldEntityTableAccess,
        };
        let house = {
            let guard = self.0.coord();
            let db = &guard.conn.db;
            let Some(auctioneer) = db.game_world_entity().guid().find(&auctioneer_guid) else {
                return Ok(None);
            };
            let Some(faction_group) = db
                .game_faction_template()
                .id()
                .find(&auctioneer.faction_template)
                .map(|template| template.faction_group)
            else {
                return Ok(None);
            };
            let house_id = lyracore_shared::auction::house_for_faction_template(
                auctioneer.faction_template,
                faction_group,
            );
            let Some(house) =
                db.game_auction_house()
                    .id()
                    .find(&house_id)
                    .map(|house| AuctionHousePolicy {
                        id: house.id,
                        deposit_rate: house.deposit_rate,
                        consignment_rate: house.consignment_rate,
                    })
            else {
                return Ok(None);
            };
            house
        };
        let refuses_interaction = self.npc_refuses_interaction(auctioneer_guid, actor.guid())?;
        Ok(Some(AuctionInteraction {
            house,
            refuses_interaction,
        }))
    }

    /// Create one auction listing. Single-database deployments use one atomic reducer; sharded
    /// deployments drive the durable Hold -> Auction -> receipt -> settle protocol.
    fn create_auction(&self, request: CreateAuctionRequest) -> Result<CreateAuctionOutcome> {
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
            next_operation_id()?
        } else {
            match self.matching_auction_hold(request) {
                Some(hold) => hold.operation_id,
                None => next_operation_id()?,
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

    /// Place one full-offer bid. The sharded path fences the bidder purse before realm-core makes
    /// its serialized Auction decision; both paths finish with the same normalized terminal Hold.
    fn place_bid(&self, request: PlaceBidRequest) -> Result<PlaceBidOutcome> {
        let operation_id = self
            .matching_unfinished_bid_hold(request)
            .map_or_else(next_operation_id, |hold| Ok(hold.operation_id))?;
        let result = if self.is_sharded() {
            self.drive_sharded_auction_bid(operation_id, request)
        } else {
            self.auction_bid_local(operation_id, request)
                .and_then(|()| self.wait_for_terminal_bid_hold(operation_id))
        };
        match result {
            Ok(hold) => bid_outcome(&hold),
            Err(error) => match auction_refusal(&error) {
                Some(refusal) => Ok(refusal.into()),
                None => Err(error),
            },
        }
    }

    /// Cancel one listing. The Gateway reads the listing from the Realm-core cache and fences the
    /// Auction Cut it computes; Realm-core refuses the Cancellation if a later bid changed that cut.
    /// A pending Cancellation of the same listing is driven again instead of fencing a second cut.
    fn cancel_auction(&self, request: CancelAuctionRequest) -> Result<CancelAuctionOutcome> {
        let pending = self
            .unfinished_auction_holds(request.actor.guid())
            .find(|hold| {
                hold.operation == lyracore_shared::auction::hold_operation::CANCEL
                    && hold.outcome == lyracore_shared::auction::bid_outcome::PENDING
                    && hold.auction_id == request.auction_id
                    && hold.house == request.house_id
            });
        let (operation_id, cut) = match pending {
            Some(hold) => (hold.operation_id, hold.offer),
            None => match self.realm_core()?.cancellation_cut(request) {
                Some(cut) => (next_operation_id()?, cut),
                None => return Ok(CancelAuctionOutcome::NotFound),
            },
        };
        let result = if self.is_sharded() {
            self.auction_hold_cancel(operation_id, request, cut)
                .and_then(|()| self.wait_for_auction_bid_hold(operation_id))
                .and_then(|hold| self.complete_auction_hold(hold))
        } else {
            self.auction_cancel_local(operation_id, request, cut)
                .and_then(|()| self.wait_for_terminal_bid_hold(operation_id))
        };
        match result {
            Ok(hold) => cancel_outcome(&hold),
            Err(error) => match auction_refusal(&error) {
                Some(refusal) => Ok(refusal.into()),
                None => Err(error),
            },
        }
    }

    /// Finish every unfinished Hold of `actor` on this Home Shard, listing Holds first. A Refusal
    /// leaves that Hold for a later attempt; a Transport Loss ends the request.
    fn resume_auction_holds(&self, actor: Actor) -> Result<()> {
        let actor_guid = actor.guid();
        for hold in self.listing_holds(actor_guid) {
            let operation_id = hold.operation_id;
            if let Err(error) = self.complete_listing_hold(&hold) {
                match auction_refusal(&error) {
                    Some(refusal) => log::warn!(
                        "listing Hold {operation_id} of {actor_guid} did not resume: {refusal:?}"
                    ),
                    None => return Err(error),
                }
            }
        }
        let holds: Vec<_> = self.unfinished_auction_holds(actor_guid).collect();
        for hold in holds {
            let operation_id = hold.operation_id;
            if let Err(error) = self.complete_auction_hold(hold) {
                match auction_refusal(&error) {
                    Some(refusal) => log::warn!(
                        "auction Hold {operation_id} of {actor_guid} did not resume: {refusal:?}"
                    ),
                    None => return Err(error),
                }
            }
        }
        Ok(())
    }

    fn auction_query(
        &self,
        actor: Actor,
        house_id: u32,
        query: AuctionQuery,
    ) -> Result<AuctionPage> {
        let player_guid = actor.guid();
        let (player_level, proficiency) = {
            let guard = self.0.coord();
            let db = &guard.conn.db;
            let player = db
                .game_world_entity()
                .guid()
                .find(&player_guid)
                .ok_or_else(|| anyhow!("auction query actor {player_guid} is not in world"))?;
            // The "usable" checkbox asks what THIS Character can wear, so the armor half reads its
            // trained passives, not the class ceiling — a level-1 Warrior must not see plate as
            // usable. One spellbook scan per query, not per row.
            let learned: Vec<u32> = db
                .game_player_spell()
                .iter()
                .filter(|s| s.character_guid == player_guid)
                .map(|s| s.spell_id)
                .collect();
            (
                u8::try_from(player.level).unwrap_or(u8::MAX),
                crate::codec::armor_proficiency(
                    ((player.unit_bytes_0 >> 8) & 0xff) as u8,
                    &learned,
                ),
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
            lyracore_shared::auction::market_of(house_id),
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
                            proficiency,
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
                    outbid_auction_ids, ..
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

impl Coordinator {
    /// The keys `pick` names for `character_guid` in THIS handle's auction index.
    pub(super) fn auction_keys(
        &self,
        pick: impl Fn(
            &crate::stdb::auction_holds::AuctionIndex,
        ) -> &crate::stdb::auction_holds::KeysByCharacter,
        character_guid: u64,
    ) -> Vec<u64> {
        let guard = self.0.coord();
        let index = guard
            .auctions
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        pick(&index).keys_of(character_guid)
    }

    /// The rows `pick` names for `character_guid` in THIS handle's auction index, read back by
    /// primary key through `find`. A row whose `owner` is no longer `character_guid` is dropped,
    /// because the index can trail the cache.
    pub(super) fn indexed_rows<T>(
        &self,
        pick: impl Fn(
            &crate::stdb::auction_holds::AuctionIndex,
        ) -> &crate::stdb::auction_holds::KeysByCharacter,
        character_guid: u64,
        find: impl Fn(&RemoteTables, u64) -> Option<T>,
        owner: impl Fn(&T) -> u64,
    ) -> Vec<T> {
        let keys = self.auction_keys(pick, character_guid);
        let guard = self.0.coord();
        keys.into_iter()
            .filter_map(|key| find(&guard.conn.db, key))
            .filter(|row| owner(row) == character_guid)
            .collect()
    }

    /// `actor_guid`'s unfinished Holds on THIS handle.
    pub(super) fn unfinished_auction_holds(
        &self,
        actor_guid: u64,
    ) -> impl Iterator<Item = AuctionBidHold> {
        self.indexed_rows(
            |index| &index.unfinished_bid_holds,
            actor_guid,
            |db, id| db.game_auction_bid_hold().operation_id().find(&id),
            |hold| hold.bidder_guid,
        )
        .into_iter()
        .filter(crate::stdb::auction_holds::hold_is_unfinished)
    }

    /// `seller_guid`'s listing Holds on THIS handle.
    pub(super) fn listing_holds(&self, seller_guid: u64) -> Vec<AuctionHold> {
        self.indexed_rows(
            |index| &index.listing_holds,
            seller_guid,
            |db, id| db.game_auction_hold().operation_id().find(&id),
            |hold| hold.seller_guid,
        )
    }
    fn drive_sharded_auction_listing(
        &self,
        operation_id: u64,
        request: crate::world::CreateAuctionRequest,
    ) -> Result<u32> {
        self.auction_hold_listing(operation_id, request)?;
        let hold = self.wait_for_auction_hold(operation_id)?;
        self.complete_listing_hold(&hold)
    }

    /// Realm-core's Auction, this Home Shard's receipt and the settle of one listing Hold, or its
    /// refund when Realm-core refuses the listing, from the phase a stopped Gateway left it in.
    /// Every phase is idempotent, so a replay changes nothing.
    fn complete_listing_hold(&self, hold: &AuctionHold) -> Result<u32> {
        let operation_id = hold.operation_id;
        let realm = self.realm_core()?;
        if let Err(error) = realm.auction_commit_listing(hold) {
            // Only a Refusal proves realm-core took nothing. A timeout or transport failure
            // leaves the Hold for the next replay, which commits idempotently.
            if matches!(classify(&error), DurableFailure::Refusal { .. })
                && !realm.auction_receipt_is_visible(operation_id)
            {
                // Mail is authoritative on Realm-core. Commit its idempotent refund receipt and
                // the exact returned value there before deleting the source Hold. If either call
                // is interrupted, the Hold remains recovery evidence and the next replay resumes
                // from the same operation id.
                realm.auction_refund_listing(hold)?;
                self.auction_release_listing_hold(hold)?;
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
                self.session_actor(request.actor),
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
                self.session_actor(request.actor),
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
        let actor = self.hold_actor(hold.seller_guid)?;
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_auction_commit_listing",
            realm_auction_commit_listing_then(
                hold.operation_id,
                actor,
                hold.item_guid,
                hold.item_entry,
                hold.item_stack_count,
                hold.item_durability,
                hold.item_enchant_id,
                hold.item_soulbound,
                hold.random_property_id,
                hold.house,
                hold.deposit_rate,
                hold.consignment_rate,
                hold.start_bid,
                hold.buyout,
                hold.duration_minutes,
                hold.deposit,
                hold.created_micros,
                hold.expires_micros,
                hold.item_text_id
            )
        )
    }

    fn auction_confirm_listing(&self, operation_id: u64, auction_id: u32) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_auction_confirm_listing",
            realm_auction_confirm_listing_then(operation_id, auction_id, self.owner_actor())
        )
    }

    fn auction_settle_listing(&self, operation_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_auction_settle_listing",
            realm_auction_settle_listing_then(operation_id, self.owner_actor())
        )
    }

    fn auction_refund_listing(&self, hold: &AuctionHold) -> Result<()> {
        let actor = self.hold_actor(hold.seller_guid)?;
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_auction_refund_listing",
            realm_auction_refund_listing_then(
                hold.operation_id,
                actor,
                hold.item_guid,
                hold.item_entry,
                hold.item_stack_count,
                hold.item_durability,
                hold.item_enchant_id,
                hold.item_soulbound,
                hold.random_property_id,
                hold.house,
                hold.deposit_rate,
                hold.consignment_rate,
                hold.start_bid,
                hold.buyout,
                hold.duration_minutes,
                hold.deposit,
                hold.created_micros,
                hold.expires_micros,
                hold.item_text_id
            )
        )
    }

    fn auction_release_listing_hold(&self, hold: &AuctionHold) -> Result<()> {
        let actor = self.hold_actor(hold.seller_guid)?;
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_auction_release_listing_hold",
            gw_auction_release_listing_hold_then(hold.operation_id, actor)
        )
    }

    /// The Character a Hold acts for. A Hold without one is corrupt.
    fn hold_actor(&self, character_guid: u64) -> Result<SessionActor> {
        let actor =
            Actor::new(character_guid).ok_or_else(|| anyhow!("auction Hold names no Character"))?;
        Ok(self.session_actor(actor))
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
        self.listing_holds(request.actor.guid())
            .into_iter()
            .find(|hold| same_auction_request(hold, request))
    }

    fn matching_active_auction_receipt(
        &self,
        request: crate::world::CreateAuctionRequest,
    ) -> Result<Option<AuctionOperationReceipt>> {
        let candidates: Vec<_> = self
            .listing_receipts(request.actor.guid())
            .into_iter()
            .filter(|receipt| same_auction_request(receipt, request))
            .collect();
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
        wait_for_cache_row(operation_id, "auction Hold", || {
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
        wait_for_cache_row(operation_id, "auction receipt", || {
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

    fn drive_sharded_auction_bid(
        &self,
        operation_id: u64,
        request: crate::world::PlaceBidRequest,
    ) -> Result<AuctionBidHold> {
        self.auction_hold_bid(operation_id, request)?;
        let hold = self.wait_for_auction_bid_hold(operation_id)?;
        self.complete_auction_hold(hold)
    }

    /// Realm-core's decision, the Home Shard's finish, and any refund phases of one Hold, from the
    /// phase a stopped Gateway left it in. Every phase is idempotent, so a replay changes nothing.
    fn complete_auction_hold(&self, mut hold: AuctionBidHold) -> Result<AuctionBidHold> {
        let operation_id = hold.operation_id;
        let realm = self.realm_core()?;
        if hold.outcome == lyracore_shared::auction::bid_outcome::PENDING {
            realm.auction_decide(&hold)?;
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
                self.session_actor(request.actor),
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
                self.session_actor(request.actor),
                request.auctioneer_guid,
                request.auction_id,
                request.house_id,
                request.offer
            )
        )
    }

    fn auction_hold_cancel(
        &self,
        operation_id: u64,
        request: crate::world::CancelAuctionRequest,
        cut: u32,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_auction_hold_cancel",
            gw_auction_hold_cancel_then(
                operation_id,
                self.session_actor(request.actor),
                request.auctioneer_guid,
                request.auction_id,
                request.house_id,
                cut
            )
        )
    }

    fn auction_cancel_local(
        &self,
        operation_id: u64,
        request: crate::world::CancelAuctionRequest,
        cut: u32,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_auction_cancel_local",
            gw_auction_cancel_local_then(
                operation_id,
                self.session_actor(request.actor),
                request.auctioneer_guid,
                request.auction_id,
                request.house_id,
                cut
            )
        )
    }

    /// Realm-core's decision for a Hold, by the operation the Hold pays for.
    fn auction_decide(&self, hold: &AuctionBidHold) -> Result<()> {
        let actor = self.hold_actor(hold.bidder_guid)?;
        use lyracore_shared::auction::hold_operation;
        match hold.operation {
            hold_operation::BID => call_reducer!(
                self.0.call_pipe().conn.reducers,
                "realm_auction_decide_bid",
                realm_auction_decide_bid_then(
                    hold.operation_id,
                    actor,
                    hold.auction_id,
                    hold.house,
                    hold.offer
                )
            ),
            hold_operation::CANCEL => call_reducer!(
                self.0.call_pipe().conn.reducers,
                "realm_auction_decide_cancel",
                realm_auction_decide_cancel_then(
                    hold.operation_id,
                    actor,
                    hold.auction_id,
                    hold.house,
                    hold.offer
                )
            ),
            operation => Err(anyhow!(
                "auction Hold {} pays for unknown operation {operation}",
                hold.operation_id
            )),
        }
    }

    /// The Auction Cut the seller pays to cancel the listing, read from THIS handle's cache.
    /// `None` when the listing is gone, is not the seller's, or lists in another market.
    fn cancellation_cut(&self, request: crate::world::CancelAuctionRequest) -> Option<u32> {
        let guard = self.0.coord();
        let auction = guard
            .conn
            .db
            .game_auction()
            .id()
            .find(&request.auction_id)?;
        listing_cut(&auction, request)
    }

    /// `seller_guid`'s listing receipts on THIS handle.
    fn listing_receipts(&self, seller_guid: u64) -> Vec<AuctionOperationReceipt> {
        self.indexed_rows(
            |index| &index.listing_receipts,
            seller_guid,
            |db, id| db.game_auction_operation_receipt().operation_id().find(&id),
            |receipt| receipt.actor_guid,
        )
    }

    fn auction_finish_bid(
        &self,
        hold: &AuctionBidHold,
        decision: &AuctionBidDecision,
    ) -> Result<()> {
        let actor = self.hold_actor(hold.bidder_guid)?;
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
                actor,
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
        let actor = self.hold_actor(hold.bidder_guid)?;
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_auction_refund_bid",
            realm_auction_refund_bid_then(
                hold.operation_id,
                actor,
                hold.auction_id,
                hold.house,
                hold.offer,
                hold.deferred_refund
            )
        )
    }

    fn auction_confirm_bid_refund(&self, hold: &AuctionBidHold) -> Result<()> {
        let actor = self.hold_actor(hold.bidder_guid)?;
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_auction_confirm_bid_refund",
            gw_auction_confirm_bid_refund_then(
                hold.operation_id,
                actor,
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
        self.unfinished_auction_holds(request.actor.guid())
            .find(|hold| {
                hold.operation == lyracore_shared::auction::hold_operation::BID
                    && hold.auction_id == request.auction_id
                    && hold.house == request.house_id
                    && hold.offer == request.offer
            })
    }

    fn wait_for_auction_bid_hold(&self, operation_id: u64) -> Result<AuctionBidHold> {
        wait_for_cache_row(operation_id, "auction bid Hold", || {
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
        wait_for_cache_row(operation_id, "auction terminal bid Hold", || {
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
        wait_for_cache_row(operation_id, "auction bid decision", || {
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
        wait_for_cache_row(hold.operation_id, "auction bid refund", || {
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
        wait_for_cache_row(operation_id, "auction settled bid refund", || {
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
}

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
    proficiency: Proficiency,
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
                && proficiency.can_equip(item.item_class, item.item_subclass)))
}

fn paginate(mut rows: Vec<Auction>, offset: u32) -> (Vec<Auction>, u32) {
    rows.sort_by_key(|row| row.id);
    let total = u32::try_from(rows.len()).unwrap_or(u32::MAX);
    let page = rows.into_iter().skip(offset as usize).take(50).collect();
    (page, total)
}

fn select_active_page(
    rows: impl IntoIterator<Item = Auction>,
    market: lyracore_shared::auction::AuctionMarket,
    now_micros: i64,
    offset: u32,
    mut matches: impl FnMut(&Auction) -> bool,
) -> (Vec<Auction>, u32) {
    paginate(
        rows.into_iter()
            .filter(|row| {
                lyracore_shared::auction::market_of(row.house) == market
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
        random_property_id: row.random_property_id,
    }
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
    row.actor_guid() == request.actor.guid()
        && row.item_guid() == request.item_guid
        && row.start_bid() == request.start_bid
        && row.buyout() == request.buyout
        && row.duration_minutes() == request.duration_minutes
        && row.house_id() == request.house_id
}

/// The Module's typed auction Refusal. Only a reducer the Module rejected carries a tag; a Transport
/// Loss stays an error with an unknown outcome.
fn auction_refusal(error: &anyhow::Error) -> Option<AuctionRefusal> {
    match classify(error) {
        DurableFailure::Refusal { reason } => AuctionRefusal::parse_tag(reason),
        DurableFailure::TransportLoss => None,
    }
}

fn bid_payload_matches(hold: &AuctionBidHold, decision: &AuctionBidDecision) -> bool {
    hold.operation_id == decision.operation_id
        && hold.operation == decision.operation
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

/// The seller's Auction Cut for `request`'s listing, when the listing is the seller's and lists in
/// the auctioneer's market.
fn listing_cut(auction: &Auction, request: crate::world::CancelAuctionRequest) -> Option<u32> {
    use lyracore_shared::auction::{auction_cut, market_of};
    if auction.owner_guid != request.actor.guid()
        || market_of(auction.house) != market_of(request.house_id)
    {
        return None;
    }
    auction_cut(auction.highest_bid, auction.consignment_rate)
}

fn cancel_outcome(hold: &AuctionBidHold) -> Result<crate::world::CancelAuctionOutcome> {
    use crate::world::CancelAuctionOutcome;
    use lyracore_shared::auction::{bid_outcome, hold_operation};
    if hold.operation != hold_operation::CANCEL {
        return Err(anyhow!(
            "auction Hold {} is not a Cancellation",
            hold.operation_id
        ));
    }
    Ok(match hold.outcome {
        bid_outcome::CANCELLED => CancelAuctionOutcome::Cancelled,
        bid_outcome::ITEM_NOT_FOUND => CancelAuctionOutcome::NotFound,
        bid_outcome::PENDING => {
            return Err(anyhow!(
                "auction Cancellation Hold {} is still pending",
                hold.operation_id
            ));
        }
        _ => CancelAuctionOutcome::Stale,
    })
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
            ));
        }
    })
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

    /// A Character of `player_class` that has trained neither level-40 armor upgrade.
    fn untrained(player_class: u8) -> Proficiency {
        Proficiency::derive(player_class, false, false)
    }

    #[test]
    fn browse_supports_every_exact_filter_and_the_existing_usable_model() {
        assert!(browse_matches(&request(), 15, untrained(1), facts()));

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
            assert!(!browse_matches(&mismatch, 15, untrained(1), facts()));
        }
        assert!(!browse_matches(&request(), 14, untrained(1), facts()));
        assert!(!browse_matches(&request(), 15, untrained(5), facts()));
        assert!(browse_matches(
            &AuctionBrowseRequest {
                usable_only: false,
                ..request()
            },
            15,
            untrained(5),
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
            untrained(5),
            facts(),
        ));
    }

    #[test]
    fn the_usable_filter_follows_trained_armor_not_the_class_ceiling() {
        let plate = BrowseFacts {
            item_class: 4,
            item_subclass: 4,
            ..facts()
        };
        let usable = AuctionBrowseRequest {
            item_class: Some(4),
            item_subclass: Some(4),
            ..request()
        };
        assert!(!browse_matches(&usable, 60, untrained(1), plate));
        assert!(browse_matches(
            &usable,
            60,
            Proficiency::derive(1, true, false),
            plate
        ));
        // A Mage trains neither upgrade, so plate stays unusable however the flags fall.
        assert!(!browse_matches(
            &usable,
            60,
            Proficiency::derive(8, true, true),
            plate
        ));
    }

    fn auction(id: u32) -> Auction {
        Auction {
            id,
            listing_operation_id: u64::from(id),
            house: 4,
            owner_guid: 1,
            item_guid: u64::from(id),
            item_entry: 25,
            item_stack_count: 1,
            item_durability: 10,
            item_enchant_id: 0,
            item_soulbound: false,
            random_property_id: 0,
            start_bid: 1,
            buyout: 0,
            highest_bidder_guid: 0,
            highest_bid: 0,
            deposit: 1,
            created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
            expires_at: spacetimedb_sdk::Timestamp::from_micros_since_unix_epoch(i64::MAX),
            revision: 0,
            deposit_rate: 5,
            consignment_rate: 5,
            item_text_id: 0,
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
    fn active_selection_excludes_expired_other_market_and_non_owner_rows_before_totals() {
        use lyracore_shared::auction::AuctionMarket;

        let mut expired = auction(1);
        expired.expires_at = spacetimedb_sdk::Timestamp::from_micros_since_unix_epoch(10);
        let mut at_deadline = auction(2);
        at_deadline.expires_at = spacetimedb_sdk::Timestamp::from_micros_since_unix_epoch(20);
        let mut other_market = auction(3);
        other_market.house = 7; // Blackwater is neutral, outside the queried Horde market.
        let mut same_market_other_house = auction(4);
        same_market_other_house.house = 5; // Thunder Bluff pools into the same Horde market.
        let mut other_owner = auction(5);
        other_owner.house = 4;
        other_owner.owner_guid = 2;
        let mut owned = auction(6);
        owned.house = 4;

        let rows = vec![
            expired,
            at_deadline,
            other_market,
            same_market_other_house,
            other_owner,
            owned,
        ];
        let (page, total) =
            select_active_page(rows, AuctionMarket::Horde, 20, 0, |row| row.owner_guid == 1);
        assert_eq!(total, 2);
        assert_eq!(
            page.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![4, 6]
        );
    }

    #[test]
    fn bidder_selection_never_reintroduces_requested_outbid_auctions() {
        use lyracore_shared::auction::AuctionMarket;

        let mut highest = auction(5);
        highest.highest_bidder_guid = 8;
        highest.highest_bid = 107;
        let mut displaced = auction(19);
        displaced.highest_bidder_guid = 9;
        displaced.highest_bid = 113;

        let requested_outbid_ids = [19, 88];
        let (page, total) = select_active_page(
            vec![displaced, highest],
            AuctionMarket::Horde,
            20,
            0,
            |row| bidder_matches(row, 8, &requested_outbid_ids),
        );
        assert_eq!(total, 1);
        assert_eq!(page.iter().map(|row| row.id).collect::<Vec<_>>(), vec![5]);
    }
}

#[cfg(test)]
mod auction_reducer_tests {
    use super::*;
    use crate::stdb::connection::ReducerCallError;

    #[test]
    #[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
    fn a_local_bid_interaction_refusal_stays_a_client_outcome() {
        use crate::accept::BlockingTaskCapacity;
        use crate::config::GatewayConfig;
        use crate::durable_test_support::Standalone;
        use crate::world::{PlaceBidOutcome, PlaceBidRequest};

        for variable in [
            "LYRACORE_SHARD_MAP",
            "LYRACORE_SHARD_MAP_FILE",
            "LYRACORE_REALM_CORE",
        ] {
            assert!(
                std::env::var_os(variable).is_none(),
                "unset {variable} for this private test"
            );
        }
        let mut standalone = Standalone::start("auction-refusal");
        standalone.publish_module();
        standalone.assert_call("claim_operator", &[]);
        standalone.assert_call("install_guid_range", &["0"]);
        let cfg = GatewayConfig {
            logon_bind: "127.0.0.1:0".into(),
            world_bind: "127.0.0.1:0".into(),
            stdb_uri: standalone.server().into(),
            module_name: standalone.shard_name().into(),
            coordinator_token: Some(standalone.owner_token()),
            gateway_id: "auction-refusal-test".into(),
            blocking_task_capacity: BlockingTaskCapacity::new(1),
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let coordinator = runtime.block_on(Coordinator::connect(&cfg)).unwrap();
        assert!(!coordinator.is_sharded());
        let outcome = coordinator
            .place_bid(PlaceBidRequest {
                actor: Actor::new(5_090_099).unwrap(),
                auctioneer_guid: 5_090_098,
                auction_id: 5_090_097,
                offer: 100,
                house_id: 1,
            })
            .expect("a Module Refusal must not end the World Session");
        assert_eq!(outcome, PlaceBidOutcome::Database);
        assert!(standalone
            .query_rows("SELECT * FROM game_auction_bid_hold")
            .is_empty());
    }

    #[test]
    fn only_a_rejected_reducer_carries_a_typed_refusal() {
        for refusal in AuctionRefusal::ALL {
            let error = anyhow::Error::from(ReducerCallError::Rejected {
                operation: "gw_auction_hold_listing".to_string(),
                reason: refusal.as_tag().to_string(),
            })
            .context("preparing auction listing");
            assert_eq!(auction_refusal(&error), Some(refusal));
        }

        let not_refusals = [
            anyhow::Error::from(ReducerCallError::transport_lost("gw_auction_hold_listing")),
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
            operation: lyracore_shared::auction::hold_operation::BID,
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
            item_entry: 0,
            random_property_id: 0,
            operation: hold.operation,
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

        assert!(crate::stdb::auction_holds::hold_is_unfinished(&hold));
        assert!(!crate::stdb::auction_holds::hold_is_unfinished(
            &AuctionBidHold {
                deferred_refund: 0,
                ..hold
            }
        ));
    }

    #[test]
    fn refused_listing_binding_matches_the_generated_commit_listing_shape() {
        let expected = include_str!("../bindings/realm_auction_commit_listing_reducer.rs")
            .replace("RealmAuctionCommitListing", "RealmAuctionRefundListing")
            .replace(
                "realm_auction_commit_listing",
                "realm_auction_refund_listing",
            );
        assert_eq!(
            include_str!("../bindings/realm_auction_refund_listing_reducer.rs"),
            expected,
            "the hand-added reducer binding must remain generator-identical"
        );
    }

    fn cancel_request() -> crate::world::CancelAuctionRequest {
        crate::world::CancelAuctionRequest {
            actor: Actor::new(7).unwrap(),
            auctioneer_guid: 42,
            auction_id: 41,
            house_id: 2,
        }
    }

    fn listing(owner_guid: u64, house: u32, highest_bid: u32) -> Auction {
        Auction {
            id: 41,
            listing_operation_id: 1,
            house,
            owner_guid,
            item_guid: 70,
            item_entry: 25,
            item_stack_count: 1,
            item_durability: 0,
            item_enchant_id: 0,
            item_soulbound: false,
            start_bid: 100,
            buyout: 0,
            highest_bidder_guid: u64::from(highest_bid != 0) * 9,
            highest_bid,
            deposit: 10,
            created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
            expires_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
            revision: 1,
            deposit_rate: 5,
            consignment_rate: 5,
            random_property_id: 0,
            item_text_id: 0,
        }
    }

    /// 5% of a bid of 201 is 10.05, truncated to 10 (`cm:AuctionHouseMgr.cpp:733-736`).
    #[test]
    fn the_gateway_fences_the_cut_of_the_sellers_own_listing_in_its_market() {
        assert_eq!(listing_cut(&listing(7, 1, 201), cancel_request()), Some(10));
        assert_eq!(listing_cut(&listing(7, 3, 0), cancel_request()), Some(0));
        assert_eq!(
            listing_cut(&listing(8, 1, 201), cancel_request()),
            None,
            "another player's listing"
        );
        assert_eq!(
            listing_cut(&listing(7, 4, 201), cancel_request()),
            None,
            "the Horde market"
        );
    }

    #[test]
    fn a_terminal_cancellation_hold_maps_to_one_client_outcome() {
        use crate::world::CancelAuctionOutcome;
        use lyracore_shared::auction::{bid_outcome, hold_operation};
        let hold = |operation, outcome| AuctionBidHold {
            operation_id: 7,
            bidder_guid: 8,
            auction_id: 41,
            house: 1,
            offer: 10,
            outcome,
            revision: 0,
            result_bidder_guid: 0,
            result_bid: 0,
            minimum_increment: 0,
            accepted_price: 10,
            deferred_refund: 0,
            operation,
        };
        for (outcome, expected) in [
            (bid_outcome::CANCELLED, CancelAuctionOutcome::Cancelled),
            (bid_outcome::ITEM_NOT_FOUND, CancelAuctionOutcome::NotFound),
            (bid_outcome::DATABASE, CancelAuctionOutcome::Stale),
        ] {
            assert_eq!(
                cancel_outcome(&hold(hold_operation::CANCEL, outcome)).unwrap(),
                expected
            );
        }
        assert!(cancel_outcome(&hold(hold_operation::CANCEL, bid_outcome::PENDING)).is_err());
        assert!(
            cancel_outcome(&hold(hold_operation::BID, bid_outcome::CANCELLED)).is_err(),
            "a bid Hold never answers a Cancellation"
        );
        assert!(!bid_payload_matches(
            &hold(hold_operation::CANCEL, bid_outcome::PENDING),
            &AuctionBidDecision {
                operation_id: 7,
                bidder_guid: 8,
                auction_id: 41,
                offer: 10,
                outcome: bid_outcome::CANCELLED,
                revision: 0,
                result_bidder_guid: 0,
                result_bid: 0,
                minimum_increment: 0,
                deferred_refund: 0,
                accepted_price: 10,
                house: 1,
                item_entry: 25,
                random_property_id: 0,
                operation: hold_operation::BID,
            }
        ));
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
            operation: lyracore_shared::auction::hold_operation::BID,
        };

        assert_eq!(
            bid_outcome(&hold).unwrap(),
            crate::world::PlaceBidOutcome::Accepted {
                minimum_increment: 25,
            }
        );
    }
}
