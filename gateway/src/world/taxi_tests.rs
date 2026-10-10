//! Taxi opcodes over an encrypted World Session.

use super::*;

#[test]
fn taxi_status_query_returns_the_persisted_bit_without_opening() {
    let mut s = quest_store();
    s.taxi.taxi_status = Some(codec::TaxiNodeStatusView {
        npc_guid: 90,
        known: false,
    });
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_TAXINODE_STATUS_QUERY {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_TAXINODE_STATUS(status) => {
            assert_eq!(status.guid.guid(), 90);
            assert!(!status.taxi_mask_node_known);
        }
        other => panic!("expected SMSG_TAXINODE_STATUS, got {other}"),
    }
    assert_eq!(
        *store.taxi.taxi_calls.lock().unwrap(),
        vec![("status", 1, 90)]
    );
    drop(client);
    server.join().unwrap();
}

#[test]
fn direct_taxi_query_and_taxi_gossip_share_one_open_operation() {
    use lyracore_shared::constants::gossip_option;
    let mut s = quest_store();
    s.npc.gossip_opts = vec![opt(0, "Show me your flight routes.", gossip_option::TAXI)];
    s.taxi.taxi_map = Some(codec::TaxiMapView {
        npc_guid: 90,
        source_client_node_id: 255,
        available_client_node_ids: vec![255, 256],
    });
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_TAXIQUERYAVAILABLENODES {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_SHOWTAXINODES(map) => {
            assert_eq!(map.guid.guid(), 90);
            assert_eq!(map.nearest_node, 255);
            assert_eq!(map.nodes.len(), 8);
            assert_eq!(map.nodes[7], 0xC000_0000);
        }
        other => panic!("expected SMSG_SHOWTAXINODES, got {other}"),
    }

    gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 0,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_SHOWTAXINODES(map) => {
            assert_eq!(map.nearest_node, 255);
            assert_eq!(map.nodes[7], 0xC000_0000);
        }
        other => panic!("expected SMSG_SHOWTAXINODES, got {other}"),
    }

    assert_eq!(
        *store.taxi.taxi_calls.lock().unwrap(),
        vec![("open", 1, 90), ("open", 1, 90)]
    );
    drop(client);
    server.join().unwrap();
}

#[test]
fn activate_taxi_gameplay_refusal_replies_and_keeps_the_socket_alive() {
    let mut s = quest_store();
    s.taxi.taxi_activation = codec::TaxiActivationResult {
        result_code: lyracore_shared::constants::taxi_protocol::ACTIVATE_NOT_ENOUGH_MONEY,
    };
    s.taxi.taxi_status = Some(codec::TaxiNodeStatusView {
        npc_guid: 90,
        known: true,
    });
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_ACTIVATETAXI {
        guid: Guid::new(90),
        source_node: 255,
        destination_node: 256,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_ACTIVATETAXIREPLY(reply) => assert_eq!(
            reply.reply,
            wow_world_messages::vanilla::ActivateTaxiReply::NotEnoughMoney
        ),
        other => panic!("expected SMSG_ACTIVATETAXIREPLY, got {other}"),
    }
    assert_eq!(
        *store.taxi.taxi_activation_inputs.lock().unwrap(),
        vec![(1, 90, 255, 256)]
    );

    // A gameplay refusal is a normal packet result, not a world-session failure.
    CMSG_TAXINODE_STATUS_QUERY {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    assert!(matches!(
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap(),
        ServerOpcodeMessage::SMSG_TAXINODE_STATUS(_)
    ));
    drop(client);
    server.join().unwrap();
}

#[test]
fn activate_taxi_success_round_trips_over_the_encrypted_socket() {
    let mut s = quest_store();
    s.taxi.taxi_activation = codec::TaxiActivationResult {
        result_code: lyracore_shared::constants::taxi_protocol::ACTIVATE_OK,
    };
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_ACTIVATETAXI {
        guid: Guid::new(90),
        source_node: 255,
        destination_node: 256,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_ACTIVATETAXIREPLY(reply) => assert_eq!(
            reply.reply,
            wow_world_messages::vanilla::ActivateTaxiReply::Ok
        ),
        other => panic!("expected SMSG_ACTIVATETAXIREPLY, got {other}"),
    }
    assert_eq!(
        *store.taxi.taxi_activation_inputs.lock().unwrap(),
        vec![(1, 90, 255, 256)]
    );

    drop(client);
    server.join().unwrap();
}

#[test]
fn taxi_gossip_transport_failure_ends_the_world_session() {
    use lyracore_shared::constants::gossip_option;
    let mut s = quest_store();
    s.npc.gossip_opts = vec![opt(0, "Show me your flight routes.", gossip_option::TAXI)];
    s.taxi.taxi_error = Some("taxi reducer transport disconnected: channel closed".into());
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
