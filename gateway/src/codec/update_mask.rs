//! Hand-rolled vanilla update-mask encoder — the shared foundation that lifts gtker 0.3's
//! `pub(crate)` wall on indexed descriptor arrays (auras 1..47, quest-log counts, multi-field item
//! descriptors all hit it; see the gtker-descriptor-setter-wall note). gtker only exposes the
//! *first slot* of each indexed array and keeps the mask serializer (`set_int`/`set_bytes`/
//! `write_into_vec`) crate-private, so anything past slot 0 is unreachable through its typed builder.
//! This module re-implements the mask at the byte level so we own every field index.
//!
//! THE WIRE FORMAT (taken from `wow_world_messages`' own `update_mask` unit tests — our oracle):
//!   1. one byte: `N` = number of u32 mask blocks that follow;
//!   2. `N` little-endian u32 mask blocks — bit `i` (block `i/32`, bit `i%32`) is set iff field
//!      index `i` is present;
//!   3. each present field's u32 value, little-endian, in ASCENDING field-index order. A u64 field
//!      (e.g. `OBJECT_FIELD_GUID`) occupies two adjacent indices `i`,`i+1` (low then high) and so
//!      emits 8 bytes; a packed-bytes field (`UNIT_FIELD_BYTES_0`) is one u32 of 4 little-endian u8s.
//!
//! `UpdateMaskValues` models only PRESENT fields, every one of them "dirty". That is exactly the
//! shape `wow_world_messages` serializes when `header == dirty_mask` (a full CREATE mask) — and it's
//! also what its `dirty_reset()→re-set-only-changed` partial-VALUES dance produces on the wire (the
//! masked-out bits, incl. the force-seeded `OBJECT_FIELD_TYPE`, contribute nothing to `h & d`). So
//! the 5875 crash-trap is structural here: we simply never insert index 2 (`OBJECT_FIELD_TYPE`) on a
//! partial update, and the byte-equivalence tests below pin our output to gtker's, both for its
//! published CREATE vectors and for gtker's typed slot-0 aura partial mask.
//!
//! Wired into the outbound path via a raw-opcode send (`codec::build_values_update_raw`) rather
//! than gtker's typed sender, because gtker's mask *reader* rejects a `OBJECT_FIELD_TYPE`-less
//! mask (the 5875 client's own reader accepts one — that asymmetry is what makes the raw-opcode
//! path work at all). Live consumers: `build_values_update_raw` (`codec/values.rs:96`) sends the
//! fog bitmask, the rest byte, and bag-slot updates through it; `full_aura_mask`/
//! `full_quest_log_mask` compose the packed multi-slot aura and quest-log arrays this module
//! exists to reach past gtker's slot-0-only setters, called live from the aura/quest-log relay
//! paths in `stdb/subscriptions.rs` and `world/mod.rs`.

use std::collections::BTreeMap;

/// Vanilla (build 5875) UNIT/PLAYER object update-field indices, each a u32 slot. Every number is
/// the literal `set_int(N, ..)` / `set_bytes(N, ..)` argument from `wow_world_messages`' setters, so
/// the byte-equivalence tests cross-check them against gtker's serializer.
///
/// This is the reference layout for the whole vanilla descriptor, not just the fields a caller
/// reaches today — `build_values_update_raw`/`full_aura_mask`/`full_quest_log_mask` consume a
/// subset live (see the module doc); the rest (health/level/faction/display-id/stat block, etc.)
/// are cross-checked by the byte-equivalence tests and wait for their own live sender, the same
/// way the aura/quest-log/fog fields did before their tick landed.
#[allow(dead_code)]
pub mod idx {
    /// `OBJECT_FIELD_GUID` — u64, occupies slots 0 and 1.
    pub const OBJECT_GUID: u16 = 0;
    /// `OBJECT_FIELD_TYPE` — the object-type bitmask. NEVER set on a partial VALUES update (5875
    /// crash-trap: re-sending it strips the PLAYER bit and the client faults at null+0x110).
    pub const OBJECT_TYPE: u16 = 2;
    pub const OBJECT_ENTRY: u16 = 3;
    /// `OBJECT_FIELD_SCALE_X` — f32.
    pub const OBJECT_SCALE_X: u16 = 4;

