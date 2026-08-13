//! Multi-shard routing — the routing half of Phase A of the elastic-sharding spec.
//!
//! A child module of `world::tests` for the same reason as its siblings: it reaches
//! `InMemoryStore` and its fake realm-core topology without widening anything. `ShardCallLog` and
//! `sharded_stores` are `pub(super)` — the cross-database transfer tests (`transfer_tests`) reuse
//! both, the same way `loot_tests` reuses `party_tests`'s fixtures.

use super::*;

// ===========================================================================================
//  Multi-shard routing — the routing half of Phase A of the elastic-sharding spec.
//  Requirement: reducer calls and subscriptions never target a shard other than the player's home
//  shard. The `InMemoryStore` pair below stands for two DATABASES sharing one ordered call log, so
//  a test can read off exactly which database served every player-scoped call of a whole live
//  session.
// ===========================================================================================

pub(super) type ShardCallLog = std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>;

/// A two-database topology: `world` (the default handle the listener hands every session — where
/// accounts, sessions, and the character list live) and `instances` (the shard that owns this
/// character's location, i.e. what `home_shard` resolves to). Both write to one shared call log.
pub(super) fn sharded_stores() -> (std::sync::Arc<InMemoryStore>, ShardCallLog) {
    sharded_stores_with_home_entity(true)
}

/// Build the routed topology with an explicit answer for whether the old-map entity still exists.
/// A genuine world-port ack follows `teleport_player`, which has removed it; ordinary routed
/// sessions remain in-world and use the default constructor above.
fn sharded_stores_with_home_entity(
    home_entity_in_world: bool,
) -> (std::sync::Arc<InMemoryStore>, ShardCallLog) {
    let calls: ShardCallLog = Default::default();
    // The character's post-world-port entity, for the re-entry test below.
    let mut ported = warrior_entity();
    ported.map_id = 1;
    let home = std::sync::Arc::new(InMemoryStore {
        entity_in_world: home_entity_in_world,
        shard: "instances".into(),
        calls: calls.clone(),
        login_entity: Some(warrior_entity()),
        worldport_entity: Some(ported),
        ..Default::default()
    });
    let world = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        calls: calls.clone(),
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        characters: vec![codec::CharacterView {
            guid: 1,
            name: "Tester".into(),
            race: 1,
            class: 1,
            level: 1,
            ..Default::default()
        }],
        login_entity: Some(warrior_entity()),
        home: Some(home),
        ..Default::default()
    });
    (world, calls)
}

/// One heartbeat at a fixed position — the movement half of the routed traffic.
fn heartbeat(timestamp: u32) -> wow_world_messages::vanilla::MSG_MOVE_HEARTBEAT_Client {
    use wow_world_messages::vanilla::{MovementInfo, MovementInfo_MovementFlags, Vector3d};
    wow_world_messages::vanilla::MSG_MOVE_HEARTBEAT_Client {
        info: MovementInfo {
            flags: MovementInfo_MovementFlags::empty(),
            timestamp,
            position: Vector3d {
                x: -8950.0,
                y: -130.0,
                z: 83.0,
            },
            orientation: 1.5,
            fall_time: 0.0,
        },
    }
}

/// Drive a full session (char-select → login → movement → an attack → disconnect) and return the
/// ordered `(shard, call)` log.
pub(super) fn drive_routed_session(
    store: std::sync::Arc<InMemoryStore>,
    calls: ShardCallLog,
) -> Vec<(String, String)> {
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store;
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);

    // Character select — REALM-scoped, before any character (and therefore any shard) is chosen.
    CMSG_CHAR_ENUM {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();

    // Enter the world: this is where the session pins itself to the character's home shard.
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    for _ in 0..10 {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }

    // In-world traffic: a movement heartbeat (flushed by the following non-movement opcode) and a
    // melee swing — one subscription-driven path and one reducer path.
    heartbeat(100)
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    CMSG_ATTACKSWING {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let _ = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec);

    drop(client); // EOF → teardown runs `logout`
    server.join().unwrap();
    let log = calls.lock().unwrap().clone();
    log
}

