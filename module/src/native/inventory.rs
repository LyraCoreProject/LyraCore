use lyracore_protocol::{
    codec,
    messages::vanilla::opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
    world::Packet,
};
use lyracore_shared::item::ItemRefusal;
use spacetimedb::{ConnectionId, ReducerContext, Table};

use super::game_native_player;
use crate::items::item_in_slot;
use crate::{game_item_template, ItemInstance};

fn packet(message: ServerOpcodeMessage) -> Packet {
    lyracore_protocol::world::encode(message).expect("bounded item message fits a world frame")
}

fn update(message: lyracore_protocol::messages::vanilla::SMSG_UPDATE_OBJECT) -> Packet {
    packet(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(message)))
}

fn send(ctx: &ReducerContext, connection: ConnectionId, packets: Vec<Packet>) {
    for packet in packets {
        super::send_raw(ctx, connection, packet.opcode, packet.body, false);
    }
}

fn slot(
    ctx: &ReducerContext,
    guid: u64,
    slot: u8,
    view: &codec::ItemInstanceView,
    packets: &mut Vec<Packet>,
) {
    packets.extend(codec::build_inv_slot_values(guid, slot, view.guid).map(update));
    if let Some(values) = codec::build_visible_item_values(guid, slot, view) {
        let packet = update(values);
        super::visibility::broadcast(ctx, guid, packet.opcode, packet.body.clone(), true);
        packets.push(packet);
    }
    if let Some((bag_slot, position)) = codec::bag_content_parts(slot) {
        if let Some(bag) = item_in_slot(ctx, guid, bag_slot) {
            let (opcode, body) = codec::build_container_slot_values(bag.guid, position, view.guid);
            packets.push(Packet { opcode, body });
        }
    }
}

fn clear_slot(ctx: &ReducerContext, item: &ItemInstance, packets: &mut Vec<Packet>) {
    // A swap can already have filled the vacated slot with another item.
    if item_in_slot(ctx, item.owner_guid, item.slot).is_none() {
        slot(
            ctx,
            item.owner_guid,
            item.slot,
            &codec::ItemInstanceView::default(),
            packets,
        );
    }
}

fn gain(
    ctx: &ReducerContext,
    row: &ItemInstance,
    count: u32,
    stack_add: bool,
    packets: &mut Vec<Packet>,
) {
    if count == 0 {
        return;
    }
    let (bag, position) = codec::bag_content_parts(row.slot).unwrap_or((255, row.slot));
    packets.push(packet(ServerOpcodeMessage::SMSG_ITEM_PUSH_RESULT(
        Box::new(codec::build_item_push_result(
            row.owner_guid,
            bag,
            u32::from(position),
            row.entry,
            count,
            stack_add,
            row.random_property_id,
        )),
    )));
    use crate::quest::{game_character_quest, game_quest_objective};
    if ctx
        .db
        .game_character_quest()
        .by_character()
        .filter(row.owner_guid)
        .any(|quest| {
            !quest.rewarded
                && !quest.failed
                && ctx
                    .db
                    .game_quest_objective()
                    .by_quest()
                    .filter(quest.quest_entry)
                    .any(|objective| objective.kind == 1 && objective.target_entry == row.entry)
        })
    {
        packets.push(packet(ServerOpcodeMessage::SMSG_QUESTUPDATE_ADD_ITEM(
            lyracore_protocol::messages::vanilla::SMSG_QUESTUPDATE_ADD_ITEM {
                required_item_id: row.entry,
                items_required: count,
            },
        )));
    }
}

