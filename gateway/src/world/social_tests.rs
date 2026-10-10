//! Friends and ignore lists over an encrypted World Session.

use super::*;

/// Read one `SMSG_FRIEND_STATUS` frame — RAW always, now that `ONLINE`/`ADDED_ONLINE` carry
/// trailing fields gtker 0.3 cannot type (see `codec::build_friend_status_raw`). Returns the
/// result, the other party's guid, and the trailing bytes (empty unless online).
fn read_friend_status(
    client: &mut UnixStream,
    dec: &mut DecrypterHalf,
) -> (FriendResult, u64, Vec<u8>) {
    let (opcode, body) = read_raw_frame(client, dec);
    assert_eq!(
        opcode,
        codec::social::SMSG_FRIEND_STATUS_OPCODE,
        "expected SMSG_FRIEND_STATUS"
    );
    let result = FriendResult::try_from(body[0]).expect("a known FriendResult byte");
    let guid = u64::from_le_bytes(body[1..9].try_into().unwrap());
    (result, guid, body[9..].to_vec())
}

#[test]
fn add_friend_by_name_then_friend_list_carries_online_presence() {
    // CMSG_ADD_FRIEND "Buddy" -> SMSG_FRIEND_STATUS AddedOnline (guid 2, resolved by
    // name); a follow-up CMSG_FRIEND_LIST then carries them online with their level/class/zone.
    let mut s = quest_store();
    s.characters = vec![codec::CharacterView {
        guid: 2,
        name: "Buddy".into(),
        class: 1,
        level: 10,
        zone_id: 12,
        ..Default::default()
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_ADD_FRIEND {
        name: "buddy".into(),
    } // case-insensitive match
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (result, guid, trailer) = read_friend_status(&mut client, &mut c_dec);
    assert_eq!(result, FriendResult::AddedOnline);
    assert_eq!(guid, 2);
    assert_eq!(trailer[0], 1, "status ONLINE");

    CMSG_FRIEND_LIST {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_FRIEND_LIST(l) => {
            assert_eq!(l.friends.len(), 1);
            assert_eq!(l.friends[0].guid.guid(), 2);
            assert!(matches!(
                l.friends[0].status,
                wow_world_messages::vanilla::Friend_FriendStatus::Online { .. }
            ));
        }
        other => panic!("expected SMSG_FRIEND_LIST, got {other}"),
    }
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_IGNORE_LIST(l) => assert!(l.ignored.is_empty()),
        other => panic!("expected SMSG_IGNORE_LIST, got {other}"),
    }

    // Removing it replies Removed and the friend list empties out.
    CMSG_DEL_FRIEND { guid: Guid::new(2) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    let (result, _, trailer) = read_friend_status(&mut client, &mut c_dec);
    assert_eq!(result, FriendResult::Removed);
    assert!(trailer.is_empty(), "a remove carries no trailing fields");
    drop(client);
    server.join().unwrap();
}

/// A friend on ANOTHER Shard resolves. Vim lives on `instances`; Ginger (this Gateway's own
/// Shard) adds them by name and gets ADDED_ONLINE with Vim's real presence, byte-exact against
/// the raw encoder.
#[test]
fn add_friend_on_another_shard_answers_added_online_with_presence() {
    let mut s = quest_store();
    s.characters = vec![codec::CharacterView {
        guid: 1,
        name: "Ginger".into(),
        race: 1,
        ..Default::default()
    }];
    let store = std::sync::Arc::new(s);
    let peer = std::sync::Arc::new(WorldFake {
        topology: TopologyState {
            shard: "instances".into(),
            ..Default::default()
        },
        characters: vec![codec::CharacterView {
            guid: 2,
            name: "Vim".into(),
            race: 1, // same team as Ginger
            class: 4,
            ..Default::default()
        }],
        social: SocialState {
            // Vim standing IN the instance: a live entity, not just a durable row, so the ADDED_ONLINE
            // fields below come off `Whereabouts::InWorld`.
            member_entities: std::sync::Mutex::new(vec![(
                2,
                codec::MemberEntity {
                    level: 22,
                    zone_id: 33,
                    ..Default::default()
                },
            )]),
            ..Default::default()
        },
        ..Default::default()
    });
    *store.topology.peers.lock().unwrap() = vec![store.clone(), peer.clone()];
    *peer.topology.peers.lock().unwrap() = vec![store.clone(), peer.clone()];

    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_ADD_FRIEND { name: "Vim".into() }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    let (result, guid, trailer) = read_friend_status(&mut client, &mut c_dec);
    assert_eq!(result, FriendResult::AddedOnline);
    assert_eq!(guid, 2);
    let mut expected = vec![1u8]; // status ONLINE
    expected.extend(33u32.to_le_bytes()); // area
    expected.extend(22u32.to_le_bytes()); // level
    expected.extend(4u32.to_le_bytes()); // class
    assert_eq!(trailer, expected);
    drop(client);
    server.join().unwrap();
}

/// A friend standing in another Shard's instance shows ONLINE with real level/class/zone through
/// `CMSG_FRIEND_LIST`, and AFK/DND status follow the live entity's `PLAYER_FLAGS` — exercised
/// through the actual socket dispatch, so it runs the SAME `world::social::friend_views`
/// composition a Coordinator serves, not a Store Fake's own reimplementation of it.
#[test]
fn friend_list_shows_a_friend_in_another_shards_instance_online_afk_or_dnd() {
    let mut s = quest_store();
    s.characters = vec![codec::CharacterView {
        guid: 1,
        name: "Ginger".into(),
        race: 1,
        ..Default::default()
    }];
    s.social.contacts = std::sync::Mutex::new(vec![(1, 2, false)]); // Ginger already friends Vim
    let store = std::sync::Arc::new(s);
    let peer = std::sync::Arc::new(WorldFake {
        topology: TopologyState {
            shard: "instances".into(),
            ..Default::default()
        },
        characters: vec![codec::CharacterView {
            guid: 2,
            name: "Vim".into(),
            race: 1, // same team as Ginger
            class: 4,
            ..Default::default()
        }],
        social: SocialState {
            member_entities: std::sync::Mutex::new(vec![(
                2,
                codec::MemberEntity {
                    level: 22,
                    zone_id: 33,
                    player_flags: lyracore_shared::constants::player_flags::DND,
                    ..Default::default()
                },
            )]),
            ..Default::default()
        },
        ..Default::default()
    });
    *store.topology.peers.lock().unwrap() = vec![store.clone(), peer.clone()];
    *peer.topology.peers.lock().unwrap() = vec![store.clone(), peer.clone()];

    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_FRIEND_LIST {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_FRIEND_LIST(l) => {
            assert_eq!(l.friends.len(), 1);
            assert_eq!(l.friends[0].guid.guid(), 2);
            match l.friends[0].status {
                wow_world_messages::vanilla::Friend_FriendStatus::Dnd { area, class, level } => {
                    assert_eq!(area.as_int(), 33);
                    assert_eq!(level.as_int(), 22);
                    assert_eq!(class, wow_world_messages::vanilla::Class::Rogue);
                }
                other => panic!("expected Dnd, got {other:?}"),
            }
        }
        other => panic!("expected SMSG_FRIEND_LIST, got {other}"),
    }
    // CMSG_FRIEND_LIST always answers with BOTH lists; leaving the SMSG_IGNORE_LIST reply unread
    // and dropping the client closes the socket with a queued write still in flight, which Linux
    // reports to the server as a reset rather than a clean EOF.
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_IGNORE_LIST(l) => assert!(l.ignored.is_empty()),
        other => panic!("expected SMSG_IGNORE_LIST, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

/// An enemy-faction friend answers FRIEND_ENEMY. The companion half — the same target is a legal
/// ignore — is a Module rule this Store Fake cannot express; it is pinned durably in
/// `module/tests/friends.rs`.
#[test]
fn add_friend_enemy_faction_answers_friend_enemy() {
    let mut s = quest_store();
    s.characters = vec![
        codec::CharacterView {
            guid: 1,
            name: "Ginger".into(),
            race: 1, // Human, Alliance
            ..Default::default()
        },
        codec::CharacterView {
            guid: 2,
            name: "Grunt".into(),
            race: 2, // Orc, Horde
            ..Default::default()
        },
    ];
    s.trade_error = Some(ContactRefusal::Enemy.as_tag().to_string());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_ADD_FRIEND {
        name: "Grunt".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (result, guid, trailer) = read_friend_status(&mut client, &mut c_dec);
    assert_eq!(result, FriendResult::Enemy);
    assert_eq!(guid, 2);
    assert!(trailer.is_empty());
    drop(client);
    server.join().unwrap();
}

#[test]
fn add_friend_unknown_name_replies_not_found() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_ADD_FRIEND {
        name: "Nobody".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (result, guid, _) = read_friend_status(&mut client, &mut c_dec);
    assert_eq!(result, FriendResult::NotFound);
    assert_eq!(guid, 0);
    drop(client);
    server.join().unwrap();
}

/// The ignore list has its own "unknown name" code.
#[test]
fn add_ignore_unknown_name_replies_ignore_not_found() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_ADD_IGNORE {
        name: "Nobody".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (result, guid, _) = read_friend_status(&mut client, &mut c_dec);
    assert_eq!(result, FriendResult::IgnoreNotFound);
    assert_eq!(guid, 0);
    drop(client);
    server.join().unwrap();
}

#[test]
fn add_ignore_by_name_replies_ignore_added() {
    let mut s = quest_store();
    s.characters = vec![codec::CharacterView {
        guid: 3,
        name: "Pest".into(),
        ..Default::default()
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_ADD_IGNORE {
        name: "Pest".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (result, guid, _) = read_friend_status(&mut client, &mut c_dec);
    assert_eq!(result, FriendResult::IgnoreAdded);
    assert_eq!(guid, 3);
    drop(client);
    server.join().unwrap();
}

#[test]
fn add_friend_maps_self_already_and_full_errors() {
    for (refusal, want) in [
        (ContactRefusal::AddSelf, FriendResult::SelfX),
        (ContactRefusal::AlreadyOnList, FriendResult::Already),
        (ContactRefusal::ListFull, FriendResult::ListFull),
    ] {
        let mut s = quest_store();
        s.characters = vec![codec::CharacterView {
            guid: 2,
            name: "Buddy".into(),
            ..Default::default()
        }];
        s.trade_error = Some(refusal.as_tag().to_string());
        let store = std::sync::Arc::new(s);
        let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
        CMSG_ADD_FRIEND {
            name: "Buddy".into(),
        }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
        let (result, ..) = read_friend_status(&mut client, &mut c_dec);
        assert_eq!(result, want, "{refusal:?} must map to {want:?}");
        drop(client);
        server.join().unwrap();
    }
}

#[test]
fn add_ignore_maps_already_and_full_errors_to_the_ignore_variants() {
    for (refusal, want) in [
        (ContactRefusal::AddSelf, FriendResult::IgnoreSelf),
        (ContactRefusal::AlreadyOnList, FriendResult::IgnoreAlready),
        (ContactRefusal::ListFull, FriendResult::IgnoreFull),
    ] {
        let mut s = quest_store();
        s.characters = vec![codec::CharacterView {
            guid: 3,
            name: "Pest".into(),
            ..Default::default()
        }];
        s.trade_error = Some(refusal.as_tag().to_string());
        let store = std::sync::Arc::new(s);
        let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
        CMSG_ADD_IGNORE {
            name: "Pest".into(),
        }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
        let (result, ..) = read_friend_status(&mut client, &mut c_dec);
        assert_eq!(result, want, "{refusal:?} must map to {want:?}");
        drop(client);
        server.join().unwrap();
    }
}

/// Every contact Refusal the Module can send reaches the client as exactly one `FriendResult`, and
/// the friends and ignore lists get their own code family for the same condition.
#[test]
fn every_contact_refusal_reaches_the_client_as_one_friend_result() {
    for refusal in ContactRefusal::ALL {
        let (friend, ignore) = match refusal {
            ContactRefusal::AddSelf => (FriendResult::SelfX, FriendResult::IgnoreSelf),
            ContactRefusal::AlreadyOnList => (FriendResult::Already, FriendResult::IgnoreAlready),
            ContactRefusal::ListFull => (FriendResult::ListFull, FriendResult::IgnoreFull),
            ContactRefusal::NotOnList => (FriendResult::NotFound, FriendResult::IgnoreNotFound),
            ContactRefusal::ActorUnavailable => (FriendResult::NotFound, FriendResult::NotFound),
            ContactRefusal::Enemy => (FriendResult::Enemy, FriendResult::Enemy),
        };
        for (is_ignore, want) in [(false, friend), (true, ignore)] {
            let mut s = quest_store();
            s.characters = vec![codec::CharacterView {
                guid: 2,
                name: "Buddy".into(),
                ..Default::default()
            }];
            s.trade_error = Some(refusal.as_tag().to_string());
            let store = std::sync::Arc::new(s);
            let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
            if is_ignore {
                CMSG_ADD_IGNORE {
                    name: "Buddy".into(),
                }
                .write_encrypted_client(&mut client, &mut c_enc)
                .unwrap();
            } else {
                CMSG_ADD_FRIEND {
                    name: "Buddy".into(),
                }
                .write_encrypted_client(&mut client, &mut c_enc)
                .unwrap();
            }
            let (result, ..) = read_friend_status(&mut client, &mut c_dec);
            assert_eq!(result, want, "{refusal:?} on ignore={is_ignore}");
            drop(client);
            server.join().unwrap();
        }
    }
}

/// The contact half of the same rule: a timed-out add left the list in an unknown state.
#[test]
fn an_add_friend_timeout_is_not_answered_as_a_refusal() {
    let mut s = quest_store();
    s.characters = vec![codec::CharacterView {
        guid: 2,
        name: "Buddy".into(),
        ..Default::default()
    }];
    s.session.login_entity = Some(warrior_entity());
    s.trade_error = Some("gw_add_friend reducer timed out after 10s".into());
    let store = std::sync::Arc::new(s);
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
    CMSG_ADD_FRIEND {
        name: "Buddy".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let error = result_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("an unknown contact outcome must end the session promptly")
        .expect_err("a timed-out contact reducer must be session-fatal");
    assert!(format!("{error:#}").contains("timed out"));
}

#[test]
fn del_friend_unknown_contact_replies_not_found() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_DEL_FRIEND {
        guid: Guid::new(404),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (result, ..) = read_friend_status(&mut client, &mut c_dec);
    assert_eq!(result, FriendResult::NotFound);
    drop(client);
    server.join().unwrap();
}

#[test]
fn del_ignore_round_trips_added_then_unknown_is_ignore_not_found() {
    let mut s = quest_store();
    s.characters = vec![codec::CharacterView {
        guid: 3,
        name: "Pest".into(),
        ..Default::default()
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);

    CMSG_ADD_IGNORE {
        name: "Pest".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (result, ..) = read_friend_status(&mut client, &mut c_dec);
    assert_eq!(result, FriendResult::IgnoreAdded);

    // Removing the just-added contact: IgnoreRemoved.
    CMSG_DEL_IGNORE { guid: Guid::new(3) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    let (result, ..) = read_friend_status(&mut client, &mut c_dec);
    assert_eq!(result, FriendResult::IgnoreRemoved);
    // Removing it again: no longer on the list -> IgnoreNotFound.
    CMSG_DEL_IGNORE { guid: Guid::new(3) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    let (result, ..) = read_friend_status(&mut client, &mut c_dec);
    assert_eq!(result, FriendResult::IgnoreNotFound);
    drop(client);
    server.join().unwrap();
}
