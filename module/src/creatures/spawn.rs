use lyracore_shared::{constants, packing, spatial};
use spacetimedb::{reducer, table, Identity, ReducerContext, Table, Timestamp};

#[cfg(feature = "debug_reducers")]
use crate::creatures::tick::game_creature_spline;
use crate::{game_active_taxi_flight, game_race_info, game_world_entity, Character, WorldEntity};

#[table(
    accessor = game_creature_template,
    public,

    index(accessor = by_aggro_range, btree(columns = [aggro_range]))
)]
pub struct CreatureTemplate {
    #[primary_key]
    pub entry: u32,
    pub name: String,
    pub subname: String,
    pub display_id: u32,
    pub level: u32,
    pub health: u32,
    pub faction_template: u32,
    pub npc_flags: u32,
    pub unit_flags: u32,
    pub creature_type: u8,
    pub creature_family: u8,
    pub type_flags: u32,
    pub rank: u8,
    pub scale: f32,
    pub base_attack_time_ms: u32,
    #[default(0)]
    pub money_min: u32,
    #[default(0)]
    pub money_max: u32,

    #[default(0)]
    pub max_level: u32,
    #[default(0)]
    pub max_level_health: u32,

    #[default(0)]
    pub aggro_range: u32,

    #[default(0)]
    pub damage_min: u32,
    #[default(0)]
    pub damage_max: u32,

    #[default(0)]
    pub armor: u32,

    #[default(0u32)]
    pub pickpocket_loot_id: u32,
    #[default(0u32)]
    pub skin_loot_id: u32,

    #[default(0u8)]
    pub trainer_type: u8,
    #[default(0u8)]
    pub trainer_class: u8,
}

#[table(accessor = game_creature_family, public)]
pub struct CreatureFamily {
    #[primary_key]
    pub family_id: u32,
    pub name: String,
    pub pet_food_mask: i32,
    pub pet_talent_type: i32,
    pub category: i32,
}

#[table(accessor = game_gossip_menu, public)]
pub struct GossipMenu {
    #[primary_key]
    pub entry: u32,
    pub text_id: u32,
}

#[table(accessor = game_gossip_menu_profile, public)]
pub struct GossipMenuProfile {
    #[primary_key]
    pub menu_id: u32,
    pub text_id: u32,
}

#[table(
    accessor = game_gossip_menu_profile_option,
    public,
    index(accessor = by_menu, btree(columns = [menu_id]))
)]
pub struct GossipMenuProfileOption {
    #[primary_key]
    pub row_id: u32,
    pub menu_id: u32,
    pub option_index: u32,
    pub icon: u32,
    pub text: String,
    pub action: u32,
    pub action_menu_id: u32,
    pub cond_type: u32,
    pub cond_value1: u32,
    pub cond_value2: u32,
}

#[table(
    accessor = game_creature_gossip_menu_override,
    public,
    index(accessor = by_instance, btree(columns = [instance_id]))
)]
pub struct CreatureGossipMenuOverride {
    #[primary_key]
    pub creature_guid: u64,
    pub menu_id: u32,
    pub map_id: u32,
    pub instance_id: u64,
}

pub(crate) fn set_creature_gossip_menu(
    ctx: &ReducerContext,
    creature_guid: u64,
    menu_id: Option<u32>,
) -> Result<(), String> {
    let table = ctx.db.game_creature_gossip_menu_override();
    let Some(menu_id) = menu_id else {
        table.creature_guid().delete(creature_guid);
        return Ok(());
    };
    let creature = ctx
        .db
        .game_world_entity()
        .guid()
        .find(creature_guid)
        .filter(|entity| !entity.is_player() && !entity.dead)
        .ok_or_else(|| format!("gossip creature {creature_guid} is unavailable"))?;
    if ctx
        .db
        .game_gossip_menu_profile()
        .menu_id()
        .find(menu_id)
        .is_none()
    {
        return Err(format!("gossip menu profile {menu_id} is missing"));
    }
    let row = CreatureGossipMenuOverride {
        creature_guid,
        menu_id,
        map_id: creature.map_id,
        instance_id: creature.instance_id,
    };
    if table.creature_guid().find(creature_guid).is_some() {
        table.creature_guid().update(row);
    } else {
        table.insert(row);
    }
    Ok(())
}

