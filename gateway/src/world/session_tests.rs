//! Session-level socket tests: the handshake and its deadlines, the admission queue, world entry
//! and world-port re-entry, movement and its coalescing, logout, and the order a World Session
//! closes in.

use super::*;

#[derive(Clone)]
struct ManualClock(Rc<Cell<Instant>>);

impl DeadlineClock for ManualClock {
    fn now(&self) -> Instant {
        self.0.get()
    }
}

struct AdvancingStream {
    input: Cursor<Vec<u8>>,
    output: Vec<u8>,
    clock: ManualClock,
    per_read: Duration,
    per_write: Duration,
    write_timeout_calls: Rc<Cell<usize>>,
}

impl Read for AdvancingStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let one_byte = buf.len().min(1);
        let read = self.input.read(&mut buf[..one_byte])?;
        self.clock.0.set(self.clock.0.get() + self.per_read);
        Ok(read)
    }
}

impl Write for AdvancingStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.output.extend_from_slice(buf);
        self.clock.0.set(self.clock.0.get() + self.per_write);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl IoDeadline for AdvancingStream {
    fn set_read_timeout(&self, _timeout: Option<Duration>) -> std::io::Result<()> {
        Ok(())
    }

    fn set_write_timeout(&self, _timeout: Option<Duration>) -> std::io::Result<()> {
        self.write_timeout_calls
            .set(self.write_timeout_calls.get() + 1);
        Ok(())
    }
}

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
        // Prove the inbound cipher works: read one encrypted client message.
        match ClientOpcodeMessage::read_encrypted(&mut s, &mut conn.decrypt).unwrap() {
            ClientOpcodeMessage::CMSG_CHAR_ENUM => {}
            other => panic!("expected encrypted CMSG_CHAR_ENUM, got {other}"),
        }
    });

    // --- client: drive the shared challenge→proof→AUTH_SESSION prefix (`drive_auth`) ---
    let (mut c_enc, mut c_dec) = drive_auth(&mut client, "TESTER", K);

    // --- client: the AUTH_OK response is the first ENCRYPTED packet ---
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(matches!(*r, SMSG_AUTH_RESPONSE::AuthOk { .. }));
        }
        other => panic!("expected encrypted SMSG_AUTH_RESPONSE, got {other}"),
    }

    // --- client: send an encrypted CMSG the server decrypts ---
    CMSG_CHAR_ENUM {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    drop(client);
    server.join().unwrap();
}

/// The typed decoder would size a buffer from the addon field and unwrap the zlib. The gateway
/// never reads the addon list, so a 4 GiB claim over garbage bytes changes nothing: the proof is
/// checked and AUTH_OK goes out.
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

    // Hand-built frame: the fixed fields, then a 4 GiB decompressed-size claim over garbage.
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