    pub const UNIT_HEALTH: u16 = 22;
    /// `UNIT_FIELD_POWER1..5` (Mana, Rage, Focus, Energy, Happiness) — add 0..4.
    pub const UNIT_POWER1: u16 = 23;
    pub const UNIT_MAXHEALTH: u16 = 28;
    /// `UNIT_FIELD_MAXPOWER1..5` — add 0..4.
    pub const UNIT_MAXPOWER1: u16 = 29;
    pub const UNIT_LEVEL: u16 = 34;
    pub const UNIT_FACTIONTEMPLATE: u16 = 35;
    /// `UNIT_FIELD_BYTES_0` — packed race|class|gender|power (4×u8).
    pub const UNIT_BYTES_0: u16 = 36;
    pub const UNIT_FLAGS: u16 = 46;

    /// `UNIT_FIELD_AURA` — 48 slots, one spell-id u32 each (slot k at `UNIT_AURA + k`).
    pub const UNIT_AURA: u16 = 47;
    /// `UNIT_FIELD_AURAFLAGS` — 6 u32s; 8 auras per u32, **4 bits each** (slot k → word `+k/8`,
    /// nibble `(k%8)*4`).
    pub const UNIT_AURAFLAGS: u16 = 95;
    /// `UNIT_FIELD_AURALEVELS` — 12 u32s; 4 auras per u32, **1 byte each** (slot k → word `+k/4`,
    /// byte `(k%4)*8`).
    pub const UNIT_AURALEVELS: u16 = 101;
    /// `UNIT_FIELD_AURAAPPLICATIONS` — 12 u32s; 4 auras per u32, 1 byte each (stack count, stored as
    /// `count-1`; a single application is 0).
    pub const UNIT_AURAAPPLICATIONS: u16 = 113;
    /// Number of aura slots in the vanilla unit descriptor.
    pub const AURA_SLOTS: u16 = 48;

    pub const UNIT_DISPLAYID: u16 = 131;
    pub const UNIT_NATIVEDISPLAYID: u16 = 132;
    /// `UNIT_FIELD_BYTES_1` — packed; byte 3 carries the vis-ghost flag (4×u8).
    pub const UNIT_BYTES_1: u16 = 138;
    /// `UNIT_FIELD_STAT0..4` — STR, AGI, STA, INT, SPI (add 0..4).
    pub const UNIT_STAT0: u16 = 150;

    /// `CONTAINER_FIELD_SLOT_1` — first bag-content slot pointer, a u64 guid at field indices 50..51.
    /// Slot N (0-indexed): `CONTAINER_FIELD_SLOT_1 + N * 2`. The gtker vanilla builder only exposes
    /// `set_container_slot_1` (this index); slots 1+ require the hand-rolled raw encoder (same
    /// 'gtker descriptor-setter wall' as multi-aura). Cross-checked against `wow_world_messages`
    /// vanilla `UpdateContainerBuilder::set_container_slot_1 → set_guid(50, ..)`.
    pub const CONTAINER_FIELD_SLOT_1: u16 = 50;

