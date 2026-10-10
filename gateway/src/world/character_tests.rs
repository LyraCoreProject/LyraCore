//! Character select over an encrypted World Session: enumerate, create and delete.

use super::*;

#[test]
fn char_enum_returns_the_seeded_character() {
    // The pre-seeded Human Warrior "Tester" must appear on the character-select screen.
    let tester = codec::CharacterView {
        guid: 1,
        name: "Tester".into(),
        race: 1,   // Human
        class: 1,  // Warrior
        gender: 0, // Male
        level: 1,
        map_id: 0,   // Eastern Kingdoms
        zone_id: 12, // Elwynn Forest
        x: -8949.95,
        y: -132.493,
        z: 83.5312,
        first_login: true,
        ..Default::default()
    };
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            characters: vec![tester],
            ..base
        }
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });

    // Full handshake, then request the character list over the encrypted channel.
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_CHAR_ENUM {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_ENUM(e) => {
            assert_eq!(e.characters.len(), 1);
            let ch = &e.characters[0];
            assert_eq!(ch.name, "Tester");
            assert_eq!(ch.guid.guid(), 1);
            assert_eq!(ch.race, Race::Human);
            assert_eq!(ch.class, Class::Warrior);
            assert_eq!(ch.level, Level::new(1));
            assert_eq!(ch.map, Map::EasternKingdoms);
        }
        other => panic!("expected SMSG_CHAR_ENUM, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn char_create_replies_success_then_name_in_use() {
    // The fake store reports a name already among its characters as in-use, else success.
    let tester = codec::CharacterView {
        guid: 1,
        name: "Tester".into(),
        race: 1,
        class: 1,
        level: 1,
        ..Default::default()
    };
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            characters: vec![tester],
            ..base
        }
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    let mk = |name: &str| CMSG_CHAR_CREATE {
        name: name.to_string(),
        race: Race::Human,
        class: Class::Warrior,
        gender: Gender::Male,
        skin_color: 0,
        face: 0,
        hair_style: 0,
        hair_color: 0,
        facial_hair: 0,
    };

    // A fresh name → CharCreateSuccess.
    mk("Newbie")
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_CREATE(m) => {
            assert_eq!(m.result, WorldResult::CharCreateSuccess)
        }
        other => panic!("expected SMSG_CHAR_CREATE, got {other}"),
    }

    // An existing name → CharCreateNameInUse, and the session stays alive.
    mk("Tester")
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_CREATE(m) => {
            assert_eq!(m.result, WorldResult::CharCreateNameInUse)
        }
        other => panic!("expected SMSG_CHAR_CREATE, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn char_delete_replies_success_and_dispatches_owned_guid() {
    let tester = codec::CharacterView {
        guid: 5,
        name: "Tester".into(),
        race: 1,
        class: 1,
        level: 1,
        ..Default::default()
    };
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            characters: vec![tester],
            ..base
        }
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_CHAR_DELETE { guid: Guid::new(5) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_DELETE(m) => {
            assert_eq!(m.result, WorldResult::CharDeleteSuccess)
        }
        other => panic!("expected SMSG_CHAR_DELETE, got {other}"),
    }
    assert_eq!(*store.character.deleted.lock().unwrap(), vec![(7, 5)]);

    drop(client);
    server.join().unwrap();
}

#[test]
fn char_delete_failure_replies_failed_and_keeps_session_alive() {
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            character: CharacterState {
                delete_outcome: Some(codec::CharDeleteOutcome::Failed),
                ..base.character
            },
            ..base
        }
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_CHAR_DELETE {
        guid: Guid::new(999),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_DELETE(m) => {
            assert_eq!(m.result, WorldResult::CharDeleteFailed)
        }
        other => panic!("expected SMSG_CHAR_DELETE, got {other}"),
    }

    // Session should still be alive: CMSG_CHAR_ENUM gets a normal reply, not a dropped socket.
    CMSG_CHAR_ENUM {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_ENUM(_) => {}
        other => panic!("expected SMSG_CHAR_ENUM, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}
