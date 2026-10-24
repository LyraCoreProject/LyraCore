use lyracore_protocol::{codec, messages::vanilla::opcodes::ServerOpcodeMessage};
use spacetimedb::ReducerContext;

use crate::creatures::game_hunter_pet_protocol;
use crate::gameobject::game_gameobject_template;
use crate::items::game_item_template;
use crate::{
    game_creature_template, game_meeting_stone, game_npc_text, game_npc_text_slot,
    game_world_entity,
};

pub(super) fn creature(ctx: &ReducerContext, entry: u32) -> Option<ServerOpcodeMessage> {
    let row = ctx.db.game_creature_template().entry().find(entry)?;
    let view = codec::CreatureView {
        entry: row.entry,
        name: row.name,
        subname: row.subname,
        display_id: row.display_id,
        creature_type: u32::from(row.creature_type),
        creature_family: row.creature_family,
        type_flags: row.type_flags,
        rank: u32::from(row.rank),
    };
    Some(ServerOpcodeMessage::SMSG_CREATURE_QUERY_RESPONSE(Box::new(
        codec::build_creature_query_response(&view),
    )))
}

pub(super) fn gameobject(ctx: &ReducerContext, entry: u32) -> ServerOpcodeMessage {
    let view = ctx
        .db
        .game_gameobject_template()
        .entry()
        .find(entry)
        .map(|row| {
            let stone = (row.type_id == lyracore_shared::constants::go_type::MEETINGSTONE)
                .then(|| ctx.db.game_meeting_stone().entry().find(entry))
                .flatten();
            let (data0, data1, data2) = stone.map_or((row.data0, row.data1, 0), |stone| {
                (stone.min_level, stone.max_level, stone.area_id)
            });
            codec::GameObjectTemplateView {
                type_id: row.type_id,
                display_id: row.display_id,
                name: row.name,
                data0,
                data1,
                data2,
            }
        });
    ServerOpcodeMessage::SMSG_GAMEOBJECT_QUERY_RESPONSE(Box::new(
        codec::build_gameobject_query_response(entry, view.as_ref()),
    ))
}

pub(super) fn item(ctx: &ReducerContext, entry: u32) -> ServerOpcodeMessage {
    let view = ctx
        .db
        .game_item_template()
        .entry()
        .find(entry)
        .map(super::view::item_template);
    ServerOpcodeMessage::SMSG_ITEM_QUERY_SINGLE_RESPONSE(Box::new(
        codec::build_item_query_response(entry, view.as_ref()),
    ))
}

pub(super) fn npc_text(ctx: &ReducerContext, text_id: u32) -> ServerOpcodeMessage {
    let view = ctx.db.game_npc_text().text_id().find(text_id).map(|base| {
        let mut view = codec::NpcTextView::default();
        let mut slots: Vec<_> = ctx
            .db
            .game_npc_text_slot()
            .by_text_id()
            .filter(text_id)
            .collect();
        if slots.is_empty() {
            view.slots[0] = (base.text.clone(), base.text, 1.0);
        } else {
            slots.sort_by_key(|row| (row.slot_index, row.id));
            for row in slots {
                if let Some(slot) = view.slots.get_mut(usize::from(row.slot_index)) {
                    *slot = (row.text_male, row.text_female, row.probability);
                }
            }
        }
        view
    });
    ServerOpcodeMessage::SMSG_NPC_TEXT_UPDATE(Box::new(codec::build_npc_text_update(
        text_id,
        view.as_ref(),
    )))
}

pub(super) fn pet_name(
    ctx: &ReducerContext,
    guid: u64,
    pet_number: u32,
) -> Option<ServerOpcodeMessage> {
    let entity = ctx
        .db
        .game_world_entity()
        .guid()
        .find(guid)
        .filter(|entity| entity.owner_guid != 0)?;
    let view = if let Some(pet) = ctx
        .db
        .game_hunter_pet_protocol()
        .by_owner()
        .filter(entity.owner_guid)
        .find(|pet| pet.live_pet_guid == guid)
    {
        if pet.pet_id as u32 != pet_number {
            return None;
        }
        codec::PetNameView {
            pet_number,
            name: pet.name,
            name_timestamp: pet.name_timestamp,
        }
    } else {
        let template = ctx.db.game_creature_template().entry().find(entity.entry)?;
        codec::PetNameView {
            pet_number,
            name: template.name,
            name_timestamp: 0,
        }
    };
    Some(ServerOpcodeMessage::SMSG_PET_NAME_QUERY_RESPONSE(Box::new(
        codec::build_pet_name_query_response(&view),
    )))
}
