use lyracore_shared::values_mask::parse_values_updates;

const OBJECT_FIELD_TYPE: u16 = 2;

const UPDATE_OBJECT: u16 = 0x00A9;
const SET_FACTION_STANDING: u16 = 0x0124;

pub fn lint_raw(opcode: u16, body: &[u8]) -> Vec<&'static str> {
    let mut hits = Vec::new();
    match opcode {
        UPDATE_OBJECT => {
            for u in parse_values_updates(body) {
                if u.fields.iter().any(|&(idx, _)| idx == OBJECT_FIELD_TYPE) {
                    hits.push(
                        "OBJECT_FIELD_TYPE re-sent in a VALUES partial (5875 null+0x110 crash class — dirty_reset missing?)",
                    );
                }
            }
        }
        // Faction-standing slots are u32 indices into a fixed 64-entry client array.
        SET_FACTION_STANDING if body.len() >= 4 => {
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
        b.push(0);
        b.push(0);
        b.push(0x01);
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
        let bad = values_frame(&[(2, 0x9), (22, 41)]);
        assert_eq!(lint_raw(0x00A9, &bad).len(), 1);
        let good = values_frame(&[(22, 41)]);
        assert!(lint_raw(0x00A9, &good).is_empty());
    }

    #[test]
    fn faction_standing_index_past_the_rep_array_is_flagged() {
        let mut bad = Vec::new();
        bad.extend_from_slice(&1u32.to_le_bytes());
        bad.extend_from_slice(&72u32.to_le_bytes());
        bad.extend_from_slice(&3175u32.to_le_bytes());
        assert_eq!(lint_raw(0x0124, &bad).len(), 1);
        let mut good = bad.clone();
        good[4..8].copy_from_slice(&19u32.to_le_bytes());
        assert!(lint_raw(0x0124, &good).is_empty());
    }

    #[test]
    fn other_opcodes_are_never_inspected() {
        assert!(lint_raw(0x0160, &[0xFF; 32]).is_empty());
    }
}
