//! Melee attack opcodes over an encrypted World Session.

use super::*;

#[test]
fn attackswing_ok_replies_attackstart_and_stop_echoes_then_clears() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_ATTACKSWING {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_ATTACKSTART(a) => {
            assert_eq!(a.attacker.guid(), 1, "self guid");
            assert_eq!(a.victim.guid(), 90);
        }
        other => panic!("expected SMSG_ATTACKSTART, got {other}"),
    }
    // Stop echoes the armed target and clears it.
    CMSG_ATTACKSTOP {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_ATTACKSTOP(a) => {
            assert_eq!(a.player.guid(), 1);
            assert_eq!(a.enemy.guid(), 90);
        }
        other => panic!("expected SMSG_ATTACKSTOP, got {other}"),
    }
    // A second stop finds no armed target → NO second echo (sentinel replies first).
    CMSG_ATTACKSTOP {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {}
        other => panic!("expected the sentinel (no ATTACKSTOP echo once cleared), got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn melee_opcodes_at_character_select_answer_nothing_and_keep_the_session_alive() {
    // No WorldEntity yet, so the seam has no attacker guid to name and no combat state to change.
    // The durable calls still go out under the legacy zero actor and the client gets no attack
    // message. The sentinel can only arrive if neither opcode replied or panicked.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_ATTACKSWING {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_ATTACKSTOP {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    CMSG_CHAR_ENUM {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_ENUM(_) => {}
        other => panic!("expected the sentinel (no melee reply at character select), got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.melee.stop_attacks.lock().unwrap().as_slice(),
        &[0],
        "the stop still reaches the durable seam under the legacy zero actor"
    );
}

#[test]
fn attackswing_desync_error_is_session_fatal() {
    // A desync-classified start_attack failure (the player's OWN entity is gone) must PROPAGATE
    // as session-fatal: run_world_session returns Err and the socket tears down for a clean relog.
    // Only the socket half is proved here; which failures are fatal, and which answer a refusal
    // instead, belongs to the melee seam's own tests.
    let mut s = quest_store();
    s.melee.start_attack_error = Some("no live entity for guid 1".into());
    let store = std::sync::Arc::new(s);
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    // Roll enter_world by hand: the server thread must RETURN the session result (not unwrap it).
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.clone()));
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drain_world_entry(&mut client, &mut c_dec);
    CMSG_ATTACKSWING {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let result = server.join().unwrap();
    let err = result.expect_err("a desync on attackswing must end the session with an error");
    assert!(
        format!("{err:#}").contains("desync"),
        "the error should carry the desync context, got: {err:#}"
    );
    drop(client);
}
