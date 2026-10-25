use lyracore_protocol::{
    codec,
    messages::vanilla::{opcodes::ServerOpcodeMessage, Guid, SMSG_PARTYKILLLOG},
};
use spacetimedb::{ConnectionId, ReducerContext, Table};

use super::{game_native_player, Rollback};
use crate::combat::{game_melee_attack, CombatEvent, MeleeAttack};

pub(super) fn attack(ctx: &ReducerContext, guid: u64, target: u64) -> Result<(), Rollback> {
    super::actor(ctx, guid)?;
    let already_armed = ctx
        .db
        .game_melee_attack()
        .attacker_guid()
        .find(guid)
        .is_some_and(|row| row.target_guid == target && row.ranged_spell_id == 0);
    crate::actor::attack(ctx, guid, target).map_err(|cause| {
        if cause.contains(lyracore_shared::ERR_ATTACK_TARGET_DEAD) {
            Rollback::reply(ServerOpcodeMessage::SMSG_ATTACKSWING_DEADTARGET, cause)
        } else if cause.contains(lyracore_shared::ERR_ATTACK_FRIENDLY) {
            Rollback::reply(ServerOpcodeMessage::SMSG_ATTACKSWING_CANT_ATTACK, cause)
        } else {
            Rollback::silent(cause)
        }
    })?;
    if already_armed {
        emit(
            ctx,
            guid,
            ServerOpcodeMessage::SMSG_ATTACKSTART(Box::new(codec::build_attack_start(
                guid, target,
            ))),
            true,
        );
    }
    Ok(())
}

pub(super) fn stop(ctx: &ReducerContext, guid: u64) -> Result<(), Rollback> {
    super::actor(ctx, guid)?;
    if ctx
        .db
        .game_melee_attack()
        .attacker_guid()
        .find(guid)
        .is_some_and(|row| row.ranged_spell_id != 0)
    {
        return Ok(());
    }
    Ok(crate::actor::stop_attack(ctx, guid)?)
}

fn emit(ctx: &ReducerContext, source: u64, message: ServerOpcodeMessage, source_only: bool) {
    if ctx.db.game_native_player().count() == 0 {
        return;
    }
    let packet = lyracore_protocol::world::encode(message)
        .expect("fixed-size combat packet fits a world frame");
    if source_only {
        if let Some(player) = ctx.db.game_native_player().character_guid().find(source) {
            super::send_raw(ctx, player.connection_id, packet.opcode, packet.body, false);
        }
    } else {
        super::visibility::broadcast(ctx, source, packet.opcode, packet.body, false);
    }
}

pub(crate) fn combat_event(ctx: &ReducerContext, event: &CombatEvent) {
    if event.ranged_spell_id != 0 {
        let ammo = (event.ammo_display_id != 0).then_some((event.ammo_display_id, 24));
        emit(
            ctx,
            event.attacker_guid,
            ServerOpcodeMessage::SMSG_SPELL_GO(Box::new(codec::build_spell_go_outcome(
                event.attacker_guid,
                event.ranged_spell_id,
                event.target_guid,
                ammo,
                event.hit_info == 2,
            ))),
            false,
        );
    } else if !event.spell_swing {
        emit(
            ctx,
            event.attacker_guid,
            ServerOpcodeMessage::SMSG_ATTACKERSTATEUPDATE(Box::new(
                codec::build_attacker_state_update(
                    event.attacker_guid,
                    event.target_guid,
                    event.damage,
                    event.hit_info,
                    event.blocked_amount,
                    0,
                ),
            )),
            false,
        );
    }
    if event.killing_blow && event.ranged_spell_id == 0 {
        emit(
            ctx,
            event.attacker_guid,
            ServerOpcodeMessage::SMSG_ATTACKSTOP(Box::new(codec::build_attack_stop(
                event.attacker_guid,
                event.target_guid,
            ))),
            false,
        );
    }
    if event.killing_blow && !is_creature(event.attacker_guid) {
        emit(
            ctx,
            event.attacker_guid,
            ServerOpcodeMessage::SMSG_PARTYKILLLOG(Box::new(SMSG_PARTYKILLLOG {
                player_with_killing_blow: Guid::new(event.attacker_guid),
                victim: Guid::new(event.target_guid),
            })),
            true,
        );
    }
}

fn is_creature(guid: u64) -> bool {
    matches!(guid >> 48, 0xf130 | 0xf140)
}

pub(crate) fn engagement_changed(
    ctx: &ReducerContext,
    old: Option<&MeleeAttack>,
    row: Option<&MeleeAttack>,
) {
    let stance = |row: &MeleeAttack| (row.target_guid, row.ranged_spell_id);
    if old.map(stance) != row.map(stance) {
        if let Some(old) = old {
            if old.ranged_spell_id != 0 {
                emit(
                    ctx,
                    old.attacker_guid,
                    ServerOpcodeMessage::SMSG_CANCEL_AUTO_REPEAT,
                    true,
                );
            } else {
                emit(
                    ctx,
                    old.attacker_guid,
                    ServerOpcodeMessage::SMSG_ATTACKSTOP(Box::new(codec::build_attack_stop(
                        old.attacker_guid,
                        old.target_guid,
                    ))),
                    false,
                );
            }
        }
        if let Some(row) = row.filter(|row| row.ranged_spell_id == 0) {
            if old.is_none() && is_creature(row.attacker_guid) {
                emit(
                    ctx,
                    row.attacker_guid,
                    ServerOpcodeMessage::SMSG_AI_REACTION(Box::new(
                        codec::build_ai_reaction_hostile(row.attacker_guid),
                    )),
                    false,
                );
            }
            emit(
                ctx,
                row.attacker_guid,
                ServerOpcodeMessage::SMSG_ATTACKSTART(Box::new(codec::build_attack_start(
                    row.attacker_guid,
                    row.target_guid,
                ))),
                false,
            );
        }
    }
    if let Some(row) = row.filter(|row| old.is_none_or(|old| old.swing_error != row.swing_error)) {
        let message = match row.swing_error {
            crate::combat::SWING_ERROR_NOT_IN_RANGE => {
                ServerOpcodeMessage::SMSG_ATTACKSWING_NOTINRANGE
            }
            crate::combat::SWING_ERROR_BAD_FACING => {
                ServerOpcodeMessage::SMSG_ATTACKSWING_BADFACING
            }
            _ => return,
        };
        emit(ctx, row.attacker_guid, message, true);
    }
}

pub(super) fn replay(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
) -> Result<(), String> {
    if let Some(row) = ctx
        .db
        .game_melee_attack()
        .attacker_guid()
        .find(guid)
        .filter(|row| row.ranged_spell_id == 0)
    {
        super::send(
            ctx,
            connection,
            ServerOpcodeMessage::SMSG_ATTACKSTART(Box::new(codec::build_attack_start(
                row.attacker_guid,
                row.target_guid,
            ))),
            false,
        )?;
    }
    Ok(())
}
