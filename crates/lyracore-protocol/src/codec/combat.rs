use super::values::write_packed_guid_u64;
use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
use wow_world_messages::vanilla::{
    AiReaction, DamageInfo, HitInfo, SMSG_SPELL_GO_CastFlags, SMSG_SPELL_GO_CastFlags_Ammo,
    SMSG_SPELL_START_CastFlags, SMSG_SPELL_START_CastFlags_Ammo, SpellCastTargets,
    SpellCastTargets_SpellCastTargetFlags, SpellCastTargets_SpellCastTargetFlags_DestLocation,
    SpellCastTargets_SpellCastTargetFlags_Unit, SpellCooldownStatus, SpellMiss, SpellMissInfo,
    SpellSchool, Vector3d, SMSG_AI_REACTION, SMSG_ATTACKERSTATEUPDATE, SMSG_ATTACKSTART,
    SMSG_ATTACKSTOP, SMSG_SPELLNONMELEEDAMAGELOG, SMSG_SPELL_COOLDOWN, SMSG_SPELL_DELAYED,
    SMSG_SPELL_FAILED_OTHER, SMSG_SPELL_GO, SMSG_SPELL_START,
};
use wow_world_messages::Guid;

/// The client manages the GCD from Spell.dbc; send this only for spell cooldowns.
pub fn build_spell_cooldown(
    caster_guid: u64,
    spell_id: u32,
    cooldown_ms: u32,
) -> SMSG_SPELL_COOLDOWN {
    SMSG_SPELL_COOLDOWN {
        guid: Guid::new(caster_guid),
        cooldowns: vec![SpellCooldownStatus {
            id: spell_id,
            cooldown_time: std::time::Duration::from_millis(cooldown_ms as u64),
        }],
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_spell_non_melee_damage_log(
    target_guid: u64,
    caster_guid: u64,
    spell_id: u32,
    damage: u32,
    school_index: u8,
    is_crit: bool,
    resisted: u32,
    absorbed: u32,
) -> SMSG_SPELLNONMELEEDAMAGELOG {
    SMSG_SPELLNONMELEEDAMAGELOG {
        target: Guid::new(target_guid),
        attacker: Guid::new(caster_guid),
        spell: spell_id,
        damage,
        school: SpellSchool::try_from(school_index).unwrap_or(SpellSchool::Normal),
        absorbed_damage: absorbed,
        resisted,
        periodic_log: false,
        unused: 0,
        blocked: 0,
        // Spell crits use 0x2; the binding's melee CriticalHit bit, 0x80, is wrong here.
        hit_info: if is_crit {
            HitInfo::AffectsVictim
        } else {
            HitInfo::NormalSwing
        },
        extend_flag: 0,
    }
}

/// START must precede GO, including instant casts, to release the client's cast state.
pub fn build_spell_start(
    caster_guid: u64,
    spell_id: u32,
    timer: u32,
    target: u64,
    ammo: Option<(u32, u32)>,
) -> SMSG_SPELL_START {
    // CAST_FLAG_UNKNOWN2 is the base START flag; ranged casts also carry AMMO.
    let flags = match ammo {
        Some((ammo_display_id, ammo_inventory_type)) => SMSG_SPELL_START_CastFlags::new(
            0x0002 | 0x0020,
            Some(SMSG_SPELL_START_CastFlags_Ammo {
                ammo_display_id,
                ammo_inventory_type,
            }),
        ),
        None => SMSG_SPELL_START_CastFlags::new_unknown2(),
    };
    let targets = if target == 0 || target == caster_guid {
        SpellCastTargets::default()
    } else {
        SpellCastTargets {
            target_flags: SpellCastTargets_SpellCastTargetFlags::new_unit(
                SpellCastTargets_SpellCastTargetFlags_Unit {
                    unit_target: Guid::new(target),
                },
            ),
        }
    };
    SMSG_SPELL_START {
        cast_item: Guid::new(caster_guid),
        caster: Guid::new(caster_guid),
        spell: spell_id,
        flags,
        timer,
        targets,
    }
}

pub fn build_spell_delayed(caster_guid: u64, delay_ms: u32) -> SMSG_SPELL_DELAYED {
    SMSG_SPELL_DELAYED {
        guid: Guid::new(caster_guid),
        delay_time: delay_ms,
    }
}

/// Send OK after COOLDOWN and before GO to release the client's current spell.
pub fn build_cast_result_failed(spell_id: u32, reason: u8) -> Vec<u8> {
    let mut v = spell_id.to_le_bytes().to_vec();
    v.push(0x02);
    v.push(reason);
    v
}

/// 1.12.1 SpellCastResult values that a refused cast reports to its caster.
pub mod cast_failure {
    pub const AFFECTING_COMBAT: u8 = 0x00;
    pub const BAD_TARGETS: u8 = 0x0A;
    pub const CASTER_AURASTATE: u8 = 0x12;
    pub const CASTER_DEAD: u8 = 0x13;
    pub const DONT_REPORT: u8 = 0x17;
    pub const INTERRUPTED: u8 = 0x23;
    pub const NO_AMMO: u8 = 0x24;
    pub const ITEM_GONE: u8 = 0x26;
    pub const LEVEL_REQUIREMENT: u8 = 0x29;
    pub const LOW_LEVEL: u8 = 0x2b;
    pub const LINE_OF_SIGHT: u8 = 0x2A;
    pub const NOT_BEHIND: u8 = 0x33;
    pub const NOT_READY: u8 = 0x3C;
    pub const NO_COMBO_POINTS: u8 = 0x46;
    pub const NO_POWER: u8 = 0x4D;
    pub const ONLY_ABOVEWATER: u8 = 0x50;
    pub const ONLY_OUTDOORS: u8 = 0x55;
    pub const ONLY_SHAPESHIFT: u8 = 0x56;
    pub const ONLY_STEALTHED: u8 = 0x57;
    pub const OUT_OF_RANGE: u8 = 0x59;
    pub const STUNNED: u8 = 0x64;
    pub const TARGETS_DEAD: u8 = 0x65;
    pub const TOO_CLOSE: u8 = 0x76;
    pub const UNIT_NOT_INFRONT: u8 = 0x7C;
}

pub fn cast_failure_reason_for_code(code: u8) -> Option<u8> {
    const CAST_FAIL_NO_POWER: u8 = 1;
    const CAST_FAIL_INTERRUPTED: u8 = 2;
    match code {
        CAST_FAIL_NO_POWER => Some(cast_failure::NO_POWER),
        CAST_FAIL_INTERRUPTED => Some(cast_failure::INTERRUPTED),
        _ => None,
    }
}

pub fn build_cast_result_ok(spell_id: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity(5);
    v.extend_from_slice(&spell_id.to_le_bytes());
    v.push(0x00); // SPELL_RESULT_STATUS_OKAY
    v
}

pub fn build_spell_go(
    caster_guid: u64,
    spell_id: u32,
    hit_target: u64,
    ammo: Option<(u32, u32)>,
) -> SMSG_SPELL_GO {
    build_spell_go_outcome(caster_guid, spell_id, hit_target, ammo, false)
}

pub fn build_spell_go_area(caster_guid: u64, spell_id: u32) -> SMSG_SPELL_GO {
    SMSG_SPELL_GO {
        cast_item: Guid::new(caster_guid),
        caster: Guid::new(caster_guid),
        spell: spell_id,
        flags: SMSG_SPELL_GO_CastFlags::new_unknown9(),
        hits: vec![],
        misses: vec![],
        targets: SpellCastTargets::default(),
    }
}

fn ground_point_targets(dest: (f32, f32, f32)) -> SpellCastTargets {
    SpellCastTargets {
        target_flags: SpellCastTargets_SpellCastTargetFlags::new_dest_location(
            SpellCastTargets_SpellCastTargetFlags_DestLocation {
                destination: Vector3d {
                    x: dest.0,
                    y: dest.1,
                    z: dest.2,
                },
            },
        ),
    }
}

pub fn build_spell_start_dest(
    caster_guid: u64,
    spell_id: u32,
    dest: (f32, f32, f32),
) -> SMSG_SPELL_START {
    SMSG_SPELL_START {
        targets: ground_point_targets(dest),
        ..build_spell_start(caster_guid, spell_id, 0, 0, None)
    }
}

pub fn build_spell_go_dest(
    caster_guid: u64,
    spell_id: u32,
    dest: (f32, f32, f32),
) -> SMSG_SPELL_GO {
    SMSG_SPELL_GO {
        targets: ground_point_targets(dest),
        ..build_spell_go_area(caster_guid, spell_id)
    }
}

pub fn build_spell_go_outcome(
    caster_guid: u64,
    spell_id: u32,
    hit_target: u64,
    ammo: Option<(u32, u32)>,
    miss: bool,
) -> SMSG_SPELL_GO {
    // Self-casts use TARGET_FLAG_SELF even when the target guid equals the caster.
    let is_self_cast = hit_target == 0 || hit_target == caster_guid;
    let hit = if is_self_cast {
        caster_guid
    } else {
        hit_target
    };
    // CAST_FLAG_UNKNOWN9 is the base GO flag; ranged casts also carry AMMO.
    let flags = match ammo {
        Some((ammo_display_id, ammo_inventory_type)) => SMSG_SPELL_GO_CastFlags::new(
            0x0100 | 0x0020,
            Some(SMSG_SPELL_GO_CastFlags_Ammo {
                ammo_display_id,
                ammo_inventory_type,
            }),
        ),
        None => SMSG_SPELL_GO_CastFlags::new_unknown9(),
    };
    let targets = if is_self_cast {
        SpellCastTargets::default()
    } else {
        SpellCastTargets {
            target_flags: SpellCastTargets_SpellCastTargetFlags::new_unit(
                SpellCastTargets_SpellCastTargetFlags_Unit {
                    unit_target: Guid::new(hit_target),
                },
            ),
        }
    };
    let (hits, misses) = if miss {
        (
            vec![],
            vec![SpellMiss {
                target: Guid::new(hit),
                miss_info: SpellMissInfo::Miss,
            }],
        )
    } else {
        (vec![Guid::new(hit)], vec![])
    };
    SMSG_SPELL_GO {
        cast_item: Guid::new(caster_guid),
        caster: Guid::new(caster_guid),
        spell: spell_id,
        flags,
        hits,
        misses,
        targets,
    }
}

pub const SMSG_PERIODICAURALOG_OPCODE: u16 = 0x024E;
pub const SMSG_SPELLENERGIZELOG_OPCODE: u16 = 0x0151;
pub const SMSG_SPELLLOGMISS_OPCODE: u16 = 0x024B;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeriodicTick {
    Damage {
        damage: u32,
        school: u8,
        absorbed: u32,
        resisted: u32,
    },
    Heal {
        amount: u32,
    },
    Energize {
        power_type: u8,
        amount: u32,
    },
}

/// gtker writes the damage school as a u8; build 5875 reads a u32.
pub fn build_periodic_aura_log(
    target_guid: u64,
    caster_guid: u64,
    spell_id: u32,
    tick: PeriodicTick,
) -> Vec<u8> {
    const SPELL_AURA_PERIODIC_DAMAGE: u32 = 3;
    const SPELL_AURA_PERIODIC_HEAL: u32 = 8;
    const SPELL_AURA_PERIODIC_ENERGIZE: u32 = 24;
    let mut body = Vec::with_capacity(40);
    write_packed_guid_u64(&mut body, target_guid);
    write_packed_guid_u64(&mut body, caster_guid);
    body.extend_from_slice(&spell_id.to_le_bytes());
    body.extend_from_slice(&1u32.to_le_bytes());
    let fields: Vec<u32> = match tick {
        PeriodicTick::Damage {
            damage,
            school,
            absorbed,
            resisted,
        } => vec![
            SPELL_AURA_PERIODIC_DAMAGE,
            damage,
            u32::from(school),
            absorbed,
            resisted,
        ],
        PeriodicTick::Heal { amount } => vec![SPELL_AURA_PERIODIC_HEAL, amount],
        PeriodicTick::Energize { power_type, amount } => {
            vec![SPELL_AURA_PERIODIC_ENERGIZE, u32::from(power_type), amount]
        }
    };
    for field in fields {
        body.extend_from_slice(&field.to_le_bytes());
    }
    body
}

/// Packed target, packed caster, spell, power type as u32, amount.
pub fn build_spell_energize_log(
    target_guid: u64,
    caster_guid: u64,
    spell_id: u32,
    power_type: u8,
    amount: u32,
) -> Vec<u8> {
    let mut body = Vec::with_capacity(30);
    write_packed_guid_u64(&mut body, target_guid);
    write_packed_guid_u64(&mut body, caster_guid);
    body.extend_from_slice(&spell_id.to_le_bytes());
    body.extend_from_slice(&u32::from(power_type).to_le_bytes());
    body.extend_from_slice(&amount.to_le_bytes());
    body
}

/// Spell, full caster guid, extended-info flag 0, count 1, full target guid, `SpellMissInfo` byte.
pub fn build_spell_log_miss(
    caster_guid: u64,
    spell_id: u32,
    target_guid: u64,
    miss_info: u8,
) -> Vec<u8> {
    let mut body = Vec::with_capacity(26);
    body.extend_from_slice(&spell_id.to_le_bytes());
    body.extend_from_slice(&caster_guid.to_le_bytes());
    body.push(0);
    body.extend_from_slice(&1u32.to_le_bytes());
    body.extend_from_slice(&target_guid.to_le_bytes());
    body.push(miss_info);
    body
}

pub fn build_spell_failed_other(caster_guid: u64, spell_id: u32) -> SMSG_SPELL_FAILED_OTHER {
    SMSG_SPELL_FAILED_OTHER {
        caster: Guid::new(caster_guid),
        id: spell_id,
    }
}

pub fn build_ai_reaction_hostile(creature_guid: u64) -> SMSG_AI_REACTION {
    SMSG_AI_REACTION {
        guid: Guid::new(creature_guid),
        reaction: AiReaction::Hostile,
    }
}

pub fn build_attack_start(attacker_guid: u64, victim_guid: u64) -> SMSG_ATTACKSTART {
    SMSG_ATTACKSTART {
        attacker: Guid::new(attacker_guid),
        victim: Guid::new(victim_guid),
    }
}

pub fn build_attack_stop(attacker_guid: u64, victim_guid: u64) -> SMSG_ATTACKSTOP {
    SMSG_ATTACKSTOP {
        player: Guid::new(attacker_guid),
        enemy: Guid::new(victim_guid),
        unknown1: 0,
    }
}

pub fn build_attacker_state_update(
    attacker_guid: u64,
    target_guid: u64,
    damage: u32,
    outcome: u8,
    blocked_amount: u32,
    spell_id: u32,
) -> crate::world::Packet {
    let message = attacker_state_update(
        attacker_guid,
        target_guid,
        damage,
        outcome,
        blocked_amount,
        spell_id,
    );
    let mut flags = message.hit_info.as_int();
    if outcome == 7 || blocked_amount > 0 {
        flags |= 0x800;
    }
    if matches!(outcome, 3 | 4) || damage > 0 || blocked_amount > 0 {
        flags |= 0x2;
    }
    let mut packet = crate::world::encode(ServerOpcodeMessage::SMSG_ATTACKERSTATEUPDATE(Box::new(
        message,
    )))
    .expect("one melee damage entry fits a world frame");
    // The binding's HitInfo enum cannot represent combined flags or HITINFO_BLOCK (0x800).
    packet.body[..4].copy_from_slice(&flags.to_le_bytes());
    packet
}

fn attacker_state_update(
    attacker_guid: u64,
    target_guid: u64,
    damage: u32,
    hit_info: u8,
    blocked_amount: u32,
    spell_id: u32,
) -> SMSG_ATTACKERSTATEUPDATE {
    let (hit_info, damage_state) = match hit_info {
        1 => (HitInfo::CriticalHit, u32::from(VICTIMSTATE_NORMAL)),
        2 => (HitInfo::Miss, u32::from(VICTIMSTATE_UNAFFECTED)),
        3 => (HitInfo::NormalSwing, u32::from(VICTIMSTATE_DODGE)),
        4 => (HitInfo::NormalSwing, u32::from(VICTIMSTATE_PARRY)),
        5 => (HitInfo::Glancing, u32::from(VICTIMSTATE_NORMAL)),
        6 => (HitInfo::Crushing, u32::from(VICTIMSTATE_NORMAL)),
        7 => (
            HitInfo::AffectsVictim,
            u32::from(if damage == 0 {
                VICTIMSTATE_BLOCK
            } else {
                VICTIMSTATE_NORMAL
            }),
        ),
        _ => (HitInfo::AffectsVictim, u32::from(VICTIMSTATE_NORMAL)),
    };
    SMSG_ATTACKERSTATEUPDATE {
        hit_info,
        attacker: Guid::new(attacker_guid),
        target: Guid::new(target_guid),
        total_damage: damage,
        damages: vec![DamageInfo {
            spell_school_mask: 0,
            damage_float: damage as f32,
            damage_uint: damage,
            absorb: 0,
            resist: 0,
        }],
        damage_state,
        unknown1: 0,
        spell_id,
        blocked_amount,
    }
}

const VICTIMSTATE_UNAFFECTED: u8 = 0;
const VICTIMSTATE_NORMAL: u8 = 1;
const VICTIMSTATE_DODGE: u8 = 2;
const VICTIMSTATE_PARRY: u8 = 3;
const VICTIMSTATE_BLOCK: u8 = 5;

#[cfg(test)]
mod tests {
    use super::*;
    use wow_world_messages::vanilla::{HitInfo, SpellSchool};

    #[test]
    fn spell_start_carries_cast_bar_timer() {
        assert_eq!(build_spell_start(0xF1, 133, 3500, 0, None).timer, 3500);
        assert_eq!(build_spell_start(0xF1, 133, 0, 0, None).timer, 0);
        let s = build_spell_start(0xF1, 75, 0, 0xBEEF, Some((5996, 24)));
        assert_eq!(
            s.flags
                .get_ammo()
                .map(|a| (a.ammo_display_id, a.ammo_inventory_type)),
            Some((5996, 24))
        );
        assert!(s.targets.target_flags.get_unit().is_some());
        let go = build_spell_go_outcome(0xF1, 75, 0xBEEF, Some((5996, 24)), true);
        assert!(go.hits.is_empty());
        assert_eq!(go.misses.len(), 1);
    }

    #[test]
    fn spell_go_aims_at_the_target_not_self() {
        let caster = 0xF130_0000_0000_0001u64;
        let target = 0xF130_0000_0000_00AAu64;
        assert_eq!(
            build_spell_go(caster, 133, target, None).hits,
            vec![Guid::new(target)]
        );
        assert_eq!(
            build_spell_go(caster, 133, 0, None).hits,
            vec![Guid::new(caster)]
        );
    }

    #[test]
    fn spell_go_targets_carry_unit_for_the_missile_trajectory() {
        // Missile animation requires TARGET_FLAG_UNIT; hits[] alone is insufficient.
        let caster = 0xF130_0000_0000_0001u64;
        let target = 0xF130_0000_0000_00AAu64;
        let go = build_spell_go(caster, 686, target, None);
        let unit = go.targets.target_flags.get_unit().expect("UNIT flag set");
        assert_eq!(unit.unit_target, Guid::new(target));
        let self_go = build_spell_go(caster, 686, 0, None);
        assert!(self_go.targets.target_flags.get_unit().is_none());
    }

    #[test]
    fn spell_go_at_a_ground_point_lists_no_hits_and_writes_the_destination_block() {
        use wow_world_messages::Message;
        let mut bytes = Vec::new();
        build_spell_go_dest(5, 1725, (1.5, -2.0, 30.25))
            .write_into_vec(&mut bytes)
            .unwrap();
        let mut want = vec![0x01, 0x05, 0x01, 0x05];
        want.extend_from_slice(&1725u32.to_le_bytes());
        want.extend_from_slice(&0x0100u16.to_le_bytes());
        want.extend_from_slice(&[0, 0]);
        want.extend_from_slice(&0x0040u16.to_le_bytes());
        for coordinate in [1.5f32, -2.0, 30.25] {
            want.extend_from_slice(&coordinate.to_le_bytes());
        }
        assert_eq!(bytes, want);
    }

    #[test]
    fn spell_delayed_carries_the_caster_guid_and_delay_ms() {
        let caster = 0xF130_0000_0000_0007u64;
        let d = build_spell_delayed(caster, 500);
        assert_eq!(d.guid, Guid::new(caster));
        assert_eq!(d.delay_time, 500);
    }

    #[test]
    fn cast_result_ok_body_is_spell_id_plus_status_byte() {
        let body = build_cast_result_ok(686);
        assert_eq!(body.len(), 5, "spell_id(u32 LE) + status(u8)");
        assert_eq!(u32::from_le_bytes(body[0..4].try_into().unwrap()), 686);
        assert_eq!(body[4], 0x00, "SPELL_RESULT_STATUS_OKAY");
    }

    #[test]
    fn spell_cooldown_carries_the_spell_and_duration() {
        let msg = build_spell_cooldown(0xF1, 6673, 6000);
        assert_eq!(msg.guid.guid(), 0xF1);
        assert_eq!(
            msg.cooldowns.len(),
            1,
            "only the just-cast spell gets a cooldown entry"
        );
        assert_eq!(msg.cooldowns[0].id, 6673);
        assert_eq!(
            msg.cooldowns[0].cooldown_time,
            std::time::Duration::from_millis(6000)
        );
    }

    #[test]
    fn attack_start_carries_attacker_and_victim() {
        let player = 1u64;
        let chicken = lyracore_shared::guid::creature(620, 1);
        let msg = build_attack_start(player, chicken);
        assert_eq!(msg.attacker.guid(), player);
        assert_eq!(msg.victim.guid(), chicken);
    }

    fn melee_packet(packet: crate::world::Packet) -> benilla_protocol::messages::AttackerState {
        let (decoded, tail) =
            benilla_protocol::messages::parse_server_with_tail(packet.opcode, &packet.body)
                .unwrap();
        assert_eq!(tail, 0);
        let benilla_protocol::messages::ServerPacket::AttackerState(swing) = decoded else {
            panic!("expected SMSG_ATTACKERSTATEUPDATE");
        };
        swing
    }

    #[test]
    fn attacker_state_update_is_one_physical_normal_swing() {
        let character = 1u64;
        let chicken = lyracore_shared::guid::creature(620, 1);
        let swing = melee_packet(build_attacker_state_update(character, chicken, 7, 0, 0, 0));
        assert_eq!(swing.attacker, character);
        assert_eq!(swing.victim, chicken);
        assert_eq!(swing.hit_info, 0x2);
        assert_eq!(swing.victim_state, 1);
        assert_eq!(swing.damage, 7);
        assert_eq!(swing.school, 0);
        assert_eq!(swing.absorb, 0);
        assert_eq!(swing.resist, 0);
        assert_eq!(swing.blocked, 0);
    }

    #[test]
    fn attacker_state_update_maps_hit_info_discriminant() {
        for (outcome, damage, blocked, flags, state) in [
            (0, 7, 0, 0x2, 1),
            (1, 14, 0, 0x82, 1),
            (2, 0, 0, 0x10, 0),
            (3, 0, 0, 0x2, 2),
            (4, 0, 0, 0x2, 3),
            (5, 4, 0, 0x4002, 1),
            (6, 15, 0, 0x8002, 1),
            (7, 5, 25, 0x802, 1),
            (7, 0, 25, 0x802, 5),
            (7, 5, 0, 0x802, 1),
        ] {
            let packet = build_attacker_state_update(1, 2, damage, outcome, blocked, 0);
            assert_eq!(packet.opcode, 0x14a);
            assert_eq!(
                u32::from_le_bytes(packet.body[..4].try_into().unwrap()),
                flags
            );
            let state_offset = packet.body.len() - 16;
            assert_eq!(
                u32::from_le_bytes(
                    packet.body[state_offset..state_offset + 4]
                        .try_into()
                        .unwrap()
                ),
                state
            );
            let swing = melee_packet(packet);
            assert_eq!(swing.damage, damage);
            assert_eq!(swing.hit_info, flags);
            assert_eq!(swing.victim_state, state);
            assert_eq!(swing.blocked, blocked);
        }
        let ranged = melee_packet(build_attacker_state_update(1, 2, 9, 0, 0, 75));
        assert_eq!(ranged.melee_spell_id, 75);
        assert_eq!(ranged.damage, 9);
    }

    #[test]
    fn cast_result_failed_body_carries_the_spell_status_and_reason() {
        assert_eq!(
            build_cast_result_failed(100, 0x4D),
            vec![100, 0, 0, 0, 0x02, 0x4D]
        );
    }

    #[test]
    fn spell_damage_log_renders_crit_and_breakdown() {
        let crit = build_spell_non_melee_damage_log(0x42, 0x7, 133, 30, 4, true, 5, 3);
        assert_eq!(crit.hit_info, HitInfo::AffectsVictim);
        assert_eq!(crit.hit_info.as_int(), 0x2);
        assert_eq!(crit.damage, 30);
        assert_eq!(crit.resisted, 5);
        assert_eq!(crit.absorbed_damage, 3);
        assert_eq!(crit.blocked, 0);
        assert_eq!(crit.school, SpellSchool::Frost);
        let normal = build_spell_non_melee_damage_log(0x42, 0x7, 133, 20, 0, false, 0, 0);
        assert_eq!(normal.hit_info, HitInfo::NormalSwing);
        assert_eq!(normal.resisted, 0);
        assert_eq!(normal.absorbed_damage, 0);
        assert_eq!(normal.school, SpellSchool::Normal);
    }
}

pub fn build_spell_modifier_msgs(rows: &[(u32, u8, i32, bool)]) -> Vec<ServerOpcodeMessage> {
    use std::collections::BTreeMap;
    use wow_world_messages::vanilla::{SMSG_SET_FLAT_SPELL_MODIFIER, SMSG_SET_PCT_SPELL_MODIFIER};
    let mut totals: BTreeMap<(bool, u8, u8), i64> = BTreeMap::new();
    for &(mask, op, amount, pct) in rows {
        for bit in 0..32u8 {
            if mask & (1 << bit) != 0 {
                *totals.entry((pct, op, bit)).or_default() += amount as i64;
            }
        }
    }
    totals
        .into_iter()
        .map(|((pct, op, bit), value)| {
            // Signed totals use their two's-complement wire representation.
            let value = value as i32 as u32;
            if pct {
                ServerOpcodeMessage::SMSG_SET_PCT_SPELL_MODIFIER(SMSG_SET_PCT_SPELL_MODIFIER {
                    eff: bit,
                    op,
                    value,
                })
            } else {
                ServerOpcodeMessage::SMSG_SET_FLAT_SPELL_MODIFIER(SMSG_SET_FLAT_SPELL_MODIFIER {
                    eff: bit,
                    op,
                    value,
                })
            }
        })
        .collect()
}

#[cfg(test)]
mod spellmod_tests {
    use super::*;

    #[test]
    fn spell_modifier_msgs_aggregate_per_op_and_bit() {
        let rows = vec![
            (0b1u32, 10u8, -100i32, false),
            (0b1, 10, -100, false),
            (0b11, 0, 5, true),
        ];
        let msgs = build_spell_modifier_msgs(&rows);
        assert_eq!(msgs.len(), 3);
        match &msgs[0] {
            ServerOpcodeMessage::SMSG_SET_FLAT_SPELL_MODIFIER(m) => {
                assert_eq!((m.eff, m.op, m.value as i32), (0, 10, -200));
            }
            other => panic!("expected FLAT first, got {other:?}"),
        }
    }
}
