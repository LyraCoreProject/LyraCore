//! gtker exposes only slot 0 of descriptor arrays and rejects type-less VALUES when reading.

use std::collections::BTreeMap;

#[allow(dead_code)]
pub mod idx {
    pub const OBJECT_GUID: u16 = 0;
    /// Never resend OBJECT_FIELD_TYPE in partial VALUES; it can strip PLAYER and crash build 5875.
    pub const OBJECT_TYPE: u16 = 2;
    pub const OBJECT_ENTRY: u16 = 3;
    pub const OBJECT_SCALE_X: u16 = 4;

    pub const UNIT_HEALTH: u16 = 22;
    pub const UNIT_POWER1: u16 = 23;
    pub const UNIT_MAXHEALTH: u16 = 28;
    pub const UNIT_MAXPOWER1: u16 = 29;
    pub const UNIT_LEVEL: u16 = 34;
    pub const UNIT_FACTIONTEMPLATE: u16 = 35;
    /// Packed race, class, gender and power, one byte each.
    pub const UNIT_BYTES_0: u16 = 36;
    pub const UNIT_VIRTUAL_ITEM_SLOT_DISPLAY: u16 = 37;
    pub const UNIT_FLAGS: u16 = 46;

    pub const UNIT_AURA: u16 = 47;
    /// Eight aura-flag nibbles per u32.
    pub const UNIT_AURAFLAGS: u16 = 95;
    /// Four aura-level bytes per u32.
    pub const UNIT_AURALEVELS: u16 = 101;
    /// Four application-count bytes per u32; each stores count minus one.
    pub const UNIT_AURAAPPLICATIONS: u16 = 113;
    pub const AURA_SLOTS: u16 = 48;

    pub const UNIT_DISPLAYID: u16 = 131;
    pub const UNIT_NATIVEDISPLAYID: u16 = 132;
    /// Byte 3 carries the ghost visibility flag.
    pub const UNIT_BYTES_1: u16 = 138;
    /// Byte 0 is sheath state; gtker mislabels the setter's parameters as PLAYER_BYTES_2.
    pub const UNIT_BYTES_2: u16 = 164;
    pub const UNIT_STAT0: u16 = 150;

    pub const CONTAINER_FIELD_SLOT_1: u16 = 50;

    pub const PLAYER_DUEL_ARBITER: u16 = 188;
    pub const PLAYER_FLAGS: u16 = 190;
    pub const PLAYER_GUILDID: u16 = 191;
    pub const PLAYER_GUILDRANK: u16 = 192;
    /// Byte 0 is facial hair; byte 3 is rest state, 1 rested or 2 normal.
    pub const PLAYER_BYTES_2: u16 = 194;
    pub const PLAYER_DUEL_TEAM: u16 = 196;
    // Build 5875: UNIT_END 188 + 0x0A; gtker exposes no quest-log setter.
    pub const PLAYER_QUEST_LOG_1_1: u16 = 198;
    pub const QUEST_LOG_SLOTS: u16 = 20;
    pub const PLAYER_FIELD_COMBO_TARGET: u16 = 714;
    pub const PLAYER_XP: u16 = 716;
    pub const PLAYER_EXPLORED_ZONES_1: u16 = 1111;
    pub const EXPLORED_ZONES_WORDS: u16 = 64;
    pub const PLAYER_FIELD_COINAGE: u16 = 1176;
    /// Byte 1 is the combo point count; bytes 0, 2 and 3 hold flags, action bars and honor rank.
    pub const PLAYER_FIELD_BYTES: u16 = 1222;

    pub const GAMEOBJECT_ROTATION: u16 = 10;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UpdateMaskValues {
    fields: BTreeMap<u16, u32>,
}

impl UpdateMaskValues {
    pub fn new() -> Self {
        Self {
            fields: BTreeMap::new(),
        }
    }

    pub fn set_u32(&mut self, index: u16, value: u32) -> &mut Self {
        self.fields.insert(index, value);
        self
    }

