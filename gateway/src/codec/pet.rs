//! Hunter-pet identity packets and descriptor updates.

use super::*;
use wow_world_messages::vanilla::SMSG_PET_NAME_QUERY_RESPONSE;

const MAX_HAPPINESS: u32 = 1_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HunterPetProtocolView {
    pub pet_id: u64,
    pub owner_guid: u64,
    pub live_pet_guid: u64,
    pub creature_entry: u32,
    pub name: String,
    pub name_timestamp: u32,
    pub level: u32,
    pub pet_xp: u32,
    pub next_level_xp: u32,
    pub happiness: u32,
    pub loyalty_level: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PetNameView {
    pub pet_number: u32,
    pub name: String,
    pub name_timestamp: u32,
}

/// Publish only Hunter-specific fields after the live creature exists. Loyalty occupies byte 1 of
/// UNIT_FIELD_BYTES_1; happiness is the otherwise hidden POWER5 pair in build 5875.
pub fn build_hunter_pet_values(pet: &HunterPetProtocolView) -> SMSG_UPDATE_OBJECT {
    values::unit_values(pet.live_pet_guid, |unit| {
        unit.set_unit_level(pet.level as i32);
        unit.set_unit_power5(pet.happiness as i32);
        unit.set_unit_maxpower5(MAX_HAPPINESS as i32);
        unit.set_unit_bytes_1(0, pet.loyalty_level, 0, 0);
        unit.set_unit_petnumber(pet.pet_id as u32 as i32);
        unit.set_unit_pet_name_timestamp(pet.name_timestamp as i32);
        unit.set_unit_petexperience(pet.pet_xp as i32);
        unit.set_unit_petnextlevelexp(pet.next_level_xp as i32);
    })
}

pub fn build_pet_name_query_response(pet: &PetNameView) -> SMSG_PET_NAME_QUERY_RESPONSE {
    SMSG_PET_NAME_QUERY_RESPONSE {
        pet_number: pet.pet_number,
        name: pet.name.clone(),
        pet_name_timestamp: pet.name_timestamp,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_world_messages::vanilla::ServerMessage;

    fn hunter_pet() -> HunterPetProtocolView {
        HunterPetProtocolView {
            pet_id: 0x1234,
            owner_guid: 7,
            live_pet_guid: 0xF130_0000_0000_0007,
            creature_entry: 3098,
            name: "Mottled Boar".into(),
            name_timestamp: 1_700_000_000,
            level: 8,
            pet_xp: 321,
            next_level_xp: 4_500,
            happiness: 166_500,
            loyalty_level: 2,
        }
    }

    #[test]
    fn hunter_identity_uses_the_vanilla_pet_descriptor_slots() {
        let msg = build_hunter_pet_values(&hunter_pet());
        let Object::Values {
            mask1: UpdateMask::Unit(unit),
            ..
        } = &msg.objects[0]
        else {
            panic!("expected unit VALUES");
        };
        assert_eq!(unit.unit_level(), Some(8));
        assert_eq!(unit.unit_power5(), Some(166_500));
        assert_eq!(unit.unit_maxpower5(), Some(1_000_000));
        assert_eq!(unit.unit_bytes_1(), Some((0, 2, 0, 0)));
        assert_eq!(unit.unit_petnumber(), Some(0x1234));
        assert_eq!(
            unit.unit_pet_name_timestamp(),
            Some(1_700_000_000u32 as i32)
        );
        assert_eq!(unit.unit_petexperience(), Some(321));
        assert_eq!(unit.unit_petnextlevelexp(), Some(4_500));
        let mut bytes = Vec::new();
        msg.write_unencrypted_server(&mut bytes).unwrap();
        let mut tail = vec![
            5, // five update-mask words through UNIT_FIELD_PETNEXTLEVELEXP
            0x00, 0x00, 0x00, 0x08, // POWER5, descriptor 27
            0x06, 0x00, 0x00, 0x00, // MAXPOWER5 + LEVEL, descriptors 33–34
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x7c, 0x00,
            0x00, // BYTES_1 through PETNEXTLEVELEXP, 138–142
        ];
        for value in [
            166_500u32,
            1_000_000,
            8,
            0x0000_0200,
            0x1234,
            1_700_000_000,
            321,
            4_500,
        ] {
            tail.extend_from_slice(&value.to_le_bytes());
        }
        assert!(bytes.ends_with(&tail), "unexpected Hunter pet VALUES tail");
    }

    #[test]
    fn pet_name_response_preserves_number_name_and_timestamp() {
        let msg = build_pet_name_query_response(&PetNameView {
            pet_number: 0x1234,
            name: "Mottled Boar".into(),
            name_timestamp: 99,
        });
        assert_eq!(msg.pet_number, 0x1234);
        assert_eq!(msg.name, "Mottled Boar");
        assert_eq!(msg.pet_name_timestamp, 99);

        let mut bytes = Vec::new();
        msg.write_unencrypted_server(&mut bytes).unwrap();
        assert_eq!(
            bytes,
            vec![
                0x00, 0x17, 0x53, 0x00, 0x34, 0x12, 0x00, 0x00, b'M', b'o', b't', b't', b'l', b'e',
                b'd', b' ', b'B', b'o', b'a', b'r', 0x00, 99, 0, 0, 0,
            ]
        );
    }
}
