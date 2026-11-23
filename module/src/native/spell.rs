use super::{bounded as message, game_native_player};
use crate::spell::{CastRefusalReason, SpellCastEventKind, StrikeSwingReport};
use crate::{SpellCastEvent, SpellImpactEvent};
use lyracore_protocol::{codec, messages as wow_world_messages, world::Packet};
use spacetimedb::{ReducerContext, Table};
use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
use wow_world_messages::vanilla::{SpellLog, SMSG_SPELLLOGEXECUTE};
use wow_world_messages::Guid;

#[derive(Clone, Copy, PartialEq, Eq)]
enum EventAudience {
    Everyone,
    Source,
    Peers,
}

fn deliver(ctx: &ReducerContext, caster: u64, audience: EventAudience, packet: Packet) {
    match audience {
        EventAudience::Source => {
            super::send_raw_to_character(ctx, caster, packet.opcode, packet.body);
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

/// Who sends a GO row's START, CAST_RESULT and SMSG_SPELL_GO.
#[derive(Clone, Copy)]
pub(crate) enum GoDelivery {
    /// The event itself.
    Pending,
    /// [`cast_launch`], before the cast's effects ran.
    Sent,
}

pub(crate) fn cast_event(ctx: &ReducerContext, event: &SpellCastEvent, delivery: GoDelivery) {
    if ctx.db.game_native_player().count() == 0 {
        return;
    }
    let launch = match delivery {
        GoDelivery::Pending => Some(go_targets(ctx, event, None)),
        GoDelivery::Sent => None,
    };
    for (audience, packet) in cast_event_messages(event, projectile(ctx, event), launch) {
        deliver(ctx, event.caster_guid, audience, packet);
    }
}

/// Sends a resolving cast's START, CAST_RESULT and GO, which name `ground_point` when it has one.
pub(crate) fn cast_launch(
    ctx: &ReducerContext,
    launch: &SpellCastEvent,
    ground_point: Option<(f32, f32, f32)>,
) {
    if ctx.db.game_native_player().count() == 0 {
        return;
    }
    let go_targets = go_targets(ctx, launch, ground_point);
    for (audience, packet) in launch_messages(launch, projectile(ctx, launch), go_targets) {
        deliver(ctx, launch.caster_guid, audience, packet);
    }
}

/// The target block of a GO; vmangos `Spell::SendSpellGo` writes the cast's own targets.
#[derive(Clone, Copy)]
enum GoTargets {
    Unit,
    /// A persistent area hits the ground, so its GO lists no unit as hit.
    Area,
    GroundPoint((f32, f32, f32)),
}

fn go_targets(
    ctx: &ReducerContext,
    event: &SpellCastEvent,
    ground_point: Option<(f32, f32, f32)>,
) -> GoTargets {
    use crate::game_spell_effect;
    if let Some(point) = ground_point {
        return GoTargets::GroundPoint(point);
    }
    let area = event.kind == SpellCastEventKind::Go as u8
        && ctx
            .db
            .game_spell_effect()
            .by_spell()
            .filter(event.spell_id)
            .any(|effect| effect.kind == crate::spell::E_PERSISTENT_AREA);
    if area {
        GoTargets::Area
    } else {
        GoTargets::Unit
    }
}

fn item_creation_log_message(caster: u64, spell: u32, items: &[u32]) -> Packet {
    let log = SMSG_SPELLLOGEXECUTE {
        caster: Guid::new(caster),
        spell,
        logs: items
            .iter()
            .map(|&item| SpellLog::CreateItem { item })
            .collect(),
    };
    message(ServerOpcodeMessage::SMSG_SPELLLOGEXECUTE(Box::new(log)))
}

pub(crate) fn item_creation_log(ctx: &ReducerContext, caster: u64, spell: u32, items: &[u32]) {
    if items.is_empty() || ctx.db.game_native_player().count() == 0 {
        return;
    }
    deliver(
        ctx,
        caster,
        EventAudience::Everyone,
        item_creation_log_message(caster, spell, items),
    );
}

pub(crate) fn spell_cooldown(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    cooldown_ms: u32,
) {
    if let Some(packet) = cooldown_message(caster_guid, spell_id, cooldown_ms) {
        deliver(ctx, caster_guid, EventAudience::Source, packet);
    }
}

fn cooldown_message(caster_guid: u64, spell_id: u32, cooldown_ms: u32) -> Option<Packet> {
    // SMSG_SPELL_GO releases pending casts. A zero-duration cooldown can leave the button stuck.
    (cooldown_ms > 0).then(|| {
        message(ServerOpcodeMessage::SMSG_SPELL_COOLDOWN(Box::new(
            codec::build_spell_cooldown(caster_guid, spell_id, cooldown_ms),
        )))
    })
}

// vmangos sets CAST_FLAG_AMMO on START and GO for every ranged-slot spell.
fn projectile(ctx: &ReducerContext, event: &SpellCastEvent) -> Option<(u32, u32)> {
    use crate::game_spell;
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
            0,
        ),
    )));
    deliver(ctx, event.caster_guid, EventAudience::Everyone, packet);
}

