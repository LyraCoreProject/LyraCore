//! Loot-window action dispatch plus the remaining corpse, GameObject, and group-loot handler.

use super::super::*;
use super::combat::ignore_refusal;
use crate::world::family::{ProtocolFamily, ProtocolReply, ProtocolRequest, ProtocolSession};
use lyracore_shared::loot::LootRefusal;
use wow_world_messages::vanilla::LootMethodError;

/// Durable reads and requests needed by the loot-window lifecycle.
pub(crate) trait LootWindowStore: Send + Sync {
    fn loot_target_money(&self, target_guid: u64) -> Result<u32>;
    fn loot_target_items(
        &self,
        target_guid: u64,
        viewer_guid: u64,
    ) -> Result<Vec<codec::LootItemView>>;
    fn use_gameobject(&self, actor: Actor, target_guid: u64) -> Result<LootWindowRequestStatus>;
    fn open_creature_loot(&self, actor: Actor, corpse_guid: u64)
        -> Result<LootWindowRequestStatus>;
    fn skin_corpse(&self, actor: Actor, target_guid: u64) -> Result<LootWindowRequestStatus>;
    fn loot_money(&self, actor: Actor, target_guid: u64) -> Result<LootWindowRequestStatus>;
    fn take_loot(
        &self,
        actor: Actor,
        target_guid: u64,
        loot_slot: u8,
    ) -> Result<LootWindowRequestStatus>;
}

/// How the Module answered a loot Durable Request. A Refusal is an outcome; a Transport Loss stays
/// an error and ends the World Session.
pub(crate) enum LootWindowRequestStatus {
    Applied,
    Refused(LootWindowRefusal),
}

/// How the Module answered a Loot Roll or master-loot Durable Request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LootActionStatus {
    Applied,
    Refused(LootRefusal),
}

/// A loot Refusal as the vanilla client can hear it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LootWindowRefusal {
    /// The Actor is not in the Loot Source's Loot Tag eligibility set.
    LootTagIneligible,
    /// The Loot Source is on another map, in another instance, or beyond loot range.
    OutOfRange,
    /// A Refusal vanilla has no loot-window code for. It is logged at the Store seam and dropped.
    Unanswered,
}

// Only recognized Module tags reach this conversion. The Coordinator maps untagged legacy
// GameObject and skinning refusals to `Unanswered` and propagates strict-core failures.
impl From<LootRefusal> for LootWindowRefusal {
    fn from(refusal: LootRefusal) -> Self {
        match refusal {
            LootRefusal::LootTagIneligible => Self::LootTagIneligible,
            LootRefusal::OutOfRange => Self::OutOfRange,
            LootRefusal::NoLootSource
            | LootRefusal::LooterUnavailable
            | LootRefusal::NothingToLoot
            | LootRefusal::RollUnavailable
            | LootRefusal::NotMasterLooter
            | LootRefusal::RecipientUnavailable
            | LootRefusal::RecipientInventoryFull => Self::Unanswered,
        }
    }
}

impl LootWindowRefusal {
    fn loot_error(self) -> Option<LootMethodError> {
        match self {
            Self::LootTagIneligible => Some(LootMethodError::DidntKill),
            Self::OutOfRange => Some(LootMethodError::TooFar),
            Self::Unanswered => None,
        }
    }
}

/// The target whose loot window is currently open for this world session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct OpenLootState {
    pub(crate) target_guid: Option<u64>,
}

/// The client traffic a loot Refusal earns. An unanswered Refusal stays silent.
fn refusal_outbound(refusal: LootWindowRefusal, target_guid: u64) -> Vec<Outbound> {
    refusal
        .loot_error()
        .map(|loot_error| {
            Outbound::One(ServerOpcodeMessage::SMSG_LOOT_RESPONSE(Box::new(
                codec::build_loot_error_response(target_guid, loot_error),
            )))
        })
        .into_iter()
        .collect()
}

/// An answered Refusal invalidates the open Loot Window; an unanswered one leaves it alone.
fn refusal_transition(
    refusal: LootWindowRefusal,
    current_state: OpenLootState,
    target_guid: u64,
) -> (OpenLootState, Vec<Outbound>) {
    let next_state = match refusal.loot_error() {
        Some(_) => OpenLootState::default(),
        None => current_state,
    };
    (next_state, refusal_outbound(refusal, target_guid))
}

fn log_refusal(session: &ProtocolSession, refusal: LootWindowRefusal) {
    log::debug!(
        "world: loot request refused (account {}): {refusal:?}",
        session.account_id
    );
}

fn finish_loot_action(status: LootActionStatus) {
    if let LootActionStatus::Refused(refusal) = status {
        log::debug!("world: loot action refused: {}", refusal.as_tag());
    }
}

/// Map a client request through the loot-window lifecycle without owning session transport.
pub(crate) struct LootWindow;

