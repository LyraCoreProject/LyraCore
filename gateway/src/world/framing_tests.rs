//! The INBOUND framing boundary: what an authenticated socket does with a header or a
//! body it cannot make sense of.
//!
//! Everything else in `world/tests.rs` sends well-formed `wow_world_messages` types, so the read
//! loop in `run_world_session_with_queue` (`world/mod.rs`) is only ever exercised on its happy path.
//! That loop is nonetheless the first thing a hostile or buggy client reaches after the handshake,
//! and it makes four separate decisions no typed test can reach:
//!
//!   1. a short/absent HEADER is a clean disconnect (`Ok(())`), not an error — the ordinary way
//!      every real session ends;
//!   2. a header whose declared `size` outruns the bytes actually sent is `Err` — the read must not
//!      block forever waiting for a body the peer will never send;
//!   3. `size` counts the u32 opcode, so `size < 4` must not underflow the `body_len` computation
//!      (it is a `saturating_sub`, and this pins that it stays one);
//!   4. an unparseable opcode/body is session-FATAL, deliberately — the stream cipher is a
//!      continuous keystream, so a frame we cannot decode means we have also lost our place in it,
//!      and continuing would interpret ciphertext as headers forever.
//!
//! These drive the REAL `run_world_session` over a real `wow_srp` cipher, using
//! `EncrypterHalf::write_encrypted_client_header` to put bytes on the wire that no typed builder can
//! produce. Each asserts the externally visible outcome — how the session ENDS and what the client
//! sees — rather than any internal flag.
//!
//! Deterministic by construction: every test writes a fixed byte sequence, closes the socket, and
//! joins the session thread. There is no timer, no scheduled reducer and no wall-clock window
//! anywhere in this file.

use super::*;
use std::os::unix::net::UnixStream;

/// The store every test here uses: one account, session key [`K`], nothing else configured. The
/// frames below never reach a handler, so no fixture beyond the handshake is needed.
fn framing_store() -> std::sync::Arc<InMemoryStore> {
    std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 42,
            session_key: K,
        }),
        ..Default::default()
    })
}

/// Handshake, then hand the caller the client's cipher halves and the joined-later session thread.
fn framing_session() -> (
    UnixStream,
    EncrypterHalf,
    DecrypterHalf,
    std::thread::JoinHandle<Result<()>>,
) {
    let store = framing_store();
    let (mut client, server_end) = world_session_socket_pair();
    let server = std::thread::spawn(move || run_world_session(server_end, store.as_ref()));
    let (c_enc, c_dec) = client_handshake(&mut client, "TESTER", K);
    (client, c_enc, c_dec, server)
}

// ===========================================================================================
//  1. Truncated / absent headers — the CLEAN close
// ===========================================================================================

/// The ordinary end of every session: the client closes with nothing pending. `UnexpectedEof` out
/// of `read_and_decrypt_client_header` is the one read failure that is NOT an error, and it has to
/// stay that way — classifying it as `Err` would make every normal logout log a spurious failure
/// and would bury the real framing errors this file's other tests produce.
#[test]
fn a_clean_disconnect_with_no_frame_pending_ends_the_session_without_an_error() {
    let (client, _c_enc, _c_dec, server) = framing_session();
    drop(client);
    server
        .join()
        .unwrap()
        .expect("an EOF at a frame boundary is a normal disconnect, not a framing error");
}

/// A header cut in half. The vanilla client header is 6 bytes (u16 size + u32 opcode); sending 3 of
/// them and closing is what a killed client or a severed link looks like mid-header.
///
/// This is still `UnexpectedEof` from the same read, so it is still the clean path — the point of
/// pinning it separately is that the loop must not try to interpret a PARTIAL header as a whole one
/// (which would decrypt 3 bytes of nothing into an arbitrary size + opcode and then block reading a
/// body that does not exist).
#[test]
fn a_header_truncated_mid_read_is_a_disconnect_not_a_decoded_frame() {
    let (mut client, _c_enc, _c_dec, server) = framing_session();
    client.write_all(&[0xAA, 0xBB, 0xCC]).unwrap();
    drop(client);
    server.join().unwrap().expect(
        "half a header is a severed connection, not a frame — it must end the session cleanly \
         rather than decode into a size/opcode pair and hang on the body read",
    );
}

// ===========================================================================================
//  2. Truncated and oversized BODIES — the loud close
// ===========================================================================================

