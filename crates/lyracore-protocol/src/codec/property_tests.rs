use super::*;

use lyracore_test_support::Rng;

const CASES: usize = 2_000;

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
        let _ = addon::parse_addon_client_chat(&body);
    }
}

fn addon_shaped(rng: &mut Rng, tail_len: usize) -> Vec<u8> {
    let mut body = Vec::with_capacity(9 + tail_len);
    let chat_type = [0u32, 1, 6, 14][rng.below(4)];
    body.extend_from_slice(&chat_type.to_le_bytes());
    body.extend_from_slice(&addon::LANG_ADDON.to_le_bytes());
    body.extend_from_slice(&rng.bytes(tail_len));
    body
}

#[test]
fn every_truncation_of_a_well_formed_addon_frame_decodes_or_declines_but_never_panics() {
    let mut full = Vec::new();
    full.extend_from_slice(&6u32.to_le_bytes());
    full.extend_from_slice(&addon::LANG_ADDON.to_le_bytes());
    full.extend_from_slice(b"TargetName\0");
    full.extend_from_slice(b"STC|1|1|PING|payload\0");

    assert_eq!(
        addon::parse_addon_client_chat(&full),
        Some((6, "STC|1|1|PING|payload".to_string())),
        "the fixture stopped being a valid addon whisper — the truncation sweep below would then \
         prove nothing"
    );

    for cut in 0..=full.len() {
        let _ = addon::parse_addon_client_chat(&full[..cut]);
    }
}

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
            assert!(
                text.starts_with(addon::BRIDGE_PREFIX),
                "accepted a non-{} envelope {text:?} (cmd {cmd:?})",
                addon::BRIDGE_PREFIX
            );
        }
    }
}

#[test]
fn the_movement_carrier_decoder_never_panics_on_any_byte_string() {
    let mut rng = Rng::new(0x4D4F_5645);
    for _ in 0..CASES {
        let len = rng.below(96);
        let _ = super::movement_info::bytes_to_movement_info(&rng.bytes(len));
    }
}

#[test]
fn oversized_movement_bodies_are_refused() {
    for len in [65_531usize, 65_532, 65_535, 70_000] {
        let body = vec![0u8; len];
        let out = super::movement_info::bytes_to_movement_info(&body);
        assert!(
            out.is_err(),
            "a {len}-byte movement body cannot be re-framed under a u16 size field; it must be \
             rejected rather than silently wrapping the length"
        );
    }
}

#[test]
fn every_generated_update_mask_round_trips_through_the_raw_values_decoder() {
    let mut rng = Rng::new(0x5641_4C55);
    for _ in 0..CASES {
        let guid = rng.next_u64();
        let n_fields = 1 + rng.below(24);
        let mut mask = update_mask::UpdateMaskValues::new();
        let mut expected: std::collections::BTreeMap<u16, u32> = Default::default();
        for _ in 0..n_fields {
            let mut idx = rng.below(1200) as u16;
            if idx == update_mask::idx::OBJECT_TYPE {
                idx += 1;
            }
            let value = rng.next_u32();
            mask.set_u32(idx, value);
            expected.insert(idx, value);
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

#[test]
fn the_outbound_frame_linter_never_panics_on_any_body() {
    let mut rng = Rng::new(0x4C49_4E54);
    let opcodes = [0x00A9u16, 0x0124, 0x0037];
    for _ in 0..CASES {
        let opcode = opcodes[rng.below(opcodes.len())];
        let len = rng.below(128);
        let _ = crate::packet_lint::lint_raw(opcode, &rng.bytes(len));
    }
}

#[test]
fn the_seeded_generator_is_reproducible_and_seed_dependent() {
    let a: Vec<u8> = Rng::new(7).bytes(64);
    let b: Vec<u8> = Rng::new(7).bytes(64);
    assert_eq!(a, b, "the same seed must replay the same byte stream");

    let c: Vec<u8> = Rng::new(8).bytes(64);
    assert_ne!(a, c, "different seeds must explore different inputs");

    assert!(
        a.windows(2).any(|w| w[0] != w[1]),
        "the generator emitted a constant stream: {a:02X?}"
    );
}
