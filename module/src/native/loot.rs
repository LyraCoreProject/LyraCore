use lyracore_protocol::{
    codec,
    messages::vanilla::{
        opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
        LootMethodError,
    },
};
use lyracore_shared::{loot::LootRefusal, loot_roll as roll};
use spacetimedb::{ConnectionId, ReducerContext};

use super::{game_native_player, Rollback};
use crate::{
    game_corpse_loot, game_item_template, game_world_entity, items::TakeLootRefusal, Refusal,
};

fn items(ctx: &ReducerContext, target: u64, guid: u64) -> Vec<codec::LootItemView> {
    let mut items: Vec<_> = ctx
        .db
        .game_corpse_loot()
        .by_corpse()
        .filter(target)
        .filter(|row| {
            if row.quest_only {
                crate::loot::quest_take_allowed(
                    row.reserved_for,
                    guid,
                    crate::loot::killer_needs_item(ctx, Some(guid), row.item_entry),
                )
            } else {
                crate::loot::group_loot_take_allowed(
                    row.withheld,
                    row.reserved_for,
                    row.master_only,
                    row.designated_looter_guid,
                    guid,
                )
            }
        })
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

fn open(ctx: &ReducerContext, connection: ConnectionId, target: u64) {
    if let Some(mut owner) = ctx.db.game_native_player().connection_id().find(connection) {
        owner.open_loot = target;
        ctx.db.game_native_player().connection_id().update(owner);
    }
}

fn refusal(target: u64, refusal: LootRefusal) -> Rollback {
    let failure = match refusal {
        LootRefusal::LootTagIneligible => LootMethodError::DidntKill,
        LootRefusal::OutOfRange => LootMethodError::TooFar,
        LootRefusal::NoLootSource
        | LootRefusal::LooterUnavailable
        | LootRefusal::NothingToLoot
        | LootRefusal::RollUnavailable
        | LootRefusal::NotMasterLooter
        | LootRefusal::RecipientUnavailable
        | LootRefusal::RecipientInventoryFull => return Rollback::silent(refusal.as_tag()),
    };
    Rollback::reply(
        ServerOpcodeMessage::SMSG_LOOT_RESPONSE(Box::new(codec::build_loot_error_response(
            target, failure,
        ))),
        refusal.as_tag(),
    )
}

pub(super) fn receive(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), Rollback> {
    super::actor(ctx, guid)?;
    let current = ctx
        .db
        .game_native_player()
        .connection_id()
        .find(connection)
        .map_or(0, |owner| owner.open_loot);
    match message {
        ClientOpcodeMessage::CMSG_LOOT(c) => {
            let target = c.guid.guid();
            crate::loot::open_creature_corpse(ctx, guid, target)
                .map_err(|reason| refusal(target, reason))?;
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
                    ));
                }
            }
            let (opcode, body) = codec::build_loot_response_raw(target, money, &items);
            super::send_raw(ctx, connection, opcode, body, false);
            open(ctx, connection, target);
            Ok(())
        }
        ClientOpcodeMessage::CMSG_GAMEOBJ_USE(c) => {
            let target = c.guid.guid();
            crate::gameobject::request_use_gameobject(ctx, guid, target)?;
            let items = items(ctx, target, guid);
            if !items.is_empty() {
                let (opcode, body) = codec::build_loot_response_raw(target, 0, &items);
                super::send_raw(ctx, connection, opcode, body, false);
                open(ctx, connection, target);
            }
            Ok(())
        }
        ClientOpcodeMessage::CMSG_LOOT_MONEY if current != 0 => {
            crate::loot::claim_money(ctx, guid, current)
                .map_err(|reason| refusal(current, reason))?;
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
        ClientOpcodeMessage::CMSG_AUTOSTORE_LOOT_ITEM(c) if current != 0 => {
            crate::items::take_loot_item(ctx, guid, current, c.item_slot).map_err(|reason| {
                use crate::actor::ActionRefusalKind as Kind;
                match reason {
                    TakeLootRefusal::Loot(reason) => refusal(current, reason),
                    TakeLootRefusal::Refused(reason) => match reason.kind {
                        Kind::OutOfRange | Kind::OtherPartition => {
                            refusal(current, LootRefusal::OutOfRange)
                        }
                        Kind::MissingActor
                        | Kind::MissingTarget
                        | Kind::DeadActor
                        | Kind::DeadTarget
                        | Kind::CannotAct
                        | Kind::InventoryFull
                        | Kind::Other
                        | Kind::MissingResource
                        | Kind::Class
                        | Kind::Level
                        | Kind::Prerequisite
                        | Kind::ProfileLimit
                        | Kind::TransferPending => Rollback::silent(reason),
                    },
                }
            })?;
            Ok(super::send(
                ctx,
                connection,
                ServerOpcodeMessage::SMSG_LOOT_REMOVED(codec::build_loot_removed(c.item_slot)),
                false,
            )?)
        }
        ClientOpcodeMessage::CMSG_LOOT_RELEASE(c) => {
            open(ctx, connection, 0);
            Ok(super::send(
                ctx,
                connection,
                ServerOpcodeMessage::SMSG_LOOT_RELEASE_RESPONSE(Box::new(
                    codec::build_loot_release_response(c.guid.guid()),
                )),
                false,
            )?)
        }
        ClientOpcodeMessage::CMSG_LOOT_ROLL(c) => {
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
        ClientOpcodeMessage::CMSG_LOOT_MASTER_GIVE(c) => Ok(crate::loot::apply_master_give(
            ctx,
            guid,
            c.loot.guid(),
            c.slot_id,
            c.player.guid(),
        )?),
        ClientOpcodeMessage::CMSG_LOOT_MONEY | ClientOpcodeMessage::CMSG_AUTOSTORE_LOOT_ITEM(_) => {
            Ok(())
        }
        _ => Err("invalid loot opcode".into()),
    }
}

pub(super) fn group_event(
    ctx: &ReducerContext,
    recipient: u64,
    kind: u8,
    other: u64,
    payload: &str,
) {
    let Some(owner) = ctx.db.game_native_player().character_guid().find(recipient) else {
        return;
    };
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
        let packet = lyracore_protocol::world::encode(message)
            .expect("bounded Loot Roll message fits a world frame");
        super::send_raw(ctx, owner.connection_id, packet.opcode, packet.body, false);
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