/// A header that promises more body than the client ever sends. `read_exact` cannot be satisfied,
/// and the session must END rather than block: a peer that can announce a body and then go quiet
/// would otherwise pin a gateway thread (and its per-player SpacetimeDB connection) forever.
///
/// The error text is asserted because it is the operator's only signal for this class — a bare
/// "session ended" would be indistinguishable from a normal logout in the log.
#[test]
fn a_body_shorter_than_its_declared_size_ends_the_session_with_an_error() {
    let (mut client, mut c_enc, _c_dec, server) = framing_session();
    // size = 4 (opcode) + 64 body bytes promised...
    c_enc
        .write_encrypted_client_header(&mut client, 4 + 64, 0x0037 /* CMSG_CHAR_ENUM */)
        .unwrap();
    // ...and only 8 delivered before the socket closes.
    client.write_all(&[0u8; 8]).unwrap();
    drop(client);

    let err = server
        .join()
        .unwrap()
        .expect_err("a body that never arrives must end the session, never block on it");
    let text = format!("{err:#}");
    assert!(
        text.contains("world read error (body)"),
        "a truncated body must be reported as a BODY read error so an operator can tell it from a \
         normal disconnect; got: {text}"
    );
}

/// The maximum a u16 size field can claim: 65535, i.e. 65531 body bytes. Nothing legitimate is that
/// large, so this is the shape a scan or a corrupted stream produces.
///
/// Two properties at once: the gateway must not pre-allocate its way into trouble on an unverified
/// length (it allocates 64 KiB here and that is the ceiling the u16 imposes — the reason the size
/// field being a u16 is load-bearing), and it must not wait forever for the 65531 bytes.
#[test]
fn a_maximum_size_header_with_no_body_ends_the_session_instead_of_waiting_for_64_kib() {
    let (mut client, mut c_enc, _c_dec, server) = framing_session();
    c_enc
        .write_encrypted_client_header(&mut client, u16::MAX, 0x0037)
        .unwrap();
    drop(client);

    let err = server.join().unwrap().expect_err(
        "a header claiming the largest body a u16 can express, with nothing behind it, must end \
         the session",
    );
    assert!(
        format!("{err:#}").contains("world read error (body)"),
        "{err:#}"
    );
}

/// `size` counts the u32 opcode plus the body, so any value below 4 is already malformed. The loop
/// computes `body_len = (hdr.size as usize).saturating_sub(4)`; a plain `-` would underflow to
/// `usize::MAX - 3` here and the very next line (`vec![0u8; body_len]`) would abort the process on
/// a multi-exabyte allocation. That is the invariant this pins, and it is the whole point: the
/// gateway must survive a size field it did not choose.
///
/// It deliberately does NOT assert that the frame is REJECTED. The loop re-frames with the declared
/// size and lets `ClientOpcodeMessage::read_unencrypted` adjudicate, so a body-less opcode like
/// `CMSG_CHAR_ENUM` decodes anyway and the session carries on — a design choice (gtker owns the
/// per-opcode length rules, not this loop) rather than an oversight. Asserting a rejection here
/// would pin a behaviour that does not exist and would break the moment gtker's tables changed.
///
/// Every value 0..=3 is exercised, because the saturating boundary is exactly this range.
#[test]
fn a_size_field_below_the_opcode_width_never_underflows_the_body_length() {
    for size in 0u16..=3 {
        let (mut client, mut c_enc, _c_dec, server) = framing_session();
        c_enc
            .write_encrypted_client_header(&mut client, size, 0x0037 /* CMSG_CHAR_ENUM */)
            .unwrap();
        drop(client);

        // `join()` returning AT ALL is the assertion. Under the underflow the session thread never
        // gets here: the allocation aborts the whole process, taking the test runner with it.
        // Whether the frame then decodes or errors is gtker's call; both are safe.
        let _resolved = server.join().unwrap_or_else(|_| {
            panic!(
                "size={size} (below the 4-byte opcode width) must not underflow `body_len` — the \
                 session thread died instead of returning"
            )
        });
    }
}

// ===========================================================================================
//  3. Unsupported and malformed OPCODES — session-fatal, and why
// ===========================================================================================

