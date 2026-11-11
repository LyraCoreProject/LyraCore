use lyracore_shared::{constants, spatial};
use spacetimedb::{reducer, table, Identity, ReducerContext, Table, Timestamp};

use crate::gossip::{
    game_creature_gossip_menu_override, game_gossip_menu_profile, CreatureGossipMenuOverride,
};
use crate::{game_world_entity, WorldEntity};

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

pub(crate) fn set_creature_gossip_menu(
    ctx: &ReducerContext,
    creature_guid: u64,
    menu_id: u32,
) -> Result<(), String> {
    let table = ctx.db.game_creature_gossip_menu_override();
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

pub(crate) fn clear_gossip_menu_override(ctx: &ReducerContext, creature_guid: u64) {
    ctx.db
        .game_creature_gossip_menu_override()
        .creature_guid()
        .delete(creature_guid);
}

pub(crate) fn clear_gossip_menu_overrides_for_instance(ctx: &ReducerContext, instance_id: u64) {
    let table = ctx.db.game_creature_gossip_menu_override();
    for row in table.by_instance().filter(&instance_id).collect::<Vec<_>>() {
        table.creature_guid().delete(row.creature_guid);
    }
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
// The importer keeps the source's waypoint movement type; only MOVEMENT_RANDOM wanders.
pub const MOVEMENT_WAYPOINT: u8 = 2;

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_normalize_spawn_timers(ctx: &ReducerContext, limit: u32) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let spawns = ctx.db.game_creature_spawn();
    let entities = ctx.db.game_world_entity();
    let never = crate::time::timer_never(ctx);
    let cap = if limit == 0 { u32::MAX } else { limit } as usize;

    let mut changed = 0usize;
    let pending: Vec<CreatureSpawn> = spawns
        .iter()
        .filter(|s| s.respawn_at <= crate::time::now(ctx) || s.despawn_at <= crate::time::now(ctx))
        .take(cap)
        .collect();
    for mut s in pending {
        let entity = entities.guid().find(s.guid);
        let alive = entity.is_some();
        let is_corpse = entity.map(|e| e.dead).unwrap_or(false);
        let mut touched = false;
        if alive && s.respawn_at <= crate::time::now(ctx) {
            s.respawn_at = never;
            touched = true;
        }
        if !is_corpse && s.despawn_at <= crate::time::now(ctx) {
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

impl CreatureSpawn {
    /// A spawn that stands idle and that neither respawn nor decay timers ever pick up.
    pub(crate) fn temporary(
        ctx: &ReducerContext,
        guid: u64,
        entry: u32,
        map_id: u32,
        instance_id: u64,
        [x, y, z, orientation]: [f32; 4],
    ) -> Self {
        CreatureSpawn {
            guid,
            entry,
            map_id,
            x,
            y,
            z,
            orientation,
            respawn_at: crate::time::timer_never(ctx),
            despawn_at: crate::time::timer_never(ctx),
            movement_type: MOVEMENT_IDLE,
            respawn_secs: 0,
            life_seq: 0,
            instance_id,
            source_guid: 0,
        }
    }
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

fn rank_hp_multiplier_permille(rank: u8) -> u32 {
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
        sheet_ap_modifiers: 0,
        sheet_ranged_ap_modifiers: 0,
        sheet_ranged_dmg_min_fraction: 0.0,
        sheet_ranged_dmg_max_fraction: 0.0,
        sheet_ranged_crit_bp: 0,
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

#[cfg(test)]
mod tests {
    use super::{
        rank_hp_multiplier_permille, rolled_creature_stats, scale_creature_damage_for_level,
        scale_health_for_rank,
    };

    #[test]
    fn creature_damage_scales_from_template_to_live_level_without_overflow() {
        assert_eq!(scale_creature_damage_for_level(5, 10, 10, 10), (5, 10));
        assert_eq!(scale_creature_damage_for_level(5, 10, 10, 12), (6, 12));
        assert_eq!(
            scale_creature_damage_for_level(u32::MAX, u32::MAX, 1, 60),
            (u32::MAX, u32::MAX)
        );
    }

    #[test]
    fn level_roll_respects_range_and_interpolates_health() {
        assert_eq!(rolled_creature_stats(3, 0, 42, 0, 12345), (3, 42));
        assert_eq!(rolled_creature_stats(3, 3, 42, 60, 999), (3, 42));
        for rand in 0..50u32 {
            let (lvl, _) = rolled_creature_stats(1, 3, 40, 80, rand);
            assert!(
                (1..=3).contains(&lvl),
                "level {lvl} out of [1,3] for rand {rand}"
            );
        }
        assert_eq!(rolled_creature_stats(1, 3, 40, 80, 0).0, 1);
        assert_eq!(rolled_creature_stats(1, 3, 40, 80, 2).0, 3);
        assert_eq!(rolled_creature_stats(1, 3, 40, 80, 0), (1, 40));
        assert_eq!(rolled_creature_stats(1, 3, 40, 80, 1), (2, 60));
        assert_eq!(rolled_creature_stats(1, 3, 40, 80, 2), (3, 80));
        assert_eq!(rolled_creature_stats(1, 3, 50, 0, 2), (3, 50));
    }

    #[test]
    fn rank_scales_health_and_rank0_is_identity() {
        for base in [0u32, 1, 42, 1000, 250_000, u32::MAX] {
            assert_eq!(
                scale_health_for_rank(base, 0),
                base,
                "rank-0 must be identity for base {base}"
            );
        }
        assert_eq!(scale_health_for_rank(100, 1), 150);
        assert_eq!(scale_health_for_rank(100, 2), 280);
        assert_eq!(scale_health_for_rank(100, 3), 400);
        assert_eq!(scale_health_for_rank(100, 4), 180);
        assert_eq!(scale_health_for_rank(42, 1), 63);
        assert_eq!(scale_health_for_rank(100, 7), 100);
        assert_eq!(scale_health_for_rank(100, 255), 100);
        assert_eq!(rank_hp_multiplier_permille(0), 1000);
        assert!(rank_hp_multiplier_permille(1) > rank_hp_multiplier_permille(0));
        assert!(rank_hp_multiplier_permille(3) > rank_hp_multiplier_permille(1));
        assert_eq!(scale_health_for_rank(u32::MAX, 1), u32::MAX);
        assert_eq!(scale_health_for_rank(u32::MAX, 2), u32::MAX);
        assert_eq!(scale_health_for_rank(u32::MAX, 3), u32::MAX);
        assert_eq!(scale_health_for_rank(u32::MAX, 4), u32::MAX);
        assert_eq!(scale_health_for_rank(2_000_000_000, 1), 3_000_000_000);
        assert_eq!(scale_health_for_rank(1_000_000_000, 2), 2_800_000_000);
        assert_eq!(scale_health_for_rank(1_000_000_000, 3), 4_000_000_000);
        assert_eq!(scale_health_for_rank(2_000_000_000, 4), 3_600_000_000);
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
        crate::items::vendor_stock::forget(ctx, guid);
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
    let now = crate::time::now(ctx);
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
                crate::time::timer_never(ctx)
            } else {
                now
            },
            despawn_at: crate::time::timer_never(ctx),
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
