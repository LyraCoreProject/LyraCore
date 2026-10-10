//! Cast dispatch through the encrypted session. Route selection, target decoding, durable dispatch
//! and message ORDER are covered per route by the focused seam tests in `handlers/cast`. What only a
//! socket can show survives here: that a cast opcode reaches the seam through the real dispatch
//! chain, that the ordered batch leaves the writer as those frames in that order, and that the
//! session supplies the caller's identity.

use super::*;

/// Vanilla opcodes for the frames the cast path emits (raw + typed) — pinned as numbers because the
/// synchronous ORDER across `Outbound::Raw` and typed sends is the contract under test.
const OP_CAST_RESULT: u16 = 0x0130;

const OP_SPELL_START: u16 = 0x0131;

const OP_SPELL_GO: u16 = 0x0132;

/// `SpellCastTargets` carrying a UNIT target (the client's selected mob).
fn unit_targets(guid: u64) -> SpellCastTargets {
    SpellCastTargets {
        target_flags: SpellCastTargets_SpellCastTargetFlags::new_unit(
            SpellCastTargets_SpellCastTargetFlags_Unit {
                unit_target: Guid::new(guid),
            },
        ),
    }
}

#[test]
fn instant_cast_sends_start_then_raw_cast_result_ok_then_go_and_threads_the_target() {
    // Root-cause client-wedge fix: an INSTANT cast must emit START(0) → raw CAST_RESULT(OK,
    // opcode 0x0130, 5-byte body) → GO synchronously, IN THAT ORDER, and the cast must reach the
    // store with the client's unit target. Raw and typed sends interleave here, so the frame order
    // is a property of the writer, not just of the batch the seam returns.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_CAST_SPELL {
        spell: 100,
        targets: unit_targets(77),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    let (op1, _) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(op1, OP_SPELL_START, "first frame must be SMSG_SPELL_START");
    let (op2, body2) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(
        op2, OP_CAST_RESULT,
        "second frame must be the raw CAST_RESULT ack"
    );
    let mut want = 100u32.to_le_bytes().to_vec();
    want.push(0x00); // SPELL_RESULT_STATUS_OKAY — 5 bytes, NO trailing reason byte
    assert_eq!(
        body2, want,
        "CAST_RESULT body is spell_id(u32 LE) + OKAY(0x00)"
    );
    let (op3, _) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(op3, OP_SPELL_GO, "third frame must be SMSG_SPELL_GO");

    drop(client);
    server.join().unwrap();
    // The unit target rode CMSG → handler → store unchanged (target-keyed effects need it).
    assert_eq!(store.cast.casts.lock().unwrap().as_slice(), &[(100, 77)]);
}

#[test]
fn auto_shot_intercept_starts_the_ranged_attack_instead_of_casting() {
    // Vanilla shape: Auto Shot (75) and wand Shoot (5019) are auto-repeat ranged attacks —
    // the handler arms start_ranged_attack with the cast's unit target, then the activation ack is
    // SMSG_SPELL_START ALONE with timer 0 (no CAST_RESULT, no GO — the cast parks in the client's
    // AUTOREPEAT slot and never resolves; each shot's GO comes from the swing-tick relay).
    // cast_spell must NOT run.
    for spell in [75u32, 5019] {
        let store = std::sync::Arc::new(quest_store());
        let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
        CMSG_CAST_SPELL {
            spell,
            targets: unit_targets(88),
        }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
        let (op, body) = read_raw_frame(&mut client, &mut c_dec);
        assert_eq!(
            op, OP_SPELL_START,
            "spell {spell}: activation ack is SPELL_START alone"
        );
        // timer (u32 LE) is the LAST 4 body bytes before... layout: cast_item(packed) caster(packed)
        // spell(4) flags(2) timer(4) targets(..). Cheap pin: the timer bytes right after the u16
        // flags must be 0 — locate spell id then skip flags. spell sits at a packed-guid-dependent
        // offset; both packed self-guids here are 2 bytes (guid 1 -> [0x01, 0x01]).
        let spell_pos = 4; // two 2-byte packed guids
        assert_eq!(
            &body[spell_pos..spell_pos + 4],
            &spell.to_le_bytes(),
            "spell id in START"
        );
        assert_eq!(
            &body[spell_pos + 6..spell_pos + 10],
            &0u32.to_le_bytes(),
            "spell {spell}: START timer must be 0 (the 0.5s wind-up is an attack-timer, not a cast bar)"
        );
        // Nothing else may follow on the activation path (the old phantom GO fired the shoot
        // animation instantly).
        let mut probe = [0u8; 1];
        assert!(
            client.read(&mut probe).map(|n| n == 0).unwrap_or(true),
            "spell {spell}: no packet may follow the activation START"
        );
        drop(client);
        server.join().unwrap();
        assert_eq!(
            store.cast.ranged_attacks.lock().unwrap().as_slice(),
            &[(88, spell)]
        );
        assert!(
            store.cast.casts.lock().unwrap().is_empty(),
            "spell {spell} must not reach cast_spell"
        );
    }
}

#[test]
fn a_cast_before_entering_the_world_answers_nothing_and_keeps_the_session_alive() {
    // A cast can arrive while the session is still at character select — a stale addon macro, a
    // reconnect race. The seam answers no frames and names a zero caster; the session must serve the
    // next opcode instead of panicking, which the CHAR_ENUM sentinel proves.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);

    CMSG_CAST_SPELL {
        spell: 100,
        targets: unit_targets(77),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_CHAR_ENUM {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_ENUM(_) => {}
        other => panic!("expected SMSG_CHAR_ENUM (the cast sent no frames), got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(store.cast.casts.lock().unwrap().as_slice(), &[(100, 77)]);
}

#[test]
fn cancelling_auto_repeat_still_tears_the_ranged_loop_down_through_stop_attack() {
    // The cancel tears the loop down only when one is armed (the `was_repeat` gate). Arm Auto Shot,
    // then cancel, and pin that the teardown reaches the durable disengage for this attacker.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_CAST_SPELL {
        spell: 75,
        targets: unit_targets(88),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (op, _) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(op, OP_SPELL_START, "the activation ack arms the loop");
    CMSG_CANCEL_AUTO_REPEAT_SPELL {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    // The cancel sends no ack of its own (the on_delete relay does), so a sentinel proves the
    // server got that far before we assert.
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {}
        other => panic!("expected the sentinel (the cancel acks nothing), got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(store.melee.stop_attacks.lock().unwrap().as_slice(), &[1]);
}

#[test]
fn cancel_aura_dispatches_with_the_wire_spell_id() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_CANCEL_AURA { id: 5555 }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.cast.cancelled_auras.lock().unwrap().as_slice(),
        &[5555]
    );
}

#[test]
fn cancel_cast_dispatches_for_the_caller() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_CANCEL_CAST { id: 133 }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(store.cast.cancelled_casts.lock().unwrap().as_slice(), &[1]);
}