pub(crate) fn clear_gossip_menu_overrides_for_instance(ctx: &ReducerContext, instance_id: u64) {
    let table = ctx.db.game_creature_gossip_menu_override();
    for row in table.by_instance().filter(&instance_id).collect::<Vec<_>>() {
        table.creature_guid().delete(row.creature_guid);
    }
}

#[table(accessor = game_npc_text, public)]
pub struct NpcText {
    #[primary_key]
    pub text_id: u32,
    pub text: String,
}

#[table(accessor = game_npc_text_slot, public, index(accessor = by_text_id, btree(columns = [text_id])))]
pub struct NpcTextSlot {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub text_id: u32,
    pub slot_index: u8,
    pub text_male: String,
    pub text_female: String,
    pub probability: f32,
}

#[table(accessor = game_gossip_option, public, index(accessor = by_entry, btree(columns = [entry])))]
pub struct GossipOption {
    #[primary_key]
    pub row_id: u32,
    pub entry: u32,
    pub option_index: u32,
    pub icon: u32,
    pub text: String,
    pub action: u32,
    pub action_menu_id: u32,
    pub cond_type: u32,
    pub cond_value1: u32,
    pub cond_value2: u32,
}

#[table(accessor = game_creature_cast, public)]
pub struct CreatureCast {
    #[primary_key]
    pub creature_entry: u32,
    pub spell_id: u32,
}

#[table(accessor = game_creature_spell, public, index(accessor = by_entry, btree(columns = [creature_entry])))]
pub struct CreatureSpell {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub creature_entry: u32,
    pub spell_id: u32,
    pub priority: u8,
    pub condition: u8,
    pub condition_value: u8,
}

pub mod cast_condition {
    pub const ALWAYS: u8 = 0;
    pub const SELF_HP_BELOW_PCT: u8 = 1;
    pub const TARGET_MISSING_AURA: u8 = 2;
    pub const SELF_MISSING_AURA: u8 = 3;
}

#[table(accessor = game_creature_waypoint, public, index(accessor = by_creature, btree(columns = [creature_guid])))]
pub struct CreatureWaypoint {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub creature_guid: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

pub const MOVEMENT_IDLE: u8 = 0;
pub const MOVEMENT_RANDOM: u8 = 1;

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_normalize_spawn_timers(ctx: &ReducerContext, limit: u32) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let spawns = ctx.db.game_creature_spawn();
    let entities = ctx.db.game_world_entity();
    let never = timer_never(ctx);
    let cap = if limit == 0 { u32::MAX } else { limit } as usize;

