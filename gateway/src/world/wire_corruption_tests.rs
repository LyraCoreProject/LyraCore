//! "SMSG_COMPRESSED_MOVES corrupts at crowd scale" — regression pins.
//!
//! A child module of `world::tests` for the same reason as its siblings — it reaches
//! `InMemoryStore`/`client_handshake`/`world_handshake` and drives the ACTUAL `spawn_writer` with a
//! REAL `wow_srp` cipher pair, without widening anything.

use super::*;

// ===========================================================================================
//  "SMSG_COMPRESSED_MOVES corrupts at crowd scale" — regression pins.
//
//  Root-cause finding: the gateway never constructs `SMSG_COMPRESSED_MOVES` — a full-repo grep for
//  `compressed_moves`/`COMPRESSED_MOVES`/`flate2` and a `git log -S "COMPRESSED_MOVES" --all` both
//  come up empty. A direct probe of `wow_world_messages` 0.3's own codec for that opcode (encode via
//  `write_into_vec`, decode via `ServerOpcodeMessage::read_unencrypted`) round-trips cleanly through
//  at least 3000 synthetic movers; it only silently truncates its u16 size field past ~9000 movers,
//  an order of magnitude past the reported corruption range. So "compressed moves corrupts" is not
//  an encode defect in that opcode.
//
//  The likelier mechanism: `SMSG_COMPRESSED_MOVES` and `SMSG_COMPRESSED_UPDATE_OBJECT` are the ONLY
//  two decoders in the entire vanilla message set with an internal `.unwrap()` (on a flate2
//  `read_to_end`) instead of a graceful `Result` — every other decoder in `wow_world_messages` 0.3
//  returns `Err` on garbage input. `wow_world_messages`' own framing reads exactly the header's
//  declared `size` bytes into an in-memory buffer BEFORE dispatching to any per-opcode decoder, so a
//  decode-internal panic on THIS opcode cannot itself desync the socket — but if an EARLIER frame's
//  declared size was ever wrong for any reason, every later header decrypts from the wrong stream
//  offset, and the first opcode whose decoder panics instead of erroring is deterministically where
//  the crash surfaces — regardless of what actually caused the desync. That makes this decoder the
//  "weakest link" any unrelated corruption converges on, not necessarily the origin.
//
//  What follows pins the two real, high-volume movement paths — `SMSG_MONSTER_MOVE` typed sends and
//  the `Outbound::Raw` peer-motion relay — through the ACTUAL `spawn_writer` (the single writer
//  thread that owns the header cipher) with a REAL `wow_srp` cipher pair, at the exact scale
//  reported (100 known-good, 150/300/500 the reported-bad range). If the writer ever
//  mis-declared one frame's size, this is where it would show up: as a decode failure on THIS test,
//  not as an unrelated panic on an opcode the gateway doesn't send.
// ===========================================================================================

