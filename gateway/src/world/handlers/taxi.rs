//! Taxi query dispatcher. The store operations are cohesive module requests: handlers never read
//! catalogue, discovery, range, or topology tables and therefore cannot fork module policy.

use super::super::*;
use crate::world::family::{
    ProtocolFamily, ProtocolReply, ProtocolRequest, ProtocolSession, WorldSessionAction,
};

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

pub(crate) struct Taxi;

impl<St: TaxiActionStore + ?Sized> ProtocolFamily<St> for Taxi {
    fn handle(
        store: &St,
        session: &mut ProtocolSession,
        request: ProtocolRequest,
    ) -> Result<ProtocolReply> {
        let msg = request.message()?;
        let actor = session.actor();
        match msg {
            ClientOpcodeMessage::CMSG_TAXINODE_STATUS_QUERY(query) => Ok(ProtocolReply::from(
                status_outbound(store, actor, query.guid.guid())?,
            )),
            ClientOpcodeMessage::CMSG_TAXIQUERYAVAILABLENODES(query) => Ok(ProtocolReply::from(
                open_taxi_outbound(store, actor, query.guid.guid())?,
            )),
            ClientOpcodeMessage::CMSG_ACTIVATETAXI(request) => {
                let (outbound, arm) = activate_taxi_outbound(
                    store,
                    actor,
                    request.guid.guid(),
                    request.source_node,
                    request.destination_node,
                )?;
                Ok(ProtocolReply {
                    outbound,
                    after_queue: actor.filter(|_| arm).map(WorldSessionAction::ArmTaxi),
                })
            }
            other => Err(anyhow!("request routed to Taxi: {other}")),
        }
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
            Ok(())
        }
    }

    fn run_taxi(
        store: &InMemoryTaxiActions,
        mut session: ProtocolSession,
        message: ClientOpcodeMessage,
    ) -> Result<ProtocolReply> {
        Taxi::handle(store, &mut session, message.into())
    }

    #[test]
    fn accepted_activation_requests_arming_after_the_reply_is_queued() {
        let store = InMemoryTaxiActions::default();
        *store.activation.lock().unwrap() = codec::TaxiActivationResult {
            result_code: lyracore_shared::constants::taxi_protocol::ACTIVATE_OK,
        };
        let reply = run_taxi(&store, ProtocolSession::in_world(7, 9), activate_query()).unwrap();
        assert!(matches!(
            reply.outbound.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_ACTIVATETAXIREPLY(
                _
            ))]
        ));
        assert!(
            matches!(reply.after_queue, Some(WorldSessionAction::ArmTaxi(actor)) if actor.guid() == 9)
        );
        assert!(!store
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.0 == "arm"));
    }

    #[test]
    fn refused_activation_does_not_request_arming() {
        let store = InMemoryTaxiActions::default();
        *store.activation.lock().unwrap() = codec::TaxiActivationResult {
            result_code: lyracore_shared::constants::taxi_protocol::ACTIVATE_NOT_ENOUGH_MONEY,
        };
        let reply = run_taxi(&store, ProtocolSession::in_world(7, 9), activate_query()).unwrap();
        assert!(reply.after_queue.is_none());
        assert!(
            matches!(reply.outbound.as_slice(), [Outbound::One(ServerOpcodeMessage::SMSG_ACTIVATETAXIREPLY(reply))] if reply.reply == wow_world_messages::vanilla::ActivateTaxiReply::NotEnoughMoney)
        );
    }

    #[test]
    fn status_query_returns_persisted_state_without_opening() {
        let store = InMemoryTaxiActions::default();
        *store.status.lock().unwrap() = Some(codec::TaxiNodeStatusView {
            npc_guid: 77,
            known: false,
        });
        let outcome = run_taxi(
            &store,
            ProtocolSession::in_world(7, 9),
            CMSG_TAXINODE_STATUS_QUERY {
                guid: Guid::new(77),
            }
            .into(),
        )
        .unwrap();
        assert!(matches!(outcome, ProtocolReply { outbound, .. } if outbound.len() == 1));
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
        let outcome = run_taxi(
            &store,
            ProtocolSession::in_world(7, 9),
            CMSG_TAXIQUERYAVAILABLENODES {
                guid: Guid::new(77),
            }
            .into(),
        )
        .unwrap();
        assert!(matches!(outcome, ProtocolReply { outbound, .. } if outbound.len() == 1));
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
        let outcome = run_taxi(
            &store,
            ProtocolSession::in_world(7, 9),
            CMSG_TAXIQUERYAVAILABLENODES {
                guid: Guid::new(77),
            }
            .into(),
        )
        .unwrap();
        assert!(matches!(outcome, ProtocolReply { outbound, .. } if outbound.is_empty()));
    }

    #[test]
    fn a_transport_loss_ends_the_world_session() {
        let store = InMemoryTaxiActions {
            fail: Some(transport_lost),
            ..Default::default()
        };
        for msg in [status_query(), activate_query()] {
            let result = run_taxi(&store, ProtocolSession::in_world(7, 9), msg);
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
            let result = run_taxi(&store, ProtocolSession::in_world(7, 9), msg);
            assert!(result.is_err(), "a taxi Refusal ends the world session");
        }
    }

    #[test]
    fn no_character_yet_gets_no_taxi_answer_and_no_store_call() {
        let store = InMemoryTaxiActions::default();
        let outcome = run_taxi(
            &store,
            ProtocolSession::new(7, "TESTER".into()),
            status_query(),
        )
        .unwrap();
        assert!(matches!(outcome, ProtocolReply { outbound, .. } if outbound.is_empty()));
        assert!(store.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn activate_request_forwards_all_untrusted_fields_and_always_replies() {
        let store = InMemoryTaxiActions::default();
        *store.activation.lock().unwrap() = codec::TaxiActivationResult {
            result_code: lyracore_shared::constants::taxi_protocol::ACTIVATE_NOT_ENOUGH_MONEY,
        };
        let outcome = run_taxi(
            &store,
            ProtocolSession::in_world(7, 9),
            CMSG_ACTIVATETAXI {
                guid: Guid::new(77),
                source_node: 255,
                destination_node: 256,
            }
            .into(),
        )
        .unwrap();
        let outbound = outcome.outbound;
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
}
