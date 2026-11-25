use lyracore_protocol::{
    codec,
    messages::vanilla::{
        opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
        LootMethodError, SMSG_INVENTORY_CHANGE_FAILURE,
    },
};
use lyracore_shared::guid;
use lyracore_shared::item::ItemRefusal;
use lyracore_shared::loot_roll as roll;
use spacetimedb::{ConnectionId, ReducerContext};

use super::{game_native_player, Rollback, Unhandled};
use crate::loot::{LootRefusal, OpenItemRefusal};
use crate::{game_corpse_loot, game_item_template, game_world_entity, Refusal};

fn items(ctx: &ReducerContext, target: u64, guid: u64) -> Vec<codec::LootItemView> {
    let mut items: Vec<_> = ctx
        .db
        .game_corpse_loot()
        .by_corpse()
        .filter(target)
        .filter(|row| row.takeable_by(ctx, guid))
        .map(|row| {
            (
                row.slot,
                row.item_entry,
                row.count,
                ctx.db
                    .game_item_template()
                    .entry()
                    .find(row.item_entry)
                    .map_or(0, |item| item.display_id),
                row.random_property_id,
            )
        })
        .collect();
    items.sort_by_key(|row| row.0);
    items
}

fn open_loot(ctx: &ReducerContext, connection: ConnectionId) -> u64 {
    ctx.db
        .game_native_player()
        .connection_id()
        .find(connection)
        .map_or(0, |owner| owner.open_loot)
}

fn open(ctx: &ReducerContext, connection: ConnectionId, target: u64) {
    if let Some(mut owner) = ctx.db.game_native_player().connection_id().find(connection) {
        owner.open_loot = target;
        ctx.db.game_native_player().connection_id().update(owner);
    }
}

/// Shows a Loot Window on `target`. vmangos `SendLoot` first releases a window open on another
/// Loot Source.
fn show(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    target: u64,
    money: u32,
    items: &[codec::LootItemView],
) -> Result<(), Rollback> {
    let current = open_loot(ctx, connection);
    if current != 0 && current != target {
        release(ctx, connection, guid, current)?;
    }
    let (opcode, body) = codec::build_loot_response_raw(target, money, items);
    super::send_raw(ctx, connection, opcode, body, false);
    open(ctx, connection, target);
    Ok(())
}

fn loot_error(target: u64, failure: LootMethodError, refusal: LootRefusal) -> Rollback {
    let (opcode, body) = codec::build_loot_error_response(target, failure);
    Rollback::replies(
        vec![lyracore_protocol::world::Packet { opcode, body }],
        refusal.as_tag(),
    )
}

fn release_response(target: u64) -> ServerOpcodeMessage {
    ServerOpcodeMessage::SMSG_LOOT_RELEASE_RESPONSE(Box::new(codec::build_loot_release_response(
        target,
    )))
}

fn inventory_failure(failure: SMSG_INVENTORY_CHANGE_FAILURE) -> ServerOpcodeMessage {
    ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(Box::new(failure))
}

fn open_refusal(target: u64, refusal: LootRefusal) -> Rollback {
    let failure = match refusal {
        LootRefusal::LootTagIneligible => LootMethodError::DidntKill,
        LootRefusal::OutOfRange => LootMethodError::TooFar,
        LootRefusal::NoLootSource
        | LootRefusal::LooterUnavailable
        | LootRefusal::LooterDead
        | LootRefusal::NothingToLoot
        | LootRefusal::ItemUnavailable
        | LootRefusal::InventoryFull
        | LootRefusal::UniqueItemHeld
        | LootRefusal::StorageRefused
        | LootRefusal::RollUnavailable
        | LootRefusal::NotMasterLooter
        | LootRefusal::RecipientUnavailable
        | LootRefusal::RecipientInventoryFull => return Rollback::silent(refusal.as_tag()),
    };
    loot_error(target, failure, refusal)
}

