use lyracore_protocol::codec;
use lyracore_protocol::messages::vanilla::{opcodes::ServerOpcodeMessage, SMSG_UPDATE_OBJECT};
use spacetimedb::ReducerContext;

use crate::aoi::{game_aoi_grant, WORLD_ENTITY};
use crate::{game_world_entity, WorldEntity};

/// A viewer outside the unit's raid sees health as a percent of a maximum of 100.
/// This follows vmangos Object::BuildValuesUpdate and Player::CanSeeHealthOf.
pub(super) fn visible_health(ctx: &ReducerContext, unit: &WorldEntity, viewer: u64) -> (u32, u32) {
    match health_owner(ctx, unit) {
        Some(owner) if sees_owner_health(ctx, owner, viewer) => (unit.health, unit.max_health),
        _ => hidden_health(unit.health, unit.max_health),
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Vitals {
    Health,
    Maximum,
}

/// Sends a health or maximum health change to each viewer of `unit` as `visible_health` shows it.
pub(super) fn send(ctx: &ReducerContext, unit: &WorldEntity, vitals: Vitals) {
    let hidden = hidden_health(unit.health, unit.max_health);
    let Some(owner) = health_owner(ctx, unit) else {
        broadcast(ctx, unit, vitals, hidden, false);
        return;
    };
    if owner == unit.guid && crate::group::group_of(ctx, owner).is_none() {
        // Only the ungrouped Character sees its own health, so one AOI packet serves the rest.
        broadcast(ctx, unit, vitals, hidden, true);
        if !super::is_world_porting(ctx, owner) {
            if let Some(packet) = encode(unit, vitals, (unit.health, unit.max_health)) {
                super::send_raw_to_character(ctx, owner, packet.opcode, packet.body);
            }
        }
        return;
    }
    for grant in ctx
        .db
        .game_aoi_grant()
        .by_subject()
        .filter((WORLD_ENTITY, unit.guid))
    {
        if super::is_world_porting(ctx, grant.viewer_guid) {
            continue;
        }
        let health = if sees_owner_health(ctx, owner, grant.viewer_guid) {
            (unit.health, unit.max_health)
        } else {
            hidden
        };
        if let Some(packet) = encode(unit, vitals, health) {
            super::send_raw_to_character(ctx, grant.viewer_guid, packet.opcode, packet.body);
        }
    }
}

fn broadcast(
    ctx: &ReducerContext,
    unit: &WorldEntity,
    vitals: Vitals,
    health: (u32, u32),
    exclude_self: bool,
) {
    if let Some(packet) = encode(unit, vitals, health) {
        super::visibility::broadcast(ctx, unit.guid, packet.opcode, packet.body, exclude_self);
    }
}

fn encode(
    unit: &WorldEntity,
    vitals: Vitals,
    (health, max_health): (u32, u32),
) -> Option<lyracore_protocol::world::Packet> {
    let values: SMSG_UPDATE_OBJECT = match vitals {
        Vitals::Health => codec::build_health_values(unit.guid, health),
        Vitals::Maximum => codec::build_max_vitals_values(
            unit.guid,
            max_health,
            (unit.unit_bytes_0 >> 24) as u8,
            unit.max_power,
        ),
    };
    lyracore_protocol::world::encode(&ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(values)))
        .inspect_err(|error| {
            spacetimedb::log::error!("native health VALUES failed for {}: {error}", unit.guid);
        })
        .ok()
}

/// The Character that answers for `unit`'s health: the Character itself or a pet's owner.
fn health_owner(ctx: &ReducerContext, unit: &WorldEntity) -> Option<u64> {
    if unit.is_player() {
        return Some(unit.guid);
    }
    ctx.db
        .game_world_entity()
        .guid()
        .find(unit.owner_guid)
        .filter(WorldEntity::is_player)
        .map(|owner| owner.guid)
}

fn sees_owner_health(ctx: &ReducerContext, owner: u64, viewer: u64) -> bool {
    owner == viewer
        || crate::group::group_of(ctx, owner).is_some_and(|member| {
            crate::group::group_of(ctx, viewer)
                .is_some_and(|other| other.group_id == member.group_id)
        })
}

fn hidden_health(health: u32, max_health: u32) -> (u32, u32) {
    if health == 0 {
        return (0, 100);
    }
    let percent = (health as f32 * 100.0 / max_health as f32) as u32;
    (percent.clamp(1, 100), 100)
}

#[cfg(test)]
mod tests {
    use super::hidden_health;

    #[test]
    fn hidden_health_truncates_to_a_percent_of_one_hundred() {
        assert_eq!(hidden_health(60, 60), (100, 100));
        assert_eq!(hidden_health(59, 60), (98, 100));
        assert_eq!(hidden_health(1, 1_000), (1, 100));
        assert_eq!(hidden_health(0, 60), (0, 100));
    }
}