    pub const PLAYER_FLAGS: u16 = 190;
    /// `PLAYER_BYTES_2`: byte 0 = facial hair, byte 3 = rest state (RESTED 0x01 → zzz + blue XP
    /// bar / NORMAL 0x02). Vanilla 1.12 index 194 (gtker `set_player_bytes_2` → `set_bytes(194, …)`,
    /// cross-checked: `PLAYER_BYTES_3 = 195`, `PLAYER_FLAGS = 190`). Relayed live on an inn crossing.
    pub const PLAYER_BYTES_2: u16 = 194;
    /// `PLAYER_QUEST_LOG_1_1` — quest-log slot 0, sub-field 0 (the quest id). 20 slots × 3 u32 each
    /// (`_1` id, `_2` packed 6-bit counters + state byte, `_3` timer); slot S sub F = this + S*3 + F.
    /// Vanilla 1.12 (build 5875): `UNIT_END(188) + 0x0A = 198`. CROSS-CHECKED against our own anchor —
    /// `PLAYER_COINAGE = 1176 = 188 + 0x3DC`, so this index scheme uses the same `UNIT_END = 188` the
    /// emulator headers assume. Three emulators (mangos-zero / cmangos / vmangos) agree byte-for-byte.
    /// VERIFIED against a live 5875 client 2026-06-24: empty + multi-quest
    /// logs render with no crash, and a kill drove the objective to "Kobold Vermin slain: 1/10" live —
    /// confirming the index AND the 6-bit-per-counter packing. (No gtker reference exists for these fields.)
    pub const PLAYER_QUEST_LOG_1_1: u16 = 198;
    /// Number of quest-log slots in the vanilla player descriptor.
    pub const QUEST_LOG_SLOTS: u16 = 20;
    pub const PLAYER_XP: u16 = 716;
    /// PLAYER_EXPLORED_ZONES: 64 consecutive u32 fields (1111..=1174) — the map-fog bitmask, one bit per
    /// game_area.area_bit. Word for area_bit b = PLAYER_EXPLORED_ZONES_1 + b/32, bit = 1 << (b%32).
    /// (PLAYER_REST_STATE_EXPERIENCE=1175 follows immediately, cross-checked vs the gtker descriptor.)
    pub const PLAYER_EXPLORED_ZONES_1: u16 = 1111;
    pub const EXPLORED_ZONES_WORDS: u16 = 64;
    pub const PLAYER_FIELD_COINAGE: u16 = 1176;

    /// `GAMEOBJECT_ROTATION` — a GAMEOBJECT-descriptor field (its own index space, separate from
    /// UNIT/PLAYER above), 4 consecutive f32 slots (rot0..3, this + 0..=3). Cross-checked against
    /// `wow_world_messages` vanilla `UpdateGameObjectBuilder::set_gameobject_rotation` → `set_float(10,
    /// ..)` — gtker's typed setter only reaches slot 0 (the descriptor-setter wall, same as multi-aura),
    /// so all 4 slots ride the hand-rolled raw encoder (issue #515). `GAMEOBJECT_STATE` follows
    /// immediately at 14, confirming the 4-slot width.
    pub const GAMEOBJECT_ROTATION: u16 = 10;
}

/// A sparse vanilla update mask: field index → u32 value, serialized in the exact wire layout the
/// 5875 client expects. Construct it, set the fields that changed, then [`to_bytes`](Self::to_bytes).
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

    /// Set one u32 field.
    pub fn set_u32(&mut self, index: u16, value: u32) -> &mut Self {
        self.fields.insert(index, value);
        self
    }

    /// Set a u64 field spanning two adjacent slots: low half at `index`, high half at `index + 1`
    /// (gtker's `set_guid`). The two halves serialize as the field's 8 little-endian bytes.
    pub fn set_u64(&mut self, index: u16, value: u64) -> &mut Self {
        self.fields.insert(index, value as u32);
        self.fields.insert(index + 1, (value >> 32) as u32);
        self
    }

    /// Set an f32 field by its IEEE-754 bit pattern (gtker's `set_float`). No live caller yet —
    /// today's live senders only touch u32/u64/packed-bytes fields — but it's exercised by the
    /// byte-equivalence tests and completes the setter family for the first f32 field a live
    /// sender needs (e.g. `OBJECT_FIELD_SCALE_X`).
    #[allow(dead_code)]
    pub fn set_f32(&mut self, index: u16, value: f32) -> &mut Self {
        self.set_u32(index, value.to_bits())
    }

    /// Set a packed-bytes field: `a,b,c,d` → one little-endian u32 (gtker's `set_bytes`). No live
    /// caller yet — see `set_f32`'s note; today's packed-bytes live sends go through `set_u32`
    /// with the bytes already packed by the caller (e.g. the rest byte in `codec/values.rs`).
    #[allow(dead_code)]
    pub fn set_bytes(&mut self, index: u16, a: u8, b: u8, c: u8, d: u8) -> &mut Self {
        self.set_u32(index, u32::from_le_bytes([a, b, c, d]))
    }

    /// OR bits into a field that may already hold other sub-fields (the nibble/byte-packed aura
    /// arrays, where several aura slots share one u32). Distinct from `set_*`, which overwrites.
    pub fn or_u32(&mut self, index: u16, bits: u32) -> &mut Self {
        *self.fields.entry(index).or_insert(0) |= bits;
        self
    }

    pub fn get(&self, index: u16) -> Option<u32> {
        self.fields.get(&index).copied()
    }

    /// Number of u32 mask blocks: `highest_set_index / 32 + 1`, or 0 when empty.
    pub fn block_count(&self) -> usize {
        match self.fields.keys().next_back() {
            Some(&max) => max as usize / 32 + 1,
            None => 0,
        }
    }

    /// Serialize into `out` in the vanilla wire layout (block count byte, mask blocks, then present
    /// values in ascending index order). Byte-identical to `wow_world_messages`'
    /// `UpdateMask::write_into_vec` for a full (header == dirty) mask.
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

    /// Convenience wrapper over [`write_to`](Self::write_to) into a fresh `Vec`. Live senders
    /// write into an existing outbound buffer via `write_to` directly; this one is the tests'
    /// entry point for the byte-equivalence assertions.
    #[allow(dead_code)]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write_to(&mut out);
        out
    }
}