// Replies follow vmangos HandleAutostoreLootItemOpcode; a failed store leaves the window open.
fn take_refusal(target: u64, refusal: LootRefusal) -> Rollback {
    let released = guid::is_gameobject(target) || guid::is_item(target);
    let reply = |message| Rollback::reply(message, refusal.as_tag());
    match refusal {
        LootRefusal::NoLootSource | LootRefusal::OutOfRange if released => {
            reply(release_response(target))
        }
        LootRefusal::NoLootSource
        | LootRefusal::LootTagIneligible
        | LootRefusal::ItemUnavailable => loot_error(target, LootMethodError::DidntKill, refusal),
        LootRefusal::OutOfRange => loot_error(target, LootMethodError::TooFar, refusal),
        LootRefusal::NothingToLoot => reply(inventory_failure(codec::build_already_looted())),
        LootRefusal::InventoryFull => reply(inventory_failure(codec::build_inventory_refusal(
            ItemRefusal::InventoryFull,
            0,
            0,
        ))),
        LootRefusal::UniqueItemHeld => {
            reply(inventory_failure(codec::build_cant_carry_more_of_this()))
        }
        LootRefusal::LooterUnavailable
        | LootRefusal::LooterDead
        | LootRefusal::StorageRefused
        | LootRefusal::RollUnavailable
        | LootRefusal::NotMasterLooter
        | LootRefusal::RecipientUnavailable
        | LootRefusal::RecipientInventoryFull => Rollback::silent(refusal.as_tag()),
    }
}

fn open_item_failure(refusal: OpenItemRefusal, item: u64) -> SMSG_INVENTORY_CHANGE_FAILURE {
    let refusal = match refusal {
        OpenItemRefusal::ItemMissing => ItemRefusal::ItemNotFound,
        OpenItemRefusal::LooterDead => ItemRefusal::PlayerDead,
        OpenItemRefusal::ItemLocked => return codec::build_item_locked(item),
        OpenItemRefusal::NotLootable => ItemRefusal::NotRightNow,
    };
    codec::build_inventory_refusal(refusal, item, 0)
}

// vmangos HandleOpenItemOpcode answers a refusal with SMSG_INVENTORY_CHANGE_FAILURE on the item.
fn open_item(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    slot: Option<u8>,
) -> Result<(), Rollback> {
    let item = slot.and_then(|slot| crate::items::item_in_slot(ctx, guid, slot));
    let item_guid = item.as_ref().map_or(0, |item| item.guid);
    let looter = super::actor(ctx, guid).map_err(|cause| {
        Rollback::reply(
            inventory_failure(codec::build_inventory_refusal(
                ItemRefusal::NotRightNow,
                item_guid,
                0,
            )),
            cause,
        )
    })?;
    crate::loot::open_item(ctx, &looter, item).map_err(|refusal| {
        Rollback::reply(
            inventory_failure(open_item_failure(refusal, item_guid)),
            refusal.as_tag(),
        )
    })?;
    let money = crate::loot::purse(ctx, item_guid);
    show(
        ctx,
        connection,
        guid,
        item_guid,
        money,
        &items(ctx, item_guid, guid),
    )
}

fn release(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    current: u64,
) -> Result<(), Rollback> {
    open(ctx, connection, 0);
    release_source(ctx, guid, current);
    Ok(super::send(
        ctx,
        connection,
        release_response(current),
        false,
    )?)
}

/// The Loot Release of `looter_guid`'s Loot Window on `source`. A teleport or a logout that closes
/// the window does it too, as vmangos `TeleportTo` and `LogoutPlayer` call `DoLootRelease`.
pub(super) fn release_source(ctx: &ReducerContext, looter_guid: u64, source: u64) {
    if guid::is_item(source) {
        crate::loot::release_item(ctx, looter_guid, source);
    } else if guid::is_gameobject(source) {
        crate::gameobject::chest_loot_released(ctx, looter_guid, source);
    }
}