fn damage_log(row: &SpellCastEvent) -> Option<Packet> {
    (row.damage > 0 || row.absorbed > 0 || row.resisted > 0 || row.blocked_amount > 0).then(|| {
        let log = codec::build_spell_non_melee_damage_log(
            row.target_guid,
            row.caster_guid,
            row.spell_id,
            row.damage,
            row.school,
            row.is_crit,
            row.resisted,
            row.absorbed,
            row.blocked_amount,
        );
        message(ServerOpcodeMessage::SMSG_SPELLNONMELEEDAMAGELOG(Box::new(
            log,
        )))
    })
}

fn strike_swing(row: &SpellCastEvent) -> Option<Packet> {
    let reports_damage = match StrikeSwingReport::from_code(row.strike_swing_report) {
        StrikeSwingReport::None => return None,
        StrikeSwingReport::WithDamage => true,
        StrikeSwingReport::WithoutDamage => false,
    };
    Some(codec::build_queued_strike_swing(codec::QueuedStrikeSwing {
        attacker_guid: row.caster_guid,
        target_guid: row.target_guid,
        spell_id: row.spell_id,
        outcome: row.swing_hit_info,
        damage: row.damage,
        school: row.school,
        absorbed: row.absorbed,
        resisted: row.resisted,
        blocked: row.blocked_amount,
        reports_damage,
    }))
}

