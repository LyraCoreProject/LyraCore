use lyracore_protocol::{codec, messages::vanilla::opcodes::ServerOpcodeMessage, world::Packet};
use lyracore_shared::{aura::AuraAmount, spell::taxonomy::*};
use spacetimedb::{ConnectionId, ReducerContext, Table};

use super::game_native_player;
use crate::{game_aura, game_spell, game_world_entity, Aura};

fn amount(aura: &Aura) -> AuraAmount {
    AuraAmount {
        kind: aura.eff_kind,
        parameter: aura.eff_p0,
        amount: aura.amount,
        stacks: aura.stacks,
    }
}

fn packet(message: ServerOpcodeMessage) -> Packet {
    lyracore_protocol::world::encode(message).expect("bounded aura message fits a world frame")
}

fn modifiers(current: &[Aura], previous: &[&Aura]) -> Vec<Packet> {
    let rows = current
        .iter()
        .map(|row| (row, row.amount))
        .chain(previous.iter().map(|row| (*row, 0)))
        .filter(|(row, _)| matches!(row.eff_kind, A_SPELLMOD_FLAT | A_SPELLMOD_PCT))
        .map(|(row, amount)| {
            (
                row.eff_p1 as u32,
                row.eff_p0 as u8,
                amount,
                row.eff_kind == A_SPELLMOD_PCT,
            )
        })
        .collect::<Vec<_>>();
    codec::build_spell_modifier_msgs(&rows)
        .into_iter()
        .map(packet)
        .collect()
}

fn sync(guid: u64, auras: &[Aura]) -> Packet {
    let slots: Vec<_> = auras
        .iter()
        .map(|aura| codec::update_mask::AuraSlot {
            slot: aura.slot,
            spell_id: aura.spell_id,
            flags: aura.flags,
            level: aura.level,
        })
        .collect();
    let (opcode, body) =
        codec::build_values_update_raw(guid, &codec::update_mask::full_aura_mask(&slots));
    Packet { opcode, body }
}

fn duration(ctx: &ReducerContext, aura: &Aura) -> Option<Packet> {
    let expiry = aura.expires_at.to_micros_since_unix_epoch();
    // i64::MAX represents a toggle Aura with no timer.
    if expiry == i64::MAX {
        return None;
    }
    let remaining = expiry.saturating_sub(ctx.timestamp.to_micros_since_unix_epoch()) / 1_000;
    (remaining > 0).then(|| {
        packet(ServerOpcodeMessage::SMSG_UPDATE_AURA_DURATION(
            codec::build_aura_duration(aura.slot, remaining.min(u32::MAX as i64) as u32),
        ))
    })
}

fn channel(ctx: &ReducerContext, aura: &Aura, removed: bool) {
    let Some(owner) = ctx
        .db
        .game_native_player()
        .character_guid()
        .find(aura.caster_guid)
    else {
        return;
    };
    let is_channel = |row: &Aura| {
        let flags = ctx
            .db
            .game_spell()
            .spell_id()
            .find(row.spell_id)
            .map_or(0, |spell| spell.cast_flags);
        lyracore_shared::spell::is_channel_aura(row.eff_kind, flags)
    };
    if !is_channel(aura) {
        return;
    }
    let active = ctx
        .db
        .game_aura()
        .by_caster()
        .filter(aura.caster_guid)
        .filter(is_channel)
        .min_by_key(|row| row.id);
    use lyracore_protocol::messages::vanilla::{
        MSG_CHANNEL_START_Server, MSG_CHANNEL_UPDATE_Server,
    };
    let message = if removed {
        if active.is_some() {
            return;
        }
        ServerOpcodeMessage::MSG_CHANNEL_UPDATE(MSG_CHANNEL_UPDATE_Server { time: 0 })
    } else {
        if active.is_none_or(|row| row.id != aura.id) {
            return;
        }
        let duration = aura
            .expires_at
            .to_micros_since_unix_epoch()
            .saturating_sub(ctx.timestamp.to_micros_since_unix_epoch())
            / 1_000;
        ServerOpcodeMessage::MSG_CHANNEL_START(MSG_CHANNEL_START_Server {
            spell: aura.spell_id,
            duration: duration.clamp(0, u32::MAX as i64) as u32,
        })
    };
    let packet = packet(message);
    super::send_raw(ctx, owner.connection_id, packet.opcode, packet.body, false);
}

