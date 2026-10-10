//! Item actions over an encrypted World Session.

use super::*;

#[test]
fn equip_item_err_sends_smsg_inventory_change_failure() {
    // Socket contract: a gameplay refusal reaches the client as an encrypted
    // SMSG_INVENTORY_CHANGE_FAILURE frame and the session keeps serving the next action.
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            trade_error: Some(ItemRefusal::CannotEquip.as_tag().into()),
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    // Slot 24 is a backpack slot (>= 23); bag 255 = INVENTORY_SLOT_BAG_0 (main bag).
    CMSG_AUTOEQUIP_ITEM {
        source_bag: 255,
        source_slot: 24,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(_) => {} // correct feedback packet
        other => panic!("expected SMSG_INVENTORY_CHANGE_FAILURE, got {other}"),
    }
    CMSG_AUTOEQUIP_ITEM {
        source_bag: 255,
        source_slot: 25,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(_) => {}
        other => panic!("expected a second SMSG_INVENTORY_CHANGE_FAILURE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn item_action_before_player_login_is_handled_without_panicking() {
    // Socket contract: an item frame arriving after the handshake but before CMSG_PLAYER_LOGIN
    // (no selected player) must not panic or error the session thread.
    let store = std::sync::Arc::new(tester_store(7));
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.clone()));
    let (mut c_enc, _c_dec) = client_handshake(&mut client, "TESTER", K);

    CMSG_AUTOEQUIP_ITEM {
        source_bag: 255,
        source_slot: 24,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);

    server
        .join()
        .expect("an item action without a selected player must not panic")
        .expect("the legacy zero-actor fallback remains a handled gameplay context");
}

#[test]
fn item_reducer_transport_loss_ends_the_world_session() {
    // Socket contract: reducer transport loss ends the session with an error and closes the
    // socket instead of being translated into gameplay feedback.
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            trade_error: Some("equip_item reducer transport disconnected: channel closed".into()),
            ..base
        }
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        result_tx
            .send(run_world_session(server_end, server_store.clone()))
            .unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drain_world_entry(&mut client, &mut c_dec);

    CMSG_AUTOEQUIP_ITEM {
        source_bag: 255,
        source_slot: 24,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    let error = result_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("transport loss must end the session promptly")
        .expect_err("a disconnected item reducer transport must be session-fatal");
    assert!(format!("{error:#}").contains("reducer transport disconnected"));
    assert!(
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).is_err(),
        "the socket closes instead of translating transport loss into gameplay feedback"
    );
}