#[test]
fn every_player_scoped_call_after_login_targets_the_home_shard_only() {
    // Requirement: once the session resolves the character's home shard, EVERY reducer call and the
    // per-player subscription run against that database — the login itself, the AOI/relay
    // subscription, movement, combat, and the teardown logout. Nothing leaks back to the default.
    let (store, calls) = sharded_stores();
    let log = drive_routed_session(store, calls);

    assert_eq!(
        log.first().map(|(s, c)| (s.as_str(), c.as_str())),
        Some(("world", "characters")),
        "character select is realm-scoped and must stay on the default database: {log:?}"
    );
    let after_login = &log[1..];
    assert!(
        after_login.iter().all(|(shard, _)| shard == "instances"),
        "no call may target a shard other than the player's home shard: {log:?}"
    );
    for expected in [
        "player_login",
        "subscribe_player_events", // the per-player connection + AOI subscriptions
        "movement_update",
        "start_attack",
        "logout",
    ] {
        assert!(
            log.iter()
                .any(|(shard, call)| shard == "instances" && call == expected),
            "{expected} must have run on the home shard: {log:?}"
        );
    }
}

#[test]
fn a_single_entry_shard_map_never_routes_and_keeps_every_call_on_the_one_database() {
    // The safety property: with no second shard to resolve to — which is what a single-entry
    // (default/unconfigured) shard map always answers — the session never swaps handles, so the
    // whole flow is served by the database the listener handed it, byte-identically to before.
    let (store, calls) = sharded_stores();
    let single = std::sync::Arc::new(InMemoryStore {
        entity_in_world: true,
        shard: "world".into(),
        calls: calls.clone(),
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        characters: store.characters.clone(),
        login_entity: Some(warrior_entity()),
        home: None, // ← a single-entry shard map: "you are already on the right shard"
        ..Default::default()
    });
    let log = drive_routed_session(single, calls);

    assert!(
        log.iter().all(|(shard, _)| shard == "world"),
        "an unrouted session must never leave its own database: {log:?}"
    );
    for expected in [
        "characters",
        "player_login",
        "subscribe_player_events",
        "movement_update",
        "logout",
    ] {
        assert!(
            log.iter().any(|(_, call)| call == expected),
            "{expected} missing: {log:?}"
        );
    }
}

#[test]
fn a_routing_flip_re_routes_the_next_entrant_and_leaves_the_resident_alone() {
    // Home-shard routing at the session level. `pool-b` stands for the shard a character was
    // just re-homed to; the mock swaps its answer between the two logins the way a shard-map
    // edit (or a realm-core index re-home) landing between them does.
    //
    // TWO claims are being pinned here, and they are different claims:
    //   1. NEW ENTRANTS follow the flip — the second session's login, subscription, movement,
    //      combat and logout all run on `pool-b`.
    //   2. RESIDENTS DO NOT — the first session's traffic stays on `instances` for its whole life,
    //      because routing is resolved once per world ENTRY and the pin is never revisited.
    //      Nothing moves a live session.
    let calls: ShardCallLog = Default::default();
    let resolutions: std::sync::Arc<std::sync::atomic::AtomicUsize> = Default::default();
    let instances = std::sync::Arc::new(InMemoryStore {
        entity_in_world: true,
        shard: "instances".into(),
        calls: calls.clone(),
        home_shard_calls: resolutions.clone(),
        login_entity: Some(warrior_entity()),
        ..Default::default()
    });
    let pool_b = std::sync::Arc::new(InMemoryStore {
        entity_in_world: true,
        shard: "pool-b".into(),
        calls: calls.clone(),
        home_shard_calls: resolutions.clone(),
        login_entity: Some(warrior_entity()),
        ..Default::default()
    });
    let world = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        calls: calls.clone(),
        home_shard_calls: resolutions.clone(),
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        characters: vec![codec::CharacterView {
            guid: 1,
            name: "Tester".into(),
            race: 1,
            class: 1,
            level: 1,
            ..Default::default()
        }],
        login_entity: Some(warrior_entity()),
        home: Some(instances),
        home_after_flip: Some(pool_b), // the routing flips between the two sessions
        ..Default::default()
    });

    // Session 1 — the RESIDENT. Enters before the flip.
    let resident = drive_routed_session(world.clone(), calls.clone());
    assert!(
        resident
            .iter()
            .skip(1)
            .all(|(shard, _)| shard == "instances"),
        "the resident's whole session must stay on the shard it entered on: {resident:?}"
    );
    assert!(
        !resident.iter().any(|(shard, _)| shard == "pool-b"),
        "no resident call may follow the flip — this ticket re-routes NEW ENTRANTS ONLY: {resident:?}"
    );
    assert_eq!(
        resolutions.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "routing is resolved exactly once per world entry, on ANY handle — nothing may \
         re-resolve a live session mid-flight"
    );

    // Session 2 — the NEXT ENTRANT. Same stores, same character, post-flip.
    calls.lock().unwrap().clear();
    let entrant = drive_routed_session(world, calls.clone());
    assert!(
        entrant.iter().skip(1).all(|(shard, _)| shard == "pool-b"),
        "the next entrant must land on the re-homed shard: {entrant:?}"
    );
    for expected in [
        "player_login",
        "subscribe_player_events",
        "movement_update",
        "logout",
    ] {
        assert!(
            entrant
                .iter()
                .any(|(shard, call)| shard == "pool-b" && call == expected),
            "{expected} must have run on the flipped shard: {entrant:?}"
        );
    }
    assert_eq!(
        resolutions.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "two world entries, two resolutions — no more, and none from either session's own traffic"
    );
}

