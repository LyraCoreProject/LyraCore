//! gtker uses u32 for the u16 zone and negative-aura mask fields; encode these packets raw.

use super::values::write_packed_guid_u64;
use lyracore_shared::constants::player_flags;
use std::ops::{BitOr, BitOrAssign};

pub const SMSG_PARTY_MEMBER_STATS_OPCODE: u16 = 0x007E;
pub const SMSG_PARTY_MEMBER_STATS_FULL_OPCODE: u16 = 0x02F2;

pub mod member_status {
    pub const OFFLINE: u8 = 0x00;
    pub const ONLINE: u8 = 0x01;
    pub const DEAD: u8 = 0x04;
    pub const GHOST: u8 = 0x08;
    pub const ZONE_OUT: u8 = 0x20;
}

// Positive aura slots 0..31 use a u32 mask.
pub const MAX_POSITIVE_AURAS: u8 = 32;
// Negative aura slots 32..47 use a u16 mask, with bit n selecting slot 32 + n.
pub const MAX_AURAS: u8 = 48;

const POSITIVE_AURA_SLOTS: usize = MAX_POSITIVE_AURAS as usize;
const NEGATIVE_AURA_SLOTS: usize = (MAX_AURAS - MAX_POSITIVE_AURAS) as usize;

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
    pub const AURAS: Self = Self(0x0000_0200);
    pub const AURAS_NEGATIVE: Self = Self(0x0000_0400);
    pub const PET_GUID: Self = Self(0x0000_0800);
    pub const PET_NAME: Self = Self(0x0000_1000);
    pub const PET_MODEL_ID: Self = Self(0x0000_2000);
    pub const PET_CUR_HP: Self = Self(0x0000_4000);
    pub const PET_MAX_HP: Self = Self(0x0000_8000);
    pub const PET_POWER_TYPE: Self = Self(0x0001_0000);
    pub const PET_CUR_POWER: Self = Self(0x0002_0000);
    pub const PET_MAX_POWER: Self = Self(0x0004_0000);
    pub const PET_AURAS: Self = Self(0x0008_0000);
    pub const PET_AURAS_NEGATIVE: Self = Self(0x0010_0000);

    pub const MEMBER: Self = Self(0x0000_01FF);
    pub const PET: Self = Self(0x001F_F800);
    pub const FULL: Self = Self(0x001F_FFFF);

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemberAuraSlot {
    pub slot: u8,
    pub spell_id: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemberPetEntity {
    pub guid: u64,
    pub name: String,
    pub display_id: u32,
    pub health: u32,
    pub max_health: u32,
    pub power: u32,
    pub max_power: u32,
    /// Byte 3 is the power type.
    pub unit_bytes_0: u32,
    pub auras: Vec<MemberAuraSlot>,
}

#[derive(Clone, Debug, Default, PartialEq)]
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
    pub auras: Vec<MemberAuraSlot>,
    pub pet: Option<MemberPetEntity>,
}

/// The wire has no absent-pet tag; zero every pet field when a pet disappears.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PetStats {
    pub guid: u64,
    pub name: String,
    pub model_id: u16,
    pub current_health: u16,
    pub max_health: u16,
    pub power_type: u8,
    pub current_power: u16,
    pub max_power: u16,
    pub auras: [u16; POSITIVE_AURA_SLOTS],
    pub auras_negative: [u16; NEGATIVE_AURA_SLOTS],
}

impl PetStats {
    fn from_entity(pet: &MemberPetEntity) -> Self {
        let narrow = |value: u32| u16::try_from(value).unwrap_or(u16::MAX);
        Self {
            guid: pet.guid,
            name: pet.name.clone(),
            model_id: narrow(pet.display_id),
            current_health: narrow(pet.health),
            max_health: narrow(pet.max_health),
            power_type: (pet.unit_bytes_0 >> 24) as u8,
            current_power: narrow(pet.power),
            max_power: narrow(pet.max_power),
            auras: positive_aura_slots(&pet.auras),
            auras_negative: negative_aura_slots(&pet.auras),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
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
    pub auras: [u16; POSITIVE_AURA_SLOTS],
    pub auras_negative: [u16; NEGATIVE_AURA_SLOTS],
    pub pet: PetStats,
}

fn positive_aura_slots(auras: &[MemberAuraSlot]) -> [u16; POSITIVE_AURA_SLOTS] {
    let mut slots = [0u16; POSITIVE_AURA_SLOTS];
    for aura in auras {
        if let Some(slot) = slots.get_mut(aura.slot as usize) {
            *slot = aura.spell_id as u16;
        }
    }
    slots
}

fn negative_aura_slots(auras: &[MemberAuraSlot]) -> [u16; NEGATIVE_AURA_SLOTS] {
    let mut slots = [0u16; NEGATIVE_AURA_SLOTS];
    for aura in auras {
        let Some(index) = aura.slot.checked_sub(MAX_POSITIVE_AURAS) else {
            continue;
        };
        if let Some(slot) = slots.get_mut(index as usize) {
            *slot = aura.spell_id as u16;
        }
    }
    slots
}

impl MemberStats {
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
            auras: positive_aura_slots(&entity.auras),
            auras_negative: negative_aura_slots(&entity.auras),
            pet: entity
                .pet
                .as_ref()
                .map_or_else(PetStats::default, PetStats::from_entity),
        }
    }
}