/// A peer that reads the challenge and never answers must give its blocking thread back at the
/// total pre-auth deadline.
#[test]
fn a_silent_world_connection_is_closed_at_the_pre_auth_deadline() {
    let store = tester_store(42);
    let (mut client, server_end) = world_session_socket_pair();
    let mut deadline = PreAuthDeadline::after(Duration::from_millis(200));
    let server = std::thread::spawn(move || {
        run_world_session_with_queue_and_deadline(
            server_end,
            std::sync::Arc::new(store),
            &LoginQueue::unlimited(),
            &mut deadline,
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
fn slow_auth_session_bytes_cannot_extend_the_pre_auth_deadline() {
    let store = tester_store(42);
    let mut input = Vec::new();
    auth_session("TESTER", 1, [0; 20])
        .write_unencrypted_client(&mut input)
        .unwrap();
    let start = Instant::now();
    let clock = ManualClock(Rc::new(Cell::new(start)));
    let mut deadline = PreAuthDeadline::with_clock(start + Duration::from_millis(8), clock.clone());
    let mut stream = AdvancingStream {
        input: Cursor::new(input),
        output: Vec::new(),
        clock,
        per_read: Duration::from_millis(1),
        per_write: Duration::ZERO,
        write_timeout_calls: Rc::new(Cell::new(0)),
    };

    let result = world_handshake_with_queue_and_deadline(
        &mut stream,
        std::sync::Arc::new(store),
        &LoginQueue::unlimited(),
        &mut deadline,
    );
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("a slow auth frame must share one total deadline"),
    };

    assert!(
        error.to_string().contains("total pre-auth deadline"),
        "{error:#}"
    );
    assert!(
        !stream.output.is_empty(),
        "the server challenge must be written before the slow client frame times out"
    );
}

#[test]
fn world_challenge_write_uses_the_absolute_pre_auth_deadline() {
    let store = tester_store(42);
    let start = Instant::now();
    let clock = ManualClock(Rc::new(Cell::new(start)));
    let write_timeout_calls = Rc::new(Cell::new(0));
    let mut deadline =
        PreAuthDeadline::with_clock(start + Duration::from_millis(10), clock.clone());
    let mut stream = AdvancingStream {
        input: Cursor::new(Vec::new()),
        output: Vec::new(),
        clock,
        per_read: Duration::ZERO,
        per_write: Duration::from_millis(20),
        write_timeout_calls: write_timeout_calls.clone(),
    };

    let result = world_handshake_with_queue_and_deadline(
        &mut stream,
        std::sync::Arc::new(store),
        &LoginQueue::unlimited(),
        &mut deadline,
    );
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("the world challenge must not write past the total deadline"),
    };

    assert_eq!(
        write_timeout_calls.get(),
        1,
        "the world challenge must write through the pre-auth I/O wrapper"
    );
    assert!(
        error.to_string().contains("pre-auth I/O deadline"),
        "{error:#}"
    );
    assert!(
        !stream.output.is_empty(),
        "the fake advances time after accepting the world challenge bytes"
    );
}

/// Once client proof succeeds, authenticated World Session traffic has no pre-auth deadline.
#[test]
fn post_auth_world_traffic_has_no_pre_auth_deadline() {
    let store = std::sync::Arc::new(tester_store(42));
    let (mut client, server_end) = world_session_socket_pair();
    let mut deadline = PreAuthDeadline::after(Duration::from_millis(200));
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session_with_queue_and_deadline(
            server_end,
            server_store.clone(),
            &LoginQueue::unlimited(),
            &mut deadline,
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
    // Occupy the only seat directly — exactly what an already-connected world session holds.
    assert_eq!(queue.request(), Admission::Admitted);

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server_queue = queue.clone();
    let server = std::thread::spawn(move || {
        let mut s = server_end;
        // Blocks in the queue's wait loop until `queue.depart()` (below) frees the one seat.
        let (conn, _encrypt) =
            world_handshake_with_queue(&mut s, server_store.clone(), &server_queue)
                .unwrap()
                .expect("handshake should eventually succeed once admitted");
        assert_eq!(conn.account_id, 42);
    });

    // --- client: drive the shared challenge→proof→AUTH_SESSION prefix (`drive_auth`) ---
    let (_c_enc, mut c_dec) = drive_auth(&mut client, "TESTER", K);

    // --- client: the FIRST encrypted response must be AuthWaitQueue at position 1, not AuthOk —
    // proving the handshake actually queued instead of bypassing the gate ---
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(
                matches!(*r, SMSG_AUTH_RESPONSE::AuthWaitQueue { queue_position: 1 }),
                "expected AuthWaitQueue at position 1, got {r:?}"
            );
        }
        other => panic!("expected encrypted SMSG_AUTH_RESPONSE, got {other}"),
    }

    // Free the one seat — mirrors what `run_world_session_with_queue`'s teardown does on a real
    // disconnect. The queued handshake must notice on its next poll and proceed to admission.
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
    server.join().unwrap();
}

#[test]
fn disconnecting_while_queued_leaves_the_line_without_taking_a_seat() {
    let store = std::sync::Arc::new(tester_store(7));
    let queue = std::sync::Arc::new(LoginQueue::new(1, 0));
    assert_eq!(queue.request(), Admission::Admitted); // occupy the only seat

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server_queue = queue.clone();
    let server = std::thread::spawn(move || {
        let mut s = server_end;
        let result =
            world_handshake_with_queue(&mut s, server_store.clone(), &server_queue).unwrap();
        assert!(
            result.is_none(),
            "a hangup while queued must end the session cleanly, not error"
        );
    });

    let (_c_enc, mut c_dec) = drive_auth(&mut client, "TESTER", K);

    // Read (and discard) the first AuthWaitQueue so we know the server has actually queued us
    // before hanging up — a hangup racing the very first send would prove nothing about `cancel`.
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(matches!(*r, SMSG_AUTH_RESPONSE::AuthWaitQueue { .. }));
        }
        other => panic!("expected AuthWaitQueue, got {other}"),
    }

    drop(client); // hang up while still queued
    server.join().unwrap();

    // The seat is still held by the ORIGINAL occupant (never touched); the queued-then-cancelled
    // connection must have left NO trace in the line.
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
    // The stateless-gateway invariant, now realm-scoped. The session key K lives in
    // `game_session` — on realm-core when it is configured, on the world DB when it is not — and
    // the gateway keeps NOTHING about it. So killing the gateway mid-session and starting a fresh
    // one must let the same account re-handshake with no re-logon, against a brand-new process
    // that has never seen this client.
    //
    // Modelled here as two handshakes against two INDEPENDENT store instances that share only the
    // realm-held session row: `store_before` is the gateway that gets killed, `store_after` is the
    // replacement. Each handshake mints its own server seed and its own client seed, so nothing
    // from the first run can be smuggled into the second — if any handshake input were
    // gateway-local rather than realm state, the second run could not succeed.
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

    // ---- the gateway is killed here; every byte of its in-process state is gone ----

    let store_after = WorldFake {
        session: SessionState {
            username: "TESTER".into(),
            session: Some(realm_session()), // re-READ from the realm, not carried over
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
    // The other half of the stateless-gateway invariant: "resume from realm state" must not
    // degrade into "resume from anything". A store that cannot answer (realm-core unreachable →
    // `lookup_session` yields no session) rejects the handshake plaintext instead of establishing
    // a session on an unverified key. `CoordinatorStore` reaches this state by way of
    // `Coordinator::realm_core()`'s Err.
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
    // The store hands out a session key, but the client computes its proof against a
    // DIFFERENT key, so the server's digest check must fail.
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

    // The failure is sent plaintext (no cipher was established).
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
            session: None, // account exists nowhere / no session
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
    // A Package `on_login` hook emits its System Message INSIDE `player_login` — before the
    // session is in the viewer registry, so the live insert relay has nobody to address. World
    // entry must replay the parked row right after the entry sequence.
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
    // CMSG_PLAYER_LOGIN must yield the full login sequence (in order), then the self
    // CREATE_OBJECT2 at the correct position/guid, then the current zone's weather — the clock
    // (SMSG_LOGIN_SETTIMESPEED) before the sky, so the client has both before it renders.
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            weather: WeatherState {
                // Elwynn Forest (the fixture's zone) is raining hard.
                zone_weather: vec![(
                    12,
                    codec::ZoneWeatherView {
                        weather_type: 1,
                        intensity: 0.8,
                    },
                )],
                ..base.weather
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

    // The 11-message login sequence, the self CREATE_OBJECT2, then the zone's weather.
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
        ],
    );
    assert_eq!(create_guid, Some(1));
    // Elwynn's stored rain, synchronized rather than faded in: an arriving client must not spend
    // a transition rendering the sky it happened to have last.
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
    // MSG_MOVE_WORLDPORT_ACK must re-run the SAME enter_world path as
    // CMSG_PLAYER_LOGIN — rebuilding the entity (now on the NEW map the module's teleport_player
    // durably wrote to the character row) and re-subscribing with a FRESH `created` dedup set — a
    // reused dedup set from the old map would suppress the initial sweep of pre-existing entities
    // through the CREATE path, leaving the new map looking empty until something moved or spawned —
    // a stale created-set is exactly what would leave entities invisible on cross-map arrival.
    let mut ported = warrior_entity();
    ported.map_id = 1; // Kalimdor — simulates teleport_player's durable cross-map write
    ported.x = 100.0;
    ported.y = 200.0;
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                // A real cross-map teleport has despawned the old-map entity before its ack arrives.
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
    // Drain world entry (map 0 — not the point of this test).
    drain_world_entry(&mut client, &mut c_dec);

    let _ = open_loot_window(&mut client, &mut c_enc, &mut c_dec, 60);

    MSG_MOVE_WORLDPORT_ACK {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    // Re-entry skips SMSG_LOGIN_VERIFY_WORLD because resending it reloads the current map.
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

    // subscribe_player_events must have fired TWICE: once at login (map 0), once at the world-port
    // re-entry (map 1) — a fresh `created` set is built for the new map each time, never reused.
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

    // The old map's open window cannot authorize a targetless request after re-entry.
    CMSG_LOOT_MONEY {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    drop(client);
    server.join().unwrap();
    assert!(
        store.loot_window.money_looted.lock().unwrap().is_empty(),
        "world-port re-entry must start with no open loot target"
    );
    // The login ends the Away Status; the world-port rides the reducer that keeps it.
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
    // A persisted `game_player_reputation` row must land in the login
    // SMSG_INITIALIZE_FACTIONS at its STORED reputation_index slot (0..63), never faction_id — the
    // guardrail that also gates the live SET_FACTION_STANDING relay (McBride ERROR).
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            character: CharacterState {
                // Stormwind's rep-index is 19 (Faction.dbc ReputationListID), NOT its faction id (72) —
                // exercising the exact index/id distinction the guardrail protects.
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

    // Drain the whole world entry so the server side doesn't see a broken pipe.
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
    // Stormwind's faction_id (72) is itself past the 64-slot array — proof that indexing by faction_id
    // (the McBride bug) would panic/crash here rather than silently landing on the wrong slot.
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

    // World entry, which carries the resident item's CREATE. The item and standing are snapshots
    // in those frames, not live insert callbacks, so neither feedback packet is lawful.
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

    // World entry, which carries the item's CREATE, then the raw update gtker's typed reader
    // rejects.
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

    let (mut c_enc, _c_dec) = client_handshake(&mut client, "TESTER", K);
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

    drop(client); // server reads the heartbeat, then EOF
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
                ..base.session
            },
            ..base
        }
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.clone()));

    let (mut c_enc, _c_dec) = client_handshake(&mut client, "TESTER", K);
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
    // The first packet is a transfer tail. Once the coordinator cache sees the entity again, the
    // next state transition must resume normal batched submission.
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
                ..base.session
            },
            ..base
        }
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.clone()));
    let (mut c_enc, _c_dec) = client_handshake(&mut client, "TESTER", K);
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
                movement_error: Some("timed out after 10s".into()),
                ..base.session
            },
            ..base
        }
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.clone()));
    let (mut c_enc, _c_dec) = client_handshake(&mut client, "TESTER", K);
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
    assert!(format!("{err:#}").contains("timed out"), "{err:#}");
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
                // The entity is gone and is NEVER coming back — not a teleport tail, a real desync.
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
    // State transitions, so every one forwards immediately (never coalesced). Far more than a
    // cross-map port's in-flight tail, and spread over more time than any loading screen's worth of
    // queued packets — a session still serving these has stopped detecting desyncs altogether.
    // Writes are best-effort: the socket SHOULD close partway through, which is the whole point.
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
                // The world-port ack is only actionable after teleport_player removed the old entity.
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
                settle_ok_calls: 1, // the LOGIN routes fine; the world-port's settle is the one that fails
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
    // The client finished loading the dungeon map and acks — this is where the transfer runs.
    MSG_MOVE_WORLDPORT_ACK {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    let aborted = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).expect(
        "the client must receive SMSG_TRANSFER_ABORTED — silence here is the loading-screen hang this fixes",
    );
    match aborted {
        ServerOpcodeMessage::SMSG_TRANSFER_ABORTED(m) => {
            assert_eq!(
                m.map,
                Map::Deadmines,
                "the abort must name the map the client is loading"
            );
        }
        other => panic!("expected SMSG_TRANSFER_ABORTED, got {other}"),
    }
    drop(client);
    let err = server.join().unwrap().expect_err(
        "a half-driven transfer must still end the session rather than enter the world",
    );
    assert!(
        format!("{err:#}").contains("instances shard unreachable"),
        "{err:#}"
    );
    assert!(
        view.viewer_of_owner(crate::stdb::world_view::OwnerGuid(1))
            .is_none(),
        "a failed transfer terminates the session and must not restore its source viewer"
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
                // The world-port ack is only actionable after teleport_player removed the old entity.
                entity_in_world: false,
                login_entity: Some(warrior_entity()),
                // Routing succeeds; the world entry on the far side is what fails.
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

    // Drain whatever the (partial) re-entry emitted and require an abort somewhere in it — the
    // client must not be left loading. `read_encrypted` returns Err once the socket closes.
    let mut aborted = None;
    while let Ok(msg) = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec) {
        if let ServerOpcodeMessage::SMSG_TRANSFER_ABORTED(m) = msg {
            aborted = Some(m);
            break;
        }
    }
    let m = aborted.expect(
        "a world-port whose ENTRY fails must still abort the client's loading screen — silence \
         here is the infinite loading bar this abort exists to kill",
    );
    assert_eq!(
        m.map,
        Map::Deadmines,
        "the abort must name the map the client is loading"
    );
    drop(client);
    let err = server
        .join()
        .unwrap()
        .expect_err("a half-entered world-port must still end the session");
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
    // Same flags + heading as the run-start -> a pure heartbeat -> held pending (nowhere near the
    // 150ms window in real wall-clock terms, since these all write back-to-back with no sleep).
    MSG_MOVE_HEARTBEAT_Client {
        info: mk(RUN, 0.0, 5.0, 200),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // A second same-vector heartbeat SUPERSEDES the first (rule 3: the pending slot IS the drop
    // mechanism) — x=5.0 must never reach the module.
    MSG_MOVE_HEARTBEAT_Client {
        info: mk(RUN, 0.0, 10.0, 300),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // A turn: different flags -> a STATE CHANGE. Must flush the pending x=10.0 heartbeat FIRST,
    // then forward the turn itself, both undelayed.
    MSG_MOVE_START_TURN_LEFT_Client {
        info: mk(TURN, 5.0, 10.0, 400),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // A stop: another STATE CHANGE, no pending left to flush — forwards immediately alone.
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
            lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT, // the FLUSHED x=10.0 heartbeat
            lyracore_shared::opcodes::movement::MSG_MOVE_START_TURN_LEFT,
            lyracore_shared::opcodes::movement::MSG_MOVE_STOP,
        ],
        "run-start, [flushed heartbeat], turn, stop — in that exact order; the turn and stop must \
         never be held, and the superseded x=5.0 intermediate must never appear at all"
    );
    // The flushed heartbeat carries the LATEST pending position (x=10.0), not the dropped x=5.0 one.
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
    // Three same-vector heartbeats, sent back-to-back (well inside the 150ms window): each
    // supersedes the last in the pending slot, so only the FINAL one (x=30.0) may ever ship.
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
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {} // the query itself was still answered normally
        other => panic!("expected SMSG_QUESTGIVER_STATUS, got {other}"),
    }
    // Safe to inspect now: the reply we just read could only have been produced AFTER the query's
    // dispatch ran (flush, then the query handler) on the single-threaded reader/dispatch loop.
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
    // The module's `entity_by_owner` failures (the player's entity is gone — a desync, e.g. a
    // schema-change publish dropped the gateway's read subscription) must be session-FATAL so the
    // handler forces a CLEAN disconnect instead of leaving a silent zombie (can't attack / can't logout).
    assert!(is_desync_error(&anyhow!("attacker not in world")));
    assert!(is_desync_error(&anyhow!("caster not in world")));
    assert!(is_desync_error(&anyhow!("no live entity for guid 5")));
    assert!(is_desync_error(&anyhow!("Player NOT IN WORLD"))); // case-insensitive
                                                               // TRANSIENT per-action failures are NOT desync — they stay swallowed (the player keeps playing,
                                                               // never disconnected). A false positive here would drop players on every dead-target swing.
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
    assert_eq!(
        depth.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "the login sequence has been fully read by the client, so the writer must have decremented \
         the egress depth for every item it wrote — a depth that only climbs makes every session \
         shed peer movement forever once it passes EGRESS_SHED_DEPTH"
    );
    drop(client);
    let _ = server.join();
}

