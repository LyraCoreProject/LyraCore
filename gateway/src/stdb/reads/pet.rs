use super::*;
use crate::codec::PetNameView;

fn resolve_pet_name(
    owner_guid: u64,
    pet_number: u32,
    pet_guid: u64,
    hunter: Option<crate::codec::HunterPetProtocolView>,
    summoned: Option<(u64, String)>,
) -> Option<PetNameView> {
    if let Some(pet) = hunter {
        return (pet.owner_guid == owner_guid
            && pet.live_pet_guid == pet_guid
            && pet.pet_id as u32 == pet_number)
            .then_some(PetNameView {
                pet_number,
                name: pet.name,
                name_timestamp: pet.name_timestamp,
            });
    }
    let (summoned_owner, name) = summoned?;
    (summoned_owner == owner_guid).then_some(PetNameView {
        pet_number,
        name,
        name_timestamp: 0,
    })
}

impl super::super::connection::Coordinator {
    /// Resolve only the requesting player's live pet. Hunter names come from the bounded durable
    /// projection; summoned-pet names remain authored creature-template data.
    pub fn pet_name(
        &self,
        owner_guid: u64,
        pet_number: u32,
        pet_guid: u64,
    ) -> Result<Option<PetNameView>, anyhow::Error> {
        if owner_guid == 0 || pet_guid == 0 {
            return Ok(None);
        }
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let Some(entity) = db.game_world_entity().guid().find(&pet_guid) else {
            return Ok(None);
        };
        if entity.owner_guid != owner_guid {
            return Ok(None);
        }
        if let Some(pet) = db
            .game_hunter_pet_protocol()
            .iter()
            .find(|pet| pet.owner_guid == owner_guid && pet.live_pet_guid == pet_guid)
        {
            return Ok(resolve_pet_name(
                owner_guid,
                pet_number,
                pet_guid,
                Some(super::super::views::hunter_pet_protocol_view(pet)),
                None,
            ));
        }
        let summoned = db
            .game_creature_template()
            .entry()
            .find(&entity.entry)
            .map(|template| (entity.owner_guid, template.name));
        Ok(resolve_pet_name(
            owner_guid, pet_number, pet_guid, None, summoned,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunter(owner_guid: u64) -> crate::codec::HunterPetProtocolView {
        crate::codec::HunterPetProtocolView {
            pet_id: 77,
            owner_guid,
            live_pet_guid: 99,
            creature_entry: 3098,
            name: "Mottled Boar".into(),
            name_timestamp: 123,
            level: 8,
            pet_xp: 0,
            next_level_xp: 4_500,
            happiness: 166_500,
            loyalty_level: 1,
        }
    }

    #[test]
    fn hunter_name_requires_owner_guid_and_pet_number() {
        let got = resolve_pet_name(7, 77, 99, Some(hunter(7)), None).unwrap();
        assert_eq!(got.name, "Mottled Boar");
        assert_eq!(got.name_timestamp, 123);
        assert!(resolve_pet_name(8, 77, 99, Some(hunter(7)), None).is_none());
        assert!(resolve_pet_name(7, 78, 99, Some(hunter(7)), None).is_none());
        assert!(resolve_pet_name(7, 77, 100, Some(hunter(7)), None).is_none());
    }

    #[test]
    fn summoned_pet_uses_authored_name_without_hunter_state() {
        let got = resolve_pet_name(7, 0, 99, None, Some((7, "Imp".into()))).unwrap();
        assert_eq!(got.name, "Imp");
        assert_eq!(got.name_timestamp, 0);
        assert!(resolve_pet_name(8, 0, 99, None, Some((7, "Imp".into()))).is_none());
    }
}