fn movement(
    ctx: &ReducerContext,
    guid: u64,
    current: &[Aura],
    changed: &[&Aura],
    own: bool,
) -> Vec<Packet> {
    let mut out = Vec::new();
    let touched = |rule: fn(&Aura) -> bool| changed.iter().any(|aura| rule(aura));
    let holds = |aura: &Aura| lyracore_shared::aura::holds_in_place(aura.eff_kind, aura.eff_p0);
    if touched(holds) && (own || matches!(guid >> 48, 0xf130 | 0xf140)) {
        let corpse = own
            && ctx
                .db
                .game_world_entity()
                .guid()
                .find(guid)
                .is_some_and(|e| {
                    e.dead && e.player_flags & lyracore_shared::constants::player_flags::GHOST == 0
                });
        let rooted = corpse || current.iter().any(holds);
        let (opcode, body) = if own {
            codec::build_force_move_root_raw(guid, rooted)
        } else {
            codec::build_spline_move_root_raw(guid, rooted)
        };
        out.push(Packet { opcode, body });
    }
    if touched(|aura| lyracore_shared::aura::moves_run_speed(aura.eff_kind, aura.eff_p0)) {
        let speed = lyracore_shared::aura::run_speed(7.0, current.iter().map(amount));
        out.push(packet(if own {
            ServerOpcodeMessage::SMSG_FORCE_RUN_SPEED_CHANGE(Box::new(
                codec::build_force_run_speed(guid, speed),
            ))
        } else {
            codec::build_spline_speed(guid, codec::ObservedSpeed::Run, speed)
        }));
    }
    let slows = |aura: &Aura| {
        aura.eff_kind == A_MOD_SPEED
            && aura.eff_p0 == i32::from(SPEED_MOVE)
            && lyracore_shared::aura::stacked_amount(aura.amount, aura.stacks) < 0
    };
    if touched(slows) {
        let slow = current
            .iter()
            .filter(|aura| slows(aura))
            .map(|aura| lyracore_shared::aura::stacked_amount(aura.amount, aura.stacks))
            .sum();
        let speed = lyracore_shared::aura::move_speed_with_pct(
            lyracore_shared::constants::speeds::RUN_BACK,
            slow,
        );
        out.push(packet(if own {
            ServerOpcodeMessage::SMSG_FORCE_RUN_BACK_SPEED_CHANGE(Box::new(
                codec::build_force_run_back_speed(guid, speed),
            ))
        } else {
            codec::build_spline_speed(guid, codec::ObservedSpeed::RunBack, speed)
        }));
    }
    if touched(|aura| aura.eff_kind == A_MOD_SPEED && aura.eff_p0 == i32::from(SPEED_SWIM)) {
        let speed = lyracore_shared::aura::swim_speed(
            lyracore_shared::constants::speeds::SWIM,
            current.iter().map(amount),
        );
        out.push(packet(if own {
            ServerOpcodeMessage::SMSG_FORCE_SWIM_SPEED_CHANGE(Box::new(
                codec::build_force_swim_speed(guid, speed),
            ))
        } else {
            codec::build_spline_speed(guid, codec::ObservedSpeed::Swim, speed)
        }));
    }
    out
}

pub(super) fn life_changed(
    ctx: &ReducerContext,
    old: &crate::WorldEntity,
    new: &crate::WorldEntity,
) {
    const GHOST: u32 = lyracore_shared::constants::player_flags::GHOST;
    let was_ghost = old.player_flags & GHOST != 0;
    let ghost = new.player_flags & GHOST != 0;
    if old.dead == new.dead && was_ghost == ghost {
        return;
    }
    let Some(owner) = ctx.db.game_native_player().character_guid().find(new.guid) else {
        return;
    };
    if owner.teleport == Some(super::teleport::NativeTeleport::WorldPort) {
        return;
    }
    let held = crate::spell::auras_on(ctx, new.guid)
        .any(|aura| lyracore_shared::aura::holds_in_place(aura.eff_kind, aura.eff_p0));
    let rooted = (new.dead && !ghost) || held;
    if old.dead != new.dead || ((old.dead && !was_ghost) || held) != rooted {
        let (opcode, body) = codec::build_force_move_root_raw(new.guid, rooted);
        super::send_raw(ctx, owner.connection_id, opcode, body, false);
    }
    if was_ghost != ghost {
        let packet = packet(codec::build_move_water_walk(new.guid, ghost));
        super::send_raw(ctx, owner.connection_id, packet.opcode, packet.body, false);
    }
}

