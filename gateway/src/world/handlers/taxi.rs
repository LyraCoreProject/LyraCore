//! Taxi query dispatcher. The store operations are cohesive module requests: handlers never read
//! catalogue, discovery, range, or topology tables and therefore cannot fork module policy.

use super::super::*;

pub(crate) trait TaxiActionStore: Send + Sync {
    fn taxi_node_status(
        &self,
        character_guid: u64,
        npc_guid: u64,
    ) -> Result<Option<codec::TaxiNodeStatusView>>;

    fn open_taxi(&self, character_guid: u64, npc_guid: u64) -> Result<Option<codec::TaxiMapView>>;

    fn activate_taxi(
        &self,
        character_guid: u64,
        npc_guid: u64,
        source_client_node_id: u32,
        destination_client_node_id: u32,
    ) -> Result<codec::TaxiActivationResult>;

    fn arm_taxi_flight(&self, character_guid: u64) -> Result<()>;
}

impl TaxiActionStore for crate::stdb::Coordinator {
    fn taxi_node_status(
        &self,
        character_guid: u64,
        npc_guid: u64,
    ) -> Result<Option<codec::TaxiNodeStatusView>> {
        crate::stdb::Coordinator::taxi_node_status(self, character_guid, npc_guid)
    }

    fn open_taxi(&self, character_guid: u64, npc_guid: u64) -> Result<Option<codec::TaxiMapView>> {
        crate::stdb::Coordinator::open_taxi(self, character_guid, npc_guid)
    }

    fn activate_taxi(
        &self,
        character_guid: u64,
        npc_guid: u64,
        source_client_node_id: u32,
        destination_client_node_id: u32,
    ) -> Result<codec::TaxiActivationResult> {
        crate::stdb::Coordinator::activate_taxi(
            self,
            character_guid,
            npc_guid,
            source_client_node_id,
            destination_client_node_id,
        )
    }

