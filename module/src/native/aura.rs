use lyracore_protocol::{
    codec,
    messages::vanilla::{
        opcodes::ServerOpcodeMessage, MSG_CHANNEL_START_Server, MSG_CHANNEL_UPDATE_Server,
    },
    world::Packet,
};
use lyracore_shared::spell::taxonomy::*;
use spacetimedb::{ConnectionId, ReducerContext, Table};

use super::{bounded as packet, game_native_player};
use crate::{game_aura, game_spell, game_world_entity, Aura};

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

fn is_visible(aura: &Aura) -> bool {
    u16::from(aura.slot) < codec::update_mask::idx::AURA_SLOTS
}

/// Merges the effect rows of each visible Aura into its one client slot.
pub(super) fn client_slots<'a>(
    auras: impl IntoIterator<Item = &'a Aura>,
) -> Vec<codec::update_mask::AuraSlot> {
    let mut slots = std::collections::BTreeMap::new();
    for aura in auras.into_iter().filter(|aura| is_visible(aura)) {
        slots
            .entry(aura.slot)
            .and_modify(|slot: &mut codec::update_mask::AuraSlot| {
                slot.stacks = slot.stacks.max(aura.stacks);
                slot.charges = slot.charges.max(aura.proc_charges);
            })
            .or_insert(codec::update_mask::AuraSlot {
                slot: aura.slot,
                spell_id: aura.spell_id,
                flags: aura.flags,
                level: aura.level,
                stacks: aura.stacks,
                charges: aura.proc_charges,
            });
    }
    slots.into_values().collect()
}

fn sync(guid: u64, auras: &[Aura]) -> Packet {
    let (opcode, body) = codec::build_values_update_raw(
        guid,
        &codec::update_mask::full_aura_mask(&client_slots(auras)),
    );
    Packet { opcode, body }
}

fn duration(ctx: &ReducerContext, aura: &Aura) -> Option<Packet> {
    let expiry = aura.expires_at.to_micros_since_unix_epoch();
    // i64::MAX marks a permanent Aura.
    if expiry == i64::MAX || !is_visible(aura) {
        return None;
    }
    let remaining =
        expiry.saturating_sub(crate::time::now(ctx).to_micros_since_unix_epoch()) / 1_000;
    (remaining > 0).then(|| {
        packet(ServerOpcodeMessage::SMSG_UPDATE_AURA_DURATION(
            codec::build_aura_duration(aura.slot, remaining.min(u32::MAX as i64) as u32),
        ))
    })
}

fn channel(ctx: &ReducerContext, aura: &Aura, removed: bool) {
    if !super::has_world_session(ctx, aura.caster_guid) {
        return;
    }
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
    if removed {
        if active.is_none() {
            channel_ended(ctx, aura.caster_guid);
        }
    } else if active.is_some_and(|row| row.id == aura.id) {
        let duration = aura
            .expires_at
            .to_micros_since_unix_epoch()
            .saturating_sub(crate::time::now(ctx).to_micros_since_unix_epoch())
            / 1_000;
        channel_started(
            ctx,
            aura.caster_guid,
            aura.spell_id,
            duration.clamp(0, u32::MAX as i64) as u32,
        );
    }
}

pub(crate) fn channel_started(ctx: &ReducerContext, guid: u64, spell_id: u32, duration_ms: u32) {
    if !super::has_world_session(ctx, guid) {
        return;
    }
    super::send_to_character(
        ctx,
        guid,
        ServerOpcodeMessage::MSG_CHANNEL_START(MSG_CHANNEL_START_Server {
            spell: spell_id,
            duration: duration_ms,
        }),
        super::EncodeFailure::Panic,
    );
}

