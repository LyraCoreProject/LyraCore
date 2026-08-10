//! The per-account connection release regressions (the fd/thread leak that ends the
//! process). A child module of `world::tests` for the same reason as its siblings: it reaches
//! `InMemoryStore` without widening anything.

use super::*;

// ===========================================================================================
//  The per-account connection release (the fd/thread leak that ends the process).
//
//  Every distinct account's `PlayerConn` costs a websocket fd + an SDK pump OS thread, and until
//  now nothing ever released one: `accept(2)` returns EMFILE when the fd table runs out and both
//  accept loops propagate it into `main`, so the gateway exits after N sessions where N is a pure
//  function of `ulimit -n`. Releasing is only safe once NO socket for the account remains — these
//  tests pin both halves of that (does release; does NOT release early).
// ===========================================================================================

/// Accounts whose cached per-account connection the store has released so far, in order.
fn released(store: &std::sync::Arc<InMemoryStore>) -> Vec<u64> {
    store.released_conns.lock().unwrap().clone()
}

#[test]
fn the_last_socket_for_an_account_releases_its_cached_connection() {
    // The leak itself: one session, opened and torn down, must reclaim the account's connection.
    let store = std::sync::Arc::new(quest_store());
    let (client, _enc, _dec, server) = enter_world(store.clone(), 1);
    assert!(
        released(&store).is_empty(),
        "nothing may be released while the session is live"
    );
    drop(client);
    server.join().unwrap();
    assert_eq!(
        released(&store),
        vec![7],
        "the account's last socket must release its cached per-account connection"
    );
}

#[test]
fn a_reconnect_racing_a_teardown_keeps_the_new_sessions_connection() {
    // THE danger case. Socket B re-logs on the same account while socket A is still tearing down;
    // both share ONE cached `PlayerConn`. Releasing on A's teardown would cut the LIVE player's
    // link — worse than the leak. A's teardown must therefore release nothing, and B must still be
    // servable afterwards.
    let store = std::sync::Arc::new(quest_store());
    let (client_a, _a_enc, _a_dec, server_a) = enter_world(store.clone(), 1);
    let (mut client_b, mut b_enc, mut b_dec, server_b) = enter_world(store.clone(), 1);

    // A goes away (the client vanished / the socket reset).
    drop(client_a);
    server_a.join().unwrap();
    assert!(
        released(&store).is_empty(),
        "A's teardown must NOT release the connection B is still using"
    );

    // B is genuinely still alive on the far side of A's teardown — served over the connection that
    // would have been closed. (`leave_world` back to character select first, so the char enum is
    // dispatched on the realm/default handle exactly as production does.)
    CMSG_LOGOUT_REQUEST {}
        .write_encrypted_client(&mut client_b, &mut b_enc)
        .unwrap();
    ServerOpcodeMessage::read_encrypted(&mut client_b, &mut b_dec).unwrap(); // SMSG_LOGOUT_RESPONSE
    ServerOpcodeMessage::read_encrypted(&mut client_b, &mut b_dec).unwrap(); // SMSG_LOGOUT_COMPLETE
    CMSG_CHAR_ENUM {}
        .write_encrypted_client(&mut client_b, &mut b_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client_b, &mut b_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_ENUM(_) => {}
        other => panic!("B must still be servable after A's teardown, got {other}"),
    }
    assert!(
        released(&store).is_empty(),
        "still nothing released while B is live"
    );

    // Only when B — the last socket — goes does the connection get reclaimed, exactly once.
    drop(client_b);
    server_b.join().unwrap();
    assert_eq!(
        released(&store),
        vec![7],
        "the LAST socket releases the connection, and only it"
    );
}

#[test]
fn a_stale_epoch_teardown_still_releases_when_it_is_the_last_socket() {
    // The release gate is the SOCKET count, not the entity epoch. A socket whose epoch was
    // superseded (so `leave_world` correctly skips `logout`) is still the last socket here, and
    // its connection must still be reclaimed — otherwise every superseded session leaks, which is
    // precisely the login-storm / mass-disconnect shape that exhausts the fd table this module
    // guards against.
    let mut s = quest_store();
    s.stale_session = true;
    let store = std::sync::Arc::new(s);
    let (client, _enc, _dec, server) = enter_world(store.clone(), 1);
    drop(client);
    server.join().unwrap();
    assert!(
        !store
            .logout_called
            .load(std::sync::atomic::Ordering::SeqCst),
        "precondition: a superseded epoch must not delete the newer session's entity"
    );
    assert_eq!(
        released(&store),
        vec![7],
        "a superseded session is still a socket, and its teardown is still a release"
    );
}
