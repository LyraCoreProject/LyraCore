use spacetimedb::{table, ReducerContext, Table};

use super::hunter_pet::{
    hunter_pet_for_owner, is_tameable, live_pet_kind, materialize_beside, PetKind, PetPresence,
    CLASS_HUNTER, CURRENT_PET,
};
use crate::npc_interaction::{NpcRefusal, NpcService};
use crate::{game_hunter_pet, game_hunter_pet_protocol, HunterPet, WorldEntity};

// StableSlotPrices.dbc in copper, indexed by the slot bought less one; vmangos MAX_PET_STABLES = 2.
const STABLE_SLOT_PRICES: [u32; 2] = [500, 50_000];

/// The Stable slots a Character has bought; no row means none.
#[table(accessor = game_character_stable)]
pub struct CharacterStable {
    #[primary_key]
    pub character_guid: u64,
    pub purchased_slots: u8,
}

crate::outside_table_catalog!(game_character_stable);

crate::character_owned!(delete, fn sweep_delete_game_character_stable(ctx, character_guid) {
    ctx.db.game_character_stable().character_guid().delete(character_guid);
});
crate::character_owned!(transfer, fn sweep_transfer_game_character_stable(ctx, character_guid, io) {
    table = game_character_stable,
    primary_key = character_guid,
});

/// Why a Stable Master request changes nothing; vmangos `WorldSession::HandleStablePet` and peers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StableRefusal {
    StableMaster(NpcRefusal),
    NotHunter,
    NoCurrentPet,
    PetOut,
    NotHunterPet,
    NoSuchPet,
    NotTameable,
    NoFreeSlot,
    AllSlotsBought,
    CannotAfford { price: u32 },
}

impl std::fmt::Display for StableRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StableMaster(refusal) => refusal.fmt(f),
            Self::NotHunter => f.write_str("only a Hunter can use the stable"),
            Self::NoCurrentPet => f.write_str("the Hunter has no current pet"),
            Self::PetOut => f.write_str("the Hunter has a pet out"),
            Self::NotHunterPet => f.write_str("the live pet is not a Hunter pet"),
            Self::NoSuchPet => f.write_str("the Hunter has no such stabled pet"),
            Self::NotTameable => f.write_str("the pet's creature is not tameable"),
            Self::NoFreeSlot => f.write_str("every bought Stable slot holds a pet"),
            Self::AllSlotsBought => f.write_str("every Stable slot is bought"),
            Self::CannotAfford { price } => write!(f, "a Stable slot costs {price} copper"),
        }
    }
}

impl From<NpcRefusal> for StableRefusal {
    fn from(refusal: NpcRefusal) -> Self {
        Self::StableMaster(refusal)
    }
}

/// One MSG_LIST_STABLED_PETS entry; `client_slot` 1 is the current pet, 2 and 3 the Stable slots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ListedPet {
    pub(crate) pet_number: u32,
    pub(crate) creature_entry: u32,
    pub(crate) level: u32,
    pub(crate) name: String,
    pub(crate) loyalty_level: u8,
    pub(crate) client_slot: u8,
}

pub(crate) struct StableListing {
    pub(crate) purchased_slots: u8,
    pub(crate) pets: Vec<ListedPet>,
}

/// A pet a Stable request brought out; the owner is sent its name before the result.
pub(crate) struct BroughtOut {
    pub(crate) live_pet_guid: u64,
    pub(crate) pet_number: u32,
}

fn stable_master(
    ctx: &ReducerContext,
    owner_guid: u64,
    npc_guid: u64,
) -> Result<WorldEntity, StableRefusal> {
    let (owner, _) =
        crate::npc_interaction::gate(ctx, owner_guid, npc_guid, NpcService::StableMaster)?;
    if owner.class() != CLASS_HUNTER {
        return Err(StableRefusal::NotHunter);
    }
    Ok(owner)
}

fn owner_pets(ctx: &ReducerContext, owner_guid: u64) -> Vec<HunterPet> {
    ctx.db
        .game_hunter_pet()
        .by_owner()
        .filter(&owner_guid)
        .collect()
}

/// The pet number the client sees: vmangos pet numbers are 32 bits.
fn pet_number(pet: &HunterPet) -> u32 {
    pet.pet_id as u32
}

fn purchased_slots(ctx: &ReducerContext, owner_guid: u64) -> u8 {
    ctx.db
        .game_character_stable()
        .character_guid()
        .find(owner_guid)
        .map_or(0, |row| row.purchased_slots)
}

