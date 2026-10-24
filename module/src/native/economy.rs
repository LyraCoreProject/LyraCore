use lyracore_protocol::{
    codec,
    messages::vanilla::opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
};
use lyracore_shared::constants::npc_flags;
use spacetimedb::{ConnectionId, ReducerContext};

use crate::items::{
    game_character_buyback, game_item_instance, game_item_random_property, game_item_template,
    game_npc_vendor, BuybackEntry,
};

fn interaction(
    ctx: &ReducerContext,
    guid: u64,
    npc: u64,
    flag: u32,
) -> Result<crate::WorldEntity, String> {
    super::actor(ctx, guid)?;
    crate::items::npc_interaction_gate(
        ctx,
        guid,
        npc,
        flag,
        "NPC",
        "NPC does not offer that service",
        "use this service",
    )
    .map(|(_, npc)| npc)
}

pub(super) fn vendor(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    npc: u64,
) -> Result<(), String> {
    let npc = interaction(ctx, guid, npc, npc_flags::VENDOR)?;
    let mut stock: Vec<_> = ctx
        .db
        .game_npc_vendor()
        .by_vendor()
        .filter(npc.entry)
        .collect();
    stock.sort_by_key(|row| row.slot);
    let discount = crate::reputation::vendor_discount_pct(ctx, guid, npc.faction_template);
    let items = stock
        .into_iter()
        .filter_map(|row| {
            ctx.db
                .game_item_template()
                .entry()
                .find(row.item_entry)
                .map(|item| codec::VendorItemView {
                    item_entry: item.entry,
                    display_id: item.display_id,
                    buy_price: item.buy_price - item.buy_price.saturating_mul(discount) / 100,
                    max_durability: item.max_durability,
                    max_count: row.max_count,
                    buy_count: item.buy_count,
                })
        })
        .collect::<Vec<_>>();
    let (opcode, body) = codec::build_list_inventory_raw(npc.guid, &items);
    super::send_raw(ctx, connection, opcode, body, false);
    Ok(())
}

pub(super) fn bank(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    npc: u64,
) -> Result<(), String> {
    interaction(ctx, guid, npc, npc_flags::BANKER)?;
    super::send(
        ctx,
        connection,
        ServerOpcodeMessage::SMSG_SHOW_BANK(codec::build_show_bank(npc)),
        false,
    )
}

fn buyback(ctx: &ReducerContext, guid: u64) -> Vec<BuybackEntry> {
    let mut entries: Vec<_> = ctx
        .db
        .game_character_buyback()
        .by_player_guid()
        .filter(guid)
        .collect();
    entries.sort_by_key(|row| std::cmp::Reverse(row.id));
    entries
}

pub(super) fn replay_buyback(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
) -> Result<(), String> {
    let entries = buyback(ctx, guid);
    if !entries.is_empty() {
        send_buyback(ctx, connection, guid, &entries, 0)?;
    }
    Ok(())
}

fn send_buyback(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    entries: &[BuybackEntry],
    previous_count: usize,
) -> Result<(), String> {
    let mut mask = codec::update_mask::UpdateMaskValues::new();
    for index in 0..12u16 {
        let wire_slot = 69 + index;
        let item_guid = 0x4090_0000_0000_0000 | u64::from(index);
        if usize::from(index) < previous_count {
            super::send(
                ctx,
                connection,
                ServerOpcodeMessage::SMSG_DESTROY_OBJECT(codec::build_destroy_object(item_guid)),
                false,
            )?;
        }
        let (item_guid, price) = if let Some(row) = entries.get(usize::from(index)) {
            let view = codec::ItemInstanceView {
                guid: item_guid,
                entry: row.item_entry,
                owner_guid: guid,
                slot: wire_slot as u8,
                stack_count: row.stack_count,
                random_property_id: row.random_property_id,
                random_property_enchant_ids: ctx
                    .db
                    .game_item_random_property()
                    .property_id()
                    .find(row.random_property_id)
                    .map_or([0; 3], |p| [p.enchant_id_1, p.enchant_id_2, p.enchant_id_3]),
                ..Default::default()
            };
            super::send(
                ctx,
                connection,
                ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(codec::build_item_create_object(
                    &view,
                ))),
                false,
            )?;
            if let Some((opcode, body)) = codec::build_random_property_values(&view) {
                super::send_raw(ctx, connection, opcode, body, false);
            }
            (item_guid, row.price)
        } else {
            (0, 0)
        };
        // Build 5875 fields: INV_SLOT base 486, BUYBACK_PRICE_1 1226, BUYBACK_TIMESTAMP_1 1238.
        mask.set_u64(486 + wire_slot * 2, item_guid);
        mask.set_u32(1226 + index, price);
        mask.set_u32(1238 + index, 0);
    }
    let (opcode, body) = codec::build_values_update_raw(guid, &mask);
    super::send_raw(ctx, connection, opcode, body, false);
    Ok(())
}