#[test]
fn logout_while_out_of_combat_succeeds_and_clears_open_loot() {
    // combat_until_ms=0 (default, never in combat) → CMSG_LOGOUT_REQUEST must reply
    // Success/Instant + LOGOUT_COMPLETE and the Store must release Account ownership.
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

    // First message: SMSG_LOGOUT_RESPONSE(Success, Instant)
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
    // Second message: SMSG_LOGOUT_COMPLETE
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOGOUT_COMPLETE => {}
        other => panic!("expected SMSG_LOGOUT_COMPLETE, got {other}"),
    }

    CMSG_LOOT_MONEY {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    drop(client);
    server.join().unwrap();

    // Releasing Account ownership removes the live Character.
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
    // combat_until_ms=u64::MAX → CMSG_LOGOUT_REQUEST must reply FailureInCombat. The session
    // stays alive (verified by sending a second request and getting a second denial), meaning the
    // entity was NOT removed during the handler — the player cannot escape combat by logging out.
    // Note: socket teardown (drop below) still calls leave_world/logout as cleanup; that is correct
    // and separate from the CMSG gate.
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                combat_until_ms: u64::MAX, // always in combat
                ..base.session
            },
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    // First request: must be denied.
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

    // Second request: still denied (session is still alive, entity still in-world).
    // If the first denial had accidentally removed the entity / transitioned to CharSelect,
    // this second request would either hang or produce a different message.
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
    // CMSG_PLAYED_TIME -> SMSG_PLAYED_TIME. The character row carries a durable
    // 3600s total plus a session_start_micros stamped in the recent past, so the reply must be
    // strictly greater than the durable floor (the live span gets folded in) and sane (not absurdly
    // large — bounds the test against a unit mixup, e.g. treating micros as millis).
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
    // The client's ack to our `.speed`-triggered SMSG_FORCE_RUN_SPEED_CHANGE must be
    // consumed cleanly (no reply, no desync/disconnect) — proven by a sentinel opcode right after it
    // still getting its normal reply on the SAME session.
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
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {} // the ack produced no reply of its own
        other => panic!("expected the sentinel (ack swallowed), got {other}"),
    }
    drop(client);
    server.join().unwrap(); // the session ran to a clean close, not a desync teardown
}

