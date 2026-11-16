use lyracore_protocol::codec;
use lyracore_protocol::messages::vanilla::opcodes::ServerOpcodeMessage;
use spacetimedb::{ReducerContext, Table};

use super::game_native_player;
use crate::aoi::{game_aoi_grant, WORLD_ENTITY};
use crate::{game_world_entity, WorldEntity};

/// A viewer outside the unit's raid sees health as a percent of a maximum of 100.
/// This follows vmangos Object::BuildValuesUpdate and Player::CanSeeHealthOf.
pub(super) fn visible_health(ctx: &ReducerContext, unit: &WorldEntity, viewer: u64) -> (u32, u32) {
    if health_owner(ctx, unit).is_some_and(|owner| raw_viewers(ctx, owner).contains(&viewer)) {
        (unit.health, unit.max_health)
    } else {
        hidden_health(unit.health, unit.max_health)
    }
}

/// Sends `unit`'s health and maximum health to each viewer as `visible_health` shows them.
pub(crate) fn send(ctx: &ReducerContext, unit: &WorldEntity) {
    if ctx.db.game_native_player().count() == 0 {
        return;
    }
    let owner = health_owner(ctx, unit);
    let exclude_connection = owner
        .and_then(|owner| super::world_session(ctx, owner))
        .map(|session| session.connection_id);
    if let Some(packet) = encode(unit, hidden_health(unit.health, unit.max_health)) {
        super::visibility::broadcast_aoi(
            ctx,
            super::NativeAoi {
                layer: WORLD_ENTITY,
                entity_guid: unit.guid,
                exclude_connection,
            },
            packet.opcode,
            packet.body,
        );
    }
    let Some(owner) = owner else {
        return;
    };
    let Some(raw) = encode(unit, (unit.health, unit.max_health)) else {
        return;
    };
    // The raw packet follows the AOI percent, so a group member keeps the raw value.
    for viewer in raw_viewers(ctx, owner) {
        let sees_unit = ctx
            .db
            .game_aoi_grant()
            .by_viewer()
            .filter((viewer, WORLD_ENTITY, unit.guid))
            .next()
            .is_some();
        if sees_unit && !super::is_world_porting(ctx, viewer) {
            super::send_raw_to_character(ctx, viewer, raw.opcode, raw.body.clone());
        }
    }
}

/// Resends the health of `characters` and their pets after their group changed.
/// vmangos Group::BroadcastGroupUpdate does this for every member and controlled unit.
pub(crate) fn group_changed(ctx: &ReducerContext, characters: impl IntoIterator<Item = u64>) {
    if ctx.db.game_native_player().count() == 0 {
        return;
    }
    let entities = ctx.db.game_world_entity();
    for guid in characters {
        if let Some(character) = entities.guid().find(guid) {
            send(ctx, &character);
        }
        for pet in entities.by_owner_guid().filter(guid) {
            send(ctx, &pet);
        }
    }
}

fn encode(
    unit: &WorldEntity,
    (health, max_health): (u32, u32),
) -> Option<lyracore_protocol::world::Packet> {
    let values = codec::build_health_values(unit.guid, health, max_health);
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

fn raw_viewers(ctx: &ReducerContext, owner: u64) -> Vec<u64> {
    match crate::group::group_of(ctx, owner) {
        Some(member) => crate::group::members_of(ctx, member.group_id)
            .into_iter()
            .map(|member| member.character_guid)
            .collect(),
        None => vec![owner],
    }
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
