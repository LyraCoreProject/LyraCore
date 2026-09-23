//! Member Stats wire mapping: `SMSG_PARTY_MEMBER_STATS` and `SMSG_PARTY_MEMBER_STATS_FULL`.
//!
//! Both packets are hand-rolled. gtker 0.3 reads the zone as a `u32` where cmangos and vmangos
//! write a `u16` (cm:GroupHandler.cpp:626-627, vm:GroupHandler.cpp:630-631). It also writes the
//! negative-aura mask as a `u32` where the client reads a `u16`, and its FULL struct has no
//! negative-aura field.

use super::values::write_packed_guid_u64;
use lyracore_shared::constants::player_flags;
use std::ops::{BitOr, BitOrAssign};

/// `SMSG_PARTY_MEMBER_STATS`: the fields that changed, pushed by the Member Stats Relay.
pub const SMSG_PARTY_MEMBER_STATS_OPCODE: u16 = 0x007E;
/// `SMSG_PARTY_MEMBER_STATS_FULL`: the answer to `CMSG_REQUEST_PARTY_MEMBER_STATS`.
pub const SMSG_PARTY_MEMBER_STATS_FULL_OPCODE: u16 = 0x02F2;

/// The member status byte (cm:Group.h:45-56). Only the states the Module models are named. PvP,
/// AFK and DND have no entity field yet, and a member in Transfer gets no packet, so `ZONE_OUT` is
/// never needed.
pub mod member_status {
    pub const OFFLINE: u8 = 0x00;
    pub const ONLINE: u8 = 0x01;
    pub const DEAD: u8 = 0x04;
    pub const GHOST: u8 = 0x08;
}

/// Which fields a Member Stats body carries, as `GROUP_UPDATE_FLAG_*` bits (cm:Group.h:66-77).
///
/// The bits are private, so a mask can name only the fields this encoder writes. The aura and pet
/// bits arrive together with their fields; until then no mask can claim a field the body lacks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GroupUpdateMask(u32);

