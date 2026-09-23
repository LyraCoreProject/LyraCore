//! The deterministic pet guid: derived from the owner's guid, so both the Module's own pet reads
//! and the Gateway's Member Stats read can find a pet with one keyed lookup instead of a table
//! scan.

/// `HIGHGUID_UNIT` (0xF130): the high 16 bits of every creature guid. The 5875 client classifies
/// an object (Unit vs other) from its guid's HIGHGUID type, so a pet must carry it or the client
/// cannot treat it as a unit (broken nameplate, targeting, pet bar).
const HIGHGUID_UNIT: u64 = 0xF130;
/// Vanilla's separate namespace for a pet owned by another server-authored unit.
const HIGHGUID_PET: u64 = 0xF140;

/// The deterministic pet guid for an owner. Player pets retain the established `HIGHGUID_UNIT`
/// namespace. A creature-owned spell guardian uses vanilla's `HIGHGUID_PET`, otherwise copying the
/// creature owner's low 48 bits would reproduce the owner's guid and collide on insert. Stable so
/// the Module's keyed `pet_of` read, its despawn delete, and the Gateway's Member Stats pet read
/// all agree. Pure.
pub fn pet_guid_for(owner_guid: u64) -> u64 {
    let high = if owner_guid >> 48 == HIGHGUID_UNIT {
        HIGHGUID_PET
    } else {
        HIGHGUID_UNIT
    };
    (high << 48) | (owner_guid & 0x0000_FFFF_FFFF_FFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_player_owners_pet_keeps_the_unit_namespace() {
        let owner = 7; // a player guid: no HIGHGUID tag of its own
        assert_eq!(pet_guid_for(owner) >> 48, HIGHGUID_UNIT);
        assert_eq!(pet_guid_for(owner) & 0x0000_FFFF_FFFF_FFFF, 7);
    }

    #[test]
    fn a_creature_owners_guardian_uses_the_pet_namespace() {
        let owner = (HIGHGUID_UNIT << 48) | 11; // the owner is itself a creature
        assert_eq!(pet_guid_for(owner) >> 48, HIGHGUID_PET);
    }
}
