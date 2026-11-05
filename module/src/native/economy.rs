use lyracore_protocol::{
    codec,
    messages::vanilla::{
        opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
        BuyBankSlotResult, BuyResult, SellItemResult,
    },
};
use lyracore_shared::constants::npc_flags;
use spacetimedb::{ConnectionId, ReducerContext};

use super::{game_native_player, Rollback};
use crate::items::{
    game_item_instance, BankSlotRefusal, BuyRefusal, BuybackItem, BuybackRefusal, NpcGate,
    SellRefusal,
};
use crate::{game_world_entity, Refusal};

fn interaction(
    ctx: &ReducerContext,
    guid: u64,
    npc: u64,
    flag: u32,
) -> Result<crate::WorldEntity, Refusal> {
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
    .map_err(Refusal::from)
}

pub(super) fn vendor(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    npc: u64,
) -> Result<(), Refusal> {
    let npc = interaction(ctx, guid, npc, npc_flags::VENDOR)?;
    let items = crate::items::vendor_offers(ctx, npc.entry)
        .into_iter()
        .map(|(row, item)| codec::VendorItemView {
            item_entry: item.entry,
            display_id: item.display_id,
            buy_price: crate::items::vendor_purchase_price(
                ctx,
                guid,
                npc.faction_template,
                item.buy_price,
            ),
            max_durability: item.max_durability,
            available_count: crate::items::vendor_stock::available(ctx, npc.guid, &row, &item),
            buy_count: item.buy_count,
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
) -> Result<(), Refusal> {
    interaction(ctx, guid, npc, npc_flags::BANKER)?;
    Ok(super::send(
        ctx,
        connection,
        ServerOpcodeMessage::SMSG_SHOW_BANK(codec::build_show_bank(npc)),
        false,
    )?)
}

pub(super) fn buyback(ctx: &ReducerContext, guid: u64) -> Vec<BuybackItem> {
    crate::items::buyback_entries(ctx, guid)
}

pub(super) fn replay_buyback(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
) -> Result<(), String> {
    for row in buyback(ctx, guid) {
        let view = buyback_item(ctx, &row);
        super::send(
            ctx,
            connection,
            ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(codec::build_item_create_object(
                &view,
            ))),
            false,
        )?;
        if let Some((opcode, body)) = codec::build_enchantment_slot_values(&view) {
            super::send_raw(ctx, connection, opcode, body, false);
        }
        buyback_values(
            ctx,
            row.character_guid,
            row.slot,
            row.item_guid,
            row.price,
            row.timestamp,
        );
    }
    Ok(())
}

pub(super) fn buyback_item(ctx: &ReducerContext, row: &BuybackItem) -> codec::ItemInstanceView {
    let mut view = super::view::item(ctx, &row.item);
    view.contained_guid = Some(0);
    if let Some(paused) = &row.temporary_enchantment {
        view.temporary_enchantment = codec::TemporaryEnchantmentView {
            enchantment: paused.enchant_id,
            duration_ms: (paused.remaining_micros / 1000).clamp(0, i64::from(u32::MAX)) as u32,
            charges: paused.charges,
        };
    }
    view
}

pub(super) fn buyback_values(
    ctx: &ReducerContext,
    character_guid: u64,
    slot: u8,
    item_guid: u64,
    price: u32,
    timestamp: u32,
) {
    let Some(owner) = ctx
        .db
        .game_native_player()
        .character_guid()
        .find(character_guid)
    else {
        return;
    };
    let index = u16::from(slot - crate::items::BUYBACK_START);
    let mut mask = codec::update_mask::UpdateMaskValues::new();
    // Build 5875: buyback links at 624, prices at 1226 and timestamps at 1238.
    mask.set_u64(624 + index * 2, item_guid);
    mask.set_u32(1226 + index, price);
    mask.set_u32(1238 + index, timestamp);
    let (opcode, body) = codec::build_values_update_raw(character_guid, &mask);
    super::send_raw(ctx, owner.connection_id, opcode, body, false);
}

pub(crate) fn buyback_removed(ctx: &ReducerContext, row: &BuybackItem) {
    buyback_values(ctx, row.character_guid, row.slot, 0, 0, 0);
}

pub(crate) fn buyback_destroyed(ctx: &ReducerContext, row: &BuybackItem) {
    let Some(owner) = ctx
        .db
        .game_native_player()
        .character_guid()
        .find(row.character_guid)
    else {
        return;
    };
    super::send(
        ctx,
        owner.connection_id,
        ServerOpcodeMessage::SMSG_DESTROY_OBJECT(codec::build_destroy_object(row.item_guid)),
        false,
    )
    .expect("bounded item message fits a world frame");
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

fn purchase_reply(
    ctx: &ReducerContext,
    vendor: u64,
    entry: u32,
    lots: u32,
) -> Option<ServerOpcodeMessage> {
    let npc = ctx.db.game_world_entity().guid().find(vendor)?;
    let (index, (offer, template)) = crate::items::vendor_offers(ctx, npc.entry)
        .into_iter()
        .enumerate()
        .find(|(_, (offer, _))| offer.item_entry == entry)?;
    let amount_for_sale = if offer.max_count == 0 {
        u32::MAX
    } else {
        crate::items::vendor_stock::available(ctx, vendor, &offer, &template)
            .saturating_sub(lots.saturating_mul(template.buy_count.max(1)))
    };
    Some(ServerOpcodeMessage::SMSG_BUY_ITEM(Box::new(
        lyracore_protocol::messages::vanilla::SMSG_BUY_ITEM {
            guid: lyracore_protocol::messages::Guid::new(vendor),
            vendor_slot: index as u32 + 1,
            amount_for_sale,
            amount_bought: lots,
        },
    )))
}

fn purchase_in_slot(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    request: &lyracore_protocol::messages::vanilla::CMSG_BUY_ITEM_IN_SLOT,
) -> Result<(), Rollback> {
    super::actor(ctx, guid)?;
    let bag = if request.bag.guid() == guid {
        255
    } else {
        let Some(bag) = ctx
            .db
            .game_item_instance()
            .guid()
            .find(request.bag.guid())
            .filter(|item| item.owner_guid == guid && (19..23).contains(&item.slot))
        else {
            return Ok(());
        };
        bag.slot
    };
    if ctx
        .db
        .game_world_entity()
        .guid()
        .find(guid)
        .is_some_and(|character| character.dead || character.health == 0)
    {
        return Ok(());
    }
    let vendor = request.vendor.guid();
    if let Some(reply) = purchase_reply(ctx, vendor, request.item, u32::from(request.amount).max(1))
    {
        super::send(ctx, connection, reply, false)?;
    }
    crate::items::apply_buy_item_to(
        ctx,
        guid,
        vendor,
        request.item,
        u32::from(request.amount),
        Some((bag, request.bag_slot)),
    )
    .map_err(|refusal| {
        if let BuyRefusal::Equipment {
            result,
            required_level,
        } = &refusal
        {
            let mut body = vec![result.as_int()];
            if *result == lyracore_protocol::messages::vanilla::InventoryResult::CantEquipLevelI {
                body.extend(u32::from(*required_level).to_le_bytes());
            }
            body.extend([0; 17]);
            return Rollback::replies(
                vec![lyracore_protocol::world::Packet {
                    opcode: 0x112,
                    body,
                }],
                refusal,
            );
        }
        let (vendor, result) = if matches!(&refusal, BuyRefusal::Vendor(_)) {
            (0, BuyResult::DistanceTooFar)
        } else if matches!(&refusal, BuyRefusal::UnknownItem) {
            (0, BuyResult::CantFindItem)
        } else {
            (vendor, buy_result(&refusal))
        };
        Rollback::reply(
            ServerOpcodeMessage::SMSG_BUY_FAILED(Box::new(codec::build_buy_failed(
                vendor,
                request.item,
                result,
            ))),
            refusal,
        )
    })
}

pub(super) fn receive(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), Rollback> {
    match message {
        ClientOpcodeMessage::CMSG_LIST_INVENTORY(c) => {
            Ok(vendor(ctx, connection, guid, c.guid.guid())?)
        }
        ClientOpcodeMessage::CMSG_BANKER_ACTIVATE(c) => {
            Ok(bank(ctx, connection, guid, c.guid.guid())?)
        }
        ClientOpcodeMessage::CMSG_BUY_ITEM(c) => {
            let vendor = c.vendor.guid();
            super::actor(ctx, guid)
                .map_err(BuyRefusal::from)
                .and_then(|_| {
                    if let Some(reply) =
                        purchase_reply(ctx, vendor, c.item, u32::from(c.amount).max(1))
                    {
                        super::send(ctx, connection, reply, false).map_err(BuyRefusal::from)?;
                    }
                    crate::items::apply_buy_item(ctx, guid, vendor, c.item, u32::from(c.amount))
                })
                .map_err(|refusal| {
                    Rollback::reply(
                        ServerOpcodeMessage::SMSG_BUY_FAILED(Box::new(codec::build_buy_failed(
                            if matches!(refusal, BuyRefusal::UnknownItem) {
                                0
                            } else {
                                vendor
                            },
                            c.item,
                            buy_result(&refusal),
                        ))),
                        refusal,
                    )
                })
        }
        ClientOpcodeMessage::CMSG_BUY_ITEM_IN_SLOT(c) => {
            purchase_in_slot(ctx, connection, guid, &c)
        }
        ClientOpcodeMessage::CMSG_SELL_ITEM(c) => {
            let result = super::actor(ctx, guid)
                .map_err(SellRefusal::from)
                .and_then(|_| owned_slot(ctx, guid, c.item.guid()).map_err(SellRefusal::from))
                .and_then(|slot| {
                    crate::items::apply_item_sell_count(
                        ctx,
                        guid,
                        c.vendor.guid(),
                        slot,
                        u32::from(c.amount),
                    )
                });
            result.map_err(|refusal| {
                Rollback::reply(
                    ServerOpcodeMessage::SMSG_SELL_ITEM(Box::new(codec::build_sell_item_refusal(
                        c.vendor.guid(),
                        c.item.guid(),
                        sell_result(&refusal),
                    ))),
                    refusal,
                )
            })?;
            Ok(())
        }
        ClientOpcodeMessage::CMSG_BUYBACK_ITEM(c) => {
            super::actor(ctx, guid)?;
            let index = c
                .slot
                .as_int()
                .checked_sub(69)
                .filter(|index| *index < 12)
                .map_or(u8::MAX, |index| index as u8);
            crate::items::apply_buyback_item(ctx, guid, c.guid.guid(), index)
                .map_err(|cause| {
                    let reply = match &cause {
                        BuybackRefusal::Vendor(_) => ServerOpcodeMessage::SMSG_SELL_ITEM(Box::new(
                            codec::build_sell_item_refusal(0, 0, SellItemResult::CantFindVendor),
                        )),
                        BuybackRefusal::Money { entry } => ServerOpcodeMessage::SMSG_BUY_FAILED(Box::new(
                            codec::build_buy_failed(c.guid.guid(), *entry, BuyResult::NotEnoughMoney),
                        )),
                        BuybackRefusal::Limit { item_guid } => ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(Box::new(
                            lyracore_protocol::messages::vanilla::SMSG_INVENTORY_CHANGE_FAILURE::CantCarryMoreOfThis {
                                item1: lyracore_protocol::messages::Guid::new(*item_guid),
                                item2: lyracore_protocol::messages::Guid::new(0),
                                bag_type_subclass: 0,
                            },
                        )),
                        BuybackRefusal::Storage { item_guid, cause } => ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(Box::new(
                            codec::build_inventory_refusal(
                                if cause.kind == crate::actor::ActionRefusalKind::InventoryFull {
                                    lyracore_shared::item::ItemRefusal::InventoryFull
                                } else { lyracore_shared::item::ItemRefusal::Internal },
                                *item_guid, 0,
                            ),
                        )),
                        BuybackRefusal::Empty | BuybackRefusal::Refused(_) => ServerOpcodeMessage::SMSG_BUY_FAILED(Box::new(
                            codec::build_buy_failed(c.guid.guid(), 0, BuyResult::CantFindItem),
                        )),
                    };
                    Rollback::reply(reply, cause)
                })
        }
        ClientOpcodeMessage::CMSG_REPAIR_ITEM(c) => {
            let result = super::actor(ctx, guid)
                .map_err(String::from)
                .and_then(|_| {
                    if c.item.guid() == 0 {
                        Ok(u8::MAX)
                    } else {
                        owned_slot(ctx, guid, c.item.guid())
                    }
                })
                .and_then(|slot| crate::items::apply_player_repair(ctx, guid, c.npc.guid(), slot));
            result.map_err(|reason| {
                Rollback::reply(
                    ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(
                        codec::build_gm_system_message(reason.clone()),
                    )),
                    reason,
                )
            })
        }
        ClientOpcodeMessage::CMSG_BUY_BANK_SLOT(c) => {
            let reply = |result| {
                ServerOpcodeMessage::SMSG_BUY_BANK_SLOT_RESULT(codec::build_buy_bank_slot_result(
                    result,
                ))
            };
            super::actor(ctx, guid)
                .map_err(|refusal| Rollback::reply(reply(BuyBankSlotResult::NotBanker), refusal))?;
            crate::items::apply_buy_bank_slot(ctx, guid, c.guid.guid())
                .map_err(|refusal| Rollback::reply(reply(bank_slot_result(&refusal)), refusal))?;
            Ok(())
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

fn buy_result(refusal: &BuyRefusal) -> BuyResult {
    match refusal {
        BuyRefusal::Vendor(refusal) => match refusal.gate {
            NpcGate::OtherMap | NpcGate::OutOfRange => BuyResult::DistanceTooFar,
            NpcGate::CharacterAbsent
            | NpcGate::CharacterDead
            | NpcGate::NpcAbsent
            | NpcGate::WrongService
            | NpcGate::NpcDead
            | NpcGate::Unfriendly => BuyResult::CantFindItem,
        },
        BuyRefusal::NotEnoughMoney { .. } => BuyResult::NotEnoughMoney,
        BuyRefusal::InventoryFull => BuyResult::CantCarryMore,
        BuyRefusal::SoldOut => BuyResult::ItemAlreadySold,
        BuyRefusal::UnknownItem => BuyResult::CantFindItem,
        BuyRefusal::Reputation => BuyResult::ReputationRequire,
        BuyRefusal::Refused(_) | BuyRefusal::Equipment { .. } => BuyResult::CantFindItem,
    }
}

fn sell_result(refusal: &SellRefusal) -> SellItemResult {
    match refusal {
        SellRefusal::Vendor(refusal) => match refusal.gate {
            NpcGate::CharacterAbsent => SellItemResult::CantSellItem,
            NpcGate::CharacterDead
            | NpcGate::NpcAbsent
            | NpcGate::WrongService
            | NpcGate::NpcDead
            | NpcGate::OtherMap
            | NpcGate::OutOfRange
            | NpcGate::Unfriendly => SellItemResult::CantFindVendor,
        },
        SellRefusal::NoItemInSlot(_) => SellItemResult::CantFindItem,
        SellRefusal::Refused(_) => SellItemResult::CantSellItem,
    }
}

fn bank_slot_result(refusal: &BankSlotRefusal) -> BuyBankSlotResult {
    match refusal {
        BankSlotRefusal::AlreadyMaxed => BuyBankSlotResult::FailedTooMany,
        BankSlotRefusal::CannotAfford { .. } => BuyBankSlotResult::InsufficientFunds,
        BankSlotRefusal::Banker(_) => BuyBankSlotResult::NotBanker,
    }
}

fn auto_bank(ctx: &ReducerContext, guid: u64, bag: u8, slot: u8) -> Result<(), Rollback> {
    let result = super::actor(ctx, guid)
        .map_err(String::from)
        .and_then(|_| {
            codec::inventory_slot(bag, slot).ok_or_else(|| "invalid inventory slot".into())
        })
        .and_then(|slot| crate::items::apply_auto_bank_item(ctx, guid, slot));
    result.map_err(|reason| {
        Rollback::reply(
            ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(Box::new(
                codec::build_inventory_change_failure(),
            )),
            reason,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::NpcRefusal;

    fn npc(gate: NpcGate, message: &str) -> NpcRefusal {
        NpcRefusal::new(gate, message)
    }

    #[test]
    fn every_buy_refusal_reports_its_smsg_buy_failed_result() {
        use BuyRefusal as R;
        use NpcGate as G;
        for (refusal, code) in [
            (R::Vendor(npc(G::CharacterAbsent, "user not in world")), 0),
            (
                R::Vendor(npc(G::CharacterDead, "dead players cannot buy")),
                0,
            ),
            (R::Vendor(npc(G::NpcAbsent, "no such vendor")), 0),
            (R::Vendor(npc(G::WrongService, "target is not a vendor")), 0),
            (R::Vendor(npc(G::NpcDead, "vendor is dead")), 0),
            (R::Vendor(npc(G::OtherMap, "vendor on another map")), 5),
            (R::Vendor(npc(G::OutOfRange, "vendor out of range")), 5),
            (
                R::Vendor(npc(G::Unfriendly, "vendor refuses interaction")),
                0,
            ),
            (R::NotEnoughMoney { have: 9, need: 10 }, 2),
            (R::InventoryFull, 8),
            (R::SoldOut, 1),
            (R::Refused("vendor does not sell that item".into()), 0),
        ] {
            assert_eq!(buy_result(&refusal).as_int(), code, "{refusal}");
        }
    }

    #[test]
    fn every_sell_refusal_reports_its_smsg_sell_item_result() {
        use NpcGate as G;
        use SellRefusal as R;
        for (refusal, code) in [
            (R::Vendor(npc(G::CharacterAbsent, "user not in world")), 2),
            (
                R::Vendor(npc(G::CharacterDead, "dead players cannot sell")),
                3,
            ),
            (R::Vendor(npc(G::NpcAbsent, "no such vendor")), 3),
            (R::Vendor(npc(G::WrongService, "target is not a vendor")), 3),
            (R::Vendor(npc(G::NpcDead, "vendor is dead")), 3),
            (R::Vendor(npc(G::OtherMap, "vendor on another map")), 3),
            (R::Vendor(npc(G::OutOfRange, "vendor out of range")), 3),
            (
                R::Vendor(npc(G::Unfriendly, "vendor refuses interaction")),
                3,
            ),
            (R::NoItemInSlot(40), 1),
            (R::Refused("item has no sell value".into()), 2),
        ] {
            assert_eq!(sell_result(&refusal).as_int(), code, "{refusal}");
        }
    }

    #[test]
    fn every_bank_slot_refusal_reports_its_smsg_buy_bank_slot_result() {
        use BankSlotRefusal as R;
        for (refusal, code) in [
            (R::AlreadyMaxed, 0),
            (R::CannotAfford { price: 1_000 }, 1),
            (
                R::Banker(npc(NpcGate::OutOfRange, "banker out of range")),
                2,
            ),
        ] {
            assert_eq!(bank_slot_result(&refusal).as_int(), code, "{refusal}");
        }
    }
}