impl GroupUpdateMask {
    pub const NONE: Self = Self(0);
    pub const STATUS: Self = Self(0x0000_0001);
    pub const CUR_HP: Self = Self(0x0000_0002);
    pub const MAX_HP: Self = Self(0x0000_0004);
    pub const POWER_TYPE: Self = Self(0x0000_0008);
    pub const CUR_POWER: Self = Self(0x0000_0010);
    pub const MAX_POWER: Self = Self(0x0000_0020);
    pub const LEVEL: Self = Self(0x0000_0040);
    pub const ZONE: Self = Self(0x0000_0080);
    pub const POSITION: Self = Self(0x0000_0100);
    /// Every field a Member Stats packet carries today.
    pub const MEMBER: Self = Self(0x0000_01FF);

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn contains(self, fields: Self) -> bool {
        self.0 & fields.0 == fields.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl BitOr for GroupUpdateMask {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for GroupUpdateMask {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// The `game_world_entity` columns Member Stats read, before they narrow to wire widths.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MemberEntity {
    pub health: u32,
    pub max_health: u32,
    pub power: u32,
    pub max_power: u32,
    /// Byte 3 is the power type.
    pub unit_bytes_0: u32,
    pub level: u32,
    pub zone_id: u32,
    pub x: f32,
    pub y: f32,
    pub dead: bool,
    pub player_flags: u32,
}

/// One group member's Member Stats in wire values. The relay compares these, so a value that
/// narrows to the same wire value is not a change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemberStats {
    pub status: u8,
    pub current_health: u16,
    pub max_health: u16,
    pub power_type: u8,
    pub current_power: u16,
    pub max_power: u16,
    pub level: u16,
    pub zone: u16,
    pub position_x: u16,
    pub position_y: u16,
}

impl MemberStats {
    /// Project a live entity. Health, power, level and zone saturate at `u16::MAX`. A coordinate
    /// keeps the bits of cmangos's `uint16(float)`, which is `x as i32 as u16` for a map coordinate.
    pub fn from_entity(entity: &MemberEntity) -> Self {
        let narrow = |value: u32| u16::try_from(value).unwrap_or(u16::MAX);
        let mut status = member_status::ONLINE;
        if entity.dead {
            status |= member_status::DEAD;
        }
        if entity.player_flags & player_flags::GHOST != 0 {
            status |= member_status::GHOST;
        }
        Self {
            status,
            current_health: narrow(entity.health),
            max_health: narrow(entity.max_health),
            power_type: (entity.unit_bytes_0 >> 24) as u8,
            current_power: narrow(entity.power),
            max_power: narrow(entity.max_power),
            level: narrow(entity.level),
            zone: narrow(entity.zone_id),
            position_x: entity.x as i32 as u16,
            position_y: entity.y as i32 as u16,
        }
    }
}

/// The fields that differ between what a viewer last received and `current`. `None` means the
/// viewer holds nothing, so every field goes. A power-type change also resends both power values,
/// because the client reads them in the new type (cm:GroupHandler.cpp:589-593).
pub fn stats_delta(previous: Option<&MemberStats>, current: &MemberStats) -> GroupUpdateMask {
    let Some(previous) = previous else {
        return GroupUpdateMask::MEMBER;
    };
    let mut mask = GroupUpdateMask::NONE;
    if previous.status != current.status {
        mask |= GroupUpdateMask::STATUS;
    }
    if previous.current_health != current.current_health {
        mask |= GroupUpdateMask::CUR_HP;
    }
    if previous.max_health != current.max_health {
        mask |= GroupUpdateMask::MAX_HP;
    }
    if previous.power_type != current.power_type {
        mask |=
            GroupUpdateMask::POWER_TYPE | GroupUpdateMask::CUR_POWER | GroupUpdateMask::MAX_POWER;
    }
    if previous.current_power != current.current_power {
        mask |= GroupUpdateMask::CUR_POWER;
    }
    if previous.max_power != current.max_power {
        mask |= GroupUpdateMask::MAX_POWER;
    }
    if previous.level != current.level {
        mask |= GroupUpdateMask::LEVEL;
    }
    if previous.zone != current.zone {
        mask |= GroupUpdateMask::ZONE;
    }
    if (previous.position_x, previous.position_y) != (current.position_x, current.position_y) {
        mask |= GroupUpdateMask::POSITION;
    }
    mask
}

/// Which of the two packets carries a Member Stats body. Both share one body grammar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberStatsPacket {
    Changed,
    Full,
}

impl MemberStatsPacket {
    const fn opcode(self) -> u16 {
        match self {
            Self::Changed => SMSG_PARTY_MEMBER_STATS_OPCODE,
            Self::Full => SMSG_PARTY_MEMBER_STATS_FULL_OPCODE,
        }
    }
}

/// Build one Member Stats packet: packed guid, `u32` mask, then each masked field in bit order
/// with the widths cmangos writes (cm:Group.h:66-77, cm:GroupHandler.cpp:585-630).
/// Returns `(opcode, body)` for [`Outbound::Raw`](crate::world::Outbound::Raw).
pub fn build_member_stats(
    packet: MemberStatsPacket,
    guid: u64,
    mask: GroupUpdateMask,
    stats: &MemberStats,
) -> (u16, Vec<u8>) {
    let mut body = Vec::with_capacity(9 + 4 + 18);
    write_packed_guid_u64(&mut body, guid);
    body.extend_from_slice(&mask.bits().to_le_bytes());
    let fields: [(GroupUpdateMask, &[u8]); 10] = [
        (GroupUpdateMask::STATUS, &[stats.status]),
        (GroupUpdateMask::CUR_HP, &stats.current_health.to_le_bytes()),
        (GroupUpdateMask::MAX_HP, &stats.max_health.to_le_bytes()),
        (GroupUpdateMask::POWER_TYPE, &[stats.power_type]),
        (
            GroupUpdateMask::CUR_POWER,
            &stats.current_power.to_le_bytes(),
        ),
        (GroupUpdateMask::MAX_POWER, &stats.max_power.to_le_bytes()),
        (GroupUpdateMask::LEVEL, &stats.level.to_le_bytes()),
        (GroupUpdateMask::ZONE, &stats.zone.to_le_bytes()),
        (GroupUpdateMask::POSITION, &stats.position_x.to_le_bytes()),
        (GroupUpdateMask::POSITION, &stats.position_y.to_le_bytes()),
    ];
    for (field, bytes) in fields {
        if mask.contains(field) {
            body.extend_from_slice(bytes);
        }
    }
    (packet.opcode(), body)
}

/// The offline answer: mask `STATUS` and status 0 (cm:GroupHandler.cpp:764-771).
pub fn build_member_offline(packet: MemberStatsPacket, guid: u64) -> (u16, Vec<u8>) {
    let offline = MemberStats {
        status: member_status::OFFLINE,
        ..MemberStats::default()
    };
    build_member_stats(packet, guid, GroupUpdateMask::STATUS, &offline)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
    use wow_world_messages::vanilla::Power;

    /// A player-shaped guid whose packed form is `03 02 01`.
    const GUID: u64 = 0x0102;

    /// Wire values for a level 20 caster standing in Elwynn Forest, near Northshire.
    fn caster() -> MemberStats {
        MemberStats {
            status: member_status::ONLINE,
            current_health: 1234,
            max_health: 1500,
            power_type: 0,
            current_power: 800,
            max_power: 1000,
            level: 20,
            zone: 12,
            position_x: 0xDD0B,
            position_y: 0xFF7C,
        }
    }

    /// Frame a body the way `Outbound::Raw` does, then decode it with gtker.
    fn gtker_decode(opcode: u16, body: &[u8]) -> ServerOpcodeMessage {
        let mut framed = ((2 + body.len()) as u16).to_be_bytes().to_vec();
        framed.extend_from_slice(&opcode.to_le_bytes());
        framed.extend_from_slice(body);
        ServerOpcodeMessage::read_unencrypted(&mut framed.as_slice()).unwrap()
    }

    #[test]
    fn opcodes_match_the_vanilla_client() {
        use wow_world_messages::vanilla::{SMSG_PARTY_MEMBER_STATS, SMSG_PARTY_MEMBER_STATS_FULL};
        use wow_world_messages::Message;
        assert_eq!(
            u32::from(SMSG_PARTY_MEMBER_STATS_OPCODE),
            SMSG_PARTY_MEMBER_STATS::OPCODE
        );
        assert_eq!(
            u32::from(SMSG_PARTY_MEMBER_STATS_FULL_OPCODE),
            SMSG_PARTY_MEMBER_STATS_FULL::OPCODE
        );
    }

    #[test]
    fn field_bits_are_the_cmangos_group_update_flags() {
        use wow_world_messages::vanilla::GroupUpdateFlags as Gtker;
        // cm:Group.h:68-77 typed from the header, then gtker's constants as a second source.
        let pinned = [
            (GroupUpdateMask::STATUS, 0x0000_0001, Gtker::STATUS),
            (GroupUpdateMask::CUR_HP, 0x0000_0002, Gtker::CUR_HP),
            (GroupUpdateMask::MAX_HP, 0x0000_0004, Gtker::MAX_HP),
            (GroupUpdateMask::POWER_TYPE, 0x0000_0008, Gtker::POWER_TYPE),
            (GroupUpdateMask::CUR_POWER, 0x0000_0010, Gtker::CUR_POWER),
            (GroupUpdateMask::MAX_POWER, 0x0000_0020, Gtker::MAX_POWER),
            (GroupUpdateMask::LEVEL, 0x0000_0040, Gtker::LEVEL),
            (GroupUpdateMask::ZONE, 0x0000_0080, Gtker::ZONE),
            (GroupUpdateMask::POSITION, 0x0000_0100, Gtker::POSITION),
        ];
        for (mask, cmangos, gtker) in pinned {
            assert_eq!((mask.bits(), mask.bits()), (cmangos, gtker));
        }
        assert_eq!(GroupUpdateMask::MEMBER.bits(), 0x1FF);
    }

    #[test]
    fn status_bits_are_the_cmangos_member_status() {
        use wow_world_messages::vanilla::GroupMemberOnlineStatus as Gtker;
        // cm:Group.h:47-51, then gtker's constants.
        let pinned = [
            (member_status::OFFLINE, 0x00, Gtker::OFFLINE),
            (member_status::ONLINE, 0x01, Gtker::ONLINE),
            (member_status::DEAD, 0x04, Gtker::DEAD),
            (member_status::GHOST, 0x08, Gtker::GHOST),
        ];
        for (bit, cmangos, gtker) in pinned {
            assert_eq!((bit, bit), (cmangos, gtker));
        }
    }

    #[test]
    fn each_field_has_the_cmangos_width() {
        // `GroupUpdateLength`, cm:Group.h:97 and vm:Group.h:153, entries 0-8.
        const GROUP_UPDATE_LENGTH: [usize; 9] = [1, 2, 2, 1, 2, 2, 2, 2, 4];
        let header = 3 + 4;
        for (bit, width) in GROUP_UPDATE_LENGTH.into_iter().enumerate() {
            let field = GroupUpdateMask(1 << bit);
            let (_, body) = build_member_stats(MemberStatsPacket::Changed, GUID, field, &caster());
            assert_eq!(body.len() - header, width, "field bit {bit}");
        }
    }

    #[test]
    fn a_full_body_matches_hand_derived_bytes() {
        let (opcode, body) = build_member_stats(
            MemberStatsPacket::Changed,
            GUID,
            GroupUpdateMask::MEMBER,
            &caster(),
        );
        assert_eq!(opcode, 0x007E);
        #[rustfmt::skip]
        let expected = [
            0x03, 0x02, 0x01,       // packed guid 0x0102
            0xFF, 0x01, 0x00, 0x00, // mask 0x1FF
            0x01,                   // status ONLINE
            0xD2, 0x04,             // current health 1234
            0xDC, 0x05,             // max health 1500
            0x00,                   // power type mana
            0x20, 0x03,             // current power 800
            0xE8, 0x03,             // max power 1000
            0x14, 0x00,             // level 20
            0x0C, 0x00,             // zone 12, u16
            0x0B, 0xDD,             // x -8949
            0x7C, 0xFF,             // y -132
        ];
        assert_eq!(body, expected);
    }

    #[test]
    fn a_single_field_body_carries_only_that_field() {
        let stats = MemberStats {
            current_health: 1000,
            ..caster()
        };
        let (_, body) = build_member_stats(
            MemberStatsPacket::Changed,
            GUID,
            GroupUpdateMask::CUR_HP,
            &stats,
        );
        assert_eq!(body, [0x03, 0x02, 0x01, 0x02, 0x00, 0x00, 0x00, 0xE8, 0x03]);
    }

    #[test]
    fn the_offline_answer_is_a_full_packet_with_status_zero() {
        let (opcode, body) = build_member_offline(MemberStatsPacket::Full, GUID);
        assert_eq!(opcode, 0x02F2);
        assert_eq!(body, [0x03, 0x02, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn gtker_reads_every_field_except_the_zone_the_same_way() {
        // gtker reads ZONE as a u32, so the cross-check leaves that one field out.
        let mask = GroupUpdateMask(GroupUpdateMask::MEMBER.bits() & !GroupUpdateMask::ZONE.bits());
        let (opcode, body) = build_member_stats(MemberStatsPacket::Changed, GUID, mask, &caster());
        let ServerOpcodeMessage::SMSG_PARTY_MEMBER_STATS(decoded) = gtker_decode(opcode, &body)
        else {
            panic!("expected SMSG_PARTY_MEMBER_STATS");
        };
        assert_eq!(decoded.guid.guid(), GUID);
        let fields = &decoded.mask;
        assert_eq!(fields.get_status().unwrap().status.as_int(), 1);
        assert_eq!(fields.get_cur_hp().unwrap().current_health, 1234);
        assert_eq!(fields.get_max_hp().unwrap().max_health, 1500);
        assert_eq!(fields.get_power_type().unwrap().power, Power::Mana);
        assert_eq!(fields.get_cur_power().unwrap().current_power, 800);
        assert_eq!(fields.get_max_power().unwrap().max_power, 1000);
        assert_eq!(fields.get_level().unwrap().level.as_int(), 20);
        let position = fields.get_position().unwrap();
        assert_eq!((position.position_x, position.position_y), (0xDD0B, 0xFF7C));

        let (opcode, body) = build_member_offline(MemberStatsPacket::Full, GUID);
        let ServerOpcodeMessage::SMSG_PARTY_MEMBER_STATS_FULL(decoded) =
            gtker_decode(opcode, &body)
        else {
            panic!("expected SMSG_PARTY_MEMBER_STATS_FULL");
        };
        assert_eq!(decoded.player.guid(), GUID);
        assert_eq!(decoded.mask.get_status().unwrap().status.as_int(), 0);
    }

    #[test]
    fn a_live_entity_projects_to_wire_values() {
        let entity = MemberEntity {
            health: 1234,
            max_health: 1500,
            power: 800,
            max_power: 1000,
            unit_bytes_0: 0x0100_0101, // human warrior with rage in byte 3
            level: 20,
            zone_id: 12,
            x: -8949.95,
            y: -132.49,
            dead: false,
            player_flags: 0,
        };
        assert_eq!(
            MemberStats::from_entity(&entity),
            MemberStats {
                power_type: 1,
                ..caster()
            }
        );
    }

    #[test]
    fn values_above_u16_saturate_and_negative_coordinates_keep_their_bits() {
        let stats = MemberStats::from_entity(&MemberEntity {
            health: 70_000,
            max_health: 65_536,
            power: 100_000,
            max_power: u32::MAX,
            x: -1.5,
            y: -32768.0,
            ..Default::default()
        });
        assert_eq!((stats.current_health, stats.max_health), (65535, 65535));
        assert_eq!((stats.current_power, stats.max_power), (65535, 65535));
        assert_eq!(stats.position_x, 0xFFFF);
        assert_eq!(stats.position_y, 0x8000);
    }

    #[test]
    fn dead_and_ghost_members_carry_their_status_bits() {
        let corpse = MemberStats::from_entity(&MemberEntity {
            dead: true,
            ..Default::default()
        });
        assert_eq!(corpse.status, 0x05);
        let ghost = MemberStats::from_entity(&MemberEntity {
            dead: true,
            player_flags: 0x10,
            ..Default::default()
        });
        assert_eq!(ghost.status, 0x0D);
    }

    #[test]
    fn a_viewer_holding_nothing_gets_every_field() {
        assert_eq!(stats_delta(None, &caster()), GroupUpdateMask::MEMBER);
    }

    #[test]
    fn each_changed_field_sets_only_its_own_bit() {
        let before = caster();
        let cases: [(MemberStats, GroupUpdateMask); 9] = [
            (
                MemberStats {
                    status: 0x05,
                    ..before
                },
                GroupUpdateMask::STATUS,
            ),
            (
                MemberStats {
                    current_health: 1,
                    ..before
                },
                GroupUpdateMask::CUR_HP,
            ),
            (
                MemberStats {
                    max_health: 1,
                    ..before
                },
                GroupUpdateMask::MAX_HP,
            ),
            (
                MemberStats {
                    current_power: 1,
                    ..before
                },
                GroupUpdateMask::CUR_POWER,
            ),
            (
                MemberStats {
                    max_power: 1,
                    ..before
                },
                GroupUpdateMask::MAX_POWER,
            ),
            (
                MemberStats {
                    level: 21,
                    ..before
                },
                GroupUpdateMask::LEVEL,
            ),
            (
                MemberStats {
                    zone: 1519,
                    ..before
                },
                GroupUpdateMask::ZONE,
            ),
            (
                MemberStats {
                    position_x: 1,
                    ..before
                },
                GroupUpdateMask::POSITION,
            ),
            (
                MemberStats {
                    position_y: 1,
                    ..before
                },
                GroupUpdateMask::POSITION,
            ),
        ];
        for (after, expected) in cases {
            assert_eq!(stats_delta(Some(&before), &after), expected, "{after:?}");
        }
        assert!(stats_delta(Some(&before), &before).is_empty());
    }

    #[test]
    fn a_power_type_change_resends_both_power_values() {
        let before = caster();
        let shapeshifted = MemberStats {
            power_type: 1,
            ..before
        };
        assert_eq!(
            stats_delta(Some(&before), &shapeshifted).bits(),
            0x08 | 0x10 | 0x20
        );
    }
}