pub(crate) fn changed(
    ctx: &ReducerContext,
    previous: Option<&ItemInstance>,
    current: Option<&ItemInstance>,
) {
    if ctx.db.game_native_player().count() == 0 {
        return;
    }
    if previous.zip(current).is_some_and(|(old, row)| {
        old.owner_guid == row.owner_guid
            && old.slot == row.slot
            && old.stack_count == row.stack_count
            && old.durability == row.durability
            && old.enchant_id == row.enchant_id
            && old.random_property_id == row.random_property_id
            && old.item_text_id == row.item_text_id
            && old.entry == row.entry
    }) {
        return;
    }
    if previous
        .zip(current)
        .is_some_and(|(old, row)| old.owner_guid != row.owner_guid)
    {
        changed(ctx, previous, None);
        changed(ctx, None, current);
        return;
    }
    let Some(row) = current.or(previous) else {
        return;
    };
    let Some(owner) = ctx
        .db
        .game_native_player()
        .character_guid()
        .find(row.owner_guid)
    else {
        return;
    };
    let mut packets = Vec::new();
    match (previous, current) {
        (None, Some(row)) => {
            let view = super::view::item(ctx, row);
            packets.push(update(codec::build_item_create_object(&view)));
            if let Some((opcode, body)) = codec::build_random_property_values(&view) {
                packets.push(Packet { opcode, body });
            }
            slot(ctx, row.owner_guid, row.slot, &view, &mut packets);
            gain(ctx, row, row.stack_count, false, &mut packets);
        }
        (Some(old), None) => {
            clear_slot(ctx, old, &mut packets);
            packets.push(packet(ServerOpcodeMessage::SMSG_DESTROY_OBJECT(
                codec::build_destroy_object(old.guid),
            )));
        }
        (Some(old), Some(row)) => {
            let view = super::view::item(ctx, row);
            if old.entry != row.entry || old.random_property_id != row.random_property_id {
                packets.push(packet(ServerOpcodeMessage::SMSG_DESTROY_OBJECT(
                    codec::build_destroy_object(row.guid),
                )));
                packets.push(update(codec::build_item_create_object(&view)));
                if let Some((opcode, body)) = codec::build_random_property_values(&view) {
                    packets.push(Packet { opcode, body });
                }
                if old.slot == row.slot {
                    slot(ctx, row.owner_guid, row.slot, &view, &mut packets);
                }
            }
            if old.slot != row.slot {
                clear_slot(ctx, old, &mut packets);
                slot(ctx, row.owner_guid, row.slot, &view, &mut packets);
            }
            if old.enchant_id != row.enchant_id {
                if row.entry == lyracore_shared::guild::GUILD_CHARTER_ENTRY {
                    let (opcode, body) =
                        codec::build_charter_petition_values(row.guid, row.enchant_id);
                    packets.push(Packet { opcode, body });
                } else {
                    packets.push(update(codec::build_item_enchantment_values(
                        row.guid,
                        view.enchantment,
                    )));
                    if old.slot == row.slot {
                        slot(ctx, row.owner_guid, row.slot, &view, &mut packets);
                    }
                }
            }
            if old.stack_count != row.stack_count || old.durability != row.durability {
                packets.push(update(codec::build_item_values(
                    row.guid,
                    row.stack_count,
                    row.durability,
                )));
            }
            if old.slot == row.slot {
                gain(
                    ctx,
                    row,
                    row.stack_count.saturating_sub(old.stack_count),
                    true,
                    &mut packets,
                );
            }
        }
        (None, None) => unreachable!(),
    }
    let affects_resistances = match (previous, current) {
        (Some(old), Some(row)) => {
            (old.slot <= 18 || row.slot <= 18)
                && (old.slot != row.slot
                    || (old.durability == 0) != (row.durability == 0)
                    || old.enchant_id != row.enchant_id
                    || old.random_property_id != row.random_property_id)
        }
        _ => row.slot <= 18,
    };
    if affects_resistances {
        let auras: Vec<_> = crate::spell::auras_on(ctx, row.owner_guid).collect();
        packets.extend(super::aura::armor(ctx, row.owner_guid, &auras));
    }
    send(ctx, owner.connection_id, packets);
    if previous.map(|row| (row.entry, row.stack_count))
        != current.map(|row| (row.entry, row.stack_count))
    {
        super::quest::item_changed(ctx, row.owner_guid, row.entry);
        if let Some(old) = previous.filter(|old| old.entry != row.entry) {
            super::quest::item_changed(ctx, old.owner_guid, old.entry);
        }
    }
}

