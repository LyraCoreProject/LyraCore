use lyracore_shared::{constants, packing, spatial};
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
    let (grid_x, grid_y) = spatial::grid_cell(character.x, character.y);
    let mut entity = WorldEntity {
        guid: character.guid,
        owner_identity,
        account_id: character.account_id,
        map_id: character.map_id,
        x: character.x,
        y: character.y,
        z: character.z,
        orientation: character.orientation,
        grid_x,
        grid_y,
        cell: lyracore_shared::spatial::grid_cell_id(grid_x, grid_y),
        last_move_ms: 0,
        type_mask: constants::type_mask::PLAYER,
        entry: 0,
        scale_x: 1.0,
        health: 0,
        max_health: 0,
        power: 0,
        max_power: 0,
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
        base_attack_time_ms: crate::combat::DEFAULT_ATTACK_TIME_MS,
        dynamic_flags: 0,
        dead: false,
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
        target_guid: 0,
        money: character.money,
        bank_bag_slots: character.bank_bag_slots,
        unit_bytes_1: if power_type == packing::power_type::RAGE {
            17u32 << 16
        } else {
            0
        },
        unit_bytes_2: 0,
        strength: 0,
        agility: 0,
        stamina: 0,
        intellect: 0,
        spirit: 0,
        npc_flags: 0,
        armor: 0,
        leg_ends_ms: 0,
        wp_target: 0,
        movement_flags: 0,
        combat_until_ms: 0,
        pickpocketed: false,
        next_swing_spell: 0,
        overpower_until_ms: 0,
        revenge_until_ms: 0,
        stance: 0,
        owner_guid: 0,
        skinned: false,
        mana_regen_paused_until_ms: 0,
        // Retain reclaim escalation across relog so repeated deaths cannot reset the delay.
        death_expire_micros: character.death_expire_micros,

        instance_id: character.pending_instance_id,
        run_speed_mult_bp: character.pending_run_speed_mult_bp,
        godmode: character.pending_godmode,
        resting: character.resting,
        sheet_str_bonus: 0,
        sheet_agi_bonus: 0,
        sheet_sta_bonus: 0,
        sheet_int_bonus: 0,
        sheet_spi_bonus: 0,
        sheet_ap_base: 0,
        sheet_ap_mods: 0,
        sheet_ap_modifiers: 0,
        sheet_ranged_ap_modifiers: 0,
        sheet_ranged_dmg_min_fraction: 0.0,
        sheet_ranged_dmg_max_fraction: 0.0,
        sheet_ranged_crit_bp: 0,
        sheet_dmg_min: 0,
        sheet_dmg_max: 0,
        sheet_crit_bp: 0,
        mount_display_id: 0,
        zone_id: crate::terrain::zone_id_at(ctx, character.map_id, character.x, character.y)
            .unwrap_or(character.zone_id),
        sheet_ranged_ap: 0,
        sheet_ranged_dmg_min: 0,
        sheet_ranged_dmg_max: 0,
        regeneration_pending: false,
        combat_active: false,
        simulation_indexed: false,
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