/// Push `n` movers' worth of both hot movement paths through a real `spawn_writer` + real cipher
/// pair, then decode every frame back and demand: no decode error, and exactly `n` of each opcode,
/// in order. A stream desync from a wrong size field would manifest here as either a decode `Err`
/// or a frame decoding to the wrong `ServerOpcodeMessage` variant.
fn movement_burst_over_a_real_cipher(n: usize) {
    let store = std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 42,
            session_key: K,
        }),
        ..Default::default()
    });
    let (mut client, server_end) = UnixStream::pair().unwrap();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        let mut s = server_end;
        let (_conn, encrypt) = world_handshake(&mut s, server_store.as_ref())
            .unwrap()
            .expect("handshake should succeed");
        (s, encrypt)
    });
    let (_c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    let (s, encrypt) = server
        .join()
        .expect("server handshake thread must not panic");

    let (tx, rx, depth) = session_channel();
    let writer = spawn_writer(s, encrypt, rx, depth, 1).expect("spawn the writer thread");

    // The reader MUST run concurrently with the writer, not after `writer.join()` — a Unix domain
    // socket's kernel send buffer is finite, and a burst past it makes the writer's `write_all`
    // block waiting for a reader that (if it only starts after the writer finishes) never comes:
    // a real deadlock, caught empirically writing this test (it hung indefinitely at n=500 with
    // both threads idle, not merely slow). Draining while the writer produces mirrors what a real
    // socket pair does in production — neither side ever waits on a queue nobody is draining.
    let reader = std::thread::spawn(move || {
        let mut monster_moves = 0usize;
        let mut heartbeats = 0usize;
        for _ in 0..(2 * n) {
            match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec) {
                Ok(ServerOpcodeMessage::SMSG_MONSTER_MOVE(_)) => monster_moves += 1,
                Ok(ServerOpcodeMessage::MSG_MOVE_HEARTBEAT(_)) => heartbeats += 1,
                Ok(other) => panic!(
                    "n={n}: decoded an unexpected opcode after {monster_moves} monster-moves + \
                     {heartbeats} heartbeats — this IS a stream desync: {other}"
                ),
                Err(e) => panic!(
                    "n={n}: decode failed after {monster_moves} monster-moves + {heartbeats} \
                     heartbeats — exactly the crash shape this file guards against (a desynced \
                     header decoding to garbage): {e}"
                ),
            }
        }
        (monster_moves, heartbeats)
    });

    for i in 0..n {
        let guid = 0x1000_0000_0000_0000u64 + i as u64;
        let start = wow_world_messages::vanilla::Vector3d {
            x: -8938.0 + i as f32,
            y: -131.0,
            z: 83.5,
        };
        let dest = wow_world_messages::vanilla::Vector3d {
            x: -8900.0 + i as f32,
            y: -100.0,
            z: 82.4,
        };
        let m = codec::build_monster_move(guid, start, dest, 500, i as u32 + 1, i % 2 == 0);
        tx.send(Outbound::One(ServerOpcodeMessage::SMSG_MONSTER_MOVE(
            Box::new(m),
        )))
        .expect("writer thread must still be alive mid-burst");

        let info = MovementInfo {
            flags: wow_world_messages::vanilla::MovementInfo_MovementFlags::empty(),
            timestamp: i as u32,
            position: wow_world_messages::vanilla::Vector3d {
                x: 100.0 + i as f32,
                y: 0.0,
                z: 0.0,
            },
            orientation: 0.0,
            fall_time: 0.0,
        };
        let info_bytes = codec::movement_info_to_bytes(&info)
            .expect("a well-formed MovementInfo must serialize");
        let (opcode, body) = codec::build_movement_relay_raw(
            lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT,
            guid,
            &info_bytes,
        )
        .expect("a valid MovementInfo carrier must always produce a relay frame");
        tx.send(Outbound::Raw { opcode, body })
            .expect("writer thread must still be alive mid-burst");
    }
    drop(tx);
    writer
        .join()
        .expect("writer thread must not panic under a movement burst");
    let (monster_moves, heartbeats) = reader.join().expect("reader thread must not panic");

    assert_eq!(
        monster_moves, n,
        "n={n}: lost or gained a typed SMSG_MONSTER_MOVE frame"
    );
    assert_eq!(
        heartbeats, n,
        "n={n}: lost or gained a raw peer-motion relay frame"
    );
}

#[test]
fn writer_thread_survives_movement_bursts_from_100_to_500_movers_without_desync() {
    // 100 is the report's own known-good control; 150/300/500 span and exceed the reported-bad
    // range (150+) and the natural live-validation target (a 300-mage raid, the bug report's own
    // repro line).
    for n in [100usize, 150, 300, 500] {
        movement_burst_over_a_real_cipher(n);
    }
}

