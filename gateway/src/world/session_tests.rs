use super::*;

#[test]
fn world_session_socket_pair_times_out_when_the_server_writes_nothing() {
    let (mut client, _server) = world_session_socket_pair();
    let mut byte = [0];
    let error = client
        .read(&mut byte)
        .expect_err("a silent server must hit the world-session read deadline");
    assert!(
        matches!(
            error.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        ),
        "a silent server must time out, got {error}"
    );
}

#[test]
fn handshake_succeeds_and_traffic_is_encrypted_both_ways() {
    let store = std::sync::Arc::new(tester_store(42));

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        let mut s = server_end;
        let (mut conn, _encrypt) = world_handshake(&mut s, server_store.clone())
            .unwrap()
            .expect("handshake should succeed");
        assert_eq!(conn.account_id, 42);
        assert_eq!(conn.account_name, "TESTER");
        match ClientOpcodeMessage::read_encrypted(&mut s, &mut conn.decrypt).unwrap() {
            ClientOpcodeMessage::CMSG_CHAR_ENUM => {}
            other => panic!("expected encrypted CMSG_CHAR_ENUM, got {other}"),
        }
    });

    let (mut c_enc, mut c_dec) = drive_auth(&mut client, "TESTER", K);

    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(matches!(*r, SMSG_AUTH_RESPONSE::AuthOk { .. }));
        }
        other => panic!("expected encrypted SMSG_AUTH_RESPONSE, got {other}"),
    }

    CMSG_CHAR_ENUM {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    drop(client);
    server.join().unwrap();
}

#[test]
fn an_auth_session_with_an_absurd_addon_size_still_completes_the_handshake() {
    let store = std::sync::Arc::new(tester_store(42));
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.clone()));

    let server_seed = match ServerOpcodeMessage::read_unencrypted(&mut client).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_CHALLENGE(c) => c.server_seed,
        other => panic!("expected SMSG_AUTH_CHALLENGE, got {other}"),
    };
    let client_seed = ProofSeed::new();
    let client_seed_value = client_seed.seed();
    let (client_proof, crypto) =
        client_seed.into_client_header_crypto(&ns("TESTER"), K, server_seed);
    let (_c_enc, mut c_dec) = crypto.split();

    let mut body = Vec::new();
    body.extend_from_slice(&5875u32.to_le_bytes());
    body.extend_from_slice(&1u32.to_le_bytes());
    body.extend_from_slice(b"TESTER\0");
    body.extend_from_slice(&client_seed_value.to_le_bytes());
    body.extend_from_slice(&client_proof);
    body.extend_from_slice(&u32::MAX.to_le_bytes());
    body.extend_from_slice(&[0xFF; 32]);
    let mut frame = Vec::new();
    frame.extend_from_slice(&((body.len() + 4) as u16).to_be_bytes());
    frame.extend_from_slice(&CMSG_AUTH_SESSION_OPCODE.to_le_bytes());
    frame.extend_from_slice(&body);
    client.write_all(&frame).unwrap();

    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(matches!(*r, SMSG_AUTH_RESPONSE::AuthOk { .. }));
        }
        other => panic!("expected encrypted AUTH_OK, got {other}"),
    }
    drop(client);
    server.join().unwrap().unwrap();
}

#[test]
fn a_silent_world_connection_is_closed_at_the_pre_auth_deadline() {
    let store = tester_store(42);
    let (mut client, server_end) = world_session_socket_pair();
    let deadline = Instant::now() + Duration::from_millis(200);
    let server = std::thread::spawn(move || {
        run_world_session_with_deadline(
            server_end,
            std::sync::Arc::new(store),
            Arc::new(LoginQueue::unlimited()),
            deadline,
        )
    });

    ServerOpcodeMessage::read_unencrypted(&mut client).unwrap();
    let err = server
        .join()
        .unwrap()
        .expect_err("a silent peer must be cut at the deadline");
    assert!(
        err.to_string().contains("total pre-auth deadline"),
        "{err:#}"
    );
    let mut byte = [0u8];
    assert_eq!(
        client.read(&mut byte).unwrap(),
        0,
        "the socket must be closed"
    );
}

#[test]
fn post_auth_world_traffic_has_no_pre_auth_deadline() {
    let store = std::sync::Arc::new(tester_store(42));
    let (mut client, server_end) = world_session_socket_pair();
    let deadline = Instant::now() + Duration::from_millis(200);
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session_with_deadline(
            server_end,
            server_store.clone(),
            Arc::new(LoginQueue::unlimited()),
            deadline,
        )
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    std::thread::sleep(Duration::from_millis(500));
    CMSG_CHAR_ENUM {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_ENUM(_) => {}
        other => panic!("expected SMSG_CHAR_ENUM, got {other}"),
    }
    drop(client);
    server.join().unwrap().unwrap();
}

#[test]
fn queued_handshake_sends_wait_queue_then_admits_once_a_seat_frees() {
    let store = std::sync::Arc::new(tester_store(42));
    let queue = std::sync::Arc::new(LoginQueue::new(1, 0));
    assert!(matches!(queue.request(), Admission::Admitted(_)));

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server_queue = queue.clone();
    let server = std::thread::spawn(move || {
        run_world_session_with_queue(server_end, server_store, server_queue)
    });

    let (_c_enc, mut c_dec) = drive_auth(&mut client, "TESTER", K);

    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(
                matches!(*r, SMSG_AUTH_RESPONSE::AuthWaitQueue { queue_position: 1 }),
                "expected AuthWaitQueue at position 1, got {r:?}"
            );
        }
        other => panic!("expected encrypted SMSG_AUTH_RESPONSE, got {other}"),
    }

    queue.depart();

    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(
                matches!(*r, SMSG_AUTH_RESPONSE::AuthOk { .. }),
                "expected AuthOk, got {r:?}"
            );
        }
        other => panic!("expected encrypted SMSG_AUTH_RESPONSE, got {other}"),
    }

    drop(client);
    server.join().unwrap().unwrap();
}

#[test]
fn disconnecting_while_queued_leaves_the_line_without_taking_a_seat() {
    let store = std::sync::Arc::new(tester_store(7));
    let queue = std::sync::Arc::new(LoginQueue::new(1, 0));
    assert!(matches!(queue.request(), Admission::Admitted(_)));

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server_queue = queue.clone();
    let server = std::thread::spawn(move || {
        run_world_session_with_queue(server_end, server_store, server_queue)
    });

    let (_c_enc, mut c_dec) = drive_auth(&mut client, "TESTER", K);

    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(matches!(*r, SMSG_AUTH_RESPONSE::AuthWaitQueue { .. }));
        }
        other => panic!("expected AuthWaitQueue, got {other}"),
    }

    drop(client);
    let _ = server.join().unwrap();

    assert_eq!(
        queue.depth(),
        0,
        "the cancelled ticket must not linger in the queue"
    );
    assert_eq!(
        queue.active(),
        1,
        "cancelling a waiter must never grant or free a seat"
    );
}

