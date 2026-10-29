pub const fn unit_bytes_0(race: u8, class: u8, gender: u8, power_type: u8) -> u32 {
    (race as u32) | ((class as u32) << 8) | ((gender as u32) << 16) | ((power_type as u32) << 24)
}

pub const fn player_bytes(skin: u8, face: u8, hair_style: u8, hair_color: u8) -> u32 {
    (skin as u32) | ((face as u32) << 8) | ((hair_style as u32) << 16) | ((hair_color as u32) << 24)
}

// PLAYER_BYTES_2 byte 3 must be RESTED or NORMAL; zero crashes the client XP bar.
pub const REST_STATE_NORMAL: u8 = 0x02;
pub const REST_STATE_RESTED: u8 = 0x01;

// PLAYER_BYTES_2: facial hair in byte 0, bank slots in byte 2, rest state in byte 3.
pub const fn player_bytes_2(facial_hair: u8, bank_bag_slots: u8) -> u32 {
    player_bytes_2_with_rest(facial_hair, bank_bag_slots, false)
}

pub const fn player_bytes_2_with_rest(facial_hair: u8, bank_bag_slots: u8, rested: bool) -> u32 {
    let rest = if rested {
        REST_STATE_RESTED
    } else {
        REST_STATE_NORMAL
    };
    (facial_hair as u32) | ((bank_bag_slots as u32) << 16) | ((rest as u32) << 24)
}

pub const fn with_bank_bag_slots(packed: u32, bank_bag_slots: u8) -> u32 {
    (packed & 0xFF00_FFFF) | ((bank_bag_slots as u32) << 16)
}

pub const fn unpack4(v: u32) -> (u8, u8, u8, u8) {
    (v as u8, (v >> 8) as u8, (v >> 16) as u8, (v >> 24) as u8)
}

pub mod power_type {
    pub const MANA: u8 = 0;
    pub const RAGE: u8 = 1;
    pub const FOCUS: u8 = 2;
    pub const ENERGY: u8 = 3;
    pub const HAPPINESS: u8 = 4;

    pub const fn for_class(class: u8) -> u8 {
        match class {
            1 => RAGE,
            4 => ENERGY,
            _ => MANA,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_bytes_0_layout() {
        let v = unit_bytes_0(1, 1, 0, power_type::RAGE);
        assert_eq!(v & 0xFF, 1);
        assert_eq!((v >> 8) & 0xFF, 1);
        assert_eq!((v >> 16) & 0xFF, 0);
        assert_eq!((v >> 24) & 0xFF, 1);
    }

    #[test]
    fn for_class_maps_warrior_rage_rogue_energy_others_mana() {
        assert_eq!(power_type::for_class(1), power_type::RAGE);
        assert_eq!(power_type::for_class(4), power_type::ENERGY);
        assert_eq!(power_type::for_class(2), power_type::MANA);
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
        let packed = player_bytes_2_with_rest(9, 1, true);
        let updated = with_bank_bag_slots(packed, 4);
        assert_eq!(unpack4(updated), (9, 0, 4, REST_STATE_RESTED));
    }
}
