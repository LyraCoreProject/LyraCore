use lyracore_protocol::messages::vanilla::{
    opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
    Level, MSG_LIST_STABLED_PETS_Server, StableResult, StabledPet, SMSG_STABLE_RESULT,
};
use spacetimedb::{ConnectionId, ReducerContext};

use super::{Rollback, Unhandled};
use crate::creatures::stable::{self, BroughtOut, StableListing, StableRefusal};

fn result(result: StableResult) -> ServerOpcodeMessage {
    ServerOpcodeMessage::SMSG_STABLE_RESULT(SMSG_STABLE_RESULT { result })
}

// vmangos answers every refusal with STABLE_ERR_STABLE except a slot the Character cannot afford.
fn refused(refusal: impl std::fmt::Display, code: StableResult) -> Rollback {
    Rollback::reply(result(code), refusal.to_string())
}

fn stable_refused(refusal: StableRefusal) -> Rollback {
    let code = match refusal {
        StableRefusal::CannotAfford { .. } => StableResult::ErrMoney,
        _ => StableResult::ErrStable,
    };
    refused(refusal, code)
}

fn listing(npc: u64, listing: StableListing) -> ServerOpcodeMessage {
    ServerOpcodeMessage::MSG_LIST_STABLED_PETS(Box::new(MSG_LIST_STABLED_PETS_Server {
        npc: lyracore_protocol::messages::Guid::new(npc),
        stable_slots: listing.purchased_slots,
        pets: listing
            .pets
            .into_iter()
            .map(|pet| StabledPet {
                pet_number: pet.pet_number,
                entry: pet.creature_entry,
                level: Level::new(pet.level.min(u32::from(u8::MAX)) as u8),
                name: pet.name,
                loyalty: u32::from(pet.loyalty_level),
                slot: pet.client_slot,
            })
            .collect(),
    }))
}

/// vmangos `SendPetNameQuery` precedes the result for a pet a stable request built.
fn brought_out(
    ctx: &ReducerContext,
    connection: ConnectionId,
    pet: BroughtOut,
) -> Result<(), Rollback> {
    let code = match pet {
        BroughtOut::Built {
            live_pet_guid,
            pet_number,
        } => {
            if let Some(name) = super::query::pet_name(ctx, live_pet_guid, pet_number) {
                super::send(ctx, connection, name, false)?;
            }
            StableResult::SuccessUnstable
        }
        BroughtOut::Dead => StableResult::SuccessUnstable,
        BroughtOut::NotBuilt => StableResult::ErrStable,
    };
    Ok(super::send(ctx, connection, result(code), false)?)
}

pub(super) fn receive(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), Unhandled> {
    let npc = match &message {
        ClientOpcodeMessage::MSG_LIST_STABLED_PETS(request) => request.npc.guid(),
        ClientOpcodeMessage::CMSG_STABLE_PET(request) => request.stable_master.guid(),
        ClientOpcodeMessage::CMSG_UNSTABLE_PET(request) => request.stable_master.guid(),
        ClientOpcodeMessage::CMSG_BUY_STABLE_SLOT(request) => request.npc.guid(),
        ClientOpcodeMessage::CMSG_STABLE_SWAP_PET(request) => request.npc.guid(),
        _ => return Err(Unhandled::Declined(message)),
    };
    // vmangos `HandleListStabledPetsOpcode` answers a refused listing with nothing.
    if let ClientOpcodeMessage::MSG_LIST_STABLED_PETS(_) = message {
        super::actor(ctx, guid).map_err(Rollback::from)?;
        let listed = stable::list_stabled_pets(ctx, guid, npc)
            .map_err(|refusal| Rollback::silent(refusal.to_string()))?;
        return Ok(super::send(ctx, connection, listing(npc, listed), false)?);
    }
    super::actor(ctx, guid).map_err(|refusal| refused(refusal, StableResult::ErrStable))?;
    match message {
        ClientOpcodeMessage::CMSG_STABLE_PET(_) => {
            stable::stable_pet(ctx, guid, npc).map_err(stable_refused)?;
            Ok(super::send(
                ctx,
                connection,
                result(StableResult::SuccessStable),
                false,
            )?)
        }
        ClientOpcodeMessage::CMSG_UNSTABLE_PET(request) => {
            let pet =
                stable::unstable_pet(ctx, guid, npc, request.pet_number).map_err(stable_refused)?;
            Ok(brought_out(ctx, connection, pet)?)
        }
        ClientOpcodeMessage::CMSG_STABLE_SWAP_PET(request) => {
            let pet = stable::swap_pet(ctx, guid, npc, request.pet_slot).map_err(stable_refused)?;
            Ok(brought_out(ctx, connection, pet)?)
        }
        ClientOpcodeMessage::CMSG_BUY_STABLE_SLOT(_) => {
            stable::buy_stable_slot(ctx, guid, npc).map_err(stable_refused)?;
            Ok(super::send(
                ctx,
                connection,
                result(StableResult::SuccessBuySlot),
                false,
            )?)
        }
        other => Err(Unhandled::Declined(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::npc_interaction::{NpcGate, NpcRefusal, NpcService};

    fn reply_body(rollback: Rollback) -> Vec<(u16, Vec<u8>)> {
        let Rollback::Refusal { replies, .. } = rollback else {
            panic!("a Stable refusal is a reply, not a failure");
        };
        replies
            .into_iter()
            .map(|packet| (packet.opcode, packet.body))
            .collect()
    }

    #[test]
    fn a_stable_refusal_sends_err_stable_and_only_a_slot_it_cannot_afford_sends_err_money() {
        let out_of_range = StableRefusal::StableMaster(NpcRefusal {
            gate: NpcGate::OutOfRange,
            service: NpcService::StableMaster,
        });
        for refusal in [
            out_of_range,
            StableRefusal::NotHunter,
            StableRefusal::NoCurrentPet,
            StableRefusal::PetOut,
            StableRefusal::NotHunterPet,
            StableRefusal::NoSuchPet,
            StableRefusal::NotTameable,
            StableRefusal::NoFreeSlot,
            StableRefusal::AllSlotsBought,
        ] {
            assert_eq!(
                reply_body(stable_refused(refusal)),
                vec![(0x0273, vec![0x06])]
            );
        }
        assert_eq!(
            reply_body(stable_refused(StableRefusal::CannotAfford { price: 500 })),
            vec![(0x0273, vec![0x01])]
        );
    }

    #[test]
    fn the_listing_names_the_stable_master_then_counts_pets_and_bought_slots() {
        let packet = super::super::bounded(listing(
            0xF130_0000_0000_0007,
            StableListing {
                purchased_slots: 1,
                pets: vec![stable::ListedPet {
                    pet_number: 0x0102_0304,
                    creature_entry: 51008,
                    level: 12,
                    name: "Fang".to_owned(),
                    loyalty_level: 3,
                    client_slot: 2,
                }],
            },
        ));
        let mut body = 0xF130_0000_0000_0007u64.to_le_bytes().to_vec();
        body.extend([1, 1]);
        body.extend(0x0102_0304u32.to_le_bytes());
        body.extend(51008u32.to_le_bytes());
        body.extend(12u32.to_le_bytes());
        body.extend(b"Fang\0");
        body.extend(3u32.to_le_bytes());
        body.push(2);
        assert_eq!((packet.opcode, packet.body), (0x026F, body));
    }
}