/// An opcode this build has no parser for. Vanilla's opcode space is sparse and the 1.12.1 client
/// never sends this one, so it means either a foreign/modified client or a stream we have lost
/// sync with.
///
/// Ending the session is the deliberate choice: header crypto is a CONTINUOUS keystream, so the
/// gateway cannot "skip this frame and carry on" — it has already consumed this frame's keystream
/// bytes and has no way to know whether the body length it read was real. Every later header would
/// decrypt to garbage. The client sees a clean close and reconnects; that is recoverable, an
/// endlessly desynced socket is not.
#[test]
fn an_unsupported_opcode_ends_the_session_rather_than_desyncing_the_keystream() {
    let (mut client, mut c_enc, _c_dec, server) = framing_session();
    c_enc
        .write_encrypted_client_header(&mut client, 4, 0xDEAD)
        .unwrap();
    drop(client);

    let err = server
        .join()
        .unwrap()
        .expect_err("an opcode with no parser must end the session");
    assert!(format!("{err:#}").contains("world read error"), "{err:#}");
}

/// A second `CMSG_AUTH_SESSION` once auth is over. The typed decoder would size a buffer from the
/// addon field and unwrap the zlib, so the loop refuses the opcode before decoding. Session-fatal
/// like every other frame the loop cannot take.
#[test]
fn an_auth_session_after_auth_is_refused_before_the_typed_decoder_sees_it() {
    let (mut client, mut c_enc, _c_dec, server) = framing_session();
    let mut body = Vec::new();
    body.extend_from_slice(&[0u8; 8]); // build, server_id
    body.extend_from_slice(b"TESTER\0");
    body.extend_from_slice(&[0u8; 24]); // client_seed, client_proof
    body.extend_from_slice(&u32::MAX.to_le_bytes()); // 4 GiB addon claim over garbage
    body.extend_from_slice(&[0xFF; 32]);
    c_enc
        .write_encrypted_client_header(
            &mut client,
            (body.len() + 4) as u16,
            CMSG_AUTH_SESSION_OPCODE,
        )
        .unwrap();
    client.write_all(&body).unwrap();
    drop(client);

    let err = server
        .join()
        .unwrap()
        .expect_err("a post-auth CMSG_AUTH_SESSION must end the session");
    assert!(
        format!("{err:#}").contains("CMSG_AUTH_SESSION after auth"),
        "{err:#}"
    );
}

/// A KNOWN opcode carrying a body its parser cannot consume: `CMSG_CAST_SPELL` needs a spell id and
/// a target block, and gets one stray byte.
///
/// This is the case an opcode allow-list would miss — the frame passes any "do we know this
/// opcode?" check and still fails to decode. Same verdict, same reason as an unknown opcode: the
/// decode failure means the declared length was not the real length.
#[test]
fn a_known_opcode_with_an_undecodable_body_is_session_fatal_too() {
    let (mut client, mut c_enc, _c_dec, server) = framing_session();
    c_enc
        .write_encrypted_client_header(&mut client, 4 + 1, 0x012E /* CMSG_CAST_SPELL */)
        .unwrap();
    client.write_all(&[0x01]).unwrap();
    drop(client);

    let err = server
        .join()
        .unwrap()
        .expect_err("a known opcode whose body will not decode must still end the session");
    assert!(format!("{err:#}").contains("world read error"), "{err:#}");
}

// ===========================================================================================
//  4. Session CLEANUP on the error path — the seat and the world entity
// ===========================================================================================

/// The login queue's seat accounting has to survive the abnormal exit, not only the polite one. A
/// malformed frame ends `run_world_session_with_queue` through its `Err` path; the `queue.depart()`
/// in the teardown runs after the read loop returns, however it returned.
///
/// If a seat leaked on the error path, a realm at `LYRACORE_MAX_SESSIONS` would lose one seat per
/// crashed or hostile client and eventually refuse every login, with nothing in the log to say why
/// and no cure short of a restart. That is the exact shape of an outage that gets misdiagnosed as
/// "the module is wedged".
#[test]
fn a_session_ending_in_a_framing_error_still_gives_its_seat_back() {
    let store = framing_store();
    let queue = std::sync::Arc::new(LoginQueue::new(1, 0));

    let (mut client, server_end) = world_session_socket_pair();
    let server_queue = queue.clone();
    let server = std::thread::spawn(move || {
        run_world_session_with_queue(server_end, store.as_ref(), &server_queue)
    });
    let (mut c_enc, _c_dec) = client_handshake(&mut client, "TESTER", K);
    assert_eq!(
        queue.active(),
        1,
        "precondition: the admitted session holds the only seat"
    );

    // End it the ugly way.
    c_enc
        .write_encrypted_client_header(&mut client, 4, 0xDEAD)
        .unwrap();
    drop(client);
    server
        .join()
        .unwrap()
        .expect_err("the malformed frame must end the session with an error");

    assert_eq!(
        queue.active(),
        0,
        "the seat must be released on the ERROR path too — otherwise every malformed frame \
         permanently shrinks the realm's capacity by one"
    );
    assert_eq!(
        queue.request(),
        Admission::Admitted,
        "and the freed seat must be immediately usable by the next login"
    );
}

