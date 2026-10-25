use lyracore_protocol::{codec, messages::vanilla::opcodes::ServerOpcodeMessage};
use lyracore_shared::pet::{pet_guid_for, PET_NAME_INVALID};
use spacetimedb::{ConnectionId, ReducerContext, Table};

use super::{game_native_player, NativeAoi};
use crate::{
    aoi::{game_aoi_grant, WORLD_ENTITY},
    creatures::{game_hunter_pet_protocol, HunterPetProtocol},
    game_creature_cast, game_world_entity,
};

pub(super) fn rename(ctx: &ReducerContext, owner: u64, pet: u64, name: &str) -> Result<(), String> {
    super::actor(ctx, owner).map_err(super::rollback_silent)?;
    crate::creatures::rename_hunter_pet(ctx, owner, pet, name).map_err(|cause| {
        if cause == PET_NAME_INVALID {
            super::rollback_reply(ServerOpcodeMessage::SMSG_PET_NAME_INVALID, cause)
        } else {
            super::rollback_silent(cause)
        }
    })
}

fn values(row: &HunterPetProtocol, owner: bool) -> ServerOpcodeMessage {
    let pet = codec::HunterPetProtocolView {
        pet_id: row.pet_id,
        owner_guid: row.owner_guid,
        live_pet_guid: row.live_pet_guid,
        creature_entry: row.creature_entry,
        name: row.name.clone(),
        name_timestamp: row.name_timestamp,
        level: row.level,
        pet_xp: row.pet_xp,
        next_level_xp: row.next_level_xp,
        happiness: row.happiness,
        loyalty_level: row.loyalty_level,
    };
    ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(if owner {
        codec::build_hunter_pet_values(&pet)
    } else {
        codec::build_hunter_pet_identity_values(&pet)
    }))
}

pub(super) fn replay(
    ctx: &ReducerContext,
    connection: ConnectionId,
    viewer: u64,
    guid: u64,
) -> Result<(), String> {
    let Some(entity) = ctx
        .db
        .game_world_entity()
        .guid()
        .find(guid)
        .filter(|entity| entity.owner_guid != 0 && entity.guid == pet_guid_for(entity.owner_guid))
    else {
        return Ok(());
    };
    let owner = entity.owner_guid == viewer;
    if let Some(pet) = ctx
        .db
        .game_hunter_pet_protocol()
        .by_owner()
        .filter(entity.owner_guid)
        .find(|pet| pet.live_pet_guid == guid)
    {
        super::send(ctx, connection, values(&pet, owner), false)?;
    }
    if owner {
        super::send(
            ctx,
            connection,
            ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(codec::build_owner_summon_values(
                viewer, guid,
            ))),
            false,
        )?;
        let spells: Vec<_> = ctx
            .db
            .game_creature_cast()
            .creature_entry()
            .find(entity.entry)
            .into_iter()
            .map(|cast| cast.spell_id)
            .collect();
        super::send(
            ctx,
            connection,
            ServerOpcodeMessage::SMSG_PET_SPELLS(Box::new(codec::build_pet_spells(guid, &spells))),
            false,
        )?;
    }
    Ok(())
}

pub(super) fn removed(ctx: &ReducerContext, connection: ConnectionId, viewer: u64, guid: u64) {
    if guid != pet_guid_for(viewer) {
        return;
    }
    for packet in [
        ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(codec::build_owner_summon_values(
            viewer, 0,
        ))),
        ServerOpcodeMessage::SMSG_PET_SPELLS(Box::new(codec::build_pet_spells_clear())),
    ] {
        super::send(ctx, connection, packet, false)
            .expect("bounded pet removal fits a world frame");
    }
}

pub(crate) fn changed(
    ctx: &ReducerContext,
    previous: Option<&HunterPetProtocol>,
    row: &HunterPetProtocol,
) {
    if ctx.db.game_native_player().count() == 0 {
        return;
    }
    let owner = ctx
        .db
        .game_native_player()
        .character_guid()
        .find(row.owner_guid);
    if let Some(owner) = &owner {
        if ctx
            .db
            .game_aoi_grant()
            .by_viewer()
            .filter(row.owner_guid)
            .any(|grant| grant.layer == WORLD_ENTITY && grant.entity_guid == row.live_pet_guid)
        {
            super::send(ctx, owner.connection_id, values(row, true), false)
                .expect("bounded pet values fit a world frame");
        }
    }
    if previous.is_none_or(|old| {
        old.pet_id != row.pet_id
            || old.live_pet_guid != row.live_pet_guid
            || old.name_timestamp != row.name_timestamp
    }) {
        let packet = lyracore_protocol::world::encode(values(row, false))
            .expect("bounded pet identity fits a world frame");
        super::visibility::broadcast_aoi(
            ctx,
            NativeAoi {
                layer: WORLD_ENTITY,
                entity_guid: row.live_pet_guid,
                exclude_connection: owner.map(|owner| owner.connection_id),
            },
            packet.opcode,
            packet.body,
        );
    }
}