pub(crate) fn channel_ended(ctx: &ReducerContext, guid: u64) {
    if !super::has_world_session(ctx, guid) {
        return;
    }
    super::send_to_character(
        ctx,
        guid,
        ServerOpcodeMessage::MSG_CHANNEL_UPDATE(MSG_CHANNEL_UPDATE_Server { time: 0 }),
        super::EncodeFailure::Panic,
    );
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
    let holds = |aura: &Aura| crate::modifiers::holds_in_place(aura.eff_kind, aura.eff_p0);
    if touched(holds) && own {
        super::motion_permit::refresh_character(ctx, guid);
    }
    if touched(holds) && (own || lyracore_shared::guid::is_unit(guid)) {
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
    if touched(|aura| crate::modifiers::moves_run_speed(aura.eff_kind, aura.eff_p0)) {
        let speed = crate::modifiers::run_speed(
            7.0,
            current.iter().map(crate::modifiers::AuraAmount::from),
        );
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
            && crate::modifiers::stacked_amount(aura.amount, aura.stacks) < 0
    };
    if touched(slows) {
        let slow = current
            .iter()
            .filter(|aura| slows(aura))
            .map(|aura| crate::modifiers::stacked_amount(aura.amount, aura.stacks))
            .sum();
        let speed = crate::modifiers::move_speed_with_pct(
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
        let speed = crate::modifiers::swim_speed(
            lyracore_shared::constants::speeds::SWIM,
            current.iter().map(crate::modifiers::AuraAmount::from),
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
    super::motion_permit::refresh_character(ctx, new.guid);
    if !super::has_world_session(ctx, new.guid) || super::is_world_porting(ctx, new.guid) {
        return;
    }
    let held = crate::spell::auras_on(ctx, new.guid)
        .any(|aura| crate::modifiers::holds_in_place(aura.eff_kind, aura.eff_p0));
    let rooted = (new.dead && !ghost) || held;
    if old.dead != new.dead || ((old.dead && !was_ghost) || held) != rooted {
        let (opcode, body) = codec::build_force_move_root_raw(new.guid, rooted);
        super::send_raw_to_character(ctx, new.guid, opcode, body);
    }
    if was_ghost != ghost {
        super::send_to_character(
            ctx,
            new.guid,
            codec::build_move_water_walk(new.guid, ghost),
            super::EncodeFailure::Panic,
        );
    }
}

pub(super) fn armor(ctx: &ReducerContext, guid: u64, current: &[Aura]) -> Option<Packet> {
    let entity = ctx.db.game_world_entity().guid().find(guid)?;
    let resistances = std::array::from_fn(|index| {
        crate::modifiers::effective_resistance(
            if index == 0 { entity.armor } else { 0 },
            crate::modifiers::resistance_bonus(ctx, guid, 1 << index),
        )
    });
    Some(packet(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
        codec::build_armor_values(
            guid,
            resistances,
            crate::modifiers::positive_armor(
                current.iter().map(crate::modifiers::AuraAmount::from),
            ),
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
    // Tick deadlines and proc cooldowns have no client-visible representation.
    if previous.zip(current).is_some_and(|(old, new)| {
        old.slot == new.slot
            && old.spell_id == new.spell_id
            && old.flags == new.flags
            && old.level == new.level
            && old.expires_at == new.expires_at
            && old.amount == new.amount
            && old.stacks == new.stacks
            && old.proc_charges == new.proc_charges
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
    if previous.into_iter().chain(current).any(is_visible) {
        let packet = sync(row.target_guid, &auras);
        super::visibility::broadcast(ctx, row.target_guid, packet.opcode, packet.body, false);
    }
    let changed: Vec<_> = previous.into_iter().chain(current).collect();
    for packet in movement(ctx, row.target_guid, &auras, &changed, false) {
        super::visibility::broadcast(ctx, row.target_guid, packet.opcode, packet.body, true);
    }
    if super::has_world_session(ctx, row.target_guid) {
        let mut packets = movement(ctx, row.target_guid, &auras, &changed, true);
        if changed.iter().any(|aura| {
            matches!(
                aura.eff_kind,
                A_MOD_DAMAGE_DONE_FLAT | A_MOD_DAMAGE_DONE_PCT
            )
        }) {
            let values = crate::spell::bonuses::damage_fields(ctx, row.target_guid);
            let (opcode, body) = codec::build_damage_bonus_values(row.target_guid, &values);
            packets.push(Packet { opcode, body });
        }
        if changed
            .iter()
            .any(|row| matches!(row.eff_kind, A_SPELLMOD_FLAT | A_SPELLMOD_PCT))
        {
            packets.extend(modifiers(&auras, &changed));
        }
        if previous
            .zip(current)
            .is_none_or(|(old, new)| old.expires_at != new.expires_at || old.slot != new.slot)
        {
            packets.extend(current.and_then(|aura| duration(ctx, aura)));
        }
        if changed
            .iter()
            .any(|aura| aura.eff_kind == A_MOD_RESISTANCE && aura.eff_p0 & 0x7f != 0)
        {
            packets.extend(armor(ctx, row.target_guid, &auras));
        }
        super::send_packets_to_character(ctx, row.target_guid, packets);
    }
}

pub(super) fn replay(ctx: &ReducerContext, connection: ConnectionId, guid: u64, own: bool) {
    let auras: Vec<_> = ctx.db.game_aura().by_target().filter(guid).collect();
    let mut packets = movement(ctx, guid, &auras, &auras.iter().collect::<Vec<_>>(), own);
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
        }
    }
    for packet in packets {
        super::send_raw(ctx, connection, packet.opcode, packet.body, false);
    }
}

#[cfg(test)]
mod tests {
    use spacetimedb::Timestamp;

    use super::*;

    fn row(effect_id: u64, stacks: u8, proc_charges: u8) -> Aura {
        Aura {
            id: effect_id,
            target_guid: 42,
            caster_guid: 42,
            spell_id: 7386,
            slot: 3,
            level: 60,
            flags: 0x09,
            applied_at: Timestamp::UNIX_EPOCH,
            expires_at: Timestamp::UNIX_EPOCH,
            effect_id,
            eff_kind: 0,
            amount: 0,
            eff_p0: 0,
            eff_p0_kind: 0,
            eff_p1: 0,
            period_ms: 0,
            amount_remaining: 0,
            stacks,
            next_tick_micros: 0,
            channel_target: 0,
            enters_combat: false,
            proc_flags: 0,
            proc_chance: 0,
            proc_ppm: 0.0,
            proc_ex: 0,
            proc_school_mask: 0,
            proc_family_name: 0,
            proc_family_flags: 0,
            proc_charges,
            proc_icd_ms: 0,
            proc_ready_micros: 0,
        }
    }

    #[test]
    fn the_effect_rows_of_one_aura_share_one_slot_with_the_largest_counts() {
        let slots = client_slots(&[row(1, 2, 0), row(2, 1, 3)]);
        let slots: Vec<_> = slots
            .iter()
            .map(|slot| (slot.slot, slot.spell_id, slot.stacks, slot.charges))
            .collect();
        assert_eq!(slots, [(3, 7386, 2, 3)]);
    }
}
