use super::game_native_player;
use crate::spell::{SpellCastEvent, SpellCastEventKind, SpellImpactEvent};
use lyracore_protocol::{codec, messages as wow_world_messages, world::Packet};
use spacetimedb::{ReducerContext, Table};
use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;

#[derive(Clone, Copy, PartialEq, Eq)]
enum EventAudience {
    Everyone,
    Source,
    Peers,
}

fn message(value: ServerOpcodeMessage) -> Packet {
    lyracore_protocol::world::encode(value).expect("bounded spell event fits a world frame")
}

fn deliver(ctx: &ReducerContext, caster: u64, audience: EventAudience, packet: Packet) {
    match audience {
        EventAudience::Source => {
            if let Some(player) = ctx.db.game_native_player().character_guid().find(caster) {
                super::send_raw(ctx, player.connection_id, packet.opcode, packet.body, false);
            }
        }
        EventAudience::Everyone | EventAudience::Peers => {
            super::visibility::broadcast(
                ctx,
                caster,
                packet.opcode,
                packet.body,
                audience == EventAudience::Peers,
            );
        }
    }
}

pub(crate) fn cast_event(ctx: &ReducerContext, event: &SpellCastEvent) {
    if ctx.db.game_native_player().count() == 0 {
        return;
    }
    for (audience, packet) in cast_event_messages(event, projectile(ctx, event)) {
        deliver(ctx, event.caster_guid, audience, packet);
    }
}

// vmangos sets CAST_FLAG_AMMO on START and GO for every ranged-slot spell.
fn projectile(ctx: &ReducerContext, event: &SpellCastEvent) -> Option<(u32, u32)> {
    use crate::spell::game_spell;
    if event.kind != SpellCastEventKind::Start as u8 && event.kind != SpellCastEventKind::Go as u8 {
        return None;
    }
    ctx.db
        .game_spell()
        .spell_id()
        .find(event.spell_id)
        .filter(|spell| spell.attributes & crate::spell::SPELL_ATTR_USES_RANGED_SLOT != 0)?;
    crate::combat::ranged_projectile(ctx, event.caster_guid)
}

pub(crate) fn spell_impact(ctx: &ReducerContext, event: &SpellImpactEvent) {
    if ctx.db.game_native_player().count() == 0 {
        return;
    }
    let packet = message(ServerOpcodeMessage::SMSG_SPELLNONMELEEDAMAGELOG(Box::new(
        codec::build_spell_non_melee_damage_log(
            event.target_guid,
            event.caster_guid,
            event.spell_id,
            event.damage,
            event.school,
            event.is_crit,
            event.resisted,
            event.absorbed,
        ),
    )));
    deliver(ctx, event.caster_guid, EventAudience::Everyone, packet);
}

fn damage_log(row: &SpellCastEvent) -> Option<Packet> {
    (row.damage > 0).then(|| {
        let log = codec::build_spell_non_melee_damage_log(
            row.target_guid,
            row.caster_guid,
            row.spell_id,
            row.damage,
            row.school,
            row.is_crit,
            row.resisted,
            row.absorbed,
        );
        message(ServerOpcodeMessage::SMSG_SPELLNONMELEEDAMAGELOG(Box::new(
            log,
        )))
    })
}