/// Crash probe (writer-side black box): [`WriterTrace::record`] must recover the SAME `(opcode,
/// size)` pair for the typed `Outbound::One` path as the wire actually carries — pinned against a
/// real gtker message rather than trusting the `write_unencrypted_server` re-derivation blind.
/// `SMSG_MONSTER_MOVE`'s body length is data-dependent (spline point count), so this also proves
/// the ring doesn't hard-code a size.
#[test]
fn writer_trace_records_the_typed_path_opcode_and_size() {
    let m = codec::build_monster_move(
        0xF130_0000_0000_0001,
        wow_world_messages::vanilla::Vector3d {
            x: -8938.0,
            y: -131.0,
            z: 83.5,
        },
        wow_world_messages::vanilla::Vector3d {
            x: -8900.0,
            y: -100.0,
            z: 82.4,
        },
        500,
        1,
        true,
    );
    let msg = ServerOpcodeMessage::SMSG_MONSTER_MOVE(Box::new(m));
    let mut plain = Vec::new();
    msg.write_unencrypted_server(&mut plain).unwrap();
    let expected_size = u16::from_be_bytes([plain[0], plain[1]]);
    let expected_opcode = u16::from_le_bytes([plain[2], plain[3]]);
    let expected_checksum = fnv1a64(&plain[4..]);

    let mut trace = WriterTrace::new();
    trace.record(&Outbound::One(msg));

    assert_eq!(
        trace.ring.len(),
        1,
        "one Outbound::One must record exactly one frame"
    );
    let entry = trace.ring[0];
    assert_eq!(entry.opcode, expected_opcode);
    assert_eq!(entry.size, expected_size);
    assert_eq!(entry.checksum, expected_checksum);
}

