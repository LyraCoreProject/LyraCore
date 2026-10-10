//! Taxi opcodes through their dispatcher, and taxi gossip over an encrypted World Session.

use super::handlers::InMemoryTaxiActions;
use super::*;
use wow_world_messages::vanilla::ActivateTaxiReply;

const PLAYER: TaxiActionPlayer = TaxiActionPlayer { self_guid: Some(1) };

/// Dispatch one taxi message and return the packets the session would send for it. An activation
/// goes through `queue_reply_then_arm`, as the session does, so the reply is queued before the
/// flight arms.
fn try_run(
    actions: &InMemoryTaxiActions,
    msg: impl Into<ClientOpcodeMessage>,
) -> Result<Vec<Outbound>> {
    match dispatch_taxi_action(actions, PLAYER, msg.into())? {
        TaxiActionOutcome::Handled { outbound } => Ok(outbound),
        TaxiActionOutcome::Activated {
            outbound,
            character_guid,
            arm,
        } => {
            let (tx, rx) = SessionTx::with_depth(0);
            queue_reply_then_arm(&tx, actions, outbound, character_guid, arm)?;
            drop(tx);
            Ok(rx.try_iter().collect())
        }
        TaxiActionOutcome::PassThrough(_) => panic!("the taxi dispatcher passed the message on"),
    }
}

fn run(actions: &InMemoryTaxiActions, msg: impl Into<ClientOpcodeMessage>) -> Vec<Outbound> {
    try_run(actions, msg).unwrap()
}

/// The one typed packet a request answered with.
fn only_message(mut sent: Vec<Outbound>) -> ServerOpcodeMessage {
    match (sent.pop(), sent.is_empty()) {
        (Some(Outbound::One(message)), true) => message,
        _ => panic!("expected exactly one typed packet"),
    }
}

fn taxi_map_90() -> codec::TaxiMapView {
    codec::TaxiMapView {
        npc_guid: 90,
        source_client_node_id: 255,
        available_client_node_ids: vec![255, 256],
    }
}

fn taxi_map_actions() -> InMemoryTaxiActions {
    let actions = InMemoryTaxiActions::default();
    *actions.map.lock().unwrap() = Some(taxi_map_90());
    actions
}

fn activate_90() -> CMSG_ACTIVATETAXI {
    CMSG_ACTIVATETAXI {
        guid: Guid::new(90),
        source_node: 255,
        destination_node: 256,
    }
}

#[test]
fn taxi_status_query_returns_the_persisted_bit_without_opening() {
    // A status answer, not a node map, shows the query did not open the taxi.
    let actions = InMemoryTaxiActions::default();
    *actions.status.lock().unwrap() = Some(codec::TaxiNodeStatusView {
        npc_guid: 90,
        known: false,
    });
    let query = CMSG_TAXINODE_STATUS_QUERY {
        guid: Guid::new(90),
    };
    match only_message(run(&actions, query)) {
        ServerOpcodeMessage::SMSG_TAXINODE_STATUS(status) => {
            assert_eq!(status.guid.guid(), 90);
            assert!(!status.taxi_mask_node_known);
        }
        other => panic!("expected SMSG_TAXINODE_STATUS, got {other}"),
    }
}

#[test]
fn direct_taxi_query_opens_the_taxi_map() {
    let actions = taxi_map_actions();
    let query = CMSG_TAXIQUERYAVAILABLENODES {
        guid: Guid::new(90),
    };
    match only_message(run(&actions, query)) {
        ServerOpcodeMessage::SMSG_SHOWTAXINODES(map) => {
            assert_eq!(map.guid.guid(), 90);
            assert_eq!(map.nearest_node, 255);
            assert_eq!(map.nodes.len(), 8);
            assert_eq!(map.nodes[7], 0xC000_0000);
        }
        other => panic!("expected SMSG_SHOWTAXINODES, got {other}"),
    }
}

#[test]
fn taxi_gossip_opens_the_same_map_as_the_direct_query() {
    // The gossip arm lives in the query handler, so this stays a socket test. It must answer
    // with exactly the packet the direct query builds.
    use lyracore_shared::constants::gossip_option;
    let direct = only_message(run(
        &taxi_map_actions(),
        CMSG_TAXIQUERYAVAILABLENODES {
            guid: Guid::new(90),
        },
    ));

    let mut s = quest_store();
    s.npc.gossip_opts = vec![opt(0, "Show me your flight routes.", gossip_option::TAXI)];
    s.taxi.taxi_map = Some(taxi_map_90());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);

    gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 0,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let gossip = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    assert_eq!(gossip, direct);

    drop(client);
    server.join().unwrap();
}

#[test]
fn activate_taxi_gameplay_refusal_replies_and_does_not_fail_the_next_query() {
    // The Module accepts only 255 -> 256, so the reply also shows the node ids arrived intact.
    let actions = InMemoryTaxiActions {
        route: Some((255, 256)),
        ..Default::default()
    };
    *actions.activation.lock().unwrap() = codec::TaxiActivationResult {
        result_code: lyracore_shared::constants::taxi_protocol::ACTIVATE_NOT_ENOUGH_MONEY,
    };
    *actions.status.lock().unwrap() = Some(codec::TaxiNodeStatusView {
        npc_guid: 90,
        known: true,
    });

    match only_message(run(&actions, activate_90())) {
        ServerOpcodeMessage::SMSG_ACTIVATETAXIREPLY(reply) => {
            assert_eq!(reply.reply, ActivateTaxiReply::NotEnoughMoney);
        }
        other => panic!("expected SMSG_ACTIVATETAXIREPLY, got {other}"),
    }

    // A gameplay refusal is a normal packet result, not a world-session failure.
    let query = CMSG_TAXINODE_STATUS_QUERY {
        guid: Guid::new(90),
    };
    assert!(matches!(
        only_message(run(&actions, query)),
        ServerOpcodeMessage::SMSG_TAXINODE_STATUS(_)
    ));
}

#[test]
fn activate_taxi_success_replies_ok() {
    let actions = InMemoryTaxiActions {
        route: Some((255, 256)),
        ..Default::default()
    };
    *actions.activation.lock().unwrap() = codec::TaxiActivationResult {
        result_code: lyracore_shared::constants::taxi_protocol::ACTIVATE_OK,
    };

    match only_message(run(&actions, activate_90())) {
        ServerOpcodeMessage::SMSG_ACTIVATETAXIREPLY(reply) => {
            assert_eq!(reply.reply, ActivateTaxiReply::Ok);
        }
        other => panic!("expected SMSG_ACTIVATETAXIREPLY, got {other}"),
    }
}

#[test]
fn taxi_gossip_transport_failure_ends_the_world_session() {
    use lyracore_shared::constants::gossip_option;
    let mut s = quest_store();
    s.npc.gossip_opts = vec![opt(0, "Show me your flight routes.", gossip_option::TAXI)];
    s.taxi.taxi_error =
        Some(|| crate::stdb::ReducerCallError::transport_lost("gw_open_taxi").into());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 0,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    server
        .join()
        .expect_err("a broken taxi reducer transport must end the world session");
}
