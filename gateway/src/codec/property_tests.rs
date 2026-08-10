//! Property tests for the hand-rolled decoders: the client-byte parsers and the
//! update-mask codec.
//!
//! WHY NOT `proptest`. These are property tests in the sense that matters (a generated input space
//! rather than a hand-picked vector), but they are driven by the fixed-seed generator below instead
//! of a proptest/quickcheck dependency. Three reasons, in order:
//!
//!   * DETERMINISM IS THE POINT. A shrinking fuzzer finds a new counterexample on some later run,
//!     which is a flaky test by construction — exactly what this workspace's test policy forbids.
//!     Every run here explores the same inputs and either always passes or always fails, and a
//!     failure names the seed that produced it.
//!   * The workspace has no `[dev-dependencies]` beyond the two `schema_parity.rs` needs, and no
//!     proptest anywhere in `Cargo.lock`. Adding one to assert "does not panic" would buy shrinking
//!     and cost a dependency on the release-critical crate.
//!   * The properties here are total ("never panics", "round-trips", "decodes no more than was
//!     encoded"), so shrinking has little to offer: any failing input is already 8-200 bytes.
//!
//! WHAT IS ACTUALLY BEING ASSERTED. Every parser in this file is reachable from bytes the server
//! does not control, and each returns `Option`/`Result`/an empty `Vec` rather than panicking. That
//! is a deliberate design property (`main.rs`'s `boundary_panic_tripwire` pins its lexical half —
//! that no `.unwrap()` is WRITTEN there). This file pins the behavioural half the scan cannot see:
//! a panic reached through indexing, slicing or arithmetic, which no source scan detects.

use super::*;

/// A fixed-seed xorshift64* — the whole generator, so a "property" run is reproducible byte for
/// byte. Not cryptographic and not meant to be; it only has to spread bits well enough to reach the
/// branch structure of a byte parser.
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        // Any non-zero state; xorshift64* is degenerate at 0.
        Self(seed | 1)
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub(crate) fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// Uniform-enough in `0..n`. `n == 0` is a caller bug and would divide by zero, so it saturates
    /// to 1 rather than panicking inside a generator.
    pub(crate) fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % (n.max(1)) as u64) as usize
    }

    /// `len` pseudo-random bytes.
    pub(crate) fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| (self.next_u64() >> 24) as u8).collect()
    }
}

/// How many generated inputs each property runs. Small enough that the whole file is well under a
/// second (these are all pure functions over ≤256-byte buffers), large enough to cover the branch
/// structure many times over.
const CASES: usize = 2_000;

// ===========================================================================================
//  The addon chat decoder — the only hand-rolled parser of CLIENT bytes in `codec/`
// ===========================================================================================

/// `parse_addon_client_chat` walks a client-supplied buffer looking for NUL terminators and skips a
/// leading CString for whisper/channel frames. All of that is slice math driven by remote input.
///
/// The property: for ANY byte string, it returns `Option` — it never panics, and it never reads
/// past the buffer. Random bytes reach the `Some` arm rarely, so the generator deliberately biases
/// toward well-shaped headers (see `addon_shaped`) as well as pure noise.
#[test]
fn the_addon_chat_decoder_never_panics_on_any_byte_string() {
    let mut rng = Rng::new(0x2023_0223);
    for case in 0..CASES {
        let len = rng.below(200);
        let body = if case % 2 == 0 {
            rng.bytes(len)
        } else {
            addon_shaped(&mut rng, len)
        };
        // The assertion is that this returns at all. A returned `Some` must additionally be a
        // string the caller can use, which `String` already guarantees.
        let _ = addon::parse_addon_client_chat(&body);
    }
}

/// A body with a plausible addon header (chat type + `LANG_ADDON`) and a random tail, so the
/// generator reaches the NUL-scanning and CString-skipping branches instead of bailing at the
/// language check on nearly every case.
fn addon_shaped(rng: &mut Rng, tail_len: usize) -> Vec<u8> {
    let mut body = Vec::with_capacity(9 + tail_len);
    // Chat types 6 (WHISPER) and 14 (CHANNEL) take the extra CString-skip branch.
    let chat_type = [0u32, 1, 6, 14][rng.below(4)];
    body.extend_from_slice(&chat_type.to_le_bytes());
    body.extend_from_slice(&addon::LANG_ADDON.to_le_bytes());
    body.extend_from_slice(&rng.bytes(tail_len));
    body
}