/// A `Batch` of N messages is N frames on the wire (the writer sends each contiguously) — the ring
/// must expand it to N entries, not collapse it to one, or a diff against the harness's per-frame
/// dump would silently misalign after the first batch.
#[test]
fn writer_trace_expands_a_batch_into_one_entry_per_message() {
    let make = |i: u32| {
        let m = codec::build_monster_move(
            0x2000_0000_0000_0000 + i as u64,
            wow_world_messages::vanilla::Vector3d {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            wow_world_messages::vanilla::Vector3d {
                x: 1.0,
                y: 1.0,
                z: 1.0,
            },
            100,
            i + 1,
            false,
        );
        ServerOpcodeMessage::SMSG_MONSTER_MOVE(Box::new(m))
    };
    let mut trace = WriterTrace::new();
    trace.record(&Outbound::Batch(vec![make(0), make(1), make(2)]));
    assert_eq!(
        trace.ring.len(),
        3,
        "a 3-message Batch must expand to 3 traced frames"
    );
}

/// `Outbound::Raw`'s entry must reflect the EXACT body handed to it — `size` is `2 + body.len()`
/// (matching `spawn_writer`'s own framing, not gtker's), and the checksum is over the raw body
/// bytes with no re-serialization in between (Raw never touches gtker's codec).
#[test]
fn writer_trace_records_the_raw_path_verbatim() {
    let body = vec![0xAAu8, 0xBB, 0xCC, 0xDD, 0xEE];
    let mut trace = WriterTrace::new();
    trace.record(&Outbound::Raw {
        opcode: 0x00EE,
        body: body.clone(),
    });
    let entry = trace.ring[0];
    assert_eq!(entry.opcode, 0x00EE);
    assert_eq!(entry.size, 2 + body.len() as u16);
    assert_eq!(entry.checksum, fnv1a64(&body));
}

/// The ring is bounded at [`WriterTrace::CAPACITY`] — a long-lived session must not grow this
/// unboundedly; pushing past capacity drops the OLDEST entry (a FIFO ring, oldest-first on dump).
#[test]
fn writer_trace_ring_caps_at_capacity_and_drops_oldest() {
    let mut trace = WriterTrace::new();
    for i in 0..(WriterTrace::CAPACITY + 5) {
        trace.push(i as u16, 0, 0);
    }
    assert_eq!(trace.ring.len(), WriterTrace::CAPACITY);
    // The oldest 5 pushes (opcodes 0..5) must have been evicted; the ring now starts at 5.
    assert_eq!(trace.ring.front().unwrap().opcode, 5);
    assert_eq!(
        trace.ring.back().unwrap().opcode,
        (WriterTrace::CAPACITY + 4) as u16
    );
}

/// `dump` writes a real file under `/tmp/gw-writer-crash/<account_id>.txt` — the whole point of the
/// black box is that it survives the process exiting the way a real crash does. Uses a distinctive
/// account id to avoid colliding with a real session's dump from a live repro run on the same box.
#[test]
fn writer_trace_dump_writes_a_file_with_the_traced_frames() {
    let mut trace = WriterTrace::new();
    trace.push(0x00EE, 31, 0xDEAD_BEEF_0000_0001);
    trace.push(0x0130, 6, 0xDEAD_BEEF_0000_0002);
    const ACCOUNT: u64 = 0x0FFF_FFFF_F209_0000; // won't collide with a real bench account id
    trace.dump(ACCOUNT, "test-induced dump, not a real session end");

    let path = format!("/tmp/gw-writer-crash/{ACCOUNT}.txt");
    let contents = std::fs::read_to_string(&path).expect("dump must write a readable file");
    assert!(
        contents.contains("opcode=0x00EE"),
        "missing first frame: {contents}"
    );
    assert!(
        contents.contains("opcode=0x0130"),
        "missing second frame: {contents}"
    );
    assert!(
        contents.contains("test-induced dump"),
        "missing the end reason: {contents}"
    );
    let _ = std::fs::remove_file(&path); // leave /tmp clean for a real repro's dumps
}

/// Hardening (the actual fix): an `Outbound::Raw` body that overflows the u16 frame-size
/// field must end the session instead of silently wrapping the declared size — the wrap is exactly
/// the "one wrong size field desyncs every later header" mechanism this whole investigation chased.
/// No current builder produces a body this large (confirmed by reading every `Outbound::Raw` call
/// site), so this exercises the guard directly rather than waiting for a caller to reach it.
///
/// Mutation-checked by hand: reverting this arm to the old `debug_assert!`-only form makes this test
/// FAIL — in `cargo test`'s own debug profile, the reinstated `debug_assert!` fires and panics the
/// `world-writer` thread (caught here as `writer.join()` returning `Err`, not `Ok`). That panic is a
/// debug-profile-only tripwire, though: in the RELEASE profile the capacity benchmark and any live
/// deploy actually run, `debug_assert!` compiles out entirely and the `as u16` cast just wraps
/// silently — a header claiming a tiny size, followed by the full oversized body, corrupting every
/// packet after it. The mutation is caught here either way (panic in debug, corruption in release);
/// this test's own build only exercises the debug-panic half of that story.
#[test]
fn oversized_raw_body_ends_the_session_instead_of_wrapping_the_size_field() {
    let store = std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 42,
            session_key: K,
        }),
        ..Default::default()
    });
    let (mut client, server_end) = UnixStream::pair().unwrap();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        let mut s = server_end;
        let (_conn, encrypt) = world_handshake(&mut s, server_store.as_ref())
            .unwrap()
            .expect("handshake should succeed");
        (s, encrypt)
    });
    let (_c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    let (s, encrypt) = server
        .join()
        .expect("server handshake thread must not panic");

    let (tx, rx, depth) = session_channel();
    let writer = spawn_writer(s, encrypt, rx, depth, 1).expect("spawn the writer thread");

    // 2 (opcode) + this body must exceed u16::MAX to hit the guard.
    let oversized_body = vec![0xABu8; u16::MAX as usize];
    tx.send(Outbound::Raw {
        opcode: 0x00A9,
        body: oversized_body,
    })
    .unwrap();
    drop(tx);

    // The guard must make the writer end the connection cleanly, not hang and not panic.
    writer
        .join()
        .expect("the writer thread must not panic on an oversized raw body");

    // Nothing valid was ever written for this frame: the client sees EOF, not a bogus small-size
    // frame followed by the oversized body. `read_encrypted` reads a 4-byte header first, so with
    // zero bytes on the wire this must fail (EOF), never `Ok`.
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec) {
        Err(_) => {}
        Ok(m) => panic!(
            "expected a clean EOF (nothing sent for the oversized frame), but decoded {m} — the \
             size field wrapped and the client just parsed a corrupt frame as if it were real"
        ),
    }
}