#[test]
fn reducer_transport_loss_ends_an_admitted_session_and_frees_one_queue_seat() {
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                // The same dead transport makes leave-world cleanup unreachable. Teardown is best-effort,
                // but the client session and its admission seat must not wait for that reducer.
                logout_error: Some("transport disconnected".into()),
                ..base.session
            },
            combat: CombatState {
                set_target_error: Some("transport disconnected".into()),
                ..base.combat
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
        let result =
            run_world_session_with_queue(server_end, server_store.clone(), server_queue.as_ref());
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
    assert!(format!("{err:#}").contains("transport disconnected"));
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
    assert_eq!(
        queue.request(),
        Admission::Admitted,
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

#[cfg(target_os = "linux")]
#[test]
fn closing_a_world_session_interrupts_a_full_socket_without_draining_its_queue() {
    use std::os::fd::AsRawFd;

    let (_client, mut socket) = world_session_socket_pair();
    socket.set_nonblocking(true).unwrap();
    loop {
        match socket.write(&[0; 65536]) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => panic!("fill owned socket: {error}"),
        }
    }
    socket.set_nonblocking(false).unwrap();
    let (mut tx, queued, _) = session_channel();
    tx.bind_socket(socket.try_clone().unwrap());
    tx.send(Outbound::Raw {
        opcode: 0,
        body: vec![],
    })
    .unwrap();
    let socket_fd = socket.as_raw_fd();
    let (started, ready) = std::sync::mpsc::channel();
    let (finished, result) = std::sync::mpsc::channel();
    let writer = std::thread::spawn(move || {
        let task = std::fs::read_link("/proc/thread-self")
            .expect("this Linux socket fixture requires /proc/thread-self");
        started.send(task).unwrap();
        finished.send(socket.write_all(&[1; 65536])).unwrap();
    });
    let task = ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let syscall_path = std::path::Path::new("/proc").join(task).join("syscall");
    assert!(
        crate::durable_test_support::poll_until(Duration::from_secs(2), || {
            let syscall = std::fs::read_to_string(&syscall_path)
                .expect("this Linux socket fixture requires visibility of its writer's syscall");
            let mut fields = syscall.split_whitespace();
            let number = fields
                .next()
                .and_then(|value| value.parse::<libc::c_long>().ok());
            let fd = fields
                .next()
                .and_then(|value| value.strip_prefix("0x"))
                .and_then(|value| i32::from_str_radix(value, 16).ok());
            matches!(number, Some(n) if n == libc::SYS_sendto || n == libc::SYS_write)
                && fd == Some(socket_fd)
        }),
        "writer did not enter its blocked socket write"
    );
    tx.close();
    assert!(result
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .is_err());
    assert!(matches!(queued.try_recv().unwrap(), Outbound::Raw { .. }));
    writer.join().unwrap();
}
