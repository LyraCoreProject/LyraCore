//! Taxi query dispatcher. The store operations are cohesive module requests: handlers never read
//! catalogue, discovery, range, or topology tables and therefore cannot fork module policy.

use super::super::*;

pub(crate) trait TaxiActionStore: Send + Sync {
    fn taxi_node_status(
        &self,
        actor: Actor,
        npc_guid: u64,
    ) -> Result<Option<codec::TaxiNodeStatusView>>;

    fn open_taxi(&self, actor: Actor, npc_guid: u64) -> Result<Option<codec::TaxiMapView>>;

    fn activate_taxi(
        &self,
        actor: Actor,
        npc_guid: u64,
        source_client_node_id: u32,
        destination_client_node_id: u32,
    ) -> Result<codec::TaxiActivationResult>;

    fn arm_taxi_flight(&self, actor: Actor) -> Result<()>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TaxiActionPlayer {
    pub(crate) self_guid: Option<u64>,
}

pub(crate) enum TaxiActionOutcome {
    Handled {
        outbound: Vec<Outbound>,
    },
    Activated {
        outbound: Vec<Outbound>,
        character_guid: u64,
        arm: bool,
    },
    PassThrough(ClientOpcodeMessage),
}

fn status_outbound<St: TaxiActionStore + ?Sized>(
    store: &St,
    actor: Option<Actor>,
    npc_guid: u64,
) -> Result<Vec<Outbound>> {
    let Some(actor) = actor else {
        return Ok(Vec::new());
    };
    match store.taxi_node_status(actor, npc_guid)? {
        Some(view) => Ok(vec![Outbound::One(
            ServerOpcodeMessage::SMSG_TAXINODE_STATUS(Box::new(codec::build_taxi_node_status(
                view,
            ))),
        )]),
        None => Ok(Vec::new()),
    }
}

fn activate_taxi_outbound<St: TaxiActionStore + ?Sized>(
    store: &St,
    actor: Option<Actor>,
    npc_guid: u64,
    source_client_node_id: u32,
    destination_client_node_id: u32,
) -> Result<(Vec<Outbound>, bool)> {
    let server_error = codec::TaxiActivationResult::default();
    let result = match actor {
        Some(actor) => store.activate_taxi(
            actor,
            npc_guid,
            source_client_node_id,
            destination_client_node_id,
        )?,
        None => server_error,
    };
    let arm = result.result_code == lyracore_shared::constants::taxi_protocol::ACTIVATE_OK;
    Ok((
        vec![Outbound::One(ServerOpcodeMessage::SMSG_ACTIVATETAXIREPLY(
            codec::build_activate_taxi_reply(result),
        ))],
        arm,
    ))
}

/// Queue the activation reply, then arm the flight. The activation is already committed, so an
/// arming failure of any kind ends the World Session.
pub(crate) fn queue_reply_then_arm<St: TaxiActionStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    outbound: Vec<Outbound>,
    character_guid: u64,
    arm: bool,
) -> Result<()> {
    for message in outbound {
        send(tx, message)?;
    }
    match Actor::new(character_guid) {
        Some(actor) if arm => store.arm_taxi_flight(actor),
        _ => Ok(()),
    }
}

/// The single gateway entry to the module's open operation. Both the direct taxi query and TAXI
/// gossip selection call this exact function. No Character yet gets no window.
pub(crate) fn open_taxi_outbound<St: TaxiActionStore + ?Sized>(
    store: &St,
    actor: Option<Actor>,
    npc_guid: u64,
) -> Result<Vec<Outbound>> {
    let Some(actor) = actor else {
        return Ok(Vec::new());
    };
    match store.open_taxi(actor, npc_guid)? {
        Some(view) => Ok(vec![Outbound::One(
            ServerOpcodeMessage::SMSG_SHOWTAXINODES(Box::new(codec::build_show_taxi_nodes(&view))),
        )]),
        None => Ok(Vec::new()),
    }
}

