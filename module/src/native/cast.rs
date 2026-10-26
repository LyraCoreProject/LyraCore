use lyracore_protocol::{
    codec,
    messages::vanilla::{
        opcodes::ServerOpcodeMessage, SpellCastTargets_SpellCastTargetFlags_Gameobject as GoTarget,
        SpellCastTargets_SpellCastTargetFlags_Item as ItemTarget, CMSG_CAST_SPELL,
    },
    world::Packet,
};
use lyracore_shared::spell::taxonomy::{
    E_DISENCHANT, E_ENCHANT_ITEM, E_FISH, E_NEXT_SWING, E_OPEN_LOCK, E_PERSISTENT_AREA,
    SPELL_ATTR_RANGED_AUTO_REPEAT,
};
use spacetimedb::{ConnectionId, ReducerContext};

use super::Rollback;
use crate::combat::game_melee_attack;
use crate::spell::{CastGateRefusal, CastRefusalReason};
use crate::Refusal;
use crate::{game_item_instance, game_item_template, game_spell, game_spell_effect};

const CAST_RESULT: u16 = 0x0130;

enum Route {
    Ordinary,
    Ranged,
    Enchant(u32),
    Disenchant,
    Fish,
    OpenLock,
}

fn packet(message: ServerOpcodeMessage) -> Result<Packet, String> {
    lyracore_protocol::world::encode(message).map_err(|error| error.to_string())
}

fn failure(spell: u32, reason: CastRefusalReason) -> Packet {
    Packet {
        opcode: CAST_RESULT,
        body: codec::build_cast_result_failed(spell, cast_result(reason)),
    }
}

fn cast_result(reason: CastRefusalReason) -> u8 {
    use codec::cast_failure as code;
    use CastRefusalReason as R;
    match reason {
        R::Unreported => code::DONT_REPORT,
        R::NoPower => code::NO_POWER,
        R::NotReady => code::NOT_READY,
        R::OutOfRange => code::OUT_OF_RANGE,
        R::TooClose => code::TOO_CLOSE,
        R::NotInFront => code::UNIT_NOT_INFRONT,
        R::NoLineOfSight => code::LINE_OF_SIGHT,
        R::NoAmmo => code::NO_AMMO,
        R::WrongStance => code::ONLY_SHAPESHIFT,
        R::NotBehind => code::NOT_BEHIND,
        R::NotStealthed => code::ONLY_STEALTHED,
        R::NoComboPoints => code::NO_COMBO_POINTS,
        R::SchoolLocked => code::INTERRUPTED,
        R::ReactWindowClosed => code::CASTER_AURASTATE,
        R::Stunned => code::STUNNED,
        R::LevelTooLow => code::LEVEL_REQUIREMENT,
        R::CasterDead | R::TargetDead => code::TARGETS_DEAD,
        R::BadTargets => code::BAD_TARGETS,
        R::MountWhileDead => code::CASTER_DEAD,
        R::MountInCombat => code::AFFECTING_COMBAT,
        R::MountIndoors => code::ONLY_OUTDOORS,
        R::MountInDeepWater => code::ONLY_ABOVEWATER,
    }
}

fn known(ctx: &ReducerContext, guid: u64, spell: u32) -> Result<(), Refusal> {
    if crate::spell::knows_spell(ctx, guid, spell) {
        Ok(())
    } else {
        Err(format!("spell {spell} is not in the caster's spellbook").into())
    }
}

fn item_slot(ctx: &ReducerContext, guid: u64, request: &CMSG_CAST_SPELL) -> Result<u8, Refusal> {
    let Some(ItemTarget::Item { item }) = request.targets.target_flags.get_item() else {
        return Err("cast requires an owned item target".into());
    };
    ctx.db
        .game_item_instance()
        .guid()
        .find(item.guid())
        .filter(|item| item.owner_guid == guid)
        .map(|item| item.slot)
        .ok_or_else(|| "item target is not in the Character's bag".into())
}