/// MSG_LIST_STABLED_PETS: the current pet, then the stabled pets in slot order.
pub(crate) fn list_stabled_pets(
    ctx: &ReducerContext,
    owner_guid: u64,
    npc_guid: u64,
) -> Result<StableListing, StableRefusal> {
    stable_master(ctx, owner_guid, npc_guid)?;
    let mut pets = owner_pets(ctx, owner_guid);
    pets.sort_by_key(|pet| (pet.stable_slot, pet.pet_id));
    Ok(StableListing {
        purchased_slots: purchased_slots(ctx, owner_guid),
        pets: pets
            .into_iter()
            .map(|pet| ListedPet {
                pet_number: pet_number(&pet),
                creature_entry: pet.creature_entry,
                level: pet.level,
                loyalty_level: pet.loyalty_level,
                client_slot: pet.stable_slot + 1,
                name: pet.name,
            })
            .collect(),
    })
}

fn first_free_slot(pets: &[HunterPet], purchased: u8) -> Option<u8> {
    (1..=purchased).find(|slot| pets.iter().all(|pet| pet.stable_slot != *slot))
}

/// Puts `pet` in `slot`; a pet that was out is dismissed and its protocol row retired.
fn put_in_slot(ctx: &ReducerContext, mut pet: HunterPet, slot: u8) {
    if PetPresence::of(pet.presence) == PetPresence::Out {
        pet.presence = PetPresence::Dismissed.code();
    }
    pet.stable_slot = slot;
    ctx.db
        .game_hunter_pet_protocol()
        .pet_id()
        .delete(pet.pet_id);
    ctx.db.game_hunter_pet().pet_id().update(pet);
}

/// The live pet, which must be the owner's Hunter pet; `Ok(None)` when no pet is out.
fn live_hunter_pet(
    ctx: &ReducerContext,
    owner_guid: u64,
) -> Result<Option<WorldEntity>, StableRefusal> {
    match super::pet_of(ctx, owner_guid) {
        Some(live) if matches!(live_pet_kind(ctx, live.guid), PetKind::Hunter { .. }) => {
            Ok(Some(live))
        }
        Some(_) => Err(StableRefusal::NotHunterPet),
        None => Ok(None),
    }
}

/// CMSG_STABLE_PET: the current pet, out or not, moves to the first free bought slot.
pub(crate) fn stable_pet(
    ctx: &ReducerContext,
    owner_guid: u64,
    npc_guid: u64,
) -> Result<(), StableRefusal> {
    stable_master(ctx, owner_guid, npc_guid)?;
    let live = live_hunter_pet(ctx, owner_guid)?;
    let current = hunter_pet_for_owner(ctx, owner_guid).ok_or(StableRefusal::NoCurrentPet)?;
    if live.is_none() && !is_tameable(ctx, current.creature_entry) {
        return Err(StableRefusal::NotTameable);
    }
    let slot = first_free_slot(
        &owner_pets(ctx, owner_guid),
        purchased_slots(ctx, owner_guid),
    )
    .ok_or(StableRefusal::NoFreeSlot)?;
    if live.is_some() {
        super::despawn_pets_on_departure(ctx, owner_guid);
    }
    let current = hunter_pet_for_owner(ctx, owner_guid).ok_or(StableRefusal::NoCurrentPet)?;
    put_in_slot(ctx, current, slot);
    Ok(())
}

/// CMSG_UNSTABLE_PET: with no pet out, a stabled pet becomes current and the current one takes
/// its slot. A dead pet stays dismissed; a living one is built beside its owner.
pub(crate) fn unstable_pet(
    ctx: &ReducerContext,
    owner_guid: u64,
    npc_guid: u64,
    requested: u32,
) -> Result<Option<BroughtOut>, StableRefusal> {
    let owner = stable_master(ctx, owner_guid, npc_guid)?;
    if super::pet_of(ctx, owner_guid).is_some() {
        return Err(StableRefusal::PetOut);
    }
    let pet = owner_pets(ctx, owner_guid)
        .into_iter()
        .find(|pet| pet.stable_slot != CURRENT_PET && pet_number(pet) == requested)
        .ok_or(StableRefusal::NoSuchPet)?;
    if !is_tameable(ctx, pet.creature_entry) {
        return Err(StableRefusal::NotTameable);
    }
    Ok(bring_out(ctx, &owner, pet.pet_id))
}