    pub fn set_u64(&mut self, index: u16, value: u64) -> &mut Self {
        self.fields.insert(index, value as u32);
        self.fields.insert(index + 1, (value >> 32) as u32);
        self
    }

    #[allow(dead_code)]
    pub fn set_f32(&mut self, index: u16, value: f32) -> &mut Self {
        self.set_u32(index, value.to_bits())
    }

    pub fn set_bytes(&mut self, index: u16, a: u8, b: u8, c: u8, d: u8) -> &mut Self {
        self.set_u32(index, u32::from_le_bytes([a, b, c, d]))
    }

    pub fn or_u32(&mut self, index: u16, bits: u32) -> &mut Self {
        *self.fields.entry(index).or_insert(0) |= bits;
        self
    }

    pub fn get(&self, index: u16) -> Option<u32> {
        self.fields.get(&index).copied()
    }

    pub fn block_count(&self) -> usize {
        match self.fields.keys().next_back() {
            Some(&max) => max as usize / 32 + 1,
            None => 0,
        }
    }

    pub fn write_to(&self, out: &mut Vec<u8>) {
        let blocks = self.block_count();
        out.push(blocks as u8);
        let mut mask = vec![0u32; blocks];
        for &index in self.fields.keys() {
            mask[index as usize / 32] |= 1 << (index % 32);
        }
        for word in &mask {
            out.extend_from_slice(&word.to_le_bytes());
        }
        for value in self.fields.values() {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }

    #[allow(dead_code)]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write_to(&mut out);
        out
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AuraSlot {
    pub slot: u8,
    pub spell_id: u32,
    pub flags: u8,
    pub level: u8,
}

pub fn write_auras(mask: &mut UpdateMaskValues, auras: &[AuraSlot]) {
    for a in auras {
        let slot = a.slot as u16;
        mask.set_u32(idx::UNIT_AURA + slot, a.spell_id);
        mask.or_u32(
            idx::UNIT_AURAFLAGS + slot / 8,
            ((a.flags & 0x0F) as u32) << ((slot % 8) * 4),
        );
        mask.or_u32(
            idx::UNIT_AURALEVELS + slot / 4,
            (a.level as u32) << ((slot % 4) * 8),
        );
    }
}

pub fn full_aura_mask(auras: &[AuraSlot]) -> UpdateMaskValues {
    let mut m = UpdateMaskValues::new();
    for s in 0..idx::AURA_SLOTS {
        m.set_u32(idx::UNIT_AURA + s, 0);
    }
    for w in 0..6u16 {
        m.set_u32(idx::UNIT_AURAFLAGS + w, 0);
    }
    for w in 0..12u16 {
        m.set_u32(idx::UNIT_AURALEVELS + w, 0);
        m.set_u32(idx::UNIT_AURAAPPLICATIONS + w, 0);
    }
    write_auras(&mut m, auras);
    m
}

pub const fn quest_log_field(slot: u16, sub: u16) -> u16 {
    idx::PLAYER_QUEST_LOG_1_1 + slot * 3 + sub
}

/// Vanilla packs four 6-bit counters below the state byte; later clients use byte counters.
pub fn pack_quest_counts(counts: &[u32], state: u8) -> u32 {
    let mut v = 0u32;
    for (i, &c) in counts.iter().take(4).enumerate() {
        v |= (c.min(63) & 0x3F) << (i as u32 * 6);
    }
    v | ((state as u32) << 24)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestLogSlot {
    pub slot: u8,
    pub quest_id: u32,
    pub counts: Vec<u32>,
    pub state: u8,
    pub timer: u32,
}

pub fn full_quest_log_mask(slots: &[QuestLogSlot]) -> UpdateMaskValues {
    let mut m = UpdateMaskValues::new();
    for s in 0..idx::QUEST_LOG_SLOTS {
        m.set_u32(quest_log_field(s, 0), 0);
        m.set_u32(quest_log_field(s, 1), 0);
        m.set_u32(quest_log_field(s, 2), 0);
    }
    for q in slots {
        let s = q.slot as u16;
        m.set_u32(quest_log_field(s, 0), q.quest_id);
        m.set_u32(quest_log_field(s, 1), pack_quest_counts(&q.counts, q.state));
        m.set_u32(quest_log_field(s, 2), q.timer);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_world_messages::vanilla::ServerMessage;

    #[test]
    fn matches_gtker_most_minimal_example() {
        let expected: &[u8] = &[
            2, 7, 0, 64, 0, 16, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 25, 0, 0, 0, 100, 0, 0, 0, 1, 1,
            1, 1,
        ];
        let mut m = UpdateMaskValues::new();
        m.set_u64(idx::OBJECT_GUID, 4);
        m.set_u32(idx::OBJECT_TYPE, 25);
        m.set_u32(idx::UNIT_HEALTH, 100);
        m.set_bytes(idx::UNIT_BYTES_0, 1, 1, 1, 1);
        assert_eq!(m.to_bytes().as_slice(), expected);
    }

    #[test]
    fn matches_gtker_small_example() {
        let expected: &[u8] = &[
            5, 23, 0, 64, 16, 28, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 24, 0, 0, 0, 4, 0, 0, 0, 0, 0,
            0, 0, 25, 0, 0, 0, 0, 0, 128, 63, 100, 0, 0, 0, 100, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0,
            1, 1, 1, 1, 50, 0, 0, 0, 50, 0, 0, 0,
        ];
        let mut m = UpdateMaskValues::new();
        m.set_u64(idx::OBJECT_GUID, 4);
        m.set_u32(idx::OBJECT_TYPE, 25);
        m.set_f32(idx::OBJECT_SCALE_X, 1.0);
        m.set_u32(idx::UNIT_HEALTH, 100);
        m.set_u32(idx::UNIT_MAXHEALTH, 100);
        m.set_u32(idx::UNIT_LEVEL, 1);
        m.set_u32(idx::UNIT_FACTIONTEMPLATE, 1);
        m.set_bytes(idx::UNIT_BYTES_0, 1, 1, 1, 1);
        m.set_u32(idx::UNIT_DISPLAYID, 50);
        m.set_u32(idx::UNIT_NATIVEDISPLAYID, 50);
        assert_eq!(m.to_bytes().as_slice(), expected);
    }

    #[test]
    fn reproduces_live_build_aura_values_slot0() {
        use wow_world_messages::vanilla::{Object, UpdateMask, UpdatePlayer, SMSG_UPDATE_OBJECT};
        use wow_world_messages::Guid;
        let (spell_id, level, flags) = (6673u32, 1u8, 0x09u8);

        let mut player = UpdatePlayer::builder()
            .set_unit_aura(spell_id as i32)
            .set_unit_auraflags(flags, 0, 0, 0)
            .set_unit_auralevels(level, 0, 0, 0)
            .set_unit_auraapplications(0, 0, 0, 0)
            .finalize();
        player.dirty_reset();
        player.set_unit_aura(spell_id as i32);
        player.set_unit_auraflags(flags, 0, 0, 0);
        player.set_unit_auralevels(level, 0, 0, 0);
        player.set_unit_auraapplications(0, 0, 0, 0);
        let mut packet = Vec::new();
        SMSG_UPDATE_OBJECT {
            has_transport: 0,
            objects: vec![Object::Values {
                guid1: Guid::new(0x42),
                mask1: UpdateMask::Player(player),
            }],
        }
        .write_unencrypted_server(&mut packet)
        .unwrap();

        let mut ours = UpdateMaskValues::new();
        write_auras(
            &mut ours,
            &[AuraSlot {
                slot: 0,
                spell_id,
                flags,
                level,
            }],
        );
        ours.set_u32(idx::UNIT_AURAAPPLICATIONS, 0);
        let ours = ours.to_bytes();

        assert!(
            packet.ends_with(&ours),
            "live aura packet tail must equal our encoder's mask\n  ours:   {ours:?}\n  packet: {packet:?}"
        );
    }

    #[test]
    fn multi_slot_auras_target_distinct_indices() {
        let mut m = UpdateMaskValues::new();
        write_auras(
            &mut m,
            &[
                AuraSlot {
                    slot: 0,
                    spell_id: 6673,
                    flags: 0x09,
                    level: 1,
                },
                AuraSlot {
                    slot: 1,
                    spell_id: 1459,
                    flags: 0x01,
                    level: 2,
                },
            ],
        );
        assert_eq!(m.get(idx::UNIT_AURA), Some(6673));
        assert_eq!(m.get(idx::UNIT_AURA + 1), Some(1459));
        assert_eq!(m.get(idx::UNIT_AURAFLAGS), Some(0x19));
        assert_eq!(m.get(idx::UNIT_AURALEVELS), Some(0x0201));
        assert!(!m.to_bytes().is_empty());
    }

    #[test]
    fn empty_mask_is_one_zero_byte() {
        assert_eq!(UpdateMaskValues::new().to_bytes(), vec![0u8]);
    }

    #[test]
    fn quest_log_field_indices() {
        assert_eq!(idx::PLAYER_QUEST_LOG_1_1, 198);
        assert_eq!(quest_log_field(0, 0), 198);
        assert_eq!(quest_log_field(0, 1), 199);
        assert_eq!(quest_log_field(0, 2), 200);
        assert_eq!(quest_log_field(1, 0), 201);
        assert_eq!(quest_log_field(19, 0), 255);
        assert_eq!(quest_log_field(19, 2), 257);
    }

    #[test]
    fn quest_counts_pack_6bit_plus_state() {
        assert_eq!(
            pack_quest_counts(&[1, 2, 3, 4], 0),
            1 | (2 << 6) | (3 << 12) | (4 << 18)
        );
        assert_eq!(pack_quest_counts(&[], 1), 1 << 24);
        assert_eq!(pack_quest_counts(&[5], 1), 5 | (1 << 24));
        assert_eq!(pack_quest_counts(&[100], 0), 63);
        assert_eq!(pack_quest_counts(&[63, 63], 0), 63 | (63 << 6));
        assert_eq!(
            pack_quest_counts(&[1, 1, 1, 1, 1], 0),
            1 | (1 << 6) | (1 << 12) | (1 << 18)
        );
    }

    #[test]
    fn full_quest_log_mask_overlays_and_clears() {
        let m = full_quest_log_mask(&[QuestLogSlot {
            slot: 2,
            quest_id: 7,
            counts: vec![10],
            state: 1,
            timer: 0,
        }]);
        assert_eq!(m.get(quest_log_field(2, 0)), Some(7));
        assert_eq!(m.get(quest_log_field(2, 1)), Some(10 | (1 << 24)));
        assert_eq!(m.get(quest_log_field(0, 0)), Some(0));
        assert_eq!(m.get(quest_log_field(19, 2)), Some(0));
        assert!(!m.to_bytes().is_empty());
    }

    #[test]
    fn empty_quest_log_mask_clears_all_slots() {
        let m = full_quest_log_mask(&[]);
        for s in 0..idx::QUEST_LOG_SLOTS {
            assert_eq!(m.get(quest_log_field(s, 0)), Some(0));
            assert_eq!(m.get(quest_log_field(s, 1)), Some(0));
            assert_eq!(m.get(quest_log_field(s, 2)), Some(0));
        }
        assert_eq!(m.get(idx::OBJECT_TYPE), None);
        let bytes = m.to_bytes();
        let mut expected = Vec::new();
        expected.push(9u8);
        for w in [0u32, 0, 0, 0, 0, 0, 0xFFFF_FFC0, 0xFFFF_FFFF, 0x0000_0003] {
            expected.extend_from_slice(&w.to_le_bytes());
        }
        expected.extend(std::iter::repeat_n(0u8, 60 * 4));
        assert_eq!(
            bytes.len(),
            277,
            "empty quest-log mask must be 277 bytes (the clear-all-slots block)"
        );
        assert_eq!(
            bytes, expected,
            "empty quest-log mask bytes diverged from the traced crash payload"
        );
    }
}
