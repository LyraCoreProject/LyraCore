//! Death and resurrection opcodes over an encrypted World Session.

use super::*;

#[test]
fn repop_request_dispatches_repop_for_the_caller() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_REPOP_REQUEST {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client); // repop's revive replicates via the entity VALUES relay, not a direct SMSG here
    server.join().unwrap();
    assert_eq!(store.death.repopped.lock().unwrap().as_slice(), &[1]);
}

#[test]
fn reclaim_corpse_dispatches_with_the_wire_corpse_guid() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_RECLAIM_CORPSE {
        guid: Guid::new(777),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.death.reclaimed_corpses.lock().unwrap().as_slice(),
        &[(1, 777)]
    );
}

#[test]
fn resurrect_response_accept_maps_status_byte_to_true() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_RESURRECT_RESPONSE {
        guid: Guid::new(42),
        status: 1,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.death.resurrect_responses.lock().unwrap().as_slice(),
        &[(1, true)]
    );
}

#[test]
fn resurrect_response_decline_maps_status_byte_to_false() {
    // Proves the `status != 0` mapping actually distinguishes decline from accept, not just that
    // SOME boolean reaches the store.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_RESURRECT_RESPONSE {
        guid: Guid::new(42),
        status: 0,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.death.resurrect_responses.lock().unwrap().as_slice(),
        &[(1, false)]
    );
}

#[test]
fn self_res_dispatches_self_resurrect_for_the_caller() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_SELF_RES {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client); // the revive replicates via the entity VALUES relay, not a direct SMSG here
    server.join().unwrap();
    assert_eq!(store.death.self_resurrects.lock().unwrap().as_slice(), &[1]);
}

#[test]
fn a_refused_self_res_sends_nothing_and_keeps_the_session() {
    let store = std::sync::Arc::new({
        let base = quest_store();
        WorldFake {
            death: DeathState {
                self_resurrect_error: Some("no Self-Resurrection Option".into()),
                ..base.death
            },
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_SELF_RES {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {}
        other => panic!("expected the sentinel (the Refusal sends nothing), got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(store.death.self_resurrects.lock().unwrap().as_slice(), &[1]);
}

#[test]
fn spirit_healer_activate_dispatches_and_confirms_the_healer_guid() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_SPIRIT_HEALER_ACTIVATE {
        guid: Guid::new(888),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_SPIRIT_HEALER_CONFIRM(p) => {
            assert_eq!(p.guid, Guid::new(888), "echoes the healer's own guid");
        }
        other => panic!("expected SMSG_SPIRIT_HEALER_CONFIRM, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    // The SMSG above echoes the WIRE guid verbatim, so it alone can't catch a swapped-argument bug —
    // this pins that the STORE call also got (self_guid, healer_guid) in the right order.
    assert_eq!(
        store.death.spirit_healer_calls.lock().unwrap().as_slice(),
        &[(1, 888)]
    );
}