/// One aura slot to render in the unit descriptor: which `slot` (0..48), the `spell_id`, the
/// 4-bit slot `flags`, and the unit `level` of the caster (shown in the tooltip).
#[derive(Clone, Copy, Debug)]
pub struct AuraSlot {
    pub slot: u8,
    pub spell_id: u32,
    pub flags: u8,
    pub level: u8,
}

/// Write a set of aura slots into a mask using the packed array layout (the unlock past gtker's
/// slot-0-only setters): spell-id one u32 per slot, flags 4 bits/slot, level + applications one
/// byte/slot. The caller decides CREATE vs partial-VALUES by which other fields it adds; this only
/// touches the aura arrays (never `OBJECT_FIELD_TYPE`), so it's safe in a partial update.
pub fn write_auras(mask: &mut UpdateMaskValues, auras: &[AuraSlot]) {
    for a in auras {
        let slot = a.slot as u16;
        mask.set_u32(idx::UNIT_AURA + slot, a.spell_id);
        // flags: 8 per u32, nibble (4 bits) each.
        mask.or_u32(
            idx::UNIT_AURAFLAGS + slot / 8,
            ((a.flags & 0x0F) as u32) << ((slot % 8) * 4),
        );
        // level: 4 per u32, one byte each.
        mask.or_u32(
            idx::UNIT_AURALEVELS + slot / 4,
            (a.level as u32) << ((slot % 4) * 8),
        );
        // applications: a single application (stack 1) is stored as 0, so nothing to OR in.
    }
}

/// Build a FULL `UNIT_FIELD_AURA` array VALUES mask from a unit's complete current aura set: every
/// one of the 48 aura slots' spell-id, plus the packed flag/level/application words, present `auras`
/// overlaid (at their own `slot`) and every other slot zeroed.
///
/// It is a FULL SYNC by necessity, not choice: (1) the spell-id slots must be zeroed so a removed
/// aura's icon clears, and (2) the flag/level/application words are PACKED — 8 auras share one
/// AURAFLAGS u32, 4 share one AURALEVELS/APPLICATIONS u32 — so a single-slot update can only write
/// the whole word and would clobber the co-located slots. Composing the words from the entire
/// current set is the only correct way to touch one slot without corrupting its neighbours. The
/// caller passes each aura at its authoritative module-assigned `slot` (never a gateway-invented
/// index). Touches only the aura arrays — never `OBJECT_FIELD_TYPE` — so it is a safe partial VALUES
/// update. This is the multi-slot shape gtker's slot-0-only setters cannot express; it rides the
/// raw-send path (`codec::build_values_update_raw`).
pub fn full_aura_mask(auras: &[AuraSlot]) -> UpdateMaskValues {
    let mut m = UpdateMaskValues::new();
    for s in 0..idx::AURA_SLOTS {
        m.set_u32(idx::UNIT_AURA + s, 0);
    }
    for w in 0..6u16 {
        m.set_u32(idx::UNIT_AURAFLAGS + w, 0); // 48 slots / 8 per word
    }
    for w in 0..12u16 {
        m.set_u32(idx::UNIT_AURALEVELS + w, 0); // 48 / 4
        m.set_u32(idx::UNIT_AURAAPPLICATIONS + w, 0);
    }
    write_auras(&mut m, auras); // overlay present auras (ORs into the packed words)
    m
}