pub(super) fn receive(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), String> {
    let slots = match &message {
        ClientOpcodeMessage::CMSG_DESTROYITEM(c) => [codec::inventory_slot(c.bag, c.slot), None],
        ClientOpcodeMessage::CMSG_SPLIT_ITEM(c) => [
            codec::inventory_slot(c.source_bag, c.source_slot),
            codec::inventory_slot(c.destination_bag, c.destination_slot),
        ],
        ClientOpcodeMessage::CMSG_AUTOEQUIP_ITEM(c) => {
            [codec::inventory_slot(c.source_bag, c.source_slot), None]
        }
        ClientOpcodeMessage::CMSG_AUTOSTORE_BAG_ITEM(c) => {
            [codec::inventory_slot(c.source_bag, c.source_slot), None]
        }
        ClientOpcodeMessage::CMSG_SWAP_INV_ITEM(c) => [
            Some(c.source_slot.as_int()),
            Some(c.destination_slot.as_int()),
        ],
        ClientOpcodeMessage::CMSG_SWAP_ITEM(c) => [
            codec::inventory_slot(c.source_bag, c.source_slot),
            codec::inventory_slot(c.destination_bag, c.destionation_slot),
        ],
        ClientOpcodeMessage::CMSG_USE_ITEM(c) => {
            [codec::inventory_slot(c.bag_index, c.bag_slot), None]
        }
        ClientOpcodeMessage::CMSG_READ_ITEM(c) => {
            [codec::inventory_slot(c.bag_index, c.slot), None]
        }
        ClientOpcodeMessage::CMSG_ITEM_NAME_QUERY(c) => {
            if let Some(template) = ctx.db.game_item_template().entry().find(c.item) {
                super::send(
                    ctx,
                    connection,
                    ServerOpcodeMessage::SMSG_ITEM_NAME_QUERY_RESPONSE(Box::new(
                        codec::build_item_name_query_response(c.item, template.name),
                    )),
                    false,
                )?;
            }
            return Ok(());
        }
        _ => return Err("invalid inventory opcode".into()),
    };
    let item_guids = slots.map(|slot| {
        slot.and_then(|slot| item_in_slot(ctx, guid, slot))
            .map_or(0, |item| item.guid)
    });
    let result = (|| {
        super::actor(ctx, guid).map_err(|_| ItemRefusal::NotRightNow)?;
        let source = slots[0].ok_or(ItemRefusal::WrongSlot)?;
        match message {
            ClientOpcodeMessage::CMSG_DESTROYITEM(c) => {
                crate::items::apply_item_destroy(ctx, guid, source, u32::from(c.amount))
            }
            ClientOpcodeMessage::CMSG_SPLIT_ITEM(c) => crate::items::apply_item_split(
                ctx,
                guid,
                source,
                u32::from(c.amount),
                slots[1].ok_or(ItemRefusal::WrongSlot)?,
            ),
            ClientOpcodeMessage::CMSG_AUTOEQUIP_ITEM(_) => {
                crate::items::apply_equip_item(ctx, guid, source)
            }
            ClientOpcodeMessage::CMSG_AUTOSTORE_BAG_ITEM(c)
                if c.source_bag == 255 && c.destination_bag == 255 && source <= 18 =>
            {
                crate::items::apply_unequip_item(ctx, guid, source)
            }
            ClientOpcodeMessage::CMSG_AUTOSTORE_BAG_ITEM(_) => Err(ItemRefusal::NotRightNow),
            ClientOpcodeMessage::CMSG_SWAP_INV_ITEM(_) | ClientOpcodeMessage::CMSG_SWAP_ITEM(_) => {
                crate::items::apply_item_move(
                    ctx,
                    guid,
                    source,
                    slots[1].ok_or(ItemRefusal::WrongSlot)?,
                )
            }
            ClientOpcodeMessage::CMSG_USE_ITEM(_) => {
                if let Some(item) = item_in_slot(ctx, guid, source) {
                    if let Some(template) = ctx
                        .db
                        .game_item_template()
                        .entry()
                        .find(item.entry)
                        .filter(|t| t.start_quest != 0)
                    {
                        if let Some(detail) =
                            super::quest::quest_detail_view(ctx, template.start_quest)
                        {
                            let (opcode, body) = codec::build_quest_details_raw(item.guid, &detail);
                            super::send_raw(ctx, connection, opcode, body, false);
                        }
                        return Ok(());
                    }
                }
                crate::items::apply_item_use(ctx, guid, source)
            }
            ClientOpcodeMessage::CMSG_READ_ITEM(_) => {
                let item = item_in_slot(ctx, guid, source).ok_or(ItemRefusal::ItemNotFound)?;
                ctx.db
                    .game_item_template()
                    .entry()
                    .find(item.entry)
                    .filter(|t| t.page_text != 0)
                    .ok_or(ItemRefusal::ItemNotFound)?;
                let (opcode, body) = codec::build_read_item_failed_raw(item.guid);
                super::send_raw(ctx, connection, opcode, body, false);
                super::send(
                    ctx,
                    connection,
                    ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(
                        codec::build_gm_system_message(
                            "Item page text is not available on this realm.".into(),
                        ),
                    )),
                    false,
                )
                .map_err(|_| ItemRefusal::Internal)
            }
            _ => Err(ItemRefusal::Internal),
        }
    })();
    result.map_err(|reason| {
        super::rollback_reply(
            ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(Box::new(
                codec::build_inventory_refusal(reason, item_guids[0], item_guids[1]),
            )),
            reason.as_tag().into(),
        )
    })
}