// ===========================================================================================
//  Movement-only load stayed clean at 500 co-located movers (the test above), but the real repro —
//  a mage-boss raid pull, `--cast-spell 133` — crashes at 150 co-located CASTERS. That narrows the
//  corrupt frame to the SPELL/COMBAT relay plane, not movement. This mirrors
//  `movement_burst_over_a_real_cipher` above (the same rig) for the frames
//  that plane actually emits per cast, enumerated by reading every `Outbound::One`/`Outbound::Raw`
//  send site on the cast/combat path (`gateway/src/stdb/subscriptions.rs`'s `on_cast`/`on_combat`
//  listeners, and `world/mod.rs`'s synchronous `CMSG_CAST_SPELL` handler):
//
//    SMSG_SPELL_START            - cast-begin (subscriptions.rs on_cast; world/mod.rs sync ack)
//    SMSG_SPELL_GO               - cast visual/finalize (subscriptions.rs on_cast + on_combat ranged;
//                                   world/mod.rs sync ack)
//    SMSG_SPELLNONMELEEDAMAGELOG - floating spell/ranged damage number (subscriptions.rs on_cast,
//                                   on_combat ranged, on_impact projectile-land)
//    SMSG_SPELL_COOLDOWN         - cooldown swipe, only for spells that have one (subscriptions.rs
//                                   on_cast)
//    SMSG_ATTACKERSTATEUPDATE    - melee swing (subscriptions.rs on_combat, non-ranged non-spell-swing)
//    SMSG_SPELL_FAILURE/SMSG_SPELL_DELAYED - interrupt/pushback signals (subscriptions.rs on_cast) —
//                                   rarer than the per-cast steady state above, not swept here
//    Outbound::Raw{0x0130} SMSG_CAST_RESULT - the caster-only cast ACK (subscriptions.rs on_cast,
//                                   world/mod.rs sync handler). DELIBERATELY EXCLUDED from this rig's
//                                   round-trip below: gtker's own typed `SMSG_CAST_RESULT` decoder has
//                                   the Success/Failure branch condition INVERTED from the real 1.12
//                                   protocol (see `codec::combat::build_cast_result_ok`'s doc comment —
//                                   this is why the gateway sends it via `Outbound::Raw` at all). A
//                                   5-byte OK body (spell_id + 0x00) makes gtker's decoder try to read
//                                   a Success-only "reason" byte that was never sent, which errors —
//                                   correctly, for the real client, but as an artifact of THIS crate's
//                                   own decoder, not of anything the gateway did wrong. Asserting this
//                                   opcode round-trips through `ServerOpcodeMessage::read_encrypted`
//                                   here would just fail on that pre-existing, unrelated gap and add
//                                   noise to this hunt, not signal.
//
//  Same rig as the movement test: real `spawn_writer`, real `wow_srp` cipher pair, a concurrently
//  draining reader (the same deadlock trap applies), interleaved with the SAME movement heartbeat
//  relay the movement test above already pinned clean — because the real repro is combat+cast ON
//  TOP OF movement, not combat alone.
// ===========================================================================================

