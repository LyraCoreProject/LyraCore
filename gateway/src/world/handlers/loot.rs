//! Loot-window action dispatch plus the remaining corpse, GameObject, and group-loot handler.

use super::super::*;
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
    fn use_gameobject(
        &self,
        account_id: u64,
        actor_guid: u64,
        target_guid: u64,
    ) -> Result<LootWindowRequestStatus>;
    fn open_creature_loot(
        &self,
        account_id: u64,
        actor_guid: u64,
        corpse_guid: u64,
    ) -> Result<LootWindowRequestStatus>;
    fn skin_corpse(
        &self,
        account_id: u64,
        actor_guid: u64,
        target_guid: u64,
    ) -> Result<LootWindowRequestStatus>;
    fn loot_money(
        &self,
        account_id: u64,
        actor_guid: u64,
        target_guid: u64,
    ) -> Result<LootWindowRequestStatus>;
    fn take_loot(
        &self,
        account_id: u64,
        actor_guid: u64,
        target_guid: u64,
        loot_slot: u8,
    ) -> Result<LootWindowRequestStatus>;
}

/// How the Module answered a loot Durable Request. A Refusal is an outcome; a timeout or transport
/// failure stays an error and ends the session.
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

// Module cores outside the loot family still refuse with untagged prose, so an untyped Rejection
// lands on `Unanswered` rather than ending the session.
impl From<LootRefusal> for LootWindowRefusal {
    fn from(refusal: LootRefusal) -> Self {
        match refusal {
            LootRefusal::LootTagIneligible => Self::LootTagIneligible,
            LootRefusal::OutOfRange => Self::OutOfRange,
            LootRefusal::NoLootSource
            | LootRefusal::LooterUnavailable
            | LootRefusal::NothingToLoot
            | LootRefusal::RollUnavailable
            | LootRefusal::NotMasterLooter => Self::Unanswered,
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

impl LootWindowStore for crate::stdb::Coordinator {
    fn loot_target_money(&self, target_guid: u64) -> Result<u32> {
        crate::stdb::Coordinator::loot_target_money(self, target_guid)
    }

    fn loot_target_items(
        &self,
        target_guid: u64,
        viewer_guid: u64,
    ) -> Result<Vec<codec::LootItemView>> {
        crate::stdb::Coordinator::corpse_loot(self, target_guid, viewer_guid)
    }

    fn use_gameobject(
        &self,
        account_id: u64,
        actor_guid: u64,
        target_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        crate::stdb::Coordinator::use_gameobject(self, account_id, actor_guid, target_guid)
    }

    fn open_creature_loot(
        &self,
        account_id: u64,
        actor_guid: u64,
        corpse_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        crate::stdb::Coordinator::open_creature_loot(self, account_id, actor_guid, corpse_guid)
    }

    fn skin_corpse(
        &self,
        account_id: u64,
        actor_guid: u64,
        target_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        crate::stdb::Coordinator::skin_corpse(self, account_id, actor_guid, target_guid)
    }

    fn loot_money(
        &self,
        account_id: u64,
        actor_guid: u64,
        target_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        crate::stdb::Coordinator::loot_money(self, account_id, actor_guid, target_guid)
    }

    fn take_loot(
        &self,
        account_id: u64,
        actor_guid: u64,
        target_guid: u64,
        loot_slot: u8,
    ) -> Result<LootWindowRequestStatus> {
        crate::stdb::Coordinator::take_loot(self, account_id, actor_guid, target_guid, loot_slot)
    }
}

/// The target whose loot window is currently open for this world session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct OpenLootState {
    pub(crate) target_guid: Option<u64>,
}

/// Authenticated player facts needed by loot-window operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LootWindowPlayer {
    pub(crate) account_id: u64,
    pub(crate) self_guid: Option<u64>,
}

impl LootWindowPlayer {
    fn actor_guid(self) -> Option<u64> {
        self.self_guid.filter(|guid| *guid != 0)
    }
}

/// Records the durable request already executed while producing an outcome; it is not a command
/// for the outcome consumer to execute again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LootWindowDurableRequest {
    UseGameObject { target_guid: u64 },
    OpenCreature { target_guid: u64 },
    SkinCreature { target_guid: u64 },
    TakeMoney { target_guid: u64 },
    TakeItem { target_guid: u64, loot_slot: u8 },
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

fn finish_loot_action(status: LootActionStatus) {
    if let LootActionStatus::Refused(refusal) = status {
        log::debug!("world: loot action refused: {}", refusal.as_tag());
    }
}

/// A handled loot request returns all session state and client traffic to apply in order.
pub(crate) enum LootWindowOutcome {
    Handled {
        next_state: OpenLootState,
        durable_request: Option<LootWindowDurableRequest>,
        outbound: Vec<Outbound>,
    },
    PassThrough(ClientOpcodeMessage),
}

/// Map a client request through the loot-window lifecycle without owning session transport.
pub(crate) fn dispatch_loot_window<St: LootWindowStore + ?Sized>(
    store: &St,
    player: LootWindowPlayer,
    current_state: OpenLootState,
    msg: ClientOpcodeMessage,
) -> Result<LootWindowOutcome> {
    match msg {
        ClientOpcodeMessage::CMSG_GAMEOBJ_USE(request) => {
            let Some(actor_guid) = player.actor_guid() else {
                return Ok(LootWindowOutcome::Handled {
                    next_state: current_state,
                    durable_request: None,
                    outbound: Vec::new(),
                });
            };
            let target_guid = request.guid.guid();
            let durable_request = Some(LootWindowDurableRequest::UseGameObject { target_guid });
            if let LootWindowRequestStatus::Refused(refusal) =
                store.use_gameobject(player.account_id, actor_guid, target_guid)?
            {
                let (next_state, outbound) =
                    refusal_transition(refusal, current_state, target_guid);
                return Ok(LootWindowOutcome::Handled {
                    next_state,
                    durable_request,
                    outbound,
                });
            }
            let items = store.loot_target_items(target_guid, actor_guid)?;
            if items.is_empty() {
                return Ok(LootWindowOutcome::Handled {
                    next_state: current_state,
                    durable_request,
                    outbound: Vec::new(),
                });
            }
            let (opcode, body) = codec::build_loot_response_raw(target_guid, 0, &items);
            Ok(LootWindowOutcome::Handled {
                next_state: OpenLootState {
                    target_guid: Some(target_guid),
                },
                durable_request,
                outbound: vec![Outbound::Raw { opcode, body }],
            })
        }
        ClientOpcodeMessage::CMSG_LOOT(request) => {
            let Some(viewer_guid) = player.actor_guid() else {
                return Ok(LootWindowOutcome::Handled {
                    next_state: current_state,
                    durable_request: None,
                    outbound: Vec::new(),
                });
            };
            let target_guid = request.guid.guid();
            let open_request = LootWindowDurableRequest::OpenCreature { target_guid };
            if let LootWindowRequestStatus::Refused(refusal) =
                store.open_creature_loot(player.account_id, viewer_guid, target_guid)?
            {
                let (next_state, outbound) =
                    refusal_transition(refusal, current_state, target_guid);
                return Ok(LootWindowOutcome::Handled {
                    next_state,
                    durable_request: Some(open_request),
                    outbound,
                });
            }
            let money = store.loot_target_money(target_guid)?;
            let items = store.loot_target_items(target_guid, viewer_guid)?;
            let durable_request = if items.is_empty() && money == 0 {
                // Skinning an empty corpse is opportunistic: a Refusal still shows the empty window.
                store.skin_corpse(player.account_id, viewer_guid, target_guid)?;
                Some(LootWindowDurableRequest::SkinCreature { target_guid })
            } else {
                Some(open_request)
            };
            let (opcode, body) = codec::build_loot_response_raw(target_guid, money, &items);
            Ok(LootWindowOutcome::Handled {
                next_state: OpenLootState {
                    target_guid: Some(target_guid),
                },
                durable_request,
                outbound: vec![Outbound::Raw { opcode, body }],
            })
        }
        ClientOpcodeMessage::CMSG_LOOT_MONEY => {
            let (Some(actor_guid), Some(target_guid)) =
                (player.actor_guid(), current_state.target_guid)
            else {
                return Ok(LootWindowOutcome::Handled {
                    next_state: current_state,
                    durable_request: None,
                    outbound: Vec::new(),
                });
            };
            let durable_request = LootWindowDurableRequest::TakeMoney { target_guid };
            let (next_state, outbound) =
                match store.loot_money(player.account_id, actor_guid, target_guid)? {
                    LootWindowRequestStatus::Applied => (
                        current_state,
                        vec![Outbound::One(ServerOpcodeMessage::SMSG_LOOT_CLEAR_MONEY)],
                    ),
                    LootWindowRequestStatus::Refused(refusal) => {
                        refusal_transition(refusal, current_state, target_guid)
                    }
                };
            Ok(LootWindowOutcome::Handled {
                next_state,
                durable_request: Some(durable_request),
                outbound,
            })
        }
        ClientOpcodeMessage::CMSG_AUTOSTORE_LOOT_ITEM(request) => {
            let (Some(actor_guid), Some(target_guid)) =
                (player.actor_guid(), current_state.target_guid)
            else {
                return Ok(LootWindowOutcome::Handled {
                    next_state: current_state,
                    durable_request: None,
                    outbound: Vec::new(),
                });
            };
            let durable_request = LootWindowDurableRequest::TakeItem {
                target_guid,
                loot_slot: request.item_slot,
            };
            let (next_state, outbound) = match store.take_loot(
                player.account_id,
                actor_guid,
                target_guid,
                request.item_slot,
            )? {
                LootWindowRequestStatus::Applied => (
                    current_state,
                    vec![Outbound::One(ServerOpcodeMessage::SMSG_LOOT_REMOVED(
                        codec::build_loot_removed(request.item_slot),
                    ))],
                ),
                LootWindowRequestStatus::Refused(refusal) => {
                    refusal_transition(refusal, current_state, target_guid)
                }
            };
            Ok(LootWindowOutcome::Handled {
                next_state,
                durable_request: Some(durable_request),
                outbound,
            })
        }
        ClientOpcodeMessage::CMSG_LOOT_RELEASE(request) => Ok(LootWindowOutcome::Handled {
            next_state: OpenLootState::default(),
            durable_request: None,
            outbound: vec![Outbound::One(
                ServerOpcodeMessage::SMSG_LOOT_RELEASE_RESPONSE(Box::new(
                    codec::build_loot_release_response(request.guid.guid()),
                )),
            )],
        }),
        other => Ok(LootWindowOutcome::PassThrough(other)),
    }
}

/// Remaining group-loot, non-window GameObject, and death-recovery operations not yet migrated to
/// a focused action interface.
pub(crate) fn handle_loot<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
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
            // sharded, it routes to realm-core instead, so the guid it authorizes as the voter must
            // be the one THIS socket authenticated with, never a literal from the packet.
            let self_guid = match &conn.state {
                WorldState::InWorld(iw) => iw.self_guid,
                WorldState::CharSelect => 0,
            };
            finish_loot_action(loot::run_vote(
                store,
                conn.account_id,
                self_guid,
                corpse_guid,
                c.item_slot,
                vote,
            )?);
        }
        ClientOpcodeMessage::CMSG_LOOT_MASTER_GIVE(c) => {
            let corpse_guid = c.loot.guid();
            let target_guid = c.player.guid();
            finish_loot_action(store.loot_master_give(
                conn.account_id,
                social::self_guid(conn).unwrap_or(0),
                corpse_guid,
                c.slot_id,
                target_guid,
            )?);
        }
        ClientOpcodeMessage::CMSG_GAMEOBJ_USE(request) => {
            let Some(actor_guid) = social::self_guid(conn).filter(|guid| *guid != 0) else {
                return Ok(None);
            };
            let target_guid = request.guid.guid();
            match store.use_gameobject(conn.account_id, actor_guid, target_guid)? {
                LootWindowRequestStatus::Applied => {
                    let items = store.loot_target_items(target_guid, actor_guid)?;
                    if !items.is_empty() {
                        if let WorldState::InWorld(iw) = &mut conn.state {
                            iw.open_loot = OpenLootState {
                                target_guid: Some(target_guid),
                            };
                        }
                        let (opcode, body) = codec::build_loot_response_raw(target_guid, 0, &items);
                        send(tx, Outbound::Raw { opcode, body })?;
                    }
                }
                LootWindowRequestStatus::Refused(refusal) => {
                    if refusal.loot_error().is_some() {
                        if let WorldState::InWorld(iw) = &mut conn.state {
                            iw.open_loot = OpenLootState::default();
                        }
                    }
                    for outbound in refusal_outbound(refusal, target_guid) {
                        send(tx, outbound)?;
                    }
                }
            }
        }
        // Enter an area trigger (CMSG_AREATRIGGER): the client fires this when the player physically
        // walks into a trigger zone (e.g. a mine for an "explore" quest). The module credits any active
        // explore quest tied to the trigger id. A transient/no-match result is logged + ignored.
        ClientOpcodeMessage::CMSG_AREATRIGGER(a) => {
            if let Err(e) = store.enter_areatrigger(
                conn.account_id,
                social::self_guid(conn).unwrap_or(0),
                a.trigger_id,
            ) {
                log::debug!(
                    "world: enter_areatrigger ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        // Gameobject template query (CMSG_GAMEOBJECT_QUERY): the client asks for a GO's name/type/display
        // before it renders/interacts. Reply with the template, or the not-found form.
        ClientOpcodeMessage::CMSG_GAMEOBJECT_QUERY(q) => {
            let tmpl = store.gameobject_template(q.entry_id)?;
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_GAMEOBJECT_QUERY_RESPONSE(
                    Box::new(codec::build_gameobject_query_response(
                        q.entry_id,
                        tmpl.as_ref(),
                    )),
                )),
            )?;
        }
        // Release Spirit after death (slice 4). The client sends this (empty body) when the player
        // clicks Release on the death screen. Revive in place at full health; the restored health
        // replicates via the on_update VALUES relay and the client leaves the death screen.
        // SMSG_CORPSE_RECLAIM_DELAY is now relay-driven (the escalated per-corpse
        // delay, not a flat 30s) — see `on_corpse_insert` in `stdb/subscriptions.rs`, which fires off
        // the SAME `game_corpse` insert `repop`'s reducer call just caused, so no explicit send here.
        ClientOpcodeMessage::CMSG_REPOP_REQUEST => {
            let self_guid = match &conn.state {
                WorldState::InWorld(iw) => iw.self_guid,
                WorldState::CharSelect => 0,
            };
            if let Err(e) = store.repop(conn.account_id, self_guid) {
                log::debug!("world: repop ignored (account {}): {e}", conn.account_id);
            }
        }
        // Corpse location query (slice 5): the client asks where the player's corpse is to draw the
        // map marker + offer "Reclaim Corpse" near it. Reply with the corpse's position, or NotFound.
        ClientOpcodeMessage::MSG_CORPSE_QUERY => {
            if let WorldState::InWorld(iw) = &conn.state {
                let loc = store.corpse_location(iw.self_guid)?;
                send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::MSG_CORPSE_QUERY(Box::new(
                        codec::build_corpse_query_response(loc)?,
                    ))),
                )?;
            }
        }
        // Reclaim your corpse (slice 5): the ghost, near its corpse and past the 30s delay, resurrects
        // at 50%. The module validates ownership/ghost/range/delay; a failure (too far, too soon, not
        // a ghost) is expected and silently ignored — the client just stays a ghost.
        ClientOpcodeMessage::CMSG_RECLAIM_CORPSE(r) => {
            if let Err(e) = store.reclaim_corpse(
                conn.account_id,
                social::self_guid(conn).unwrap_or(0),
                r.guid.guid(),
            ) {
                log::debug!(
                    "world: reclaim_corpse ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        // Resurrection accept-prompt response: the dead player answered the SMSG_RESURRECT_REQUEST
        // offer. `status` is vanilla's accept(1)/decline(0) byte; the offer's guid is ignored (mirrors
        // `reclaim_corpse`'s own-corpse derivation — the module resolves the pending offer from the
        // CALLER via `ctx.sender()`, never the wire guid). A failure (no pending offer — already
        // answered/lapsed) is expected and silently ignored.
        ClientOpcodeMessage::CMSG_RESURRECT_RESPONSE(r) => {
            let self_guid = match &conn.state {
                WorldState::InWorld(iw) => iw.self_guid,
                WorldState::CharSelect => 0,
            };
            if let Err(e) = store.resurrect_response(conn.account_id, self_guid, r.status != 0) {
                log::debug!(
                    "world: resurrect_response ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        // Spirit-Healer resurrection: a ghost activated the graveyard Spirit Healer (npc_flags
        // SPIRITHEALER). The module res's in place at 50% + applies Resurrection Sickness; on success
        // reply with SMSG_SPIRIT_HEALER_CONFIRM (echoing the healer's guid) so the client closes the
        // dialog. The res itself replicates via the entity VALUES relay (health > 0 + cleared ghost
        // bits), exactly like reclaim_corpse. A failure (not a ghost) is per-action — log + ignore.
        ClientOpcodeMessage::CMSG_SPIRIT_HEALER_ACTIVATE(s) => {
            let self_guid = match &conn.state {
                WorldState::InWorld(iw) => iw.self_guid,
                WorldState::CharSelect => 0,
            };
            match store.spirit_healer_res(conn.account_id, self_guid, s.guid.guid()) {
                Ok(()) => send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_SPIRIT_HEALER_CONFIRM(
                        SMSG_SPIRIT_HEALER_CONFIRM { guid: s.guid },
                    )),
                )?,
                Err(e) => {
                    log::debug!(
                        "world: spirit_healer_res ignored (account {}): {e}",
                        conn.account_id
                    )
                }
            }
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
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

    #[derive(Default)]
    struct InMemoryLootWindow {
        money: u32,
        items_by_viewer: HashMap<u64, Vec<codec::LootItemView>>,
        money_reads: Mutex<Vec<u64>>,
        item_reads: Mutex<Vec<(u64, u64)>>,
        use_requests: Mutex<Vec<(u64, u64, u64)>>,
        open_requests: Mutex<Vec<(u64, u64, u64)>>,
        operations: Mutex<Vec<&'static str>>,
        skin_requests: Mutex<Vec<(u64, u64, u64)>>,
        money_take_requests: Mutex<Vec<(u64, u64, u64)>>,
        item_take_requests: Mutex<Vec<(u64, u64, u64, u8)>>,
        skin_refusal: Option<LootWindowRefusal>,
        skin_fatal_error: Option<String>,
        use_refusal: Option<LootWindowRefusal>,
        use_fatal_error: Option<String>,
        open_refusal: Option<LootWindowRefusal>,
        open_fatal_error: Option<String>,
        money_take_refusal: Option<LootWindowRefusal>,
        money_take_fatal_error: Option<String>,
        item_take_refusal: Option<LootWindowRefusal>,
        item_take_fatal_error: Option<String>,
    }

    fn request_status(
        refusal: Option<LootWindowRefusal>,
        fatal_error: &Option<String>,
    ) -> Result<LootWindowRequestStatus> {
        match (fatal_error, refusal) {
            (Some(error), _) => Err(anyhow::anyhow!(error.clone())),
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
            account_id: u64,
            actor_guid: u64,
            target_guid: u64,
        ) -> Result<LootWindowRequestStatus> {
            self.operations.lock().unwrap().push("use gameobject");
            self.use_requests
                .lock()
                .unwrap()
                .push((account_id, actor_guid, target_guid));
            request_status(self.use_refusal, &self.use_fatal_error)
        }

        fn open_creature_loot(
            &self,
            account_id: u64,
            actor_guid: u64,
            corpse_guid: u64,
        ) -> Result<LootWindowRequestStatus> {
            self.operations.lock().unwrap().push("open creature loot");
            self.open_requests
                .lock()
                .unwrap()
                .push((account_id, actor_guid, corpse_guid));
            request_status(self.open_refusal, &self.open_fatal_error)
        }

        fn skin_corpse(
            &self,
            account_id: u64,
            actor_guid: u64,
            target_guid: u64,
        ) -> Result<LootWindowRequestStatus> {
            self.skin_requests
                .lock()
                .unwrap()
                .push((account_id, actor_guid, target_guid));
            request_status(self.skin_refusal, &self.skin_fatal_error)
        }

        fn loot_money(
            &self,
            account_id: u64,
            actor_guid: u64,
            target_guid: u64,
        ) -> Result<LootWindowRequestStatus> {
            self.money_take_requests
                .lock()
                .unwrap()
                .push((account_id, actor_guid, target_guid));
            request_status(self.money_take_refusal, &self.money_take_fatal_error)
        }

        fn take_loot(
            &self,
            account_id: u64,
            actor_guid: u64,
            target_guid: u64,
            loot_slot: u8,
        ) -> Result<LootWindowRequestStatus> {
            self.item_take_requests.lock().unwrap().push((
                account_id,
                actor_guid,
                target_guid,
                loot_slot,
            ));
            request_status(self.item_take_refusal, &self.item_take_fatal_error)
        }
    }

    fn player() -> LootWindowPlayer {
        LootWindowPlayer {
            account_id: 7,
            self_guid: Some(42),
        }
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

        let outcome = dispatch_loot_window(
            &store,
            LootWindowPlayer {
                account_id: 7,
                self_guid: None,
            },
            current_state,
            open_creature(60),
        )
        .unwrap();

        assert!(matches!(
            outcome,
            LootWindowOutcome::Handled {
                next_state,
                durable_request,
                outbound,
            } if next_state == current_state
                && durable_request.is_none()
                && outbound.is_empty()
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

        let outcome = dispatch_loot_window(
            &store,
            LootWindowPlayer {
                account_id: 7,
                self_guid: Some(0),
            },
            current_state,
            open_creature(60),
        )
        .unwrap();

        assert!(matches!(
            outcome,
            LootWindowOutcome::Handled {
                next_state,
                durable_request,
                outbound,
            } if next_state == current_state
                && durable_request.is_none()
                && outbound.is_empty()
        ));
        assert!(store.money_reads.lock().unwrap().is_empty());
        assert!(store.item_reads.lock().unwrap().is_empty());
        assert!(store.skin_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn chest_use_precedes_generated_loot_and_opens_the_shared_window() {
        let mut store = InMemoryLootWindow::default();
        store.items_by_viewer.insert(42, vec![(4, 117, 2, 321)]);
        let current_state = OpenLootState {
            target_guid: Some(11),
        };

        let outcome =
            dispatch_loot_window(&store, player(), current_state, open_chest(90)).unwrap();

        let LootWindowOutcome::Handled {
            next_state,
            durable_request,
            outbound,
        } = outcome
        else {
            panic!("chest use passed through")
        };
        assert_eq!(next_state.target_guid, Some(90));
        assert_eq!(
            durable_request,
            Some(LootWindowDurableRequest::UseGameObject { target_guid: 90 })
        );
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
        assert_eq!(
            store.use_requests.lock().unwrap().as_slice(),
            &[(7, 42, 90)]
        );
        assert_eq!(store.item_reads.lock().unwrap().as_slice(), &[(90, 42)]);
        assert!(store.open_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn successful_empty_chest_use_keeps_the_current_window_state() {
        let store = InMemoryLootWindow::default();
        let current_state = OpenLootState {
            target_guid: Some(11),
        };

        let outcome =
            dispatch_loot_window(&store, player(), current_state, open_chest(90)).unwrap();

        assert!(matches!(
            outcome,
            LootWindowOutcome::Handled {
                next_state,
                durable_request,
                outbound,
            } if next_state == current_state
                && durable_request == Some(LootWindowDurableRequest::UseGameObject { target_guid: 90 })
                && outbound.is_empty()
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

        let outcome =
            dispatch_loot_window(&store, player(), current_state, open_chest(90)).unwrap();

        assert!(matches!(
            outcome,
            LootWindowOutcome::Handled {
                next_state,
                durable_request,
                outbound,
            } if next_state == current_state
                && durable_request == Some(LootWindowDurableRequest::UseGameObject { target_guid: 90 })
                && outbound.is_empty()
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
            use_fatal_error: Some("transport disconnected".into()),
            ..Default::default()
        };
        let current_state = OpenLootState {
            target_guid: Some(11),
        };

        let error = dispatch_loot_window(&store, player(), current_state, open_chest(90))
            .err()
            .expect("fatal GameObject failure was handled");

        assert_eq!(error.to_string(), "transport disconnected");
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

        let outcome = dispatch_loot_window(
            &store,
            player(),
            OpenLootState {
                target_guid: Some(11),
            },
            open_creature(60),
        )
        .unwrap();

        let LootWindowOutcome::Handled {
            next_state,
            durable_request,
            outbound,
        } = outcome
        else {
            panic!("creature open passed through")
        };
        assert_eq!(next_state, OpenLootState::default());
        assert_eq!(
            durable_request,
            Some(LootWindowDurableRequest::OpenCreature { target_guid: 60 })
        );
        assert_didnt_kill(&outbound, 60);
        assert_eq!(
            store.open_requests.lock().unwrap().as_slice(),
            &[(7, 42, 60)]
        );
        assert!(store.money_reads.lock().unwrap().is_empty());
        assert!(store.item_reads.lock().unwrap().is_empty());
        assert!(store.skin_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn creature_open_transport_failure_propagates_without_loot_reads() {
        let store = InMemoryLootWindow {
            open_fatal_error: Some("transport disconnected".into()),
            ..Default::default()
        };

        let error = dispatch_loot_window(
            &store,
            player(),
            OpenLootState::default(),
            open_creature(60),
        )
        .err()
        .expect("fatal creature-open failure was handled");

        assert_eq!(error.to_string(), "transport disconnected");
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

        let outcome =
            dispatch_loot_window(&store, player(), current_state, open_creature(60)).unwrap();

        assert!(matches!(
            outcome,
            LootWindowOutcome::Handled {
                next_state,
                durable_request: Some(LootWindowDurableRequest::OpenCreature { target_guid: 60 }),
                outbound,
            } if next_state == current_state && outbound.is_empty()
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
        let outcome = dispatch_loot_window(
            &InMemoryLootWindow::default(),
            player(),
            current_state,
            release(request_target),
        )
        .unwrap();
        let LootWindowOutcome::Handled {
            next_state,
            durable_request,
            outbound,
        } = outcome
        else {
            panic!("release passed through")
        };
        assert_eq!(durable_request, None);
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

        let outcome = dispatch_loot_window(
            &store,
            player(),
            current_state,
            ClientOpcodeMessage::CMSG_LOOT_MONEY,
        )
        .unwrap();

        assert!(matches!(
            outcome,
            LootWindowOutcome::Handled {
                next_state,
                durable_request,
                outbound,
            } if next_state == current_state
                && durable_request == Some(LootWindowDurableRequest::TakeMoney { target_guid: 60 })
                && matches!(outbound.as_slice(), [Outbound::One(ServerOpcodeMessage::SMSG_LOOT_CLEAR_MONEY)])
        ));
        assert_eq!(
            store.money_take_requests.lock().unwrap().as_slice(),
            &[(7, 42, 60)]
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

        let outcome = dispatch_loot_window(
            &store,
            player(),
            current_state,
            ClientOpcodeMessage::CMSG_LOOT_MONEY,
        )
        .unwrap();

        assert!(matches!(
            outcome,
            LootWindowOutcome::Handled {
                next_state,
                durable_request,
                outbound,
            } if next_state == current_state
                && durable_request == Some(LootWindowDurableRequest::TakeMoney { target_guid: 60 })
                && outbound.is_empty()
        ));
        assert_eq!(
            store.money_take_requests.lock().unwrap().as_slice(),
            &[(7, 42, 60)]
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
            | LootRefusal::NotMasterLooter => None,
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
                let outcome = dispatch_loot_window(&store, player(), open_window, message).unwrap();
                let LootWindowOutcome::Handled {
                    next_state,
                    outbound,
                    ..
                } = outcome
                else {
                    panic!("{refusal:?} passed through")
                };
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

        let outcome = dispatch_loot_window(
            &store,
            player(),
            OpenLootState {
                target_guid: Some(60),
            },
            ClientOpcodeMessage::CMSG_LOOT_MONEY,
        )
        .unwrap();

        let LootWindowOutcome::Handled {
            next_state,
            durable_request,
            outbound,
        } = outcome
        else {
            panic!("money take passed through")
        };
        assert_eq!(next_state, OpenLootState::default());
        assert_eq!(
            durable_request,
            Some(LootWindowDurableRequest::TakeMoney { target_guid: 60 })
        );
        assert_didnt_kill(&outbound, 60);
        assert_eq!(
            store.money_take_requests.lock().unwrap().as_slice(),
            &[(7, 42, 60)]
        );
    }

    #[test]
    fn money_take_without_an_open_target_has_no_operation_or_outbound() {
        let store = InMemoryLootWindow::default();

        let outcome = dispatch_loot_window(
            &store,
            player(),
            OpenLootState::default(),
            ClientOpcodeMessage::CMSG_LOOT_MONEY,
        )
        .unwrap();

        assert!(matches!(
            outcome,
            LootWindowOutcome::Handled {
                next_state: OpenLootState { target_guid: None },
                durable_request,
                outbound,
            } if durable_request.is_none() && outbound.is_empty()
        ));
        assert!(store.money_take_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn item_take_success_uses_the_open_target_and_only_removes_the_requested_slot() {
        let store = InMemoryLootWindow::default();
        let current_state = OpenLootState {
            target_guid: Some(75),
        };

        let outcome = dispatch_loot_window(
            &store,
            player(),
            current_state,
            ClientOpcodeMessage::CMSG_AUTOSTORE_LOOT_ITEM(CMSG_AUTOSTORE_LOOT_ITEM {
                item_slot: 3,
            }),
        )
        .unwrap();

        assert!(matches!(
            outcome,
            LootWindowOutcome::Handled {
                next_state,
                durable_request,
                outbound,
            } if next_state == current_state
                && durable_request == Some(LootWindowDurableRequest::TakeItem { target_guid: 75, loot_slot: 3 })
                && matches!(outbound.as_slice(), [Outbound::One(ServerOpcodeMessage::SMSG_LOOT_REMOVED(removed))] if removed.slot == 3)
        ));
        assert_eq!(
            store.item_take_requests.lock().unwrap().as_slice(),
            &[(7, 42, 75, 3)]
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

        let outcome = dispatch_loot_window(
            &store,
            player(),
            current_state,
            ClientOpcodeMessage::CMSG_AUTOSTORE_LOOT_ITEM(CMSG_AUTOSTORE_LOOT_ITEM {
                item_slot: 3,
            }),
        )
        .unwrap();

        assert!(matches!(
            outcome,
            LootWindowOutcome::Handled {
                next_state,
                durable_request,
                outbound,
            } if next_state == current_state
                && durable_request == Some(LootWindowDurableRequest::TakeItem { target_guid: 75, loot_slot: 3 })
                && outbound.is_empty()
        ));
        assert_eq!(
            store.item_take_requests.lock().unwrap().as_slice(),
            &[(7, 42, 75, 3)]
        );
    }

    #[test]
    fn item_take_ownership_refusal_closes_the_window_and_returns_didnt_kill() {
        let store = InMemoryLootWindow {
            item_take_refusal: Some(LootWindowRefusal::LootTagIneligible),
            ..Default::default()
        };

        let outcome = dispatch_loot_window(
            &store,
            player(),
            OpenLootState {
                target_guid: Some(75),
            },
            ClientOpcodeMessage::CMSG_AUTOSTORE_LOOT_ITEM(CMSG_AUTOSTORE_LOOT_ITEM {
                item_slot: 3,
            }),
        )
        .unwrap();

        let LootWindowOutcome::Handled {
            next_state,
            durable_request,
            outbound,
        } = outcome
        else {
            panic!("item take passed through")
        };
        assert_eq!(next_state, OpenLootState::default());
        assert_eq!(
            durable_request,
            Some(LootWindowDurableRequest::TakeItem {
                target_guid: 75,
                loot_slot: 3,
            })
        );
        assert_didnt_kill(&outbound, 75);
        assert_eq!(
            store.item_take_requests.lock().unwrap().as_slice(),
            &[(7, 42, 75, 3)]
        );
    }

    #[test]
    fn item_take_without_an_open_target_has_no_operation_or_outbound() {
        let store = InMemoryLootWindow::default();

        let outcome = dispatch_loot_window(
            &store,
            player(),
            OpenLootState::default(),
            ClientOpcodeMessage::CMSG_AUTOSTORE_LOOT_ITEM(CMSG_AUTOSTORE_LOOT_ITEM {
                item_slot: 3,
            }),
        )
        .unwrap();

        assert!(matches!(
            outcome,
            LootWindowOutcome::Handled {
                next_state: OpenLootState { target_guid: None },
                durable_request,
                outbound,
            } if durable_request.is_none() && outbound.is_empty()
        ));
        assert!(store.item_take_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn fatal_money_and_item_take_failures_propagate() {
        let operations = [
            (
                "gw_loot_money reducer transport disconnected: channel closed",
                ClientOpcodeMessage::CMSG_LOOT_MONEY,
            ),
            (
                "gw_loot_money reducer failed: transport disconnected",
                ClientOpcodeMessage::CMSG_LOOT_MONEY,
            ),
            (
                "send gw_loot_money: connection closed",
                ClientOpcodeMessage::CMSG_LOOT_MONEY,
            ),
            (
                "gw_loot_money reducer timed out after 10s",
                ClientOpcodeMessage::CMSG_LOOT_MONEY,
            ),
        ];
        for (message, request) in operations {
            let store = InMemoryLootWindow {
                money_take_fatal_error: Some(message.into()),
                ..Default::default()
            };

            let error = dispatch_loot_window(
                &store,
                player(),
                OpenLootState {
                    target_guid: Some(60),
                },
                request,
            )
            .err()
            .expect("fatal money-take failure was handled");

            assert_eq!(error.to_string(), message);
        }

        for message in [
            "gw_take_loot reducer transport disconnected: channel closed",
            "gw_take_loot reducer failed: transport disconnected",
            "send gw_take_loot: connection closed",
            "gw_take_loot reducer timed out after 10s",
        ] {
            let store = InMemoryLootWindow {
                item_take_fatal_error: Some(message.into()),
                ..Default::default()
            };

            let error = dispatch_loot_window(
                &store,
                player(),
                OpenLootState {
                    target_guid: Some(75),
                },
                ClientOpcodeMessage::CMSG_AUTOSTORE_LOOT_ITEM(CMSG_AUTOSTORE_LOOT_ITEM {
                    item_slot: 3,
                }),
            )
            .err()
            .expect("fatal item-take failure was handled");

            assert_eq!(error.to_string(), message);
        }
    }

    #[test]
    fn creature_open_returns_the_viewers_current_loot_and_replaces_the_open_target() {
        let mut store = InMemoryLootWindow {
            money: 25,
            ..Default::default()
        };
        store.items_by_viewer.insert(42, vec![(3, 2589, 5, 200)]);

        let outcome = dispatch_loot_window(
            &store,
            player(),
            OpenLootState {
                target_guid: Some(11),
            },
            open_creature(60),
        )
        .unwrap();

        let LootWindowOutcome::Handled {
            next_state,
            durable_request,
            outbound,
        } = outcome
        else {
            panic!("creature open passed through")
        };
        assert_eq!(next_state.target_guid, Some(60));
        assert_eq!(
            durable_request,
            Some(LootWindowDurableRequest::OpenCreature { target_guid: 60 })
        );
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
        assert_eq!(
            store.open_requests.lock().unwrap().as_slice(),
            &[(7, 42, 60)]
        );
        assert!(store.skin_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn two_viewers_of_one_creature_receive_their_own_visible_items() {
        let mut store = InMemoryLootWindow::default();
        store.items_by_viewer.insert(42, vec![(0, 6948, 1, 100)]);
        store.items_by_viewer.insert(43, vec![(2, 2589, 5, 200)]);

        let first = dispatch_loot_window(
            &store,
            player(),
            OpenLootState::default(),
            open_creature(60),
        )
        .unwrap();
        let second = dispatch_loot_window(
            &store,
            LootWindowPlayer {
                account_id: 8,
                self_guid: Some(43),
            },
            OpenLootState::default(),
            open_creature(60),
        )
        .unwrap();

        let LootWindowOutcome::Handled {
            outbound: first_outbound,
            ..
        } = first
        else {
            panic!("first creature open passed through")
        };
        let LootWindowOutcome::Handled {
            outbound: second_outbound,
            ..
        } = second
        else {
            panic!("second creature open passed through")
        };
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

        let outcome = dispatch_loot_window(
            &store,
            player(),
            OpenLootState::default(),
            open_creature(61),
        )
        .unwrap();

        let LootWindowOutcome::Handled {
            next_state,
            durable_request,
            outbound,
        } = outcome
        else {
            panic!("creature open passed through")
        };
        assert_eq!(next_state.target_guid, Some(61));
        assert_eq!(
            durable_request,
            Some(LootWindowDurableRequest::SkinCreature { target_guid: 61 })
        );
        let [Outbound::Raw { opcode, body }] = outbound.as_slice() else {
            panic!("expected one raw loot window")
        };
        assert_eq!(*opcode, 0x0160);
        assert_eq!(&body[0..8], &61u64.to_le_bytes());
        assert_eq!(&body[9..13], &0u32.to_le_bytes());
        assert_eq!(body[13], 0);
        assert_eq!(
            store.skin_requests.lock().unwrap().as_slice(),
            &[(7, 42, 61)]
        );
    }

    #[test]
    fn skinning_refusal_is_a_handled_empty_window() {
        let store = InMemoryLootWindow {
            skin_refusal: Some(LootWindowRefusal::Unanswered),
            ..Default::default()
        };

        let outcome = dispatch_loot_window(
            &store,
            player(),
            OpenLootState::default(),
            open_creature(61),
        )
        .unwrap();

        assert!(matches!(
            outcome,
            LootWindowOutcome::Handled {
                next_state: OpenLootState {
                    target_guid: Some(61)
                },
                durable_request: Some(LootWindowDurableRequest::SkinCreature {
                    target_guid: 61
                }),
                outbound,
            } if matches!(outbound.as_slice(), [Outbound::Raw { opcode: 0x0160, body }] if body[13] == 0)
        ));
    }

    #[test]
    fn fatal_skinning_failures_propagate() {
        for message in [
            "gw_skin reducer transport disconnected: channel closed",
            "gw_skin reducer failed: transport disconnected",
            "send gw_skin: connection closed",
            "gw_skin reducer timed out after 10s",
        ] {
            let store = InMemoryLootWindow {
                skin_fatal_error: Some(message.into()),
                ..Default::default()
            };

            let error = dispatch_loot_window(
                &store,
                player(),
                OpenLootState::default(),
                open_creature(61),
            )
            .err()
            .expect("fatal skinning failure was handled");

            assert_eq!(error.to_string(), message);
        }
    }
}