    fn arm_taxi_flight(&self, character_guid: u64) -> Result<()> {
        crate::stdb::Coordinator::arm_taxi_flight(self, character_guid)
    }
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
    character_guid: u64,
    npc_guid: u64,
) -> Result<Vec<Outbound>> {
    match store.taxi_node_status(character_guid, npc_guid) {
        Ok(Some(view)) => Ok(vec![Outbound::One(
            ServerOpcodeMessage::SMSG_TAXINODE_STATUS(Box::new(codec::build_taxi_node_status(
                view,
            ))),
        )]),
        Ok(None) => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn activate_taxi_outbound<St: TaxiActionStore + ?Sized>(
    store: &St,
    character_guid: u64,
    npc_guid: u64,
    source_client_node_id: u32,
    destination_client_node_id: u32,
) -> Result<(Vec<Outbound>, bool)> {
    let result = store.activate_taxi(
        character_guid,
        npc_guid,
        source_client_node_id,
        destination_client_node_id,
    )?;
    let arm = result.result_code == lyracore_shared::constants::taxi_protocol::ACTIVATE_OK;
    Ok((
        vec![Outbound::One(ServerOpcodeMessage::SMSG_ACTIVATETAXIREPLY(
            codec::build_activate_taxi_reply(result),
        ))],
        arm,
    ))
}

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
    if arm {
        store.arm_taxi_flight(character_guid)
    } else {
        Ok(())
    }
}

/// The single gateway entry to the module's open operation. Both the direct taxi query and TAXI
/// gossip selection call this exact function.
pub(crate) fn open_taxi_outbound<St: TaxiActionStore + ?Sized>(
    store: &St,
    character_guid: u64,
    npc_guid: u64,
) -> Result<Vec<Outbound>> {
    match store.open_taxi(character_guid, npc_guid) {
        Ok(Some(view)) => Ok(vec![Outbound::One(
            ServerOpcodeMessage::SMSG_SHOWTAXINODES(Box::new(codec::build_show_taxi_nodes(&view))),
        )]),
        Ok(None) => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

pub(crate) fn dispatch_taxi_action<St: TaxiActionStore + ?Sized>(
    store: &St,
    player: TaxiActionPlayer,
    msg: ClientOpcodeMessage,
) -> Result<TaxiActionOutcome> {
    match msg {
        ClientOpcodeMessage::CMSG_TAXINODE_STATUS_QUERY(query) => Ok(TaxiActionOutcome::Handled {
            outbound: status_outbound(store, player.self_guid.unwrap_or(0), query.guid.guid())?,
        }),
        ClientOpcodeMessage::CMSG_TAXIQUERYAVAILABLENODES(query) => {
            Ok(TaxiActionOutcome::Handled {
                outbound: open_taxi_outbound(
                    store,
                    player.self_guid.unwrap_or(0),
                    query.guid.guid(),
                )?,
            })
        }
        ClientOpcodeMessage::CMSG_ACTIVATETAXI(request) => {
            let character_guid = player.self_guid.unwrap_or(0);
            let (outbound, arm) = activate_taxi_outbound(
                store,
                character_guid,
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
mod tests {
    use super::*;
    use std::sync::Mutex;
    use wow_world_messages::{
        vanilla::{CMSG_ACTIVATETAXI, CMSG_TAXINODE_STATUS_QUERY, CMSG_TAXIQUERYAVAILABLENODES},
        Guid,
    };

    #[derive(Default)]
    struct InMemoryTaxiActions {
        calls: Mutex<Vec<(&'static str, u64, u64)>>,
        status: Mutex<Option<codec::TaxiNodeStatusView>>,
        map: Mutex<Option<codec::TaxiMapView>>,
        activation: Mutex<codec::TaxiActivationResult>,
        activation_inputs: Mutex<Vec<(u64, u64, u32, u32)>>,
        fail: bool,
        arm_tx_probe: Mutex<Option<SessionTx>>,
        arm_observed_depth: Mutex<Option<usize>>,
    }

    impl TaxiActionStore for InMemoryTaxiActions {
        fn taxi_node_status(
            &self,
            character_guid: u64,
            npc_guid: u64,
        ) -> Result<Option<codec::TaxiNodeStatusView>> {
            self.calls
                .lock()
                .unwrap()
                .push(("status", character_guid, npc_guid));
            if self.fail {
                anyhow::bail!("offline")
            }
            Ok(*self.status.lock().unwrap())
        }

        fn open_taxi(
            &self,
            character_guid: u64,
            npc_guid: u64,
        ) -> Result<Option<codec::TaxiMapView>> {
            self.calls
                .lock()
                .unwrap()
                .push(("open", character_guid, npc_guid));
            if self.fail {
                anyhow::bail!("offline")
            }
            Ok(self.map.lock().unwrap().clone())
        }

        fn activate_taxi(
            &self,
            character_guid: u64,
            npc_guid: u64,
            source_client_node_id: u32,
            destination_client_node_id: u32,
        ) -> Result<codec::TaxiActivationResult> {
            self.calls
                .lock()
                .unwrap()
                .push(("activate", character_guid, npc_guid));
            self.activation_inputs.lock().unwrap().push((
                character_guid,
                npc_guid,
                source_client_node_id,
                destination_client_node_id,
            ));
            if self.fail {
                anyhow::bail!("offline")
            }
            Ok(*self.activation.lock().unwrap())
        }

        fn arm_taxi_flight(&self, character_guid: u64) -> Result<()> {
            self.calls.lock().unwrap().push(("arm", character_guid, 0));
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

    #[test]
    fn gameplay_refusals_are_nonfatal_but_store_failures_propagate() {
        let refused = InMemoryTaxiActions::default();
        let outcome = dispatch_taxi_action(
            &refused,
            TaxiActionPlayer { self_guid: Some(9) },
            CMSG_TAXIQUERYAVAILABLENODES {
                guid: Guid::new(77),
            }
            .into(),
        )
        .unwrap();
        assert!(matches!(outcome, TaxiActionOutcome::Handled { outbound } if outbound.is_empty()));

        let failed = InMemoryTaxiActions {
            fail: true,
            ..Default::default()
        };
        let result = dispatch_taxi_action(
            &failed,
            TaxiActionPlayer { self_guid: Some(9) },
            CMSG_TAXINODE_STATUS_QUERY {
                guid: Guid::new(77),
            }
            .into(),
        );
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("infrastructure failures must end the world session"),
        };
        assert!(error.to_string().contains("offline"));
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
        let (outbound, arm) = activate_taxi_outbound(&store, 9, 77, 255, 256).unwrap();

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
        let (outbound, arm) = activate_taxi_outbound(&store, 9, 77, 255, 256).unwrap();

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

    #[test]
    fn activate_infrastructure_failure_is_session_fatal() {
        let store = InMemoryTaxiActions {
            fail: true,
            ..Default::default()
        };
        let error = match dispatch_taxi_action(
            &store,
            TaxiActionPlayer { self_guid: Some(9) },
            CMSG_ACTIVATETAXI {
                guid: Guid::new(77),
                source_node: 255,
                destination_node: 256,
            }
            .into(),
        ) {
            Err(error) => error,
            Ok(_) => panic!("transport failure must propagate"),
        };
        assert!(error.to_string().contains("offline"));
    }
}