    let mut changed = 0usize;
    let pending: Vec<CreatureSpawn> = spawns
        .iter()
        .filter(|s| s.respawn_at <= ctx.timestamp || s.despawn_at <= ctx.timestamp)
        .take(cap)
        .collect();
    for mut s in pending {
        let entity = entities.guid().find(s.guid);
        let alive = entity.is_some();
        let is_corpse = entity.map(|e| e.dead).unwrap_or(false);
        let mut touched = false;
        if alive && s.respawn_at <= ctx.timestamp {
            s.respawn_at = never;
            touched = true;
        }
        if !is_corpse && s.despawn_at <= ctx.timestamp {
            s.despawn_at = never;
            touched = true;
        }
        if touched {
            spawns.guid().update(s);
            changed += 1;
        }
    }
    spacetimedb::log::info!(
        "debug_normalize_spawn_timers: disarmed {changed} spawn row(s) (limit {limit}); \
         re-run until it reports 0"
    );
    Ok(())
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_retire_region_creatures(
    ctx: &ReducerContext,
    map_id: u32,
    gx_min: i32,
    gx_max: i32,
    gy_min: i32,
    gy_max: i32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let spawns = ctx.db.game_creature_spawn();
    let splines = ctx.db.game_creature_spline();
    let doomed: Vec<u64> = spawns
        .iter()
        .filter(|s| {
            let (gx, gy) = lyracore_shared::spatial::grid_cell(s.x, s.y);
            s.map_id == map_id && gx >= gx_min && gx <= gx_max && gy >= gy_min && gy <= gy_max
        })
        .map(|s| s.guid)
        .collect();
    for g in &doomed {
        crate::WorldEntity::delete(ctx, g);
        splines.guid().delete(g);
        spawns.guid().delete(g);
    }
    if !doomed.is_empty() {
        crate::threat::clear_all(ctx);
    }
    spacetimedb::log::info!(
        "debug_retire_region_creatures: retired {} creature(s) in map {map_id} cells \
         ({gx_min}..{gx_max}, {gy_min}..{gy_max})",
        doomed.len()
    );
    Ok(())
}

pub(crate) fn timer_never(ctx: &spacetimedb::ReducerContext) -> spacetimedb::Timestamp {
    ctx.timestamp
        .checked_add(spacetimedb::TimeDuration::from_micros(
            u32::MAX as i64 * 1_000_000,
        ))
        .unwrap_or(ctx.timestamp)
}

#[table(accessor = game_creature_spawn_map, public)]
pub struct CreatureSpawnMap {
    #[primary_key]
    pub map_id: u32,
}

#[table(
    accessor = game_creature_spawn,
    public,
    index(accessor = by_respawn_at, btree(columns = [respawn_at])),
    index(accessor = by_despawn_at, btree(columns = [despawn_at])),
    index(accessor = by_entry_map, btree(columns = [entry, map_id])),
    index(accessor = by_source_guid, btree(columns = [source_guid])),
    index(accessor = by_instance, btree(columns = [instance_id])),
    index(accessor = by_map_instance, btree(columns = [map_id, instance_id]))
)]
#[derive(Clone)]
pub struct CreatureSpawn {
    #[primary_key]
    pub guid: u64,
    pub entry: u32,
    pub map_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
    pub respawn_at: Timestamp,
    pub despawn_at: Timestamp,
    #[default(0)]
    pub movement_type: u8,

    #[default(0)]
    pub respawn_secs: u32,

