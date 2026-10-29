use std::collections::BTreeSet;

use lyracore_protocol::codec::{self, member_status, MemberStatsPacket};
use spacetimedb::{
    log, reducer, table, ConnectionId, ReducerContext, ScheduleAt, SpacetimeType, Table,
    TimeDuration,
};

use super::{game_native_player, Rollback};
use crate::game_hunter_pet_protocol;
use crate::{aoi::game_aoi_grant, game_creature_template, game_world_entity, spell::game_aura};

#[derive(SpacetimeType, Clone)]
pub struct MemberAura {
    slot: u8,
    spell_id: u32,
}

#[derive(SpacetimeType, Clone)]
pub struct MemberPet {
    guid: u64,
    name: String,
    display_id: u32,
    health: u32,
    max_health: u32,
    power: u32,
    max_power: u32,
    unit_bytes_0: u32,
    auras: Vec<MemberAura>,
}

#[derive(SpacetimeType, Clone)]
pub struct MemberEntity {
    health: u32,
    max_health: u32,
    power: u32,
    max_power: u32,
    unit_bytes_0: u32,
    level: u32,
    zone_id: u32,
    x: f32,
    y: f32,
    dead: bool,
    player_flags: u32,
    auras: Vec<MemberAura>,
    pet: Option<MemberPet>,
}

#[derive(SpacetimeType, Clone)]
pub struct MemberSnapshot {
    guid: u64,
    entity: Option<MemberEntity>,
}

#[table(accessor = game_native_member_stats, scheduled(tick_native_member_stats))]
pub struct NativeMemberStats {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub scheduled_at: ScheduleAt,
    #[unique]
    pub connection_id: ConnectionId,
    #[unique]
    pub owner_guid: u64,
    pub snapshots: Vec<MemberSnapshot>,
}

crate::character_owned!(delete, fn sweep_delete_game_native_member_stats(ctx, character_guid) {
    ctx.db.game_native_member_stats().owner_guid().delete(character_guid);
});

// Member snapshots describe packets sent on one native connection.
crate::character_owned!(not_transported, fn sweep_transfer_game_native_member_stats());

pub(super) fn start(ctx: &ReducerContext, connection_id: ConnectionId, owner_guid: u64) {
    let records = ctx.db.game_native_member_stats();
    if records.connection_id().find(connection_id).is_none() {
        records.insert(NativeMemberStats {
            id: 0,
            scheduled_at: crate::time::schedule_interval(ctx, TimeDuration::from_micros(5_000_000)),
            connection_id,
            owner_guid,
            snapshots: Vec::new(),
        });
    }
}

pub(super) fn stop(ctx: &ReducerContext, connection_id: ConnectionId) {
    ctx.db
        .game_native_member_stats()
        .connection_id()
        .delete(connection_id);
}

fn mates(ctx: &ReducerContext, guid: u64) -> Result<Vec<u64>, crate::Refusal> {
    let Some((member, group)) = crate::group::checked_group_membership(ctx, guid)
        .map_err(|cause| format!("member stats for {guid}: {cause:?}"))?
    else {
        return Ok(Vec::new());
    };
    let members =
        crate::group::bounded_roster(ctx, member.group_id, crate::group::group_kind_of(&group))
            .ok_or_else(|| format!("group {} exceeds its member limit", member.group_id))?;
    Ok(members
        .into_iter()
        .map(|member| member.character_guid)
        .filter(|member| *member != guid)
        .collect())
}

enum Presence {
    Live(MemberEntity),
    Loading,
    Offline,
}

fn presence(ctx: &ReducerContext, guid: u64) -> Presence {
    if let Some(owner) = ctx.db.game_native_player().character_guid().find(guid) {
        if owner.teleport == Some(super::teleport::NativeTeleport::WorldPort) {
            return Presence::Loading;
        }
    }
    match ctx.db.game_world_entity().guid().find(guid) {
        Some(entity) => Presence::Live(MemberEntity {
            health: entity.health,
            max_health: entity.max_health,
            power: entity.power,
            max_power: entity.max_power,
            unit_bytes_0: entity.unit_bytes_0,
            level: entity.level,
            zone_id: entity.zone_id,
            x: entity.x,
            y: entity.y,
            dead: entity.dead,
            player_flags: entity.player_flags,
            auras: auras(ctx, guid),
            pet: pet(ctx, guid),
        }),
        None => Presence::Offline,
    }
}

fn auras(ctx: &ReducerContext, guid: u64) -> Vec<MemberAura> {
    ctx.db
        .game_aura()
        .by_target()
        .filter(guid)
        .map(|aura| MemberAura {
            slot: aura.slot,
            spell_id: aura.spell_id,
        })
        .collect()
}

fn pet(ctx: &ReducerContext, guid: u64) -> Option<MemberPet> {
    let pet_guid = lyracore_shared::pet::pet_guid_for(guid);
    let pet = ctx
        .db
        .game_world_entity()
        .guid()
        .find(pet_guid)
        .filter(|pet| pet.owner_guid == guid)?;
    let name = ctx
        .db
        .game_hunter_pet_protocol()
        .by_owner()
        .filter(guid)
        .find(|hunter| hunter.live_pet_guid == pet_guid)
        .map(|hunter| hunter.name)
        .or_else(|| {
            ctx.db
                .game_creature_template()
                .entry()
                .find(pet.entry)
                .map(|template| template.name)
        })
        .unwrap_or_default();
    Some(MemberPet {
        guid: pet.guid,
        name,
        display_id: pet.display_id,
        health: pet.health,
        max_health: pet.max_health,
        power: pet.power,
        max_power: pet.max_power,
        unit_bytes_0: pet.unit_bytes_0,
        auras: auras(ctx, pet_guid),
    })
}