pub(super) fn receive(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    request: &CMSG_CAST_SPELL,
) -> Result<(), Rollback> {
    let caster = super::actor(ctx, guid).map_err(|refusal| {
        Rollback::replies(
            vec![failure(request.spell, CastRefusalReason::Unreported)],
            refusal,
        )
    })?;
    let spell = request.spell;
    let header = ctx.db.game_spell().spell_id().find(spell);
    let effects: Vec<_> = ctx
        .db
        .game_spell_effect()
        .by_spell()
        .filter(spell)
        .collect();
    let route = if header
        .as_ref()
        .is_some_and(|row| row.cast_flags & SPELL_ATTR_RANGED_AUTO_REPEAT != 0)
    {
        Route::Ranged
    } else if let Some(effect) = effects
        .iter()
        .find(|row| matches!(row.kind, E_ENCHANT_ITEM | E_DISENCHANT))
    {
        if effect.kind == E_ENCHANT_ITEM {
            Route::Enchant(effect.p0 as u32)
        } else {
            Route::Disenchant
        }
    } else if effects.iter().any(|row| row.kind == E_FISH) {
        Route::Fish
    } else if effects.iter().any(|row| row.kind == E_OPEN_LOCK) {
        Route::OpenLock
    } else {
        Route::Ordinary
    };
    let target = request
        .targets
        .target_flags
        .get_unit()
        .map_or(0, |target| target.unit_target.guid());
    let dest = request
        .targets
        .target_flags
        .get_dest_location()
        .map(|dest| (dest.destination.x, dest.destination.y, dest.destination.z));
    let mut prelude = Vec::new();
    if matches!(route, Route::Ordinary)
        && header.as_ref().is_none_or(|row| row.cast_time_ms == 0)
        && !effects.iter().any(|row| row.kind == E_NEXT_SWING)
    {
        let start = dest.map_or_else(
            || codec::build_spell_start(guid, spell, 0, 0, None),
            |dest| codec::build_spell_start_dest(guid, spell, dest),
        );
        let go = if let Some(dest) = dest {
            codec::build_spell_go_dest(guid, spell, dest)
        } else if effects.iter().any(|row| row.kind == E_PERSISTENT_AREA) {
            codec::build_spell_go_area(guid, spell)
        } else {
            codec::build_spell_go(guid, spell, target, None)
        };
        prelude = vec![
            packet(ServerOpcodeMessage::SMSG_SPELL_START(Box::new(start)))?,
            Packet {
                opcode: CAST_RESULT,
                body: codec::build_cast_result_ok(spell),
            },
            packet(ServerOpcodeMessage::SMSG_SPELL_GO(Box::new(go)))?,
        ];
        // Instant casts must release the owner's cast state before Aura updates.
        for packet in &prelude {
            super::send_raw(ctx, connection, packet.opcode, packet.body.clone(), false);
        }
    }
    let result = (|| -> Result<(), CastGateRefusal> {
        known(ctx, guid, spell)?;
        match route {
            Route::Ranged => crate::actor::ranged_attack(ctx, guid, target, spell),
            Route::Enchant(enchantment) => Ok(crate::professions::apply_enchant_item(
                ctx,
                guid,
                item_slot(ctx, guid, request)?,
                enchantment,
            )?),
            Route::Disenchant => Ok(crate::professions::apply_disenchant(
                ctx,
                guid,
                item_slot(ctx, guid, request)?,
            )?),
            Route::Fish => Ok(crate::professions::apply_fish(ctx, guid)?),
            Route::OpenLock => {
                let object = match request.targets.target_flags.get_gameobject() {
                    Some(GoTarget::Gameobject { gameobject }) => gameobject.guid(),
                    Some(GoTarget::ObjectUnk { object_unk }) => object_unk.guid(),
                    None => {
                        return Err("pick_lock: no GameObject target in cast".to_string().into())
                    }
                };
                crate::gameobject::apply_pick_lock(ctx, guid, object)
            }
            Route::Ordinary => {
                if matches!(
                    request.targets.target_flags.get_item(),
                    Some(ItemTarget::Item { .. })
                ) {
                    Ok(crate::creatures::apply_item_target_spell(
                        ctx,
                        guid,
                        spell,
                        item_slot(ctx, guid, request)?,
                    )?)
                } else if let Some((x, y, z)) = dest {
                    crate::spell::apply_cast_spell_at(ctx, &caster, spell, target, x, y, z)
                } else {
                    crate::spell::do_cast_spell(ctx, caster, spell, target)
                }
            }
        }
    })();
    if let Err(refusal) = result {
        let response = failure(spell, refusal.reason);
        if matches!(route, Route::Ranged) {
            // Ranged Gates precede all writes; a refused retarget still cancels the old repeat loop.
            cancel_repeat(ctx, guid);
            super::send_raw(ctx, connection, response.opcode, response.body, false);
            return Ok(());
        }
        prelude.push(response);
        return Err(Rollback::replies(prelude, refusal));
    }
    match route {
        Route::Ordinary => {}
        Route::Ranged => {
            let ammo = crate::combat::equipped_ranged_weapon(ctx, guid)
                .filter(|weapon| matches!(weapon.3, 2 | 3 | 18))
                .and_then(|_| crate::combat::find_ammo(ctx, guid))
                .and_then(|item| ctx.db.game_item_template().entry().find(item.entry))
                .map(|template| (template.display_id, 24));
            super::send(
                ctx,
                connection,
                ServerOpcodeMessage::SMSG_SPELL_START(Box::new(codec::build_spell_start(
                    guid, spell, 0, target, ammo,
                ))),
                false,
            )?;
        }
        _ => {
            super::send(
                ctx,
                connection,
                ServerOpcodeMessage::SMSG_SPELL_START(Box::new(codec::build_spell_start(
                    guid, spell, 0, 0, None,
                ))),
                false,
            )?;
            super::send_raw(
                ctx,
                connection,
                CAST_RESULT,
                codec::build_cast_result_ok(spell),
                false,
            );
            super::send(
                ctx,
                connection,
                ServerOpcodeMessage::SMSG_SPELL_GO(Box::new(codec::build_spell_go(
                    guid, spell, 0, None,
                ))),
                false,
            )?;
        }
    }
    Ok(())
}