impl<St: LootWindowStore + ?Sized> ProtocolFamily<St> for LootWindow {
    fn handle(
        store: &St,
        session: &mut ProtocolSession,
        request: ProtocolRequest,
    ) -> Result<ProtocolReply> {
        let msg = request.message()?;
        let current_state = match &session.state {
            WorldState::InWorld(world) => world.open_loot,
            WorldState::CharSelect => OpenLootState::default(),
        };
        match msg {
            ClientOpcodeMessage::CMSG_GAMEOBJ_USE(request) => {
                let Some(actor) = session.actor() else {
                    return Ok(ProtocolReply::default());
                };
                let target_guid = request.guid.guid();
                if let LootWindowRequestStatus::Refused(refusal) =
                    store.use_gameobject(actor, target_guid)?
                {
                    log_refusal(session, refusal);
                    let (next_state, outbound) =
                        refusal_transition(refusal, current_state, target_guid);
                    return Ok({
                        if let WorldState::InWorld(world) = &mut session.state {
                            world.open_loot = next_state;
                        }
                        ProtocolReply::from(outbound)
                    });
                }
                let items = store.loot_target_items(target_guid, actor.guid())?;
                if items.is_empty() {
                    return Ok(ProtocolReply::default());
                }
                let (opcode, body) = codec::build_loot_response_raw(target_guid, 0, &items);
                Ok({
                    if let WorldState::InWorld(world) = &mut session.state {
                        world.open_loot = OpenLootState {
                            target_guid: Some(target_guid),
                        };
                    }
                    ProtocolReply::from(vec![Outbound::Raw { opcode, body }])
                })
            }
            ClientOpcodeMessage::CMSG_LOOT(request) => {
                let Some(viewer) = session.actor() else {
                    return Ok(ProtocolReply::default());
                };
                let target_guid = request.guid.guid();
                if let LootWindowRequestStatus::Refused(refusal) =
                    store.open_creature_loot(viewer, target_guid)?
                {
                    log_refusal(session, refusal);
                    let (next_state, outbound) =
                        refusal_transition(refusal, current_state, target_guid);
                    return Ok({
                        if let WorldState::InWorld(world) = &mut session.state {
                            world.open_loot = next_state;
                        }
                        ProtocolReply::from(outbound)
                    });
                }
                let money = store.loot_target_money(target_guid)?;
                let items = store.loot_target_items(target_guid, viewer.guid())?;
                if items.is_empty() && money == 0 {
                    // Skinning an empty corpse is opportunistic: a Refusal still shows the empty window.
                    store.skin_corpse(viewer, target_guid)?;
                }
                let (opcode, body) = codec::build_loot_response_raw(target_guid, money, &items);
                Ok({
                    if let WorldState::InWorld(world) = &mut session.state {
                        world.open_loot = OpenLootState {
                            target_guid: Some(target_guid),
                        };
                    }
                    ProtocolReply::from(vec![Outbound::Raw { opcode, body }])
                })
            }
            ClientOpcodeMessage::CMSG_LOOT_MONEY => {
                let (Some(actor), Some(target_guid)) = (session.actor(), current_state.target_guid)
                else {
                    return Ok(ProtocolReply::default());
                };
                let (next_state, outbound) = match store.loot_money(actor, target_guid)? {
                    LootWindowRequestStatus::Applied => (
                        current_state,
                        vec![Outbound::One(ServerOpcodeMessage::SMSG_LOOT_CLEAR_MONEY)],
                    ),
                    LootWindowRequestStatus::Refused(refusal) => {
                        log_refusal(session, refusal);
                        refusal_transition(refusal, current_state, target_guid)
                    }
                };
                Ok({
                    if let WorldState::InWorld(world) = &mut session.state {
                        world.open_loot = next_state;
                    }
                    ProtocolReply::from(outbound)
                })
            }
            ClientOpcodeMessage::CMSG_AUTOSTORE_LOOT_ITEM(request) => {
                let (Some(actor), Some(target_guid)) = (session.actor(), current_state.target_guid)
                else {
                    return Ok(ProtocolReply::default());
                };
                let (next_state, outbound) =
                    match store.take_loot(actor, target_guid, request.item_slot)? {
                        LootWindowRequestStatus::Applied => (
                            current_state,
                            vec![Outbound::One(ServerOpcodeMessage::SMSG_LOOT_REMOVED(
                                codec::build_loot_removed(request.item_slot),
                            ))],
                        ),
                        LootWindowRequestStatus::Refused(refusal) => {
                            log_refusal(session, refusal);
                            refusal_transition(refusal, current_state, target_guid)
                        }
                    };
                Ok({
                    if let WorldState::InWorld(world) = &mut session.state {
                        world.open_loot = next_state;
                    }
                    ProtocolReply::from(outbound)
                })
            }
            ClientOpcodeMessage::CMSG_LOOT_RELEASE(request) => Ok({
                if let WorldState::InWorld(world) = &mut session.state {
                    world.open_loot = OpenLootState::default();
                }
                ProtocolReply::from(vec![Outbound::One(
                    ServerOpcodeMessage::SMSG_LOOT_RELEASE_RESPONSE(Box::new(
                        codec::build_loot_release_response(request.guid.guid()),
                    )),
                )])
            }),
            other => Err(anyhow!("request routed to LootWindow: {other}")),
        }
    }
}

/// Handle loot rolls, general GameObject use and death recovery.
pub(crate) struct Loot;

