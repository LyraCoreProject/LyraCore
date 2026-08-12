//! Bit-packing helpers for the few descriptor fields the slice writes as packed bytes.
//!
//! Byte order within each packed field follows the 1.12 convention. Confirm against the
//! generated `wow_world_messages` definitions before relying on the exact byte positions
//! for appearance (`PLAYER_BYTES*`); the semantic intent is documented inline.

/// Pack `UNIT_FIELD_BYTES_0`: byte0 = race, byte1 = class, byte2 = gender, byte3 = power type.
pub const fn unit_bytes_0(race: u8, class: u8, gender: u8, power_type: u8) -> u32 {
    (race as u32) | ((class as u32) << 8) | ((gender as u32) << 16) | ((power_type as u32) << 24)
}

/// Pack `PLAYER_BYTES`: byte0 = skin, byte1 = face, byte2 = hair style, byte3 = hair color.
pub const fn player_bytes(skin: u8, face: u8, hair_style: u8, hair_color: u8) -> u32 {
    (skin as u32) | ((face as u32) << 8) | ((hair_style as u32) << 16) | ((hair_color as u32) << 24)
}

/// Rest state, stored in `PLAYER_BYTES_2` byte 3. MUST be a valid value (RESTED=0x01,
/// NORMAL=0x02): byte 3 = 0 makes the 5875 client's `GetRestState()` return a nil exhaustion
/// multiplier, crashing `MainMenuBar.lua` (the XP/rested bar) with an arithmetic-on-nil error.
pub const REST_STATE_NORMAL: u8 = 0x02;
/// RESTED (196) — the client draws the zzz icon + turns the XP bar blue when byte 3 holds this.
pub const REST_STATE_RESTED: u8 = 0x01;

/// Pack `PLAYER_BYTES_2`: byte0 = facial hair, byte2 = owned bank bag slot count, byte3 = rest state
/// (NORMAL so the client's XP/rested bar has a valid exhaustion state). Byte 1 is unused.
pub const fn player_bytes_2(facial_hair: u8, bank_bag_slots: u8) -> u32 {
    player_bytes_2_with_rest(facial_hair, bank_bag_slots, false)
}

/// Pack `PLAYER_BYTES_2` with an explicit rest state (196) — `resting` → RESTED byte (zzz + blue bar),
/// else NORMAL. Used at spawn so a character who logged out in an inn logs back in already showing
/// rested, and the purchased bank bag slot count shows without a relog.
pub const fn player_bytes_2_with_rest(facial_hair: u8, bank_bag_slots: u8, resting: bool) -> u32 {
    let rest = if resting {
        REST_STATE_RESTED
    } else {
        REST_STATE_NORMAL
    };
    (facial_hair as u32) | ((bank_bag_slots as u32) << 16) | ((rest as u32) << 24)
}

/// Replace byte 2 (owned bank bag slot count) on an already-packed `PLAYER_BYTES_2`, leaving byte 0
/// (facial hair) and byte 3 (rest state — see the HAZARD note on [`REST_STATE_NORMAL`]) untouched.
/// Used by the purchase path, which knows the new count but not the other two bytes' current values.
pub const fn with_bank_bag_slots(packed: u32, bank_bag_slots: u8) -> u32 {
    (packed & 0xFF00_FFFF) | ((bank_bag_slots as u32) << 16)
}

/// Unpack a packed `u32` descriptor field back into its four little-endian bytes — the inverse of
/// the packers above. Lives here so the descriptor byte format has a single home (the gateway
/// codec unpacks the same fields the module/seed pack).
pub const fn unpack4(v: u32) -> (u8, u8, u8, u8) {
    (v as u8, (v >> 8) as u8, (v >> 16) as u8, (v >> 24) as u8)
}

/// Power type ids (the 4th byte of `UNIT_FIELD_BYTES_0`).
pub mod power_type {
    pub const MANA: u8 = 0;
    pub const RAGE: u8 = 1;
    pub const FOCUS: u8 = 2;
    pub const ENERGY: u8 = 3;
    pub const HAPPINESS: u8 = 4;

    /// The power type for a given class (slice-relevant classes).
    pub const fn for_class(class: u8) -> u8 {
        match class {
            1 => RAGE,   // Warrior
            4 => ENERGY, // Rogue
            _ => MANA,   // everyone else (slice uses Warrior)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_bytes_0_layout() {
        // Human(1) Warrior(1) male(0) rage(1)
        let v = unit_bytes_0(1, 1, 0, power_type::RAGE);
        assert_eq!(v & 0xFF, 1); // race
        assert_eq!((v >> 8) & 0xFF, 1); // class
        assert_eq!((v >> 16) & 0xFF, 0); // gender
        assert_eq!((v >> 24) & 0xFF, 1); // power type = rage
    }

    #[test]
    fn for_class_maps_warrior_rage_rogue_energy_others_mana() {
        assert_eq!(power_type::for_class(1), power_type::RAGE); // Warrior
        assert_eq!(power_type::for_class(4), power_type::ENERGY); // Rogue
        assert_eq!(power_type::for_class(2), power_type::MANA); // everyone else (e.g. Paladin)
    }

    #[test]
    fn player_bytes_layout() {
        let v = player_bytes(0x11, 0x22, 0x33, 0x44);
        assert_eq!(v, 0x4433_2211);
    }

    #[test]
    fn pack_unpack_round_trips() {
        assert_eq!(
            unpack4(unit_bytes_0(1, 1, 0, power_type::RAGE)),
            (1, 1, 0, power_type::RAGE)
        );
        assert_eq!(
            unpack4(player_bytes(0x11, 0x22, 0x33, 0x44)),
            (0x11, 0x22, 0x33, 0x44)
        );
        // player_bytes_2 must carry a valid rest state in byte 3 (else the client XP-bar Lua crashes).
        assert_eq!(unpack4(player_bytes_2(7, 0)), (7, 0, 0, REST_STATE_NORMAL));
    }

    #[test]
    fn player_bytes_2_carries_the_bank_slot_count_in_byte_2() {
        assert_eq!(unpack4(player_bytes_2(7, 3)), (7, 0, 3, REST_STATE_NORMAL));
        assert_eq!(
            unpack4(player_bytes_2_with_rest(7, 3, true)),
            (7, 0, 3, REST_STATE_RESTED)
        );
    }

    #[test]
    fn with_bank_bag_slots_touches_only_byte_2() {
        let packed = player_bytes_2_with_rest(9, 1, true); // facial hair 9, RESTED
        let updated = with_bank_bag_slots(packed, 4);
        assert_eq!(unpack4(updated), (9, 0, 4, REST_STATE_RESTED));
    }
}
