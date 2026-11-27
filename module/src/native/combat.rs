use lyracore_protocol::{
    codec,
    messages::vanilla::{opcodes::ServerOpcodeMessage, Guid, SMSG_PARTYKILLLOG},
};
use spacetimedb::{ConnectionId, ReducerContext, Table};

use super::{game_native_player, Rollback};
use crate::combat::AttackRefusal;
use crate::{game_melee_attack, CombatEvent, MeleeAttack};

pub(super) fn attack(ctx: &ReducerContext, guid: u64, target: u64) -> Result<(), Rollback> {
    super::actor(ctx, guid)?;
    let current = ctx.db.game_melee_attack().attacker_guid().find(guid);
    let already_armed = current
        .as_ref()
        .is_some_and(|row| row.target_guid == target && row.ranged_spell_id == 0);
    crate::combat::start_melee_attack(ctx, guid, target)
        .map_err(|refusal| attack_rollback(refusal, guid, target, current.is_some()))?;
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

fn attack_rollback(refusal: AttackRefusal, guid: u64, target: u64, has_victim: bool) -> Rollback {
    match refusal {
        AttackRefusal::DeadTarget => Rollback::reply(
            ServerOpcodeMessage::SMSG_ATTACKSWING_DEADTARGET,
            "target is dead",
        ),
        AttackRefusal::FriendlyTarget => Rollback::reply(
            ServerOpcodeMessage::SMSG_ATTACKSWING_CANT_ATTACK,
            "target is friendly",
        ),
        // vmangos HandleAttackSwingOpcode stops the client's attack only when there is no victim.
        AttackRefusal::Evading if has_victim => Rollback::silent("target is evading"),
        AttackRefusal::Evading => Rollback::reply(
            ServerOpcodeMessage::SMSG_ATTACKSTOP(Box::new(codec::build_attack_stop(guid, target))),
            "target is evading",
        ),
        AttackRefusal::Refused(refusal) => Rollback::silent(refusal),
    }
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
    let packet = super::bounded(message);
    if source_only {
        super::send_raw_to_character(ctx, source, packet.opcode, packet.body);
    } else {
        super::visibility::broadcast(ctx, source, packet.opcode, packet.body, false);
    }
}

/// vmangos `Player::SendAttackSwingCancelAttack`: the client stops its melee and ranged attack.
pub(crate) fn cancel_combat(ctx: &ReducerContext, character_guid: u64) {
    emit(
        ctx,
        character_guid,
        ServerOpcodeMessage::SMSG_CANCEL_COMBAT,
        true,
    );
}

pub(crate) fn damage_shield(
    ctx: &ReducerContext,
    bearer: u64,
    attacker: u64,
    damage: u32,
    school: u8,
) {
    use lyracore_protocol::messages::vanilla::SMSG_SPELLDAMAGESHIELD;
    emit(
        ctx,
        bearer,
        ServerOpcodeMessage::SMSG_SPELLDAMAGESHIELD(Box::new(SMSG_SPELLDAMAGESHIELD {
            victim: Guid::new(bearer),
            caster: Guid::new(attacker),
            damage,
            school: school.try_into().unwrap_or_default(),
        })),
        false,
    );
}

pub(crate) fn spell_log_miss(ctx: &ReducerContext, caster: u64, target: u64, spell: u32, miss: u8) {
    super::visibility::broadcast(
        ctx,
        caster,
        codec::SMSG_SPELLLOGMISS_OPCODE,
        codec::build_spell_log_miss(caster, spell, target, miss),
        false,
    );
}

pub(crate) fn spell_or_damage_immune(ctx: &ReducerContext, caster: u64, target: u64, spell: u32) {
    use lyracore_protocol::messages::vanilla::SMSG_SPELLORDAMAGE_IMMUNE;
    emit(
        ctx,
        caster,
        ServerOpcodeMessage::SMSG_SPELLORDAMAGE_IMMUNE(Box::new(SMSG_SPELLORDAMAGE_IMMUNE {
            caster: Guid::new(caster),
            target: Guid::new(target),
            id: spell,
            debug_log_format: false,
        })),
        false,
    );
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
                super::spell::swing_miss_info(event.hit_info),
            ))),
            false,
        );
    } else if !event.spell_swing && ctx.db.game_native_player().count() != 0 {
        // A queued strike's swing follows its SMSG_SPELL_GO, so native/spell.rs sends it.
        let packet = codec::build_attacker_state_update(
            event.attacker_guid,
            event.target_guid,
            event.damage,
            event.hit_info,
            event.blocked_amount,
            0,
        );
        super::visibility::broadcast(ctx, event.attacker_guid, packet.opcode, packet.body, false);
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
    if event.killing_blow && !lyracore_shared::guid::is_unit(event.attacker_guid) {
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
            if old.is_none() && lyracore_shared::guid::is_unit(row.attacker_guid) {
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
    viewer_guid: u64,
) -> Result<(), String> {
    if let Some(row) = ctx
        .db
        .game_melee_attack()
        .attacker_guid()
        .find(guid)
        .filter(|row| row.ranged_spell_id == 0 && row.target_guid != viewer_guid)
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

pub(crate) fn combo_points_changed(
    ctx: &ReducerContext,
    owner_guid: u64,
    target_guid: u64,
    points: u32,
) {
    if let Some(owner) = super::world_session(ctx, owner_guid) {
        send_combo_points(ctx, owner.connection_id, owner_guid, target_guid, points);
    }
}

pub(super) fn replay_combo_points(ctx: &ReducerContext, connection: ConnectionId, guid: u64) {
    if let Some(held) = crate::combo::held(ctx, guid) {
        send_combo_points(ctx, connection, guid, held.target_guid, held.points);
    }
}

fn send_combo_points(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    target_guid: u64,
    points: u32,
) {
    let points = points.min(crate::combo::MAX_COMBO) as u8;
    let (opcode, body) = codec::build_combo_points_values(guid, target_guid, points);
    super::send_raw(ctx, connection, opcode, body, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_attack_on_a_returning_creature_is_answered_with_an_attack_stop() {
        let Rollback::Refusal { replies, .. } =
            attack_rollback(AttackRefusal::Evading, 1, 2, false)
        else {
            panic!("an evading target is a refusal, not a failure");
        };
        let stop = lyracore_protocol::world::encode(&ServerOpcodeMessage::SMSG_ATTACKSTOP(
            Box::new(codec::build_attack_stop(1, 2)),
        ))
        .unwrap();
        assert_eq!(replies.len(), 1);
        assert_eq!(
            (replies[0].opcode, &replies[0].body),
            (stop.opcode, &stop.body)
        );
    }

    #[test]
    fn an_attack_on_a_returning_creature_keeps_the_current_victim_without_a_stop() {
        let Rollback::Refusal { replies, .. } = attack_rollback(AttackRefusal::Evading, 1, 2, true)
        else {
            panic!("an evading target is a refusal, not a failure");
        };
        assert!(replies.is_empty());
    }
}