/// A GO row's packets; `launch` is `None` when [`cast_launch`] already sent its launch packets.
fn cast_event_messages(
    row: &SpellCastEvent,
    ammo: Option<(u32, u32)>,
    launch: Option<GoTargets>,
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
        // vmangos Spell::cancel and Spell::cast: teardown for everyone, then the caster's result.
        out.push((
            EventAudience::Everyone,
            message(ServerOpcodeMessage::SMSG_SPELL_FAILED_OTHER(Box::new(
                codec::build_spell_failed_other(row.caster_guid, row.spell_id),
            ))),
        ));
        // Only a failed SMSG_CAST_RESULT releases the client's in-flight cast and its cast bar.
        if let Some(reason) = CastRefusalReason::from_failure_code(row.failure_reason) {
            out.push((
                EventAudience::Source,
                Packet {
                    opcode: codec::SMSG_CAST_RESULT_OPCODE,
                    body: codec::build_cast_result_failed(
                        row.spell_id,
                        super::cast::cast_result(reason),
                    ),
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
    if let Some(go_targets) = launch {
        out.extend(launch_messages(row, ammo, go_targets));
    }
    // vmangos sends a queued strike's swing after its GO and before its damage log.
    out.extend(strike_swing(row).map(|packet| (EventAudience::Everyone, packet)));
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

    if let Some(packet) = cooldown_message(row.caster_guid, row.spell_id, row.cooldown_ms) {
        out.push((EventAudience::Source, packet));
    }
    out
}

fn launch_messages(
    row: &SpellCastEvent,
    ammo: Option<(u32, u32)>,
    go_targets: GoTargets,
) -> Vec<(EventAudience, Packet)> {
    let mut out = Vec::new();
    // A client-initiated cast's own handler already sent its caster START and CAST_RESULT.
    let start_audience = if row.client_initiated {
        EventAudience::Peers
    } else {
        EventAudience::Everyone
    };
    if !row.is_completion {
        let start = codec::build_spell_start(row.caster_guid, row.spell_id, 0, 0, ammo);
        out.push((
            start_audience,
            message(ServerOpcodeMessage::SMSG_SPELL_START(Box::new(start))),
        ));
    }
    // Timed completions need caster-only CAST_RESULT before GO; instant casts already sent it.
    if row.is_completion && !row.client_initiated {
        out.push((
            EventAudience::Source,
            Packet {
                opcode: codec::SMSG_CAST_RESULT_OPCODE,
                body: codec::build_cast_result_ok(row.spell_id),
            },
        ));
    }
    // vmangos Spell::SendSpellGo: one GO, with its hit and miss lists, to the caster and observers.
    out.push((
        EventAudience::Everyone,
        message(ServerOpcodeMessage::SMSG_SPELL_GO(Box::new(spell_go(
            row, ammo, go_targets,
        )))),
    ));
    out
}

fn spell_go(
    row: &SpellCastEvent,
    ammo: Option<(u32, u32)>,
    go_targets: GoTargets,
) -> wow_world_messages::vanilla::SMSG_SPELL_GO {
    let (caster, spell) = (row.caster_guid, row.spell_id);
    match go_targets {
        GoTargets::GroundPoint(point) => codec::build_spell_go_dest(caster, spell, point),
        GoTargets::Area => codec::build_spell_go_area(caster, spell),
        GoTargets::Unit => {
            let swing_miss = (row.is_completion && row.damage == 0)
                .then(|| swing_miss_info(row.swing_hit_info))
                .flatten();
            let miss = cast_miss_info(row.miss_info).or(swing_miss);
            codec::build_spell_go_outcome(caster, spell, row.target_guid, ammo, miss)
        }
    }
}

/// The SMSG_SPELL_GO miss for an `AttackOutcome::wire` code that deals no damage.
pub(super) fn swing_miss_info(hit_info: u8) -> Option<wow_world_messages::vanilla::SpellMissInfo> {
    use wow_world_messages::vanilla::SpellMissInfo;
    match hit_info {
        2 => Some(SpellMissInfo::Miss),
        3 => Some(SpellMissInfo::Dodge),
        4 => Some(SpellMissInfo::Parry),
        8 => Some(SpellMissInfo::Immune),
        _ => None,
    }
}

fn cast_miss_info(code: u8) -> Option<wow_world_messages::vanilla::SpellMissInfo> {
    use crate::spell::{
        CAST_MISS_DODGE, CAST_MISS_IMMUNE, CAST_MISS_IMMUNE2, CAST_MISS_MISS, CAST_MISS_PARRY,
        CAST_MISS_RESIST,
    };
    use wow_world_messages::vanilla::SpellMissInfo;
    match code {
        CAST_MISS_MISS => Some(SpellMissInfo::Miss),
        CAST_MISS_RESIST => Some(SpellMissInfo::Resist),
        CAST_MISS_DODGE => Some(SpellMissInfo::Dodge),
        CAST_MISS_PARRY => Some(SpellMissInfo::Parry),
        CAST_MISS_IMMUNE => Some(SpellMissInfo::Immune),
        CAST_MISS_IMMUNE2 => Some(SpellMissInfo::Immune2),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn event() -> SpellCastEvent {
        SpellCastEvent {
            id: 0,
            caster_guid: 1,
            spell_id: 12544,
            created_at: spacetimedb::Timestamp::UNIX_EPOCH,
            target_guid: 1,
            cast_time_ms: 0,
            is_completion: false,
            damage: 0,
            school: 0,
            is_crit: false,
            resisted: 0,
            absorbed: 0,
            is_interrupted: false,
            cooldown_ms: 10000,
            delay_ms: 0,
            healed: 0,
            is_proc_log: false,
            swing_hit_info: 0,
            client_initiated: false,
            map_id: 0,
            instance_id: 0,
            grid_x: 0,
            grid_y: 0,
            failure_reason: 0,
            kind: SpellCastEventKind::Go as u8,
            miss_info: 0,
            energized: 0,
            power_type: 0,
            blocked_amount: 0,
            strike_swing_report: 0,
        }
    }

    #[test]
    fn zero_damage_still_logs_absorption_and_resistance() {
        let mut row = event();
        row.kind = SpellCastEventKind::ProcLog as u8;
        assert!(cast_event_messages(&row, None, Some(GoTargets::Unit)).is_empty());
        for (damage, absorbed, resisted) in [(1, 0, 0), (0, 1, 0), (0, 0, 1)] {
            row.damage = damage;
            row.absorbed = absorbed;
            row.resisted = resisted;
            let packets = cast_event_messages(&row, None, Some(GoTargets::Unit));
            assert_eq!(packets.len(), 1);
            assert_eq!(packets[0].1.opcode, 0x0250);
            assert!(matches!(packets[0].0, EventAudience::Everyone));
        }
    }

    #[test]
    fn a_fully_blocked_queued_strike_sends_go_then_its_swing_then_its_damage_log() {
        let mut row = event();
        row.spell_id = 78;
        row.target_guid = 2;
        row.is_completion = true;
        row.cooldown_ms = 0;
        row.blocked_amount = 14;
        row.strike_swing_report = StrikeSwingReport::WithoutDamage as u8;
        let packets = cast_event_messages(&row, None, Some(GoTargets::Unit));
        let opcodes: Vec<_> = packets.iter().map(|(_, packet)| packet.opcode).collect();
        assert_eq!(
            opcodes,
            [codec::SMSG_CAST_RESULT_OPCODE, 0x0132, 0x014a, 0x0250]
        );
        assert!(packets[1..]
            .iter()
            .all(|(audience, _)| matches!(audience, EventAudience::Everyone)));
    }

    #[test]
    fn a_dodged_queued_strike_sends_its_swing_without_a_damage_log() {
        let mut row = event();
        row.spell_id = 78;
        row.target_guid = 2;
        row.is_completion = true;
        row.cooldown_ms = 0;
        row.swing_hit_info = 3;
        row.strike_swing_report = StrikeSwingReport::WithoutDamage as u8;
        let opcodes: Vec<_> = cast_event_messages(&row, None, Some(GoTargets::Unit))
            .iter()
            .map(|(_, packet)| packet.opcode)
            .collect();
        assert_eq!(opcodes, [codec::SMSG_CAST_RESULT_OPCODE, 0x0132, 0x014a]);
    }

    #[test]
    fn cast_cooldowns_go_to_the_caster_while_spell_go_reaches_observers() {
        for (caster, client_initiated) in [(1, false), (1, true), (0xF130000001000001, false)] {
            let mut row = event();
            row.caster_guid = caster;
            row.target_guid = caster;
            row.client_initiated = client_initiated;
            let packets = cast_event_messages(&row, None, Some(GoTargets::Unit));
            let cooldowns: Vec<_> = packets
                .iter()
                .filter(|(_, packet)| packet.opcode == 0x0134)
                .collect();
            assert_eq!(cooldowns.len(), 1);
            assert!(matches!(cooldowns[0].0, EventAudience::Source));
            let go = packets
                .iter()
                .find(|(_, packet)| packet.opcode == 0x0132)
                .unwrap();
            assert!(matches!(go.0, EventAudience::Everyone));
        }
    }

    #[test]
    fn a_launched_cast_sends_only_its_outcome_after_its_effects() {
        let mut row = event();
        row.target_guid = 9;
        row.damage = 40;
        let opcodes: Vec<_> = cast_event_messages(&row, None, None)
            .iter()
            .map(|(_, packet)| packet.opcode)
            .collect();
        assert_eq!(opcodes, [0x0250, 0x0134], "the damage log and the cooldown");
    }

    #[test]
    fn an_avoided_client_cast_reaches_its_caster_as_one_go_with_the_target_missed() {
        let mut row = event();
        row.target_guid = 9;
        row.client_initiated = true;
        row.cooldown_ms = 0;
        for (miss_info, reason) in [
            (crate::spell::CAST_MISS_MISS, 1),
            (crate::spell::CAST_MISS_DODGE, 3),
            (crate::spell::CAST_MISS_PARRY, 4),
            (crate::spell::CAST_MISS_IMMUNE, 7),
        ] {
            row.miss_info = miss_info;
            let packets = cast_event_messages(&row, None, Some(GoTargets::Unit));
            let opcodes: Vec<_> = packets.iter().map(|(_, packet)| packet.opcode).collect();
            assert_eq!(opcodes, [0x0131, 0x0132], "START to peers, then the GO");
            assert!(matches!(packets[0].0, EventAudience::Peers));
            assert!(matches!(packets[1].0, EventAudience::Everyone));
            let go = &packets[1].1.body;
            // Two 2-byte packed guids, spell u32, flags u16, hit count, miss count, then misses.
            assert_eq!(&go[10..12], &[0, 1], "no hit and one miss: {go:?}");
            assert_eq!(&go[12..20], &9u64.to_le_bytes(), "{go:?}");
            assert_eq!(go[20], reason, "{go:?}");
        }
    }

    #[test]
    fn a_created_item_is_logged_as_an_execute_log() {
        let packet = item_creation_log_message(51, 5504, &[5350]);
        assert_eq!(packet.opcode, 0x024C);
        assert_eq!(
            packet.body,
            [
                &[0x01, 0x33][..],
                &5504u32.to_le_bytes(),
                &1u32.to_le_bytes(),
                &24u32.to_le_bytes(),
                &1u32.to_le_bytes(),
                &5350u32.to_le_bytes(),
            ]
            .concat()
        );
    }

    #[test]
    fn a_refused_completion_reports_its_own_gate_result_to_the_caster_only() {
        let mut row = event();
        row.kind = SpellCastEventKind::Interrupt as u8;
        row.is_interrupted = true;
        row.failure_reason = CastRefusalReason::OutOfRange.failure_code();
        let packets = cast_event_messages(&row, None, Some(GoTargets::Unit));
        assert_eq!(packets.len(), 2);
        assert!(matches!(packets[0].0, EventAudience::Everyone));
        assert_ne!(packets[0].1.opcode, codec::SMSG_CAST_RESULT_OPCODE);
        assert!(matches!(packets[1].0, EventAudience::Source));
        assert_eq!(packets[1].1.opcode, codec::SMSG_CAST_RESULT_OPCODE);
        assert_eq!(
            packets[1].1.body,
            codec::build_cast_result_failed(row.spell_id, 0x59)
        );
    }

    #[test]
    fn an_interrupt_without_a_failure_code_sends_no_cast_result() {
        let mut row = event();
        row.kind = SpellCastEventKind::Interrupt as u8;
        row.is_interrupted = true;
        let packets = cast_event_messages(&row, None, Some(GoTargets::Unit));
        assert_eq!(packets.len(), 1);
        assert!(matches!(packets[0].0, EventAudience::Everyone));
    }
}