/// Truncation is the shape a severed or mis-sized frame actually takes, and it is the one this
/// parser's slice math is most exposed to: every prefix of a VALID frame is tried.
#[test]
fn every_truncation_of_a_well_formed_addon_frame_decodes_or_declines_but_never_panics() {
    let mut full = Vec::new();
    full.extend_from_slice(&6u32.to_le_bytes()); // CHAT_TYPE_WHISPER — takes the CString skip
    full.extend_from_slice(&addon::LANG_ADDON.to_le_bytes());
    full.extend_from_slice(b"TargetName\0");
    full.extend_from_slice(b"STC|1|1|PING|payload\0");

    // Sanity floor: the untruncated frame must actually parse, or this sweep is vacuous.
    assert_eq!(
        addon::parse_addon_client_chat(&full).as_deref(),
        Some("STC|1|1|PING|payload"),
        "the fixture stopped being a valid addon whisper — the truncation sweep below would then \
         prove nothing"
    );

    for cut in 0..=full.len() {
        let _ = addon::parse_addon_client_chat(&full[..cut]);
    }
}

/// The `STC` envelope splitter runs on an already-decoded String, i.e. still on remote content. It
/// splits on `|` and indexes the resulting fields; a short envelope must decline, not index out of
/// bounds.
#[test]
fn the_bridge_envelope_splitter_never_panics_and_only_accepts_the_stc_shape() {
    let mut rng = Rng::new(0x5743_0050);
    let alphabet = b"STC|0123456789ABCxyz\\\"' \0\n";
    for _ in 0..CASES {
        let len = rng.below(64);
        let text: String = (0..len)
            .map(|_| alphabet[rng.below(alphabet.len())] as char)
            .collect();
        if let Some((cmd, _payload)) = addon::parse_bridge_envelope(&text) {
            // The one content invariant: an accepted envelope really was an `STC` one. If this ever
            // fires, the bridge would be forwarding another server's addon traffic into our
            // `client_command` reducer.
            assert!(
                text.starts_with(addon::BRIDGE_PREFIX),
                "accepted a non-{} envelope {text:?} (cmd {cmd:?})",
                addon::BRIDGE_PREFIX
            );
        }
    }
}

// ===========================================================================================
//  The movement-info carrier
// ===========================================================================================

/// `bytes_to_movement_info` re-decodes a raw movement body by wrapping it back into a synthetic
/// frame — arithmetic on a remote-supplied length. It must return `Result` for every input.
#[test]
fn the_movement_carrier_decoder_never_panics_on_any_byte_string() {
    let mut rng = Rng::new(0x4D4F_5645);
    for _ in 0..CASES {
        let len = rng.below(96);
        let _ = movement::bytes_to_movement_info(&rng.bytes(len));
    }
}

/// The one arithmetic edge in that function: it prepends a 4-byte header, so a body within 4 bytes
/// of `u16::MAX` overflows the frame-size field. That must be an `Err`, not a wrap — a wrapped size
/// is the "one wrong size field desyncs every later header" failure this codebase has already paid
/// for once on the outbound side (the `SMSG_COMPRESSED_MOVES` crowd-scale corruption).
#[test]
fn a_movement_body_that_would_overflow_the_frame_size_is_an_error_not_a_wrap() {
    for len in [65_531usize, 65_532, 65_535, 70_000] {
        let body = vec![0u8; len];
        let out = movement::bytes_to_movement_info(&body);
        assert!(
            out.is_err(),
            "a {len}-byte movement body cannot be re-framed under a u16 size field; it must be \
             rejected rather than silently wrapping the length"
        );
    }
}

// ===========================================================================================
//  The update-mask codec — encode/decode round trip
// ===========================================================================================