    #[default(0u64)]
    pub life_seq: u64,
    #[default(0u64)]
    pub instance_id: u64,
    #[default(0u64)]
    pub source_guid: u64,
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn creature_spawn_evidence(
    ctx: &ReducerContext,
    entry: u32,
    map_id: u32,
    limit: usize,
) -> Vec<CreatureSpawn> {
    const MAX_RESULTS: usize = 128;
    ctx.db
        .game_creature_spawn()
        .by_entry_map()
        .filter((entry, map_id))
        .take(limit.min(MAX_RESULTS))
        .collect()
}

pub(crate) fn current_life_seq(ctx: &spacetimedb::ReducerContext, creature_guid: u64) -> u64 {
    if let Some(spawn) = ctx.db.game_creature_spawn().guid().find(creature_guid) {
        return spawn.life_seq;
    }
    crate::creatures::eventai::summon_life_seq(ctx, creature_guid)
}

pub(crate) fn rolled_creature_stats(
    min_level: u32,
    max_level: u32,
    min_health: u32,
    max_health: u32,
    rand: u32,
) -> (u32, u32) {
    if max_level <= min_level {
        return (min_level, min_health);
    }
    let span = max_level - min_level;
    let level = min_level + rand % (span + 1);
    let health = if max_health > min_health {
        min_health + (max_health - min_health) * (level - min_level) / span
    } else {
        min_health
    };
    (level, health)
}

pub(crate) fn rank_hp_multiplier_permille(rank: u8) -> u32 {
    match rank {
        0 => 1000,
        1 => 1500,
        2 => 2800,
        3 => 4000,
        4 => 1800,
        _ => 1000,
    }
}

pub(crate) fn scale_health_for_rank(base_health: u32, rank: u8) -> u32 {
    let permille = rank_hp_multiplier_permille(rank);
    ((base_health as u64 * permille as u64) / 1000).min(u32::MAX as u64) as u32
}

pub(crate) fn scale_creature_damage_for_level(
    min: u32,
    max: u32,
    template_level: u32,
    live_level: u32,
) -> (u32, u32) {
    if template_level == 0 || live_level == template_level {
        return (min, max);
    }
    let scale = |value: u32| {
        (u64::from(value) * u64::from(live_level) / u64::from(template_level))
            .min(u64::from(u32::MAX)) as u32
    };
    (scale(min).max(1), scale(max).max(1))
}

pub(crate) fn insert_creature_entity(ctx: &spacetimedb::ReducerContext, mut entity: WorldEntity) {
    if let Some(floor) = crate::vmap::floor_z(
        ctx,
        entity.map_id,
        entity.instance_id,
        entity.x,
        entity.y,
        entity.z,
    ) {
        entity.z = entity.z.max(floor);
    }
    let payload = crate::hooks::CreatureSpawnPayload {
        guid: entity.guid,
        entry: entity.entry,
        x: entity.x,
        y: entity.y,
        z: entity.z,
    };
    entity.insert(ctx);
    // Fire after insertion because handlers may resolve the creature by guid.
    crate::hooks::fire_on_creature_spawn(ctx, &payload);
}

pub(crate) fn build_creature_entity(
    ctx: &ReducerContext,
    spawn: &CreatureSpawn,
    tmpl: &CreatureTemplate,
    rand: u32,
    instance_id: u64,
) -> WorldEntity {
    let mut entity = build_creature_from_template(spawn, tmpl, rand, instance_id);
    if let Some(stats) = super::catalogue::level_stats(ctx, tmpl, entity.level) {
        entity.health = stats.health;
        entity.max_health = stats.health;
        entity.armor = stats.armor;
    }
    let (display_id, scale) = super::catalogue::display(ctx, tmpl, ctx.random());
    entity.display_id = display_id;
    entity.native_display_id = display_id;
    entity.scale_x = scale;
    entity
}

pub(crate) fn build_creature_from_template(
    spawn: &CreatureSpawn,
    tmpl: &CreatureTemplate,
    rand: u32,
    instance_id: u64,
) -> WorldEntity {
    let (grid_x, grid_y) = spatial::grid_cell(spawn.x, spawn.y);
    let (level, rolled_health) = rolled_creature_stats(
        tmpl.level,
        tmpl.max_level,
        tmpl.health,
        tmpl.max_level_health,
        rand,
    );
    let health = scale_health_for_rank(rolled_health, tmpl.rank);
    WorldEntity {
        guid: spawn.guid,
        owner_identity: Identity::ZERO,
        account_id: 0,
        map_id: spawn.map_id,
        x: spawn.x,
        y: spawn.y,
        z: spawn.z,
        orientation: spawn.orientation,
        grid_x,
        grid_y,
        cell: lyracore_shared::spatial::grid_cell_id(grid_x, grid_y),
        last_move_ms: 0,
        type_mask: constants::type_mask::CREATURE,
        entry: spawn.entry,
        scale_x: tmpl.scale,
        health,
        max_health: health,
        power: 0,
        max_power: 0,
        level,
        faction_template: tmpl.faction_template,
        unit_bytes_0: 0x0101,
        display_id: tmpl.display_id,
        native_display_id: tmpl.display_id,
        unit_flags: tmpl.unit_flags,
        base_attack_time_ms: tmpl.base_attack_time_ms,
        dynamic_flags: 0,
        dead: false,
        player_bytes: 0,
        player_bytes_2: 0,
        player_bytes_3: 0,
        player_flags: 0,
        xp: 0,
        next_level_xp: 0,
        target_guid: 0,
        money: 0,
        unit_bytes_1: 0,
        unit_bytes_2: 0,
        strength: 0,
        agility: 0,
        stamina: 0,
        intellect: 0,
        spirit: 0,
        npc_flags: tmpl.npc_flags,
        armor: tmpl.armor,
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
        death_expire_micros: 0,
        instance_id,
        run_speed_mult_bp: 10_000,
        godmode: false,
        resting: false,
        sheet_str_bonus: 0,
        sheet_agi_bonus: 0,
        sheet_sta_bonus: 0,
        sheet_int_bonus: 0,
        sheet_spi_bonus: 0,
        sheet_ap_base: 0,
        sheet_ap_mods: 0,
        sheet_dmg_min: 0,
        sheet_dmg_max: 0,
        sheet_crit_bp: 0,
        bank_bag_slots: 0,
        mount_display_id: 0,
        zone_id: 0,
        sheet_ranged_ap: 0,
        sheet_ranged_dmg_min: 0,
        sheet_ranged_dmg_max: 0,
        regeneration_pending: false,
        combat_active: false,
        simulation_indexed: false,
    }
}

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
            character.resting,
        ),
        player_bytes_3: character.gender as u32,
        player_flags: 0,
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
        entity.unit_flags |= lyracore_shared::constants::unit_flags::TAXI_FLIGHT;
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