/// CMSG_STABLE_SWAP_PET: the current pet, out or not, takes the slot of the requested pet, which
/// becomes current. vmangos looks the requested pet up by number in any slot.
pub(crate) fn swap_pet(
    ctx: &ReducerContext,
    owner_guid: u64,
    npc_guid: u64,
    requested: u32,
) -> Result<Option<BroughtOut>, StableRefusal> {
    let owner = stable_master(ctx, owner_guid, npc_guid)?;
    let live = live_hunter_pet(ctx, owner_guid)?;
    if live.is_none() {
        let current = hunter_pet_for_owner(ctx, owner_guid).ok_or(StableRefusal::NoCurrentPet)?;
        if !is_tameable(ctx, current.creature_entry) {
            return Err(StableRefusal::NotTameable);
        }
    }
    let pet = owner_pets(ctx, owner_guid)
        .into_iter()
        .find(|pet| pet_number(pet) == requested)
        .ok_or(StableRefusal::NoSuchPet)?;
    if !is_tameable(ctx, pet.creature_entry) {
        return Err(StableRefusal::NotTameable);
    }
    if live.is_some() {
        super::despawn_pets_on_departure(ctx, owner_guid);
    }
    Ok(bring_out(ctx, &owner, pet.pet_id))
}

/// Makes `pet_id` the current pet; the previous current pet takes its slot.
fn bring_out(ctx: &ReducerContext, owner: &WorldEntity, pet_id: u64) -> Option<BroughtOut> {
    let pets = ctx.db.game_hunter_pet();
    let mut pet = pets.pet_id().find(pet_id)?;
    if let Some(current) = hunter_pet_for_owner(ctx, owner.guid).filter(|row| row.pet_id != pet_id)
    {
        put_in_slot(ctx, current, pet.stable_slot);
    }
    pet.stable_slot = CURRENT_PET;
    if PetPresence::of(pet.presence) == PetPresence::Dead {
        pets.pet_id().update(pet);
        return None;
    }
    let pet = pets.pet_id().update(pet);
    let number = pet_number(&pet);
    materialize_beside(ctx, owner, pet, None);
    super::pet_of(ctx, owner.guid).map(|live| BroughtOut {
        live_pet_guid: live.guid,
        pet_number: number,
    })
}

/// CMSG_BUY_STABLE_SLOT: the next slot costs its StableSlotPrices.dbc price.
pub(crate) fn buy_stable_slot(
    ctx: &ReducerContext,
    owner_guid: u64,
    npc_guid: u64,
) -> Result<(), StableRefusal> {
    let mut owner = stable_master(ctx, owner_guid, npc_guid)?;
    let purchased = purchased_slots(ctx, owner_guid);
    let price = *STABLE_SLOT_PRICES
        .get(usize::from(purchased))
        .ok_or(StableRefusal::AllSlotsBought)?;
    if owner.money < price {
        return Err(StableRefusal::CannotAfford { price });
    }
    owner.money -= price;
    owner.update(ctx);
    let row = CharacterStable {
        character_guid: owner_guid,
        purchased_slots: purchased + 1,
    };
    let stables = ctx.db.game_character_stable();
    if purchased == 0 {
        stables.insert(row);
    } else {
        stables.character_guid().update(row);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pet_in(slot: u8) -> HunterPet {
        let at = spacetimedb::Timestamp::from_micros_since_unix_epoch(0);
        HunterPet {
            pet_id: u64::from(slot) + 10,
            owner_guid: 1,
            creature_entry: 51008,
            creature_family: 5,
            name: "Fang".to_owned(),
            rename_allowed: false,
            level: 10,
            pet_xp: 0,
            loyalty_level: 1,
            loyalty_progress: 0,
            happiness: 0,
            created_at: at,
            care_updated_at: at,
            loyalty_updated_at: at,
            name_timestamp: 0,
            presence: 0,
            display_id: 0,
            saved_health: 0,
            saved_power: 0,
            stable_slot: slot,
        }
    }

    #[test]
    fn a_pet_takes_the_first_bought_slot_no_pet_holds() {
        assert_eq!(first_free_slot(&[pet_in(CURRENT_PET)], 0), None);
        assert_eq!(first_free_slot(&[pet_in(CURRENT_PET)], 2), Some(1));
        assert_eq!(first_free_slot(&[pet_in(1)], 1), None);
        assert_eq!(first_free_slot(&[pet_in(1)], 2), Some(2));
        assert_eq!(first_free_slot(&[pet_in(2)], 2), Some(1));
        assert_eq!(first_free_slot(&[pet_in(1), pet_in(2)], 2), None);
    }
}