/// Push `n` casters' worth of one full per-cast sequence (SPELL_START → SPELL_GO →
/// SPELLNONMELEEDAMAGELOG → SPELL_COOLDOWN, mirroring a single boss-target Fireball rotation) PLUS one
/// boss-swing ATTACKERSTATEUPDATE and one peer-motion heartbeat, through a real `spawn_writer` + real
/// cipher pair, then decode every frame back and demand: no decode error, and exactly `n` of each
/// opcode, in order. A stream desync from a wrong size field on ANY of these opcodes would manifest
/// here as a decode `Err` or a wrong-variant decode — precisely the crash shape this file guards
/// against, but attributable to the SPELL/COMBAT plane instead of movement.
fn combat_cast_burst_over_a_real_cipher(n: usize) {
    let store = std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 42,
            session_key: K,
        }),
        ..Default::default()
    });
    let (mut client, server_end) = UnixStream::pair().unwrap();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        let mut s = server_end;
        let (_conn, encrypt) = world_handshake(&mut s, server_store.as_ref())
            .unwrap()
            .expect("handshake should succeed");
        (s, encrypt)
    });
    let (_c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    let (s, encrypt) = server
        .join()
        .expect("server handshake thread must not panic");

    let (tx, rx, depth) = session_channel();
    let writer = spawn_writer(s, encrypt, rx, depth, 1).expect("spawn the writer thread");

    const SPELL_ID: u32 = 133; // Fireball — the crash repro's own `--cast-spell 133`
    const BOSS_GUID: u64 = 0xF130_0000_0000_0001; // one shared raid target, mirrors raid-mode BENCH_ARGS
    const FRAMES_PER_CASTER: usize = 6; // START + GO + DAMAGE_LOG + COOLDOWN + ATTACKERSTATEUPDATE + heartbeat

    // The reader MUST run concurrently with the writer — see `movement_burst_over_a_real_cipher`'s
    // comment on the exact same deadlock this rig hit empirically at n=500 there.
    let reader = std::thread::spawn(move || {
        let mut starts = 0usize;
        let mut goes = 0usize;
        let mut damage_logs = 0usize;
        let mut cooldowns = 0usize;
        let mut swings = 0usize;
        let mut heartbeats = 0usize;
        for _ in 0..(FRAMES_PER_CASTER * n) {
            match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec) {
                Ok(ServerOpcodeMessage::SMSG_SPELL_START(_)) => starts += 1,
                Ok(ServerOpcodeMessage::SMSG_SPELL_GO(_)) => goes += 1,
                Ok(ServerOpcodeMessage::SMSG_SPELLNONMELEEDAMAGELOG(_)) => damage_logs += 1,
                Ok(ServerOpcodeMessage::SMSG_SPELL_COOLDOWN(_)) => cooldowns += 1,
                Ok(ServerOpcodeMessage::SMSG_ATTACKERSTATEUPDATE(_)) => swings += 1,
                Ok(ServerOpcodeMessage::MSG_MOVE_HEARTBEAT(_)) => heartbeats += 1,
                Ok(other) => panic!(
                    "n={n}: decoded an unexpected opcode after {starts} starts + {goes} goes + \
                     {damage_logs} damage-logs + {cooldowns} cooldowns + {swings} swings + \
                     {heartbeats} heartbeats — this IS a stream desync, on the SPELL/COMBAT plane: \
                     {other}"
                ),
                Err(e) => panic!(
                    "n={n}: decode failed after {starts} starts + {goes} goes + {damage_logs} \
                     damage-logs + {cooldowns} cooldowns + {swings} swings + {heartbeats} \
                     heartbeats — exactly the crash shape this file guards against, on the \
                     SPELL/COMBAT plane: {e}"
                ),
            }
        }
        (starts, goes, damage_logs, cooldowns, swings, heartbeats)
    });

    for i in 0..n {
        let caster = 0x1000_0000_0000_0000u64 + i as u64;
        let damage = 40 + (i as u32 % 60); // varies the body, like a real crit/resist spread would

        let start = codec::build_spell_start(caster, SPELL_ID, 0, 0, None);
        tx.send(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(
            Box::new(start),
        )))
        .expect("writer thread must still be alive mid-burst");

        let go = codec::build_spell_go(caster, SPELL_ID, BOSS_GUID, None);
        tx.send(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_GO(Box::new(
            go,
        ))))
        .expect("writer thread must still be alive mid-burst");

        let log = codec::build_spell_non_melee_damage_log(
            BOSS_GUID,
            caster,
            SPELL_ID,
            damage,
            2, /* fire */
            i % 5 == 0,
            0,
            0,
        );
        tx.send(Outbound::One(
            ServerOpcodeMessage::SMSG_SPELLNONMELEEDAMAGELOG(Box::new(log)),
        ))
        .expect("writer thread must still be alive mid-burst");

        let cd = codec::build_spell_cooldown(caster, SPELL_ID, 1500);
        tx.send(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_COOLDOWN(
            Box::new(cd),
        )))
        .expect("writer thread must still be alive mid-burst");

        // The boss swinging back at whichever caster it's leashed onto this tick — real raid load is
        // casts AND incoming melee at once, not casts in isolation.
        let swing = codec::build_attacker_state_update(BOSS_GUID, caster, damage, 0, 0, 0);
        tx.send(Outbound::One(
            ServerOpcodeMessage::SMSG_ATTACKERSTATEUPDATE(Box::new(swing)),
        ))
        .expect("writer thread must still be alive mid-burst");

        let info = MovementInfo {
            flags: wow_world_messages::vanilla::MovementInfo_MovementFlags::empty(),
            timestamp: i as u32,
            position: wow_world_messages::vanilla::Vector3d {
                x: -8938.0 + i as f32,
                y: -131.0,
                z: 83.5,
            },
            orientation: 0.0,
            fall_time: 0.0,
        };
        let info_bytes = codec::movement_info_to_bytes(&info)
            .expect("a well-formed MovementInfo must serialize");
        let (opcode, body) = codec::build_movement_relay_raw(
            lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT,
            caster,
            &info_bytes,
        )
        .expect("a valid MovementInfo carrier must always produce a relay frame");
        tx.send(Outbound::Raw { opcode, body })
            .expect("writer thread must still be alive mid-burst");
    }
    drop(tx);
    writer
        .join()
        .expect("writer thread must not panic under a combat+cast burst");
    let (starts, goes, damage_logs, cooldowns, swings, heartbeats) =
        reader.join().expect("reader thread must not panic");

    assert_eq!(starts, n, "n={n}: lost or gained a SMSG_SPELL_START frame");
    assert_eq!(goes, n, "n={n}: lost or gained a SMSG_SPELL_GO frame");
    assert_eq!(
        damage_logs, n,
        "n={n}: lost or gained a SMSG_SPELLNONMELEEDAMAGELOG frame"
    );
    assert_eq!(
        cooldowns, n,
        "n={n}: lost or gained a SMSG_SPELL_COOLDOWN frame"
    );
    assert_eq!(
        swings, n,
        "n={n}: lost or gained a SMSG_ATTACKERSTATEUPDATE frame"
    );
    assert_eq!(
        heartbeats, n,
        "n={n}: lost or gained a peer-motion relay frame"
    );
}

#[test]
fn writer_thread_survives_combat_cast_bursts_from_100_to_300_casters_without_desync() {
    // 100 = clean per the movement test's own control range; 150 = the discriminator's OWN reported
    // crash threshold ("crashes at 150" combat+cast, vs. clean at 500 movement-only); 200/300 span and
    // exceed it (300 is the natural raid-repro target — `BENCH_ARGS="--class mage --boss <hogger>
    // --cast-spell 133"`, a 300-mage raid). If this ever goes red, the failure message above names
    // the exact opcode and frame-count offset where the SPELL/COMBAT plane desynced — the root
    // cause, headlessly. If it stays green through all four, the corrupt frame is NOT produced by any
    // of these send paths, and the next probe is the byte-level capture (the wire harness's
    // `/tmp/wire-crash/` dump) taken at the moment of a LIVE repro.
    for n in [100usize, 150, 200, 300] {
        combat_cast_burst_over_a_real_cipher(n);
    }
}
