use lyracore_shared::{constants, packing};
use spacetimedb::{Identity, ReducerContext};

use crate::{game_active_taxi_flight, game_race_info, Character, WorldEntity};

pub(crate) fn build_player_entity(
    ctx: &ReducerContext,
    character: &Character,
    owner_identity: Identity,
) -> WorldEntity {
    let level = character.level as u32;
    let power_type = packing::power_type::for_class(character.class);
    let race_info = ctx.db.game_race_info().race().find(character.race);
    let display = match &race_info {
        Some(ri) if character.gender != 0 => ri.female_display,
        Some(ri) => ri.male_display,
        None => 49,
    };
    let faction = race_info
        .as_ref()
        .map(|ri| ri.faction_template)
        .unwrap_or(1);
    let mut entity = WorldEntity {
        owner_identity,
        account_id: character.account_id,
        type_mask: constants::type_mask::PLAYER,
        level,
        faction_template: faction,
        unit_bytes_0: packing::unit_bytes_0(
            character.race,
            character.class,
            character.gender,
            power_type,
        ),
        display_id: display,
        native_display_id: display,
        unit_flags: constants::unit_flags::PLAYER_CONTROLLED,
        player_bytes: packing::player_bytes(
            character.skin,
            character.face,
            character.hair_style,
            character.hair_color,
        ),
        player_bytes_2: packing::player_bytes_2_with_rest(
            character.facial_hair,
            character.bank_bag_slots,
            crate::rest::rest_state_after_bonus(
                character.rest_state,
                character.rested_xp,
                character.level,
            ) == packing::REST_STATE_RESTED,
        ),
        player_bytes_3: character.gender as u32,
        player_flags: if character.resting {
            lyracore_shared::constants::player_flags::RESTING
        } else {
            0
        },
        xp: character.xp,
        next_level_xp: character.next_level_xp,
        money: character.money,
        bank_bag_slots: character.bank_bag_slots,
        unit_bytes_1: if power_type == packing::power_type::RAGE {
            17u32 << 16
        } else {
            0
        },
        // Retain reclaim escalation across relog so repeated deaths cannot reset the delay.
        death_expire_micros: character.death_expire_micros,
        run_speed_mult_bp: character.pending_run_speed_mult_bp,
        godmode: character.pending_godmode,
        resting: character.resting,
        zone_id: crate::terrain::zone_id_at(ctx, character.map_id, character.x, character.y)
            .unwrap_or(character.zone_id),
        ..WorldEntity::blank(
            character.guid,
            character.map_id,
            character.pending_instance_id,
            [character.x, character.y, character.z, character.orientation],
        )
    };
    if let Some(flight) = ctx
        .db
        .game_active_taxi_flight()
        .character_guid()
        .find(character.guid)
    {
        entity.mount_display_id = flight.mount_display_id;
        entity.unit_flags |= lyracore_shared::constants::unit_flags::TAXI_FLIGHT
            | lyracore_shared::constants::unit_flags::REMOVE_CLIENT_CONTROL;
    }
    crate::stats::apply_level_stats(ctx, &mut entity, character.race, character.class, level);
    // Persisted health zero means uninitialized; ghosts retain their separate death state.
    entity.health = if character.health == 0 {
        entity.max_health
    } else {
        character.health.clamp(1, entity.max_health)
    };
    entity.power = if character.health != 0 && power_type == packing::power_type::MANA {
        character.power.min(entity.max_power)
    } else {
        crate::stats::starting_power(power_type, entity.max_power)
    };
    entity
}

#[cfg(test)]
mod tests {
    use lyracore_shared::constants::unit_flags;

    #[test]
    fn player_controlled_flag_matches_the_wire_value() {
        assert_eq!(unit_flags::PLAYER_CONTROLLED, 0x0000_0008);
    }

    #[test]
    fn combat_and_taxi_flag_transitions_retain_player_controlled() {
        let entered_combat = unit_flags::PLAYER_CONTROLLED | unit_flags::IN_COMBAT;
        let in_taxi = entered_combat | unit_flags::TAXI_FLIGHT;
        let left_combat = in_taxi & !unit_flags::IN_COMBAT;
        let landed = left_combat & !unit_flags::TAXI_FLIGHT;
        assert_ne!(landed & unit_flags::PLAYER_CONTROLLED, 0);
    }
}