#[reducer]
pub fn import_creature_spawns(ctx: &ReducerContext, packed: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    super::population::check_definitions(ctx)?;
    super::population::clear_all(ctx);
    let entities = ctx.db.game_world_entity();
    let spawns = ctx.db.game_creature_spawn();

    let creature_guids: Vec<u64> = entities
        .iter()
        .filter(|e| e.type_mask == constants::type_mask::CREATURE)
        .map(|e| e.guid)
        .collect();
    for g in creature_guids {
        crate::creatures::despawn_creature_entity(ctx, g);
    }
    let spawn_guids: Vec<_> = spawns.iter().map(|s| (s.guid, s.source_guid)).collect();
    for (guid, source_guid) in spawn_guids {
        if source_guid != 0 {
            crate::instance::remove_spawn_waypoints(ctx, guid);
        }
        spawns.guid().delete(guid);
    }
    crate::threat::clear_all(ctx);

    let maps = ctx.db.game_creature_spawn_map();
    for map in maps.iter().collect::<Vec<_>>() {
        maps.map_id().delete(map.map_id);
    }

    if load_spawn_batch(ctx, &packed)? == 0 {
        return Err("import payload was empty".to_string());
    }
    Ok(())
}

#[reducer]
pub fn import_creature_spawns_append(ctx: &ReducerContext, packed: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    load_spawn_batch(ctx, &packed)?;
    Ok(())
}

fn load_spawn_batch(ctx: &ReducerContext, packed: &str) -> Result<u32, String> {
    let spawns = ctx.db.game_creature_spawn();
    let now = ctx.timestamp;
    let mut loaded = 0u32;
    for row in packed.split(';').filter(|r| !r.is_empty()) {
        let f: Vec<&str> = row.split(',').collect();
        if f.len() != 8 && f.len() != 9 {
            return Err(format!(
                "import row needs 8 or 9 fields, got {}: {row}",
                f.len()
            ));
        }
        let pu64 = |s: &str| s.parse::<u64>().map_err(|_| format!("bad u64: {s}"));
        let pu32 = |s: &str| s.parse::<u32>().map_err(|_| format!("bad u32: {s}"));
        let pu8 = |s: &str| s.parse::<u8>().map_err(|_| format!("bad u8: {s}"));
        let pf32 = |s: &str| s.parse::<f32>().map_err(|_| format!("bad f32: {s}"));
        let map_id = pu32(f[2])?;
        let spawn = CreatureSpawn {
            guid: pu64(f[0])?,
            entry: pu32(f[1])?,
            map_id,
            x: pf32(f[3])?,
            y: pf32(f[4])?,
            z: pf32(f[5])?,
            orientation: pf32(f[6])?,
            respawn_at: if lyracore_shared::instance::has_instance_population(map_id) {
                timer_never(ctx)
            } else {
                now
            },
            despawn_at: timer_never(ctx),
            movement_type: pu8(f[7])?,
            respawn_secs: if f.len() == 9 { pu32(f[8])? } else { 0 },
            life_seq: 0,
            source_guid: 0,
            instance_id: 0,
        };
        spawns
            .try_insert(spawn)
            .map_err(|e| format!("duplicate spawn guid {}: {e}", f[0]))?;
        let maps = ctx.db.game_creature_spawn_map();
        if maps.map_id().find(map_id).is_none() {
            maps.insert(CreatureSpawnMap { map_id });
        }
        loaded += 1;
    }
    Ok(loaded)
}