impl<St: DeathStore + LootRollStore + LootWindowStore + NpcStore + ShardRoutingStore + ?Sized>
    ProtocolFamily<St> for Loot
{
    fn handle(
        store: &St,
        session: &mut ProtocolSession,
        request: ProtocolRequest,
    ) -> Result<ProtocolReply> {
        let msg = request.message()?;
        let mut outbound = Vec::new();
        let actor = session.actor();
        match msg {
            // Group loot methods: a need/greed vote, and the master looter's
            // explicit assign. Both are per-action — a rejection (no roll open, already voted, not the
            // master) is logged + ignored rather than tearing the session; the live vote/winner/master
            // packets ride the `game_group_event` roll relay (`stdb/subscriptions.rs`), not a direct
            // reply here.
            ClientOpcodeMessage::CMSG_LOOT_ROLL(c) => {
                let corpse_guid = c.item.guid();
                let vote = c.vote.as_int();
                // Unsharded, `loot::run_vote` is exactly the call above (`store.loot_roll`);
                // sharded, it routes to realm-core instead, so the Actor it votes as must be the one
                // THIS socket authenticated with, never a literal from the packet.
                let actor = actor.ok_or_else(|| anyhow!("CMSG_LOOT_ROLL before world entry"))?;
                finish_loot_action(loot::run_vote(
                    store,
                    actor,
                    corpse_guid,
                    c.item_slot,
                    vote,
                )?);
            }
            ClientOpcodeMessage::CMSG_LOOT_MASTER_GIVE(c) => {
                let corpse_guid = c.loot.guid();
                let target_guid = c.player.guid();
                let actor =
                    actor.ok_or_else(|| anyhow!("CMSG_LOOT_MASTER_GIVE before world entry"))?;
                finish_loot_action(store.loot_master_give(
                    actor,
                    corpse_guid,
                    c.slot_id,
                    target_guid,
                )?);
            }
            ClientOpcodeMessage::CMSG_GAMEOBJ_USE(request) => {
                let Some(actor) = actor else {
                    return Ok(outbound.into());
                };
                let target_guid = request.guid.guid();
                match store.use_gameobject(actor, target_guid)? {
                    LootWindowRequestStatus::Applied => {
                        let items = store.loot_target_items(target_guid, actor.guid())?;
                        if !items.is_empty() {
                            if let WorldState::InWorld(iw) = &mut session.state {
                                iw.open_loot = OpenLootState {
                                    target_guid: Some(target_guid),
                                };
                            }
                            let (opcode, body) =
                                codec::build_loot_response_raw(target_guid, 0, &items);
                            outbound.push(Outbound::Raw { opcode, body });
                        }
                    }
                    LootWindowRequestStatus::Refused(refusal) => {
                        if refusal.loot_error().is_some() {
                            if let WorldState::InWorld(iw) = &mut session.state {
                                iw.open_loot = OpenLootState::default();
                            }
                        }
                        outbound.extend(refusal_outbound(refusal, target_guid));
                    }
                }
            }
            // Enter an area trigger (CMSG_AREATRIGGER): the client fires this when the player physically
            // walks into a trigger zone (e.g. a mine for an "explore" quest). The module credits any active
            // explore quest tied to the trigger id. A transient/no-match result is logged + ignored.
            ClientOpcodeMessage::CMSG_AREATRIGGER(a) => {
                if let Some(actor) = session.actor() {
                    super::trainer::settle_per_action(
                        "enter_areatrigger",
                        session.account_id,
                        store.enter_areatrigger(actor, a.trigger_id),
                    )?;
                }
            }
            // Gameobject template query (CMSG_GAMEOBJECT_QUERY): the client asks for a GO's name/type/display
            // before it renders/interacts. Reply with the template, or the not-found form.
            ClientOpcodeMessage::CMSG_GAMEOBJECT_QUERY(q) => {
                let tmpl = store.gameobject_template(q.entry_id)?;
                outbound.push(Outbound::One(
                    ServerOpcodeMessage::SMSG_GAMEOBJECT_QUERY_RESPONSE(Box::new(
                        codec::build_gameobject_query_response(q.entry_id, tmpl.as_ref()),
                    )),
                ));
            }
            // Release Spirit after death. The client sends this (empty body) when the player
            // clicks Release on the death screen. Revive in place at full health; the restored health
            // replicates via the on_update VALUES relay and the client leaves the death screen.
            // SMSG_CORPSE_RECLAIM_DELAY is now relay-driven (the escalated per-corpse
            // delay, not a flat 30s) — see `on_corpse_insert` in `stdb/subscriptions.rs`, which fires off
            // the SAME `game_corpse` insert `repop`'s reducer call just caused, so no explicit send here.
            ClientOpcodeMessage::CMSG_REPOP_REQUEST => {
                if let Some(actor) = actor {
                    ignore_refusal("repop", session.account_id, store.repop(actor))?;
                }
            }
            // Corpse location query: the client asks where the player's corpse is to draw the
            // map marker + offer "Reclaim Corpse" near it. Reply with the corpse's position, or NotFound.
            ClientOpcodeMessage::MSG_CORPSE_QUERY => {
                if let WorldState::InWorld(iw) = &session.state {
                    let loc = store.corpse_location(iw.self_guid)?;
                    outbound.push(Outbound::One(ServerOpcodeMessage::MSG_CORPSE_QUERY(
                        Box::new(codec::build_corpse_query_response(loc)?),
                    )));
                }
            }
            // Reclaim your corpse: the ghost, near its corpse and past the 30s delay, resurrects
            // at 50%. The module validates ownership/ghost/range/delay; a failure (too far, too soon, not
            // a ghost) is expected and silently ignored — the client just stays a ghost.
            ClientOpcodeMessage::CMSG_RECLAIM_CORPSE(r) => {
                if let Some(actor) = actor {
                    let result = store.reclaim_corpse(actor, r.guid.guid());
                    ignore_refusal("reclaim_corpse", session.account_id, result)?;
                }
            }
            // Resurrection accept-prompt response: the dead player answered the SMSG_RESURRECT_REQUEST
            // offer. `status` is vanilla's accept(1)/decline(0) byte; the offer's guid is ignored (mirrors
            // `reclaim_corpse`'s own-corpse derivation — the module resolves the pending offer from the
            // CALLER via `ctx.sender()`, never the wire guid). A failure (no pending offer — already
            // answered/lapsed) is expected and silently ignored.
            ClientOpcodeMessage::CMSG_RESURRECT_RESPONSE(r) => {
                if let Some(actor) = actor {
                    let result = store.resurrect_response(actor, r.status != 0);
                    ignore_refusal("resurrect_response", session.account_id, result)?;
                }
            }
            // The death dialog's second button: use the Self-Resurrection Option the Module wrote into
            // PLAYER_SELF_RES_SPELL. The revive replicates through the entity VALUES relay. A Refusal
            // (already used, already alive) is expected after a race and sends nothing.
            ClientOpcodeMessage::CMSG_SELF_RES => {
                if let Some(actor) = actor {
                    ignore_refusal(
                        "self_resurrect",
                        session.account_id,
                        store.self_resurrect(actor),
                    )?;
                }
            }
            // Spirit-Healer resurrection: a ghost activated the graveyard Spirit Healer (npc_flags
            // SPIRITHEALER). The module res's in place at 50% + applies Resurrection Sickness; on success
            // reply with SMSG_SPIRIT_HEALER_CONFIRM (echoing the healer's guid) so the client closes the
            // dialog. The res itself replicates via the entity VALUES relay (health > 0 + cleared ghost
            // bits), exactly like reclaim_corpse. A failure (not a ghost) is per-action — log + ignore.
            ClientOpcodeMessage::CMSG_SPIRIT_HEALER_ACTIVATE(s) => {
                if let Some(actor) = actor {
                    let result = store.spirit_healer_res(actor, s.guid.guid());
                    let revived = result.is_ok();
                    ignore_refusal("spirit_healer_res", session.account_id, result)?;
                    if revived {
                        outbound.push(Outbound::One(
                            ServerOpcodeMessage::SMSG_SPIRIT_HEALER_CONFIRM(
                                SMSG_SPIRIT_HEALER_CONFIRM { guid: s.guid },
                            ),
                        ));
                    }
                }
            }
            other => return Err(anyhow!("request routed to Loot: {other}")),
        }
        Ok(outbound.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use wow_world_messages::vanilla::{
        LootMethodError, SMSG_LOOT_RESPONSE_LootMethod, CMSG_AUTOSTORE_LOOT_ITEM, CMSG_GAMEOBJ_USE,
        CMSG_LOOT, CMSG_LOOT_RELEASE,
    };
    use wow_world_messages::Guid;

    use crate::stdb::ReducerCallError;

    #[derive(Default)]
    struct InMemoryLootWindow {
        money: u32,
        items_by_viewer: HashMap<u64, Vec<codec::LootItemView>>,
        money_reads: Mutex<Vec<u64>>,
        item_reads: Mutex<Vec<(u64, u64)>>,
        use_requests: Mutex<Vec<(u64, u64)>>,
        open_requests: Mutex<Vec<(u64, u64)>>,
        operations: Mutex<Vec<&'static str>>,
        skin_requests: Mutex<Vec<(u64, u64)>>,
        money_take_requests: Mutex<Vec<(u64, u64)>>,
        item_take_requests: Mutex<Vec<(u64, u64, u8)>>,
        skin_refusal: Option<LootWindowRefusal>,
        skin_failure: Option<Failure>,
        use_refusal: Option<LootWindowRefusal>,
        use_failure: Option<Failure>,
        open_refusal: Option<LootWindowRefusal>,
        open_failure: Option<Failure>,
        money_take_refusal: Option<LootWindowRefusal>,
        money_take_failure: Option<Failure>,
        item_take_refusal: Option<LootWindowRefusal>,
        item_take_failure: Option<Failure>,
    }

    /// How a Store call fails before the Store has an answer to hand the handler.
    #[derive(Clone, Copy)]
    enum Failure {
        TransportLost,
        /// The Store returns a Module refusal it does not decode, such as a boundary failure.
        Refused(&'static str),
    }

    fn request_status(
        operation: &str,
        refusal: Option<LootWindowRefusal>,
        failure: Option<Failure>,
    ) -> Result<LootWindowRequestStatus> {
        match (failure, refusal) {
            (Some(Failure::TransportLost), _) => {
                Err(ReducerCallError::transport_lost(operation).into())
            }
            (Some(Failure::Refused(reason)), _) => {
                Err(ReducerCallError::refused(operation, reason).into())
            }
            (None, Some(refusal)) => Ok(LootWindowRequestStatus::Refused(refusal)),
            (None, None) => Ok(LootWindowRequestStatus::Applied),
        }
    }

    impl LootWindowStore for InMemoryLootWindow {
        fn loot_target_money(&self, target_guid: u64) -> Result<u32> {
            self.money_reads.lock().unwrap().push(target_guid);
            Ok(self.money)
        }

        fn loot_target_items(
            &self,
            target_guid: u64,
            viewer_guid: u64,
        ) -> Result<Vec<codec::LootItemView>> {
            self.operations.lock().unwrap().push("read generated loot");
            self.item_reads
                .lock()
                .unwrap()
                .push((target_guid, viewer_guid));
            Ok(self
                .items_by_viewer
                .get(&viewer_guid)
                .cloned()
                .unwrap_or_default())
        }

        fn use_gameobject(
            &self,
            actor: Actor,
            target_guid: u64,
        ) -> Result<LootWindowRequestStatus> {
            self.operations.lock().unwrap().push("use gameobject");
            self.use_requests
                .lock()
                .unwrap()
                .push((actor.guid(), target_guid));
            request_status("gw_use_gameobject", self.use_refusal, self.use_failure)
        }

        fn open_creature_loot(
            &self,
            actor: Actor,
            corpse_guid: u64,
        ) -> Result<LootWindowRequestStatus> {
            self.operations.lock().unwrap().push("open creature loot");
            self.open_requests
                .lock()
                .unwrap()
                .push((actor.guid(), corpse_guid));
            request_status(
                "gw_open_creature_loot",
                self.open_refusal,
                self.open_failure,
            )
        }

        fn skin_corpse(&self, actor: Actor, target_guid: u64) -> Result<LootWindowRequestStatus> {
            self.skin_requests
                .lock()
                .unwrap()
                .push((actor.guid(), target_guid));
            request_status("gw_skin", self.skin_refusal, self.skin_failure)
        }

        fn loot_money(&self, actor: Actor, target_guid: u64) -> Result<LootWindowRequestStatus> {
            self.money_take_requests
                .lock()
                .unwrap()
                .push((actor.guid(), target_guid));
            request_status(
                "gw_loot_money",
                self.money_take_refusal,
                self.money_take_failure,
            )
        }

        fn take_loot(
            &self,
            actor: Actor,
            target_guid: u64,
            loot_slot: u8,
        ) -> Result<LootWindowRequestStatus> {
            self.item_take_requests
                .lock()
                .unwrap()
                .push((actor.guid(), target_guid, loot_slot));
            request_status(
                "gw_take_loot",
                self.item_take_refusal,
                self.item_take_failure,
            )
        }
    }

    fn assert_transport_loss(error: &anyhow::Error) {
        assert_eq!(
            crate::stdb::classify(error),
            crate::stdb::DurableFailure::TransportLoss
        );
    }

    fn run_window<St: LootWindowStore + ?Sized>(
        store: &St,
        mut session: ProtocolSession,
        current: OpenLootState,
        message: ClientOpcodeMessage,
    ) -> Result<(OpenLootState, Vec<Outbound>)> {
        if let WorldState::InWorld(world) = &mut session.state {
            world.open_loot = current;
        }
        let reply = LootWindow::handle(store, &mut session, message.into())?;
        let state = match &session.state {
            WorldState::InWorld(world) => world.open_loot,
            WorldState::CharSelect => OpenLootState::default(),
        };
        Ok((state, reply.outbound))
    }

    fn session() -> ProtocolSession {
        ProtocolSession::in_world(7, 42)
    }

    fn open_creature(target_guid: u64) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_LOOT(CMSG_LOOT {
            guid: Guid::new(target_guid),
        })
    }

    fn open_chest(target_guid: u64) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_GAMEOBJ_USE(CMSG_GAMEOBJ_USE {
            guid: Guid::new(target_guid),
        })
    }

    fn assert_loot_error(outbound: &[Outbound], corpse_guid: u64, expected: LootMethodError) {
        let [Outbound::One(ServerOpcodeMessage::SMSG_LOOT_RESPONSE(response))] = outbound else {
            panic!("expected one loot error response")
        };
        assert_eq!(response.guid.guid(), corpse_guid);
        assert_eq!(
            response.loot_method,
            SMSG_LOOT_RESPONSE_LootMethod::ErrorX {
                loot_error: expected,
            }
        );
        assert_eq!(response.gold.as_int(), 0);
        assert!(response.items.is_empty());
    }

    fn assert_didnt_kill(outbound: &[Outbound], corpse_guid: u64) {
        assert_loot_error(outbound, corpse_guid, LootMethodError::DidntKill);
    }

    #[test]
    fn creature_open_without_a_player_context_does_not_touch_loot_state() {
        let store = InMemoryLootWindow::default();
        let current_state = OpenLootState {
            target_guid: Some(11),
        };

        let outcome = run_window(
            &store,
            ProtocolSession::new(7, "TESTER".into()),
            current_state,
            open_creature(60),
        )
        .unwrap();

        assert!(matches!(
            outcome,
            (next_state, outbound) if next_state == OpenLootState::default() && outbound.is_empty()
        ));
        assert!(store.money_reads.lock().unwrap().is_empty());
        assert!(store.item_reads.lock().unwrap().is_empty());
        assert!(store.skin_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn creature_open_with_a_zero_player_guid_does_not_touch_loot_state() {
        let store = InMemoryLootWindow::default();
        let current_state = OpenLootState {
            target_guid: Some(11),
        };

        let outcome = run_window(
            &store,
            ProtocolSession::in_world(7, 0),
            current_state,
            open_creature(60),
        )
        .unwrap();

        assert!(matches!(
            outcome,
            (next_state, outbound) if next_state == current_state && outbound.is_empty()
        ));
        assert!(store.money_reads.lock().unwrap().is_empty());
        assert!(store.item_reads.lock().unwrap().is_empty());
        assert!(store.skin_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn chest_use_precedes_generated_loot_and_opens_the_shared_window() {
        let mut store = InMemoryLootWindow::default();
        store.items_by_viewer.insert(42, vec![(4, 117, 2, 321, 0)]);
        let current_state = OpenLootState {
            target_guid: Some(11),
        };

        let outcome = run_window(&store, session(), current_state, open_chest(90)).unwrap();

        let (next_state, outbound) = outcome;
        assert_eq!(next_state.target_guid, Some(90));

        let [Outbound::Raw { opcode, body }] = outbound.as_slice() else {
            panic!("expected one raw loot window")
        };
        assert_eq!(*opcode, 0x0160);
        assert_eq!(&body[0..8], &90u64.to_le_bytes());
        assert_eq!(&body[9..13], &0u32.to_le_bytes());
        assert_eq!(body[13], 1);
        assert_eq!(
            store.operations.lock().unwrap().as_slice(),
            &["use gameobject", "read generated loot"]
        );
        assert_eq!(store.use_requests.lock().unwrap().as_slice(), &[(42, 90)]);
        assert_eq!(store.item_reads.lock().unwrap().as_slice(), &[(90, 42)]);
        assert!(store.open_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn successful_empty_chest_use_keeps_the_current_window_state() {
        let store = InMemoryLootWindow::default();
        let current_state = OpenLootState {
            target_guid: Some(11),
        };

        let outcome = run_window(&store, session(), current_state, open_chest(90)).unwrap();

        assert!(matches!(
            outcome,
            (next_state, outbound) if next_state == current_state && outbound.is_empty()
        ));
        assert_eq!(
            store.operations.lock().unwrap().as_slice(),
            &["use gameobject", "read generated loot"]
        );
    }

    #[test]
    fn refused_chest_use_does_not_read_loot_or_change_window_state() {
        let store = InMemoryLootWindow {
            use_refusal: Some(LootWindowRefusal::Unanswered),
            ..Default::default()
        };
        let current_state = OpenLootState {
            target_guid: Some(11),
        };

        let outcome = run_window(&store, session(), current_state, open_chest(90)).unwrap();

        assert!(matches!(
            outcome,
            (next_state, outbound) if next_state == current_state && outbound.is_empty()
        ));
        assert_eq!(
            store.operations.lock().unwrap().as_slice(),
            &["use gameobject"]
        );
        assert!(store.item_reads.lock().unwrap().is_empty());
    }

    #[test]
    fn fatal_chest_use_failure_propagates_without_reading_loot() {
        let store = InMemoryLootWindow {
            use_failure: Some(Failure::TransportLost),
            ..Default::default()
        };
        let current_state = OpenLootState {
            target_guid: Some(11),
        };

        let error = run_window(&store, session(), current_state, open_chest(90))
            .err()
            .expect("fatal GameObject failure was handled");

        assert_transport_loss(&error);
        assert_eq!(
            store.operations.lock().unwrap().as_slice(),
            &["use gameobject"]
        );
        assert!(store.item_reads.lock().unwrap().is_empty());
    }

    #[test]
    fn missing_actor_chest_failure_propagates_without_reading_loot() {
        let failure = lyracore_shared::loot::LootBoundaryFailure::MissingActor.as_tag();
        let store = InMemoryLootWindow {
            use_failure: Some(Failure::Refused(failure)),
            ..Default::default()
        };

        let error = run_window(
            &store,
            session(),
            OpenLootState {
                target_guid: Some(11),
            },
            open_chest(90),
        )
        .err()
        .expect("missing Actor failure was handled");

        assert!(error.to_string().contains(failure));
        assert_eq!(
            store.operations.lock().unwrap().as_slice(),
            &["use gameobject"]
        );
        assert!(store.item_reads.lock().unwrap().is_empty());
    }

    #[test]
    fn creature_open_ownership_refusal_closes_the_window_without_loot_reads() {
        let store = InMemoryLootWindow {
            open_refusal: Some(LootWindowRefusal::LootTagIneligible),
            ..Default::default()
        };

        let outcome = run_window(
            &store,
            session(),
            OpenLootState {
                target_guid: Some(11),
            },
            open_creature(60),
        )
        .unwrap();

        let (next_state, outbound) = outcome;
        assert_eq!(next_state, OpenLootState::default());

        assert_didnt_kill(&outbound, 60);
        assert_eq!(store.open_requests.lock().unwrap().as_slice(), &[(42, 60)]);
        assert!(store.money_reads.lock().unwrap().is_empty());
        assert!(store.item_reads.lock().unwrap().is_empty());
        assert!(store.skin_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn creature_open_transport_failure_propagates_without_loot_reads() {
        let store = InMemoryLootWindow {
            open_failure: Some(Failure::TransportLost),
            ..Default::default()
        };

        let error = run_window(
            &store,
            session(),
            OpenLootState::default(),
            open_creature(60),
        )
        .err()
        .expect("fatal creature-open failure was handled");

        assert_transport_loss(&error);
        assert!(store.money_reads.lock().unwrap().is_empty());
        assert!(store.item_reads.lock().unwrap().is_empty());
    }

    #[test]
    fn unrelated_creature_open_refusal_keeps_the_window_without_loot_reads() {
        let store = InMemoryLootWindow {
            open_refusal: Some(LootWindowRefusal::Unanswered),
            ..Default::default()
        };
        let current_state = OpenLootState {
            target_guid: Some(11),
        };

        let outcome = run_window(&store, session(), current_state, open_creature(60)).unwrap();

        assert!(matches!(
            outcome,
            (next_state, outbound) if next_state == current_state && outbound.is_empty()
        ));
        assert!(store.money_reads.lock().unwrap().is_empty());
        assert!(store.item_reads.lock().unwrap().is_empty());
    }

    fn release(target_guid: u64) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_LOOT_RELEASE(CMSG_LOOT_RELEASE {
            guid: Guid::new(target_guid),
        })
    }

    fn dispatch_release(current_state: OpenLootState, request_target: u64) -> OpenLootState {
        let outcome = run_window(
            &InMemoryLootWindow::default(),
            session(),
            current_state,
            release(request_target),
        )
        .unwrap();
        let (next_state, outbound) = outcome;

        assert!(matches!(
            outbound.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_LOOT_RELEASE_RESPONSE(response))]
                if response.guid.guid() == request_target
        ));
        next_state
    }

    #[test]
    fn money_take_success_uses_the_open_target_and_only_clears_money() {
        let store = InMemoryLootWindow::default();
        let current_state = OpenLootState {
            target_guid: Some(60),
        };

        let outcome = run_window(
            &store,
            session(),
            current_state,
            ClientOpcodeMessage::CMSG_LOOT_MONEY,
        )
        .unwrap();

        assert!(matches!(
            outcome,
            (next_state, outbound) if next_state == current_state && matches!(outbound.as_slice(), [Outbound::One(ServerOpcodeMessage::SMSG_LOOT_CLEAR_MONEY)])
        ));
        assert_eq!(
            store.money_take_requests.lock().unwrap().as_slice(),
            &[(42, 60)]
        );
    }

    #[test]
    fn money_take_refusal_keeps_the_window_without_a_false_clear() {
        let store = InMemoryLootWindow {
            money_take_refusal: Some(LootWindowRefusal::Unanswered),
            ..Default::default()
        };
        let current_state = OpenLootState {
            target_guid: Some(60),
        };

        let outcome = run_window(
            &store,
            session(),
            current_state,
            ClientOpcodeMessage::CMSG_LOOT_MONEY,
        )
        .unwrap();

        assert!(matches!(
            outcome,
            (next_state, outbound) if next_state == current_state && outbound.is_empty()
        ));
        assert_eq!(
            store.money_take_requests.lock().unwrap().as_slice(),
            &[(42, 60)]
        );
    }

    #[test]
    fn every_module_refusal_has_one_client_result_code() {
        let expected = |refusal| match refusal {
            LootRefusal::LootTagIneligible => Some(LootMethodError::DidntKill),
            LootRefusal::OutOfRange => Some(LootMethodError::TooFar),
            LootRefusal::NoLootSource
            | LootRefusal::LooterUnavailable
            | LootRefusal::NothingToLoot
            | LootRefusal::RollUnavailable
            | LootRefusal::NotMasterLooter
            | LootRefusal::RecipientUnavailable
            | LootRefusal::RecipientInventoryFull => None,
        };
        let open_window = OpenLootState {
            target_guid: Some(60),
        };

        for refusal in LootRefusal::ALL {
            let loot_error = expected(refusal);

            for (message, store) in [
                (
                    open_creature(60),
                    InMemoryLootWindow {
                        open_refusal: Some(refusal.into()),
                        ..Default::default()
                    },
                ),
                (
                    open_chest(60),
                    InMemoryLootWindow {
                        use_refusal: Some(refusal.into()),
                        ..Default::default()
                    },
                ),
                (
                    ClientOpcodeMessage::CMSG_LOOT_MONEY,
                    InMemoryLootWindow {
                        money_take_refusal: Some(refusal.into()),
                        ..Default::default()
                    },
                ),
                (
                    ClientOpcodeMessage::CMSG_AUTOSTORE_LOOT_ITEM(CMSG_AUTOSTORE_LOOT_ITEM {
                        item_slot: 3,
                    }),
                    InMemoryLootWindow {
                        item_take_refusal: Some(refusal.into()),
                        ..Default::default()
                    },
                ),
            ] {
                let outcome = run_window(&store, session(), open_window, message).unwrap();
                let (next_state, outbound) = outcome;
                match loot_error {
                    // An answered Refusal invalidates the Loot Window the client is showing.
                    Some(loot_error) => {
                        assert_loot_error(&outbound, 60, loot_error);
                        assert_eq!(next_state, OpenLootState::default(), "{refusal:?}");
                    }
                    None => {
                        assert!(outbound.is_empty(), "{refusal:?}");
                        assert_eq!(next_state, open_window, "{refusal:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn money_take_ownership_refusal_closes_the_window_and_returns_didnt_kill() {
        let store = InMemoryLootWindow {
            money_take_refusal: Some(LootWindowRefusal::LootTagIneligible),
            ..Default::default()
        };

        let outcome = run_window(
            &store,
            session(),
            OpenLootState {
                target_guid: Some(60),
            },
            ClientOpcodeMessage::CMSG_LOOT_MONEY,
        )
        .unwrap();

        let (next_state, outbound) = outcome;
        assert_eq!(next_state, OpenLootState::default());

        assert_didnt_kill(&outbound, 60);
        assert_eq!(
            store.money_take_requests.lock().unwrap().as_slice(),
            &[(42, 60)]
        );
    }

    #[test]
    fn money_take_without_an_open_target_has_no_operation_or_outbound() {
        let store = InMemoryLootWindow::default();

        let outcome = run_window(
            &store,
            session(),
            OpenLootState::default(),
            ClientOpcodeMessage::CMSG_LOOT_MONEY,
        )
        .unwrap();

        assert!(matches!(
            outcome,
            (OpenLootState { target_guid: None }, outbound) if outbound.is_empty()
        ));
        assert!(store.money_take_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn item_take_success_uses_the_open_target_and_only_removes_the_requested_slot() {
        let store = InMemoryLootWindow::default();
        let current_state = OpenLootState {
            target_guid: Some(75),
        };

        let outcome = run_window(
            &store,
            session(),
            current_state,
            ClientOpcodeMessage::CMSG_AUTOSTORE_LOOT_ITEM(CMSG_AUTOSTORE_LOOT_ITEM {
                item_slot: 3,
            }),
        )
        .unwrap();

        assert!(matches!(
            outcome,
            (next_state, outbound) if next_state == current_state && matches!(outbound.as_slice(), [Outbound::One(ServerOpcodeMessage::SMSG_LOOT_REMOVED(removed))] if removed.slot == 3)
        ));
        assert_eq!(
            store.item_take_requests.lock().unwrap().as_slice(),
            &[(42, 75, 3)]
        );
    }

    #[test]
    fn item_take_refusal_keeps_the_window_without_a_false_slot_removal() {
        let store = InMemoryLootWindow {
            item_take_refusal: Some(LootWindowRefusal::Unanswered),
            ..Default::default()
        };
        let current_state = OpenLootState {
            target_guid: Some(75),
        };

        let outcome = run_window(
            &store,
            session(),
            current_state,
            ClientOpcodeMessage::CMSG_AUTOSTORE_LOOT_ITEM(CMSG_AUTOSTORE_LOOT_ITEM {
                item_slot: 3,
            }),
        )
        .unwrap();

        assert!(matches!(
            outcome,
            (next_state, outbound) if next_state == current_state && outbound.is_empty()
        ));
        assert_eq!(
            store.item_take_requests.lock().unwrap().as_slice(),
            &[(42, 75, 3)]
        );
    }

    #[test]
    fn item_take_ownership_refusal_closes_the_window_and_returns_didnt_kill() {
        let store = InMemoryLootWindow {
            item_take_refusal: Some(LootWindowRefusal::LootTagIneligible),
            ..Default::default()
        };

        let outcome = run_window(
            &store,
            session(),
            OpenLootState {
                target_guid: Some(75),
            },
            ClientOpcodeMessage::CMSG_AUTOSTORE_LOOT_ITEM(CMSG_AUTOSTORE_LOOT_ITEM {
                item_slot: 3,
            }),
        )
        .unwrap();

        let (next_state, outbound) = outcome;
        assert_eq!(next_state, OpenLootState::default());

        assert_didnt_kill(&outbound, 75);
        assert_eq!(
            store.item_take_requests.lock().unwrap().as_slice(),
            &[(42, 75, 3)]
        );
    }

    #[test]
    fn item_take_without_an_open_target_has_no_operation_or_outbound() {
        let store = InMemoryLootWindow::default();

        let outcome = run_window(
            &store,
            session(),
            OpenLootState::default(),
            ClientOpcodeMessage::CMSG_AUTOSTORE_LOOT_ITEM(CMSG_AUTOSTORE_LOOT_ITEM {
                item_slot: 3,
            }),
        )
        .unwrap();

        assert!(matches!(
            outcome,
            (OpenLootState { target_guid: None }, outbound) if outbound.is_empty()
        ));
        assert!(store.item_take_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn transport_loss_on_a_money_or_item_take_propagates() {
        let store = InMemoryLootWindow {
            money_take_failure: Some(Failure::TransportLost),
            item_take_failure: Some(Failure::TransportLost),
            ..Default::default()
        };
        let open = OpenLootState {
            target_guid: Some(60),
        };

        for request in [ClientOpcodeMessage::CMSG_LOOT_MONEY, take_item(3)] {
            let error = run_window(&store, session(), open, request)
                .err()
                .expect("transport loss was handled");

            assert_transport_loss(&error);
        }
    }

    #[test]
    fn creature_open_returns_the_viewers_current_loot_and_replaces_the_open_target() {
        let mut store = InMemoryLootWindow {
            money: 25,
            ..Default::default()
        };
        store.items_by_viewer.insert(42, vec![(3, 2589, 5, 200, 0)]);

        let outcome = run_window(
            &store,
            session(),
            OpenLootState {
                target_guid: Some(11),
            },
            open_creature(60),
        )
        .unwrap();

        let (next_state, outbound) = outcome;
        assert_eq!(next_state.target_guid, Some(60));

        let [Outbound::Raw { opcode, body }] = outbound.as_slice() else {
            panic!("expected one raw loot window")
        };
        assert_eq!(*opcode, 0x0160);
        assert_eq!(&body[0..8], &60u64.to_le_bytes());
        assert_eq!(&body[9..13], &25u32.to_le_bytes());
        assert_eq!(body[13], 1);
        assert_eq!(body[14], 3);
        assert_eq!(&body[15..19], &2589u32.to_le_bytes());
        assert_eq!(&body[19..23], &5u32.to_le_bytes());
        assert_eq!(&body[23..27], &200u32.to_le_bytes());
        assert_eq!(store.money_reads.lock().unwrap().as_slice(), &[60]);
        assert_eq!(store.item_reads.lock().unwrap().as_slice(), &[(60, 42)]);
        assert_eq!(store.open_requests.lock().unwrap().as_slice(), &[(42, 60)]);
        assert!(store.skin_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn two_viewers_of_one_creature_receive_their_own_visible_items() {
        let mut store = InMemoryLootWindow::default();
        store.items_by_viewer.insert(42, vec![(0, 6948, 1, 100, 0)]);
        store.items_by_viewer.insert(43, vec![(2, 2589, 5, 200, 0)]);

        let first = run_window(
            &store,
            session(),
            OpenLootState::default(),
            open_creature(60),
        )
        .unwrap();
        let second = run_window(
            &store,
            ProtocolSession::in_world(7, 43),
            OpenLootState::default(),
            open_creature(60),
        )
        .unwrap();

        let (_, first_outbound) = first;
        let (_, second_outbound) = second;
        let [Outbound::Raw {
            body: first_body, ..
        }] = first_outbound.as_slice()
        else {
            panic!("expected the first viewer's loot window")
        };
        let [Outbound::Raw {
            body: second_body, ..
        }] = second_outbound.as_slice()
        else {
            panic!("expected the second viewer's loot window")
        };
        assert_eq!(&first_body[15..19], &6948u32.to_le_bytes());
        assert_eq!(&second_body[15..19], &2589u32.to_le_bytes());
        assert_eq!(
            store.item_reads.lock().unwrap().as_slice(),
            &[(60, 42), (60, 43)]
        );
    }

    #[test]
    fn release_clears_the_open_target_and_acknowledges_the_request_target() {
        let next_state = dispatch_release(
            OpenLootState {
                target_guid: Some(60),
            },
            91,
        );
        assert_eq!(next_state, OpenLootState::default());
    }

    #[test]
    fn duplicate_release_keeps_state_empty_and_is_acknowledged() {
        let next_state = dispatch_release(
            OpenLootState {
                target_guid: Some(60),
            },
            60,
        );
        let next_state = dispatch_release(next_state, 60);

        assert_eq!(next_state, OpenLootState::default());
    }

    #[test]
    fn fully_empty_creature_attempts_skinning_and_returns_an_empty_window() {
        let store = InMemoryLootWindow::default();

        let outcome = run_window(
            &store,
            session(),
            OpenLootState::default(),
            open_creature(61),
        )
        .unwrap();

        let (next_state, outbound) = outcome;
        assert_eq!(next_state.target_guid, Some(61));

        let [Outbound::Raw { opcode, body }] = outbound.as_slice() else {
            panic!("expected one raw loot window")
        };
        assert_eq!(*opcode, 0x0160);
        assert_eq!(&body[0..8], &61u64.to_le_bytes());
        assert_eq!(&body[9..13], &0u32.to_le_bytes());
        assert_eq!(body[13], 0);
        assert_eq!(store.skin_requests.lock().unwrap().as_slice(), &[(42, 61)]);
    }

    #[test]
    fn skinning_refusal_is_a_handled_empty_window() {
        let store = InMemoryLootWindow {
            skin_refusal: Some(LootWindowRefusal::Unanswered),
            ..Default::default()
        };

        let outcome = run_window(
            &store,
            session(),
            OpenLootState::default(),
            open_creature(61),
        )
        .unwrap();

        assert!(matches!(
            outcome,
            (OpenLootState {
                    target_guid: Some(61)
                }, outbound) if matches!(outbound.as_slice(), [Outbound::Raw { opcode: 0x0160, body }] if body[13] == 0)
        ));
    }

    #[test]
    fn transport_loss_on_skinning_propagates() {
        let store = InMemoryLootWindow {
            skin_failure: Some(Failure::TransportLost),
            ..Default::default()
        };

        let error = run_window(
            &store,
            session(),
            OpenLootState::default(),
            open_creature(61),
        )
        .err()
        .expect("transport loss was handled");

        assert_transport_loss(&error);
    }

    /// Run one request as the session does: apply the state transition, return the client traffic.
    fn run(
        store: &InMemoryLootWindow,
        state: &mut OpenLootState,
        msg: ClientOpcodeMessage,
    ) -> Vec<Outbound> {
        let (next_state, outbound) = run_window(store, session(), *state, msg).unwrap();
        *state = next_state;
        outbound
    }

    fn take_money() -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_LOOT_MONEY
    }

    fn take_item(item_slot: u8) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_AUTOSTORE_LOOT_ITEM(CMSG_AUTOSTORE_LOOT_ITEM { item_slot })
    }

    /// `(slot, item_id, count, display_id)` of item `index` in a raw loot window body: the header is
    /// 8 guid + 1 method + 4 money + 1 count bytes, then 22 bytes per item.
    fn window_item(body: &[u8], index: usize) -> (u8, u32, u32, u32) {
        let base = 14 + index * 22;
        let word =
            |at: usize| u32::from_le_bytes(body[base + at..base + at + 4].try_into().unwrap());
        (body[base], word(1), word(5), word(9))
    }

    fn assert_clear_money(outbound: &[Outbound]) {
        // A solo looter gets no SMSG_LOOT_MONEY_NOTIFY: the client prints its own copper line.
        assert!(matches!(
            outbound,
            [Outbound::One(ServerOpcodeMessage::SMSG_LOOT_CLEAR_MONEY)]
        ));
    }

    #[test]
    fn chest_use_then_item_take_removes_the_slot_from_the_tracked_chest() {
        let mut store = InMemoryLootWindow::default();
        store.items_by_viewer.insert(42, vec![(4, 117, 2, 321, 0)]);
        let mut state = OpenLootState::default();

        let opened = run(&store, &mut state, open_chest(90));
        let [Outbound::Raw { opcode, body }] = opened.as_slice() else {
            panic!("expected one raw loot window")
        };
        assert_eq!(*opcode, 0x0160);
        assert_eq!(&body[0..8], &90u64.to_le_bytes());
        assert_eq!(window_item(body, 0), (4, 117, 2, 321));
        assert_eq!(state.target_guid, Some(90));

        let taken = run(&store, &mut state, take_item(4));
        assert!(matches!(
            taken.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_LOOT_REMOVED(removed))] if removed.slot == 4
        ));
        assert_eq!(
            store.item_take_requests.lock().unwrap().as_slice(),
            &[(42, 90, 4)]
        );
    }

    #[test]
    fn corpse_with_money_opens_unskinned_and_money_take_loots_the_tracked_corpse() {
        let store = InMemoryLootWindow {
            money: 25,
            ..Default::default()
        };
        let mut state = OpenLootState::default();

        let opened = run(&store, &mut state, open_creature(60));
        let [Outbound::Raw { opcode, body }] = opened.as_slice() else {
            panic!("expected one raw loot window")
        };
        assert_eq!(*opcode, 0x0160);
        assert_eq!(&body[0..8], &60u64.to_le_bytes());
        assert_eq!(&body[9..13], &25u32.to_le_bytes());
        assert!(store.skin_requests.lock().unwrap().is_empty());

        assert_clear_money(&run(&store, &mut state, take_money()));
        assert_eq!(
            store.money_take_requests.lock().unwrap().as_slice(),
            &[(42, 60)]
        );
    }

    #[test]
    fn money_take_on_a_copperless_corpse_still_clears_the_money_row() {
        let store = InMemoryLootWindow::default();
        let mut state = OpenLootState::default();
        run(&store, &mut state, open_creature(60));

        assert_clear_money(&run(&store, &mut state, take_money()));
        assert_eq!(
            store.money_take_requests.lock().unwrap().as_slice(),
            &[(42, 60)]
        );
    }

    #[test]
    fn takes_after_a_release_do_nothing() {
        let store = InMemoryLootWindow {
            money: 25,
            ..Default::default()
        };
        let mut state = OpenLootState::default();
        run(&store, &mut state, open_creature(60));

        let released = run(&store, &mut state, release(60));
        assert!(matches!(
            released.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_LOOT_RELEASE_RESPONSE(response))]
                if response.guid.guid() == 60
        ));

        assert!(run(&store, &mut state, take_money()).is_empty());
        assert!(run(&store, &mut state, take_item(3)).is_empty());
        assert!(store.money_take_requests.lock().unwrap().is_empty());
        assert!(store.item_take_requests.lock().unwrap().is_empty());
    }
}