#[test]
fn a_restarted_gateway_completes_the_handshake_from_realm_state_alone() {
    let realm_session = || WorldSession {
        account_id: 42,
        session_key: K,
    };
    let handshake_once = |store: WorldFake| {
        let (mut client, server_end) = world_session_socket_pair();
        let server = std::thread::spawn(move || {
            let mut s = server_end;
            let established = world_handshake(&mut s, std::sync::Arc::new(store))
                .unwrap()
                .expect("handshake should succeed");
            established.0.account_id
        });
        client_handshake(&mut client, "TESTER", K);
        drop(client);
        server.join().unwrap()
    };

    let store_before = WorldFake {
        session: SessionState {
            username: "TESTER".into(),
            session: Some(realm_session()),
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(handshake_once(store_before), 42);

    let store_after = WorldFake {
        session: SessionState {
            username: "TESTER".into(),
            session: Some(realm_session()),
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        handshake_once(store_after),
        42,
        "a fresh gateway must re-establish the session from realm-held state alone"
    );
}

#[test]
fn a_gateway_that_cannot_reach_the_session_store_rejects_rather_than_guessing() {
    let store = WorldFake {
        session: SessionState {
            username: "TESTER".into(),
            session: None,
            ..Default::default()
        },
        ..Default::default()
    };
    let (mut client, server_end) = world_session_socket_pair();
    let server = std::thread::spawn(move || {
        let mut s = server_end;
        assert!(
            world_handshake(&mut s, std::sync::Arc::new(store))
                .unwrap()
                .is_none(),
            "no session material ⇒ no session, never a best-effort one"
        );
    });

    let (_enc, _dec) = drive_auth(&mut client, "TESTER", K);
    match ServerOpcodeMessage::read_unencrypted(&mut client).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(matches!(*r, SMSG_AUTH_RESPONSE::AuthUnknownAccount));
        }
        other => panic!("expected a plaintext rejection, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn bad_proof_is_rejected() {
    let store = WorldFake {
        session: SessionState {
            username: "TESTER".into(),
            session: Some(WorldSession {
                account_id: 1,
                session_key: K,
            }),
            ..Default::default()
        },
        ..Default::default()
    };

    let (mut client, server_end) = world_session_socket_pair();
    let server = std::thread::spawn(move || {
        let mut s = server_end;
        let conn = world_handshake(&mut s, std::sync::Arc::new(store)).unwrap();
        assert!(conn.is_none(), "bad proof must not establish a connection");
    });

    let server_seed = match ServerOpcodeMessage::read_unencrypted(&mut client).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_CHALLENGE(c) => c.server_seed,
        other => panic!("expected SMSG_AUTH_CHALLENGE, got {other}"),
    };

    let wrong_key = [0x11u8; 40];
    let client_seed = ProofSeed::new();
    let client_seed_value = client_seed.seed();
    let (client_proof, _crypto) =
        client_seed.into_client_header_crypto(&ns("TESTER"), wrong_key, server_seed);

    auth_session("TESTER", client_seed_value, client_proof)
        .write_unencrypted_client(&mut client)
        .unwrap();

    match ServerOpcodeMessage::read_unencrypted(&mut client).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(matches!(*r, SMSG_AUTH_RESPONSE::AuthFailed));
        }
        other => panic!("expected plaintext SMSG_AUTH_RESPONSE failure, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn unknown_account_is_rejected_cleanly() {
    let store = WorldFake {
        session: SessionState {
            username: "TESTER".into(),
            session: None,
            ..Default::default()
        },
        ..Default::default()
    };

    let (mut client, server_end) = world_session_socket_pair();
    let server = std::thread::spawn(move || {
        let mut s = server_end;
        assert!(world_handshake(&mut s, std::sync::Arc::new(store))
            .unwrap()
            .is_none());
    });

    let (_enc, _dec) = drive_auth(&mut client, "NOBODY", K);

    match ServerOpcodeMessage::read_unencrypted(&mut client).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(matches!(*r, SMSG_AUTH_RESPONSE::AuthUnknownAccount));
        }
        other => panic!("expected SMSG_AUTH_RESPONSE unknown-account, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn login_replays_a_pending_package_system_message() {
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                pending_system_messages: vec!["Example Package is active.".to_string()],
                ..base.session
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
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drain_world_entry(&mut client, &mut c_dec);
    let replay = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    let ServerOpcodeMessage::SMSG_MESSAGECHAT(actual) = replay else {
        panic!("expected the parked System Message after the entry sequence, got {replay:?}");
    };
    assert_eq!(
        *actual,
        codec::build_gm_system_message("Example Package is active.".to_string())
    );

    drop(client);
    server.join().unwrap();
}

#[test]
fn player_login_emits_sequence_then_self_create() {
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            weather: WeatherState {
                zone_weather: vec![(
                    12,
                    codec::ZoneWeatherView {
                        weather_type: 1,
                        intensity: 0.8,
                    },
                )],
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
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    let mut tags = Vec::new();
    let mut create_guid = None;
    let mut weather = None;
    for message in drain_world_entry(&mut client, &mut c_dec) {
        match message {
            ServerOpcodeMessage::SMSG_LOGIN_VERIFY_WORLD(m) => {
                tags.push("verify_world");
                assert_eq!(m.map, Map::EasternKingdoms);
                assert!((m.position.x - (-8949.95)).abs() < 0.01);
            }
            ServerOpcodeMessage::SMSG_ACCOUNT_DATA_TIMES(_) => tags.push("account_data_times"),
            ServerOpcodeMessage::SMSG_LOGIN_SETTIMESPEED(_) => tags.push("settimespeed"),
            ServerOpcodeMessage::SMSG_TUTORIAL_FLAGS(_) => tags.push("tutorial_flags"),
            ServerOpcodeMessage::SMSG_INITIAL_SPELLS(_) => tags.push("initial_spells"),
            ServerOpcodeMessage::SMSG_SET_PROFICIENCY(m) => tags.push(match m.class.as_int() {
                2 => "weapon_proficiency",
                4 => "armor_proficiency",
                other => panic!("unexpected proficiency item class {other}"),
            }),
            ServerOpcodeMessage::SMSG_ACTION_BUTTONS(_) => tags.push("action_buttons"),
            ServerOpcodeMessage::SMSG_INITIALIZE_FACTIONS(_) => tags.push("init_factions"),
            ServerOpcodeMessage::SMSG_SET_REST_START(_) => tags.push("set_rest_start"),
            ServerOpcodeMessage::SMSG_BINDPOINTUPDATE(_) => tags.push("bindpoint"),
            ServerOpcodeMessage::SMSG_UPDATE_OBJECT(m) => {
                tags.push("update_object");
                if let Object::CreateObject2 { guid3, .. } = &m.objects[0] {
                    create_guid = Some(guid3.guid());
                } else {
                    panic!("expected CreateObject2 in self-spawn");
                }
            }
            ServerOpcodeMessage::SMSG_WEATHER(m) => {
                tags.push("weather");
                weather = Some(*m);
            }
            ServerOpcodeMessage::SMSG_INIT_WORLD_STATES(m) => {
                tags.push("world_states");
                assert_eq!(
                    (m.map, m.area),
                    (
                        Map::EasternKingdoms,
                        wow_world_messages::vanilla::Area::ElwynnForest
                    )
                );
                assert!(m.states.is_empty());
            }
            other => panic!("unexpected message in login sequence: {other}"),
        }
    }

    assert_eq!(
        tags,
        vec![
            "verify_world",
            "account_data_times",
            "settimespeed",
            "tutorial_flags",
            "initial_spells",
            "weapon_proficiency",
            "armor_proficiency",
            "action_buttons",
            "init_factions",
            "set_rest_start",
            "bindpoint",
            "update_object",
            "weather",
            "world_states",
        ],
    );
    assert_eq!(create_guid, Some(1));
    let weather = weather.expect("world entry sends the zone's weather");
    assert_eq!(weather.weather_type, WeatherType::Rain);
    assert_eq!(weather.grade, 0.8);
    assert_eq!(weather.sound_id, 8535);
    assert_eq!(weather.change, WeatherChangeType::Instant);

    drop(client);
    server.join().unwrap();
}

#[test]
fn worldport_ack_reenters_with_fresh_subscription_and_empty_loot_state() {
    let mut ported = warrior_entity();
    ported.map_id = 1;
    ported.x = 100.0;
    ported.y = 200.0;
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                entity_in_world: false,
                login_entity: Some(warrior_entity()),
                worldport_entity: Some(ported),
                ..base.session
            },
            loot_window: LootWindowState {
                corpse_money: 25,
                ..base.loot_window
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
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drain_world_entry(&mut client, &mut c_dec);

    let _ = open_loot_window(&mut client, &mut c_enc, &mut c_dec, 60);

    MSG_MOVE_WORLDPORT_ACK {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    let mut create_guid = None;
    for message in drain_world_entry(&mut client, &mut c_dec) {
        match message {
            ServerOpcodeMessage::SMSG_LOGIN_VERIFY_WORLD(_) => {
                panic!(
                    "the re-entry sequence must NOT resend SMSG_LOGIN_VERIFY_WORLD — it makes the \
                     client reload the map it just loaded (a second loading screen)"
                );
            }
            ServerOpcodeMessage::SMSG_UPDATE_OBJECT(m) => {
                if let Object::CreateObject2 { guid3, .. } = &m.objects[0] {
                    create_guid = Some(guid3.guid());
                }
            }
            _ => {}
        }
    }
    assert_eq!(
        create_guid,
        Some(1),
        "the self entity is re-created (despawn+rebuild), not a stale row"
    );

    let calls = store.session.subscribed.lock().unwrap().clone();
    assert_eq!(
        calls.len(),
        2,
        "subscribe_player_events must run again on WORLDPORT_ACK"
    );
    assert_eq!(calls[0].1, 0, "the first subscription is for the login map");
    assert_eq!(
        calls[1].1, 1,
        "the second subscription is for the NEW (post-teleport) map"
    );
    assert!(
        (calls[1].2 - 100.0).abs() < 0.01,
        "the re-entry must use the NEW position (verify-world no longer carries it — the \
         subscription placement is the observable)"
    );

    CMSG_LOOT_MONEY {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    drop(client);
    server.join().unwrap();
    assert!(
        store.loot_window.money_looted.lock().unwrap().is_empty(),
        "world-port re-entry must start with no open loot target"
    );
    assert_eq!(
        store.session.login_entries.lock().unwrap().as_slice(),
        [codec::WorldEntry::FreshLogin, codec::WorldEntry::WorldPort]
    );
}

#[test]
fn worldport_removes_the_source_viewer_before_routing_and_registers_a_replacement() {
    let view = std::sync::Arc::new(crate::stdb::world_view::WorldView::new(true));
    let mut ported = warrior_entity();
    ported.map_id = 1;
    ported.x = 100.0;
    ported.y = 200.0;
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                entity_in_world: false,
                login_entity: Some(warrior_entity()),
                worldport_entity: Some(ported),
                relay_view: Some(view.clone()),
                ..base.session
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
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drain_world_entry(&mut client, &mut c_dec);
    let source_session = view
        .viewer_of_owner(crate::stdb::world_view::OwnerGuid(1))
        .expect("login registers the source viewer")
        .session;

    MSG_MOVE_WORLDPORT_ACK {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drain_world_entry(&mut client, &mut c_dec);

    assert_eq!(
        store
            .topology
            .viewer_present_at_settle
            .lock()
            .unwrap()
            .as_slice(),
        &[false, false],
        "neither initial routing nor world-port routing may see a registered source viewer; the \
         second observation is the transfer-cascade safety boundary"
    );
    let destination_session = view
        .viewer_of_owner(crate::stdb::world_view::OwnerGuid(1))
        .expect("destination world entry registers its viewer")
        .session;
    assert_ne!(
        destination_session, source_session,
        "world-port must replace, not reuse, the source viewer lifetime"
    );

    drop(client);
    server.join().unwrap();
}

#[test]
fn login_initialize_factions_carries_persisted_standing_at_its_reputation_index() {
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            character: CharacterState {
                reputations: vec![(19, 3175, false)],
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
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    let mut factions = None;
    for message in drain_world_entry(&mut client, &mut c_dec) {
        if let ServerOpcodeMessage::SMSG_INITIALIZE_FACTIONS(m) = message {
            factions = Some(m.factions);
        }
    }
    let factions = factions.expect("SMSG_INITIALIZE_FACTIONS not found in login sequence");
    assert_eq!(factions.len(), 64);
    assert_eq!(
        factions[19].standing, 3175,
        "slot 19 (the stored reputation_index) must carry the standing"
    );
    for (i, f) in factions.iter().enumerate() {
        if i != 19 {
            assert_eq!(f.standing, 0, "slot {i} should remain the Neutral/0 stub");
        }
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn login_with_resident_items_and_reputation_emits_no_gain_feedback() {
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            cast: CastState {
                player_items_fixture: vec![codec::ItemInstanceView {
                    guid: 0x4000_0000_0000_0001,
                    entry: 25,
                    owner_guid: 1,
                    slot: 23,
                    stack_count: 1,
                    durability: 20,
                    max_durability: 20,
                    container_slots: 0,
                    random_property_id: 0,
                    random_property_enchant_ids: [0; 3],
                    item_text_id: 0,
                    enchantment: 0,
                }],
                ..base.cast
            },
            character: CharacterState {
                reputations: vec![(19, 3175, false)],
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
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    for message in drain_world_entry(&mut client, &mut c_dec) {
        assert!(
            !matches!(
                message,
                ServerOpcodeMessage::SMSG_ITEM_PUSH_RESULT(_)
                    | ServerOpcodeMessage::SMSG_SET_FACTION_STANDING(_)
            ),
            "resident login state must not look like a newly gained item or reputation change"
        );
    }
    client
        .set_read_timeout(Some(std::time::Duration::from_millis(50)))
        .unwrap();
    assert!(
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).is_err(),
        "login queued an unexpected post-snapshot feedback frame"
    );

    drop(client);
    server.join().unwrap();
}

#[test]
fn login_fills_a_resident_suffix_items_enchantment_slots_after_the_entry_batch() {
    let item_guid = 0x4000_0000_0000_0001;
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            cast: CastState {
                player_items_fixture: vec![codec::ItemInstanceView {
                    guid: item_guid,
                    entry: 25,
                    owner_guid: 1,
                    slot: 23,
                    stack_count: 1,
                    random_property_id: 22,
                    random_property_enchant_ids: [73, 0, 0],
                    ..Default::default()
                }],
                ..base.cast
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
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    drain_world_entry(&mut client, &mut c_dec);
    let (opcode, body) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(opcode, 0x00A9);
    let updates = lyracore_shared::values_mask::parse_values_updates(&body);
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].guid, item_guid);
    assert_eq!(updates[0].fields, vec![(31, 73)]);

    drop(client);
    server.join().unwrap();
}

#[test]
fn inbound_movement_is_recorded_under_its_opcode() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_HEARTBEAT_Client, MovementInfo, MovementInfo_MovementFlags, Vector3d,
    };

    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                entity_in_world: true,
                login_entity: Some(warrior_entity()),
                ..base.session
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
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drain_world_entry(&mut client, &mut c_dec);
    let info = MovementInfo {
        flags: MovementInfo_MovementFlags::empty(),
        timestamp: 12345,
        position: Vector3d {
            x: -8950.0,
            y: -130.0,
            z: 83.0,
        },
        orientation: 1.5,
        fall_time: 0.0,
    };
    MSG_MOVE_HEARTBEAT_Client { info }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    drop(client);
    server.join().unwrap();

    let moves = store.session.moves.lock().unwrap();
    assert_eq!(moves.len(), 1);
    let (opcode, x, _, _, o, t) = moves[0];
    assert_eq!(
        opcode,
        lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT
    );
    assert!((x - (-8950.0)).abs() < 0.01);
    assert!((o - 1.5).abs() < 0.001);
    assert_eq!(t, 12345);
}

#[test]
fn a_movement_packet_for_a_despawned_entity_never_kills_the_session() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_HEARTBEAT_Client, MSG_MOVE_START_FORWARD_Client, MovementInfo,
        MovementInfo_MovementFlags, Vector3d,
    };
    let calls: ShardCallLog = Default::default();
    let entity_presence = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            topology: TopologyState {
                calls: calls.clone(),
                ..base.topology
            },
            session: SessionState {
                entity_presence: Some(entity_presence.clone()),
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            ..base
        }
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.clone()));

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drain_world_entry(&mut client, &mut c_dec);
    let beat = |t: u32| MSG_MOVE_HEARTBEAT_Client {
        info: MovementInfo {
            flags: MovementInfo_MovementFlags::empty(),
            timestamp: t,
            position: Vector3d {
                x: -8950.0,
                y: -130.0,
                z: 83.0,
            },
            orientation: 1.5,
            fall_time: 0.0,
        },
    };
    beat(1)
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    for _ in 0..100 {
        if store
            .session
            .entity_presence_checks
            .load(std::sync::atomic::Ordering::SeqCst)
            != 0
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_ne!(
        store
            .session
            .entity_presence_checks
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the encrypted movement packet must reach the coordinator presence gate"
    );
    entity_presence.store(true, std::sync::atomic::Ordering::SeqCst);
    MSG_MOVE_START_FORWARD_Client { info: beat(2).info }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client);

    server.join().unwrap().expect(
        "a movement packet for a despawned entity must be DROPPED, not session-fatal — \
                 closing the socket here strands the client on a loading screen with no error",
    );
    assert_eq!(
        calls
.lock()
.unwrap()
.iter()
.filter(|(_, c)| c == "movement_update")
.count(),
        1,
        "the transfer-tail packet must not enter the batch, while movement resumes when presence returns"
    );
}

#[test]
fn a_reappearing_entity_resets_the_movement_desync_tolerance() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_START_FORWARD_Client, MSG_MOVE_STOP_Client, MovementInfo,
        MovementInfo_MovementFlags, Vector3d,
    };
    let present = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                entity_presence: Some(present.clone()),
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            ..base
        }
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.clone()));
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drain_world_entry(&mut client, &mut c_dec);
    let info = |t| MovementInfo {
        flags: MovementInfo_MovementFlags::empty(),
        timestamp: t,
        position: Vector3d {
            x: -8950.0,
            y: -130.0,
            z: 83.0,
        },
        orientation: 1.5,
        fall_time: 0.0,
    };
    let send_tail = |start: u32, end: u32, client: &mut UnixStream, enc: &mut EncrypterHalf| {
        for t in start..end {
            let sent = if t % 2 == 0 {
                MSG_MOVE_START_FORWARD_Client { info: info(t) }
                    .write_encrypted_client(&mut *client, enc)
            } else {
                MSG_MOVE_STOP_Client { info: info(t) }.write_encrypted_client(&mut *client, enc)
            };
            sent.unwrap();
        }
    };
    let wait_for_checks = |n| {
        for _ in 0..100 {
            if store
                .session
                .entity_presence_checks
                .load(std::sync::atomic::Ordering::SeqCst)
                >= n
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("the gateway did not check entity presence {n} times");
    };

    send_tail(0, MOVE_DESYNC_TOLERANCE, &mut client, &mut c_enc);
    wait_for_checks(MOVE_DESYNC_TOLERANCE as usize);
    present.store(true, std::sync::atomic::Ordering::SeqCst);
    MSG_MOVE_START_FORWARD_Client {
        info: info(MOVE_DESYNC_TOLERANCE),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    wait_for_checks(MOVE_DESYNC_TOLERANCE as usize + 1);
    present.store(false, std::sync::atomic::Ordering::SeqCst);
    send_tail(
        MOVE_DESYNC_TOLERANCE + 1,
        MOVE_DESYNC_TOLERANCE * 2 + 1,
        &mut client,
        &mut c_enc,
    );
    wait_for_checks(MOVE_DESYNC_TOLERANCE as usize * 2 + 1);
    present.store(true, std::sync::atomic::Ordering::SeqCst);
    MSG_MOVE_STOP_Client {
        info: info(MOVE_DESYNC_TOLERANCE * 2 + 1),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server
        .join()
        .unwrap()
        .expect("a restored entity must reset the tolerance before a later transfer tail arrives");
    assert_eq!(
        store.session.moves.lock().unwrap().len(),
        2,
        "only movements sent while the entity was present may reach the shared batch"
    );
}

#[test]
fn a_movement_failure_that_is_not_a_desync_is_still_session_fatal() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_HEARTBEAT_Client, MovementInfo, MovementInfo_MovementFlags, Vector3d,
    };
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                entity_in_world: true,
                movement_transport_lost: true,
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            ..base
        }
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.clone()));
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drain_world_entry(&mut client, &mut c_dec);
    MSG_MOVE_HEARTBEAT_Client {
        info: MovementInfo {
            flags: MovementInfo_MovementFlags::empty(),
            timestamp: 1,
            position: Vector3d {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            orientation: 0.0,
            fall_time: 0.0,
        },
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    let err = server
        .join()
        .unwrap()
        .expect_err("a non-desync movement failure must still end the session");
    assert_eq!(
        crate::stdb::classify(&err),
        crate::stdb::DurableFailure::TransportLoss,
        "{err:#}"
    );
}

#[test]
fn a_movement_desync_that_never_heals_still_ends_the_session() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_START_FORWARD_Client, MSG_MOVE_STOP_Client, MovementInfo,
        MovementInfo_MovementFlags, Vector3d,
    };
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                entity_in_world: false,
                ..base.session
            },
            ..base
        }
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.clone()));
    let (mut c_enc, _c_dec) = client_handshake(&mut client, "TESTER", K);
    let info = |t: u32| MovementInfo {
        flags: MovementInfo_MovementFlags::empty(),
        timestamp: t,
        position: Vector3d {
            x: -8950.0,
            y: -130.0,
            z: 83.0,
        },
        orientation: 1.5,
        fall_time: 0.0,
    };
    for i in 0..200u32 {
        let ok = if i % 2 == 0 {
            MSG_MOVE_START_FORWARD_Client { info: info(i) }
                .write_encrypted_client(&mut client, &mut c_enc)
                .is_ok()
        } else {
            MSG_MOVE_STOP_Client { info: info(i) }
                .write_encrypted_client(&mut client, &mut c_enc)
                .is_ok()
        };
        if !ok {
            break;
        }
    }
    drop(client);
    let err = server.join().unwrap().expect_err(
        "a movement desync that never heals must STILL end the session: tolerating the tail of a \
         cross-map port is one thing, serving a permanently desynced client a frozen world forever \
         is the hang a stuck cross-map transfer causes, with no loading screen to blame it on",
    );
    assert!(format!("{err:#}").contains("not in world"), "{err:#}");
}