/// Field index of quest-log `slot` (0..[`idx::QUEST_LOG_SLOTS`]) sub-field `sub` (0 = quest id,
/// 1 = packed counters+state, 2 = timer). Each slot is 3 contiguous u32s (vanilla 1.12 layout).
pub const fn quest_log_field(slot: u16, sub: u16) -> u16 {
    idx::PLAYER_QUEST_LOG_1_1 + slot * 3 + sub
}

/// Pack a quest slot's middle u32 (`_n_2`): four objective counters at **6 bits each** (0..63, vanilla
/// caps here — NOT a byte per objective like TBC/WotLK) in bits 0–5/6–11/12–17/18–23, plus the quest
/// `state` in byte 3 (bits 24–31; 0 = incomplete, 1 = complete, 2 = failed). Counts above 63 saturate.
/// Pure/testable.
pub fn pack_quest_counts(counts: &[u32], state: u8) -> u32 {
    let mut v = 0u32;
    for (i, &c) in counts.iter().take(4).enumerate() {
        v |= (c.min(63) & 0x3F) << (i as u32 * 6);
    }
    v | ((state as u32) << 24)
}

/// One quest-log slot to render in the player descriptor: `slot` (0..20), the `quest_id`, its per-
/// objective `counts`, the `state` (0/1/2), and the `timer` (0 if untimed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestLogSlot {
    pub slot: u8,
    pub quest_id: u32,
    pub counts: Vec<u32>,
    pub state: u8,
    pub timer: u32,
}

