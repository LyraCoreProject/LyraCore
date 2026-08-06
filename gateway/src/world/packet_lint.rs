//! Outbound packet lint (testing-hardening §3.2): the 5875 client's ROOT-CAUSED crash/wedge
//! classes are byte-recognizable, so the raw-frame chokepoint checks every hand-rolled body
//! against them and logs a loud `packet-lint VIOLATION` line — turning each hard-won client-crash
//! diagnosis (danger-zones §1) into a permanent regression wall the suite can grep for.
//!
//! Always on: it inspects only `Outbound::Raw` frames, and of those only the two opcodes with a
//! known crash class — a handful of branch instructions next to the header encryption.
//! The TYPED path's equivalent guard lives at TEST time instead (`codec::values` serializes every
//! `build_*_values` builder and asserts the mask never carries bit 2) — gtker's builders can't be
//! cheaply re-parsed per send, and `player_values`' dirty_reset is the single producer there.

use lyracore_shared::values_mask::parse_values_updates;

/// The `OBJECT_FIELD_TYPE` descriptor index — re-sending it inside a VALUES partial overwrites the
/// client's cached object type and null-derefs on the next player-frame dispatch (the 2026-06-16
/// combat crash; `codec::values::build_health_values` carries the full story).
const OBJECT_FIELD_TYPE: u16 = 2;

/// SMSG_UPDATE_OBJECT / SMSG_SET_FACTION_STANDING raw opcodes (the two linted frames).
const UPDATE_OBJECT: u16 = 0x00A9;
const SET_FACTION_STANDING: u16 = 0x0124;

/// Lint one raw outbound frame. Returns one message per violation (empty = clean).
pub fn lint_raw(opcode: u16, body: &[u8]) -> Vec<&'static str> {
    let mut hits = Vec::new();
    match opcode {
        UPDATE_OBJECT => {
            // Only VALUES blocks are parsed (the decoder stops at a CREATE block, which
            // legitimately carries TYPE in its full mask) — so any bit-2 hit here is a real
            // partial-update violation.
            for u in parse_values_updates(body) {
                if u.fields.iter().any(|&(idx, _)| idx == OBJECT_FIELD_TYPE) {
                    hits.push(
                        "OBJECT_FIELD_TYPE re-sent in a VALUES partial (5875 null+0x110 crash class — dirty_reset missing?)",
                    );
                }
            }
        }
        SET_FACTION_STANDING => {
            // Layout (build_set_faction_standing_raw): count u32, then per faction
            // reputation_index u32 + standing u32. An index >= 64 addresses past the client's
            // fixed rep array — the McBride ERROR #132 crash class.
            if body.len() >= 4 {
                let count = u32::from_le_bytes(body[0..4].try_into().unwrap()) as usize;
                for i in 0..count {
                    let off = 4 + i * 8;
                    let Some(idx_bytes) = body.get(off..off + 4) else {
                        break;
                    };
                    let idx = u32::from_le_bytes(idx_bytes.try_into().unwrap());
                    if idx >= 64 {
                        hits.push(
                            "SET_FACTION_STANDING reputation index >= 64 (past the client's rep array — ERROR #132 crash class)",
                        );
                    }
                }
            }
        }
        _ => {}
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values_frame(fields: &[(u16, u32)]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&1u32.to_le_bytes());
        b.push(0); // has_transport
        b.push(0); // VALUES
        b.push(0x01); // packed guid: 1 low byte
        b.push(0x09);
        let max_block = fields.iter().map(|(f, _)| f / 32).max().unwrap() as u8 + 1;
        b.push(max_block);
        let mut blocks = vec![0u32; max_block as usize];
        for &(f, _) in fields {
            blocks[(f / 32) as usize] |= 1 << (f % 32);
        }
        for w in &blocks {
            b.extend_from_slice(&w.to_le_bytes());
        }
        let mut sorted: Vec<_> = fields.to_vec();
        sorted.sort_by_key(|&(f, _)| f);
        for (_, v) in sorted {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b
    }

    #[test]
    fn type_bit_in_a_values_partial_is_flagged() {
        // The exact 2026-06-16 crash shape: TYPE (2) re-sent next to a health write (22).
        let bad = values_frame(&[(2, 0x9), (22, 41)]);
        assert_eq!(lint_raw(0x00A9, &bad).len(), 1);
        // The correct partial — health alone — is clean.
        let good = values_frame(&[(22, 41)]);
        assert!(lint_raw(0x00A9, &good).is_empty());
    }

    #[test]
    fn faction_standing_index_past_the_rep_array_is_flagged() {
        let mut bad = Vec::new();
        bad.extend_from_slice(&1u32.to_le_bytes());
        bad.extend_from_slice(&72u32.to_le_bytes()); // the McBride bug: faction ID as index
        bad.extend_from_slice(&3175u32.to_le_bytes());
        assert_eq!(lint_raw(0x0124, &bad).len(), 1);
        let mut good = bad.clone();
        good[4..8].copy_from_slice(&19u32.to_le_bytes()); // Stormwind's real rep-index
        assert!(lint_raw(0x0124, &good).is_empty());
    }

    #[test]
    fn other_opcodes_are_never_inspected() {
        assert!(lint_raw(0x0160, &[0xFF; 32]).is_empty());
    }
}