#[test]
fn a_world_port_whose_transfer_cannot_be_driven_aborts_the_clients_loading_screen() {
    let view = std::sync::Arc::new(crate::stdb::world_view::WorldView::new(true));
    let xdb = FakeShardDb::with_character(
        1,
        FakeChar {
            map_id: 36,
            instance_id: 7,
            payload: "gear+spells".into(),
        },
    );
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                entity_in_world: false,
                login_entity: Some(warrior_entity()),
                relay_view: Some(view.clone()),
                ..base.session
            },
            characters: vec![],
            transfer: TransferState {
                xdb: Some(xdb),
                ..base.transfer
            },
            topology: TopologyState {
                settle_error: Some("instances shard unreachable".into()),
                settle_ok_calls: 1,
                ..base.topology
            },
            ..base
        }
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.clone()));
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drain_world_entry(&mut client, &mut c_dec);
    MSG_MOVE_WORLDPORT_ACK {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    assert_eq!(
        read_raw_frame(&mut client, &mut c_dec),
        (0x0040, vec![0x02]),
        "SMSG_TRANSFER_ABORTED must contain only the NotFound reason"
    );
    drop(client);
    let err = server
        .join()
        .unwrap()
        .expect_err("a failed Transfer must end the World Session");
    assert!(
        format!("{err:#}").contains("instances shard unreachable"),
        "{err:#}"
    );
    assert!(
        view.viewer_of_owner(crate::stdb::world_view::OwnerGuid(1))
            .is_none(),
        "a failed Transfer ends the World Session and must not restore its source viewer"
    );
    assert!(
        store
            .session
            .logout_called
            .load(std::sync::atomic::Ordering::SeqCst),
        "early viewer removal keeps InWorld intact so the existing abort teardown still logs out"
    );
}