/// Build a FULL quest-log VALUES mask: all [`idx::QUEST_LOG_SLOTS`] slots' 3 u32s, with the present
/// `slots` overlaid at their own index and every other slot zeroed. Full sync (like [`full_aura_mask`])
/// so a turned-in / abandoned quest's slot clears. Touches only PLAYER_QUEST_LOG fields — never
/// `OBJECT_FIELD_TYPE` — so it is a safe partial VALUES update over the raw-send path
/// (`codec::build_values_update_raw`). Unlike auras, quest slots are independent (no cross-slot bit
/// packing), so a single slot *could* be updated alone; the full sync is chosen for clear-on-removal.
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

    /// The keystone: reproduce, byte-for-byte, `wow_world_messages`' own `most_minimal_example`
    /// serialized vector. If our block-count / mask-bit / value-order logic is wrong anywhere this
    /// fails against the crate author's gold standard.
    #[test]
    fn matches_gtker_most_minimal_example() {
        let expected: &[u8] = &[
            2, // block count
            7, 0, 64, 0, 16, 0, 0, 0, // 2 mask blocks
            4, 0, 0, 0, 0, 0, 0, 0, // OBJECT_FIELD_GUID (u64 = 4)
            25, 0, 0, 0, // OBJECT_FIELD_TYPE (0x19)
            100, 0, 0, 0, // UNIT_FIELD_HEALTH
            1, 1, 1, 1, // UNIT_FIELD_BYTES_0 (Human/Warrior/Female/Rage)
        ];
        let mut m = UpdateMaskValues::new();
        m.set_u64(idx::OBJECT_GUID, 4);
        m.set_u32(idx::OBJECT_TYPE, 25);
        m.set_u32(idx::UNIT_HEALTH, 100);
        m.set_bytes(idx::UNIT_BYTES_0, 1, 1, 1, 1);
        assert_eq!(m.to_bytes().as_slice(), expected);
    }

    /// A second gold-standard vector (gtker's `small_example`): a fuller CREATE mask spanning 5
    /// blocks, with an f32 (scale) and the high-index display fields (131/132). Cross-checks our
    /// block-count growth and the f32 / split-block bit math against the crate author's output.
    #[test]
    fn matches_gtker_small_example() {
        let expected: &[u8] = &[
            5, // block count
            23, 0, 64, 16, 28, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 24, 0, 0, 0, // 5 mask blocks
            4, 0, 0, 0, 0, 0, 0, 0, // OBJECT_FIELD_GUID (u64 = 4)
            25, 0, 0, 0, // OBJECT_FIELD_TYPE
            0, 0, 128, 63, // OBJECT_FIELD_SCALE_X (1.0)
            100, 0, 0, 0, // UNIT_FIELD_HEALTH
            100, 0, 0, 0, // UNIT_FIELD_MAXHEALTH
            1, 0, 0, 0, // UNIT_FIELD_LEVEL
            1, 0, 0, 0, // UNIT_FIELD_FACTIONTEMPLATE
            1, 1, 1, 1, // UNIT_FIELD_BYTES_0
            50, 0, 0, 0, // UNIT_FIELD_DISPLAYID
            50, 0, 0, 0, // UNIT_FIELD_NATIVEDISPLAYID
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

    /// The partial-VALUES discipline against gtker's typed slot-0 aura mask: our multi-aura encoder's
    /// single-slot output must be exactly the mask gtker's `UpdatePlayer` slot-0 aura builder puts on
    /// the wire. `gtker`'s mask serializer is `pub(crate)`, but the full `SMSG_UPDATE_OBJECT` is public
    /// and the mask is its tail — so we serialize the gtker-typed packet and assert it ENDS WITH our
    /// bytes. Proves we reproduce gtker's bytes (and, since `dirty_reset` strips it, that the slot-0
    /// shape never leaks `OBJECT_FIELD_TYPE`).
    #[test]
    fn reproduces_live_build_aura_values_slot0() {
        use wow_world_messages::vanilla::{Object, UpdateMask, UpdatePlayer, SMSG_UPDATE_OBJECT};
        use wow_world_messages::Guid;
        let (spell_id, level, flags) = (6673u32, 1u8, 0x09u8);

        // gtker's typed slot-0 aura VALUES update, with the finalize→dirty_reset→re-set-only-changed
        // discipline the live builders use so OBJECT_FIELD_TYPE is stripped (never re-sent).
        let mut player = UpdatePlayer::builder()
            .set_unit_aura(spell_id as i32)
            .set_unit_auraflags(flags, 0, 0, 0)
            .set_unit_auralevels(level, 0, 0, 0)
            .set_unit_auraapplications(0, 0, 0, 0) // stack count 1 is stored as 0
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

        // Ours, via the multi-aura writer with a single slot. The gtker builder also writes
        // auraapplications word 0 (value 0) — a present-but-zero field sets its mask bit and emits a
        // zero value word — so include it to match byte-for-byte.
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

    /// The unlock gtker can't express: a second aura slot lands at the right indices/bits.
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
        // Distinct spell-id slots.
        assert_eq!(m.get(idx::UNIT_AURA), Some(6673));
        assert_eq!(m.get(idx::UNIT_AURA + 1), Some(1459));
        // Both slots' flag nibbles share word 0: slot0 nibble0 = 0x9, slot1 nibble1 = 0x1.
        assert_eq!(m.get(idx::UNIT_AURAFLAGS), Some(0x19));
        // Both slots' level bytes share word 0: slot0 byte0 = 1, slot1 byte1 = 2.
        assert_eq!(m.get(idx::UNIT_AURALEVELS), Some(0x0201));
        // Round-trips through the serializer.
        assert!(!m.to_bytes().is_empty());
    }

    /// An empty mask is a single zero block-count byte (no blocks, no values).
    #[test]
    fn empty_mask_is_one_zero_byte() {
        assert_eq!(UpdateMaskValues::new().to_bytes(), vec![0u8]);
    }

    /// Quest-log field indices: slot 0 at 198/199/200, stride 3, last slot (19) at 255/256/257. These
    /// are the indices a wrong value crashes the client on, so pin them.
    #[test]
    fn quest_log_field_indices() {
        assert_eq!(idx::PLAYER_QUEST_LOG_1_1, 198);
        assert_eq!(quest_log_field(0, 0), 198); // slot 0: quest id
        assert_eq!(quest_log_field(0, 1), 199); // slot 0: counters+state
        assert_eq!(quest_log_field(0, 2), 200); // slot 0: timer
        assert_eq!(quest_log_field(1, 0), 201); // slot 1: stride 3
        assert_eq!(quest_log_field(19, 0), 255); // last slot id
        assert_eq!(quest_log_field(19, 2), 257); // last slot timer (block 8)
    }

    /// Counts pack 6 bits each (max 63, NOT a byte) with state in byte 3 — the vanilla subtlety.
    #[test]
    fn quest_counts_pack_6bit_plus_state() {
        // Four distinct counts in their 6-bit lanes.
        assert_eq!(
            pack_quest_counts(&[1, 2, 3, 4], 0),
            1 | (2 << 6) | (3 << 12) | (4 << 18)
        );
        // State in byte 3 (bits 24-31), independent of the counters.
        assert_eq!(pack_quest_counts(&[], 1), 1 << 24);
        assert_eq!(pack_quest_counts(&[5], 1), 5 | (1 << 24));
        // Saturates at 63 (a count of 100 must not bleed into the next objective's lane).
        assert_eq!(pack_quest_counts(&[100], 0), 63);
        assert_eq!(pack_quest_counts(&[63, 63], 0), 63 | (63 << 6));
        // Only the first 4 objectives are packed (vanilla cap).
        assert_eq!(
            pack_quest_counts(&[1, 1, 1, 1, 1], 0),
            1 | (1 << 6) | (1 << 12) | (1 << 18)
        );
    }

    /// A full quest-log mask zeroes all 20 slots and overlays the present ones (so removals clear).
    #[test]
    fn full_quest_log_mask_overlays_and_clears() {
        let m = full_quest_log_mask(&[QuestLogSlot {
            slot: 2,
            quest_id: 7,
            counts: vec![10],
            state: 1,
            timer: 0,
        }]);
        // Slot 2 carries the quest id + packed (count 10, state 1); other slots present but zero.
        assert_eq!(m.get(quest_log_field(2, 0)), Some(7));
        assert_eq!(m.get(quest_log_field(2, 1)), Some(10 | (1 << 24)));
        assert_eq!(m.get(quest_log_field(0, 0)), Some(0)); // empty slot zeroed (clears a removed quest)
        assert_eq!(m.get(quest_log_field(19, 2)), Some(0));
        // 20 slots × 3 fields all present.
        assert!(!m.to_bytes().is_empty());
    }

    /// `full_quest_log_mask(&[])` is the all-zero 60-field block the relay sends when a turn-in empties the
    /// quest log — it CLEARS every slot client-side (slot 0's quest_id → 0 is how the client removes the
    /// quest). NOT a crash payload: an earlier theory blamed it for the McBride turn-in crash, but the real
    /// cause was SMSG_SET_FACTION_STANDING sending faction_id instead of the rep-index (fixed in dee80cb).
    /// This pins the EXACT shape so a future mask/index drift is caught.
    ///
    /// Wire math (matches the traced packet to the byte): mask = 1 block-count byte + 9 mask blocks
    /// (bits 198..=257 ⇒ 257/32+1 = 9) + 60 zero u32 values = 1 + 36 + 240 = 277. The full raw VALUES
    /// body wraps it: 4 (amount) + 1 (has_transport) + 1 (update_type) + 2 (packed guid=1) + 277 = 285.
    #[test]
    fn empty_quest_log_mask_clears_all_slots() {
        let m = full_quest_log_mask(&[]);
        // All 60 quest-log fields present and zero — every slot's id/counters/timer cleared.
        for s in 0..idx::QUEST_LOG_SLOTS {
            assert_eq!(m.get(quest_log_field(s, 0)), Some(0));
            assert_eq!(m.get(quest_log_field(s, 1)), Some(0));
            assert_eq!(m.get(quest_log_field(s, 2)), Some(0));
        }
        // NEVER OBJECT_FIELD_TYPE (the other 5875 null+0x110 trap) — present here only by absence.
        assert_eq!(m.get(idx::OBJECT_TYPE), None);
        // Exact mask bytes: 9 blocks then 60 zero value words. Block 6 = bits 198..=223 (bits 6..=31
        // of word 6 ⇒ 0xFFFFFFC0), block 7 = bits 224..=255 (all ⇒ 0xFFFFFFFF), block 8 = bits 256,257
        // (⇒ 0x00000003); blocks 0..=5 zero.
        let bytes = m.to_bytes();
        let mut expected = Vec::new();
        expected.push(9u8); // block count
        for w in [0u32, 0, 0, 0, 0, 0, 0xFFFF_FFC0, 0xFFFF_FFFF, 0x0000_0003] {
            expected.extend_from_slice(&w.to_le_bytes());
        }
        expected.extend(std::iter::repeat_n(0u8, 60 * 4)); // 60 zero value words
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