pub(super) fn receive(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), Unhandled> {
    let open_loot = || open_loot(ctx, connection);
    let handled: Result<(), Rollback> = match message {
        ClientOpcodeMessage::CMSG_LOOT(c) => {
            super::actor(ctx, guid)?;
            let target = c.guid.guid();
            crate::loot::open_creature_corpse(ctx, guid, target)
                .map_err(|reason| open_refusal(target, reason))?;
            let money = ctx
                .db
                .game_world_entity()
                .guid()
                .find(target)
                .map_or(0, |row| row.money);
            let items = items(ctx, target, guid);
            if items.is_empty() && money == 0 {
                if let Err(reason) = crate::professions::skin_corpse(ctx, guid, target) {
                    let (opcode, body) = codec::build_loot_response_raw(target, 0, &[]);
                    return Err(Rollback::replies(
                        vec![lyracore_protocol::world::Packet { opcode, body }],
                        reason,
                    )
                    .into());
                }
            }
            show(ctx, connection, guid, target, money, &items)
        }
        ClientOpcodeMessage::CMSG_GAMEOBJ_USE(c) => {
            super::actor(ctx, guid)?;
            let target = c.guid.guid();
            crate::gameobject::request_use_gameobject(ctx, guid, target)?;
            let items = items(ctx, target, guid);
            let money = crate::loot::purse(ctx, target);
            if !items.is_empty() || money != 0 {
                show(ctx, connection, guid, target, money, &items)?;
            }
            Ok(())
        }
        ClientOpcodeMessage::CMSG_LOOT_MONEY => {
            super::actor(ctx, guid)?;
            let current = open_loot();
            if current == 0 {
                return Ok(());
            }
            // vmangos HandleLootMoneyOpcode answers no refusal.
            crate::loot::claim_money(ctx, guid, current)
                .map_err(|reason| Rollback::silent(reason.as_tag()))?;
            for viewer in ctx.db.game_native_player().by_open_loot().filter(current) {
                super::send(
                    ctx,
                    viewer.connection_id,
                    ServerOpcodeMessage::SMSG_LOOT_CLEAR_MONEY,
                    false,
                )?;
            }
            Ok(())
        }
        ClientOpcodeMessage::CMSG_AUTOSTORE_LOOT_ITEM(c) => {
            super::actor(ctx, guid)?;
            let current = open_loot();
            if current == 0 {
                return Ok(());
            }
            let taken = crate::loot::take_item(ctx, guid, current, c.item_slot)
                .map_err(|reason| take_refusal(current, reason))?;
            // A shared row leaves every open Loot Window; a reserved copy leaves only its taker's.
            let viewers: Vec<ConnectionId> = ctx
                .db
                .game_native_player()
                .by_open_loot()
                .filter(current)
                .filter(|viewer| viewer.connection_id == connection || taken.reserved_for == 0)
                .map(|viewer| viewer.connection_id)
                .collect();
            for viewer in viewers {
                super::send(
                    ctx,
                    viewer,
                    ServerOpcodeMessage::SMSG_LOOT_REMOVED(codec::build_loot_removed(taken.slot)),
                    false,
                )?;
            }
            Ok(())
        }
        ClientOpcodeMessage::CMSG_LOOT_RELEASE(_) => {
            super::actor(ctx, guid)?;
            let current = open_loot();
            if current == 0 {
                return Ok(());
            }
            release(ctx, connection, guid, current)
        }
        ClientOpcodeMessage::CMSG_OPEN_ITEM(c) => open_item(
            ctx,
            connection,
            guid,
            codec::inventory_slot(c.bag_index, c.slot),
        ),
        ClientOpcodeMessage::CMSG_LOOT_ROLL(c) => {
            super::actor(ctx, guid)?;
            let slot =
                u8::try_from(c.item_slot).map_err(|_| Refusal::from("invalid Loot Roll slot"))?;
            Ok(crate::loot::cast_vote_on(
                ctx,
                c.item.guid(),
                slot,
                guid,
                c.vote.as_int(),
            )?)
        }
        ClientOpcodeMessage::CMSG_LOOT_MASTER_GIVE(c) => {
            super::actor(ctx, guid)?;
            Ok(crate::loot::apply_master_give(
                ctx,
                guid,
                c.loot.guid(),
                c.slot_id,
                c.player.guid(),
            )?)
        }
        other => return Err(Unhandled::Declined(other)),
    };
    Ok(handled?)
}