#[test]
fn a_world_port_whose_world_entry_fails_also_aborts_the_clients_loading_screen() {
    let xdb = FakeShardDb::with_character(
        1,
        FakeChar {
            map_id: 36,
            instance_id: 7,
            payload: "gear+spells".into(),
        },
    );
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                entity_in_world: false,
                login_entity: Some(warrior_entity()),
                worldport_login_error: Some("character 1 is stranded on map 36".into()),
                ..base.session
            },
            characters: vec![],
            transfer: TransferState {
                xdb: Some(xdb),
                ..base.transfer
            },
            ..base
        }
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.clone()));
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drain_world_entry(&mut client, &mut c_dec);
    MSG_MOVE_WORLDPORT_ACK {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    // World entry fails before it queues any arrival messages.
    assert_eq!(
        read_raw_frame(&mut client, &mut c_dec),
        (0x0040, vec![0x02]),
        "SMSG_TRANSFER_ABORTED must contain only the NotFound reason"
    );
    drop(client);
    let err = server
        .join()
        .unwrap()
        .expect_err("a failed world-port must end the World Session");
    assert!(format!("{err:#}").contains("stranded"), "{err:#}");
}

#[test]
fn movement_state_changes_forward_immediately_and_in_order_never_delayed() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_HEARTBEAT_Client, MSG_MOVE_START_FORWARD_Client, MSG_MOVE_START_TURN_LEFT_Client,
        MSG_MOVE_STOP_Client, MovementInfo, MovementInfo_MovementFlags, Vector3d,
    };

    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);

    let mk = |flags: u32, orientation: f32, x: f32, timestamp: u32| MovementInfo {
        flags: MovementInfo_MovementFlags::new(flags, None, None, None, None),
        timestamp,
        position: Vector3d { x, y: 0.0, z: 0.0 },
        orientation,
        fall_time: 0.0,
    };
    const RUN: u32 = 0x1;
    const TURN: u32 = 0x1 | 0x10;
    const STOPPED: u32 = 0x0;

    MSG_MOVE_START_FORWARD_Client {
        info: mk(RUN, 0.0, 0.0, 100),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    MSG_MOVE_HEARTBEAT_Client {
        info: mk(RUN, 0.0, 5.0, 200),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    MSG_MOVE_HEARTBEAT_Client {
        info: mk(RUN, 0.0, 10.0, 300),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    MSG_MOVE_START_TURN_LEFT_Client {
        info: mk(TURN, 5.0, 10.0, 400),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    MSG_MOVE_STOP_Client {
        info: mk(STOPPED, 5.0, 10.0, 500),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    drop(client);
    server.join().unwrap();

    let moves = store.session.moves.lock().unwrap();
    let opcodes: Vec<u32> = moves.iter().map(|(op, ..)| *op).collect();
    assert_eq!(
        opcodes,
        vec![
            lyracore_shared::opcodes::movement::MSG_MOVE_START_FORWARD,
            lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT,
            lyracore_shared::opcodes::movement::MSG_MOVE_START_TURN_LEFT,
            lyracore_shared::opcodes::movement::MSG_MOVE_STOP,
        ],
        "run-start, [flushed heartbeat], turn, stop — in that exact order; the turn and stop must \
         never be held, and the superseded x=5.0 intermediate must never appear at all"
    );
    assert!(
        (moves[1].1 - 10.0).abs() < 0.01,
        "flushed heartbeat must carry the superseding x=10.0, not the dropped x=5.0"
    );
    assert_eq!(
        moves.len(),
        4,
        "exactly one heartbeat survives coalescing out of the two sent"
    );
}

#[test]
fn non_movement_opcode_flushes_pending_heartbeat_before_being_handled() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_HEARTBEAT_Client, MSG_MOVE_START_FORWARD_Client, MovementInfo,
        MovementInfo_MovementFlags, Vector3d,
    };

    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    let mk = |x: f32, timestamp: u32| MovementInfo {
        flags: MovementInfo_MovementFlags::new(0x1, None, None, None, None),
        timestamp,
        position: Vector3d { x, y: 0.0, z: 0.0 },
        orientation: 0.0,
        fall_time: 0.0,
    };

    MSG_MOVE_START_FORWARD_Client { info: mk(0.0, 100) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    MSG_MOVE_HEARTBEAT_Client {
        info: mk(10.0, 200),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    MSG_MOVE_HEARTBEAT_Client {
        info: mk(20.0, 300),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    MSG_MOVE_HEARTBEAT_Client {
        info: mk(30.0, 400),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {}
        other => panic!("expected SMSG_QUESTGIVER_STATUS, got {other}"),
    }
    {
        let moves = store.session.moves.lock().unwrap();
        assert_eq!(
            moves.len(),
            2,
            "baseline + exactly ONE flushed heartbeat — if coalescing weren't happening, all 3 \
             heartbeats would have forwarded individually (4 total), not 2"
        );
        assert!(
            (moves[1].1 - 30.0).abs() < 0.01,
            "the flushed heartbeat must carry the LATEST superseding position, not an earlier dropped one"
        );
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn desync_error_classifies_entity_missing_as_fatal_but_not_transient() {
    assert!(is_desync_error(&anyhow!("attacker not in world")));
    assert!(is_desync_error(&anyhow!("caster not in world")));
    assert!(is_desync_error(&anyhow!("no live entity for guid 5")));
    assert!(is_desync_error(&anyhow!("Player NOT IN WORLD")));
    assert!(!is_desync_error(&anyhow!("target is dead")));
    assert!(!is_desync_error(&anyhow!(
        "target out of range (35.0 yd > 30 yd)"
    )));
    assert!(!is_desync_error(&anyhow!(
        "not enough power: have 0, need 30"
    )));
    assert!(!is_desync_error(&anyhow!(
        "spell not ready (global cooldown)"
    )));
    assert!(!is_desync_error(&anyhow!("cannot attack self")));
}

#[test]
fn the_writer_drains_the_egress_depth_back_to_zero_b2() {
    let store = std::sync::Arc::new(quest_store());
    let (client, _c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    let depth = store
        .session
        .session_depth
        .lock()
        .unwrap()
        .clone()
        .expect("the session must have handed its egress handle to subscribe_player_events");
    // The writer releases each entry just after its socket write, so the client can read first.
    assert!(
        crate::durable_test_support::poll_until(std::time::Duration::from_secs(5), || {
            depth.load(std::sync::atomic::Ordering::Relaxed) == 0
        }),
        "the writer must decrement the egress depth for every item it wrote — a depth that only \
         climbs makes every session shed peer movement forever once it passes EGRESS_SHED_DEPTH"
    );
    drop(client);
    let _ = server.join();
}

#[test]
fn logout_while_out_of_combat_succeeds_and_clears_open_loot() {
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            loot_window: LootWindowState {
                corpse_money: 25,
                ..base.loot_window
            },
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    let _ = open_loot_window(&mut client, &mut c_enc, &mut c_dec, 60);

    CMSG_LOGOUT_REQUEST {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOGOUT_RESPONSE(r) => {
            use wow_world_messages::vanilla::{LogoutResult, LogoutSpeed};
            assert_eq!(
                r.result,
                LogoutResult::Success,
                "expected Success, got {:?}",
                r.result
            );
            assert_eq!(r.speed, LogoutSpeed::Instant);
        }
        other => panic!("expected SMSG_LOGOUT_RESPONSE, got {other}"),
    }
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOGOUT_COMPLETE => {}
        other => panic!("expected SMSG_LOGOUT_COMPLETE, got {other}"),
    }

    CMSG_LOOT_MONEY {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    drop(client);
    server.join().unwrap();

    assert!(
        store
            .session
            .logout_called
            .load(std::sync::atomic::Ordering::SeqCst),
        "Account ownership must be released after successful logout"
    );
    assert!(
        store.loot_window.money_looted.lock().unwrap().is_empty(),
        "logout must discard the open loot target"
    );
}

#[test]
fn logout_while_in_combat_is_denied() {
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                combat_until_ms: u64::MAX,
                ..base.session
            },
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_LOGOUT_REQUEST {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOGOUT_RESPONSE(r) => {
            use wow_world_messages::vanilla::LogoutResult;
            assert_eq!(
                r.result,
                LogoutResult::FailureInCombat,
                "expected FailureInCombat on first request, got {:?}",
                r.result
            );
        }
        other => panic!("expected SMSG_LOGOUT_RESPONSE(denial) on first request, got {other}"),
    }

    CMSG_LOGOUT_REQUEST {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOGOUT_RESPONSE(r) => {
            use wow_world_messages::vanilla::LogoutResult;
            assert_eq!(
                r.result,
                LogoutResult::FailureInCombat,
                "expected FailureInCombat on second request (session still alive), got {:?}",
                r.result
            );
        }
        other => panic!("expected SMSG_LOGOUT_RESPONSE(denial) on second request, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn played_time_replies_with_the_durable_total_plus_the_live_session_span() {
    let durable_secs: u32 = 3600;
    let session_started_secs_ago: u64 = 5;
    let now_micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_micros() as u64;
    let session_start_micros = now_micros - session_started_secs_ago * 1_000_000;

    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            characters: vec![codec::CharacterView {
                guid: 1,
                name: "Tester".into(),
                played_total_secs: durable_secs,
                session_start_micros,
                ..Default::default()
            }],
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_PLAYED_TIME {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_PLAYED_TIME(p) => {
            assert!(
                p.total_played_time >= durable_secs,
                "reply {} must be at least the durable floor {durable_secs}",
                p.total_played_time
            );
            assert!(
                p.total_played_time < durable_secs + 60,
                "reply {} should only add a few seconds of live session, not run away",
                p.total_played_time
            );
            assert_eq!(
                p.total_played_time, p.level_played_time,
                "level time mirrors total (untracked per-level in this slice)"
            );
        }
        other => panic!("expected SMSG_PLAYED_TIME, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn force_run_speed_change_ack_is_swallowed_with_no_reply_and_no_session_teardown() {
    use wow_world_messages::vanilla::{
        MovementInfo, MovementInfo_MovementFlags, Vector3d, CMSG_FORCE_RUN_SPEED_CHANGE_ACK,
    };
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_FORCE_RUN_SPEED_CHANGE_ACK {
        guid: Guid::new(1),
        counter: 1,
        info: MovementInfo {
            flags: MovementInfo_MovementFlags::empty(),
            timestamp: 0,
            position: Vector3d {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            orientation: 0.0,
            fall_time: 0.0,
        },
        new_speed: 21.0,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {}
        other => panic!("expected the sentinel (ack swallowed), got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn reducer_transport_loss_ends_an_admitted_session_and_frees_one_queue_seat() {
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                logout_transport_lost: true,
                ..base.session
            },
            combat: CombatState {
                set_target_transport_lost: true,
            },
            ..base
        }
    });
    let queue = std::sync::Arc::new(LoginQueue::new(1, 0));
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server_queue = queue.clone();
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = run_world_session_with_queue(server_end, server_store.clone(), server_queue);
        result_tx.send(result).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drain_world_entry(&mut client, &mut c_dec);
    assert_eq!(
        queue.active(),
        1,
        "the admitted session holds the only seat"
    );

    CMSG_SET_SELECTION {
        target: Guid::new(321),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    let err = result_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("transport loss must end the session promptly")
        .expect_err("a disconnected reducer transport must end the world session");
    assert_eq!(
        crate::stdb::classify(&err),
        crate::stdb::DurableFailure::TransportLoss
    );
    assert!(
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).is_err(),
        "the world socket closes after the fatal reducer result"
    );
    assert!(
        store
            .session
            .logout_called
            .load(std::sync::atomic::Ordering::SeqCst),
        "teardown still attempts leave-world cleanup"
    );
    assert_eq!(queue.active(), 0, "the ended session released its seat");
    assert!(
        matches!(queue.request(), Admission::Admitted(_)),
        "one replacement session is admitted"
    );
    assert!(
        matches!(queue.request(), Admission::Queued(_)),
        "only one seat was released"
    );
    drop(client);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn competing_world_sessions_close_the_old_socket_without_removing_the_winner() {
    use crate::accept::BlockingTaskCapacity;
    use crate::config::GatewayConfig;
    use crate::durable_test_support::Standalone;
    use crate::stdb::Coordinator;

    fn start(
        coord: Coordinator,
        runtime: tokio::runtime::Handle,
    ) -> (UnixStream, std::sync::mpsc::Receiver<Result<()>>) {
        let (client, server) = world_session_socket_pair();
        let (done, result) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _entered = runtime.enter();
            let _ = done.send(run_world_session(server, std::sync::Arc::new(coord)));
        });
        (client, result)
    }

    fn login(client: &mut UnixStream) -> (EncrypterHalf, DecrypterHalf) {
        let (mut encrypt, mut decrypt) = client_handshake(client, "TEST", [7; 40]);
        CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
            .write_encrypted_client(&mut *client, &mut encrypt)
            .unwrap();
        for _ in 0..100 {
            if matches!(
                ServerOpcodeMessage::read_encrypted(&mut *client, &mut decrypt).unwrap(),
                ServerOpcodeMessage::SMSG_WEATHER(_)
            ) {
                return (encrypt, decrypt);
            }
        }
        panic!("World Session did not complete its entry batch");
    }

    for name in [
        "LYRACORE_SHARD_MAP",
        "LYRACORE_SHARD_MAP_FILE",
        "LYRACORE_REALM_CORE",
    ] {
        assert!(
            std::env::var_os(name).is_none(),
            "unset {name} for this private fixture"
        );
    }
    let mut fixture = Standalone::start("account-world-sessions");
    fixture.publish_module();
    fixture.assert_call("claim_operator", &[]);
    fixture.assert_call("install_guid_range", &["0"]);
    fixture.assert_call("gw_heartbeat", &[]);
    let cfg = GatewayConfig {
        logon_bind: "127.0.0.1:0".into(),
        world_bind: "127.0.0.1:0".into(),
        stdb_uri: fixture.server().into(),
        module_name: fixture.shard_name().into(),
        coordinator_token: Some(fixture.owner_token()),
        gateway_id: "world-owner-a".into(),
        blocking_task_capacity: BlockingTaskCapacity::new(2),
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let a = runtime.block_on(Coordinator::connect(&cfg)).unwrap();
    let b = runtime
        .block_on(Coordinator::connect(&GatewayConfig {
            gateway_id: "world-owner-b".into(),
            ..cfg
        }))
        .unwrap();
    let (release_pool, hold_pool) = std::sync::mpsc::channel();
    let (pool_started, pool_ready) = std::sync::mpsc::channel();
    runtime.spawn_blocking(move || {
        pool_started.send(()).unwrap();
        let _ = hold_pool.recv_timeout(Duration::from_secs(40));
    });
    pool_ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let account = a.account_by_username("TEST").unwrap().unwrap().id;
    a.establish_session(account, &[7; 40], a.bound_identity(account).unwrap())
        .unwrap();
    let (mut first, first_done) = start(a, runtime.handle().clone());
    let _first_cipher = login(&mut first);

    let (mut refused, refused_done) = start(b.clone(), runtime.handle().clone());
    let (mut encrypt, _) = client_handshake(&mut refused, "TEST", [7; 40]);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut refused, &mut encrypt)
        .unwrap();
    let refusal = refused_done
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap_err();
    assert!(
        refusal.to_string().contains("ACCOUNT_IN_USE"),
        "{refusal:#}"
    );
    assert!(first_done.try_recv().is_err());

    fixture.assert_sql("UPDATE game_account_claim SET expires_micros = 0");
    let (mut winner, winner_done) = start(b, runtime.handle().clone());
    let (mut winner_encrypt, mut winner_decrypt) = login(&mut winner);
    let first_deadline = fixture.query_rows("SELECT expires_micros FROM game_account_claim")[0]
        ["expires_micros"]
        .clone();
    first_done
        .recv_timeout(Duration::from_secs(25))
        .expect("lost Account ownership must close the old World Session")
        .unwrap();
    assert_eq!(
        fixture
            .query_rows("SELECT * FROM game_world_entity WHERE guid = 1")
            .len(),
        1
    );
    assert!(winner_done.try_recv().is_err());
    assert!(
        crate::durable_test_support::poll_until(Duration::from_secs(5), || fixture
            .query_rows("SELECT expires_micros FROM game_account_claim")[0]["expires_micros"]
            != first_deadline),
        "the winning Account Claim must renew while the Tokio blocking pool is full"
    );
    CMSG_LOGOUT_REQUEST {}
        .write_encrypted_client(&mut winner, &mut winner_encrypt)
        .unwrap();
    let mut logged_out = false;
    for _ in 0..100 {
        if read_raw_frame(&mut winner, &mut winner_decrypt).0
            == lyracore_shared::opcodes::world::SMSG_LOGOUT_COMPLETE
        {
            logged_out = true;
            break;
        }
    }
    assert!(
        logged_out,
        "winning World Session must still process logout"
    );
    winner.shutdown(std::net::Shutdown::Write).unwrap();
    winner_done
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    drop(winner);
    release_pool.send(()).unwrap();
    assert!(fixture
        .query_rows("SELECT * FROM game_world_entity WHERE guid = 1")
        .is_empty());
}

#[test]
fn an_addon_command_refusal_is_dropped_and_a_transport_loss_ends_the_session() {
    let conn = in_world_conn(42, 1);
    let text = "STC\tv1|example.order|same|1/1|follow|77";
    let answering = |error: fn(&str) -> crate::stdb::ReducerCallError| WorldFake {
        session: SessionState {
            client_command_error: Some(error),
            ..Default::default()
        },
        ..Default::default()
    };

    let refusing = answering(|op| crate::stdb::ReducerCallError::refused(op, "UNKNOWN_COMMAND"));
    assert!(handle_addon_message(&refusing, &conn, text)
        .unwrap()
        .is_empty());

    let lost = answering(crate::stdb::ReducerCallError::transport_lost);
    assert!(handle_addon_message(&lost, &conn, text).is_err());
}

#[test]
fn a_package_command_uses_the_world_session_actor_without_a_party_gate() {
    let store = WorldFake::default();
    let conn = in_world_conn(42, 1);
    let text = "STC\tv1|package.wave_defense.select|1|1/1|healer|999";
    assert!(handle_addon_message(&store, &conn, text)
        .unwrap()
        .is_empty());
    assert_eq!(
        *store.session.client_commands.lock().unwrap(),
        vec![(1, "package.wave_defense.select".into(), "healer|999".into())]
    );
}

#[test]
fn a_package_refusal_replies_privately_and_transport_loss_ends_the_world_session() {
    let conn = in_world_conn(42, 1);
    let text = "STC\tv1|package.wave_defense.ready|1|1/1|yes";
    let store = WorldFake {
        session: SessionState {
            client_command_error: Some(|op| {
                crate::stdb::ReducerCallError::refused(op, "NotQueued")
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    let replies = handle_addon_message(&store, &conn, text).unwrap();
    let [Outbound::Raw { opcode, body }] = replies.as_slice() else {
        panic!("expected one private addon reply");
    };
    assert_eq!(*opcode, codec::addon::SMSG_MESSAGECHAT_OPCODE);
    assert_eq!(body[0], 6);
    assert_eq!(&body[5..13], &1u64.to_le_bytes());
    assert_eq!(
        &body[17..body.len() - 2],
        b"STC\tv1|package.wave_defense.ready.result|0|1/1|Refused|NotQueued"
    );
    let lost = WorldFake {
        session: SessionState {
            client_command_error: Some(crate::stdb::ReducerCallError::transport_lost),
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(handle_addon_message(&lost, &conn, text).is_err());
}

#[test]
fn oversized_and_malformed_package_frames_do_not_reach_the_store() {
    let store = WorldFake::default();
    let conn = in_world_conn(42, 1);
    for frame in [
        "STC\tv2|package.wave_defense.status|1|1/1|".into(),
        "STC\tv1|package.wave_defense.status|1|1/2|".into(),
        format!(
            "STC\tv1|package.wave_defense.select|1|1/1|{}",
            "x".repeat(1025)
        ),
    ] {
        assert!(handle_addon_message(&store, &conn, &frame)
            .unwrap()
            .is_empty());
    }
    assert!(store.session.client_commands.lock().unwrap().is_empty());
}
