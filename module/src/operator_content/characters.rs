use lyracore_shared::constants::player_flags;
use spacetimedb::{log, reducer, ReducerContext};

use crate::native::game_native_player;
use crate::transport::game_transport_passenger;
use crate::{game_character, game_map, game_world_entity};

// Read docs/operator-content-api.md#character-teleport before changing the admission rules.
#[reducer]
pub fn operator_teleport_character(
    ctx: &ReducerContext,
    name: String,
    map_id: u32,
    x: f32,
    y: f32,
    z: f32,
    o: f32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    super::check_position(x, y, z, o)?;
    let map = ctx
        .db
        .game_map()
        .id()
        .find(map_id)
        .ok_or_else(|| format!("map {map_id} is not in the Realm's map catalogue"))?;
    if map.map_type != 0 || lyracore_shared::instance::has_instance_population(map_id) {
        return Err(format!("map {map_id} requires an Instance"));
    }
    super::check_map(ctx, map_id, x, y)?;

    let mut matches = crate::helpers::characters_by_name(ctx, &name);
    let mut character = matches
        .next()
        .ok_or_else(|| format!("character {name:?} does not exist or is in Transfer"))?;
    if matches.next().is_some() {
        return Err(format!(
            "character name {name:?} is ambiguous ignoring ASCII case"
        ));
    }
    let guid = character.guid;
    if ctx
        .db
        .game_native_player()
        .character_guid()
        .find(guid)
        .is_some_and(|native| native.teleport.is_some())
    {
        return Err("character is already teleporting".into());
    }
    if crate::taxi::is_in_flight(ctx, guid) {
        return Err("character is on a taxi flight".into());
    }
    if ctx
        .db
        .game_transport_passenger()
        .character_guid()
        .find(guid)
        .is_some()
    {
        return Err("character is aboard a transport".into());
    }

    let entity = ctx.db.game_world_entity().guid().find(guid);
    if let Some(entity) = &entity {
        if entity.player_flags & player_flags::GHOST != 0 {
            return Err("character is a ghost".into());
        }
        if entity.dead {
            return Err("character is dead".into());
        }
        let now_ms = crate::time::now(ctx).to_micros_since_unix_epoch().max(0) as u64 / 1_000;
        if entity.combat_active || entity.combat_until_ms > now_ms {
            return Err("character is in combat".into());
        }
        crate::world::teleport_player(ctx, guid, map_id, 0, x, y, z, o);
    } else {
        if character.online {
            return Err("character is online without a world entity; retry after logout".into());
        }
        if character.pending_ghost {
            return Err("character is a ghost".into());
        }
        character.map_id = map_id;
        character.x = x;
        character.y = y;
        character.z = z;
        character.orientation = o;
        character.zone_id = crate::terrain::zone_id_at(ctx, map_id, x, y).unwrap_or(0);
        character.pending_instance_id = 0;
        ctx.db.game_character().guid().update(character);
    }
    log::info!(
        "operator {} teleport character {guid} {name:?} {} to map {map_id} at ({x}, {y}, {z}) facing {o}",
        ctx.sender(),
        if entity.is_some() { "in world" } else { "offline" },
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_coordinate_and_orientation_must_be_finite() {
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for index in 0..4 {
                let mut pose = [100.0, 100.0, 10.5, 1.25];
                pose[index] = invalid;
                assert_eq!(
                    super::super::check_position(pose[0], pose[1], pose[2], pose[3]),
                    Err("the position and orientation must be finite".to_string())
                );
            }
        }
    }
}
