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

use crate::combat::game_melee_attack;
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

fn failure(spell: u32, reason: &str) -> Packet {
    Packet {
        opcode: CAST_RESULT,
        body: codec::build_cast_result_failed(spell, codec::cast_failure_reason_for(reason)),
    }
}

fn known(ctx: &ReducerContext, guid: u64, spell: u32) -> Result<(), String> {
    if crate::spell::knows_spell(ctx, guid, spell) {
        Ok(())
    } else {
        Err(format!("spell {spell} is not in the caster's spellbook"))
    }
}

fn item_slot(ctx: &ReducerContext, guid: u64, request: &CMSG_CAST_SPELL) -> Result<u8, String> {
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
) -> Result<(), String> {
    let caster = super::actor(ctx, guid)
        .map_err(|cause| super::rollback_packets(vec![failure(request.spell, &cause)], cause))?;
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
    let result = known(ctx, guid, spell).and_then(|_| match route {
        Route::Ranged => crate::actor::ranged_attack(ctx, guid, target, spell),
        Route::Enchant(enchantment) => crate::professions::apply_enchant_item(
            ctx,
            guid,
            item_slot(ctx, guid, request)?,
            enchantment,
        ),
        Route::Disenchant => {
            crate::professions::apply_disenchant(ctx, guid, item_slot(ctx, guid, request)?)
        }
        Route::Fish => crate::professions::apply_fish(ctx, guid),
        Route::OpenLock => {
            let object = match request.targets.target_flags.get_gameobject() {
                Some(GoTarget::Gameobject { gameobject }) => gameobject.guid(),
                Some(GoTarget::ObjectUnk { object_unk }) => object_unk.guid(),
                None => return Err("pick_lock: no GameObject target in cast".into()),
            };
            crate::gameobject::apply_pick_lock(ctx, guid, object)
        }
        Route::Ordinary => {
            if matches!(
                request.targets.target_flags.get_item(),
                Some(ItemTarget::Item { .. })
            ) {
                crate::creatures::apply_item_target_spell(
                    ctx,
                    guid,
                    spell,
                    item_slot(ctx, guid, request)?,
                )
            } else if let Some((x, y, z)) = dest {
                crate::spell::apply_cast_spell_at(ctx, &caster, spell, target, x, y, z)
            } else {
                crate::spell::do_cast_spell(ctx, caster, spell, target)
            }
        }
    });
    if let Err(cause) = result {
        let response = failure(spell, &cause);
        if matches!(route, Route::Ranged) {
            // Ranged Gates precede all writes; a refused retarget still cancels the old repeat loop.
            cancel_repeat(ctx, guid);
            super::send_raw(ctx, connection, response.opcode, response.body, false);
            return Ok(());
        }
        prelude.push(response);
        return Err(super::rollback_packets(prelude, cause));
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
        crate::combat::stop_attack_for(ctx, guid);
    }
}
