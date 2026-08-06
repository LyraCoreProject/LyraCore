//! Raw decoder for `SMSG_UPDATE_OBJECT` VALUES blocks (testing-hardening §3.1).
//!
//! gtker's typed reader REJECTS a TYPE-less partial VALUES mask (bit 2 absent — which is exactly
//! what a correct 5875 server sends; see `codec::values`'s dirty_reset doc), so every wire assert
//! on a live field change used to be a bespoke `body.windows(4)` byte scan. This is the one shared
//! decoder instead: parse the leading VALUES objects out of a raw frame and hand back
//! `(field_index, raw_word)` pairs addressed exactly like the update-mask descriptor table
//! (e.g. `PLAYER_SKILL_INFO` starts at field 718, 3 words per slot; `UNIT_FIELD_HEALTH` = 22).
//!
//! Scope: VALUES blocks only, and only the LEADING run of them — a CREATE/MOVEMENT block has a
//! variable-length movement payload this deliberately does not decode (the typed reader handles
//! full CREATEs fine; VALUES is the gap). Frames that interleave a CREATE before a VALUES lose the
//! tail — acceptable for asserts, which watch many frames.

/// One decoded VALUES block: which entity, and every `(field_index, raw_word)` the mask carried.
#[derive(Debug, Clone, PartialEq)]
pub struct ValuesUpdate {
    pub guid: u64,
    pub fields: Vec<(u16, u32)>,
}

/// Parse the leading VALUES objects from a raw `SMSG_UPDATE_OBJECT` body. Malformed/truncated
/// input returns what was decoded so far (asserts treat "nothing matched" as the failure signal —
/// a parser panic would kill the whole probe on the first exotic frame).
pub fn parse_values_updates(body: &[u8]) -> Vec<ValuesUpdate> {
    let mut out = Vec::new();
    let mut r = Reader { b: body, i: 0 };
    let Some(count) = r.u32() else { return out };
    if r.u8().is_none() {
        return out; // has_transport
    }
    for _ in 0..count {
        match r.u8() {
            Some(0) => {} // UPDATE_TYPE_VALUES — the only block this decodes
            _ => break,   // CREATE/MOVEMENT/OOR: variable-length payloads we don't skip
        }
        let Some(guid) = r.packed_guid() else { break };
        let Some(n_blocks) = r.u8() else { break };
        let mut mask = Vec::with_capacity(n_blocks as usize);
        for _ in 0..n_blocks {
            match r.u32() {
                Some(w) => mask.push(w),
                None => return out,
            }
        }
        let mut fields = Vec::new();
        for (block_i, &word) in mask.iter().enumerate() {
            for bit in 0..32u16 {
                if word & (1 << bit) != 0 {
                    let Some(v) = r.u32() else { return out };
                    fields.push((block_i as u16 * 32 + bit, v));
                }
            }
        }
        out.push(ValuesUpdate { guid, fields });
    }
    out
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.b.get(self.i)?;
        self.i += 1;
        Some(v)
    }
    fn u32(&mut self) -> Option<u32> {
        let s = self.b.get(self.i..self.i + 4)?;
        self.i += 4;
        Some(u32::from_le_bytes(s.try_into().unwrap()))
    }
    fn packed_guid(&mut self) -> Option<u64> {
        let mask = self.u8()?;
        let mut guid = 0u64;
        for byte_i in 0..8 {
            if mask & (1 << byte_i) != 0 {
                guid |= (self.u8()? as u64) << (8 * byte_i);
            }
        }
        Some(guid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-assemble a VALUES frame: one object, guid 9, field 718 = 356 (a Fishing skill word)
    /// and field 22 = 41 (UNIT_FIELD_HEALTH) — two mask blocks apart, exercising block indexing.
    fn frame(fields: &[(u16, u32)], guid: u64) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&1u32.to_le_bytes()); // count
        b.push(0); // has_transport
        b.push(0); // UPDATE_TYPE_VALUES
                   // packed guid: low bytes only
        let gb = guid.to_le_bytes();
        let mut mask = 0u8;
        let mut bytes = Vec::new();
        for (i, &x) in gb.iter().enumerate() {
            if x != 0 {
                mask |= 1 << i;
                bytes.push(x);
            }
        }
        b.push(mask);
        b.extend_from_slice(&bytes);
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
    fn decodes_fields_across_mask_blocks_with_the_right_indices() {
        let body = frame(&[(718, 356), (22, 41)], 0x9);
        let ups = parse_values_updates(&body);
        assert_eq!(ups.len(), 1);
        assert_eq!(ups[0].guid, 0x9);
        assert_eq!(
            ups[0].fields,
            vec![(22, 41), (718, 356)],
            "ascending field order"
        );
    }

    #[test]
    fn decodes_a_high_guid_packed_form() {
        let body = frame(&[(1102, 5)], 0xF130_0000_0000_0009);
        let ups = parse_values_updates(&body);
        assert_eq!(ups[0].guid, 0xF130_0000_0000_0009);
        assert_eq!(ups[0].fields, vec![(1102, 5)]);
    }

    #[test]
    fn truncated_or_foreign_blocks_never_panic() {
        let body = frame(&[(22, 41)], 0x9);
        for cut in 0..body.len() {
            let _ = parse_values_updates(&body[..cut]); // must not panic at any truncation
        }
        // A CREATE-typed first block yields nothing (we don't skip movement payloads).
        let mut create = body.clone();
        create[5] = 2; // UPDATE_TYPE_CREATE_OBJECT
        assert!(parse_values_updates(&create).is_empty());
    }
}