pub fn stats_delta(previous: Option<&MemberStats>, current: &MemberStats) -> GroupUpdateMask {
    let Some(previous) = previous else {
        return GroupUpdateMask::FULL;
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
    if previous.auras != current.auras {
        mask |= GroupUpdateMask::AURAS;
    }
    if previous.auras_negative != current.auras_negative {
        mask |= GroupUpdateMask::AURAS_NEGATIVE;
    }
    mask | pet_delta(&previous.pet, &current.pet)
}

fn pet_delta(previous: &PetStats, current: &PetStats) -> GroupUpdateMask {
    let mut mask = GroupUpdateMask::NONE;
    if previous.guid != current.guid {
        mask |= GroupUpdateMask::PET_GUID;
    }
    if previous.name != current.name {
        mask |= GroupUpdateMask::PET_NAME;
    }
    if previous.model_id != current.model_id {
        mask |= GroupUpdateMask::PET_MODEL_ID;
    }
    if previous.current_health != current.current_health {
        mask |= GroupUpdateMask::PET_CUR_HP;
    }
    if previous.max_health != current.max_health {
        mask |= GroupUpdateMask::PET_MAX_HP;
    }
    if previous.power_type != current.power_type {
        mask |= GroupUpdateMask::PET_POWER_TYPE
            | GroupUpdateMask::PET_CUR_POWER
            | GroupUpdateMask::PET_MAX_POWER;
    }
    if previous.current_power != current.current_power {
        mask |= GroupUpdateMask::PET_CUR_POWER;
    }
    if previous.max_power != current.max_power {
        mask |= GroupUpdateMask::PET_MAX_POWER;
    }
    if previous.auras != current.auras {
        mask |= GroupUpdateMask::PET_AURAS;
    }
    if previous.auras_negative != current.auras_negative {
        mask |= GroupUpdateMask::PET_AURAS_NEGATIVE;
    }
    mask
}

pub fn full_update_mask(stats: &MemberStats) -> GroupUpdateMask {
    let mask = GroupUpdateMask::MEMBER | GroupUpdateMask::AURAS | GroupUpdateMask::AURAS_NEGATIVE;
    if stats.pet.guid == 0 {
        mask
    } else {
        mask | GroupUpdateMask::PET
    }
}

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

fn push_cstr(body: &mut Vec<u8>, s: &str) {
    body.extend_from_slice(s.as_bytes());
    body.push(0);
}

fn positive_dirty_mask(
    previous: Option<&[u16; POSITIVE_AURA_SLOTS]>,
    current: &[u16; POSITIVE_AURA_SLOTS],
) -> u32 {
    match previous {
        None => u32::MAX,
        Some(previous) => {
            let mut dirty = 0u32;
            for (i, (&before, &after)) in previous.iter().zip(current).enumerate() {
                if before != after {
                    dirty |= 1 << i;
                }
            }
            dirty
        }
    }
}

fn negative_dirty_mask(
    previous: Option<&[u16; NEGATIVE_AURA_SLOTS]>,
    current: &[u16; NEGATIVE_AURA_SLOTS],
) -> u16 {
    match previous {
        None => u16::MAX,
        Some(previous) => {
            let mut dirty = 0u16;
            for (i, (&before, &after)) in previous.iter().zip(current).enumerate() {
                if before != after {
                    dirty |= 1 << i;
                }
            }
            dirty
        }
    }
}

fn write_positive_aura_block(body: &mut Vec<u8>, dirty: u32, current: &[u16; POSITIVE_AURA_SLOTS]) {
    body.extend_from_slice(&dirty.to_le_bytes());
    for (i, &value) in current.iter().enumerate() {
        if dirty & (1 << i) != 0 {
            body.extend_from_slice(&value.to_le_bytes());
        }
    }
}

fn write_negative_aura_block(body: &mut Vec<u8>, dirty: u16, current: &[u16; NEGATIVE_AURA_SLOTS]) {
    body.extend_from_slice(&dirty.to_le_bytes());
    for (i, &value) in current.iter().enumerate() {
        if dirty & (1 << i) != 0 {
            body.extend_from_slice(&value.to_le_bytes());
        }
    }
}

pub fn build_member_stats(
    packet: MemberStatsPacket,
    guid: u64,
    mask: GroupUpdateMask,
    previous: Option<&MemberStats>,
    stats: &MemberStats,
) -> (u16, Vec<u8>) {
    let mut body = Vec::with_capacity(64);
    write_packed_guid_u64(&mut body, guid);
    body.extend_from_slice(&mask.bits().to_le_bytes());

    if mask.contains(GroupUpdateMask::STATUS) {
        body.push(stats.status);
    }
    if mask.contains(GroupUpdateMask::CUR_HP) {
        body.extend_from_slice(&stats.current_health.to_le_bytes());
    }
    if mask.contains(GroupUpdateMask::MAX_HP) {
        body.extend_from_slice(&stats.max_health.to_le_bytes());
    }
    if mask.contains(GroupUpdateMask::POWER_TYPE) {
        body.push(stats.power_type);
    }
    if mask.contains(GroupUpdateMask::CUR_POWER) {
        body.extend_from_slice(&stats.current_power.to_le_bytes());
    }
    if mask.contains(GroupUpdateMask::MAX_POWER) {
        body.extend_from_slice(&stats.max_power.to_le_bytes());
    }
    if mask.contains(GroupUpdateMask::LEVEL) {
        body.extend_from_slice(&stats.level.to_le_bytes());
    }
    if mask.contains(GroupUpdateMask::ZONE) {
        body.extend_from_slice(&stats.zone.to_le_bytes());
    }
    if mask.contains(GroupUpdateMask::POSITION) {
        body.extend_from_slice(&stats.position_x.to_le_bytes());
        body.extend_from_slice(&stats.position_y.to_le_bytes());
    }
    if mask.contains(GroupUpdateMask::AURAS) {
        let dirty = positive_dirty_mask(previous.map(|p| &p.auras), &stats.auras);
        write_positive_aura_block(&mut body, dirty, &stats.auras);
    }
    if mask.contains(GroupUpdateMask::AURAS_NEGATIVE) {
        let dirty = negative_dirty_mask(previous.map(|p| &p.auras_negative), &stats.auras_negative);
        write_negative_aura_block(&mut body, dirty, &stats.auras_negative);
    }
    if mask.contains(GroupUpdateMask::PET_GUID) {
        body.extend_from_slice(&stats.pet.guid.to_le_bytes());
    }
    if mask.contains(GroupUpdateMask::PET_NAME) {
        push_cstr(&mut body, &stats.pet.name);
    }
    if mask.contains(GroupUpdateMask::PET_MODEL_ID) {
        body.extend_from_slice(&stats.pet.model_id.to_le_bytes());
    }
    if mask.contains(GroupUpdateMask::PET_CUR_HP) {
        body.extend_from_slice(&stats.pet.current_health.to_le_bytes());
    }
    if mask.contains(GroupUpdateMask::PET_MAX_HP) {
        body.extend_from_slice(&stats.pet.max_health.to_le_bytes());
    }
    if mask.contains(GroupUpdateMask::PET_POWER_TYPE) {
        body.push(stats.pet.power_type);
    }
    if mask.contains(GroupUpdateMask::PET_CUR_POWER) {
        body.extend_from_slice(&stats.pet.current_power.to_le_bytes());
    }
    if mask.contains(GroupUpdateMask::PET_MAX_POWER) {
        body.extend_from_slice(&stats.pet.max_power.to_le_bytes());
    }
    if mask.contains(GroupUpdateMask::PET_AURAS) {
        let dirty = positive_dirty_mask(previous.map(|p| &p.pet.auras), &stats.pet.auras);
        write_positive_aura_block(&mut body, dirty, &stats.pet.auras);
    }
    if mask.contains(GroupUpdateMask::PET_AURAS_NEGATIVE) {
        let dirty = negative_dirty_mask(
            previous.map(|p| &p.pet.auras_negative),
            &stats.pet.auras_negative,
        );
        write_negative_aura_block(&mut body, dirty, &stats.pet.auras_negative);
    }
    (packet.opcode(), body)
}

pub fn build_member_status(packet: MemberStatsPacket, guid: u64, status: u8) -> (u16, Vec<u8>) {
    let stats = MemberStats {
        status,
        ..MemberStats::default()
    };
    build_member_stats(packet, guid, GroupUpdateMask::STATUS, None, &stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
    use wow_world_messages::vanilla::Power;

    const GUID: u64 = 0x0102;

    fn full_group_update_mask() -> GroupUpdateMask {
        GroupUpdateMask::MEMBER
            | GroupUpdateMask::AURAS
            | GroupUpdateMask::AURAS_NEGATIVE
            | GroupUpdateMask::PET
    }

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
            ..MemberStats::default()
        }
    }

    fn caster_with_auras() -> MemberStats {
        let mut stats = caster();
        stats.auras[0] = 100;
        stats.auras[5] = 200;
        stats.auras_negative[3] = 300;
        stats
    }

    fn pet() -> PetStats {
        let mut pet = PetStats {
            guid: 85,
            name: "Fluffy".into(),
            model_id: 618,
            current_health: 50,
            max_health: 60,
            power_type: 2,
            current_power: 40,
            max_power: 100,
            ..PetStats::default()
        };
        pet.auras[2] = 400;
        pet.auras_negative[8] = 500;
        pet
    }

    fn full_positive_aura_bytes(occupied: &[(usize, u16)]) -> Vec<u8> {
        let mut slots = [0u16; POSITIVE_AURA_SLOTS];
        for &(slot, spell_id) in occupied {
            slots[slot] = spell_id;
        }
        let mut bytes = vec![0xFF, 0xFF, 0xFF, 0xFF];
        for spell_id in slots {
            bytes.extend_from_slice(&spell_id.to_le_bytes());
        }
        bytes
    }

    fn full_negative_aura_bytes(occupied: &[(usize, u16)]) -> Vec<u8> {
        let mut slots = [0u16; NEGATIVE_AURA_SLOTS];
        for &(slot, spell_id) in occupied {
            slots[slot] = spell_id;
        }
        let mut bytes = vec![0xFF, 0xFF];
        for spell_id in slots {
            bytes.extend_from_slice(&spell_id.to_le_bytes());
        }
        bytes
    }

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
            (GroupUpdateMask::AURAS, 0x0000_0200, Gtker::AURAS),
            (GroupUpdateMask::AURAS_NEGATIVE, 0x0000_0400, Gtker::AURAS_2),
            (GroupUpdateMask::PET_GUID, 0x0000_0800, Gtker::PET_GUID),
            (GroupUpdateMask::PET_NAME, 0x0000_1000, Gtker::PET_NAME),
            (
                GroupUpdateMask::PET_MODEL_ID,
                0x0000_2000,
                Gtker::PET_MODEL_ID,
            ),
            (GroupUpdateMask::PET_CUR_HP, 0x0000_4000, Gtker::PET_CUR_HP),
            (GroupUpdateMask::PET_MAX_HP, 0x0000_8000, Gtker::PET_MAX_HP),
            (
                GroupUpdateMask::PET_POWER_TYPE,
                0x0001_0000,
                Gtker::PET_POWER_TYPE,
            ),
            (
                GroupUpdateMask::PET_CUR_POWER,
                0x0002_0000,
                Gtker::PET_CUR_POWER,
            ),
            (
                GroupUpdateMask::PET_MAX_POWER,
                0x0004_0000,
                Gtker::PET_MAX_POWER,
            ),
            (GroupUpdateMask::PET_AURAS, 0x0008_0000, Gtker::PET_AURAS),
            (
                GroupUpdateMask::PET_AURAS_NEGATIVE,
                0x0010_0000,
                Gtker::PET_AURAS_2,
            ),
        ];
        for (mask, cmangos, gtker) in pinned {
            assert_eq!((mask.bits(), mask.bits()), (cmangos, gtker));
        }
        assert_eq!(GroupUpdateMask::MEMBER.bits(), 0x1FF);
        assert_eq!(GroupUpdateMask::PET.bits(), 0x1F_F800);
        assert_eq!(full_group_update_mask().bits(), 0x1F_FFFF);
    }

    #[test]
    fn status_bits_are_the_cmangos_member_status() {
        use wow_world_messages::vanilla::GroupMemberOnlineStatus as Gtker;
        let pinned = [
            (member_status::OFFLINE, 0x00, Gtker::OFFLINE),
            (member_status::ONLINE, 0x01, Gtker::ONLINE),
            (member_status::DEAD, 0x04, Gtker::DEAD),
            (member_status::GHOST, 0x08, Gtker::GHOST),
            (member_status::ZONE_OUT, 0x20, Gtker::ZONE_OUT),
        ];
        for (bit, cmangos, gtker) in pinned {
            assert_eq!((bit, bit), (cmangos, gtker));
        }
    }

    #[test]
    fn each_field_has_the_cmangos_width() {
        const GROUP_UPDATE_LENGTH: [usize; 9] = [1, 2, 2, 1, 2, 2, 2, 2, 4];
        let header = 3 + 4;
        for (bit, width) in GROUP_UPDATE_LENGTH.into_iter().enumerate() {
            let field = GroupUpdateMask(1 << bit);
            let (_, body) =
                build_member_stats(MemberStatsPacket::Changed, GUID, field, None, &caster());
            assert_eq!(body.len() - header, width, "field bit {bit}");
        }
    }

    #[test]
    fn each_pet_scalar_field_has_the_cmangos_width() {
        let stats = MemberStats {
            pet: pet(),
            ..caster()
        };
        let header = 3 + 4;
        let widths = [
            (GroupUpdateMask::PET_GUID, 8),
            (GroupUpdateMask::PET_MODEL_ID, 2),
            (GroupUpdateMask::PET_CUR_HP, 2),
            (GroupUpdateMask::PET_MAX_HP, 2),
            (GroupUpdateMask::PET_POWER_TYPE, 1),
            (GroupUpdateMask::PET_CUR_POWER, 2),
            (GroupUpdateMask::PET_MAX_POWER, 2),
        ];
        for (field, width) in widths {
            let (_, body) =
                build_member_stats(MemberStatsPacket::Changed, GUID, field, None, &stats);
            assert_eq!(body.len() - header, width, "{field:?}");
        }
    }

    #[test]
    fn a_full_body_matches_hand_derived_bytes() {
        let (opcode, body) = build_member_stats(
            MemberStatsPacket::Changed,
            GUID,
            GroupUpdateMask::MEMBER,
            None,
            &caster(),
        );
        assert_eq!(opcode, 0x007E);
        #[rustfmt::skip]
        let expected = [
            0x03, 0x02, 0x01,
            0xFF, 0x01, 0x00, 0x00,
            0x01,
            0xD2, 0x04,
            0xDC, 0x05,
            0x00,
            0x20, 0x03,
            0xE8, 0x03,
            0x14, 0x00,
            0x0C, 0x00,
            0x0B, 0xDD,
            0x7C, 0xFF,
        ];
        assert_eq!(body, expected);
    }

    #[test]
    fn a_full_body_with_auras_and_a_pet_matches_hand_derived_bytes() {
        let stats = MemberStats {
            pet: pet(),
            ..caster_with_auras()
        };
        let mask = full_update_mask(&stats);
        assert_eq!(mask, full_group_update_mask());
        let (opcode, body) = build_member_stats(MemberStatsPacket::Full, GUID, mask, None, &stats);
        assert_eq!(opcode, 0x02F2);

        let mut expected = vec![
            0x03, 0x02, 0x01, 0xFF, 0xFF, 0x1F, 0x00, 0x01, 0xD2, 0x04, 0xDC, 0x05, 0x00, 0x20,
            0x03, 0xE8, 0x03, 0x14, 0x00, 0x0C, 0x00, 0x0B, 0xDD, 0x7C, 0xFF,
        ];
        expected.extend(full_positive_aura_bytes(&[(0, 100), (5, 200)]));
        expected.extend(full_negative_aura_bytes(&[(3, 300)]));
        expected.extend([
            0x55, 0, 0, 0, 0, 0, 0, 0, b'F', b'l', b'u', b'f', b'f', b'y', 0x00, 0x6A, 0x02, 0x32,
            0x00, 0x3C, 0x00, 0x02, 0x28, 0x00, 0x64, 0x00,
        ]);
        expected.extend(full_positive_aura_bytes(&[(2, 400)]));
        expected.extend(full_negative_aura_bytes(&[(8, 500)]));
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
            None,
            &stats,
        );
        assert_eq!(body, [0x03, 0x02, 0x01, 0x02, 0x00, 0x00, 0x00, 0xE8, 0x03]);
    }

    #[test]
    fn positive_and_negative_auras_send_their_ids_at_the_right_bits() {
        let before = caster();
        let after = caster_with_auras();
        let mask = stats_delta(Some(&before), &after);
        assert_eq!(
            mask,
            GroupUpdateMask::AURAS | GroupUpdateMask::AURAS_NEGATIVE
        );

        let (_, body) = build_member_stats(
            MemberStatsPacket::Changed,
            GUID,
            mask,
            Some(&before),
            &after,
        );
        let header = 3 + 4;
        #[rustfmt::skip]
        let expected = [
            0x21, 0x00, 0x00, 0x00,
            0x64, 0x00,
            0xC8, 0x00,
            0x08, 0x00,
            0x2C, 0x01,
        ];
        assert_eq!(&body[header..], expected);
    }

    #[test]
    fn removing_a_buff_sends_only_its_bit_with_id_zero() {
        let before = caster_with_auras();
        let mut after = before.clone();
        after.auras[0] = 0;

        let mask = stats_delta(Some(&before), &after);
        assert_eq!(mask, GroupUpdateMask::AURAS);

        let (_, body) = build_member_stats(
            MemberStatsPacket::Changed,
            GUID,
            mask,
            Some(&before),
            &after,
        );
        let header = 3 + 4;
        assert_eq!(&body[header..], [0x01, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn replacing_an_aura_in_one_slot_sends_only_that_bit_with_the_new_id() {
        let before = caster_with_auras();
        let mut after = before.clone();
        after.auras[0] = 150;

        let mask = stats_delta(Some(&before), &after);
        assert_eq!(mask, GroupUpdateMask::AURAS);

        let (_, body) = build_member_stats(
            MemberStatsPacket::Changed,
            GUID,
            mask,
            Some(&before),
            &after,
        );
        let header = 3 + 4;
        assert_eq!(&body[header..], [0x01, 0x00, 0x00, 0x00, 0x96, 0x00]);
    }

    #[test]
    fn adding_a_debuff_sends_only_the_negative_bit() {
        let before = caster();
        let mut after = before.clone();
        after.auras_negative[10] = 700;

        let mask = stats_delta(Some(&before), &after);
        assert_eq!(mask, GroupUpdateMask::AURAS_NEGATIVE);

        let (_, body) = build_member_stats(
            MemberStatsPacket::Changed,
            GUID,
            mask,
            Some(&before),
            &after,
        );
        let header = 3 + 4;
        assert_eq!(&body[header..], [0x00, 0x04, 0xBC, 0x02]);
    }

    #[test]
    fn removing_a_debuff_sends_only_its_bit_with_id_zero() {
        let before = caster_with_auras();
        let mut after = before.clone();
        after.auras_negative[3] = 0;

        let mask = stats_delta(Some(&before), &after);
        assert_eq!(mask, GroupUpdateMask::AURAS_NEGATIVE);

        let (_, body) = build_member_stats(
            MemberStatsPacket::Changed,
            GUID,
            mask,
            Some(&before),
            &after,
        );
        let header = 3 + 4;
        assert_eq!(&body[header..], [0x08, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn a_pet_aura_delta_sends_only_the_pet_bit() {
        let before = MemberStats {
            pet: pet(),
            ..caster()
        };
        let mut after = before.clone();
        after.pet.auras[2] = 450;

        let mask = stats_delta(Some(&before), &after);
        assert_eq!(mask, GroupUpdateMask::PET_AURAS);

        let (_, body) = build_member_stats(
            MemberStatsPacket::Changed,
            GUID,
            mask,
            Some(&before),
            &after,
        );
        let header = 3 + 4;
        assert_eq!(&body[header..], [0x04, 0x00, 0x00, 0x00, 0xC2, 0x01]);
    }

    #[test]
    fn a_hunter_pet_sends_guid_name_display_health_power_and_auras() {
        let stats = MemberStats {
            pet: pet(),
            ..caster()
        };
        assert!(stats_delta(None, &stats).contains(GroupUpdateMask::PET));

        let (_, body) = build_member_stats(
            MemberStatsPacket::Changed,
            GUID,
            GroupUpdateMask::PET,
            None,
            &stats,
        );
        let decoded = gtker_decode(SMSG_PARTY_MEMBER_STATS_OPCODE, &body);
        let ServerOpcodeMessage::SMSG_PARTY_MEMBER_STATS(decoded) = decoded else {
            panic!("expected SMSG_PARTY_MEMBER_STATS");
        };
        assert_eq!(decoded.mask.get_pet_guid().unwrap().pet.guid(), 85);
        assert_eq!(decoded.mask.get_pet_name().unwrap().pet_name, "Fluffy");
        assert_eq!(decoded.mask.get_pet_model_id().unwrap().pet_display_id, 618);
        assert_eq!(
            decoded.mask.get_pet_cur_hp().unwrap().pet_current_health,
            50
        );
        assert_eq!(decoded.mask.get_pet_max_hp().unwrap().pet_max_health, 60);
        assert_eq!(
            decoded.mask.get_pet_cur_power().unwrap().pet_current_power,
            40
        );
        assert_eq!(decoded.mask.get_pet_max_power().unwrap().pet_max_power, 100);
        let pet_auras = decoded.mask.get_pet_auras().unwrap();
        assert_eq!(pet_auras.pet_auras.auras()[2], Some(400));
    }

    #[test]
    fn dismissing_the_pet_sends_the_pet_bits_zeroed_once() {
        let before = MemberStats {
            pet: pet(),
            ..caster()
        };
        let after = caster();

        let mask = stats_delta(Some(&before), &after);
        assert!(mask.contains(GroupUpdateMask::PET_GUID));
        assert!(mask.contains(GroupUpdateMask::PET_NAME));
        assert!(mask.contains(GroupUpdateMask::PET_AURAS));
        assert!(mask.contains(GroupUpdateMask::PET_AURAS_NEGATIVE));
        assert!(
            !mask.contains(GroupUpdateMask::STATUS),
            "only the pet changed"
        );

        let (_, body) = build_member_stats(
            MemberStatsPacket::Changed,
            GUID,
            mask,
            Some(&before),
            &after,
        );
        let header = 3 + 4;
        #[rustfmt::skip]
        let expected = [
            0, 0, 0, 0, 0, 0, 0, 0,
            0x00,
            0x00, 0x00,
            0x00, 0x00,
            0x00, 0x00,
            0x00,
            0x00, 0x00,
            0x00, 0x00,
            0x04, 0x00, 0x00, 0x00,
            0x00, 0x00,
            0x00, 0x01,
            0x00, 0x00,
        ];
        assert_eq!(&body[header..], expected);

        assert!(stats_delta(Some(&after), &after).is_empty());
    }

    #[test]
    fn a_member_without_a_pet_gets_a_full_packet_with_no_pet_bits() {
        let stats = caster();
        let mask = full_update_mask(&stats);
        assert_eq!(
            mask,
            GroupUpdateMask::MEMBER | GroupUpdateMask::AURAS | GroupUpdateMask::AURAS_NEGATIVE
        );
        assert!(!mask.contains(GroupUpdateMask::PET));

        let (_, body) = build_member_stats(MemberStatsPacket::Full, GUID, mask, None, &stats);
        assert_eq!(
            body.len(),
            (3 + 4) + 18 + (4 + POSITIVE_AURA_SLOTS * 2) + (2 + NEGATIVE_AURA_SLOTS * 2)
        );
    }

    #[test]
    fn the_offline_answer_is_a_full_packet_with_status_zero() {
        let (opcode, body) =
            build_member_status(MemberStatsPacket::Full, GUID, member_status::OFFLINE);
        assert_eq!(opcode, 0x02F2);
        assert_eq!(body, [0x03, 0x02, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn gtker_reads_every_field_except_the_zone_the_same_way() {
        // gtker reads ZONE as a u32, so the cross-check leaves that one field out.
        let mask = GroupUpdateMask(GroupUpdateMask::MEMBER.bits() & !GroupUpdateMask::ZONE.bits());
        let (opcode, body) =
            build_member_stats(MemberStatsPacket::Changed, GUID, mask, None, &caster());
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

        let (opcode, body) =
            build_member_status(MemberStatsPacket::Full, GUID, member_status::OFFLINE);
        let ServerOpcodeMessage::SMSG_PARTY_MEMBER_STATS_FULL(decoded) =
            gtker_decode(opcode, &body)
        else {
            panic!("expected SMSG_PARTY_MEMBER_STATS_FULL");
        };
        assert_eq!(decoded.player.guid(), GUID);
        assert_eq!(decoded.mask.get_status().unwrap().status.as_int(), 0);
    }

    #[test]
    fn gtkers_own_reader_treats_our_negative_aura_block_as_a_different_shape() {
        use std::io::Cursor;
        use wow_world_messages::vanilla::AuraMask;

        let before = caster();
        let after = caster_with_auras();
        let (_, body) = build_member_stats(
            MemberStatsPacket::Changed,
            GUID,
            GroupUpdateMask::AURAS_NEGATIVE,
            Some(&before),
            &after,
        );
        let header = 3 + 4;
        let negative_block = &body[header..];
        assert_eq!(
            negative_block,
            [0x08, 0x00, 0x2C, 0x01],
            "our u16 mask (slot 35, bit 3) plus one u16 spell id"
        );

        let mut cursor = Cursor::new(negative_block.to_vec());
        assert!(
            AuraMask::read(&mut cursor).is_err(),
            "gtker reads our 2-byte mask plus its 2-byte id as a 4-byte mask, then expects more \
             trailing ids than the 4-byte block actually holds"
        );
    }

    #[test]
    fn a_live_entity_projects_to_wire_values() {
        let entity = MemberEntity {
            health: 1234,
            max_health: 1500,
            power: 800,
            max_power: 1000,
            unit_bytes_0: 0x0100_0101,
            level: 20,
            zone_id: 12,
            x: -8949.95,
            y: -132.49,
            dead: false,
            player_flags: 0,
            ..MemberEntity::default()
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
    fn occupied_aura_slots_and_a_pet_project_to_wire_values() {
        let entity = MemberEntity {
            auras: vec![
                MemberAuraSlot {
                    slot: 0,
                    spell_id: 100,
                },
                MemberAuraSlot {
                    slot: 5,
                    spell_id: 200,
                },
                MemberAuraSlot {
                    slot: 35,
                    spell_id: 300,
                },
            ],
            pet: Some(MemberPetEntity {
                guid: 85,
                name: "Fluffy".into(),
                display_id: 618,
                health: 50,
                max_health: 60,
                power: 40,
                max_power: 100,
                unit_bytes_0: 0x0200_0000,
                auras: vec![
                    MemberAuraSlot {
                        slot: 2,
                        spell_id: 400,
                    },
                    MemberAuraSlot {
                        slot: 40,
                        spell_id: 500,
                    },
                ],
            }),
            ..caster_entity()
        };
        let stats = MemberStats::from_entity(&entity);
        assert_eq!(stats.auras, caster_with_auras().auras);
        assert_eq!(stats.auras_negative, caster_with_auras().auras_negative);
        assert_eq!(stats.pet, pet());
    }

    #[test]
    fn no_pet_and_no_auras_project_to_the_default_sentinel() {
        let stats = MemberStats::from_entity(&caster_entity());
        assert_eq!(stats.auras, [0u16; 32]);
        assert_eq!(stats.auras_negative, [0u16; 16]);
        assert_eq!(stats.pet, PetStats::default());
        assert_eq!(
            stats.pet.guid, 0,
            "guid 0 is the wire's own no-pet sentinel"
        );
    }

    fn caster_entity() -> MemberEntity {
        MemberEntity {
            health: 1234,
            max_health: 1500,
            power: 800,
            max_power: 1000,
            unit_bytes_0: 0,
            level: 20,
            zone_id: 12,
            x: -8949.95,
            y: -132.49,
            dead: false,
            player_flags: 0,
            ..MemberEntity::default()
        }
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
    fn a_viewer_holding_nothing_gets_every_field_pet_included() {
        assert_eq!(stats_delta(None, &caster()), full_group_update_mask());
        let stats = MemberStats {
            pet: pet(),
            ..caster_with_auras()
        };
        assert_eq!(stats_delta(None, &stats), full_group_update_mask());
    }

    #[test]
    fn each_changed_field_sets_only_its_own_bit() {
        let before = caster();
        let cases: [(MemberStats, GroupUpdateMask); 9] = [
            (
                MemberStats {
                    status: 0x05,
                    ..before.clone()
                },
                GroupUpdateMask::STATUS,
            ),
            (
                MemberStats {
                    current_health: 1,
                    ..before.clone()
                },
                GroupUpdateMask::CUR_HP,
            ),
            (
                MemberStats {
                    max_health: 1,
                    ..before.clone()
                },
                GroupUpdateMask::MAX_HP,
            ),
            (
                MemberStats {
                    current_power: 1,
                    ..before.clone()
                },
                GroupUpdateMask::CUR_POWER,
            ),
            (
                MemberStats {
                    max_power: 1,
                    ..before.clone()
                },
                GroupUpdateMask::MAX_POWER,
            ),
            (
                MemberStats {
                    level: 21,
                    ..before.clone()
                },
                GroupUpdateMask::LEVEL,
            ),
            (
                MemberStats {
                    zone: 1519,
                    ..before.clone()
                },
                GroupUpdateMask::ZONE,
            ),
            (
                MemberStats {
                    position_x: 1,
                    ..before.clone()
                },
                GroupUpdateMask::POSITION,
            ),
            (
                MemberStats {
                    position_y: 1,
                    ..before.clone()
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
            ..before.clone()
        };
        assert_eq!(
            stats_delta(Some(&before), &shapeshifted).bits(),
            0x08 | 0x10 | 0x20
        );
    }

    #[test]
    fn a_pet_power_type_change_resends_both_pet_power_values() {
        let before = MemberStats {
            pet: pet(),
            ..caster()
        };
        let mut shapeshifted = before.clone();
        shapeshifted.pet.power_type = 0;
        assert_eq!(
            stats_delta(Some(&before), &shapeshifted).bits(),
            0x1_0000 | 0x2_0000 | 0x4_0000
        );
    }
}