fn cast_event_messages(
    row: &SpellCastEvent,
    ammo: Option<(u32, u32)>,
) -> Vec<(EventAudience, Packet)> {
    let mut out = Vec::new();
    let kind = match row.kind {
        1 => SpellCastEventKind::Start,
        2 => SpellCastEventKind::Go,
        3 => SpellCastEventKind::Interrupt,
        4 => SpellCastEventKind::Pushback,
        5 => SpellCastEventKind::ProcLog,
        6 => SpellCastEventKind::PeriodicLog,
        _ => return out,
    };
    if kind == SpellCastEventKind::Interrupt {
        // vmangos Spell::cancel order: unit teardown for everyone, then the caster's failed result.
        out.push((
            EventAudience::Everyone,
            message(ServerOpcodeMessage::SMSG_SPELL_FAILED_OTHER(Box::new(
                codec::build_spell_failed_other(row.caster_guid, row.spell_id),
            ))),
        ));
        // Only a failed SMSG_CAST_RESULT releases the client's in-flight cast and its cast bar.
        if let Some(reason) = codec::cast_failure_reason_for_code(row.failure_reason) {
            out.push((
                EventAudience::Source,
                Packet {
                    opcode: 0x0130,
                    body: codec::build_cast_result_failed(row.spell_id, reason),
                },
            ));
        }
        return out;
    }
    if kind == SpellCastEventKind::Pushback {
        let m = codec::build_spell_delayed(row.caster_guid, row.delay_ms);
        out.push((
            EventAudience::Everyone,
            message(ServerOpcodeMessage::SMSG_SPELL_DELAYED(Box::new(m))),
        ));
        return out;
    }
    if kind == SpellCastEventKind::ProcLog {
        out.extend(damage_log(row).map(|packet| (EventAudience::Everyone, packet)));
        return out;
    }
    if kind == SpellCastEventKind::PeriodicLog {
        out.extend(periodic_log_outbound(row).map(|out| (EventAudience::Everyone, out)));
        return out;
    }
    if kind == SpellCastEventKind::Start {
        let start =
            codec::build_spell_start(row.caster_guid, row.spell_id, row.cast_time_ms, 0, ammo);
        out.push((
            EventAudience::Everyone,
            message(ServerOpcodeMessage::SMSG_SPELL_START(Box::new(start))),
        ));
        return out;
    }
    let launch_audience = if row.client_initiated {
        EventAudience::Peers
    } else {
        EventAudience::Everyone
    };
    if !row.is_completion {
        let start = codec::build_spell_start(row.caster_guid, row.spell_id, 0, 0, ammo);
        out.push((
            launch_audience,
            message(ServerOpcodeMessage::SMSG_SPELL_START(Box::new(start))),
        ));
    }
    // Timed completions need caster-only CAST_RESULT before GO; instant casts already sent it.
    if row.is_completion && !row.client_initiated {
        out.push((
            EventAudience::Source,
            Packet {
                opcode: 0x0130,
                body: codec::build_cast_result_ok(row.spell_id),
            },
        ));
    }
    let mut go = codec::build_spell_go(row.caster_guid, row.spell_id, row.target_guid, ammo);
    if row.is_completion && row.damage == 0 {
        use wow_world_messages::vanilla::{SpellMiss, SpellMissInfo};
        let miss_info = match row.swing_hit_info {
            2 => Some(SpellMissInfo::Miss),
            3 => Some(SpellMissInfo::Dodge),
            4 => Some(SpellMissInfo::Parry),
            _ => None,
        };
        if let Some(mi) = miss_info {
            go.hits.clear();
            go.misses = vec![SpellMiss {
                target: wow_world_messages::Guid::new(row.target_guid),
                miss_info: mi,
            }];
        }
    }
    if let Some(mi) = cast_miss_info(row.miss_info) {
        use wow_world_messages::vanilla::SpellMiss;
        go.hits.clear();
        go.misses = vec![SpellMiss {
            target: wow_world_messages::Guid::new(row.target_guid),
            miss_info: mi,
        }];
    }
    out.push((
        launch_audience,
        message(ServerOpcodeMessage::SMSG_SPELL_GO(Box::new(go))),
    ));
    if row.client_initiated {
        if let Some(mi) = cast_miss_info(row.miss_info) {
            // The cast handler already sent this caster a GO listing the target as hit.
            out.push((
                EventAudience::Source,
                Packet {
                    opcode: codec::SMSG_SPELLLOGMISS_OPCODE,
                    body: codec::build_spell_log_miss(
                        row.caster_guid,
                        row.spell_id,
                        row.target_guid,
                        mi.as_int(),
                    ),
                },
            ));
        }
    }

    out.extend(damage_log(row).map(|packet| (EventAudience::Everyone, packet)));
    if row.healed > 0 {
        use wow_world_messages::vanilla::SMSG_SPELLHEALLOG;
        use wow_world_messages::Guid;
        out.push((
            EventAudience::Everyone,
            message(ServerOpcodeMessage::SMSG_SPELLHEALLOG(Box::new(
                SMSG_SPELLHEALLOG {
                    victim: Guid::new(row.target_guid),
                    caster: Guid::new(row.caster_guid),
                    id: row.spell_id,
                    damage: row.healed,
                    critical: row.is_crit,
                },
            ))),
        ));
    }

    if row.energized > 0 {
        out.push((
            EventAudience::Everyone,
            Packet {
                opcode: codec::SMSG_SPELLENERGIZELOG_OPCODE,
                body: codec::build_spell_energize_log(
                    row.target_guid,
                    row.caster_guid,
                    row.spell_id,
                    row.power_type,
                    row.energized,
                ),
            },
        ));
    }

    // SMSG_SPELL_GO releases pending casts. A zero-duration cooldown can leave the button stuck.
    if row.cooldown_ms > 0 {
        let cd = codec::build_spell_cooldown(row.caster_guid, row.spell_id, row.cooldown_ms);
        out.push((
            EventAudience::Everyone,
            message(ServerOpcodeMessage::SMSG_SPELL_COOLDOWN(Box::new(cd))),
        ));
    }
    out
}

/// Codes are lockstep with `CAST_MISS_*` in `module/src/spell/tables.rs`.
fn cast_miss_info(code: u8) -> Option<wow_world_messages::vanilla::SpellMissInfo> {
    use wow_world_messages::vanilla::SpellMissInfo;
    const CAST_MISS_MISS: u8 = 1;
    const CAST_MISS_RESIST: u8 = 2;
    const CAST_MISS_DODGE: u8 = 3;
    const CAST_MISS_PARRY: u8 = 4;
    match code {
        CAST_MISS_MISS => Some(SpellMissInfo::Miss),
        CAST_MISS_RESIST => Some(SpellMissInfo::Resist),
        CAST_MISS_DODGE => Some(SpellMissInfo::Dodge),
        CAST_MISS_PARRY => Some(SpellMissInfo::Parry),
        _ => None,
    }
}

fn periodic_log_outbound(row: &SpellCastEvent) -> Option<Packet> {
    let tick = if row.healed > 0 {
        codec::PeriodicTick::Heal { amount: row.healed }
    } else if row.energized > 0 {
        codec::PeriodicTick::Energize {
            power_type: row.power_type,
            amount: row.energized,
        }
    } else if row.damage > 0 || row.absorbed > 0 || row.resisted > 0 {
        codec::PeriodicTick::Damage {
            damage: row.damage,
            school: row.school,
            absorbed: row.absorbed,
            resisted: row.resisted,
        }
    } else {
        return None;
    };
    Some(Packet {
        opcode: codec::SMSG_PERIODICAURALOG_OPCODE,
        body: codec::build_periodic_aura_log(row.target_guid, row.caster_guid, row.spell_id, tick),
    })
}