#[test]
fn a_world_port_keeps_the_pin_when_the_home_shard_still_owns_the_new_map() {
    // A world-port re-resolves routing (the new map may belong to another shard). When the shard
    // the session is ALREADY on still owns the destination it answers "no swap needed" — which must
    // KEEP the pin, not silently drop the session back to the default database.
    let (store, calls) = sharded_stores_with_home_entity(false);
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    for _ in 0..10 {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }
    MSG_MOVE_WORLDPORT_ACK {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    // 9, not 10: the re-entry sequence omits SMSG_LOGIN_VERIFY_WORLD.
    for _ in 0..9 {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }
    drop(client);
    server.join().unwrap();

    let log = calls.lock().unwrap().clone();
    assert!(
        log.iter().all(|(shard, _)| shard == "instances"),
        "the world-port re-entry must stay on the pinned home shard: {log:?}"
    );
    assert_eq!(
        log.iter().filter(|(_, c)| c == "player_login").count(),
        2,
        "login + world-port re-entry both ran on the home shard: {log:?}"
    );
    assert_eq!(
        log.iter()
            .filter(|(_, c)| c == "subscribe_player_events")
            .count(),
        2,
        "the re-entry re-subscribed on the home shard, not the default one: {log:?}"
    );
}

#[test]
fn a_spurious_worldport_ack_is_ignored_on_a_session_pinned_off_the_default_shard() {
    // The gate reads the live entity through the handler's `store`, which `on_home_shard!` has
    // already routed home. If either stops holding, the stray ack re-runs the world entry.
    let calls: ShardCallLog = Default::default();
    let home = std::sync::Arc::new(InMemoryStore {
        shard: "instances".into(),
        calls: calls.clone(),
        login_entity: Some(warrior_entity()),
        // The live entity IS in the world on the home shard — the ack is spurious.
        entity_in_world: true,
        ..Default::default()
    });
    let world = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        calls: calls.clone(),
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        characters: vec![codec::CharacterView {
            guid: 1,
            name: "Tester".into(),
            race: 1,
            class: 1,
            level: 1,
            ..Default::default()
        }],
        login_entity: Some(warrior_entity()),
        // The default handle has NO entity for this guid (it lives on `instances`) — a
        // default-shard read here would wrongly answer "absent" and re-enter.
        entity_in_world: false,
        home: Some(home.clone()),
        ..Default::default()
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = world.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    for _ in 0..10 {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }

    // The stray ack: no teleport despawned the entity — it must be dropped, not answered.
    MSG_MOVE_WORLDPORT_ACK {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client); // EOF right after — the session loop consumes the ack, then tears down
    server.join().unwrap();

    assert_eq!(
        home.login_calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a spurious ack on a pinned session must be ignored — the world entry ran again"
    );
    assert_eq!(
        home.subscribed.lock().unwrap().len(),
        1,
        "a spurious ack must not tear down and re-register the session's subscriptions"
    );
}

#[test]
fn a_spurious_worldport_ack_is_ignored_on_the_default_shard() {
    // The single-database twin of the test above — the `entity_in_world: true` ignore path was
    // untested before these two.
    let store = std::sync::Arc::new(InMemoryStore {
        login_entity: Some(warrior_entity()),
        entity_in_world: true,
        ..tester_store(7)
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    for _ in 0..10 {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }
    MSG_MOVE_WORLDPORT_ACK {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client);
    server.join().unwrap();

    assert_eq!(
        store.login_calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a spurious ack with the entity live must be ignored"
    );
}

#[test]
fn a_logout_to_character_select_releases_the_home_shard_pin() {
    // Adversarial-review finding: `leave_world` returns the socket to CharSelect but the session
    // stays open, so the NEXT character-select frames (char enum / create / delete) are dispatched
    // through `on_home_shard!` again. Those are REALM-scoped — `game_account` / `game_character`
    // live on the default database — so a pin left over from the character we just logged out of
    // would serve the character list off an instance shard (which, being empty, shows the player
    // no characters at all, and would create/delete rows on the wrong database).
    let (store, calls) = sharded_stores();
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);

    // Enter the world (pins to "instances"), then log out back to character select.
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    for _ in 0..10 {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }
    CMSG_LOGOUT_REQUEST {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap(); // SMSG_LOGOUT_RESPONSE
    ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap(); // SMSG_LOGOUT_COMPLETE

    // Back at character select: this must be served by the REALM (default) database again.
    CMSG_CHAR_ENUM {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    let enumerated = match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_ENUM(e) => e.characters.len(),
        other => panic!("expected SMSG_CHAR_ENUM, got {other}"),
    };
    drop(client);
    server.join().unwrap();

    let log = calls.lock().unwrap().clone();
    assert_eq!(
        log.iter()
            .filter(|(s, c)| c == "characters" && s == "world")
            .count(),
        1,
        "the post-logout character enum must run on the realm/default database: {log:?}"
    );
    assert_eq!(
        enumerated, 1,
        "the player must still see their characters after a logout"
    );
}

#[test]
fn a_freshly_created_characters_first_login_transfers_off_the_default_shard() {
    // Requirement: `create_character` always writes to the DEFAULT/realm shard, even when the
    // start position routes to a different one under `LYRACORE_SHARD_MAP` — a deliberate decision (see
    // the doc comment on `impl WorldStore for Coordinator::create_character`): create-then-
    // transfer-on-first-login, not create-directly-on-the-owning-shard. That decision rides the
    // SAME `route_home`/`settle_home_shard` machinery every other login already uses — prove it
    // end to end for a guid the CREATE call ITSELF produced, not one hardcoded
    // independently of it (the tautology the first version of this test was caught on: a bare
    // `CMSG_PLAYER_LOGIN { guid: Guid::new(1) }` logs into `sharded_stores()`'s pre-seeded
    // "Tester" fixture whether or not the preceding CREATE ever ran).
    //
    // So this drives the REAL 1.12 flow instead: CREATE, then re-ENUMERATE — `SMSG_CHAR_CREATE`
    // carries no guid, the client is expected to learn it from the next `CMSG_CHAR_ENUM` — and
    // pick "Newbie"'s guid out of THAT reply before logging in with it. `InMemoryStore::
    // create_character` now actually records the character (see its doc comment) so this guid is
    // genuinely create-produced, and `sharded_stores()`'s single connected `home` shard
    // (`instances`) stands in for a start map that routes off `world`.
    let (store, calls) = sharded_stores();

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);

    // The creation itself must land on the default (`world`) shard — the chosen behaviour, not an
    // accident of this test.
    CMSG_CHAR_CREATE {
        name: "Newbie".into(),
        race: Race::Orc,
        class: Class::Warrior,
        gender: Gender::Male,
        skin_color: 0,
        face: 0,
        hair_style: 0,
        hair_color: 0,
        facial_hair: 0,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_CREATE(m) => {
            assert_eq!(m.result, WorldResult::CharCreateSuccess)
        }
        other => panic!("expected SMSG_CHAR_CREATE, got {other}"),
    }

    // Learn the new character's guid the way a real client does: re-enumerate.
    CMSG_CHAR_ENUM {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    let new_guid = match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_ENUM(e) => e
            .characters
            .iter()
            .find(|c| c.name == "Newbie")
            .unwrap_or_else(|| {
                panic!(
                    "the freshly created character never appeared in the char enum — without \
                     this, the login below cannot possibly be testing the character CREATE just \
                     produced. characters: {:?}",
                    e.characters
                )
            })
            .guid
            .guid(),
        other => panic!("expected SMSG_CHAR_ENUM, got {other}"),
    };

    // That same character's FIRST login, in the SAME session, right after creation.
    CMSG_PLAYER_LOGIN {
        guid: Guid::new(new_guid),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    for _ in 0..10 {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }
    drop(client);
    server.join().unwrap();

    let log = calls.lock().unwrap().clone();
    for expected in ["player_login", "subscribe_player_events", "logout"] {
        assert!(
            log.iter()
                .any(|(shard, call)| shard == "instances" && call == expected),
            "the freshly created character's FIRST login must drive the transfer onto its start \
             map's owning shard, exactly like every later world entry — {expected} never ran on \
             `instances`: {log:?}"
        );
    }
    assert!(
        !log.iter().any(|(shard, call)| shard == "world"
            && (call == "player_login" || call == "subscribe_player_events")),
        "the first login must not stay on the default shard once routing resolves it elsewhere: \
         {log:?}"
    );
}
