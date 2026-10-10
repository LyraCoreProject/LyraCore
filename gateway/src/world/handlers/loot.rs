//! Loot family: the Loot Window lifecycle, GameObject use, and Loot Roll votes.

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

/// Authenticated player facts needed by loot-window operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LootWindowPlayer {
    pub(crate) account_id: u64,
    pub(crate) self_guid: Option<u64>,
}

impl LootWindowPlayer {
    fn actor(self) -> Option<Actor> {
        self.self_guid.and_then(Actor::new)
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

fn log_refusal(player: LootWindowPlayer, refusal: LootWindowRefusal) {
    log::debug!(
        "world: loot request refused (account {}): {refusal:?}",
        player.account_id
    );
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
            let Some(actor) = player.actor() else {
                return Ok(LootWindowOutcome::Handled {
                    next_state: current_state,
                    durable_request: None,
                    outbound: Vec::new(),
                });
            };
            let target_guid = request.guid.guid();
            let durable_request = Some(LootWindowDurableRequest::UseGameObject { target_guid });
            if let LootWindowRequestStatus::Refused(refusal) =
                store.use_gameobject(actor, target_guid)?
            {
                log_refusal(player, refusal);
                let (next_state, outbound) =
                    refusal_transition(refusal, current_state, target_guid);
                return Ok(LootWindowOutcome::Handled {
                    next_state,
                    durable_request,
                    outbound,
                });
            }
            let items = store.loot_target_items(target_guid, actor.guid())?;
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
            let Some(viewer) = player.actor() else {
                return Ok(LootWindowOutcome::Handled {
                    next_state: current_state,
                    durable_request: None,
                    outbound: Vec::new(),
                });
            };
            let target_guid = request.guid.guid();
            let open_request = LootWindowDurableRequest::OpenCreature { target_guid };
            if let LootWindowRequestStatus::Refused(refusal) =
                store.open_creature_loot(viewer, target_guid)?
            {
                log_refusal(player, refusal);
                let (next_state, outbound) =
                    refusal_transition(refusal, current_state, target_guid);
                return Ok(LootWindowOutcome::Handled {
                    next_state,
                    durable_request: Some(open_request),
                    outbound,
                });
            }
            let money = store.loot_target_money(target_guid)?;
            let items = store.loot_target_items(target_guid, viewer.guid())?;
            let durable_request = if items.is_empty() && money == 0 {
                // Skinning an empty corpse is opportunistic: a Refusal still shows the empty window.
                store.skin_corpse(viewer, target_guid)?;
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
            let (Some(actor), Some(target_guid)) = (player.actor(), current_state.target_guid)
            else {
                return Ok(LootWindowOutcome::Handled {
                    next_state: current_state,
                    durable_request: None,
                    outbound: Vec::new(),
                });
            };
            let durable_request = LootWindowDurableRequest::TakeMoney { target_guid };
            let (next_state, outbound) = match store.loot_money(actor, target_guid)? {
                LootWindowRequestStatus::Applied => (
                    current_state,
                    vec![Outbound::One(ServerOpcodeMessage::SMSG_LOOT_CLEAR_MONEY)],
                ),
                LootWindowRequestStatus::Refused(refusal) => {
                    log_refusal(player, refusal);
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
            let (Some(actor), Some(target_guid)) = (player.actor(), current_state.target_guid)
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
            let (next_state, outbound) =
                match store.take_loot(actor, target_guid, request.item_slot)? {
                    LootWindowRequestStatus::Applied => (
                        current_state,
                        vec![Outbound::One(ServerOpcodeMessage::SMSG_LOOT_REMOVED(
                            codec::build_loot_removed(request.item_slot),
                        ))],
                    ),
                    LootWindowRequestStatus::Refused(refusal) => {
                        log_refusal(player, refusal);
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

/// Loot Roll votes and the master looter's assignment. Each Refusal is logged and dropped; the
/// roll packets ride the `game_group_event` relay.
pub(crate) fn handle_loot<St: LootRollStore + ShardRoutingStore + ?Sized>(
    store: &St,
    conn: &WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    let actor = social::self_guid(conn).and_then(Actor::new);
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
            let actor = actor.ok_or_else(|| anyhow!("CMSG_LOOT_MASTER_GIVE before world entry"))?;
            finish_loot_action(store.loot_master_give(
                actor,
                corpse_guid,
                c.slot_id,
                target_guid,
            )?);
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
        store.items_by_viewer.insert(42, vec![(4, 117, 2, 321, 0)]);
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
            use_failure: Some(Failure::TransportLost),
            ..Default::default()
        };
        let current_state = OpenLootState {
            target_guid: Some(11),
        };

        let error = dispatch_loot_window(&store, player(), current_state, open_chest(90))
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

        let error = dispatch_loot_window(
            &store,
            player(),
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

        let error = dispatch_loot_window(
            &store,
            player(),
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
            &[(42, 60)]
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
            &[(42, 75, 3)]
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
            &[(42, 75, 3)]
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
            let error = dispatch_loot_window(&store, player(), open, request)
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
        assert_eq!(store.open_requests.lock().unwrap().as_slice(), &[(42, 60)]);
        assert!(store.skin_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn two_viewers_of_one_creature_receive_their_own_visible_items() {
        let mut store = InMemoryLootWindow::default();
        store.items_by_viewer.insert(42, vec![(0, 6948, 1, 100, 0)]);
        store.items_by_viewer.insert(43, vec![(2, 2589, 5, 200, 0)]);

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
        assert_eq!(store.skin_requests.lock().unwrap().as_slice(), &[(42, 61)]);
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
    fn transport_loss_on_skinning_propagates() {
        let store = InMemoryLootWindow {
            skin_failure: Some(Failure::TransportLost),
            ..Default::default()
        };

        let error = dispatch_loot_window(
            &store,
            player(),
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
        let LootWindowOutcome::Handled {
            next_state,
            outbound,
            ..
        } = dispatch_loot_window(store, player(), *state, msg).unwrap()
        else {
            panic!("the request passed through")
        };
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