pub(crate) fn dispatch_taxi_action<St: TaxiActionStore + ?Sized>(
    store: &St,
    player: TaxiActionPlayer,
    msg: ClientOpcodeMessage,
) -> Result<TaxiActionOutcome> {
    let actor = player.self_guid.and_then(Actor::new);
    match msg {
        ClientOpcodeMessage::CMSG_TAXINODE_STATUS_QUERY(query) => Ok(TaxiActionOutcome::Handled {
            outbound: status_outbound(store, actor, query.guid.guid())?,
        }),
        ClientOpcodeMessage::CMSG_TAXIQUERYAVAILABLENODES(query) => {
            Ok(TaxiActionOutcome::Handled {
                outbound: open_taxi_outbound(store, actor, query.guid.guid())?,
            })
        }
        ClientOpcodeMessage::CMSG_ACTIVATETAXI(request) => {
            let character_guid = player.self_guid.unwrap_or(0);
            let (outbound, arm) = activate_taxi_outbound(
                store,
                actor,
                request.guid.guid(),
                request.source_node,
                request.destination_node,
            )?;
            Ok(TaxiActionOutcome::Activated {
                outbound,
                character_guid,
                arm,
            })
        }
        other => Ok(TaxiActionOutcome::PassThrough(other)),
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::stdb::ReducerCallError;
    use std::sync::Mutex;
    use wow_world_messages::{
        vanilla::{CMSG_ACTIVATETAXI, CMSG_TAXINODE_STATUS_QUERY, CMSG_TAXIQUERYAVAILABLENODES},
        Guid,
    };

    #[derive(Default)]
    pub(crate) struct InMemoryTaxiActions {
        pub(crate) calls: Mutex<Vec<(&'static str, u64, u64)>>,
        pub(crate) status: Mutex<Option<codec::TaxiNodeStatusView>>,
        pub(crate) map: Mutex<Option<codec::TaxiMapView>>,
        pub(crate) activation: Mutex<codec::TaxiActivationResult>,
        pub(crate) activation_inputs: Mutex<Vec<(u64, u64, u32, u32)>>,
        /// Every Durable Request fails with this error.
        pub(crate) fail: Option<fn() -> anyhow::Error>,
        /// The one (source, destination) node pair the Module accepts; any other pair answers
        /// `ACTIVATE_NO_SUCH_PATH`. `None` accepts every pair.
        pub(crate) route: Option<(u32, u32)>,
        pub(crate) arm_tx_probe: Mutex<Option<SessionTx>>,
        pub(crate) arm_observed_depth: Mutex<Option<usize>>,
    }

    impl TaxiActionStore for InMemoryTaxiActions {
        fn taxi_node_status(
            &self,
            actor: Actor,
            npc_guid: u64,
        ) -> Result<Option<codec::TaxiNodeStatusView>> {
            self.calls
                .lock()
                .unwrap()
                .push(("status", actor.guid(), npc_guid));
            if let Some(error) = self.fail {
                return Err(error());
            }
            Ok((*self.status.lock().unwrap()).filter(|view| view.npc_guid == npc_guid))
        }

        fn open_taxi(&self, actor: Actor, npc_guid: u64) -> Result<Option<codec::TaxiMapView>> {
            self.calls
                .lock()
                .unwrap()
                .push(("open", actor.guid(), npc_guid));
            if let Some(error) = self.fail {
                return Err(error());
            }
            Ok(self
                .map
                .lock()
                .unwrap()
                .clone()
                .filter(|view| view.npc_guid == npc_guid))
        }

        fn activate_taxi(
            &self,
            actor: Actor,
            npc_guid: u64,
            source_client_node_id: u32,
            destination_client_node_id: u32,
        ) -> Result<codec::TaxiActivationResult> {
            self.calls
                .lock()
                .unwrap()
                .push(("activate", actor.guid(), npc_guid));
            self.activation_inputs.lock().unwrap().push((
                actor.guid(),
                npc_guid,
                source_client_node_id,
                destination_client_node_id,
            ));
            if let Some(error) = self.fail {
                return Err(error());
            }
            if self
                .route
                .is_some_and(|route| route != (source_client_node_id, destination_client_node_id))
            {
                return Ok(codec::TaxiActivationResult {
                    result_code: lyracore_shared::constants::taxi_protocol::ACTIVATE_NO_SUCH_PATH,
                });
            }
            Ok(*self.activation.lock().unwrap())
        }

        fn arm_taxi_flight(&self, actor: Actor) -> Result<()> {
            self.calls.lock().unwrap().push(("arm", actor.guid(), 0));
            if let Some(tx) = self.arm_tx_probe.lock().unwrap().as_ref() {
                *self.arm_observed_depth.lock().unwrap() = Some(tx.depth());
            }
            Ok(())
        }
    }

    #[test]
    fn status_query_returns_persisted_state_without_opening() {
        let store = InMemoryTaxiActions::default();
        *store.status.lock().unwrap() = Some(codec::TaxiNodeStatusView {
            npc_guid: 77,
            known: false,
        });
        let outcome = dispatch_taxi_action(
            &store,
            TaxiActionPlayer { self_guid: Some(9) },
            CMSG_TAXINODE_STATUS_QUERY {
                guid: Guid::new(77),
            }
            .into(),
        )
        .unwrap();
        assert!(matches!(outcome, TaxiActionOutcome::Handled { outbound } if outbound.len() == 1));
        assert_eq!(*store.calls.lock().unwrap(), vec![("status", 9, 77)]);
    }

    #[test]
    fn direct_query_uses_the_shared_open_operation() {
        let store = InMemoryTaxiActions::default();
        *store.map.lock().unwrap() = Some(codec::TaxiMapView {
            npc_guid: 77,
            source_client_node_id: 255,
            available_client_node_ids: vec![255, 256],
        });
        let outcome = dispatch_taxi_action(
            &store,
            TaxiActionPlayer { self_guid: Some(9) },
            CMSG_TAXIQUERYAVAILABLENODES {
                guid: Guid::new(77),
            }
            .into(),
        )
        .unwrap();
        assert!(matches!(outcome, TaxiActionOutcome::Handled { outbound } if outbound.len() == 1));
        assert_eq!(*store.calls.lock().unwrap(), vec![("open", 9, 77)]);
    }

    fn transport_lost() -> anyhow::Error {
        ReducerCallError::transport_lost("gw_open_taxi").into()
    }

    fn refused() -> anyhow::Error {
        ReducerCallError::refused("gw_open_taxi", "not a flight master").into()
    }

    fn status_query() -> ClientOpcodeMessage {
        CMSG_TAXINODE_STATUS_QUERY {
            guid: Guid::new(77),
        }
        .into()
    }

    fn activate_query() -> ClientOpcodeMessage {
        CMSG_ACTIVATETAXI {
            guid: Guid::new(77),
            source_node: 255,
            destination_node: 256,
        }
        .into()
    }

    #[test]
    fn an_unanswered_query_is_nonfatal() {
        let store = InMemoryTaxiActions::default();
        let outcome = dispatch_taxi_action(
            &store,
            TaxiActionPlayer { self_guid: Some(9) },
            CMSG_TAXIQUERYAVAILABLENODES {
                guid: Guid::new(77),
            }
            .into(),
        )
        .unwrap();
        assert!(matches!(outcome, TaxiActionOutcome::Handled { outbound } if outbound.is_empty()));
    }

    #[test]
    fn a_transport_loss_ends_the_world_session() {
        let store = InMemoryTaxiActions {
            fail: Some(transport_lost),
            ..Default::default()
        };
        for msg in [status_query(), activate_query()] {
            let result = dispatch_taxi_action(&store, TaxiActionPlayer { self_guid: Some(9) }, msg);
            assert!(
                result.is_err(),
                "a lost transport must end the world session"
            );
        }
    }

    /// A taxi answer arrives only after the request commits, so any failure ends the World Session.
    #[test]
    fn a_refusal_ends_the_world_session() {
        let store = InMemoryTaxiActions {
            fail: Some(refused),
            ..Default::default()
        };
        for msg in [status_query(), activate_query()] {
            let result = dispatch_taxi_action(&store, TaxiActionPlayer { self_guid: Some(9) }, msg);
            assert!(result.is_err(), "a taxi Refusal ends the world session");
        }
    }

    #[test]
    fn no_character_yet_gets_no_taxi_answer_and_no_store_call() {
        let store = InMemoryTaxiActions::default();
        let outcome =
            dispatch_taxi_action(&store, TaxiActionPlayer { self_guid: None }, status_query())
                .unwrap();
        assert!(matches!(outcome, TaxiActionOutcome::Handled { outbound } if outbound.is_empty()));
        assert!(store.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn activate_request_forwards_all_untrusted_fields_and_always_replies() {
        let store = InMemoryTaxiActions::default();
        *store.activation.lock().unwrap() = codec::TaxiActivationResult {
            result_code: lyracore_shared::constants::taxi_protocol::ACTIVATE_NOT_ENOUGH_MONEY,
        };
        let outcome = dispatch_taxi_action(
            &store,
            TaxiActionPlayer { self_guid: Some(9) },
            CMSG_ACTIVATETAXI {
                guid: Guid::new(77),
                source_node: 255,
                destination_node: 256,
            }
            .into(),
        )
        .unwrap();
        let outbound = match outcome {
            TaxiActionOutcome::Activated { outbound, .. } => outbound,
            TaxiActionOutcome::Handled { .. } => panic!("activate returned ordinary outcome"),
            TaxiActionOutcome::PassThrough(_) => panic!("activate must be consumed"),
        };
        assert!(matches!(
            outbound.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_ACTIVATETAXIREPLY(reply))]
                if reply.reply == wow_world_messages::vanilla::ActivateTaxiReply::NotEnoughMoney
        ));
        assert_eq!(
            *store.activation_inputs.lock().unwrap(),
            vec![(9, 77, 255, 256)]
        );
    }

    #[test]
    fn activation_reply_is_queued_before_the_arm_side_effect() {
        let store = InMemoryTaxiActions::default();
        *store.activation.lock().unwrap() = codec::TaxiActivationResult {
            result_code: lyracore_shared::constants::taxi_protocol::ACTIVATE_OK,
        };
        let (tx, rx) = SessionTx::with_depth(0);
        *store.arm_tx_probe.lock().unwrap() = Some(tx.clone());
        let (outbound, arm) = activate_taxi_outbound(&store, Actor::new(9), 77, 255, 256).unwrap();

        queue_reply_then_arm(&tx, &store, outbound, 9, arm).unwrap();

        assert_eq!(*store.arm_observed_depth.lock().unwrap(), Some(1));
        assert!(matches!(
            rx.try_recv(),
            Ok(Outbound::One(ServerOpcodeMessage::SMSG_ACTIVATETAXIREPLY(
                _
            )))
        ));
    }

    #[test]
    fn activation_refusal_is_queued_without_calling_arm() {
        let store = InMemoryTaxiActions::default();
        *store.activation.lock().unwrap() = codec::TaxiActivationResult {
            result_code: lyracore_shared::constants::taxi_protocol::ACTIVATE_NOT_ENOUGH_MONEY,
        };
        let (tx, rx) = SessionTx::with_depth(0);
        *store.arm_tx_probe.lock().unwrap() = Some(tx.clone());
        let (outbound, arm) = activate_taxi_outbound(&store, Actor::new(9), 77, 255, 256).unwrap();

        queue_reply_then_arm(&tx, &store, outbound, 9, arm).unwrap();

        assert_eq!(tx.depth(), 1);
        assert_eq!(*store.arm_observed_depth.lock().unwrap(), None);
        assert!(!store
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.0 == "arm"));
        assert!(
            matches!(rx.try_recv(), Ok(Outbound::One(ServerOpcodeMessage::SMSG_ACTIVATETAXIREPLY(reply))) if reply.reply == wow_world_messages::vanilla::ActivateTaxiReply::NotEnoughMoney)
        );
    }
}