impl MemberEntity {
    fn stats(&self) -> codec::MemberStats {
        let auras = |auras: &[MemberAura]| {
            auras
                .iter()
                .map(|aura| codec::MemberAuraSlot {
                    slot: aura.slot,
                    spell_id: aura.spell_id,
                })
                .collect()
        };
        codec::MemberStats::from_entity(&codec::MemberEntity {
            health: self.health,
            max_health: self.max_health,
            power: self.power,
            max_power: self.max_power,
            unit_bytes_0: self.unit_bytes_0,
            level: self.level,
            zone_id: self.zone_id,
            x: self.x,
            y: self.y,
            dead: self.dead,
            player_flags: self.player_flags,
            auras: auras(&self.auras),
            pet: self.pet.as_ref().map(|pet| codec::MemberPetEntity {
                guid: pet.guid,
                name: pet.name.clone(),
                display_id: pet.display_id,
                health: pet.health,
                max_health: pet.max_health,
                power: pet.power,
                max_power: pet.max_power,
                unit_bytes_0: pet.unit_bytes_0,
                auras: auras(&pet.auras),
            }),
        })
    }
}

pub(super) fn request(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    target: u64,
) -> Result<(), Rollback> {
    let packet = if mates(ctx, guid)?.contains(&target) {
        match presence(ctx, target) {
            Presence::Live(entity) => {
                let stats = entity.stats();
                codec::build_member_stats(
                    MemberStatsPacket::Full,
                    target,
                    codec::full_update_mask(&stats),
                    None,
                    &stats,
                )
            }
            Presence::Loading => codec::build_member_status(
                MemberStatsPacket::Full,
                target,
                member_status::ONLINE | member_status::ZONE_OUT,
            ),
            Presence::Offline => {
                codec::build_member_status(MemberStatsPacket::Full, target, member_status::OFFLINE)
            }
        }
    } else {
        codec::build_member_status(MemberStatsPacket::Full, target, member_status::OFFLINE)
    };
    if let Some(mut record) = ctx
        .db
        .game_native_member_stats()
        .connection_id()
        .find(connection)
    {
        let count = record.snapshots.len();
        record.snapshots.retain(|snapshot| snapshot.guid != target);
        if record.snapshots.len() != count {
            ctx.db.game_native_member_stats().id().update(record);
        }
    }
    super::send_raw(ctx, connection, packet.0, packet.1, false);
    Ok(())
}

#[reducer]
pub fn tick_native_member_stats(ctx: &ReducerContext, record: NativeMemberStats) {
    if !crate::time::scheduler_authorized(ctx) {
        return;
    }
    let Some(mut record) = ctx.db.game_native_member_stats().id().find(record.id) else {
        return;
    };
    let Some(owner) = ctx
        .db
        .game_native_player()
        .connection_id()
        .find(record.connection_id)
        .filter(|owner| owner.character_guid == record.owner_guid)
    else {
        stop(ctx, record.connection_id);
        return;
    };
    if owner.teleport.is_some() {
        return;
    }
    let mates = match mates(ctx, owner.character_guid) {
        Ok(mates) if !mates.is_empty() => mates,
        Ok(_) => {
            stop(ctx, record.connection_id);
            return;
        }
        Err(cause) => {
            log::error!("{cause}");
            return;
        }
    };
    let visible: BTreeSet<_> = ctx
        .db
        .game_aoi_grant()
        .by_viewer()
        .filter(owner.character_guid)
        .filter(|grant| grant.layer == crate::aoi::WORLD_ENTITY)
        .map(|grant| grant.entity_guid)
        .collect();
    let count = record.snapshots.len();
    record
        .snapshots
        .retain(|snapshot| mates.contains(&snapshot.guid) && !visible.contains(&snapshot.guid));
    let mut changed = count != record.snapshots.len();
    for guid in mates {
        if visible.contains(&guid) {
            continue;
        }
        let previous = record
            .snapshots
            .iter()
            .find(|snapshot| snapshot.guid == guid);
        let (entity, packet) = match presence(ctx, guid) {
            Presence::Loading => continue,
            Presence::Offline => {
                if previous.is_some_and(|previous| previous.entity.is_none()) {
                    continue;
                }
                (
                    None,
                    Some(codec::build_member_status(
                        MemberStatsPacket::Changed,
                        guid,
                        member_status::OFFLINE,
                    )),
                )
            }
            Presence::Live(entity) => {
                let current = entity.stats();
                let previous = previous
                    .and_then(|snapshot| snapshot.entity.as_ref())
                    .map(MemberEntity::stats);
                let delta = codec::stats_delta(previous.as_ref(), &current);
                let packet = (!delta.is_empty()).then(|| {
                    codec::build_member_stats(
                        MemberStatsPacket::Changed,
                        guid,
                        delta,
                        previous.as_ref(),
                        &current,
                    )
                });
                (Some(entity), packet)
            }
        };
        if let Some((opcode, body)) = packet {
            super::send_raw(ctx, record.connection_id, opcode, body, false);
            record.snapshots.retain(|snapshot| snapshot.guid != guid);
            record.snapshots.push(MemberSnapshot { guid, entity });
            changed = true;
        }
    }
    if changed {
        ctx.db.game_native_member_stats().id().update(record);
    }
}