pub(super) fn armor(ctx: &ReducerContext, guid: u64, current: &[Aura]) -> Option<Packet> {
    let entity = ctx.db.game_world_entity().guid().find(guid)?;
    let resistances = std::array::from_fn(|index| {
        lyracore_shared::resistance::effective_resistance(
            if index == 0 { entity.armor } else { 0 },
            crate::spell::resistance_bonus(ctx, guid, 1 << index),
        )
    });
    Some(packet(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
        codec::build_armor_values(
            guid,
            resistances,
            lyracore_shared::resistance::positive_armor(current.iter().map(amount)),
        ),
    ))))
}

pub(crate) fn changed(ctx: &ReducerContext, previous: Option<&Aura>, current: Option<&Aura>) {
    if ctx.db.game_native_player().count() == 0 {
        return;
    }
    let Some(row) = current.or(previous) else {
        return;
    };
    // Tick deadlines and proc counters have no client-visible representation.
    if previous.zip(current).is_some_and(|(old, new)| {
        old.slot == new.slot
            && old.spell_id == new.spell_id
            && old.flags == new.flags
            && old.level == new.level
            && old.expires_at == new.expires_at
            && old.amount == new.amount
            && old.stacks == new.stacks
            && old.eff_kind == new.eff_kind
            && old.eff_p0 == new.eff_p0
            && old.eff_p1 == new.eff_p1
    }) {
        return;
    }
    if previous.is_none() || current.is_none() {
        channel(ctx, row, current.is_none());
    }
    let auras: Vec<_> = ctx
        .db
        .game_aura()
        .by_target()
        .filter(row.target_guid)
        .collect();
    let packet = sync(row.target_guid, &auras);
    super::visibility::broadcast(ctx, row.target_guid, packet.opcode, packet.body, false);
    let changed: Vec<_> = previous.into_iter().chain(current).collect();
    for packet in movement(ctx, row.target_guid, &auras, &changed, false) {
        super::visibility::broadcast(ctx, row.target_guid, packet.opcode, packet.body, true);
    }
    if let Some(owner) = ctx
        .db
        .game_native_player()
        .character_guid()
        .find(row.target_guid)
    {
        let mut packets = movement(ctx, row.target_guid, &auras, &changed, true);
        if changed
            .iter()
            .any(|row| matches!(row.eff_kind, A_SPELLMOD_FLAT | A_SPELLMOD_PCT))
        {
            packets.extend(modifiers(&auras, &changed));
        }
        if previous.is_none_or(|old| Some(old.expires_at) != current.map(|new| new.expires_at)) {
            packets.extend(current.and_then(|aura| duration(ctx, aura)));
        }
        if changed
            .iter()
            .any(|aura| aura.eff_kind == A_MOD_RESISTANCE && aura.eff_p0 & 0x7f != 0)
        {
            packets.extend(armor(ctx, row.target_guid, &auras));
        }
        for packet in packets {
            super::send_raw(ctx, owner.connection_id, packet.opcode, packet.body, false);
        }
    }
}

pub(super) fn replay(ctx: &ReducerContext, connection: ConnectionId, guid: u64, own: bool) {
    let auras: Vec<_> = ctx.db.game_aura().by_target().filter(guid).collect();
    let mut packets = Vec::new();
    if !auras.is_empty() {
        packets.push(sync(guid, &auras));
    }
    packets.extend(movement(
        ctx,
        guid,
        &auras,
        &auras.iter().collect::<Vec<_>>(),
        own,
    ));
    if own {
        packets.extend(modifiers(&auras, &[]));
        packets.extend(auras.iter().filter_map(|aura| duration(ctx, aura)));
        packets.extend(armor(ctx, guid, &auras));
        if let Some(entity) = ctx.db.game_world_entity().guid().find(guid) {
            let ghost = entity.player_flags & lyracore_shared::constants::player_flags::GHOST != 0;
            if entity.dead && !ghost {
                let (opcode, body) = codec::build_force_move_root_raw(guid, true);
                packets.push(Packet { opcode, body });
            }
            if ghost {
                packets.push(packet(codec::build_move_water_walk(guid, true)));
            }
            packets.push(packet(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                codec::build_sheet_stats_values(guid, &super::view::sheet_stats(&entity)),
            ))));
        }
    }
    for packet in packets {
        super::send_raw(ctx, connection, packet.opcode, packet.body, false);
    }
}