/// The other half of teardown: the player's world ENTITY. A session that dies on a framing error
/// while the player is in the world must still run `logout`, or the character stays spawned —
/// visible to everyone, unattackable, and blocking their own relog.
///
/// The clean-logout path is already covered (`logout_while_out_of_combat_succeeds`); this pins that
/// the same cleanup happens when the loop unwinds instead of returning `Ok`, which is the case an
/// early-return added to the error branch would silently break.
#[test]
fn a_session_that_dies_in_world_still_deletes_the_players_entity() {
    let store = std::sync::Arc::new(quest_store());
    let queue = std::sync::Arc::new(LoginQueue::unlimited());

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server_queue = queue.clone();
    let server = std::thread::spawn(move || {
        run_world_session_with_queue(server_end, server_store.as_ref(), &server_queue)
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);

    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    // Drain the login sequence; the last frames are partial-VALUES updates gtker's decoder rejects
    // by design, so read tolerantly (same reason as `enter_world`'s drain).
    for _ in 0..10 {
        let _ = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec);
    }
    assert!(
        !store
            .logout_called
            .load(std::sync::atomic::Ordering::SeqCst),
        "precondition: the player is in the world and has not logged out"
    );

    // Die on a malformed frame rather than logging out.
    c_enc
        .write_encrypted_client_header(&mut client, 4, 0xDEAD)
        .unwrap();
    drop(client);
    server
        .join()
        .unwrap()
        .expect_err("the malformed frame must end the session with an error");

    assert!(
        store
            .logout_called
            .load(std::sync::atomic::Ordering::SeqCst),
        "an in-world session that ends on an error must still run `logout` — otherwise the \
         character stays spawned in the world with nobody driving it, and the player cannot relog \
         onto it"
    );
}

/// The counterweight to the three tests above, and the reason they are worth having: the read loop
/// must be strict about FRAMING without becoming strict about CONTENT.
///
/// An addon frame (`CMSG_MESSAGECHAT` in the addon pseudo-language) whose `STC` envelope is
/// malformed is other-server traffic or an addon bug — never grounds to disconnect. It is dropped
/// inside `handle_addon_message`, and the proof that the session survived is that a perfectly
/// ordinary `CMSG_CHAR_ENUM` sent AFTER it is still answered.
#[test]
fn a_malformed_addon_envelope_is_dropped_and_the_session_keeps_serving() {
    let store = framing_store();
    let (mut client, server_end) = world_session_socket_pair();
    let server = std::thread::spawn(move || run_world_session(server_end, store.as_ref()));
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);

    // A well-FORMED addon chat frame carrying a body the `STC` envelope parser rejects.
    let mut body = Vec::new();
    body.extend_from_slice(&0u32.to_le_bytes()); // chat type SAY — no target CString to skip
    body.extend_from_slice(&codec::addon::LANG_ADDON.to_le_bytes());
    body.extend_from_slice(b"NOTSTC|garbage\0");
    c_enc
        .write_encrypted_client_header(
            &mut client,
            4 + body.len() as u16,
            codec::addon::CMSG_MESSAGECHAT_OPCODE,
        )
        .unwrap();
    client.write_all(&body).unwrap();

    // The session must still be alive to answer this.
    CMSG_CHAR_ENUM {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    let reply = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec)
        .expect("the session must still answer after a malformed addon envelope");
    assert!(
        matches!(reply, ServerOpcodeMessage::SMSG_CHAR_ENUM(_)),
        "expected the char list, got {reply} — a foreign addon frame must never be session-fatal"
    );

    drop(client);
    server
        .join()
        .unwrap()
        .expect("a dropped addon frame must leave the session ending cleanly");
}