/// The core property of the hand-rolled mask: whatever `UpdateMaskValues` encodes,
/// `lyracore_shared::values_mask::parse_values_updates` decodes back EXACTLY — same guid, same
/// `(field_index, word)` set, in ascending index order.
///
/// This matters because the two halves are written independently and neither can be checked against
/// gtker: gtker's typed reader REJECTS the TYPE-less partial mask a correct 5875 server sends, which
/// is the whole reason both of these exist. The round trip is the only oracle available offline, and
/// it is a real one — an off-by-one in the block count, the bit order, or the ascending-value order
/// breaks it immediately.
///
/// `OBJECT_FIELD_TYPE` (index 2) is excluded by the generator, because setting it on a partial
/// VALUES update is the 5875 null+0x110 client crash and `build_values_update_raw` `debug_assert!`s
/// against it.
#[test]
fn every_generated_update_mask_round_trips_through_the_raw_values_decoder() {
    let mut rng = Rng::new(0x5641_4C55);
    for _ in 0..CASES {
        let guid = rng.next_u64();
        let n_fields = 1 + rng.below(24);
        let mut mask = update_mask::UpdateMaskValues::new();
        let mut expected: std::collections::BTreeMap<u16, u32> = Default::default();
        for _ in 0..n_fields {
            // 0..1200 spans the real descriptor range (PLAYER_FIELD_COINAGE = 1176) and keeps the
            // block count inside the u8 the wire format allots it.
            let mut idx = rng.below(1200) as u16;
            if idx == update_mask::idx::OBJECT_TYPE {
                idx += 1;
            }
            let value = rng.next_u32();
            mask.set_u32(idx, value);
            expected.insert(idx, value); // last write wins, exactly like the map inside the mask
        }

        let (opcode, body) = values::build_values_update_raw(guid, &mask);
        assert_eq!(
            opcode, 0x00A9,
            "the raw VALUES builder must keep emitting SMSG_UPDATE_OBJECT"
        );

        let decoded = lyracore_shared::values_mask::parse_values_updates(&body);
        assert_eq!(
            decoded.len(),
            1,
            "one encoded object must decode to exactly one VALUES block; body was {body:02X?}"
        );
        assert_eq!(
            decoded[0].guid, guid,
            "the packed guid did not survive the round trip"
        );
        let want: Vec<(u16, u32)> = expected.into_iter().collect();
        assert_eq!(
            decoded[0].fields, want,
            "the decoded field set differs from what was encoded (guid {guid:#X})"
        );
    }
}

/// A decoder for frames off a socket has to survive a frame that stops early. The property is
/// stronger than "does not panic": a truncated frame may decode FEWER fields than were encoded, but
/// it must never invent one — a phantom `(index, value)` pair would make a packet-lint assertion
/// fire on a field the server never sent.
#[test]
fn every_truncation_of_an_encoded_mask_decodes_a_subset_and_never_invents_a_field() {
    let mut rng = Rng::new(0x5452_554E);
    for _ in 0..200 {
        let guid = rng.next_u64();
        let mut mask = update_mask::UpdateMaskValues::new();
        let mut encoded: Vec<(u16, u32)> = Vec::new();
        for _ in 0..(1 + rng.below(8)) {
            let mut idx = rng.below(400) as u16;
            if idx == update_mask::idx::OBJECT_TYPE {
                idx += 1;
            }
            let value = rng.next_u32();
            mask.set_u32(idx, value);
            encoded.push((idx, value));
        }
        let (_, body) = values::build_values_update_raw(guid, &mask);

        for cut in 0..=body.len() {
            let decoded = lyracore_shared::values_mask::parse_values_updates(&body[..cut]);
            assert!(
                decoded.len() <= 1,
                "a single-object frame truncated to {cut} bytes decoded {} objects",
                decoded.len()
            );
            if let Some(v) = decoded.first() {
                for (idx, value) in &v.fields {
                    assert!(
                        encoded.contains(&(*idx, *value)),
                        "truncating to {cut} bytes produced field ({idx}, {value:#X}) that was \
                         never encoded"
                    );
                }
            }
        }
    }
}

// ===========================================================================================
//  The outbound frame linter — it parses our OWN bytes, but with the same exposure
// ===========================================================================================

/// `lint_raw` re-parses an already-built frame to catch the two crash-class mistakes. It runs on
/// every raw send in debug builds, so a panic in it would take down a session over a diagnostic.
/// The property: any `(opcode, body)` pair returns a `Vec` of violations, never an unwind.
#[test]
fn the_outbound_frame_linter_never_panics_on_any_body() {
    let mut rng = Rng::new(0x4C49_4E54);
    // The two opcodes it actually inspects, plus one it must pass through untouched.
    let opcodes = [0x00A9u16, 0x0124, 0x0037];
    for _ in 0..CASES {
        let opcode = opcodes[rng.below(opcodes.len())];
        let len = rng.below(128);
        let _ = crate::world::packet_lint::lint_raw(opcode, &rng.bytes(len));
    }
}

/// The generator itself has to be trustworthy, or every "never panics" above could be passing on a
/// stream of identical bytes. Two properties: a fixed seed reproduces exactly (that is what makes
/// these tests deterministic), and different seeds diverge (that is what makes them a search).
#[test]
fn the_seeded_generator_is_reproducible_and_seed_dependent() {
    let a: Vec<u8> = Rng::new(7).bytes(64);
    let b: Vec<u8> = Rng::new(7).bytes(64);
    assert_eq!(a, b, "the same seed must replay the same byte stream");

    let c: Vec<u8> = Rng::new(8).bytes(64);
    assert_ne!(a, c, "different seeds must explore different inputs");

    // Not a stuck value: 64 draws from a working generator are not all equal.
    assert!(
        a.windows(2).any(|w| w[0] != w[1]),
        "the generator emitted a constant stream: {a:02X?}"
    );
}
