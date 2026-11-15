// The 1.12.1 client classifies an object guid by its high part, bits 48..63.
pub const ITEM: u64 = 0x4000;
pub const MO_TRANSPORT: u64 = 0x1FC0;
pub const DYNAMIC_OBJECT: u64 = 0xF100;
pub const CORPSE: u64 = 0xF101;
pub const GAMEOBJECT: u64 = 0xF110;
pub const UNIT: u64 = 0xF130;
pub const PET: u64 = 0xF140;

pub const LOW_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

// A creature guid's low part gives 24 bits each to the template entry and the spawn identifier.
pub const CREATURE_COMPONENT_MASK: u32 = 0x00FF_FFFF;

#[must_use]
pub const fn high(guid: u64) -> u64 {
    guid >> 48
}

#[must_use]
pub const fn low(guid: u64) -> u64 {
    guid & LOW_MASK
}

/// `low` must fit in 48 bits; it is not masked.
#[must_use]
pub const fn compose(high: u64, low: u64) -> u64 {
    (high << 48) | low
}

#[must_use]
pub const fn creature(entry: u32, spawn_id: u32) -> u64 {
    compose(
        UNIT,
        (((entry & CREATURE_COMPONENT_MASK) as u64) << 24)
            | (spawn_id & CREATURE_COMPONENT_MASK) as u64,
    )
}

#[must_use]
pub const fn creature_spawn_id(guid: u64) -> u32 {
    (guid & CREATURE_COMPONENT_MASK as u64) as u32
}

#[must_use]
pub const fn is_creature(guid: u64) -> bool {
    high(guid) == UNIT
}

/// A creature or a pet.
#[must_use]
pub const fn is_unit(guid: u64) -> bool {
    matches!(high(guid), UNIT | PET)
}

#[must_use]
pub const fn is_gameobject(guid: u64) -> bool {
    high(guid) == GAMEOBJECT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_creature_guid_packs_entry_and_spawn_under_the_unit_high_part() {
        assert_eq!(creature(620, 1), 0xF130_0002_6C00_0001);
        assert_eq!(creature_spawn_id(creature(620, 1)), 1);
        assert!(is_creature(creature(620, 1)));
    }
}