pub(super) fn cancel_repeat(ctx: &ReducerContext, guid: u64) {
    if ctx
        .db
        .game_melee_attack()
        .attacker_guid()
        .find(guid)
        .is_some_and(|row| row.ranged_spell_id != 0)
    {
        crate::combat::MeleeAttack::end_auto_repeat(ctx, guid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cast_refusal_reason_reports_its_1_12_1_cast_result() {
        use CastRefusalReason as R;
        for (reason, code) in [
            (R::Unreported, 0x17),
            (R::NoPower, 0x4D),
            (R::NotReady, 0x3C),
            (R::OutOfRange, 0x59),
            (R::TooClose, 0x76),
            (R::NotInFront, 0x7C),
            (R::NoLineOfSight, 0x2A),
            (R::NoAmmo, 0x24),
            (R::WrongStance, 0x56),
            (R::NotBehind, 0x33),
            (R::NotStealthed, 0x57),
            (R::NoComboPoints, 0x46),
            (R::SchoolLocked, 0x23),
            (R::ReactWindowClosed, 0x12),
            (R::Stunned, 0x64),
            (R::LevelTooLow, 0x29),
            (R::CasterDead, 0x65),
            (R::TargetDead, 0x65),
            (R::BadTargets, 0x0A),
            (R::MountWhileDead, 0x13),
            (R::MountInCombat, 0x00),
            (R::MountIndoors, 0x55),
            (R::MountInDeepWater, 0x50),
        ] {
            assert_eq!(cast_result(reason), code, "{reason:?}");
        }
    }

    #[test]
    fn a_refused_cast_sends_its_reason_in_smsg_cast_result() {
        let packet = failure(133, CastRefusalReason::NoPower);
        assert_eq!(packet.opcode, 0x0130);
        assert_eq!(packet.body, vec![133, 0, 0, 0, 0x02, 0x4D]);
    }
}