pub(super) fn group_event(
    ctx: &ReducerContext,
    recipient: u64,
    kind: u8,
    other: u64,
    payload: &str,
) {
    if !super::has_world_session(ctx, recipient) {
        return;
    }
    let message = match kind {
        roll::event_kind::ROLL_START => {
            roll::decode_start(payload).map(|(corpse, slot, entry, countdown, property)| {
                ServerOpcodeMessage::SMSG_LOOT_START_ROLL(Box::new(codec::build_loot_start_roll(
                    corpse, slot, entry, countdown, property,
                )))
            })
        }
        roll::event_kind::ROLL_VOTE => {
            roll::decode_vote(payload).map(|(corpse, slot, entry, number, vote, _, property)| {
                ServerOpcodeMessage::SMSG_LOOT_ROLL(Box::new(codec::build_loot_roll(
                    corpse, slot, other, entry, number, vote, property,
                )))
            })
        }
        roll::event_kind::ROLL_WON => {
            roll::decode_won(payload).map(|(corpse, slot, entry, number, vote, property)| {
                ServerOpcodeMessage::SMSG_LOOT_ROLL_WON(Box::new(codec::build_loot_roll_won(
                    corpse, slot, entry, other, number, vote, property,
                )))
            })
        }
        roll::event_kind::ALL_PASSED => {
            roll::decode_all_passed(payload).map(|(corpse, slot, entry, property)| {
                ServerOpcodeMessage::SMSG_LOOT_ALL_PASSED(Box::new(codec::build_loot_all_passed(
                    corpse, slot, entry, property,
                )))
            })
        }
        roll::event_kind::MASTER_LIST => roll::decode_master_list(payload).map(|(_, eligible)| {
            ServerOpcodeMessage::SMSG_LOOT_MASTER_LIST(Box::new(codec::build_loot_master_list(
                &eligible,
            )))
        }),
        roll::event_kind::MONEY_SHARE => roll::decode_money_share(payload).map(|share| {
            ServerOpcodeMessage::SMSG_LOOT_MONEY_NOTIFY(codec::build_loot_money_notify(share))
        }),
        _ => return,
    };
    if let Some(message) = message {
        super::send_to_character(ctx, recipient, message, super::EncodeFailure::Panic);
    } else {
        spacetimedb::log::error!("invalid native Loot Roll event {kind} for Character {recipient}");
    }
}

pub(super) fn dynamic_flags(ctx: &ReducerContext, entity: &crate::WorldEntity, viewer: u64) -> u32 {
    use lyracore_shared::constants::unit_dynamic_flags::{LOOTABLE, TAPPED_BY_PLAYER};
    if entity.dead {
        if crate::loot::corpse_access(ctx, viewer, entity.guid).is_err() {
            return entity.dynamic_flags & !LOOTABLE;
        }
    } else if !entity.is_player() {
        use crate::loot::tag::{game_creature_quest_tap, LiveLootTagEligibility};
        if ctx
            .db
            .game_creature_quest_tap()
            .creature_guid()
            .find(entity.guid)
            .is_some()
        {
            return if crate::loot::tag::live_loot_tag_eligibility(ctx, entity.guid, viewer)
                == LiveLootTagEligibility::Available
            {
                entity.dynamic_flags | TAPPED_BY_PLAYER
            } else {
                entity.dynamic_flags & !TAPPED_BY_PLAYER
            };
        }
    }
    entity.dynamic_flags
}