fn owned_slot(ctx: &ReducerContext, guid: u64, item: u64) -> Result<u8, String> {
    ctx.db
        .game_item_instance()
        .guid()
        .find(item)
        .filter(|row| row.owner_guid == guid)
        .map(|row| row.slot)
        .ok_or_else(|| "Character does not own that item".into())
}

pub(super) fn receive(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), String> {
    match message {
        ClientOpcodeMessage::CMSG_LIST_INVENTORY(c) => {
            vendor(ctx, connection, guid, c.guid.guid()).map_err(super::rollback_silent)
        }
        ClientOpcodeMessage::CMSG_BANKER_ACTIVATE(c) => {
            bank(ctx, connection, guid, c.guid.guid()).map_err(super::rollback_silent)
        }
        ClientOpcodeMessage::CMSG_BUY_ITEM(c) => {
            let vendor = c.vendor.guid();
            super::actor(ctx, guid)
                .and_then(|_| {
                    crate::items::apply_buy_item(ctx, guid, vendor, c.item, u32::from(c.amount))
                })
                .map_err(|reason| {
                    super::rollback_reply(
                        ServerOpcodeMessage::SMSG_BUY_FAILED(Box::new(codec::build_buy_failed(
                            vendor, c.item, &reason,
                        ))),
                        reason,
                    )
                })
        }
        ClientOpcodeMessage::CMSG_SELL_ITEM(c) => {
            let previous = buyback(ctx, guid).len();
            let result = super::actor(ctx, guid)
                .and_then(|_| owned_slot(ctx, guid, c.item.guid()))
                .and_then(|slot| {
                    crate::items::apply_item_sell_count(
                        ctx,
                        guid,
                        c.vendor.guid(),
                        slot,
                        u32::from(c.amount),
                    )
                });
            result.map_err(|reason| {
                super::rollback_reply(
                    ServerOpcodeMessage::SMSG_SELL_ITEM(Box::new(codec::build_sell_item_refusal(
                        c.vendor.guid(),
                        c.item.guid(),
                        codec::sell_refusal_result(&reason),
                    ))),
                    reason,
                )
            })?;
            send_buyback(ctx, connection, guid, &buyback(ctx, guid), previous)
        }
        ClientOpcodeMessage::CMSG_BUYBACK_ITEM(c) => {
            let previous = buyback(ctx, guid).len();
            let slot = c.slot.as_int();
            if !(69..81).contains(&slot) {
                return Ok(());
            }
            super::actor(ctx, guid)
                .and_then(|_| {
                    crate::items::apply_buyback_item(ctx, guid, c.guid.guid(), (slot - 69) as u8)
                })
                .map_err(super::rollback_silent)?;
            send_buyback(ctx, connection, guid, &buyback(ctx, guid), previous)
        }
        ClientOpcodeMessage::CMSG_REPAIR_ITEM(c) => {
            let result = super::actor(ctx, guid)
                .and_then(|_| {
                    if c.item.guid() == 0 {
                        Ok(u8::MAX)
                    } else {
                        owned_slot(ctx, guid, c.item.guid())
                    }
                })
                .and_then(|slot| crate::items::apply_player_repair(ctx, guid, c.npc.guid(), slot));
            result.map_err(|reason| {
                super::rollback_reply(
                    ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(
                        codec::build_gm_system_message(reason.clone()),
                    )),
                    reason,
                )
            })
        }
        ClientOpcodeMessage::CMSG_BUY_BANK_SLOT(c) => {
            let result = super::actor(ctx, guid).and_then(|_| {
                crate::items::buy_bank_slot_result(crate::items::apply_buy_bank_slot(
                    ctx,
                    guid,
                    c.guid.guid(),
                ))
            });
            let reply =
                ServerOpcodeMessage::SMSG_BUY_BANK_SLOT_RESULT(codec::build_buy_bank_slot_reply(
                    result.as_ref().map(|_| ()).map_err(String::as_str),
                ));
            match result {
                Ok(()) => super::send(ctx, connection, reply, false),
                Err(reason) => Err(super::rollback_reply(reply, reason)),
            }
        }
        ClientOpcodeMessage::CMSG_AUTOBANK_ITEM(c) => {
            auto_bank(ctx, guid, c.bag_index, c.slot_index)
        }
        ClientOpcodeMessage::CMSG_AUTOSTORE_BANK_ITEM(c) => {
            auto_bank(ctx, guid, c.bag_index, c.slot_index)
        }
        _ => Err("invalid economy opcode".into()),
    }
}

fn auto_bank(ctx: &ReducerContext, guid: u64, bag: u8, slot: u8) -> Result<(), String> {
    let result = super::actor(ctx, guid)
        .and_then(|_| {
            codec::inventory_slot(bag, slot).ok_or_else(|| "invalid inventory slot".into())
        })
        .and_then(|slot| crate::items::apply_auto_bank_item(ctx, guid, slot));
    result.map_err(|reason| {
        super::rollback_reply(
            ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(Box::new(
                codec::build_inventory_change_failure(),
            )),
            reason,
        )
    })
}
